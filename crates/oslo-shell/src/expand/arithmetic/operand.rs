//! Expansion of the expression *text*, before any of it is read as arithmetic.
//!
//! POSIX makes `$(( … ))` two steps rather than one: the expression first undergoes parameter
//! expansion, command substitution and quote removal, and only the text that survives is
//! arithmetic. Collapsing the two is what made `$(( $(wc -l < f) * 2 ))` abort the whole command —
//! the `$` reached the arithmetic tokeniser, which has no token for it — and what left
//! `$(($1 - 1))` unable to see a positional parameter.
//!
//! The expansion machinery is the word expander's, reused rather than reimplemented, so `${#s}`,
//! backticks and nested `$(( ))` all mean here exactly what they mean in a command word. There is
//! one deliberate exception, and it is the reason this is not a bare call to
//! [`crate::expand::expand_word_to_string`]: a leading `~` is the bitwise-NOT operator in an
//! arithmetic expression, not a home directory.

use crate::env::Environment;
use crate::expand::word::{expand_word_part, field_text};
use crate::lexer::parse_single_word;
use oslo_base::ast::{Word, WordPart};
use oslo_base::error::Result;

/// Run every expansion an arithmetic expression is subject to, yielding the text to tokenise.
pub fn expand_expression(env: &mut Environment, expr: &str) -> Result<String> {
    // An expression built only from operators, digits and names — nearly all of them — has nothing
    // to expand, and handing it to the word lexer could only find new ways to be wrong about it.
    if !expr.contains(['$', '`', '\'', '"', '\\']) {
        return Ok(expr.to_string());
    }

    expand_expression_word(env, &parse_single_word(expr)?)
}

/// [`expand_expression`] for text already lexed as a word: an indexed array's subscript.
pub(crate) fn expand_expression_word(env: &mut Environment, word: &Word) -> Result<String> {
    let mut out = String::new();
    for part in &word.parts {
        out.push_str(&render(env, part)?);
    }
    Ok(out)
}

/// One word part as the text the arithmetic tokeniser should see.
fn render(env: &mut Environment, part: &WordPart) -> Result<String> {
    match part {
        // The word lexer reads a `~` that opens a word as a tilde expansion. Inside `$(( ))` it is
        // bitwise NOT, so `$((~5))` stays `~5` instead of becoming somebody's home directory.
        WordPart::Tilde(user) => return Ok(format!("~{user}")),
        // bash removes double quotes here but not single ones: `$(( "1" + 1 ))` is 2 while
        // `$(( '2' * 3 ))` is a syntax error. Putting the quotes back is how that stays an error —
        // the arithmetic tokeniser has no token for `'`.
        WordPart::SingleQuoted(s) => return Ok(format!("'{s}'")),
        // A backslash keeps only the escapes double quotes have, so `$(( \1 ))` is an error too.
        WordPart::Escaped(s) => {
            return Ok(s
                .chars()
                .map(|c| match c {
                    '$' | '`' | '"' | '\\' => c.to_string(),
                    _ => format!("\\{c}"),
                })
                .collect());
        }
        _ => {}
    }

    let fields = expand_word_part(env, part, false)?;
    // Only `$@` produces more than one field, and there is no field splitting inside `$(( ))`, so
    // the fields are rejoined. Whatever `$(( $@ ))` then means is the arithmetic parser's problem.
    Ok(fields
        .iter()
        .map(|f| field_text(f))
        .collect::<Vec<_>>()
        .join(" "))
}

#[cfg(test)]
mod tests {
    use super::expand_expression;
    use crate::env::Environment;

    fn expand(vars: &[(&str, &str)], expr: &str) -> String {
        let mut env = Environment::new();
        for (k, v) in vars {
            env.set_var(k, v, false);
        }
        expand_expression(&mut env, expr).unwrap_or_else(|e| panic!("{expr}: {e}"))
    }

    /// The common case must not touch the word lexer at all.
    #[test]
    fn an_expression_without_expansions_is_returned_verbatim() {
        for expr in ["1 + 2", "x << 2", "a ? b : c", "~x", "2#101", "", "  "] {
            assert_eq!(expand(&[], expr), expr);
        }
    }

    #[test]
    fn parameters_are_expanded_before_the_expression_is_scanned() {
        assert_eq!(expand(&[("x", "41")], "$x + 1"), "41 + 1");
        assert_eq!(expand(&[("x", "41")], "${x} + 1"), "41 + 1");
        assert_eq!(expand(&[("s", "abcde")], "${#s} * 2"), "5 * 2");
        // An unset parameter is empty text, which the arithmetic parser then rejects on its own
        // terms rather than silently reading as 0.
        assert_eq!(expand(&[], "$nosuch"), "");
    }

    #[test]
    fn a_default_payload_still_works_inside_an_expression() {
        assert_eq!(expand(&[], "${n:-7} + 1"), "7 + 1");
    }

    /// A nested `$(( ))` is evaluated by the word expander and splices its result back in.
    #[test]
    fn nested_arithmetic_is_evaluated_first() {
        assert_eq!(expand(&[], "$((1 + 2)) * 3"), "3 * 3");
    }

    /// bash removes double quotes from an arithmetic expression but not single quotes:
    /// `$(( "1" + 1 ))` is 2 and `$(( '2' * 3 ))` is a syntax error. Both halves are deliberate.
    #[test]
    fn double_quotes_are_removed_and_single_quotes_are_not() {
        assert_eq!(expand(&[("x", "4")], "\"$x\" + 1"), "4 + 1");
        assert_eq!(expand(&[], "'2' + 1"), "'2' + 1");
    }

    /// `~` opens a word, and the word lexer would otherwise turn `$((~5))` into a home directory.
    #[test]
    fn a_leading_tilde_stays_the_bitwise_not_operator() {
        assert_eq!(expand(&[("x", "5")], "~$x"), "~5");
        assert_eq!(expand(&[("x", "5")], "~x + $x"), "~x + 5");
    }
}
