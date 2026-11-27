//! Array literals: `(a b)`, `([2]=a [5]=b)`, `([key]=v)`, and the shape an associative array
//! takes from them.
//!
//! `a=(…)` written on its own reaches the evaluator as a structured assignment; `declare a=(…)`
//! reaches its builtin as the text `a=(…)`, because an assignment written after a command word is
//! an ordinary argument, and the parser reads that text back into the same elements. Both become
//! the same list of [`LiteralItem`]s here, so the two spellings cannot mean different things.

use crate::env::Environment;
use crate::env::scope::{AssocArray, ShellArray};
use crate::expand::arithmetic::eval_arithmetic;
use crate::expand::{expand_word, expand_word_to_string};
use oslo_base::ast::ArrayElement;
use oslo_base::error::{Result, ShellError};

/// One element of a literal, expanded.
pub enum LiteralItem {
    /// `[key]=value`: the key as text, arithmetic only once it is known to be an index.
    Keyed { key: String, value: String },
    /// A bare element, one per field it expanded to.
    Plain(String),
}

/// Expand the body of a literal written as a builtin's operand: `declare a=(…)`. It may span
/// lines and hold comments, as the literal of an assignment may.
pub fn literal_items(env: &mut Environment, body: &str) -> Result<Vec<LiteralItem>> {
    let elements = crate::syntax::array_literal(body)?;
    literal_items_from_ast(env, &elements)
}

/// The items of a literal the parser took apart.
///
/// A keyed element's value is one string; a bare element is an ordinary word in list context, so
/// `a=($list)` splits and `a=(*.c)` globs, both into separate elements.
pub fn literal_items_from_ast(
    env: &mut Environment,
    elements: &[ArrayElement],
) -> Result<Vec<LiteralItem>> {
    let mut items = Vec::new();
    for element in elements {
        match &element.index {
            Some(index) => items.push(LiteralItem::Keyed {
                key: expand_word_to_string(env, index)?,
                value: expand_word_to_string(env, &element.value)?,
            }),
            None => items.extend(
                expand_word(env, &element.value)?
                    .into_iter()
                    .map(LiteralItem::Plain),
            ),
        }
    }
    Ok(items)
}

/// Assign a literal to `name`: `name=(…)`, or `name+=(…)` when `append`.
///
/// An associative array takes `[key]=value` items, or bare items as alternating keys and values;
/// anything else is an indexed array, where a bare item goes to the index after the last one
/// written. `Ok(false)` means the name refused the assignment, with the reason already printed.
pub fn assign_literal(
    env: &mut Environment,
    name: &str,
    items: Vec<LiteralItem>,
    append: bool,
) -> Result<bool> {
    let name = env.ref_target_name(name).into_owned();
    if env.is_readonly(&name) {
        env.complain_readonly(&name, "");
        return Ok(false);
    }
    if env.is_assoc(&name) {
        let map = build_assoc(env, &name, items, append)?;
        return Ok(env.set_assoc(&name, map));
    }
    let array = build_indexed(env, &name, items, append)?;
    Ok(env.set_array(&name, array))
}

fn build_indexed(
    env: &mut Environment,
    name: &str,
    items: Vec<LiteralItem>,
    append: bool,
) -> Result<ShellArray> {
    let attrs = env.variable(name).map(|v| v.attrs).unwrap_or_default();
    let mut array = match (append, env.get_array(name)) {
        (false, _) => ShellArray::default(),
        (true, Some(array)) => array.clone(),
        (true, None) => match env.get_var(name) {
            Some(text) => ShellArray::from_values([text.to_string()]),
            None => ShellArray::default(),
        },
    };
    let mut next = array.next_index();
    for item in items {
        match item {
            LiteralItem::Keyed { key, value } => {
                let trimmed = key.trim();
                let index = match trimmed.is_empty() {
                    true => None,
                    false => array.resolve(eval_arithmetic(env, trimmed)?),
                };
                let Some(index) = index else {
                    return Err(super::bad_subscript(&format!("[{key}]={value}")));
                };
                array.set(index, env.transform_with(attrs, &value));
                next = index + 1;
            }
            LiteralItem::Plain(value) => {
                array.set(next, env.transform_with(attrs, &value));
                next += 1;
            }
        }
    }
    Ok(array)
}

fn build_assoc(
    env: &mut Environment,
    name: &str,
    items: Vec<LiteralItem>,
    append: bool,
) -> Result<AssocArray> {
    let attrs = env.variable(name).map(|v| v.attrs).unwrap_or_default();
    let mut map = match append {
        true => env.get_assoc(name).cloned().unwrap_or_default(),
        false => AssocArray::default(),
    };
    let keyed = items.iter().any(|i| matches!(i, LiteralItem::Keyed { .. }));
    let mut pending_key: Option<String> = None;
    for item in items {
        match item {
            LiteralItem::Keyed { key, value } => {
                if key.is_empty() {
                    return Err(super::bad_subscript(&format!("{name}[{key}]")));
                }
                map.set(&key, env.transform_with(attrs, &value));
            }
            LiteralItem::Plain(word) if keyed => {
                return Err(ShellError::ExpansionError(format!(
                    "{name}: {word}: must use subscript when assigning associative array"
                )));
            }
            LiteralItem::Plain(word) => match pending_key.take() {
                None => pending_key = Some(word),
                Some(key) => map.set(&key, env.transform_with(attrs, &word)),
            },
        }
    }
    if let Some(key) = pending_key {
        map.set(&key, "");
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::{assign_literal, literal_items};
    use crate::env::Environment;

    fn assign(env: &mut Environment, name: &str, body: &str) {
        let items = literal_items(env, body).unwrap();
        assert!(assign_literal(env, name, items, false).unwrap());
    }

    #[test]
    fn a_literal_body_becomes_its_elements() {
        let mut env = Environment::new();
        assign(&mut env, "oslo_l1", "1 2 3");
        assert_eq!(env.get_array("oslo_l1").unwrap().joined(","), "1,2,3");
    }

    #[test]
    fn quoting_decides_the_element_boundaries() {
        let mut env = Environment::new();
        env.set_var("oslo_al", "a b", false);
        assign(&mut env, "oslo_l2", "\"$oslo_al\" c");
        assert_eq!(env.get_array("oslo_l2").unwrap().len(), 2);
        assign(&mut env, "oslo_l3", "$oslo_al c");
        assert_eq!(env.get_array("oslo_l3").unwrap().len(), 3);
    }

    /// An explicit index places its element, and a literal may span lines and hold comments.
    #[test]
    fn an_explicit_index_moves_the_running_position() {
        let mut env = Environment::new();
        assign(&mut env, "oslo_l4", "a [5]=b c");
        let array = env.get_array("oslo_l4").unwrap();
        assert_eq!(array.indices().collect::<Vec<_>>(), vec![0, 5, 6]);
        assign(&mut env, "oslo_l7", "\n  [1]=''\n  [3]=b # note\n");
        let array = env.get_array("oslo_l7").unwrap();
        assert_eq!(array.indices().collect::<Vec<_>>(), vec![1, 3]);
        assert!(literal_items(&mut env, "a ) ; b (").is_err());
    }

    #[test]
    fn an_associative_literal_takes_keys() {
        let mut env = Environment::new();
        env.declare_assoc("oslo_l5");
        assign(&mut env, "oslo_l5", "[a]=1 [\"b c\"]=2");
        let map = env.get_assoc("oslo_l5").unwrap();
        assert_eq!(map.get("b c"), Some("2"));
        env.declare_assoc("oslo_l6");
        assign(&mut env, "oslo_l6", "k1 v1 k2 v2");
        assert_eq!(env.get_assoc("oslo_l6").unwrap().get("k2"), Some("v2"));
    }
}
