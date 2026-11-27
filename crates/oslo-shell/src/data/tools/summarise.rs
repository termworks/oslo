//! The verbs that make a stream *smaller*: `group-by`, `count`, `distinct`, `stats`.
//!
//! ```text
//! ps | group-by user | count
//! ls | distinct kind
//! df | stats free
//! ```
//!
//! # Why these are the ones worth adding
//!
//! Every verb oslo had was **selection** — `where`, `cols`, `first`, `sort-by`. Selection keeps rows
//! and throws rows away; none of it can answer *how many*, *which distinct*, or *how much in total*.
//! That is the difference between rows being a nicer `awk` and being a different tool: `ps |
//! group-by user | count` is a query `ps | grep` cannot express at all.
//!
//! **In Rust, though a Lua tool could now do it.** These are what every pipeline reaches for, so
//! they have to exist in `oslo-minimal` and be fast enough that nobody drops back to
//! `sort | uniq -c`.
//!
//! # `join` is not here
//!
//! It needs a *second* input stream, and oslo's pipeline is a line — there is no shape for "and also
//! read this". Adding one is a change to the pipeline, not another verb, and pretending otherwise
//! would mean a `join` that reads its other side from somewhere the planner cannot see.

use crate::data::path::Path;
use crate::data::{Record, Val, render_transport};

/// The column a row is grouped or counted by, rendered so two values group together exactly when
/// they are the same value.
///
/// `render_transport` rather than `render_display`: a `Size` displays as `4.2G` at two different
/// byte counts, and grouping on what something *looks like* is a bug that only shows up on large
/// numbers.
fn key_of(row: &Record, field: &Path) -> Option<String> {
    field.get_or_absent(row).map(render_transport)
}

/// One row per distinct value of `field`, each carrying how many had it and the rows themselves.
///
/// **Order is first-seen, not sorted.** A pipeline that wants order says `sort-by`, and a group-by
/// that sorted would quietly undo an earlier `sort-by` — which is the kind of thing that is only
/// noticed once the output is already wrong.
pub fn group_by(rows: &[Record], field: &str) -> Vec<Record> {
    let field = &Path::parse(field);
    let mut order: Vec<String> = Vec::new();
    let mut groups: std::collections::HashMap<String, (Val, Vec<Record>)> =
        std::collections::HashMap::new();
    for row in rows {
        // A row without the column is not in any group. Dropping it beats inventing an empty group
        // that a later `count` would report as real.
        let (Some(key), Some(value)) = (key_of(row, field), field.get_or_absent(row)) else {
            continue;
        };
        if !groups.contains_key(&key) {
            order.push(key.clone());
            groups.insert(key.clone(), (value.clone(), Vec::new()));
        }
        if let Some((_, kept)) = groups.get_mut(&key) {
            kept.push(row.clone());
        }
    }
    order
        .into_iter()
        .filter_map(|key| {
            let (value, kept) = groups.remove(&key)?;
            let mut out = Record::new();
            out.set(field.literal(), value);
            out.set("count", Val::Int(kept.len() as i64));
            // The rows themselves, so `group-by` composes with everything after it rather than
            // being a dead end that only `count` can follow.
            out.set("rows", Val::table(kept));
            Some(out)
        })
        .collect()
}

/// How many rows — or, after `group-by`, how many in each group.
///
/// **It notices what it was handed.** A stream that already has a `count` column is a grouped one,
/// and counting *those* rows would answer "how many groups" to somebody who asked "how many in
/// each", which is the reading that makes `group-by | count` useless.
pub fn count(rows: &[Record]) -> Vec<Record> {
    let grouped = rows.iter().all(|row| row.get("count").is_some());
    if grouped && !rows.is_empty() {
        return rows
            .iter()
            .map(|row| {
                let mut out = Record::new();
                for (name, value) in row.columns().iter().zip(row.values()) {
                    // The rows are dropped: `count` is the summary, and carrying every row through
                    // it would make `| count` the most expensive way to ask for a number.
                    if name != "rows" {
                        out.set(name, value.clone());
                    }
                }
                out
            })
            .collect();
    }
    vec![Record::from_pairs([("count", Val::Int(rows.len() as i64))])]
}

/// Distinct rows, or rows distinct by one field.
///
/// Keeps the **first** of each, so `sort-by name | distinct kind` gives the alphabetically first of
/// each kind rather than an arbitrary one.
///
/// **Named `distinct` rather than `uniq`, and that is not a style choice.** Every name that can
/// carry structure has to be one oslo invented, or a script written before oslo existed could reach
/// the structured path by accident — and `uniq` is coreutils. `ls | uniq` is an ordinary thing to
/// write, and with the name taken it printed `ls`'s columns instead of deduplicating its lines.
pub fn distinct(rows: &[Record], field: Option<&str>) -> Vec<Record> {
    let field = field.map(Path::parse);
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    rows.iter()
        .filter(|row| {
            let key = match &field {
                Some(field) => match key_of(row, field) {
                    Some(key) => key,
                    // Without the column there is nothing to be distinct by, so the row goes
                    // through: dropping it would make `distinct` quietly a filter as well.
                    None => return true,
                },
                None => row
                    .columns()
                    .iter()
                    .zip(row.values())
                    .map(|(name, value)| format!("{name}={}", render_transport(value)))
                    .collect::<Vec<_>>()
                    .join("\u{0}"),
            };
            seen.insert(key)
        })
        .cloned()
        .collect()
}

/// min, max, sum, mean and count over one numeric column.
///
/// **Only the rows where the column is a number.** A column that is text in some rows is not an
/// error — rows are allowed to disagree about their columns — but averaging over the ones that are
/// not numbers would be arithmetic on nothing. `count` says how many took part, so a surprising
/// mean can be seen for what it is.
pub fn stats(rows: &[Record], field: &str) -> Vec<Record> {
    let path = Path::parse(field);
    let numbers: Vec<f64> = rows
        .iter()
        .filter_map(|row| number(path.get_or_absent(row)))
        .collect();
    let mut out = Record::new();
    out.set("field", Val::Str(field.to_string()));
    out.set("count", Val::Int(numbers.len() as i64));
    if numbers.is_empty() {
        return vec![out];
    }
    let sum: f64 = numbers.iter().sum();
    let min = numbers.iter().copied().fold(f64::INFINITY, f64::min);
    let max = numbers.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    // Whole numbers stay whole: a sum of byte counts reported as `4194304.0` reads as a rounding
    // error that is not there.
    out.set("min", exact(min));
    out.set("max", exact(max));
    out.set("sum", exact(sum));
    out.set("mean", Val::Float(sum / numbers.len() as f64));
    vec![out]
}

/// One cell as a number, for the cells that are one.
///
/// A `Size` counts as its bytes and a `Duration` as its nanoseconds — the same rule the Lua bridge
/// follows, so `stats size` and `where 'size > 1024'` agree about what a size is.
fn number(value: Option<&Val>) -> Option<f64> {
    match value? {
        Val::Int(n) => Some(*n as f64),
        Val::Float(n) => Some(*n),
        Val::Size(n) => Some(*n as f64),
        Val::Duration(n) | Val::Time(n) => Some(*n as f64),
        // Text that happens to be a number counts: `parse` and `from json` both produce strings for
        // fields nothing has told them the type of.
        Val::Str(text) => text.trim().parse().ok(),
        _ => None,
    }
}

fn exact(value: f64) -> Val {
    if value.fract() == 0.0 && value.abs() < 9.0e15 {
        Val::Int(value as i64)
    } else {
        Val::Float(value)
    }
}

/// What each column is: its name, the kind its cells are, and how many rows have one.
///
/// **The question you ask a stream you have never seen.** `kubectl get … -o json | from json` can
/// produce forty columns of unknown shape, and the alternative to this is `first 1 | to json` and
/// reading it. It answers the kind rather than a sample, because the kind is what decides whether
/// `sort-by` will order numerically and whether `where 'x > 1'` will compare or raise.
///
/// A column whose rows disagree reports `mixed`, which is a fact worth seeing: it is why a filter
/// works on some rows and raises on others.
pub fn describe(rows: &[Record]) -> Vec<Record> {
    let table = Val::table(rows.to_vec());
    table
        .columns()
        .into_iter()
        .map(|name| {
            let mut kinds: Vec<&'static str> = Vec::new();
            let mut filled = 0i64;
            for row in rows {
                let Some(cell) = row.get(&name) else { continue };
                filled += 1;
                let kind = kind_of(cell);
                if !kinds.contains(&kind) {
                    kinds.push(kind);
                }
            }
            // `nothing` does not make a column mixed: a null is an absent value, not another kind.
            let named: Vec<&str> = kinds.iter().copied().filter(|k| *k != "nothing").collect();
            let kind = match named.as_slice() {
                [] => "nothing",
                [one] => one,
                _ => "mixed",
            };
            Record::from_pairs([
                ("column", Val::Str(name)),
                ("type", Val::Str(kind.to_string())),
                ("filled", Val::Int(filled)),
                ("rows", Val::Int(rows.len() as i64)),
            ])
        })
        .collect()
}

fn kind_of(value: &Val) -> &'static str {
    match value {
        Val::Null => "nothing",
        Val::Bool(_) => "bool",
        Val::Int(_) => "int",
        Val::Float(_) => "float",
        Val::Str(_) => "string",
        Val::Bytes(_) => "binary",
        Val::Size(_) => "filesize",
        Val::Duration(_) => "duration",
        Val::Time(_) => "time",
        Val::List(_) => "list",
        Val::Record(_) => "record",
        Val::Error(_) => "error",
    }
}

/// How often each value of a column occurs, most frequent first, with a bar.
///
/// `group-by` then `count` answers the same numbers in first-seen order and without the shape. This
/// is the one you reach for to *look* at a distribution, so it sorts and draws — and the bar is in
/// the display face only by being text, which is a compromise the two-renderer rule tolerates
/// because the bar is the value, not a decoration on it.
pub fn histogram(rows: &[Record], field: &str) -> Vec<Record> {
    let counted = group_by(rows, field);
    let mut out: Vec<(Val, i64)> = counted
        .iter()
        .filter_map(|row| {
            let value = row.get(field)?.clone();
            let Val::Int(n) = row.get("count")? else {
                return None;
            };
            Some((value, *n))
        })
        .collect();
    // Descending, because a histogram is read from the top.
    out.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    let most = out.first().map(|(_, n)| *n).unwrap_or(0).max(1);
    out.into_iter()
        .map(|(value, n)| {
            // Twenty cells at the widest, so the bar fits beside the other columns on any terminal.
            let width = ((n as f64 / most as f64) * 20.0).round() as usize;
            Record::from_pairs([
                (field, value),
                ("count", Val::Int(n)),
                ("bar", Val::Str("█".repeat(width.max(1)))),
            ])
        })
        .collect()
}

#[cfg(test)]
#[path = "summarise/tests.rs"]
mod tests;
