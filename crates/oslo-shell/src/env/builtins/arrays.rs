//! Array literals as the *declaration builtins* see them.
//!
//! `a=(1 2)` written on its own reaches the evaluator as a structured assignment and never comes
//! here. `declare a=(1 2)`, `local a=(1 2)` and `readonly a=(1 2)` do not: an assignment written
//! after a command word is an ordinary argument, so the builtin receives the eight characters
//! `a=(1 2)` and this module is what turns them back into elements.
//!
//! Storing those characters as a scalar is exactly the failure this round removes — it is how
//! `declare a=(1 2); echo "$a"` used to print `(1 2)`.

use crate::env::Environment;
use crate::env::scope::{ShellArray, is_valid_identifier};
use crate::expand::arithmetic::eval_arithmetic;
use oslo_base::error::{Result, ShellError};

/// Expand the body of an array literal into the elements it denotes.
///
/// The body is read as the value of `a=(…)` is, so `declare -a a=([0]=x [2]=y)` places its
/// elements and a literal may span lines. An unindexed element is expanded in *list* context:
/// `declare -a a=($list)` and `a=(*.c)` produce one element per resulting field.
pub fn array_elements(env: &mut Environment, body: &str) -> Result<ShellArray> {
    let elements = crate::syntax::array_literal(body)?;
    crate::exec::simple::build_array(env, "", &elements, false)
}

/// Drop one element if `operand` names one — `unset 'a[1]'`.
///
/// `None` means the operand is an ordinary name and the caller should unset the whole variable.
/// Removing an element leaves a *hole*: `a=(1 2 3); unset 'a[1]'` keeps `a[2]` at index 2, which
/// is why the array store is sparse and why this cannot be a shift.
pub fn unset_element(env: &mut Environment, operand: &str) -> Option<Result<()>> {
    let body = operand.strip_suffix(']')?;
    let open = body.find('[')?;
    let (name, subscript) = (&body[..open], &body[open + 1..]);

    if !is_valid_identifier(name) {
        return None;
    }
    if env.is_readonly(name) {
        return Some(Err(ShellError::ExpansionError(format!(
            "{name}: cannot unset: readonly variable"
        ))));
    }
    // The subscript is arithmetic here too, so `unset 'a[i]'` reads `i`.
    Some(match eval_arithmetic(env, subscript) {
        Ok(index) => {
            env.unset_array_element(name, index);
            Ok(())
        }
        Err(e) => Err(e),
    })
}

#[cfg(test)]
mod tests {
    use super::{array_elements, unset_element};
    use crate::env::Environment;
    use crate::env::scope::ShellArray;

    #[test]
    fn a_literal_body_becomes_its_elements() {
        let mut env = Environment::new();
        let array = array_elements(&mut env, "1 2 3").unwrap();
        assert_eq!(array.joined(","), "1,2,3");
        assert!(array_elements(&mut env, "").unwrap().is_empty());
    }

    /// Quoting decides the element boundaries, exactly as in an `a=(…)` assignment.
    #[test]
    fn quoting_decides_the_element_boundaries() {
        let mut env = Environment::new();
        env.set_var("oslo_al", "a b", false);
        assert_eq!(array_elements(&mut env, "\"$oslo_al\" c").unwrap().len(), 2);
        assert_eq!(array_elements(&mut env, "$oslo_al c").unwrap().len(), 3);
    }

    /// An explicit index places its element, and a literal may span lines.
    #[test]
    fn an_explicit_index_places_its_element() {
        let mut env = Environment::new();
        let array = array_elements(&mut env, "[2]=x y").unwrap();
        assert_eq!(array.indices().collect::<Vec<_>>(), vec![2, 3]);
        let array = array_elements(&mut env, "\n  [1]=''\n  [3]=b # note\n").unwrap();
        assert_eq!(array.indices().collect::<Vec<_>>(), vec![1, 3]);
        assert_eq!(array.get(3), Some("b"));
        assert!(array_elements(&mut env, "a ) ; b (").is_err());
    }

    /// Unsetting an element leaves a hole rather than shifting what follows it down.
    #[test]
    fn unsetting_an_element_leaves_the_other_indices_alone() {
        let mut env = Environment::new();
        env.set_array("oslo_ue", ShellArray::from_values(["1", "2", "3"]));
        assert!(unset_element(&mut env, "oslo_ue[1]").is_some());
        let array = env.get_array("oslo_ue").unwrap();
        assert_eq!(array.indices().collect::<Vec<_>>(), vec![0, 2]);
        assert_eq!(array.joined(" "), "1 3");
    }

    /// A plain name is not this function's business; the caller unsets the whole variable.
    #[test]
    fn a_plain_name_is_not_an_element() {
        let mut env = Environment::new();
        assert!(unset_element(&mut env, "oslo_ue").is_none());
        assert!(unset_element(&mut env, "1bad[0]").is_none());
    }
}
