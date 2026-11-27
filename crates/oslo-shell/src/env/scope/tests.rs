//! Unit tests for the environment itself: the assignments that used to abort the process.
//!
//! In this file rather than at the bottom of `scope.rs` because the module is at the 600-line
//! limit the project enforces, and because these tests reach for private fields — which a child
//! module can do and a sibling could not.

use super::{Environment, is_valid_identifier};

#[test]
fn identifier_rules_match_posix_names() {
    for ok in ["a", "_", "_x9", "FOO_BAR", "x1"] {
        assert!(is_valid_identifier(ok), "{ok} should be a valid name");
    }
    for bad in ["", "=1", "1a", "a b", "a-b", "a=b", "a\0", "é", "$x"] {
        assert!(!is_valid_identifier(bad), "{bad:?} should be rejected");
    }
}

/// Each of these reached `env::set_var`/`remove_var` before R1.7 and aborted the shell.
#[test]
fn unrepresentable_assignments_are_refused_not_fatal() {
    let mut env = Environment::new();
    assert!(!env.set_var("=1", "x", true));
    assert!(!env.set_var("", "x", true));
    assert!(!env.set_var("a=b", "x", true));
    assert_eq!(env.get_var("=1"), None);

    // A NUL cannot survive the trip through `environ`, so the whole assignment is refused
    // rather than silently truncated at the NUL.
    assert!(!env.set_var("NUL_TEST", "a\0b", true));
    assert_eq!(env.get_var("NUL_TEST"), None);
}

#[test]
fn exporting_a_nul_bearing_value_is_refused() {
    let mut env = Environment::new();
    // Not exported, so this never touches `environ` — the value only becomes a problem when
    // `export` tries to publish it.
    env.vars.insert(
        "NUL_EXPORT".to_string(),
        super::Var::scalar("a\0b", super::Attrs::empty()),
    );
    assert!(!env.export_var("NUL_EXPORT"));
    assert!(!env.get_exported_vars().contains_key("NUL_EXPORT"));
}

#[test]
fn unset_of_an_impossible_name_is_a_no_op() {
    let mut env = Environment::new();
    env.unset_var("=1");
    env.unset_var("");
}

/// A rejected `local` must not be recorded in the scope frame: `pop_scope` would then hand
/// the same impossible name to `remove_var` and abort at function exit instead.
#[test]
fn rejected_local_does_not_poison_the_scope_frame() {
    let mut env = Environment::new();
    env.push_scope();
    assert!(!env.set_local_var("=1", "x"));
    assert!(!env.set_local_exported_var("BAD\0NAME", "x"));
    env.pop_scope();
}

#[test]
fn valid_unexported_assignment_still_works() {
    let mut env = Environment::new();
    assert!(env.set_var("PLAIN_TEST", "value", false));
    assert_eq!(env.get_var("PLAIN_TEST"), Some("value"));
}
