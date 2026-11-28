//! `${a[@]}` and `${a[*]}` with the operators that mean something for a whole array.
//!
//! An indexed array lists its elements by index, an associative one in its walk order, and a
//! plain scalar is a list of one — except under a slice, where bash takes `${x[@]:1}` on a scalar
//! as a substring of it.

use super::array::{list_fields, seam_list, star_stays_apart, whole_list_transform};
use super::{Target, operators};
use crate::env::Environment;
use crate::expand::word::{Field, Origin, Run, expand_word_to_string};
use oslo_base::ast::{ParamExpansion, Subscript};
use oslo_base::error::{Result, ShellError};

/// What a name holds, seen as a list.
enum Shape {
    Indexed,
    Assoc,
    Scalar,
    Unset,
}

/// The values, the keys or indices, and the shape of `name`.
fn listing(env: &Environment, name: &str) -> (Vec<String>, Vec<String>, Shape) {
    if let Some(array) = env.get_array(name) {
        return (
            array.values().map(str::to_string).collect(),
            array.indices().map(|i| i.to_string()).collect(),
            Shape::Indexed,
        );
    }
    if let Some(map) = env.get_assoc(name) {
        return (
            map.values().map(str::to_string).collect(),
            map.keys().map(str::to_string).collect(),
            Shape::Assoc,
        );
    }
    match env.get_var(name) {
        Some(value) => (
            vec![value.to_string()],
            vec!["0".to_string()],
            Shape::Scalar,
        ),
        None => (Vec::new(), Vec::new(), Shape::Unset),
    }
}

/// The elements `${name[@]:offset:count}` selects, by the rule each shape follows.
fn slice(
    env: &mut Environment,
    name: &str,
    values: Vec<String>,
    shape: &Shape,
    offset: i64,
    count: Option<usize>,
) -> Vec<String> {
    match shape {
        Shape::Indexed => env
            .get_array(name)
            .map(|array| array.slice(offset, count))
            .unwrap_or_default(),
        // Positions from 1, as bash counts an associative array: `:0` and `:1` both start at the
        // first, and a negative offset counts back from one past the last.
        Shape::Assoc => {
            let len = values.len() as i64;
            let start = if offset < 0 { len + offset + 1 } else { offset };
            let skip = (start - 1).max(0) as usize;
            values
                .into_iter()
                .skip(skip)
                .take(count.unwrap_or(usize::MAX))
                .collect()
        }
        Shape::Scalar | Shape::Unset => Vec::new(),
    }
}

/// `${a[@]}` and `${a[*]}`, with an operator.
pub(super) fn whole_array(
    env: &mut Environment,
    name: &str,
    subscript: &Subscript,
    expansion_type: &ParamExpansion,
    origin: Origin,
) -> Result<Vec<Field>> {
    let (values, keys, shape) = listing(env, name);
    let star = matches!(subscript, Subscript::Joined);
    let separate = !star || star_stays_apart(env, origin);
    let at = if star { "*" } else { "@" };
    let operator_form = !matches!(
        expansion_type,
        ParamExpansion::Normal
            | ParamExpansion::DefaultValue { .. }
            | ParamExpansion::UseAlternative { .. }
    );

    if let ParamExpansion::Transform(op) = expansion_type {
        let listed = whole_list_transform(&format!("{name}[@]"), *op, &values, || {
            super::lists::array(env, name, *op, &keys, &values)
        })?;
        if let Some(listed) = listed {
            let separate = separate && !listed.single;
            let fields = list_fields(&listed.words, separate, &env.ifs_separator(), origin);
            return Ok(seam_list(fields, separate, star, origin));
        }
    }

    let values: Vec<String> = match expansion_type {
        ParamExpansion::Normal => values,

        // `${#a[@]}` is the number of elements, and a single field whichever subscript asked.
        ParamExpansion::Length => {
            // Under `set -u` bash counts only an array that is set, and abandons the line
            // otherwise — a scalar included.
            let counted = env
                .variable(name)
                .is_some_and(|v| v.is_set() && (v.indexed().is_some() || v.assoc().is_some()));
            if env.nounset() && !counted {
                return Err(ShellError::ExpansionError(format!(
                    "{name}: unbound variable"
                )));
            }
            return Ok(vec![vec![Run::new(values.len().to_string(), origin)]]);
        }

        // `${!a[@]}` lists the indices in use, or the keys.
        ParamExpansion::Indirect(_) => keys,

        ParamExpansion::Substring { offset, length } => {
            if matches!(shape, Shape::Scalar) {
                let text = operators::apply(env, &Target::Param(name), expansion_type)?;
                return Ok(vec![vec![Run::new(text, origin)]]);
            }
            let (offset, count) = operators::slice_operands(env, offset, length.as_ref())?;
            slice(env, name, values, &shape, offset, count)
        }

        operator if operators::is_elementwise(operator) => {
            operators::map_elements(env, &values, operator)?
        }

        // `${a[@]+alt}` and `${a[@]:-d}` test the array, then answer with a word.
        ParamExpansion::UseAlternative {
            alternative,
            test_null,
        } => match present(&values, *test_null) {
            true => vec![expand_word_to_string(env, alternative)?],
            false => return Ok(Vec::new()),
        },
        ParamExpansion::DefaultValue {
            default,
            assign_if_unset,
            test_null,
        } => match (present(&values, *test_null), assign_if_unset) {
            (true, _) => values,
            (false, false) => vec![expand_word_to_string(env, default)?],
            (false, true) => {
                return Err(ShellError::ExpansionError(format!(
                    "{name}[{at}]: bad array subscript"
                )));
            }
        },
        ParamExpansion::ErrorIfUnset { message, test_null } => {
            if present(&values, *test_null) {
                values
            } else {
                let message = expand_word_to_string(env, message)?;
                let message = match message.is_empty() {
                    true => "parameter null or not set".to_string(),
                    false => message,
                };
                return Err(ShellError::UnsetParameter(format!(
                    "{name}[{at}]: {message}"
                )));
            }
        }
        _ => {
            return Err(ShellError::ExpansionError(format!(
                "${{{name}[{at}]…}}: this operator on a whole array is not supported yet"
            )));
        }
    };

    let fields = list_fields(&values, separate, &env.ifs_separator(), origin);
    if star || operator_form {
        return Ok(seam_list(fields, separate, star, origin));
    }
    Ok(fields)
}

/// Whether a whole array counts as set, for the `+`, `-`, `=` and `?` families.
///
/// An array with no elements is unset; the `:` variants also treat one whose every element is
/// empty as unset, which is the same distinction `${x-d}` and `${x:-d}` draw for a scalar.
fn present(values: &[String], test_null: bool) -> bool {
    if values.is_empty() {
        return false;
    }
    !test_null || values.iter().any(|value| !value.is_empty())
}
