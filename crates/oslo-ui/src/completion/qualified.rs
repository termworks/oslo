//! `pattern(qualifiers)` at the prompt: `rm **/*.log(older 7d, larger 1M)`.
//!
//! Prompt-only, and always turned into literal filenames before the line runs — by Tab, by
//! `expand-glob`, or by Enter. **Why this is safe:** an unquoted `(` in the middle of a word is a
//! syntax error in sh and bash, so no working command line contains one, and a script never passes
//! through the editor, so what a script means cannot change. What runs, and what history records,
//! is the filenames. The qualifiers are `oslo_base::glob::qualify`'s, the same the `glob` builtin
//! and `oslo.glob` take.

use crate::words::{Quote, quote_replacement, unquote};
use oslo_base::glob::qualify;
use oslo_base::glob::walk::{self, Options};

/// Directory entries one expansion may read before it refuses. Larger than Tab's budget, because
/// the answer goes onto a command line, where a partial list is worse than none.
const ENTRIES: usize = 200_000;

/// One `PATTERN(QUALIFIERS)` on a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Qualified {
    /// Byte span, the closing `)` included.
    pub start: usize,
    pub end: usize,
    pub pattern: String,
    pub qualifiers: String,
}

/// Every qualified glob on `line`, outside quotes.
///
/// A `(` counts only when it is attached to a word that globs, is not `$(`, and holds something:
/// `foo() {`, `$(date)`, `arr=(a b)` and `( subshell )` are all left alone.
pub fn find_all(line: &str) -> Vec<Qualified> {
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    let mut out = Vec::new();
    let (mut quote, mut escaped) = (None::<char>, false);
    let (mut word_start, mut globbed) = (0, false);
    let mut i = 0;
    while i < chars.len() {
        let (at, ch) = chars[i];
        i += 1;
        if escaped {
            escaped = false;
            continue;
        }
        if let Some(q) = quote {
            match ch {
                c if c == q => quote = None,
                '\\' if q == '"' => escaped = true,
                _ => {}
            }
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '\'' | '"' => quote = Some(ch),
            '*' | '?' | '[' => globbed = true,
            '(' => {
                let attached = at > word_start && !line[..at].ends_with('$');
                let close = (attached && globbed).then(|| closing(line, at)).flatten();
                if let Some(close) = close {
                    let qualifiers = line[at + 1..close].trim();
                    if !qualifiers.is_empty() {
                        out.push(Qualified {
                            start: word_start,
                            end: close + 1,
                            pattern: unquote(&line[word_start..at]),
                            qualifiers: qualifiers.to_string(),
                        });
                        while i < chars.len() && chars[i].0 <= close {
                            i += 1;
                        }
                    }
                }
                word_start = at + 1;
                globbed = false;
            }
            c if c.is_whitespace() || matches!(c, ';' | '|' | '&' | '<' | '>' | ')') => {
                word_start = at + c.len_utf8();
                globbed = false;
            }
            _ => {}
        }
    }
    out
}

/// The `)` that closes the `(` at `open`, quotes and nesting honoured.
fn closing(line: &str, open: usize) -> Option<usize> {
    let (mut depth, mut quote) = (0usize, None::<char>);
    for (offset, ch) in line[open..].char_indices() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(ch),
            (None, '(') => depth += 1,
            (None, ')') => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + offset);
                }
            }
            _ => {}
        }
    }
    None
}

/// The qualified glob whose span holds `pos`, if any.
pub fn at(line: &str, pos: usize) -> Option<Qualified> {
    find_all(line)
        .into_iter()
        .find(|q| q.start <= pos && pos <= q.end)
}

/// Every match, quoted for the line — or why there is none to give.
pub fn expand(q: &Qualified) -> Result<Vec<String>, String> {
    let quals = qualify::parse(&q.qualifiers)?;
    let (typed_root, read_root, rest) = tilde_split(&q.pattern);
    let mut options = Options {
        globstar: true,
        budget: Some(ENTRIES),
        ..walk::shell_options()
    };
    options.dotglob |= quals.hidden;
    options.nocase |= quals.nocase;
    let chars: Vec<(char, bool)> = read_root
        .chars()
        .map(|c| (c, false))
        .chain(rest.chars().map(|c| (c, true)))
        .collect();
    let Some(expansion) = walk::expand_counted(&chars, &options) else {
        return Err(format!("`{}` is not a pattern", q.pattern));
    };
    if !expansion.complete {
        return Err(format!(
            "`{}` reaches more than {ENTRIES} entries; narrow it",
            q.pattern
        ));
    }
    let whole = format!("{read_root}{rest}");
    let kept = qualify::apply(
        expansion.paths,
        &quals,
        qualify::base_of(&whole),
        options.collate,
    );
    if kept.is_empty() {
        return Err(format!("`{}({})` matches nothing", q.pattern, q.qualifiers));
    }
    Ok(kept
        .into_iter()
        .map(|read| {
            let tail = read.strip_prefix(read_root.as_str()).unwrap_or(&read);
            quote_replacement(&format!("{typed_root}{tail}"), Quote::None)
        })
        .collect())
}

/// A `~` is read from `$HOME` and written back as the `~` that was typed.
fn tilde_split(pattern: &str) -> (String, String, &str) {
    if let Some(after) = pattern.strip_prefix('~') {
        let cut = after.find('/').unwrap_or(after.len());
        let (user, tail) = after.split_at(cut);
        let home = oslo_base::tilde::expand(user, &oslo_base::tilde::from_process);
        return (format!("~{user}"), home, tail);
    }
    (String::new(), String::new(), pattern)
}

/// `line` with every qualified glob replaced by its matches — what Enter runs.
///
/// `Ok(None)` when there is none. An `Err` names the one that could not be expanded, and the line
/// is left as typed.
pub fn rewrite(line: &str) -> Result<Option<String>, String> {
    let found = find_all(line);
    if found.is_empty() {
        return Ok(None);
    }
    let mut out = String::with_capacity(line.len());
    let mut at = 0;
    for q in &found {
        let words = expand(q)?;
        out.push_str(&line[at..q.start]);
        out.push_str(&words.join(" "));
        at = q.end;
    }
    out.push_str(&line[at..]);
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::find_all;

    fn spans(line: &str) -> Vec<(String, String)> {
        find_all(line)
            .into_iter()
            .map(|q| (q.pattern, q.qualifiers))
            .collect()
    }

    #[test]
    fn a_glob_with_parentheses_attached_is_qualified() {
        assert_eq!(
            spans("rm **/*.log(older 7d, larger 1M) -v"),
            [("**/*.log".to_string(), "older 7d, larger 1M".to_string())]
        );
        assert_eq!(spans("ls *(dir)"), [("*".to_string(), "dir".to_string())]);
        assert_eq!(
            spans("ls *(re 'a)b')"),
            [("*".to_string(), "re 'a)b'".to_string())],
            "a `)` inside quotes does not close the list"
        );
    }

    #[test]
    fn everything_else_with_a_parenthesis_is_left_alone() {
        for line in [
            "echo $(date)",
            "foo() { echo hi; }",
            "arr=(a b c)",
            "( cd /tmp && ls )",
            "echo \"*.log(older 7d)\"",
            "echo '*(dir)'",
            "echo plain(word)",
            "ls *()",
        ] {
            assert!(find_all(line).is_empty(), "{line}");
        }
    }
}
