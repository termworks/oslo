//! The `${!…}` forms that are not a second lookup of one name.
//!
//! `${!prefix*}` and `${!prefix@}` list the set variables whose names start with `prefix`;
//! `${!ref}` on a nameref is the name it holds; and an indirection whose target is `a[@]` stands
//! for every element of `a`, so `"${!i}"` with `i='a[@]'` is as many words as `a` has.

use super::array::{expand_array_ref, list_fields};
use super::origin_of;
use crate::env::Environment;
use crate::env::scope::is_valid_identifier;
use crate::env::scope::nameref::split_subscript;
use crate::expand::word::{Field, Run};
use oslo_base::ast::{ParamExpansion, Subscript};
use oslo_base::error::Result;

/// The fields `${!name<inner>}` contributes, or `None` when it is an ordinary indirection.
pub(super) fn fields(
    env: &mut Environment,
    name: &str,
    inner: &ParamExpansion,
    in_quotes: bool,
) -> Result<Option<Vec<Field>>> {
    let origin = origin_of(in_quotes);
    let plain = matches!(inner, ParamExpansion::Normal);
    if plain
        && let Some(prefix) = name.strip_suffix('*').or_else(|| name.strip_suffix('@'))
        && is_valid_identifier(prefix)
    {
        let names = env.names_with_prefix(prefix);
        let separate = name.ends_with('@');
        return Ok(Some(list_fields(
            &names,
            separate,
            &env.ifs_separator(),
            origin,
        )));
    }
    if plain && let Some(target) = env.nameref_target(name) {
        return Ok(Some(vec![vec![Run::new(target.to_string(), origin)]]));
    }
    let Some(value) = env.get_param(name) else {
        return Ok(None);
    };
    match split_subscript(&value) {
        Some((array, sub @ ("@" | "*"))) if is_valid_identifier(array) => {
            let subscript = match sub {
                "@" => Subscript::All,
                _ => Subscript::Joined,
            };
            let array = array.to_string();
            expand_array_ref(env, &array, &subscript, inner, in_quotes).map(Some)
        }
        _ => Ok(None),
    }
}
