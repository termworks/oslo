//! Turning one line of input into named variables, the way `IFS` says to.
//!
//! `read` splits on the same `IFS` rules as ordinary word splitting — POSIX 2.6.5, where the
//! whitespace half of `IFS` collapses and every other character delimits exactly one field — so
//! the decision of *what the fields are* is delegated to [`crate::expand::fields::split_field`]
//! rather than reimplemented. What cannot be delegated is `read`'s one deviation: the last name
//! receives the unsplit remainder, separators and all, which needs byte positions that a list of
//! finished fields no longer carries.

use super::read_input::InputLine;
use crate::env::scope::Environment;
use crate::expand::fields::{ifs_of, split_field};
use crate::expand::word::{Origin, Run, field_text};

/// One `IFS` delimiter matched in the line.
struct Delim {
    /// Length in bytes, so a multi-byte `IFS` character is skipped whole.
    len: usize,
    /// IFS whitespace collapses; every other IFS character delimits one field on its own.
    whitespace: bool,
}

/// A line plus the `IFS` it is being read under.
struct Splitter<'a> {
    ifs: &'a str,
    line: &'a InputLine,
}

impl Splitter<'_> {
    fn len(&self) -> usize {
        self.line.bytes.len()
    }

    /// The delimiter starting at byte `i`, if any.
    ///
    /// A byte that arrived escaped is data no matter what `IFS` says — that is the whole point of
    /// `a\ b` reading as one field.
    fn delim_at(&self, i: usize) -> Option<Delim> {
        if i >= self.len() || self.line.escaped[i] {
            return None;
        }
        for ch in self.ifs.chars() {
            let encoded = oslo_base::lossless::decode(ch.encode_utf8(&mut [0u8; 4]));
            if self.line.bytes[i..].starts_with(&encoded) {
                return Some(Delim {
                    len: encoded.len(),
                    whitespace: ch.is_whitespace(),
                });
            }
        }
        None
    }

    fn skip_whitespace(&self, mut pos: usize) -> usize {
        while pos < self.len() {
            match self.delim_at(pos) {
                Some(d) if d.whitespace => pos += d.len,
                _ => break,
            }
        }
        pos
    }

    /// Scan to the next delimiter. Advancing one byte at a time is safe: a UTF-8 continuation
    /// byte can never begin a character, so it can never begin a delimiter either.
    fn field_end(&self, mut pos: usize) -> usize {
        while pos < self.len() && self.delim_at(pos).is_none() {
            pos += 1;
        }
        pos
    }

    /// Step over exactly one delimiter: a run of IFS whitespace, at most one other IFS
    /// character, then more IFS whitespace. `a : b` under `IFS=" :"` is two fields, not four.
    fn consume_delim(&self, pos: usize) -> usize {
        let pos = self.skip_whitespace(pos);
        match self.delim_at(pos) {
            Some(d) if !d.whitespace => self.skip_whitespace(pos + d.len),
            _ => pos,
        }
    }

    /// The end of the range with trailing IFS *whitespace* removed. A trailing `:` under
    /// `IFS=:` is not whitespace and stays: `read x` over `a::` assigns `a::`.
    fn trim_trailing_whitespace(&self, start: usize, end: usize) -> usize {
        let mut pos = start;
        let mut content_end = start;
        while pos < end {
            match self.delim_at(pos) {
                Some(d) if d.whitespace => pos += d.len,
                Some(d) => {
                    pos += d.len;
                    content_end = pos;
                }
                None => {
                    pos += 1;
                    content_end = pos;
                }
            }
        }
        content_end
    }

    fn slice(&self, start: usize, end: usize) -> String {
        oslo_base::lossless::from_bytes(&self.line.bytes[start..end])
    }

    /// The byte range as runs for [`split_field`]: escaped stretches are [`Origin::Quoted`], so
    /// the shared splitter already knows not to cut them, and the rest is expansion output.
    fn runs(&self, start: usize, end: usize) -> Vec<Run> {
        let mut runs = Vec::new();
        let mut pos = start;
        while pos < end {
            let escaped = self.line.escaped[pos];
            let mut run_end = pos;
            while run_end < end && self.line.escaped[run_end] == escaped {
                run_end += 1;
            }
            let origin = if escaped {
                Origin::Quoted
            } else {
                Origin::Expanded
            };
            runs.push(Run::new(self.slice(pos, run_end), origin));
            pos = run_end;
        }
        runs
    }

    fn fields_in(&self, start: usize, end: usize) -> Vec<String> {
        if start >= end {
            return Vec::new();
        }
        split_field(self.ifs, self.runs(start, end))
            .iter()
            .map(|f| field_text(f))
            .collect()
    }

    /// What the last name gets: the remainder, separators included.
    ///
    /// bash keeps the separators only when there is something to separate. Where the remainder
    /// holds a single field, that field is assigned — which is why `IFS=: read x` over `a:`
    /// yields `a` while over `a::` it yields `a::`, the second remainder being two fields.
    fn remainder(&self, pos: usize) -> String {
        let start = self.skip_whitespace(pos);
        let end = self.trim_trailing_whitespace(start, self.len());
        let fields = self.fields_in(start, end);
        match fields.len() {
            0 => String::new(),
            1 => fields.into_iter().next().unwrap_or_default(),
            _ => self.slice(start, end),
        }
    }
}

/// Split `line` across `names`, giving the last name the unsplit remainder.
///
/// `names` must not be empty; the caller handles `REPLY` and `-N`, neither of which splits.
pub fn assign_fields(env: &mut Environment, names: &[String], line: &InputLine) -> bool {
    let ifs = ifs_of(env);
    let splitter = Splitter { ifs, line };

    let mut pos = 0;
    let leading = names
        .len()
        .checked_sub(1)
        .expect("caller rejects an empty name list");
    let mut values: Vec<String> = Vec::with_capacity(names.len());
    for _ in 0..leading {
        pos = splitter.skip_whitespace(pos);
        let start = pos;
        pos = splitter.field_end(pos);
        values.push(splitter.slice(start, pos));
        pos = splitter.consume_delim(pos);
    }
    values.push(splitter.remainder(pos));

    let mut assigned = true;
    for (name, value) in names.iter().zip(values) {
        assigned &= super::read::assign_name(env, name, &value);
    }
    assigned
}

/// Every field the line splits into, for `read -a`.
pub fn all_fields(env: &Environment, line: &InputLine) -> Vec<String> {
    let ifs = ifs_of(env);
    let splitter = Splitter { ifs, line };
    splitter.fields_in(0, splitter.len())
}

#[cfg(test)]
mod tests {
    use super::super::read_input::InputLine;
    use super::assign_fields;
    use crate::env::scope::Environment;

    /// Build a line with no escaped bytes — the shape all the IFS cases below need.
    fn line(text: &str) -> InputLine {
        InputLine::from_text(text)
    }

    /// Read `text` under `ifs` into `names`, returning what each name ends up holding.
    fn read_into(ifs: &str, text: &str, names: &[&str]) -> Vec<String> {
        let mut env = Environment::new();
        env.set_var("IFS", ifs, false);
        let owned: Vec<String> = names.iter().map(|n| (*n).to_string()).collect();
        assign_fields(&mut env, &owned, &line(text));
        owned
            .iter()
            .map(|n| env.get_var(n).unwrap_or_default().to_string())
            .collect()
    }

    #[test]
    fn a_non_whitespace_ifs_delimits_one_field_each() {
        assert_eq!(read_into(":", "a::b", &["x", "y", "z"]), ["a", "", "b"]);
        assert_eq!(
            read_into(":", "root:x:0", &["u", "p", "i"]),
            ["root", "x", "0"]
        );
    }

    /// The remainder keeps its separators once there is more than one field left in it.
    #[test]
    fn the_last_name_takes_the_rest_verbatim() {
        assert_eq!(read_into(":", "a::b", &["x", "y"]), ["a", ":b"]);
        assert_eq!(read_into(" ", "1  2   3   ", &["a", "b"]), ["1", "2   3"]);
        assert_eq!(read_into(" :", "a :  b :  c", &["x", "y"]), ["a", "b :  c"]);
    }

    /// …but a remainder that is one field is assigned as that field, delimiter stripped. This
    /// asymmetry is bash's, and it is why `a:` and `a::` do not behave alike.
    #[test]
    fn a_single_field_remainder_loses_its_trailing_delimiter() {
        assert_eq!(read_into(":", "a:", &["x"]), ["a"]);
        assert_eq!(read_into(":", ":", &["x"]), [""]);
        assert_eq!(read_into(":", "a::", &["x"]), ["a::"]);
        assert_eq!(read_into(":", ":a:", &["x"]), [":a:"]);
        assert_eq!(read_into(":", "a:b:", &["x", "y"]), ["a", "b"]);
    }

    #[test]
    fn ifs_whitespace_is_stripped_at_both_ends_and_collapses() {
        assert_eq!(read_into(" \t\n", "   x\ty   ", &["a"]), ["x\ty"]);
        assert_eq!(read_into(" ", "   ", &["a"]), [""]);
        assert_eq!(read_into(" :", "  :  ", &["x", "y"]), ["", ""]);
    }

    /// Whitespace that is not in IFS is ordinary data, even at the edges of a field.
    #[test]
    fn only_ifs_characters_delimit() {
        assert_eq!(read_into(":", "  a : b  ", &["x", "y"]), ["  a ", " b  "]);
    }

    #[test]
    fn an_empty_ifs_disables_splitting_entirely() {
        assert_eq!(read_into("", "a b c", &["x", "y"]), ["a b c", ""]);
    }

    #[test]
    fn extra_names_are_emptied() {
        assert_eq!(read_into(":", "a", &["x", "y", "z"]), ["a", "", ""]);
        assert_eq!(read_into(":", "", &["x", "y"]), ["", ""]);
    }

    #[test]
    fn an_escaped_delimiter_is_data() {
        let mut env = Environment::new();
        env.set_var("IFS", " ", false);
        let mut input = line("a b c");
        input.mark_escaped(1); // the first space arrived as `\ `
        let names = ["x".to_string(), "y".to_string()];
        assign_fields(&mut env, &names, &input);
        assert_eq!(env.get_var("x"), Some("a b"));
        assert_eq!(env.get_var("y"), Some("c"));
    }
}
