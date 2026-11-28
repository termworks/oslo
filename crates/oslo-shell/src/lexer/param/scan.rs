//! Finding the seams in a `${…}` body.
//!
//! Purely positional: this module never decides what an operator *means*, only where one is.
//! Both entry points share the same hazard — a `:` or `/` that belongs to a nested expansion, a
//! quoted run, or a backslash escape is not a separator, so `${a#${b:-c}}` splits on the `#` and
//! `${v/${sep:-/}/-}` splits on the second `/`, not the one inside the payload.

/// The `${…}` operators.
///
/// Order matters only between operators that share a prefix, and there the longer one comes
/// first: `%%` must never be read as `%` followed by a pattern, `//` never as `/`, and the whole
/// `:-` family must be tried before the bare `:` that introduces a substring.
pub(super) const PARAM_OPERATORS: &[&str] = &[
    ":-", ":=", ":+", ":?", ":", //
    "//", "/#", "/%", "/", //
    "%%", "%", "##", "#", //
    "^^", "^", ",,", ",", "~~", "~", //
    "-", "=", "+", "?", "@",
];

/// Find the operator that splits a `${…}` body into name and argument.
///
/// Scanning rather than `str::find` per operator, for two reasons. Nested expansions have their
/// own operators — `${a#${b:-c}}` must split on the `#`, not on the `:-` four characters later,
/// which would cut the name in half mid-`${`. And the winner is the *leftmost* operator, not the
/// first one a fixed search order happens to hit: `${v%a:-b}` strips a suffix, it has no default.
pub(super) fn find_param_operator(content: &str) -> Option<(usize, &'static str)> {
    let chars: Vec<(usize, char)> = content.char_indices().collect();
    let mut k = 0;
    let mut depth = 0usize;

    while k < chars.len() {
        let (offset, ch) = chars[k];
        match ch {
            '\\' => {
                k += 2;
                continue;
            }
            '\'' | '"' | '`' => {
                k = skip_quoted(&chars, k);
                continue;
            }
            '$' if matches!(chars.get(k + 1), Some((_, '{')) | Some((_, '('))) => {
                depth += 1;
                k += 2;
                continue;
            }
            '}' | ')' if depth > 0 => {
                depth -= 1;
            }
            // An array subscript is part of the *name*, and its contents are arithmetic — which
            // shares almost every character with the operator table. Without this, `${a[i-1]}`
            // would split on the `-` and ask for `${a[i}` defaulted to `1]`.
            '[' if depth == 0 => {
                k = skip_subscript(&chars, k);
                continue;
            }
            // At offset 0 there would be no name left, and `#`/`!` there are the prefix forms.
            _ if depth == 0 && k > 0 => {
                if let Some(op) = PARAM_OPERATORS
                    .iter()
                    .find(|op| content[offset..].starts_with(**op))
                {
                    return Some((offset, op));
                }
            }
            _ => {}
        }
        k += 1;
    }

    None
}

/// Split `a[expr]` into the array name and the raw subscript, or `None` if there is no subscript.
///
/// The *first* `[` opens it, so `a[b[0]]` is the array `a` subscripted by `b[0]`. A body with no
/// closing bracket is not a subscript at all: it stays part of the name and the expander rejects
/// it, which is how `${a[1}` becomes a diagnostic instead of a silent empty string.
pub(super) fn split_name_subscript(name: &str) -> Option<(&str, &str)> {
    let body = name.strip_suffix(']')?;
    let open = body.find('[')?;
    Some((&body[..open], &body[open + 1..]))
}

/// What counts as a nested group when looking for an operand separator.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Nesting {
    /// Only `${…}` and `$(…)` nest. A bare parenthesis is data — in `${v/(/X}` it is the pattern.
    Expansion,
    /// Parentheses nest too, because the operand is an arithmetic expression.
    Arithmetic,
}

/// Split `rest` at the first `sep` that is not quoted, escaped or nested.
///
/// `None` for the tail means the separator never appeared, which each caller reads differently:
/// no length for a substring, an empty replacement for a pattern replacement.
pub(super) fn split_top_level(rest: &str, sep: char, nesting: Nesting) -> (&str, Option<&str>) {
    let chars: Vec<(usize, char)> = rest.char_indices().collect();
    let mut k = 0;
    let mut depth = 0usize;

    while k < chars.len() {
        let (offset, ch) = chars[k];
        match ch {
            '\\' => {
                k += 2;
                continue;
            }
            '\'' | '"' | '`' => {
                k = skip_quoted(&chars, k);
                continue;
            }
            '$' if matches!(chars.get(k + 1), Some((_, '{')) | Some((_, '('))) => {
                depth += 1;
                k += 2;
                continue;
            }
            '(' if nesting == Nesting::Arithmetic => depth += 1,
            '}' | ')' if depth > 0 => depth -= 1,
            c if c == sep && depth == 0 => {
                return (&rest[..offset], Some(&rest[offset + c.len_utf8()..]));
            }
            _ => {}
        }
        k += 1;
    }

    (rest, None)
}

/// Index just past the `[…]` subscript that starts at `k`, or just past the `[` if it never
/// closes — an unterminated subscript is a name the expander will reject, not a reason to loop.
///
/// Brackets nest so that `${a[b[0]]}` is one subscript: bash evaluates the body as arithmetic,
/// and arithmetic can index another array.
fn skip_subscript(chars: &[(usize, char)], k: usize) -> usize {
    let mut depth = 0usize;
    let mut i = k;
    while i < chars.len() {
        match chars[i].1 {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    k + 1
}

/// Index just past the quoted run that starts at `k`.
fn skip_quoted(chars: &[(usize, char)], k: usize) -> usize {
    let closer = chars[k].1;
    let mut i = k + 1;
    while i < chars.len() {
        match chars[i].1 {
            // A backslash is data inside `'…'`; everywhere else it hides the next character.
            '\\' if closer != '\'' => i += 2,
            c if c == closer => return i + 1,
            _ => i += 1,
        }
    }
    i
}

#[cfg(test)]
mod tests {
    use super::{Nesting, find_param_operator, split_name_subscript, split_top_level};

    /// A subscript belongs to the name: its contents are arithmetic, and `-`, `+`, `%`, `#`, `^`
    /// and `,` are all operators in the table.
    #[test]
    fn a_subscript_is_not_operator_territory() {
        assert_eq!(find_param_operator("a[i-1]"), None);
        assert_eq!(find_param_operator("a[@]"), None);
        assert_eq!(find_param_operator("a[b[0]]"), None);
        // An operator *after* the subscript is still ours.
        assert_eq!(find_param_operator("a[0]#x"), Some((4, "#")));
        assert_eq!(find_param_operator("a[i+1]%%.x"), Some((6, "%%")));
        // An unterminated subscript must not loop; the whole body stays a (bad) name.
        assert_eq!(find_param_operator("a[1"), None);
    }

    #[test]
    fn a_name_splits_from_its_subscript() {
        assert_eq!(split_name_subscript("a[1]"), Some(("a", "1")));
        assert_eq!(split_name_subscript("a[@]"), Some(("a", "@")));
        assert_eq!(split_name_subscript("a[b[0]]"), Some(("a", "b[0]")));
        assert_eq!(split_name_subscript("plain"), None);
        assert_eq!(split_name_subscript("a[1"), None);
    }

    /// The leftmost operator wins, and a longer one beats the shorter it starts with.
    #[test]
    fn the_leftmost_longest_operator_wins() {
        assert_eq!(find_param_operator("v%%.*"), Some((1, "%%")));
        assert_eq!(find_param_operator("v%a:-b"), Some((1, "%")));
        assert_eq!(find_param_operator("v:-1"), Some((1, ":-")));
        assert_eq!(find_param_operator("v: -1"), Some((1, ":")));
        assert_eq!(find_param_operator("v//a/b"), Some((1, "//")));
        assert_eq!(find_param_operator("v/#a/b"), Some((1, "/#")));
        assert_eq!(find_param_operator("v^^"), Some((1, "^^")));
        assert_eq!(find_param_operator("v,,"), Some((1, ",,")));
    }

    /// A name on its own has no operator, and offset 0 is never one: the prefix forms `${#v}`
    /// and `${!v}` are handled before this runs, and `${-}` / `${?}` are special parameters
    /// rather than an operator with an empty name.
    #[test]
    fn a_bare_name_has_no_operator() {
        assert_eq!(find_param_operator("HOME"), None);
        assert_eq!(find_param_operator("12"), None);
        assert_eq!(find_param_operator("-"), None);
        assert_eq!(find_param_operator("?"), None);
        // `${:-x}` does split, at the `-`, leaving an empty name — which the expander rejects
        // as a bad substitution, exactly as bash does. Never silently, which is the point.
        assert_eq!(find_param_operator(":-x"), Some((1, "-")));
    }

    /// An operator belonging to a nested expansion, a quoted run or an escape is not ours.
    #[test]
    fn a_nested_or_quoted_operator_is_skipped() {
        assert_eq!(find_param_operator("a#${b:-c}"), Some((1, "#")));
        assert_eq!(find_param_operator("a#$(x:-y)"), Some((1, "#")));
        assert_eq!(find_param_operator("v#'a-b'"), Some((1, "#")));
    }

    #[test]
    fn a_separator_splits_only_at_the_top_level() {
        let split = |s, sep, n| split_top_level(s, sep, n);
        assert_eq!(split("2:3", ':', Nesting::Arithmetic), ("2", Some("3")));
        assert_eq!(split("2", ':', Nesting::Arithmetic), ("2", None));
        // The second `/` is data: `${v/x//y}` replaces `x` with `/y`.
        assert_eq!(split("x//y", '/', Nesting::Expansion), ("x", Some("/y")));
        assert_eq!(split("x", '/', Nesting::Expansion), ("x", None));
        // A nested expansion carries its own separators.
        assert_eq!(
            split("${a:-1}:${b}", ':', Nesting::Arithmetic),
            ("${a:-1}", Some("${b}"))
        );
        assert_eq!(
            split("${a:-/}/-", '/', Nesting::Expansion),
            ("${a:-/}", Some("-"))
        );
        // And an escaped or quoted separator is data either way.
        assert_eq!(
            split("\\/x/y", '/', Nesting::Expansion),
            ("\\/x", Some("y"))
        );
        assert_eq!(
            split("'a/b'/y", '/', Nesting::Expansion),
            ("'a/b'", Some("y"))
        );
    }

    /// Parentheses nest only for an arithmetic operand; in a glob pattern they are data.
    #[test]
    fn parentheses_nest_only_where_they_are_structural() {
        assert_eq!(
            split_top_level("(a:b):2", ':', Nesting::Arithmetic),
            ("(a:b)", Some("2"))
        );
        assert_eq!(
            split_top_level("(a/b)/c", '/', Nesting::Expansion),
            ("(a", Some("b)/c"))
        );
    }
}
