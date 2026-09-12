//! The transport face: what a program reads.

use super::*;

/// A value as a program should read it: plain, complete, one record per line.
///
/// No colour, no borders, no truncation, no abbreviation. A size is its number of bytes, because
/// the program on the other end will do arithmetic on it and `4.2G` is not a number.
///
/// **A cell is escaped, because the separators are in band.** Records are separated by a newline
/// and cells by a tab, so a cell that contains either used to break the framing silently: one row
/// arrived as two, and every column after the tab shifted by one. That made `to text` lossy for
/// exactly the data a shell meets most — a filename with a tab in it, a `cmdline` spanning lines —
/// and it corrupted every hand-over into a byte suffix, which is rendered the same way.
///
/// See `escape_cell` for the form. Nothing *un*escapes on the way back in: `lines` and `parse`
/// read arbitrary bytes from programs that never heard of oslo, and a backslash in their output is
/// a backslash.
pub fn render_transport(value: &Val) -> String {
    match value {
        Val::List(items) => items
            .iter()
            .map(render_transport)
            .collect::<Vec<_>>()
            .join("\n"),
        Val::Record(record) => record
            .values()
            .iter()
            .map(|cell| escape_cell(&render_transport(cell)))
            .collect::<Vec<_>>()
            .join("\t"),
        Val::Size(bytes) => bytes.to_string(),
        Val::Duration(ns) => ns.to_string(),
        Val::Time(ns) => ns.to_string(),
        other => scalar(other),
    }
}

/// A cell with the separators spelled rather than written.
///
/// `\` first, or unescaping could not tell `\t` the two characters from `\t` the tab. A nested list
/// or record inside a cell is rendered by [`render_transport`] with its own newlines and tabs, and
/// this catches those too — which is why it is applied to the rendered cell rather than to the
/// string inside it.
fn escape_cell(text: &str) -> String {
    if !text.contains(['\\', '\t', '\n', '\r']) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out
}

/// The inverse of `escape_cell`, for a reader that knows it is reading oslo's own transport.
///
/// Deliberately **not** applied by `lines` or `parse`: those read whatever a program wrote, and a
/// program that emits a literal backslash means one.
pub fn unescape_cell(text: &str) -> String {
    if !text.contains('\\') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            // Not a form this wrote: keep both characters rather than eat the backslash.
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}
