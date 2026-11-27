//! Expanding a reference that stands for a *list*: `${a[…]}`, and `$@`/`$*` under an operator.
//!
//! The whole point of the field-list representation is reused here rather than reinvented:
//! `"${a[@]}"` is *one [`Field`] per element*, and **no field at all** when the array is empty —
//! byte for byte the rule that makes `"$@"` with no positionals vanish instead of becoming one
//! empty argument. `cmd "${paths[@]}"` with an empty `paths` therefore runs `cmd`, not `cmd ""`.
//!
//! `[*]` is the other half of the same pair: one field, elements joined by the first character of
//! IFS, exactly as `$*` joins the positionals. [`list_fields`] is that pair, and both the array
//! path and the positional path end in it.
//!
//! The positional parameters live here for the same reason. `${@:1}` and `${a[@]:1}` are one
//! operation over two different lists, and `${@#pat}` and `${a[@]#pat}` are another; when the two
//! were implemented separately, only the array half was implemented at all and `"${@:2}"` quietly
//! sliced *characters* out of the space-joined arguments.
//!
//! What is still missing errors instead of guessing: the `${a[@]:-d}` family has list semantics
//! oslo does not have yet, and answering it with the first element or an empty string would be
//! the quiet wrong answer this shell is being audited for.

use super::{Target, operators, origin_of};
use crate::env::Environment;
use crate::env::scope::is_valid_identifier;
use crate::expand::arithmetic::eval_arithmetic;
use crate::expand::word::{Field, Origin, Run, expand_word_to_string};
use oslo_base::ast::{ParamExpansion, Subscript, Word};
use oslo_base::error::{Result, ShellError};

/// Expand `${name[subscript]<op>}` into the fields it contributes.
pub fn expand_array_ref(
    env: &mut Environment,
    name: &str,
    subscript: &Subscript,
    expansion_type: &ParamExpansion,
    in_quotes: bool,
) -> Result<Vec<Field>> {
    // Only a *variable* can be subscripted. `${1[@]}` and `${@[0]}` are bad substitutions in
    // bash, and answering them with an empty string would invent an array nothing ever created.
    if !is_valid_identifier(name) {
        return Err(ShellError::MalformedExpansion(format!(
            "${{{name}[…]}}: bad substitution"
        )));
    }
    let origin = origin_of(in_quotes);

    match subscript {
        Subscript::All | Subscript::Joined => {
            whole_array(env, name, subscript, expansion_type, origin)
        }
        Subscript::Index(word) => {
            let index = eval_index(env, name, word)?;
            check_nounset(env, name, index, expansion_type)?;
            let text = operators::apply(env, &Target::Element { name, index }, expansion_type)?;
            Ok(vec![vec![Run::new(text, origin)]])
        }
    }
}

/// `${a[@]}` and `${a[*]}`, with the operators that mean something for a whole array.
fn whole_array(
    env: &mut Environment,
    name: &str,
    subscript: &Subscript,
    expansion_type: &ParamExpansion,
    origin: Origin,
) -> Result<Vec<Field>> {
    // A plain scalar is a one-element array as far as `[@]` is concerned: `v=x` makes
    // `"${v[@]}"` one field holding `x` and `${#v[@]}` the number 1, which is the same identity
    // that already makes `${v[0]}` answer `x`. An unset name is the empty array.
    //
    // The elements are *copied out* rather than borrowed, because the operators below need `env`
    // mutably: a pattern or a slice offset may contain a command substitution.
    let (values, indices) = match env.get_array(name) {
        Some(array) => (
            array.values().map(str::to_string).collect::<Vec<String>>(),
            array.indices().map(|i| i.to_string()).collect::<Vec<_>>(),
        ),
        None => match env.get_var(name) {
            Some(value) => (vec![value.to_string()], vec!["0".to_string()]),
            None => (Vec::new(), Vec::new()),
        },
    };
    let star = matches!(subscript, Subscript::Joined);
    let separate = !star || star_stays_apart(env, origin);
    let operator_form = !matches!(
        expansion_type,
        ParamExpansion::Normal
            | ParamExpansion::DefaultValue { .. }
            | ParamExpansion::UseAlternative { .. }
    );

    let values: Vec<String> = match expansion_type {
        ParamExpansion::Normal => values,

        // `${#a[@]}` is the number of elements, never the width of the joined text — and it is a
        // single field whichever subscript asked for it.
        ParamExpansion::Length => {
            return Ok(vec![vec![Run::new(values.len().to_string(), origin)]]);
        }

        // `${!a[@]}` lists the indices *in use*. A sparse array is why this is not `0..len`.
        ParamExpansion::Indirect(_) => indices,

        // `${a[@]:1:2}` selects *elements*. Slicing the joined text instead would answer with a
        // string cut mid-element, which is exactly the shape of the `"${@:2}"` corruption.
        ParamExpansion::Substring { offset, length } => {
            operators::slice_list(env, &values, offset, length.as_ref())?
        }

        // `${a[@]#pat}`, `${a[@]^^}`, `${a[@]/x/y}` rewrite every element and keep the count. A
        // sparse array keeps its holes: the values are rewritten, the indices are not touched.
        operator if operators::is_elementwise(operator) => {
            operators::map_elements(env, &values, operator)?
        }

        // **`${a[@]+alt}` and `${a[@]:-d}` test the array, then answer with a word.** An array is
        // "set" when it has at least one element, so the whole list collapses to whichever branch
        // wins — the result is the word, not the array. stdenv's `runHook` is written
        // `${!hooksSlice+"${!hooksSlice}"}`, so a shell without these two cannot run a hook.
        ParamExpansion::UseAlternative {
            alternative,
            test_null,
        } => {
            let present = present(&values, *test_null);
            let text = if present {
                crate::expand::word::expand_word_to_string(env, alternative)?
            } else {
                return Ok(Vec::new());
            };
            vec![text]
        }
        ParamExpansion::DefaultValue {
            default,
            assign_if_unset: false,
            test_null,
        } => {
            if present(&values, *test_null) {
                values
            } else {
                vec![crate::expand::word::expand_word_to_string(env, default)?]
            }
        }

        // What is left is the `${a[@]:-d}`, `${a[@]:=d}`, `${a[@]+alt}` and `${a[@]?msg}` family.
        // Their list semantics are real bash and are not implemented; answering with a
        // plausible-looking string would be worse than saying so.
        _ => {
            return Err(ShellError::ExpansionError(format!(
                "${{{name}[{}]…}}: this operator on a whole array is not supported yet",
                if star { "*" } else { "@" }
            )));
        }
    };

    let fields = list_fields(&values, separate, &env.ifs_separator(), origin);
    if star || operator_form {
        return Ok(seam_list(fields, separate, star, origin));
    }
    Ok(fields)
}

/// The fields a list contributes: one per value when `separate`, or a single joined field.
///
/// `separate` is `[@]` and `"$@"`; joined is `[*]` and `"$*"`. Separate with no values is **no
/// field at all**, which is what makes `cmd "${paths[@]}"` run `cmd` rather than `cmd ""`.
pub(super) fn list_fields(
    values: &[String],
    separate: bool,
    separator: &str,
    origin: Origin,
) -> Vec<Field> {
    if separate {
        return values
            .iter()
            .map(|value| vec![Run::new(value.clone(), origin)])
            .collect();
    }
    vec![vec![Run::new(values.join(separator), origin)]]
}

/// `$@` and `$*` under an operator that acts on the list of positionals.
///
/// `None` means the operator is not list-valued and the caller should take the scalar path, which
/// is the right answer for `${#@}` (a count) and for the `${@:-d}` family.
pub(super) fn positional_list(
    env: &mut Environment,
    name: &str,
    expansion_type: &ParamExpansion,
    origin: Origin,
) -> Result<Option<Vec<Field>>> {
    let values = match expansion_type {
        // bash slices `$0 $1 $2 …`, one list longer than `$@` itself: `${@:0}` picks up the
        // shell's own name and `${@:1}` is the first argument, which is why every
        // argument-forwarding idiom is written `"${@:1}"` and not `"${@:0}"`.
        ParamExpansion::Substring { offset, length } => {
            let mut all = vec![env.get_param("0").unwrap_or_default()];
            all.extend(env.get_positional().iter().cloned());
            operators::slice_list(env, &all, offset, length.as_ref())?
        }
        // `${@#pat}` strips from each argument; `$0` takes no part in this one.
        operator if operators::is_elementwise(operator) => {
            let values = env.get_positional().to_vec();
            operators::map_elements(env, &values, operator)?
        }
        _ => return Ok(None),
    };
    let separate = name == "@" || star_stays_apart(env, origin);
    let fields = list_fields(&values, separate, &env.ifs_separator(), origin);
    Ok(Some(seam_list(fields, separate, name == "*", origin)))
}

/// Whether an unquoted `$*` or `${a[*]}` stays one field per element, which a null IFS makes it.
pub(super) fn star_stays_apart(env: &Environment, origin: Origin) -> bool {
    origin == Origin::Expanded && crate::expand::join::ifs_is_null(env)
}

/// Mark the fields of a list operator's result for a single-string context.
///
/// An unquoted `[*]` kept apart by a null IFS is glued back together there, and a quoted `[@]`
/// operator form is seamed; see [`crate::expand::join`].
fn seam_list(fields: Vec<Field>, separate: bool, star: bool, origin: Origin) -> Vec<Field> {
    match (separate, star, origin) {
        (true, true, _) => crate::expand::join::marked(fields, Origin::Glue),
        (true, false, Origin::Quoted) => crate::expand::join::marked(fields, Origin::Seam),
        _ => fields,
    }
}

/// Evaluate a subscript as arithmetic, which is what bash does for an indexed array: `${a[i+1]}`
/// reads `i`, and `${a[x]}` with `x` unset reads element 0.
fn eval_index(env: &mut Environment, name: &str, word: &Word) -> Result<i64> {
    let text = expand_word_to_string(env, word)?;
    let text = text.trim();
    if text.is_empty() {
        return Err(ShellError::ExpansionError(format!(
            "{name}: bad array subscript"
        )));
    }
    eval_arithmetic(env, text)
}

/// `set -u` applies to an element that was never assigned, exactly as it does to a plain variable.
///
/// The operators that exist to *define* what an absent value means are exempt, for the reason they
/// are exempt for scalars: `${a[3]-default}` is how a script reads a possibly-absent element under
/// `set -u`, and erroring on it would leave no way to do so.
fn check_nounset(
    env: &Environment,
    name: &str,
    index: i64,
    expansion_type: &ParamExpansion,
) -> Result<()> {
    if !env.nounset() {
        return Ok(());
    }
    if matches!(
        expansion_type,
        ParamExpansion::DefaultValue { .. }
            | ParamExpansion::UseAlternative { .. }
            | ParamExpansion::ErrorIfUnset { .. }
    ) {
        return Ok(());
    }
    let exists = match env.get_array(name) {
        Some(array) => array.get(index).is_some(),
        // The same scalar-is-one-element identity `Target::value` applies; without it `set -u` made
        // `${v[0]}` on a plain variable fatal, where bash answers the value.
        None => index == 0 && env.get_var(name).is_some(),
    };
    if exists {
        return Ok(());
    }
    Err(ShellError::UnsetParameter(format!(
        "{name}[{index}]: unbound variable"
    )))
}

/// Whether a whole array counts as set, for the `+` and `:-` families.
///
/// An array with no elements is unset; the `:` variants also treat one whose every element is
/// empty as unset, which is the same distinction `${x-d}` and `${x:-d}` draw for a scalar.
fn present(values: &[String], test_null: bool) -> bool {
    if values.is_empty() {
        return false;
    }
    !test_null || values.iter().any(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::expand_array_ref;
    use crate::env::Environment;
    use crate::env::scope::ShellArray;
    use crate::expand::word::field_text;
    use oslo_base::ast::{ParamExpansion, ReplaceScope, Subscript, Word};

    /// Expand `${name[sub]<op>}` over an array of `values`, as a list of field texts.
    fn fields(values: &[&str], sub: Subscript, op: ParamExpansion) -> Result<Vec<String>, String> {
        let mut env = Environment::new();
        env.set_array("oslo_arr", ShellArray::from_values(values.to_vec()));
        expand_array_ref(&mut env, "oslo_arr", &sub, &op, true)
            .map(|fields| fields.iter().map(|f| field_text(f)).collect())
            .map_err(|e| e.to_string())
    }

    fn index(n: &str) -> Subscript {
        Subscript::Index(Word::from_literal(n))
    }

    /// The R2.2 rule, applied to arrays: one field per element, and none when there are none.
    #[test]
    fn at_is_one_field_per_element() {
        let got = fields(&["a b", "c"], Subscript::All, ParamExpansion::Normal);
        assert_eq!(got, Ok(vec!["a b".to_string(), "c".to_string()]));
        assert_eq!(
            fields(&[], Subscript::All, ParamExpansion::Normal),
            Ok(vec![])
        );
    }

    /// `[*]` is one field however many elements there are — including none.
    #[test]
    fn star_is_always_one_field() {
        let got = fields(&["a", "b"], Subscript::Joined, ParamExpansion::Normal);
        assert_eq!(got, Ok(vec!["a b".to_string()]));
        let got = fields(&[], Subscript::Joined, ParamExpansion::Normal);
        assert_eq!(got, Ok(vec![String::new()]));
    }

    #[test]
    fn length_counts_elements_and_indices_list_them() {
        let got = fields(&["a", "b", "c"], Subscript::All, ParamExpansion::Length);
        assert_eq!(got, Ok(vec!["3".to_string()]));
        let got = fields(
            &["a", "b"],
            Subscript::All,
            ParamExpansion::Indirect(Box::new(ParamExpansion::Normal)),
        );
        assert_eq!(got, Ok(vec!["0".to_string(), "1".to_string()]));
    }

    /// A sparse array is the reason `${!a[@]}` cannot be `0..len`.
    #[test]
    fn indices_report_the_holes() {
        let mut env = Environment::new();
        let mut array = ShellArray::from_values(["a", "b", "c"]);
        array.remove(1);
        env.set_array("oslo_sparse", array);
        let got = expand_array_ref(
            &mut env,
            "oslo_sparse",
            &Subscript::All,
            &ParamExpansion::Indirect(Box::new(ParamExpansion::Normal)),
            true,
        )
        .unwrap();
        let texts: Vec<String> = got.iter().map(|f| field_text(f)).collect();
        assert_eq!(texts, vec!["0".to_string(), "2".to_string()]);
        assert_eq!(
            fields(&[], Subscript::All, ParamExpansion::Length),
            Ok(vec!["0".to_string()])
        );
    }

    /// A subscript is arithmetic, so `${a[i+1]}` reads `i` rather than a variable called `i+1`.
    #[test]
    fn a_subscript_is_evaluated_as_arithmetic() {
        let mut env = Environment::new();
        env.set_array("oslo_ar2", ShellArray::from_values(["x", "y", "z"]));
        env.set_var("oslo_i", "1", false);
        let sub = Subscript::Index(Word::from_literal("oslo_i+1"));
        let got =
            expand_array_ref(&mut env, "oslo_ar2", &sub, &ParamExpansion::Normal, true).unwrap();
        assert_eq!(field_text(&got[0]), "z");
    }

    #[test]
    fn an_operator_applies_to_the_selected_element() {
        let got = fields(&["a.c", "b.c"], index("1"), ParamExpansion::Length);
        assert_eq!(got, Ok(vec!["3".to_string()]));
        // An absent element is empty, not an error.
        let got = fields(&["a"], index("9"), ParamExpansion::Normal);
        assert_eq!(got, Ok(vec![String::new()]));
    }

    /// `${a[@]:-d}` and `${a[@]+alt}` test the array and answer with a word — bash's list
    /// semantics, and what stdenv's `runHook` is written on.
    #[test]
    fn the_default_and_alternative_forms_test_the_whole_array() {
        let alt = |test_null| ParamExpansion::UseAlternative {
            alternative: Word::from_literal("yes"),
            test_null,
        };
        // A non-empty array is set.
        assert_eq!(
            fields(&["a", "b"], Subscript::All, alt(false)),
            Ok(vec!["yes".to_string()])
        );
        // An empty one is not, and answers with no field at all.
        assert_eq!(fields(&[], Subscript::All, alt(false)), Ok(Vec::new()));
        // `:` also treats an array of empty strings as unset.
        assert_eq!(fields(&[""], Subscript::All, alt(true)), Ok(Vec::new()));

        let def = ParamExpansion::DefaultValue {
            default: Word::from_literal("d"),
            assign_if_unset: false,
            test_null: true,
        };
        assert_eq!(
            fields(&["a", "b"], Subscript::All, def.clone()),
            Ok(vec!["a".to_string(), "b".to_string()])
        );
        assert_eq!(fields(&[], Subscript::All, def), Ok(vec!["d".to_string()]));
    }

    /// What is still unimplemented must say so rather than answer something plausible: the
    /// assigning `${a[@]:=d}` and the erroring `${a[@]?msg}` have list semantics oslo lacks.
    #[test]
    fn an_unimplemented_whole_array_operator_is_an_error() {
        let op = ParamExpansion::DefaultValue {
            default: Word::from_literal("d"),
            assign_if_unset: true,
            test_null: true,
        };
        assert!(fields(&["a", "b"], Subscript::All, op).is_err());
        let op = ParamExpansion::ErrorIfUnset {
            message: Word::from_literal("m"),
            test_null: true,
        };
        assert!(fields(&[], Subscript::Joined, op).is_err());
    }

    fn slice(offset: &str, length: Option<&str>) -> ParamExpansion {
        ParamExpansion::Substring {
            offset: Word::from_literal(offset),
            length: length.map(Word::from_literal),
        }
    }

    /// R11/B4: `${a[@]:1}` selects *elements*. Slicing the joined text instead would answer with
    /// a string cut in the middle of an element.
    #[test]
    fn a_slice_selects_elements_not_characters() {
        let v = ["alpha", "beta", "gamma"];
        let got = fields(&v, Subscript::All, slice("1", None));
        assert_eq!(got, Ok(vec!["beta".to_string(), "gamma".to_string()]));
        let got = fields(&v, Subscript::All, slice("1", Some("1")));
        assert_eq!(got, Ok(vec!["beta".to_string()]));
        // A negative offset counts back from the end; one that lands before the start selects
        // nothing, and `[@]` with nothing selected is no field at all.
        assert_eq!(
            fields(&v, Subscript::All, slice("-1", None)),
            Ok(vec!["gamma".to_string()])
        );
        assert_eq!(fields(&v, Subscript::All, slice("-9", None)), Ok(vec![]));
        assert_eq!(fields(&v, Subscript::All, slice("9", None)), Ok(vec![]));
        // `[*]` joins whatever the slice selected, and is one field even when that is nothing.
        let got = fields(&v, Subscript::Joined, slice("1", None));
        assert_eq!(got, Ok(vec!["beta gamma".to_string()]));
        assert_eq!(
            fields(&v, Subscript::Joined, slice("9", None)),
            Ok(vec![String::new()])
        );
    }

    /// bash makes a negative length fatal for a list, where the same length on a string names an
    /// end position. Answering `${a[@]:0:-1}` with the elements bar the last would be wrong.
    #[test]
    fn a_negative_slice_length_is_an_error() {
        assert!(fields(&["a", "b"], Subscript::All, slice("0", Some("-1"))).is_err());
    }

    /// R11/B4: `${a[@]#p}` and friends rewrite every element and keep the count.
    #[test]
    fn a_pattern_operator_maps_over_the_elements() {
        let op = ParamExpansion::RemoveSuffix {
            pattern: Word::from_literal(".c"),
            longest: false,
        };
        let got = fields(&["f.c", "g.c"], Subscript::All, op);
        assert_eq!(got, Ok(vec!["f".to_string(), "g".to_string()]));

        let op = ParamExpansion::CaseConvert {
            pattern: None,
            upper: true,
            all: true,
        };
        let got = fields(&["ab", "cd"], Subscript::All, op);
        assert_eq!(got, Ok(vec!["AB".to_string(), "CD".to_string()]));

        // `[*]` maps first and joins after — the joined text is not what the pattern saw.
        let op = ParamExpansion::Replace {
            pattern: Word::from_literal("-"),
            replacement: Word::from_literal("+"),
            scope: ReplaceScope::All,
        };
        let got = fields(&["a-b", "c-d"], Subscript::Joined, op);
        assert_eq!(got, Ok(vec!["a+b c+d".to_string()]));
    }

    /// A slice indexes the elements *in use*, so a hole shifts what `:1` selects rather than
    /// leaving an empty field where the hole was.
    #[test]
    fn a_slice_of_a_sparse_array_counts_present_elements() {
        let mut env = Environment::new();
        let mut array = ShellArray::from_values(["x", "y", "z"]);
        array.remove(1);
        env.set_array("oslo_sparse_slice", array);
        let got = expand_array_ref(
            &mut env,
            "oslo_sparse_slice",
            &Subscript::All,
            &slice("1", None),
            true,
        )
        .unwrap();
        let texts: Vec<String> = got.iter().map(|f| field_text(f)).collect();
        assert_eq!(texts, vec!["z".to_string()]);
    }

    /// A scalar is a one-element array to `[@]`, and that identity has to survive the operators.
    #[test]
    fn a_scalar_slices_and_maps_as_one_element() {
        let mut env = Environment::new();
        env.set_var("oslo_scalar", "solo", false);
        let mut text = |op: &ParamExpansion| {
            expand_array_ref(&mut env, "oslo_scalar", &Subscript::All, op, true)
                .unwrap()
                .iter()
                .map(|f| field_text(f))
                .collect::<Vec<_>>()
        };
        assert_eq!(text(&slice("0", None)), vec!["solo".to_string()]);
        assert_eq!(text(&slice("1", None)), Vec::<String>::new());
    }

    #[test]
    fn an_empty_subscript_is_rejected() {
        assert!(fields(&["a"], index(""), ParamExpansion::Normal).is_err());
    }
}
