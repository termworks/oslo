//! Delimiter-separated text, in and out: `from csv`, `from tsv`, `to csv`, `to tsv`.
//!
//! ```text
//! cat report.csv | from csv | where 'amount > 100' | to json
//! ps | cols pid name | to csv > processes.csv
//! ```
//!
//! # Hand-rolled, and that is a decision about the build
//!
//! oslo ships as a **static musl binary with no C toolchain**, so every dependency is a question
//! about what the release can still be. CSV is a hundred lines and its edge cases are the ones this
//! module already had to settle for [`crate::data::render_transport`] — a separator inside a value,
//! a newline inside a value, and how you say a quote. Taking a crate for that would be borrowing
//! risk to avoid work already done.
//!
//! # The quoting rule
//!
//! RFC 4180, which is what every spreadsheet writes and reads: a field is quoted when it contains
//! the delimiter, a quote, a newline or a carriage return, and a quote inside a quoted field is
//! written twice. **A field is never quoted when it does not need to be**, so a plain table stays
//! greppable — the same reason `render_transport` stays line-oriented.
//!
//! Two more things need it, and both are about the value coming back as itself. A field padded
//! with whitespace is quoted, and so is a string that would otherwise be read as a number — `007`
//! written bare comes back as `7`. Quoting is the only mark a CSV has for "this is text", so the
//! reader treats a quoted field as text and nothing else: no trimming, no number. An *unquoted*
//! field keeps the older, looser rule, because `a, 1, 2` is how a CSV gets written by hand.
//!
//! # Not the same as `to text`
//!
//! `to text` is oslo's own transport: tab separated, backslash escapes, no header, meant to be read
//! back by `lines`. `to csv` is for somebody else's program, so it carries a header row and quotes
//! rather than escapes. Two audiences, two formats; conflating them is how a header ends up in a
//! hand-over.

use crate::data::{Record, Val, render_transport};

/// The character between fields.
pub fn delimiter(format: &str) -> Option<char> {
    match format {
        "csv" => Some(','),
        "tsv" => Some('\t'),
        _ => None,
    }
}

/// Rows from delimited text, taking the first line as the column names.
pub fn from_delimited(input: &str, delimiter: char) -> Result<Vec<Record>, String> {
    let mut records = split(input, delimiter)?;
    if records.is_empty() {
        return Ok(Vec::new());
    }
    let names = records.remove(0);
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let names: Vec<String> = names.into_iter().map(|field| field.text).collect();
    Ok(records
        .into_iter()
        .map(|cells| {
            let mut row = Record::new();
            for (name, cell) in names.iter().zip(cells) {
                row.set(name, scalar(&cell.text, cell.quoted));
            }
            row
        })
        .collect())
}

/// Rows as delimited text, with a header row.
///
/// The header is the union of every row's columns, in first-seen order — the same rule the drawn
/// table follows, because rows are allowed to disagree about their columns and a reader of a CSV
/// needs one shape.
pub fn to_delimited(rows: &[Record], delimiter: char) -> String {
    let table = Val::table(rows.to_vec());
    let columns = table.columns();
    if columns.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    push_line(
        &mut out,
        columns.iter().map(|name| (name.as_str(), false)),
        delimiter,
    );
    for row in rows {
        let cells: Vec<(String, bool)> = columns
            .iter()
            .map(|name| match row.get(name) {
                Some(cell) => {
                    let text = render_transport(cell);
                    let ambiguous = matches!(cell, Val::Str(_)) && !text_reads_back_as_text(&text);
                    (text, ambiguous)
                }
                None => (String::new(), false),
            })
            .collect();
        push_line(
            &mut out,
            cells.iter().map(|(text, force)| (text.as_str(), *force)),
            delimiter,
        );
    }
    out
}

/// Whether an unquoted field of this text would come back as the string it started as.
///
/// **The writer's half of the quoting rule.** `"007"` is a string, and written bare it reads back
/// as the number 7 — so `to csv | from csv` was not the identity for exactly the values people
/// keep in a CSV because a spreadsheet would mangle them: zero-padded ids, version strings, phone
/// numbers. Teaching the reader to respect quotes fixes nothing on its own if the writer never
/// writes them.
fn text_reads_back_as_text(text: &str) -> bool {
    matches!(scalar(text, false), Val::Str(_))
}

fn push_line<'a>(out: &mut String, cells: impl Iterator<Item = (&'a str, bool)>, delimiter: char) {
    let quoted: Vec<String> = cells
        .map(|(cell, force)| quote(cell, delimiter, force))
        .collect();
    out.push_str(&quoted.join(&delimiter.to_string()));
    out.push('\n');
}

/// A field, quoted only when it has to be.
///
/// Leading or trailing whitespace counts as having to be. Written bare it survives this reader,
/// which takes an unquoted field as it finds it, but it does not survive every reader — and a
/// value that means `"  007  "` is exactly the value that must not come back as `7`.
fn quote(text: &str, delimiter: char, force: bool) -> String {
    let padded = text.starts_with(char::is_whitespace) || text.ends_with(char::is_whitespace);
    let needs = force
        || padded
        || text.contains(delimiter)
        || text.contains('"')
        || text.contains('\n')
        || text.contains('\r');
    match needs {
        false => text.to_string(),
        true => format!("\"{}\"", text.replace('"', "\"\"")),
    }
}

/// Split a whole document into rows of fields, honouring quotes.
///
/// A newline inside a quoted field does **not** end the record, which is the difference between a
/// parser and a `split('\n')` — and the case that quietly corrupts a spreadsheet export.
/// Whether `text` ends at a record boundary rather than inside a quoted field.
///
/// **Asked of the real parser rather than of a copy of its rules.** A streamed document is cut into
/// batches, and a cut inside `"one\ntwo"` would turn one record into two — silently, and only for
/// data that happens to quote a newline. The rules that decide it are not simple (a quote opens a
/// field only at its start, and `""` is an escaped quote inside one), so a second implementation of
/// them is a second thing to keep in step. This runs the same `split` and asks whether it was happy.
pub fn is_complete(text: &str, delimiter: char) -> bool {
    split(text, delimiter).is_ok()
}

/// One field, and whether the document quoted it.
///
/// **Quoting is the only thing in a CSV that says "this is text".** Without it a reader cannot tell
/// the number 7 from the string `007`, or an empty field from a padded one, so dropping the flag
/// here is what made `"007"` come back as `7` and `"  pad  "` come back as `pad`.
struct Field {
    text: String,
    quoted: bool,
}

fn split(input: &str, delimiter: char) -> Result<Vec<Vec<Field>>, String> {
    let mut rows = Vec::new();
    let mut row: Vec<Field> = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut was_quoted = false;
    let mut chars = input.chars().peekable();
    let mut any = false;
    let take = |cell: &mut String, was_quoted: &mut bool| Field {
        text: std::mem::take(cell),
        quoted: std::mem::replace(was_quoted, false),
    };

    while let Some(c) = chars.next() {
        any = true;
        if quoted {
            match c {
                '"' => match chars.peek() {
                    // A doubled quote is one quote inside the field.
                    Some('"') => {
                        cell.push('"');
                        chars.next();
                    }
                    _ => quoted = false,
                },
                other => cell.push(other),
            }
            continue;
        }
        match c {
            '"' if cell.is_empty() => {
                quoted = true;
                was_quoted = true;
            }
            c if c == delimiter => row.push(take(&mut cell, &mut was_quoted)),
            '\r' => {}
            '\n' => {
                row.push(take(&mut cell, &mut was_quoted));
                rows.push(std::mem::take(&mut row));
            }
            other => cell.push(other),
        }
    }
    if quoted {
        return Err("a quoted field is never closed".to_string());
    }
    // A document that does not end in a newline still has a last record.
    if !cell.is_empty() || !row.is_empty() {
        row.push(take(&mut cell, &mut was_quoted));
        rows.push(row);
    }
    if !any {
        return Ok(Vec::new());
    }
    Ok(rows)
}

/// A cell as the most specific kind it plainly is — `parse`'s rule, so a column of numbers compares
/// as numbers whichever bridge produced it.
///
/// **A quoted field is text and nothing else.** That is what the quotes are for, and reading them
/// as a suggestion is how `"007"` became `7` and `"  pad  "` became `pad` — the two values a
/// spreadsheet quotes precisely so they arrive intact. An unquoted field keeps the old rule,
/// trimming included, because `a, 1, 2` is how people write a CSV by hand and the space before
/// the `1` is spacing rather than data.
fn scalar(text: &str, quoted: bool) -> Val {
    if quoted {
        return Val::Str(text.to_string());
    }
    let trimmed = text.trim();
    if let Ok(i) = trimmed.parse::<i64>() {
        return Val::Int(i);
    }
    if let Ok(f) = trimmed.parse::<f64>()
        && trimmed.contains('.')
    {
        return Val::Float(f);
    }
    Val::Str(trimmed.to_string())
}

#[cfg(test)]
#[path = "formats/tests.rs"]
mod tests;
