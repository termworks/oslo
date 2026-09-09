//! The day-one verbs: `to`, `cols`, `get`, `sort-by`, `first`, `final`, `length`, `each`.
//!
//! ```text
//! df | sort-by free | first 3
//! ps | where 'cpu > 10' | cols pid name | to json | jq .
//! ls | each 'print(name)'
//! ```
//!
//! **`to json` is not optional.** Without a way out, the structured world is a walled garden the
//! moment somebody wants `jq`, `curl` or a file — and a shell that can only talk to itself has
//! replaced a universal interface with a private one. It is the reason this stage exists at all;
//! the rest are a day's work each once records exist.
//!
//! # Why `cols` and not `select`
//!
//! `select` is a bash keyword, and oslo's parser refuses it as one. Anybody arriving from nushell
//! will reach for `select` by reflex, so the name is worth stating plainly rather than discovering:
//! it is `cols`.

use crate::data::path::Path;
use crate::data::{Record, Val, render_display, render_transport};

/// Keep only the named columns, in the order they were asked for.
///
/// A name may be a path — `cols Id 'State.Running'` — and the column it produces is named by the
/// path as written, so `cols a.b | get a.b` finds what this made.
pub fn cols(rows: &[Record], names: &[String]) -> Vec<Record> {
    let paths: Vec<Path> = names.iter().map(|name| Path::parse(name)).collect();
    rows.iter()
        .map(|row| {
            let mut out = Record::new();
            for path in &paths {
                // A column that is not there is simply absent, not an error: rows are allowed to
                // disagree about their columns, so asking for one that only some rows have is a
                // reasonable thing to do. The stream-wide check in `tools::unknown_column` is what
                // catches a name *no* row has, which is the typo.
                if let Some(value) = path.get_or_absent(row) {
                    out.set(path.literal(), value.clone());
                }
            }
            out
        })
        .collect()
}

/// One column's values, as a list of one-column rows.
///
/// Kept as rows rather than bare scalars so that everything downstream can still assume it is
/// looking at rows — one shape, not two.
pub fn get(rows: &[Record], name: &str) -> Vec<Record> {
    let path = Path::parse(name);
    rows.iter()
        .filter_map(|row| {
            path.get_or_absent(row)
                .map(|value| Record::from_pairs([(path.literal(), value.clone())]))
        })
        .collect()
}

/// How a sort was asked for.
///
/// Flags rather than verbs, because `sort-by -r` costs no name and `sort-desc` would cost one —
/// and the name budget is what the whole structured half is rationed by.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SortOptions {
    pub reverse: bool,
    /// `file2` before `file10`: digit runs compare as numbers even inside text.
    pub natural: bool,
    pub ignore_case: bool,
}

/// Sort by one or more columns, ascending unless told otherwise.
///
/// Numbers compare as numbers and text as text — a size sorts by its bytes, which is the whole
/// reason `Val::Size` exists rather than the string `4.2G`.
///
/// **Keys are tried in order**, and the sort is stable, so `sort-by dept name` orders by department
/// and settles ties by name. A row missing a key sorts as the empty value rather than being dropped:
/// rows are allowed to disagree about their columns, and refusing the whole stream over one gap
/// would make `sort-by` unusable on exactly the ragged data it is most wanted for.
pub fn sort_by(rows: &[Record], keys: &[String], options: SortOptions) -> Vec<Record> {
    let paths: Vec<Path> = keys.iter().map(|key| Path::parse(key)).collect();
    let mut out = rows.to_vec();
    out.sort_by(|a, b| {
        let mut order = std::cmp::Ordering::Equal;
        for path in &paths {
            order = compare(path.get_or_absent(a), path.get_or_absent(b), options);
            if order != std::cmp::Ordering::Equal {
                break;
            }
        }
        match options.reverse {
            true => order.reverse(),
            false => order,
        }
    });
    out
}

/// The rows in the opposite order.
///
/// Not the same as `sort-by -r`, which orders by a key: this reverses whatever order the stream
/// already has, which is the only way to undo a `group-by`'s first-seen order or to read a log
/// backwards.
pub fn reverse(rows: &[Record]) -> Vec<Record> {
    rows.iter().rev().cloned().collect()
}

fn compare(a: Option<&Val>, b: Option<&Val>, options: SortOptions) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (numeric(a), numeric(b)) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
        _ => {
            let (x, y) = (text(a), text(b));
            let (x, y) = match options.ignore_case {
                true => (x.to_lowercase(), y.to_lowercase()),
                false => (x, y),
            };
            match options.natural {
                true => natural(&x, &y),
                false => x.cmp(&y),
            }
        }
    }
}

/// Compare text with digit runs read as numbers: `file2` before `file10`.
///
/// Plain `cmp` puts `file10` first because `1` sorts below `2`, which is the single most complained
/// about thing about sorting filenames. Leading zeros do not change the value, so `file02` and
/// `file2` compare equal on the number and fall back to the text — otherwise the order between them
/// would depend on which happened to be scanned first.
fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(p), Some(q)) if p.is_ascii_digit() && q.is_ascii_digit() => {
                let left = digits(&mut x);
                let right = digits(&mut y);
                // Compare as numbers of unbounded length, so a run longer than u64 does not wrap.
                let trimmed = |s: &str| s.trim_start_matches('0').to_string();
                let (l, r) = (trimmed(&left), trimmed(&right));
                match l.len().cmp(&r.len()).then_with(|| l.cmp(&r)) {
                    Ordering::Equal => {}
                    other => return other,
                }
            }
            (Some(p), Some(q)) => {
                if p != q {
                    return p.cmp(&q);
                }
                x.next();
                y.next();
            }
        }
    }
}

fn digits(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut run = String::new();
    while chars.peek().is_some_and(char::is_ascii_digit) {
        run.push(chars.next().unwrap_or_default());
    }
    run
}

fn numeric(value: Option<&Val>) -> Option<f64> {
    match value? {
        Val::Int(i) => Some(*i as f64),
        Val::Float(f) => Some(*f),
        Val::Size(b) => Some(*b as f64),
        Val::Duration(n) | Val::Time(n) => Some(*n as f64),
        Val::Bool(b) => Some(u8::from(*b) as f64),
        _ => None,
    }
}

fn text(value: Option<&Val>) -> String {
    value.map(render_transport).unwrap_or_default()
}

/// The first or the closing `n` rows.
pub fn first(rows: &[Record], n: usize) -> Vec<Record> {
    rows.iter().take(n).cloned().collect()
}

/// The verb is `final`; the function cannot be, because `final` is a reserved word in Rust.
///
/// **It was `last`, which is a command util-linux ships.** Every name that can carry structure has
/// to be one oslo invented, or `<a rows producer> | last` quietly stops meaning what a script said
/// — the same defect `uniq` had, found in the same sweep.
pub fn final_rows(rows: &[Record], n: usize) -> Vec<Record> {
    let skip = rows.len().saturating_sub(n);
    rows.iter().skip(skip).cloned().collect()
}

/// How many rows there are, as a single row so the shape stays the same.
pub fn length(rows: &[Record]) -> Vec<Record> {
    vec![Record::from_pairs([(
        "length",
        Val::Int(rows.len() as i64),
    )])]
}

/// The bytes a `to` conversion produces.
pub fn to_format(rows: &[Record], format: &str) -> Result<String, String> {
    match format {
        "json" => Ok(to_json(rows)),
        // The plain, complete rendering — the same one a pipe would have received anyway. Named so
        // a script can ask for it rather than depending on where its output happens to go.
        "text" => Ok(render_transport(&Val::table(rows.to_vec()))),
        // The human rendering, for a script that wants the table a person would see.
        "table" => Ok(render_display(&Val::table(rows.to_vec()))),
        // For somebody else's program, so a header row and RFC 4180 quoting — where `text` is
        // oslo's own transport and escapes instead. Two audiences, two formats.
        // The closing newline comes off and nothing else does. `trim_end` took the last *field*
        // with it whenever that field ended in a space, so a value survived every row but the last.
        "csv" | "tsv" => {
            let text = super::formats::to_delimited(
                rows,
                super::formats::delimiter(format).unwrap_or(','),
            );
            Ok(text.strip_suffix('\n').unwrap_or(&text).to_string())
        }
        other => Err(format!(
            "to: {other}: unknown format; oslo knows json, csv, tsv, text and table"
        )),
    }
}

/// Rows as JSON, **with the columns in the order the record has them**.
///
/// Written out by hand rather than through `serde_json::Map`, whose ordering is alphabetical: a
/// record is ordered on purpose — that is what the insertion-ordered table work was for — and
/// `df | to json` reordering the columns would throw it away at the last step. Only the values go
/// through serde, which is where the escaping rules live and where hand-rolling would be a bug.
fn to_json(rows: &[Record]) -> String {
    let mut out = String::from("[\n");
    for (i, row) in rows.iter().enumerate() {
        if i > 0 {
            out.push_str(",\n");
        }
        out.push_str("  {\n");
        for (j, (name, value)) in row.columns().iter().zip(row.values()).enumerate() {
            if j > 0 {
                out.push_str(",\n");
            }
            let key = serde_json::Value::String(name.clone());
            let rendered =
                serde_json::to_string(&json_of(value)).unwrap_or_else(|_| "null".to_string());
            out.push_str(&format!("    {key}: {rendered}"));
        }
        out.push_str("\n  }");
    }
    out.push_str("\n]");
    out
}

fn json_of_record(row: &Record) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (name, value) in row.columns().iter().zip(row.values()) {
        map.insert(name.clone(), json_of(value));
    }
    serde_json::Value::Object(map)
}

fn json_of(value: &Val) -> serde_json::Value {
    match value {
        Val::Null => serde_json::Value::Null,
        Val::Bool(b) => serde_json::Value::Bool(*b),
        Val::Int(i) => serde_json::Value::from(*i),
        Val::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        // A size goes out as its number of bytes, not as `4.2G`: whatever reads this JSON will do
        // arithmetic on it, and `4.2G` is not a number.
        Val::Size(b) => serde_json::Value::from(*b),
        Val::Duration(n) | Val::Time(n) => serde_json::Value::from(*n),
        Val::Str(s) => serde_json::Value::String(s.clone()),
        Val::Bytes(b) => serde_json::Value::from(b.len()),
        // An error is reported as an object rather than dropped, so the reader can see that this
        // one cell failed while the rest of the row is good.
        Val::Error(message) => {
            let mut map = serde_json::Map::new();
            map.insert(
                "error".to_string(),
                serde_json::Value::String(message.clone()),
            );
            serde_json::Value::Object(map)
        }
        Val::List(items) => serde_json::Value::Array(items.iter().map(json_of).collect()),
        Val::Record(record) => json_of_record(record),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<Record> {
        vec![
            Record::from_pairs([
                ("name", Val::Str("b".into())),
                ("size", Val::Size(2048)),
                ("n", Val::Int(2)),
            ]),
            Record::from_pairs([
                ("name", Val::Str("a".into())),
                ("size", Val::Size(512)),
                ("n", Val::Int(1)),
            ]),
        ]
    }

    /// Columns come back in the order they were asked for, not the order they were stored.
    #[test]
    fn cols_keeps_the_order_requested() {
        let picked = cols(&rows(), &["n".to_string(), "name".to_string()]);
        assert_eq!(picked[0].columns(), ["n", "name"]);
        assert_eq!(picked[0].get("name"), Some(&Val::Str("b".into())));
    }

    /// A size sorts by its bytes. This is the whole argument for a tagged scalar: `4.2G` and
    /// `900M` sort the wrong way round as text.
    #[test]
    fn sorting_a_size_is_numeric() {
        let sorted = sort_by(&rows(), &["size".to_string()], SortOptions::default());
        assert_eq!(sorted[0].get("size"), Some(&Val::Size(512)));
        assert_eq!(sorted[1].get("size"), Some(&Val::Size(2048)));
    }

    fn named(names: &[&str]) -> Vec<Record> {
        names
            .iter()
            .map(|n| Record::from_pairs([("name", Val::Str((*n).into()))]))
            .collect()
    }

    fn names_of(rows: &[Record]) -> Vec<String> {
        rows.iter()
            .map(|r| match r.get("name") {
                Some(Val::Str(s)) => s.clone(),
                other => format!("{other:?}"),
            })
            .collect()
    }

    fn by_name(rows: &[Record], options: SortOptions) -> Vec<String> {
        names_of(&sort_by(rows, &["name".to_string()], options))
    }

    /// `-n`: a digit run inside text compares as the number it is, so `file2` precedes `file10` —
    /// which plain text ordering gets backwards, and is the most complained about thing about
    /// sorting filenames.
    #[test]
    fn a_natural_sort_reads_digit_runs_as_numbers() {
        let rows = named(&["file10", "file2", "file1"]);
        assert_eq!(
            by_name(
                &rows,
                SortOptions {
                    natural: true,
                    ..SortOptions::default()
                }
            ),
            ["file1", "file2", "file10"]
        );
        assert_eq!(
            by_name(&rows, SortOptions::default()),
            ["file1", "file10", "file2"],
            "and plain ordering still puts file10 second"
        );
    }

    /// Leading zeros do not change the number, so the text decides between them rather than the
    /// order they happened to arrive in.
    #[test]
    fn leading_zeros_do_not_change_the_number() {
        let natural = SortOptions {
            natural: true,
            ..SortOptions::default()
        };
        assert_eq!(
            by_name(&named(&["file2", "file02", "file1"]), natural),
            ["file1", "file02", "file2"]
        );
    }

    /// `-r` reverses the ordering; `reverse` reverses the stream. They are not the same thing, and
    /// that is why both exist.
    #[test]
    fn reverse_the_order_and_reverse_the_stream_differ() {
        let rows = named(&["b", "a", "c"]);
        assert_eq!(
            by_name(
                &rows,
                SortOptions {
                    reverse: true,
                    ..SortOptions::default()
                }
            ),
            ["c", "b", "a"],
            "-r orders by the key, descending"
        );
        assert_eq!(
            names_of(&reverse(&rows)),
            ["c", "a", "b"],
            "reverse turns the stream round, whatever order it had"
        );
    }

    /// `-i` folds case, and the sort stays stable so equal keys keep their arrival order.
    #[test]
    fn ignoring_case_is_stable() {
        let rows = named(&["b", "A", "a", "B"]);
        assert_eq!(
            by_name(
                &rows,
                SortOptions {
                    ignore_case: true,
                    ..SortOptions::default()
                }
            ),
            ["A", "a", "b", "B"]
        );
    }

    /// Keys are tried in order and the sort is stable, so the second settles ties in the first.
    #[test]
    fn several_keys_break_ties_in_order() {
        let rows = vec![
            Record::from_pairs([("d", Val::Int(2)), ("n", Val::Str("b".into()))]),
            Record::from_pairs([("d", Val::Int(1)), ("n", Val::Str("z".into()))]),
            Record::from_pairs([("d", Val::Int(2)), ("n", Val::Str("a".into()))]),
        ];
        let keys = ["d".to_string(), "n".to_string()];
        let sorted = sort_by(&rows, &keys, SortOptions::default());
        let seen: Vec<String> = sorted
            .iter()
            .map(|r| {
                format!(
                    "{}{}",
                    render_transport(r.get("d").unwrap()),
                    match r.get("n") {
                        Some(Val::Str(s)) => s.clone(),
                        _ => String::new(),
                    }
                )
            })
            .collect();
        assert_eq!(seen, ["1z", "2a", "2b"]);
    }

    /// Text sorts as text.
    #[test]
    fn sorting_text_is_alphabetical() {
        let sorted = sort_by(&rows(), &["name".to_string()], SortOptions::default());
        assert_eq!(sorted[0].get("name"), Some(&Val::Str("a".into())));
    }

    /// The ends of a table, and its size, all still rows.
    #[test]
    fn first_final_and_length_keep_the_shape() {
        assert_eq!(first(&rows(), 1).len(), 1);
        assert_eq!(
            final_rows(&rows(), 1)[0].get("name"),
            Some(&Val::Str("a".into()))
        );
        assert_eq!(length(&rows())[0].get("length"), Some(&Val::Int(2)));
        assert_eq!(first(&rows(), 99).len(), 2, "asking for more than there is");
    }

    /// **The exit door.** A size leaves as a number, because whatever reads this will compute with
    /// it — and the output is real JSON that `jq` can read.
    #[test]
    fn to_json_is_json_and_sizes_are_numbers() {
        let json = to_format(&rows(), "json").expect("json");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        let first = &parsed[0];
        assert_eq!(first["size"], serde_json::Value::from(2048));
        assert_eq!(first["name"], serde_json::Value::from("b"));
    }

    /// The columns come out in the record's order, not sorted.
    ///
    /// A record is ordered deliberately — the whole insertion-ordered table change was for this —
    /// and serialising through a sorted map would throw it away at the very last step.
    #[test]
    fn to_json_keeps_the_column_order() {
        let json = to_format(&rows(), "json").expect("json");
        let name_at = json.find("\"name\"").expect("name");
        let size_at = json.find("\"size\"").expect("size");
        let n_at = json.find("\"n\":").expect("n");
        assert!(
            name_at < size_at && size_at < n_at,
            "columns must keep record order, got:\n{json}"
        );
    }

    /// An unknown format is refused by name rather than producing nothing.
    #[test]
    fn an_unknown_format_is_refused() {
        assert!(to_format(&rows(), "yaml").is_err());
    }

    /// Only the closing newline comes off the end of a CSV.
    ///
    /// `trim_end` took the trailing spaces of the last row's last field with it, so a value
    /// survived every row but the final one — invisible in a test that calls `to_delimited`
    /// directly, because the trimming lives here rather than there.
    #[test]
    fn the_last_field_keeps_its_spaces() {
        let rows = vec![
            Record::from_pairs([("v", Val::Str("aa  ".into()))]),
            Record::from_pairs([("v", Val::Str("bb  ".into()))]),
        ];
        let csv = to_format(&rows, "csv").expect("csv");
        assert_eq!(csv.lines().count(), 3, "header and two rows: {csv:?}");
        assert_eq!(csv.lines().nth(2), Some("\"bb  \""), "{csv:?}");
        assert!(!csv.ends_with('\n'), "the closing newline still comes off");
    }
}
