//! Which commands `set -e` is allowed to judge.
//!
//! Two pieces of state, both thread-local because the test binaries evaluate scripts on several
//! threads at once and one test's context must not leak into another's:
//!
//! - **a counter** of enclosing constructs that have suspended errexit — the condition of
//!   `if`/`while`/`until`, a non-final command of an and-or list, anything under `!`. It nests,
//!   so it counts rather than sets.
//! - **a flag** saying whether the status now in hand came from an exempt command, which has to
//!   outlive that command because the status does. A compound inherits its body's last status; if
//!   that status was never judgeable, the compound carrying it is not judgeable either.

use std::cell::Cell;

thread_local! {
    /// How many enclosing constructs have suspended `set -e`.
    ///
    /// A counter rather than a field on [`Environment`] because the exemption is a property of
    /// the *evaluation in progress*, not of a scope: a function called from a condition runs
    /// exempt (`set -e; f || true` runs all of `f`) even though it has its own variable scope,
    /// and the same function called normally does not. Dynamic extent is exactly what a counter
    /// around a `?`-bearing call expresses.
    ///
    /// `fork` copies it as it stands, which is what makes `if (false; echo x); then` print `x`:
    /// the subshell inherits the exemption its parent was under.
    ///
    /// Thread-local, not a plain `static`: the test binaries evaluate scripts on several threads
    /// at once, and one test's condition context must not exempt another's.
    static ERREXIT_SUSPENDED: Cell<u32> = const { Cell::new(0) };

    /// Whether the status just produced came from a command `set -e` may not judge.
    ///
    /// **The exemption has to outlive the command, because the status does.** `false && echo no`
    /// is exempt and leaves status 1 behind; put it last in an `if` body and the `if` inherits
    /// that 1, and judging the *compound* on it would punish the shell for a status POSIX said
    /// was not judgeable. bash, dash and busybox all carry on; oslo exited, which
    /// `/usr/sbin/on_ac_power` runs into on any machine with no battery.
    ///
    /// A flag rather than a counter: it describes one status, not a nesting extent, and each
    /// `run_and_record` clears it before running so nothing stale can be read.
    static STATUS_EXEMPT: Cell<bool> = const { Cell::new(false) };

    /// Whether the failure the status in hand describes has already fired the ERR trap.
    ///
    /// **One failing command is one ERR, however many constructs carry its status out.** A
    /// compound inherits its body's last status, so `f(){ false; }; f` failed twice as far as the
    /// trap could see — once for the `false` and once for the call reporting the 1 it left — and
    /// `case x in x) false;; esac` did the same. bash fires once.
    ///
    /// Set after the handler runs rather than before, because the handler's own commands come
    /// back through here and would clear it on the way.
    static ERR_REPORTED: Cell<bool> = const { Cell::new(false) };
}

/// Note whether the failing status in hand has already been reported to the ERR trap.
pub(super) fn set_err_reported(reported: bool) {
    ERR_REPORTED.with(|e| e.set(reported));
}

/// Whether the ERR trap has already run for the failure in hand.
pub(super) fn err_reported() -> bool {
    ERR_REPORTED.with(|e| e.get())
}

/// Note whether the status now in hand is one `set -e` may judge.
pub(super) fn set_status_exempt(exempt: bool) {
    STATUS_EXEMPT.with(|e| e.set(exempt));
}

/// Forget any exemption, because a boundary the exemption does not cross has been passed.
///
/// A function call is the one such boundary: the body's *status* is the call's status, but the
/// body's exemption is the body's own — see the note at the call site in
/// [`crate::exec::simple`].
pub(crate) fn clear_status_exempt() {
    set_status_exempt(false);
}

/// Whether the status now in hand came from an exempt command.
pub(super) fn status_exempt() -> bool {
    STATUS_EXEMPT.with(|e| e.get())
}

/// A live suspension of `set -e`; errexit resumes when this is dropped.
///
/// Returned rather than taking a closure so a caller can hold it across a `?` — the counter is
/// restored on the unwind path too, which is the whole reason it is a guard and not a pair of
/// calls.
pub(crate) struct ErrExitSuspension {
    // Not `Send`: the counter it decrements is the *creating* thread's.
    _not_send: std::marker::PhantomData<*const ()>,
}

impl Drop for ErrExitSuspension {
    fn drop(&mut self) {
        ERREXIT_SUSPENDED.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

/// Exempt everything evaluated while the returned guard lives from `set -e`.
///
/// The POSIX exemptions (2.9.1): the condition of `if`/`elif`/`while`/`until`, every command of an
/// and-or list but the last, and anything under `!`. They nest, so this counts rather than sets.
pub(crate) fn suspend_errexit() -> ErrExitSuspension {
    ERREXIT_SUSPENDED.with(|d| d.set(d.get() + 1));
    ErrExitSuspension {
        _not_send: std::marker::PhantomData,
    }
}

/// Whether an enclosing construct has exempted the command about to be judged.
///
/// Read by `crate::exec::simple::posix` as well as by errexit itself: bash applies the same
/// exemption list to POSIX 2.8.1's "a special builtin's utility error ends the shell", so
/// `bash --posix -c 'export BAD-NAME=1 || true; echo alive'` prints `alive` while the same
/// command on its own does not.
pub(crate) fn errexit_suspended() -> bool {
    ERREXIT_SUSPENDED.with(|d| d.get()) > 0
}
