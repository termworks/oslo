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
//! One element is `${a[i]}`, where the subscript is arithmetic, or `${m[key]}`, where it is the
//! key itself; the whole-array forms live in `whole`.

use super::whole::whole_array;
use super::{Target, operators, origin_of};
use crate::env::Environment;
use crate::env::builtins::arrays::Key;
use crate::env::scope::is_valid_identifier;
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
            let Some(key) = element_key(env, name, word)? else {
                return Ok(vec![vec![Run::new(String::new(), origin)]]);
            };
            check_nounset(env, name, &key, expansion_type)?;
            let target = Target::Element { name, key };
            let text = operators::apply(env, &target, expansion_type)?;
            Ok(vec![vec![Run::new(text, origin)]])
        }
    }
}

/// The key a subscript names in `name`, or `None` for one that names nothing, which bash reports
/// and expands to the empty string.
fn element_key(env: &mut Environment, name: &str, word: &Word) -> Result<Option<Key>> {
    let text = expand_word_to_string(env, word)?;
    if env.is_assoc(name) {
        if text.is_empty() {
            eprintln!("{}{name}[]: bad array subscript", env.origin());
            return Ok(None);
        }
        return Ok(Some(Key::Name(text)));
    }
    let index = eval_index(name, &text, env)?;
    if index < 0 {
        let resolved = match env.get_array(name) {
            Some(array) => array.resolve(index),
            None => (index == -1 && env.get_var(name).is_some()).then_some(0),
        };
        let Some(resolved) = resolved else {
            eprintln!("{}{name}: bad array subscript", env.origin());
            return Ok(None);
        };
        return Ok(Some(Key::Index(resolved)));
    }
    Ok(Some(Key::Index(index)))
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
    let separate = name == "@" || star_stays_apart(env, origin);
    if let ParamExpansion::Transform(op) = expansion_type {
        let values = env.get_positional().to_vec();
        let listed = whole_list_transform(name, *op, &values, || {
            super::lists::positional(env, *op, &values)
        })?;
        if let Some(listed) = listed {
            let separate = separate && !listed.single;
            let fields = list_fields(&listed.words, separate, &env.ifs_separator(), origin);
            return Ok(Some(seam_list(fields, separate, name == "*", origin)));
        }
    }
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
    let fields = list_fields(&values, separate, &env.ifs_separator(), origin);
    Ok(Some(seam_list(fields, separate, name == "*", origin)))
}

/// Check a transformation of a whole list and run it when it reads the list as a whole.
///
/// `Ok(None)` leaves an element-wise one to the caller. An unknown letter is an error only when
/// there is something to transform.
pub(super) fn whole_list_transform(
    shown: &str,
    op: char,
    values: &[String],
    run: impl FnOnce() -> Option<super::lists::Listed>,
) -> Result<Option<super::lists::Listed>> {
    if super::transform::is_known(op) {
        return Ok(run());
    }
    if values.is_empty() {
        return Ok(Some(super::lists::Listed {
            words: Vec::new(),
            single: false,
        }));
    }
    Err(super::transform::unknown(shown, op))
}

/// Whether an unquoted `$*` or `${a[*]}` stays one field per element, which a null IFS makes it.
pub(super) fn star_stays_apart(env: &Environment, origin: Origin) -> bool {
    origin == Origin::Expanded && crate::expand::join::ifs_is_null(env)
}

/// Mark the fields of a list operator's result for a single-string context.
///
/// An unquoted `[*]` kept apart by a null IFS is glued back together there, and a quoted `[@]`
/// operator form is seamed; see [`crate::expand::join`].
pub(super) fn seam_list(
    fields: Vec<Field>,
    separate: bool,
    star: bool,
    origin: Origin,
) -> Vec<Field> {
    match (separate, star, origin) {
        (true, true, _) => crate::expand::join::marked(fields, Origin::Glue),
        (true, false, Origin::Quoted) => crate::expand::join::marked(fields, Origin::Seam),
        _ => fields,
    }
}

/// Evaluate a subscript as arithmetic, which is what bash does for an indexed array: `${a[i+1]}`
/// reads `i`, and `${a[x]}` with `x` unset reads element 0.
fn eval_index(name: &str, text: &str, env: &mut Environment) -> Result<i64> {
    let text = text.trim();
    if text.is_empty() {
        return Err(ShellError::ExpansionError(format!(
            "{name}: bad array subscript"
        )));
    }
    crate::expand::arithmetic::eval_subscript(env, text)
}

/// `set -u` applies to an element that was never assigned, exactly as it does to a plain variable.
///
/// The operators that exist to *define* what an absent value means are exempt, for the reason they
/// are exempt for scalars: `${a[3]-default}` is how a script reads a possibly-absent element under
/// `set -u`.
fn check_nounset(
    env: &Environment,
    name: &str,
    key: &Key,
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
    // `${#a[5]}` of an array that is set is 0, missing element or not.
    if matches!(expansion_type, ParamExpansion::Length)
        && env
            .variable(name)
            .is_some_and(|v| v.is_set() && (v.indexed().is_some() || v.assoc().is_some()))
    {
        return Ok(());
    }
    let target = Target::Element {
        name,
        key: match key {
            Key::Index(i) => Key::Index(*i),
            Key::Name(k) => Key::Name(k.clone()),
        },
    };
    if target.value(env).is_some() {
        return Ok(());
    }
    Err(ShellError::UnsetParameter(format!(
        "{}: unbound variable",
        target.display()
    )))
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

    /// `${a[@]:=d}` and `${a[@]:?msg}` answer with the elements of a set array, and fail on an
    /// empty one: bash cannot assign to `a[@]`, and `:?` is the error it asks for.
    #[test]
    fn the_assigning_and_erroring_forms_act_on_the_whole_array() {
        let op = ParamExpansion::DefaultValue {
            default: Word::from_literal("d"),
            assign_if_unset: true,
            test_null: true,
        };
        assert_eq!(
            fields(&["a", "b"], Subscript::All, op.clone()),
            Ok(vec!["a".to_string(), "b".to_string()])
        );
        assert!(fields(&[], Subscript::All, op).is_err());
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

    /// A scalar is a one-element array to `[@]`, except under a slice, which bash takes as a
    /// substring of it.
    #[test]
    fn a_scalar_slices_as_a_string() {
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
        assert_eq!(text(&slice("1", None)), vec!["olo".to_string()]);
    }

    #[test]
    fn an_empty_subscript_is_rejected() {
        assert!(fields(&["a"], index(""), ParamExpansion::Normal).is_err());
    }
}
