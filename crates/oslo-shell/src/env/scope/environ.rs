//! Publishing shell variables to the process environment.
//!
//! Split from `scope.rs` because it is one idea with one soundness argument: every write to
//! `environ` in the shell funnels through here, so the `unsafe` justification is stated once
//! rather than repeated at each call site.

use std::env;

/// Whether `name`/`value` can be handed to the process environment without aborting.
///
/// Deliberately weaker than [`is_valid_identifier`]: names inherited from `environ` may contain
/// characters no shell would accept (`BASH_FUNC_x%%`), and forwarding those to a child is
/// correct. Only the three things `environ` genuinely cannot represent are refused — an empty
/// name, a `=` in a name, and a NUL anywhere, since NUL terminates a C string.
pub(super) fn is_environ_safe(name: &str, value: &str) -> bool {
    !name.is_empty() && !name.contains(['=', '\0']) && !value.contains('\0')
}

/// Report a name/value the process environment cannot represent; `true` if it was rejected.
///
/// The last line of defence for callers that reach [`Environment::set_var`] without doing their
/// own validation (`read`, a `for` loop variable, a `${x=default}` expansion): the assignment is
/// dropped with a diagnostic rather than aborting the shell.
/// `origin` is [`Environment::origin`]'s answer, so a script names its own file and line — the
/// caller has it and this does not, which is the only reason it is a parameter.
pub(super) fn reject_unrepresentable(origin: &str, name: &str, value: &str) -> bool {
    if name.is_empty() || name.contains(['=', '\0']) {
        eprintln!(
            "{origin}{}: not a valid identifier",
            oslo_base::shown::shown(name)
        );
        true
    } else if value.contains('\0') {
        eprintln!("{origin}{name}: value contains a NUL byte");
        true
    } else {
        false
    }
}

/// Publish `name=value` to the process environment; a pair `environ` cannot hold is dropped.
///
/// Every write to `environ` in this module funnels through here so the soundness argument lives
/// in exactly one place.
pub(super) fn environ_set(name: &str, value: &str) {
    if !is_environ_safe(name, value) {
        return;
    }
    // SAFETY: `std::env::set_var` is unsafe in edition 2024 because it mutates the global
    // `environ` with no synchronisation, so a concurrent `getenv` in another thread could read a
    // freed pointer. oslo is single-threaded: nothing in the crate spawns a thread, the parser,
    // interpreter, builtins and Lua engine all run on the main thread, and a forked child starts
    // with only the forking thread alive. The guard above rules out the other failure mode — the
    // call panics on an empty name, a `=` in the name, or a NUL in either half.
    unsafe { env::set_var(name, value) }
}

/// Drop `name` from the process environment; a name `environ` cannot hold is ignored.
pub(super) fn environ_remove(name: &str) {
    if !is_environ_safe(name, "") {
        return;
    }
    // SAFETY: as in `environ_set` — no other thread exists to observe the mutation, and the
    // guard above excludes the names `remove_var` panics on.
    unsafe { env::remove_var(name) }
}
