//! Array elements as builtins and assignments name them: `a[1]`, `m[key]`, `a[@]`.
//!
//! Every builtin that takes a name — `declare`, `local`, `read`, `printf -v`, `mapfile`, `unset` —
//! may be handed `name[subscript]` instead, and every one of them goes through here, so a subscript
//! means the same thing whichever of them was given it: arithmetic for an indexed array, a string
//! for an associative one.

mod literal;

pub use literal::{LiteralItem, assign_literal, literal_items, literal_items_from_ast};

use crate::env::Environment;
use crate::env::scope::is_valid_identifier;
use crate::env::scope::nameref::split_subscript;
use oslo_base::error::{Result, ShellError};

/// Where an element write lands once its subscript is evaluated.
pub enum Key {
    Index(i64),
    Name(String),
}

/// A name as a builtin's operand may give it: plain, or one element.
pub enum Target<'a> {
    Name(&'a str),
    Element { name: &'a str, subscript: &'a str },
}

/// Read `text` as a target, or `None` if it is neither a name nor `name[subscript]`.
pub fn parse_target(text: &str) -> Option<Target<'_>> {
    if is_valid_identifier(text) {
        return Some(Target::Name(text));
    }
    let (name, subscript) = split_subscript(text)?;
    (is_valid_identifier(name) && !subscript.is_empty())
        .then_some(Target::Element { name, subscript })
}

/// The error a subscript that names no element answers with.
pub fn bad_subscript(what: &str) -> ShellError {
    ShellError::ExpansionError(format!("{what}: bad array subscript"))
}

/// Expand a subscript written in a builtin's operand, the way a word inside `"…"` expands.
fn expand_subscript_text(env: &mut Environment, text: &str) -> Result<String> {
    if !text.contains(['$', '`', '\'', '"', '\\']) {
        return Ok(text.to_string());
    }
    let word = crate::lexer::parse_single_word(text)?;
    crate::expand::expand_word_to_string(env, &word)
}

/// The key `text` names in `name`: the string itself for an associative array, its arithmetic
/// value for anything else. Only an empty subscript is refused; a blank one is index 0.
pub fn key_for(env: &mut Environment, name: &str, text: &str) -> Result<Key> {
    if text.is_empty() {
        return Err(bad_subscript(&format!("{name}[{text}]")));
    }
    if env.is_assoc(name) {
        return Ok(Key::Name(text.to_string()));
    }
    Ok(Key::Index(index_of(env, text)?))
}

/// The key an assignment's `name[subscript]` names. An indexed array's subscript is expanded once
/// as arithmetic text and read as it stands, as `${name[subscript]}` reads it.
pub fn assigned_key(
    env: &mut Environment,
    name: &str,
    index: &oslo_base::ast::Word,
) -> Result<Key> {
    if env.is_assoc(name) {
        let text = crate::expand::expand_word_to_string(env, index)?;
        return key_for(env, name, &text);
    }
    if index.is_unwritten() {
        return Err(bad_subscript(&format!("{name}[]")));
    }
    let text = crate::expand::arithmetic::expand_expression_word(env, index)?;
    match text.trim() {
        "" => Ok(Key::Index(0)),
        trimmed => Ok(Key::Index(
            crate::expand::arithmetic::eval_expanded_subscript(env, trimmed)?,
        )),
    }
}

/// `-v name[subscript]` with the subscript evaluated as an element write would be, expanded first
/// when `expand`, written back as `name[key]`; `None` when the operand names no array element.
pub(crate) fn resolve_element(
    env: &mut Environment,
    operand: &str,
    expand: bool,
) -> Result<Option<String>> {
    let Some(Target::Element { name, subscript }) = parse_target(operand) else {
        return Ok(None);
    };
    if matches!(subscript, "@" | "*") {
        return Ok(None);
    }
    let key = match expand {
        true => {
            let text = expand_subscript_text(env, subscript)?;
            key_for(env, name, &text)?
        }
        false if env.is_assoc(name) => key_for(env, name, subscript)?,
        false => match subscript.trim() {
            "" => Key::Index(0),
            text => Key::Index(crate::expand::arithmetic::eval_expanded_subscript(
                env, text,
            )?),
        },
    };
    Ok(Some(match key {
        Key::Index(index) => format!("{name}[{index}]"),
        Key::Name(key) => format!("{name}[{key}]"),
    }))
}

/// The index a subscript of an indexed array evaluates to: a blank one is 0.
pub(super) fn index_of(env: &mut Environment, text: &str) -> Result<i64> {
    match text.trim() {
        "" => Ok(0),
        trimmed => crate::expand::arithmetic::eval_subscript(env, trimmed),
    }
}

/// `name[subscript]=value` where the subscript is still text, as a builtin's operand gives it.
pub fn assign_element(
    env: &mut Environment,
    name: &str,
    subscript: &str,
    value: &str,
) -> Result<bool> {
    let text = expand_subscript_text(env, subscript)?;
    let key = key_for(env, name, &text)?;
    write_element(env, name, key, value, false)
}

/// Assign `value` to `text`, which may be a plain name or `name[subscript]`.
///
/// `Ok(false)` when the assignment was refused, with the reason already printed.
pub fn assign_target(env: &mut Environment, text: &str, value: &str) -> Result<bool> {
    match parse_target(text) {
        Some(Target::Name(name)) => Ok(env.set_var(name, value, false)),
        Some(Target::Element { name, subscript }) => assign_element(env, name, subscript, value),
        None => Err(ShellError::ExpansionError(format!(
            "`{}': not a valid identifier",
            oslo_base::shown::shown(text)
        ))),
    }
}

/// Write one element, appending to what is there when `append`.
///
/// A negative index counts back from the end; one that reaches before the start is an error.
pub fn write_element(
    env: &mut Environment,
    name: &str,
    key: Key,
    value: &str,
    append: bool,
) -> Result<bool> {
    match key {
        Key::Name(key) => {
            let existing = env
                .get_assoc(name)
                .and_then(|m| m.get(&key))
                .map(str::to_string);
            let value = combined(env, name, existing, value, append);
            Ok(env.set_assoc_element(name, &key, &value))
        }
        Key::Index(index) => {
            let resolved = match env.get_array(name) {
                Some(array) => array.resolve(index),
                None => {
                    let highest = i64::from(env.get_var(name).is_some()) - 1;
                    let resolved = highest + 1 + index;
                    match index {
                        i if i >= 0 => Some(i),
                        _ => (resolved >= 0).then_some(resolved),
                    }
                }
            };
            let Some(resolved) = resolved else {
                return Err(bad_subscript(&format!("{name}[{index}]")));
            };
            let existing = element_text(env, name, resolved);
            let value = combined(env, name, existing, value, append);
            Ok(env.set_array_element(name, resolved, &value))
        }
    }
}

/// One element of an indexed array, or of a scalar seen as one.
fn element_text(env: &Environment, name: &str, index: i64) -> Option<String> {
    match env.get_array(name) {
        Some(array) => array.get(index).map(str::to_string),
        None if index == 0 => env.get_var(name).map(str::to_string),
        None => None,
    }
}

/// `name+=value` without a subscript: it extends `$name`, which on an array is element 0.
pub fn append_scalar(env: &mut Environment, name: &str, value: &str) -> bool {
    let existing = env.get_var(name).map(str::to_string);
    let combined = combined(env, name, existing, value, true);
    env.set_var(name, &combined, false)
}

/// What `+=` stores: the sum for an integer name, the concatenation otherwise.
fn combined(
    env: &Environment,
    name: &str,
    existing: Option<String>,
    value: &str,
    append: bool,
) -> String {
    if !append {
        return value.to_string();
    }
    let existing = existing.unwrap_or_default();
    if env.is_integer(name) {
        let or_zero = |t: &str| match t.trim().is_empty() {
            true => "0".to_string(),
            false => t.to_string(),
        };
        return format!("({})+({})", or_zero(&existing), or_zero(value));
    }
    oslo_base::lossless::normalize(format!("{existing}{value}"))
}

/// Drop one element if `operand` names one — `unset 'a[1]'`, `unset 'm[key]'`.
///
/// `None` means the operand is an ordinary name and the caller should unset the whole variable.
/// `a[@]` and `a[*]` empty an indexed array; on an associative array they are ordinary keys.
pub fn unset_element(env: &mut Environment, operand: &str) -> Option<Result<()>> {
    let (name, subscript) = split_subscript(operand)?;
    if !is_valid_identifier(name) {
        return None;
    }
    if env.is_readonly(name) {
        return Some(Err(ShellError::ExpansionError(format!(
            "{name}: cannot unset: readonly variable"
        ))));
    }
    if !env.is_assoc(name) && matches!(subscript, "@" | "*") {
        if env.get_array(name).is_some() {
            env.set_array(name, crate::env::scope::ShellArray::default());
        } else {
            env.unset_var(name);
        }
        return Some(Ok(()));
    }
    Some((|| {
        let text = expand_subscript_text(env, subscript)?;
        match key_for(env, name, &text)? {
            Key::Name(key) => env.unset_assoc_element(name, &key),
            Key::Index(index) => {
                let in_range = match env.get_array(name) {
                    Some(array) => array.resolve(index).is_some(),
                    None => index >= -1,
                };
                if !in_range {
                    return Err(bad_subscript(&format!("[{index}]")));
                }
                env.unset_array_element(name, index);
            }
        }
        Ok(())
    })())
}

#[cfg(test)]
mod tests {
    use super::{assign_target, unset_element};
    use crate::env::Environment;
    use crate::env::scope::ShellArray;

    #[test]
    fn unsetting_an_element_leaves_the_other_indices_alone() {
        let mut env = Environment::new();
        env.set_array("oslo_ue", ShellArray::from_values(["1", "2", "3"]));
        assert!(unset_element(&mut env, "oslo_ue[1]").is_some());
        let array = env.get_array("oslo_ue").unwrap();
        assert_eq!(array.indices().collect::<Vec<_>>(), vec![0, 2]);
        assert_eq!(array.joined(" "), "1 3");
    }

    #[test]
    fn a_plain_name_is_not_an_element() {
        let mut env = Environment::new();
        assert!(unset_element(&mut env, "oslo_ue").is_none());
        assert!(unset_element(&mut env, "1bad[0]").is_none());
    }

    /// The shared helper behind `read a[1]`, `printf -v 'a[1]'` and `declare 'a[1]=x'`.
    #[test]
    fn a_target_may_be_an_element() {
        let mut env = Environment::new();
        assert!(assign_target(&mut env, "oslo_t1[2]", "x").unwrap());
        assert_eq!(env.get_array("oslo_t1").unwrap().get(2), Some("x"));
        env.declare_assoc("oslo_t2");
        assert!(assign_target(&mut env, "oslo_t2[k k]", "v").unwrap());
        assert_eq!(env.get_assoc("oslo_t2").unwrap().get("k k"), Some("v"));
        assert!(assign_target(&mut env, "1x", "v").is_err());
    }
}
