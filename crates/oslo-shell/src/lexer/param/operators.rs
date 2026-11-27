//! What a `${…}` operator *means*.
//!
//! The counterpart to `scan`, which only finds where an operator is: given the operator text and
//! the argument after it, this builds the [`ParamExpansion`] the expander will act on. Kept apart
//! so the positional hazards (nesting, quoting, the leftmost-longest rule) stay in one file and
//! the operator table stays in another.

use super::Context;
use super::scan::{Nesting, split_top_level};
use crate::lexer::quoting::{parse_operand, parse_word_source};
use oslo_base::ast::{ParamExpansion, ReplaceScope, Word};
use oslo_base::error::Result;

/// The `word` of `${x-word}`, `${x+word}` and `${x?word}`, lexed for the context it sits in.
///
/// **Inside double quotes the payload's own quotes are not quotes.** `echo "${x-'q'}"` is `'q'` in
/// bash and was `q` here; `"${x:-a\ b}"` kept its backslash there and lost it here; `"${x:-~}"` was
/// a home directory here and a tilde there. The unquoted spellings already agreed, so only the
/// double-quoted uses were wrong, and silently.
///
/// A nested *double* quote is still a quote, which is not a detail: `"${1+"$@"}"` is the pre-POSIX
/// way to forward an argument list, and treating that `"` as literal joins the arguments into one.
///
/// This applies to the payload operators only. A *pattern* and a *replacement* process their quotes
/// in either context, which is bash's rule and is why `"${v#'a'}"` still strips the `a`.
fn payload(rest: &str, ctx: Context) -> Result<Word> {
    parse_operand(rest, ctx.in_double_quotes, ctx.tilde)
}

/// Build the expansion for `op` from the text that follows it.
///
/// Every operand stays a [`oslo_base::ast::Word`]: `${x:-$HOME}` has to expand its default, and
/// `${v/$sep/-}` has to expand its pattern, both at use time rather than here.
///
/// `ctx` says whether the whole `${…}` was written inside double quotes, and which tilde rule a
/// payload follows; both change only a payload — see [`payload`].
pub(super) fn operator_expansion(op: &str, rest: &str, ctx: Context) -> Result<ParamExpansion> {
    Ok(match op {
        // A substring operand is arithmetic, so parentheses nest and `${v:(-1)}` splits correctly.
        ":" => {
            let (offset, length) = split_top_level(rest, ':', Nesting::Arithmetic);
            ParamExpansion::Substring {
                offset: parse_word_source(offset)?,
                length: length.map(parse_word_source).transpose()?,
            }
        }

        // `${v/pat}` with no second `/` deletes the match, so an absent replacement is empty
        // rather than an error. Parentheses are ordinary pattern data here, unlike above.
        "/" | "//" | "/#" | "/%" => {
            let (pattern, replacement) = split_top_level(rest, '/', Nesting::Expansion);
            ParamExpansion::Replace {
                pattern: parse_word_source(pattern)?,
                replacement: parse_word_source(replacement.unwrap_or(""))?,
                scope: match op {
                    "//" => ReplaceScope::All,
                    "/#" => ReplaceScope::Prefix,
                    "/%" => ReplaceScope::Suffix,
                    _ => ReplaceScope::First,
                },
            }
        }

        // The optional operand selects which characters are eligible; absent means all of them.
        "^" | "^^" | "," | ",," => ParamExpansion::CaseConvert {
            pattern: if rest.is_empty() {
                None
            } else {
                Some(parse_word_source(rest)?)
            },
            upper: op.starts_with('^'),
            all: op.chars().count() == 2,
        },

        _ => {
            // A leading `:` is not a different operator, only a different notion of "absent":
            // the `:` forms treat a set-but-empty parameter as unset, the colon-less ones do not.
            let test_null = op.starts_with(':');
            let bare = op.strip_prefix(':').unwrap_or(op);
            // **Only the payload operators take the quoting of their context.** `#`, `##`, `%` and
            // `%%` share this arm and are *patterns*, which process their own quotes in either
            // context — `"${v#'a'}"` strips the `a` in bash. See [`payload`].
            let arg = match bare {
                "-" | "=" | "+" | "?" => payload(rest, ctx)?,
                _ => parse_word_source(rest)?,
            };
            match bare {
                "-" => ParamExpansion::DefaultValue {
                    default: arg,
                    assign_if_unset: false,
                    test_null,
                },
                "=" => ParamExpansion::DefaultValue {
                    default: arg,
                    assign_if_unset: true,
                    test_null,
                },
                "+" => ParamExpansion::UseAlternative {
                    alternative: arg,
                    test_null,
                },
                "?" => ParamExpansion::ErrorIfUnset {
                    message: arg,
                    test_null,
                },
                "%%" => ParamExpansion::RemoveSuffix {
                    pattern: arg,
                    longest: true,
                },
                "%" => ParamExpansion::RemoveSuffix {
                    pattern: arg,
                    longest: false,
                },
                "##" => ParamExpansion::RemovePrefix {
                    pattern: arg,
                    longest: true,
                },
                _ => ParamExpansion::RemovePrefix {
                    pattern: arg,
                    longest: false,
                },
            }
        }
    })
}
