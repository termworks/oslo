//! IFS field splitting.

use crate::env::Environment;
use crate::expand::word::{Field, Origin, Run};

/// The active IFS, defaulting to space/tab/newline when the variable is unset.
pub fn ifs_of(env: &Environment) -> &str {
    env.get_var("IFS").unwrap_or(" \t\n")
}

/// Split one field on IFS, honouring where each run came from.
///
/// Only runs produced by an unquoted expansion are eligible. POSIX splits the *result* of an
/// expansion and nothing else, so under `IFS=:` the word `a:b:c` is a single argument while
/// `$v` holding `a:b:c` is three — a distinction the caller cannot recover once the runs are
/// concatenated, which is why splitting happens here rather than on the flattened text.
///
/// The delimiter rules are POSIX 2.6.5, which treats the two halves of IFS differently:
/// IFS *whitespace* collapses into its neighbours and disappears at either end, while every
/// other IFS character delimits exactly one field. `IFS=:` over `a::b` is therefore three
/// fields with an empty one in the middle, and over `a b` it is one field containing a space.
pub fn split_field(ifs: &str, field: Field) -> Vec<Field> {
    if !field.iter().any(Run::splits) {
        return vec![field];
    }

    let mut splitter = Splitter {
        ifs,
        done: Vec::new(),
        open: None,
    };
    for run in field {
        if run.origin.is_mark() {
            continue;
        }
        if run.splits() {
            splitter.split_text(&run.text);
        } else {
            // Quoted and literal text joins whatever field is being built, and opens one if
            // none is: `x"$@"` and `$v"post"` both keep their quoted half attached.
            splitter.open.get_or_insert_default().push(run);
        }
    }
    splitter.finish()
}

/// [`split_field`] with every byte a character of its own when `bytes`, as the C locale splits: a
/// multibyte IFS character is then several delimiters.
pub fn split_field_in(ifs: &str, mut field: Field, bytes: bool) -> Vec<Field> {
    if !bytes || ifs.is_ascii() {
        return split_field(ifs, field);
    }
    let ifs = crate::expand::locale::bytewise(ifs);
    crate::expand::locale::runs_in_locale(&mut field, true);
    let mut fields = split_field(&ifs, field);
    for run in fields.iter_mut().flatten() {
        run.text = crate::expand::locale::from_bytewise(&run.text);
    }
    fields
}

/// The running state of a split: the fields already delimited, and the one still growing.
///
/// `open: None` means no field is being built, which is what distinguishes a delimiter that
/// closes a field from one that merely separates two others. It is why leading IFS whitespace
/// produces nothing at all while a leading `:` under `IFS=:` produces an empty first field.
struct Splitter<'a> {
    ifs: &'a str,
    done: Vec<Field>,
    open: Option<Field>,
}

impl Splitter<'_> {
    fn is_ifs(&self, ch: char) -> bool {
        self.ifs.contains(ch)
    }

    /// IFS whitespace: the subset of IFS that collapses rather than delimiting one field each.
    fn is_ifs_space(&self, ch: char) -> bool {
        ch.is_whitespace() && self.is_ifs(ch)
    }

    fn close(&mut self) {
        if let Some(field) = self.open.take() {
            self.done.push(field);
        }
    }

    /// Split the text of one unquoted expansion, appending to whatever is already open.
    ///
    /// With an empty IFS nothing is a delimiter, so the text passes through as a single piece —
    /// while an expansion that produced nothing at all still contributes no field.
    fn split_text(&mut self, text: &str) {
        let chars: Vec<char> = text.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let start = i;
            while i < chars.len() && !self.is_ifs(chars[i]) {
                i += 1;
            }
            if i > start {
                // The delimiters are gone; what remains is ordinary unquoted text, which still
                // globs but must not be split a second time.
                let piece: String = chars[start..i].iter().collect();
                self.open
                    .get_or_insert_default()
                    .push(Run::new(piece, Origin::Literal));
            }
            if i == chars.len() {
                break;
            }

            // One delimiter is: any run of IFS whitespace, at most one other IFS character, then
            // any run of IFS whitespace. `a : b` under `IFS=" :"` is two fields, not four.
            let had_field = self.open.is_some();
            let mut saw_non_space = false;
            while i < chars.len() && self.is_ifs_space(chars[i]) {
                i += 1;
            }
            if i < chars.len() && self.is_ifs(chars[i]) {
                saw_non_space = true;
                i += 1;
                while i < chars.len() && self.is_ifs_space(chars[i]) {
                    i += 1;
                }
            }

            if had_field {
                self.close();
            } else if saw_non_space {
                // Nothing preceded this delimiter, so the field it closes is the empty one:
                // `:a` is two fields, whereas the leading blank of ` a` is simply stripped.
                self.done.push(Vec::new());
            }
        }
    }

    /// A field still open at the end of the word survives; a trailing delimiter left none, which
    /// is why `a:` under `IFS=:` is one field rather than two.
    fn finish(mut self) -> Vec<Field> {
        self.close();
        self.done
    }
}

#[cfg(test)]
mod tests {
    use super::split_field;
    use crate::expand::word::{Origin, Run, field_text};

    fn split(ifs: &str, runs: Vec<Run>) -> Vec<String> {
        split_field(ifs, runs)
            .iter()
            .map(|f| field_text(f))
            .collect()
    }

    fn split_expansion(ifs: &str, text: &str) -> Vec<String> {
        split(ifs, vec![Run::new(text, Origin::Expanded)])
    }

    #[test]
    fn literal_runs_are_never_split() {
        let runs = vec![Run::new("a:b:c", Origin::Literal)];
        assert_eq!(split(":", runs), vec!["a:b:c"]);
    }

    #[test]
    fn quoted_runs_are_never_split() {
        let runs = vec![Run::new("a b", Origin::Quoted)];
        assert_eq!(split(" ", runs), vec!["a b"]);
    }

    #[test]
    fn expansion_output_is_split() {
        let runs = vec![Run::new("a b  c", Origin::Expanded)];
        assert_eq!(split(" ", runs), vec!["a", "b", "c"]);
    }

    /// A delimiter inside an expansion still ends the field the literal text before it started.
    #[test]
    fn a_delimiter_closes_a_field_that_began_in_a_literal() {
        let runs = vec![
            Run::new("pre", Origin::Literal),
            Run::new(" x ", Origin::Expanded),
            Run::new("post", Origin::Quoted),
        ];
        assert_eq!(split(" ", runs), vec!["pre", "x", "post"]);
    }

    #[test]
    fn an_expansion_that_produced_nothing_leaves_no_field() {
        let runs = vec![Run::new("", Origin::Expanded)];
        assert!(split(" ", runs).is_empty());
        // …even with no delimiters to split on at all.
        assert!(split("", vec![Run::new("", Origin::Expanded)]).is_empty());
    }

    #[test]
    fn a_quoted_empty_run_survives_as_an_empty_field() {
        let runs = vec![Run::new("", Origin::Quoted)];
        assert_eq!(split(" ", runs), vec![""]);
    }

    #[test]
    fn empty_ifs_disables_splitting() {
        let runs = vec![Run::new("a b", Origin::Expanded)];
        assert_eq!(split("", runs), vec!["a b"]);
    }

    /// POSIX 2.6.5: every non-whitespace IFS character delimits exactly one field, so the gap
    /// between two of them is an empty field rather than nothing at all.
    #[test]
    fn non_whitespace_ifs_delimits_one_field_each() {
        assert_eq!(split_expansion(":", "a::b"), vec!["a", "", "b"]);
        assert_eq!(split_expansion(":", "::"), vec!["", ""]);
        assert_eq!(split_expansion(":", ":"), vec![""]);
    }

    /// A leading non-whitespace delimiter opens with an empty field; a trailing one closes the
    /// last field without opening another.
    #[test]
    fn leading_and_trailing_non_whitespace_are_not_symmetric() {
        assert_eq!(split_expansion(":", ":a:"), vec!["", "a"]);
        assert_eq!(split_expansion(":", "a:"), vec!["a"]);
        assert_eq!(split_expansion(":", "a::"), vec!["a", ""]);
    }

    /// IFS whitespace collapses into runs and is stripped at both ends, producing no empty
    /// fields of its own.
    #[test]
    fn ifs_whitespace_collapses_and_strips() {
        assert_eq!(split_expansion(" \t\n", "  a \t b  "), vec!["a", "b"]);
        assert!(split_expansion(" ", "   ").is_empty());
    }

    /// Whitespace adjacent to a non-whitespace delimiter is absorbed by it, so `a : b` is two
    /// fields — but whitespace *between* two delimiters does not merge them.
    #[test]
    fn whitespace_is_absorbed_into_an_adjacent_delimiter() {
        assert_eq!(split_expansion(" :", "a : b"), vec!["a", "b"]);
        assert_eq!(split_expansion(" :", "a: :b"), vec!["a", "", "b"]);
        assert_eq!(split_expansion(" :", ": a"), vec!["", "a"]);
        assert_eq!(split_expansion(" :", "a :"), vec!["a"]);
        assert_eq!(split_expansion(" :", "  :  "), vec![""]);
    }

    /// Splitting is a property of the whole word, not of one run: an unquoted expansion that
    /// begins with whitespace still closes the field the text before it opened.
    #[test]
    fn splitting_spans_run_boundaries() {
        let runs = vec![
            Run::new("pre", Origin::Literal),
            Run::new(":a:", Origin::Expanded),
            Run::new("post", Origin::Quoted),
        ];
        assert_eq!(split(":", runs), vec!["pre", "a", "post"]);
    }

    /// The pieces a split leaves behind are unquoted, so they still glob — but they carry no
    /// second round of splitting.
    #[test]
    fn split_pieces_stay_globbable() {
        let fields = split_field(" ", vec![Run::new("a* b", Origin::Expanded)]);
        assert_eq!(fields.len(), 2);
        assert!(fields[0][0].globs());
        assert!(!fields[0][0].splits());
    }
}
