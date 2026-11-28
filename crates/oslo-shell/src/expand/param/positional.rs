//! `$@` and `$*` under the `-`, `=`, `+` and `?` operators.
//!
//! The list stays a list: `"${@:-x}"` with two positionals is two words, and `"${@+a b}"` with
//! none is no word at all. The tests read the list rather than a joined string, which is how
//! bash decides them:
//!
//! * the list is *set* when there is at least one positional;
//! * it is *null* when it is unset or holds one empty positional, except that a quoted `$*` is
//!   null exactly when its IFS-joined text is empty.

use super::{origin_of, payload_word_fields, positional_fields};
use crate::env::Environment;
use crate::expand::word::Field;
use oslo_base::ast::ParamExpansion;
use oslo_base::error::{Result, ShellError};

/// The fields `${@<op>word}` or `${*<op>word}` contributes, or `None` for any other operator.
pub(super) fn operator_fields(
    env: &mut Environment,
    name: &str,
    expansion_type: &ParamExpansion,
    in_quotes: bool,
) -> Result<Option<Vec<Field>>> {
    let own = |env: &Environment| positional_fields(env, name, origin_of(in_quotes));
    let fields = match expansion_type {
        ParamExpansion::DefaultValue {
            default,
            assign_if_unset,
            test_null,
        } => {
            if present(env, name, in_quotes, *test_null) {
                own(env)
            } else if *assign_if_unset {
                return Err(ShellError::ExpansionError(format!(
                    "${name}: cannot assign in this way"
                )));
            } else {
                payload_word_fields(env, default, in_quotes)?
            }
        }
        ParamExpansion::UseAlternative {
            alternative,
            test_null,
        } => {
            if present(env, name, in_quotes, *test_null) {
                payload_word_fields(env, alternative, in_quotes)?
            } else {
                own(env)
            }
        }
        ParamExpansion::ErrorIfUnset { message, test_null } => {
            if present(env, name, in_quotes, *test_null) {
                own(env)
            } else {
                let msg = crate::expand::word::expand_word_to_string(env, message)?;
                let msg = if msg.is_empty() {
                    "parameter null or not set".to_string()
                } else {
                    msg
                };
                return Err(ShellError::UnsetParameter(format!("{name}: {msg}")));
            }
        }
        _ => return Ok(None),
    };
    Ok(Some(fields))
}

/// Whether the positional list counts as present for an operator with this `test_null`.
fn present(env: &Environment, name: &str, in_quotes: bool, test_null: bool) -> bool {
    let list = env.get_positional();
    if list.is_empty() {
        return false;
    }
    if !test_null {
        return true;
    }
    if name == "*" && in_quotes {
        return !list.join(&env.ifs_separator()).is_empty();
    }
    list.len() > 1 || !list[0].is_empty()
}
