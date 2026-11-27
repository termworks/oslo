//! Applying a `${…}` operator to the value it was given.
//!
//! Split from the module that *finds* the value so that a scalar and an array element share one
//! implementation of every operator: `${v:-d}` and `${a[0]:-d}` differ only in where the value
//! came from and, for the assigning forms, where it goes back to. That is what [`Target`] carries.
//!
//! Two operators mean something different when the reference stands for a *list* — `${a[@]}`,
//! `$@`, `$*`. Slicing selects elements ([`slice_list`]) and the pattern operators rewrite each
//! element ([`map_elements`]); neither touches the string those elements would join into. Both
//! live here, once, so that the array path and the positional path cannot drift apart the way
//! they did when `"${@:2}"` character-sliced the space-joined positionals.

use super::{Target, pattern};
use crate::env::Environment;
use crate::expand::arithmetic::eval_arithmetic;
use crate::expand::glob::ShellPattern;
use crate::expand::word::{expand_word_to_pattern, expand_word_to_string};
use oslo_base::ast::{ParamExpansion, ReplaceScope, Word};
use oslo_base::error::{Result, ShellError};

/// The single string a `${…}` reference stands for, once its value is known.
pub(super) fn apply(
    env: &mut Environment,
    target: &Target<'_>,
    expansion_type: &ParamExpansion,
) -> Result<String> {
    let name = target.display();
    let val = target.value(env);
    let text = match expansion_type {
        ParamExpansion::Normal => val.unwrap_or_default(),

        // `${#@}` and `${#*}` count the positionals rather than measuring the string they would
        // have joined into. `${#}` never reaches here: it is `$#`, a plain reference.
        ParamExpansion::Length => match target {
            Target::Param("@" | "*") => env.get_positional().len().to_string(),
            // Characters, not bytes — `${#v}` on a UTF-8 value must not report its encoding.
            _ => val.map_or_else(|| "0".to_string(), |v| v.chars().count().to_string()),
        },

        ParamExpansion::DefaultValue {
            default,
            assign_if_unset,
            test_null,
        } => {
            if is_present(&val, *test_null) {
                val.unwrap_or_default()
            } else {
                let text = expand_word_to_string(env, default)?;
                // Assigning the *expanded* text is the whole point: `${x:=$y}` used to persist
                // the two characters `$y` into `x`.
                if *assign_if_unset {
                    target.assign(env, &text)?;
                }
                text
            }
        }

        ParamExpansion::UseAlternative {
            alternative,
            test_null,
        } => {
            if is_present(&val, *test_null) {
                expand_word_to_string(env, alternative)?
            } else {
                String::new()
            }
        }

        ParamExpansion::ErrorIfUnset { message, test_null } => {
            if is_present(&val, *test_null) {
                val.unwrap_or_default()
            } else {
                let msg = expand_word_to_string(env, message)?;
                let msg = if msg.is_empty() {
                    "parameter null or not set".to_string()
                } else {
                    msg
                };
                return Err(ShellError::UnsetParameter(format!("{name}: {msg}")));
            }
        }

        ParamExpansion::RemovePrefix {
            pattern: word,
            longest,
        } => {
            let pat = compile_pattern(env, word)?;
            let value = val.unwrap_or_default();
            pattern::remove_prefix(&value, &pat, *longest)
        }

        ParamExpansion::RemoveSuffix {
            pattern: word,
            longest,
        } => {
            let pat = compile_pattern(env, word)?;
            let value = val.unwrap_or_default();
            pattern::remove_suffix(&value, &pat, *longest)
        }

        ParamExpansion::Substring { offset, length } => {
            let start = eval_operand(env, offset)?;
            let count = match length {
                Some(word) => Some(eval_operand(env, word)?),
                None => None,
            };
            let value = val.unwrap_or_default();
            pattern::substring(&value, start, count)
                .map_err(|n| ShellError::ExpansionError(format!("{n}: substring expression < 0")))?
        }

        ParamExpansion::Replace {
            pattern: pat_word,
            replacement,
            scope,
        } => {
            let pat = compile_pattern(env, pat_word)?;
            // Runs rather than a string: an unquoted `&` stands for the matched text and a quoted
            // one does not, and nothing on the finished string can tell them apart.
            let rep = pattern::Replacement::from_runs(&expand_word_to_pattern(env, replacement)?);
            let value = val.unwrap_or_default();
            match scope {
                ReplaceScope::First => pattern::replace(&value, &pat, &rep, false),
                ReplaceScope::All => pattern::replace(&value, &pat, &rep, true),
                ReplaceScope::Prefix => pattern::replace_prefix(&value, &pat, &rep),
                ReplaceScope::Suffix => pattern::replace_suffix(&value, &pat, &rep),
            }
        }

        ParamExpansion::CaseConvert {
            pattern: selector,
            upper,
            all,
        } => {
            let selector = match selector {
                Some(word) => Some(compile_pattern(env, word)?),
                None => None,
            };
            let value = val.unwrap_or_default();
            pattern::convert_case(&value, selector.as_ref(), *upper, *all)
        }

        // `${!name}` reads `name`'s value and expands *that* parameter. Only the second lookup
        // may come up empty: bash makes a `name` that does not *hold a name* a fatal expansion
        // error, and it names a different culprit depending on which step failed.
        ParamExpansion::Indirect(inner) => match val {
            // **An unset positional is empty, not an error** — `${!1}` in a function called with
            // no arguments. bash draws the line here and not at "unset": a *variable* that does
            // not exist is a mistake worth reporting, and a positional that was simply not passed
            // is the ordinary case every hook helper is written around. stdenv's `runHook`,
            // `runPhase` and `substituteStream` all reach this on their first line.
            None if name.chars().all(|c| c.is_ascii_digit()) => String::new(),
            None => {
                return Err(ShellError::ExpansionError(format!(
                    "{name}: invalid indirect expansion"
                )));
            }
            // **A subscript is allowed through, because bash allows it and stdenv depends on it.**
            // `runHook` builds the string `"${hookName%Hook}Hooks[@]"` and expands *that* — so the
            // thing an indirection names is an array reference as often as it is a plain name, and
            // rejecting it here is what left every dev shell hook unreachable.
            Some(indirect) if array_reference(&indirect) => {
                return crate::expand::param::expand_indirect_array(env, &indirect, inner);
            }
            // An empty value lands here too, which is why the message quotes nothing.
            Some(indirect) if !super::is_param_name(&indirect) => {
                return Err(ShellError::ExpansionError(format!(
                    "{indirect}: invalid variable name"
                )));
            }
            // **Applied to the second parameter, not the first.** `${!v:-d}` asks whether the
            // thing `v` names is set, so the operator has to run against that one — testing `v`
            // instead would answer about the pointer rather than the target.
            Some(indirect) => {
                if env.nounset()
                    && !defines_absence(inner)
                    && env.get_param(&indirect).is_none()
                {
                    return Err(ShellError::UnsetParameter(format!(
                        "!{name}: unbound variable"
                    )));
                }
                return apply(env, &Target::Param(&indirect), inner);
            }
        },
    };

    Ok(text)
}

/// Expand an operator's pattern operand and compile it, quoting and all.
///
/// The quoting has to survive: `${v#"$prefix"}` is how a script strips a *literal* prefix, and
/// flattening the operand to a string first turned every `*` a variable happened to contain into
/// a metacharacter.
fn compile_pattern(env: &mut Environment, word: &Word) -> Result<ShellPattern> {
    Ok(crate::expand::glob::pattern_from_runs(
        &expand_word_to_pattern(env, word)?,
    ))
}

/// Whether the operator says what an unset parameter means, which exempts it from `set -u`.
fn defines_absence(expansion_type: &ParamExpansion) -> bool {
    matches!(
        expansion_type,
        ParamExpansion::DefaultValue { .. }
            | ParamExpansion::UseAlternative { .. }
            | ParamExpansion::ErrorIfUnset { .. }
    )
}

/// Whether the parameter counts as "set" for the `${x-d}` family.
///
/// The `:` forms treat a set-but-empty parameter as absent, the colon-less ones test only for
/// unset — which is the entire difference between `x=; ${x:-d}` (`d`) and `x=; ${x-d}` (empty).
pub(super) fn is_present(val: &Option<String>, test_null: bool) -> bool {
    match val {
        Some(v) => !(test_null && v.is_empty()),
        None => false,
    }
}

/// Does this operator rewrite *each element* of a list, rather than the list as a whole?
///
/// `${a[@]#pat}` strips the prefix from every element and `${@^^}` upper-cases every positional;
/// the answer is a list of the same length. Slicing is the other list-valued form and deliberately
/// not here — it selects elements instead of rewriting them, so it has its own entry point.
pub(super) fn is_elementwise(expansion_type: &ParamExpansion) -> bool {
    matches!(
        expansion_type,
        ParamExpansion::RemovePrefix { .. }
            | ParamExpansion::RemoveSuffix { .. }
            | ParamExpansion::Replace { .. }
            | ParamExpansion::CaseConvert { .. }
    )
}

/// Apply an element-wise operator to every value, keeping the list's length.
///
/// # The pattern is expanded once, not once per element
///
/// Each element used to go through [`apply`], which expands the operator's *pattern* — and the
/// pattern does not depend on the element it is matched against. With `$@` holding five positionals
/// and a pattern that is itself an expansion, that is five expansions per level; nest it twelve
/// deep and it is 5¹². The fuzzer found it as a timeout, and the measurement is not subtle:
///
/// ```text
/// set -- 1 2 3 4 5; : $(( ${@/${@/${@/…/x}/x}/x} ))   twelve levels
///   bash   0.007 s
///   oslo  >20     s      before this; 0.004 s after
/// ```
///
/// So the words are expanded here, once, and the elements are mapped over what came out. Nothing
/// about the result changes — this is the same pattern applied to the same values.
pub(super) fn map_elements(
    env: &mut Environment,
    values: &[String],
    expansion_type: &ParamExpansion,
) -> Result<Vec<String>> {
    debug_assert!(
        is_elementwise(expansion_type),
        "map_elements is only defined for the operators is_elementwise admits"
    );
    let prepared = Prepared::compile(env, expansion_type)?;
    values
        .iter()
        .map(|value| Ok(prepared.apply(value)))
        .collect()
}

/// An element-wise operator with its words already expanded.
///
/// Built once per list by [`map_elements`]; see the note there for what that is worth.
enum Prepared {
    RemovePrefix(ShellPattern, bool),
    RemoveSuffix(ShellPattern, bool),
    Replace(ShellPattern, pattern::Replacement, ReplaceScope),
    CaseConvert(Option<ShellPattern>, bool, bool),
}

impl Prepared {
    fn compile(env: &mut Environment, expansion_type: &ParamExpansion) -> Result<Prepared> {
        Ok(match expansion_type {
            ParamExpansion::RemovePrefix { pattern, longest } => {
                Prepared::RemovePrefix(compile_pattern(env, pattern)?, *longest)
            }
            ParamExpansion::RemoveSuffix { pattern, longest } => {
                Prepared::RemoveSuffix(compile_pattern(env, pattern)?, *longest)
            }
            ParamExpansion::Replace {
                pattern,
                replacement,
                scope,
            } => Prepared::Replace(
                compile_pattern(env, pattern)?,
                // See the note at the other Replace site: quoting decides whether `&` is the
                // matched text, and only the runs still carry it.
                pattern::Replacement::from_runs(&expand_word_to_pattern(env, replacement)?),
                *scope,
            ),
            ParamExpansion::CaseConvert {
                pattern,
                upper,
                all,
            } => Prepared::CaseConvert(
                match pattern {
                    Some(word) => Some(compile_pattern(env, word)?),
                    None => None,
                },
                *upper,
                *all,
            ),
            // Unreachable: `map_elements` asserts what it was given, and `is_elementwise` admits
            // exactly the four above. Answering with the value unchanged is what a fifth operator
            // arriving here should do — not a panic in the middle of somebody's expansion.
            _ => Prepared::CaseConvert(None, false, false),
        })
    }

    fn apply(&self, value: &str) -> String {
        match self {
            Prepared::RemovePrefix(pattern, longest) => {
                pattern::remove_prefix(value, pattern, *longest)
            }
            Prepared::RemoveSuffix(pattern, longest) => {
                pattern::remove_suffix(value, pattern, *longest)
            }
            Prepared::Replace(pattern, replacement, scope) => match scope {
                ReplaceScope::First => pattern::replace(value, pattern, replacement, false),
                ReplaceScope::All => pattern::replace(value, pattern, replacement, true),
                ReplaceScope::Prefix => pattern::replace_prefix(value, pattern, replacement),
                ReplaceScope::Suffix => pattern::replace_suffix(value, pattern, replacement),
            },
            Prepared::CaseConvert(selector, upper, all) => {
                pattern::convert_case(value, selector.as_ref(), *upper, *all)
            }
        }
    }
}

/// `${list:offset:length}` — the elements a slice selects.
///
/// A negative offset counts back from the end, as it does for a string. A negative *length* does
/// not: see [`window`].
///
/// The caller decides *which* list. `${a[@]:1}` passes the array's elements; `${@:1}` passes
/// `$0` followed by the positionals, which is why `${@:1}` is the first argument.
pub(super) fn slice_list(
    env: &mut Environment,
    values: &[String],
    offset: &Word,
    length: Option<&Word>,
) -> Result<Vec<String>> {
    let start = eval_operand(env, offset)?;
    let count = match length {
        Some(word) => Some(eval_operand(env, word)?),
        None => None,
    };
    let (from, to) = window(values.len(), start, count)
        .map_err(|n| ShellError::ExpansionError(format!("{n}: substring expression < 0")))?;
    Ok(values[from..to].to_vec())
}

/// The half-open element range `offset` and `length` select out of a list of `len` items.
///
/// `Err` carries the offending length, for the caller to report as bash does. This is where the
/// list rule parts company with the string rule and the difference is not an oversight: a
/// negative length names an *end position* in `${v:1:-1}`, but slicing a list with one is a fatal
/// `substring expression < 0` in bash however many elements there are, so `${a[@]:0:-1}` and
/// `${@:1:-1}` are errors where `${v:0:-1}` is a value.
fn window(
    len: usize,
    offset: i64,
    length: Option<i64>,
) -> std::result::Result<(usize, usize), i64> {
    let len = len as i64;

    let start = if offset < 0 {
        // Still negative after counting back from the end means the window starts before the
        // list, and bash selects nothing at all rather than clamping to the front.
        let from_end = len + offset;
        if from_end < 0 {
            return Ok((0, 0));
        }
        from_end
    } else {
        offset.min(len)
    };

    let end = match length {
        None => len,
        Some(n) if n < 0 => return Err(n),
        Some(n) => (start + n).min(len),
    };

    Ok((start as usize, end.max(start) as usize))
}

/// A substring operand is an arithmetic expression, so `${v:i+1:n-1}` works.
///
/// An absent operand is 0 rather than an error: `${v: }` is degenerate but not fatal, and the
/// arithmetic evaluator rejects an empty expression.
fn eval_operand(env: &mut Environment, word: &Word) -> Result<i64> {
    let text = expand_word_to_string(env, word)?;
    let text = text.trim();
    if text.is_empty() {
        return Ok(0);
    }
    eval_arithmetic(env, text)
}

/// Whether an indirection's target names an array element or slice rather than a plain parameter.
///
/// Shape only — `a[@]`, `a[0]`, `a[i+1]`. Whether the array exists is the expander's business.
fn array_reference(text: &str) -> bool {
    let Some((name, rest)) = text.split_once('[') else {
        return false;
    };
    !name.is_empty() && rest.ends_with(']') && super::is_param_name(name)
}

#[cfg(test)]
mod tests {
    use super::window;

    #[test]
    fn a_window_clamps_and_counts_back_from_the_end() {
        assert_eq!(window(3, 1, None), Ok((1, 3)));
        assert_eq!(window(3, 0, Some(2)), Ok((0, 2)));
        assert_eq!(window(3, 1, Some(0)), Ok((1, 1)));
        assert_eq!(window(3, -1, None), Ok((2, 3)));
        assert_eq!(window(3, -2, Some(1)), Ok((1, 2)));
        // Past either end selects nothing rather than producing a range that would panic.
        assert_eq!(window(3, 9, None), Ok((3, 3)));
        assert_eq!(window(3, 1, Some(9)), Ok((1, 3)));
        assert_eq!(window(3, -9, None), Ok((0, 0)));
        assert_eq!(window(0, 0, None), Ok((0, 0)));
    }

    /// The one place the list rule is *not* the string rule: `${v:0:-1}` drops the last character
    /// but `${a[@]:0:-1}` is fatal, whatever the array holds.
    #[test]
    fn a_negative_length_is_fatal_for_a_list() {
        assert_eq!(window(3, 0, Some(-1)), Err(-1));
        assert_eq!(window(4, 1, Some(-1)), Err(-1));
        assert_eq!(window(0, 0, Some(-1)), Err(-1));
    }
}
