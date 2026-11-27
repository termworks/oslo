//! Parsing the body of a `${…}` parameter expansion.
//!
//! The lexer hands this module the raw text between `${` and its matching `}` and gets back the
//! [`WordPart::Variable`] it stands for. Splitting the body is the whole job, and it is harder
//! than it looks for three reasons.
//!
//! *The operator is the leftmost one, not the first one a search order happens to find.*
//! `${v%a:-b}` strips a suffix; it has no default. And the search has to skip nested expansions,
//! quotes and backslashes, so `${a#${b:-c}}` splits on the `#` rather than cutting the name in
//! half mid-`${`.
//!
//! *A colon is two different operators.* `${v:-d}` is a default and `${v:2:3}` is a substring, so
//! the `:-` family has to be tried before a bare `:`. That is also why bash needs `${v: -1}` or
//! `${v:(-1)}` for a negative offset — and why this module needs nothing extra to support them.
//!
//! *An unrecognised body must stay an error.* Anything this module cannot split is passed through
//! as a parameter *name*, and `crate::expand::param` rejects a name that is not one. The silent
//! empty string that fallback used to produce is what hid nine unimplemented operators.

use super::TildeMode;
use super::quoting::parse_word_source;
use operators::operator_expansion;
use oslo_base::ast::{ParamExpansion, Subscript, WordPart};
use oslo_base::error::Result;
use scan::{find_param_operator, split_name_subscript};

mod operators;
mod scan;

/// Turn the raw body of a `${…}` into the word part it denotes.
///
/// `tilde` is the tilde rule a `${x-word}` payload is lexed under.
pub(super) fn parse_braced_body(
    content: &str,
    in_double_quotes: bool,
    tilde: TildeMode,
) -> Result<WordPart> {
    let ctx = Context {
        in_double_quotes,
        tilde,
    };
    if let Some(part) = prefix_form(content, ctx)? {
        return Ok(part);
    }

    let Some((idx, op)) = find_param_operator(content) else {
        return reference(content, ParamExpansion::Normal);
    };
    // `${x:}` has no offset at all, which bash reports as a bad substitution.
    if op == ":" && idx + 1 == content.len() {
        return reference(content, ParamExpansion::Normal);
    }

    reference(
        &content[..idx],
        operator_expansion(op, &content[idx + op.len()..], ctx)?,
    )
}

/// What an operand's lexing depends on besides its own text.
#[derive(Clone, Copy)]
pub(super) struct Context {
    /// Whether the `${…}` was written inside double quotes.
    pub in_double_quotes: bool,
    /// The tilde rule of a payload.
    pub tilde: TildeMode,
}

/// Build the reference `name` denotes, which is an array element whenever it has a subscript.
///
/// The one place `a[1]` stops being a name: it used to be handed to the expander whole, which
/// looked up a *variable literally called* `a[1]` — so `m[x]=1; echo ${m[x]}` appeared to work
/// only because the assignment had created a variable of that same odd name.
fn reference(name: &str, expansion_type: ParamExpansion) -> Result<WordPart> {
    let Some((name, subscript)) = split_name_subscript(name) else {
        return Ok(WordPart::Variable {
            name: name.to_string(),
            expansion_type,
        });
    };
    Ok(WordPart::ArrayRef {
        name: name.to_string(),
        subscript: parse_subscript(subscript)?,
        expansion_type,
    })
}

/// `[@]`, `[*]`, or an arithmetic index.
fn parse_subscript(text: &str) -> Result<Subscript> {
    Ok(match text {
        "@" => Subscript::All,
        "*" => Subscript::Joined,
        _ => Subscript::Index(parse_word_source(text)?),
    })
}

/// The two operators that sit *before* the name: `${#name}` and `${!name}`.
///
/// A lone `#` or `!` is not one of them. `${#}` is `$#` and `${!}` is `$!`, plain references to a
/// special parameter, so the marker only counts when a name follows it.
///
/// `${!prefix*}` — bash's name-listing form — reaches here as the name `prefix*`, which is not a
/// parameter name and so becomes a `bad substitution` error rather than a wrong answer. `${!a[@]}`
/// does *not*: a subscripted `!` is the list of indices in use, which the expander implements.
fn prefix_form(content: &str, ctx: Context) -> Result<Option<WordPart>> {
    let mut chars = content.chars();
    let expansion_type = match chars.next() {
        Some('#') => ParamExpansion::Length,
        Some('!') => return indirect_form(chars.as_str(), ctx).map(Some),
        _ => return Ok(None),
    };
    let name = chars.as_str();
    if name.is_empty() {
        return Ok(None);
    }
    reference(name, expansion_type).map(Some)
}

/// `${!name}` and `${!name<op>word}`.
///
/// **The operator is split off the remainder, not left in the name.** Without this the whole of
/// `!v:-d` was carried through as a parameter name, which is not one — so every combined form
/// answered `bad substitution`, including the `${!hooksSlice+…}` that stdenv's `runHook` is built
/// on. What remains after the `!` is an ordinary body, so it is parsed as one.
fn indirect_form(body: &str, ctx: Context) -> Result<WordPart> {
    if body.is_empty() {
        // `${!}` is the last background pid, not an indirection.
        return reference("!", ParamExpansion::Normal);
    }
    let inner = match find_param_operator(body) {
        Some((idx, op)) => {
            let expansion = operator_expansion(op, &body[idx + op.len()..], ctx)?;
            return reference(&body[..idx], ParamExpansion::Indirect(Box::new(expansion)));
        }
        None => ParamExpansion::Normal,
    };
    reference(body, ParamExpansion::Indirect(Box::new(inner)))
}

#[cfg(test)]
mod tests {
    use crate::lexer::{Lexer, Token};
    use oslo_base::ast::{ParamExpansion, ReplaceScope, Subscript, Word, WordPart};

    fn one_part(src: &str) -> WordPart {
        let mut parts = match Lexer::new(src).next() {
            Ok(Token::Word(w)) => w.parts,
            other => panic!("expected a word from {src:?}, got {other:?}"),
        };
        assert_eq!(parts.len(), 1, "expected one part from {src:?}");
        parts.remove(0)
    }

    /// The `expansion_type` of a `${…}`, with the name checked along the way.
    fn op_of(src: &str, expect_name: &str) -> ParamExpansion {
        let WordPart::Variable {
            name,
            expansion_type,
        } = one_part(src)
        else {
            panic!("expected a variable from {src:?}");
        };
        assert_eq!(name, expect_name, "name from {src:?}");
        expansion_type
    }

    fn lit(s: &str) -> Word {
        Word::from_literal(s)
    }

    // --- R2.6: brace depth, quote state, and the leftmost operator ---

    /// `${x:-${y}}` used to stop at the first `}`, leaving `}` behind as literal text.
    #[test]
    fn a_braced_payload_may_contain_a_braced_expansion() {
        assert_eq!(
            op_of("${x:-${y}}", "x"),
            ParamExpansion::DefaultValue {
                default: Word {
                    parts: vec![WordPart::Variable {
                        name: "y".into(),
                        expansion_type: ParamExpansion::Normal,
                    }]
                },
                assign_if_unset: false,
                test_null: true,
            }
        );
    }

    /// The operator search has to skip nested expansions: the `:-` here belongs to the inner
    /// `${b:-c}`, so splitting on it would cut the name off mid-`${`.
    #[test]
    fn a_nested_operator_does_not_win() {
        assert!(matches!(
            op_of("${a#${b:-c}}", "a"),
            ParamExpansion::RemovePrefix { longest: false, .. }
        ));
    }

    /// `%%` beats `%`, and the leftmost operator beats a later one of a different kind.
    #[test]
    fn the_leftmost_longest_operator_wins() {
        assert!(matches!(
            op_of("${v%%.*}", "v"),
            ParamExpansion::RemoveSuffix { longest: true, .. }
        ));
        assert!(matches!(
            op_of("${v%a:-b}", "v"),
            ParamExpansion::RemoveSuffix { longest: false, .. }
        ));
    }

    /// A `}` inside quotes, or inside a nested substitution, is data.
    #[test]
    fn a_quoted_brace_does_not_close_the_expansion() {
        assert_eq!(
            op_of("${x:-'a}b'}", "x"),
            ParamExpansion::DefaultValue {
                default: Word {
                    parts: vec![WordPart::SingleQuoted("a}b".into())]
                },
                assign_if_unset: false,
                test_null: true,
            }
        );
        assert!(matches!(
            op_of("${x:-$(echo })}", "x"),
            ParamExpansion::DefaultValue { .. }
        ));
    }

    /// The payload is a word, not text: `${x:-$HOME}` has to expand later, not print `$HOME`.
    #[test]
    fn a_payload_is_parsed_as_a_word() {
        let ParamExpansion::DefaultValue { default, .. } = op_of("${x:-$(pwd)/sub}", "x") else {
            panic!("expected a default-value expansion");
        };
        assert_eq!(
            default.parts,
            vec![
                WordPart::CommandSubstitution("pwd".into()),
                WordPart::Literal("/sub".into()),
            ]
        );
    }

    // --- R2.4: the forms the lexer used to drop ---

    /// The prefix operators, and the lone `#`/`!` that are special parameters instead.
    #[test]
    fn the_prefix_operators_need_a_name_after_them() {
        assert_eq!(op_of("${#v}", "v"), ParamExpansion::Length);
        assert_eq!(
            op_of("${!v}", "v"),
            ParamExpansion::Indirect(Box::new(ParamExpansion::Normal))
        );
        // `${#}` is `$#` and `${!}` is `$!` — a marker with nothing after it is the parameter.
        assert_eq!(op_of("${#}", "#"), ParamExpansion::Normal);
        assert_eq!(op_of("${!}", "!"), ParamExpansion::Normal);
        assert_eq!(op_of("${#@}", "@"), ParamExpansion::Length);
    }

    /// `${!prefix*}` is bash's name-listing form, which oslo does not implement. It must reach
    /// the expander as a non-name so it errors, not as something that quietly expands.
    #[test]
    fn the_name_listing_form_is_left_as_a_bad_name() {
        assert_eq!(
            op_of("${!pre*}", "pre*"),
            ParamExpansion::Indirect(Box::new(ParamExpansion::Normal))
        );
    }

    #[test]
    fn the_colon_less_forms_only_test_for_unset() {
        assert_eq!(
            op_of("${v-d}", "v"),
            ParamExpansion::DefaultValue {
                default: lit("d"),
                assign_if_unset: false,
                test_null: false,
            }
        );
        assert_eq!(
            op_of("${v=d}", "v"),
            ParamExpansion::DefaultValue {
                default: lit("d"),
                assign_if_unset: true,
                test_null: false,
            }
        );
        assert_eq!(
            op_of("${v+s}", "v"),
            ParamExpansion::UseAlternative {
                alternative: lit("s"),
                test_null: false,
            }
        );
        assert_eq!(
            op_of("${v?e}", "v"),
            ParamExpansion::ErrorIfUnset {
                message: lit("e"),
                test_null: false,
            }
        );
    }

    /// The `:` family still wins over the bare `:`, which is what makes `${v:-1}` a default and
    /// not an offset of -1.
    #[test]
    fn the_colon_family_beats_the_bare_colon() {
        assert!(matches!(
            op_of("${v:-1}", "v"),
            ParamExpansion::DefaultValue { .. }
        ));
        assert!(matches!(
            op_of("${v:=1}", "v"),
            ParamExpansion::DefaultValue {
                assign_if_unset: true,
                ..
            }
        ));
        assert!(matches!(
            op_of("${v:+1}", "v"),
            ParamExpansion::UseAlternative { .. }
        ));
        assert!(matches!(
            op_of("${v:?1}", "v"),
            ParamExpansion::ErrorIfUnset { .. }
        ));
    }

    #[test]
    fn substring_splits_offset_from_length() {
        assert_eq!(
            op_of("${v:2:3}", "v"),
            ParamExpansion::Substring {
                offset: lit("2"),
                length: Some(lit("3")),
            }
        );
        assert_eq!(
            op_of("${v:2}", "v"),
            ParamExpansion::Substring {
                offset: lit("2"),
                length: None,
            }
        );
    }

    /// A negative offset needs a space or parens, exactly as in bash, because `:-` is a default.
    #[test]
    fn a_negative_offset_needs_a_space_or_parens() {
        assert_eq!(
            op_of("${v: -3}", "v"),
            ParamExpansion::Substring {
                offset: lit(" -3"),
                length: None,
            }
        );
        assert_eq!(
            op_of("${v:(-3):2}", "v"),
            ParamExpansion::Substring {
                offset: lit("(-3)"),
                length: Some(lit("2")),
            }
        );
    }

    /// A `:` inside a nested expansion belongs to it, not to the substring split.
    #[test]
    fn substring_operands_may_nest() {
        assert_eq!(
            op_of("${v:${a:-1}:$((b+1))}", "v"),
            ParamExpansion::Substring {
                offset: Word {
                    parts: vec![WordPart::Variable {
                        name: "a".into(),
                        expansion_type: ParamExpansion::DefaultValue {
                            default: lit("1"),
                            assign_if_unset: false,
                            test_null: true,
                        },
                    }]
                },
                length: Some(Word {
                    parts: vec![WordPart::Arithmetic("b+1".into())]
                }),
            }
        );
    }

    #[test]
    fn replacement_covers_every_scope() {
        let scopes = [
            ("${v/a/b}", ReplaceScope::First),
            ("${v//a/b}", ReplaceScope::All),
            ("${v/#a/b}", ReplaceScope::Prefix),
            ("${v/%a/b}", ReplaceScope::Suffix),
        ];
        for (src, want) in scopes {
            assert_eq!(
                op_of(src, "v"),
                ParamExpansion::Replace {
                    pattern: lit("a"),
                    replacement: lit("b"),
                    scope: want,
                }
            );
        }
    }

    /// Only the *first* separator splits: a later `/` is part of the replacement text.
    #[test]
    fn a_slash_in_the_replacement_is_data() {
        assert_eq!(
            op_of("${v/x//y}", "v"),
            ParamExpansion::Replace {
                pattern: lit("x"),
                replacement: lit("/y"),
                scope: ReplaceScope::First,
            }
        );
    }

    /// With no second `/` the match is deleted, so the replacement is empty rather than absent.
    #[test]
    fn a_missing_replacement_is_empty() {
        assert_eq!(
            op_of("${v/x}", "v"),
            ParamExpansion::Replace {
                pattern: lit("x"),
                replacement: Word::default(),
                scope: ReplaceScope::First,
            }
        );
    }

    /// The pattern is a word, so `${v/$sep/-}` searches for the *value* of `sep`, and a `/`
    /// inside a nested expansion does not split the operands.
    #[test]
    fn a_replacement_pattern_is_a_word() {
        assert_eq!(
            op_of("${v/${a:-/}/-}", "v"),
            ParamExpansion::Replace {
                pattern: Word {
                    parts: vec![WordPart::Variable {
                        name: "a".into(),
                        expansion_type: ParamExpansion::DefaultValue {
                            default: lit("/"),
                            assign_if_unset: false,
                            test_null: true,
                        },
                    }]
                },
                replacement: lit("-"),
                scope: ReplaceScope::First,
            }
        );
    }

    #[test]
    fn case_conversion_reads_both_directions_and_its_selector() {
        let cases = [
            ("${v^}", true, false),
            ("${v^^}", true, true),
            ("${v,}", false, false),
            ("${v,,}", false, true),
        ];
        for (src, upper, all) in cases {
            assert_eq!(
                op_of(src, "v"),
                ParamExpansion::CaseConvert {
                    pattern: None,
                    upper,
                    all,
                }
            );
        }
        assert_eq!(
            op_of("${v^^[aeiou]}", "v"),
            ParamExpansion::CaseConvert {
                pattern: Some(lit("[aeiou]")),
                upper: true,
                all: true,
            }
        );
    }

    /// The property the whole module protects: a body it cannot split stays a *name*, so the
    /// expander rejects it.
    #[test]
    fn an_unknown_operator_is_left_as_a_bad_name() {
        assert_eq!(op_of("${a b}", "a b"), ParamExpansion::Normal);
        assert_eq!(op_of("${x:}", "x:"), ParamExpansion::Normal);
    }

    /// `@` takes one letter, and `~` toggles case.
    #[test]
    fn transformations_and_toggles_parse() {
        assert_eq!(op_of("${v@Q}", "v"), ParamExpansion::Transform('Q'));
        assert_eq!(op_of("${v@QQ}", "v"), ParamExpansion::Transform('\0'));
        assert_eq!(op_of("${@@Q}", "@"), ParamExpansion::Transform('Q'));
        assert_eq!(
            op_of("${v~~}", "v"),
            ParamExpansion::CaseToggle {
                pattern: None,
                all: true
            }
        );
    }

    // --- R8.1: array subscripts ---

    /// The name, subscript and operator of a `${name[sub]…}`.
    fn ref_of(src: &str) -> (String, Subscript, ParamExpansion) {
        match one_part(src) {
            WordPart::ArrayRef {
                name,
                subscript,
                expansion_type,
            } => (name, subscript, expansion_type),
            other => panic!("expected an array reference from {src:?}, got {other:?}"),
        }
    }

    #[test]
    fn a_subscript_splits_from_the_name() {
        assert_eq!(
            ref_of("${a[1]}"),
            (
                "a".into(),
                Subscript::Index(lit("1")),
                ParamExpansion::Normal
            )
        );
        assert_eq!(
            ref_of("${a[@]}"),
            ("a".into(), Subscript::All, ParamExpansion::Normal)
        );
        assert_eq!(
            ref_of("${a[*]}"),
            ("a".into(), Subscript::Joined, ParamExpansion::Normal)
        );
    }

    /// `${#a[@]}` is the element count and `${!a[@]}` the index list, so both prefix operators
    /// have to survive the split rather than swallowing the subscript into the name.
    #[test]
    fn the_prefix_operators_keep_the_subscript() {
        assert_eq!(
            ref_of("${#a[@]}"),
            ("a".into(), Subscript::All, ParamExpansion::Length)
        );
        assert_eq!(
            ref_of("${!a[@]}"),
            (
                "a".into(),
                Subscript::All,
                ParamExpansion::Indirect(Box::new(ParamExpansion::Normal))
            )
        );
    }

    /// The subscript is arithmetic, and arithmetic is full of characters the operator table also
    /// claims. `${a[i-1]}` is one element, not `${a[i}` defaulted to `1]`.
    #[test]
    fn arithmetic_inside_a_subscript_is_not_an_operator() {
        let (name, subscript, op) = ref_of("${a[i-1]}");
        assert_eq!(name, "a");
        assert_eq!(subscript, Subscript::Index(lit("i-1")));
        assert_eq!(op, ParamExpansion::Normal);
    }

    /// An operator after the subscript still applies to the element.
    #[test]
    fn an_operator_after_a_subscript_still_parses() {
        let (name, subscript, op) = ref_of("${a[0]:-d}");
        assert_eq!(name, "a");
        assert_eq!(subscript, Subscript::Index(lit("0")));
        assert!(matches!(op, ParamExpansion::DefaultValue { .. }));
    }

    /// An unterminated subscript is not a subscript: it stays part of the name, which the
    /// expander then rejects instead of guessing at what was meant.
    #[test]
    fn an_unterminated_subscript_stays_a_bad_name() {
        assert_eq!(op_of("${a[1}", "a[1"), ParamExpansion::Normal);
    }
}
