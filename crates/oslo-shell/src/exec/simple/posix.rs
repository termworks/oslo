//! What POSIX mode changes about the *outcome* of a command.
//!
//! POSIX 2.8.1 says a non-interactive shell exits when a **special builtin** hits a utility error
//! and when a **variable assignment error** happens. Both rules are narrower than they first look,
//! and getting them wrong in the generous direction kills the shell on ordinary failures. Measured
//! against `bash --posix -c`:
//!
//! ```text
//! shift 5; echo alive              diagnostic, "alive", status 0   -- shift is special, not fatal
//! export BAD-NAME=1; echo alive    diagnostic, no "alive", status 1 -- fatal
//! set -o nosuchopt; echo alive     diagnostic, no "alive", status 2 -- fatal, `set`'s own status
//! readonly r=1; r=2; echo alive    diagnostic, no "alive", status 127 -- fatal
//! : > /nonexistent/x; echo alive   diagnostic, no "alive", status 1 -- fatal
//! echo hi > /nonexistent/x; …      diagnostic, "alive", status 0    -- `echo` is not special
//! false; cd /nowhere; command -Z   all carry on
//! ```
//!
//! So the rule is *utility error*, not *non-zero status*: `shift 5` returns non-zero and lives.
//! A builtin says which it had by returning [`ShellError::UtilityError`] rather than `Ok(n)`;
//! there is no other way to tell, because a usage error is an ordinary `Ok(2)`.
//!
//! Fatal here means `ShellError::Exit`, not a new unwinding path: `exit` is already the thing the
//! whole evaluator knows how to carry — a subshell absorbs it and the parent carries on
//! (`bash --posix -c '(readonly r=1; r=2); echo outer'` prints `outer`), a function does not, and
//! the EXIT trap still runs.

use crate::env::Environment;
use crate::env::scope::is_special_builtin;
use oslo_base::error::{Result, ShellError};

/// Whether POSIX's "the shell shall exit" rules apply to this shell at all.
///
/// Interactive shells are excluded by POSIX itself, and bash agrees: a typo at a `--posix` prompt
/// must not log you out.
fn exits_on_error(env: &Environment) -> bool {
    env.posix() && !env.interactive()
}

/// Fold a builtin's result into the status the shell carries on with.
///
/// The only reader of [`ShellError::UtilityError`]. Anything else — including an ordinary
/// non-zero `Ok`, which is what `shift 5` produces — is passed straight through.
pub(super) fn resolve_builtin_result(
    env: &Environment,
    name: &str,
    result: Result<i32>,
) -> Result<i32> {
    match result {
        Err(ShellError::UtilityError { status, fatal, .. }) => {
            // The same contexts `set -e` exempts also exempt this, and bash agrees:
            // `export BAD-NAME=1` on its own ends a `--posix` shell, `export BAD-NAME=1 || true`
            // and `if export BAD-NAME=1; then :; fi` both print what follows them.
            //
            // The exemption is deliberately *not* extended to `redirect_failure` or
            // `assignment_failure` below, which bash keeps fatal even under `||` — measured, not
            // assumed: `bash --posix -c ': > /nonexistent/x || true; echo alive'` prints nothing.
            let exempt = crate::exec::pipeline::errexit_suspended();
            if exits_on_error(env) && is_special_builtin(name) && !exempt {
                Err(ShellError::Exit(fatal))
            } else {
                Ok(status)
            }
        }
        other => other,
    }
}

/// A redirection that could not be set up, on a command that is about to run as a builtin.
///
/// `status` is what [`super::report_redirect_failure`] already decided the command is worth, and
/// the diagnostic is already printed. All that is left is whether the shell survives it — and for
/// a special builtin under POSIX it does not, which is the half `report_redirect_failure`
/// documented and deliberately skipped for six rounds because there was no POSIX mode to key off.
pub(super) fn redirect_failure(env: &Environment, name: &str, status: i32) -> Result<i32> {
    if exits_on_error(env) && is_special_builtin(name) {
        return Err(ShellError::Exit(status));
    }
    Ok(status)
}

/// An assignment the environment refused: read-only, or a name it cannot represent.
///
/// POSIX calls this a *variable assignment error* and ends a non-interactive shell over it, with
/// no builtin involved at all — `r=2` on a read-only `r` is enough. Everywhere else the rest of
/// the top-level command is abandoned with status 1, which is what bash does outside POSIX mode.
///
/// The diagnostic came from `Environment::set_var`, which is where the reason is known.
pub(super) fn assignment_failure(env: &Environment, name: &str) -> Result<i32> {
    let err = ShellError::assignment_error(format!("{}: assignment failed", name));
    if exits_on_error(env) {
        // 127 only where bash gives one, which is `-c`. The same program in a file exits 1 —
        // see `exit_error_status` in `main`, which draws the same line for every other fatal
        // error and cannot draw it here because this one becomes an `Exit` before it gets there.
        return Err(ShellError::Exit(
            match env.option(crate::env::options::ShellOption::CommandString) {
                true => err.fatal_exit_status(env.posix()),
                false => err.failure_status(),
            },
        ));
    }
    // Outside POSIX mode bash abandons the rest of the top-level command instead, with status 1.
    Err(ShellError::Abandoned)
}

/// Whether command search puts a special builtin ahead of a shell function (POSIX 2.9.1.1).
///
/// bash follows POSIX here only under `--posix`, so this is a mode question rather than a
/// conformance one — and unlike the rules above it applies to interactive shells too.
pub(super) fn special_builtins_outrank_functions(env: &Environment) -> bool {
    env.posix()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::options::ShellOption;
    use oslo_base::error::FATAL_EXIT_STATUS;

    fn posix_env() -> Environment {
        let mut env = Environment::new();
        env.set_option(ShellOption::Posix, true);
        env
    }

    /// The regression this module exists to prevent: `shift 5` fails, and a naive `status != 0`
    /// test would have ended the shell over it.
    #[test]
    fn an_ordinary_non_zero_status_is_not_fatal() {
        let env = posix_env();
        let out = resolve_builtin_result(&env, "shift", Ok(1));
        assert_eq!(out.expect("not fatal"), 1);
    }

    /// A special builtin's utility error ends a POSIX shell, with the builtin's own status.
    #[test]
    fn a_special_builtin_utility_error_exits_a_posix_shell() {
        let env = posix_env();
        let err = ShellError::utility_error("export: not a valid identifier", 1);
        match resolve_builtin_result(&env, "export", Err(err)) {
            Err(ShellError::Exit(1)) => {}
            other => panic!("expected Exit(1), got {:?}", other),
        }
        let err = ShellError::utility_error("set: invalid option name", 2);
        match resolve_builtin_result(&env, "set", Err(err)) {
            Err(ShellError::Exit(2)) => {}
            other => panic!("expected Exit(2), got {:?}", other),
        }
    }

    /// Outside POSIX mode the same error is just a failed command — bash prints the diagnostic,
    /// returns 1 and runs the next command.
    #[test]
    fn the_same_error_is_survivable_without_posix_mode() {
        let env = Environment::new();
        let err = ShellError::utility_error("export: not a valid identifier", 1);
        let out = resolve_builtin_result(&env, "export", Err(err));
        assert_eq!(out.expect("not fatal"), 1);
    }

    /// Special is the operative word: `read` and `cd` are builtins, not special ones.
    #[test]
    fn a_regular_builtin_survives_its_own_utility_error() {
        let env = posix_env();
        let err = ShellError::utility_error("read: bad option", 2);
        let out = resolve_builtin_result(&env, "read", Err(err));
        assert_eq!(out.expect("not fatal"), 2);
    }

    /// An interactive POSIX shell reports and carries on; POSIX exempts it, and a shell that
    /// logged the user out on a typo would be unusable.
    #[test]
    fn an_interactive_posix_shell_does_not_exit() {
        let mut env = posix_env();
        env.set_option(ShellOption::Interactive, true);
        let err = ShellError::utility_error("export: not a valid identifier", 1);
        assert_eq!(
            resolve_builtin_result(&env, "export", Err(err)).expect("not fatal"),
            1
        );
        assert_eq!(redirect_failure(&env, ":", 1).expect("not fatal"), 1);
        assert!(matches!(
            assignment_failure(&env, "r"),
            Err(ShellError::Abandoned)
        ));
    }

    #[test]
    fn a_redirection_error_is_fatal_only_on_a_special_builtin() {
        let env = posix_env();
        match redirect_failure(&env, ":", 1) {
            Err(ShellError::Exit(1)) => {}
            other => panic!("expected Exit(1), got {:?}", other),
        }
        assert_eq!(redirect_failure(&env, "echo", 1).expect("not fatal"), 1);
        assert_eq!(
            redirect_failure(&Environment::new(), ":", 1).expect("not fatal"),
            1
        );
    }

    /// An assignment error needs no builtin, and its statuses differ three ways: 1 for the
    /// command, and for the shell that gave up either 127 or 1 depending on how it was started.
    ///
    /// **127 only under `-c`.** `bash --posix -c 'readonly r=1; r=2'` answers 127 and the same
    /// program in a file answers 1 — the `-c` path exits by a route that leaves 127 behind. oslo
    /// answered 127 to both.
    #[test]
    fn an_assignment_error_is_fatal_in_posix_mode_only() {
        let mut dash_c = posix_env();
        dash_c.set_option(crate::env::options::ShellOption::CommandString, true);
        match assignment_failure(&dash_c, "r") {
            Err(ShellError::Exit(status)) => assert_eq!(status, FATAL_EXIT_STATUS),
            other => panic!("expected Exit({FATAL_EXIT_STATUS}), got {:?}", other),
        }
        // A script file: still fatal, still POSIX-only, but the ordinary failure status.
        match assignment_failure(&posix_env(), "r") {
            Err(ShellError::Exit(status)) => assert_eq!(status, 1),
            other => panic!("expected Exit(1), got {:?}", other),
        }
        // Outside POSIX mode the rest of the top-level command is abandoned instead.
        assert!(matches!(
            assignment_failure(&Environment::new(), "r"),
            Err(ShellError::Abandoned)
        ));
    }
}
