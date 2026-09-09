//! Waiting on a child, and turning what came back into a status.
//!
//! Split from the pipeline itself because it is a different question: these answer *what happened*
//! to a process, where the rest of the module decides what to run next.

use super::*;

/// Whether this shell is talking to a person.
///
/// Process-global rather than a field on [`Environment`] because it is a property of the
/// invocation, not of a scope, and every forked child inherits it as it stands. `main` sets it
/// when it starts the REPL; a `-c` command or a script leaves it false, which is what keeps the
/// job notice out of `x=$(cmd &)`.
pub(super) static INTERACTIVE: AtomicBool = AtomicBool::new(false);

/// Declare whether the shell is interactive. Called once, by `main`.
pub fn set_interactive(yes: bool) {
    INTERACTIVE.store(yes, Ordering::Relaxed);
}

/// Whether the shell is interactive; see [`set_interactive`].
pub fn is_interactive() -> bool {
    INTERACTIVE.load(Ordering::Relaxed)
}

/// Collapse an evaluation result to the single number a caller with no richer channel can report.
///
/// This is the *only* correct way to turn a `Result<i32>` into an exit status in a forked child
/// — a subshell, a pipeline stage, a background job, a command substitution. Writing
/// `.unwrap_or(1)` there instead is what made `( exit 3 )` report 1: `exit`/`return` unwind as
/// errors carrying their code, and discarding the error discards the code along with the
/// diagnostic for every real failure.
pub fn status_of(result: Result<i32>) -> i32 {
    match result {
        Ok(status) => status,
        Err(err) => report_error_status(err),
    }
}

/// Report `err` and give the status it leaves behind.
///
/// Control flow (`exit`, `return`, `break`, `continue`) passes through silently with its own
/// code. A genuine failure is announced on stderr first: in a forked child this is the last
/// place the diagnostic can be printed at all, since the parent will only ever see a number.
pub fn report_error_status(err: ShellError) -> i32 {
    if let Some(status) = err.control_flow_status() {
        return status;
    }
    eprintln!("oslo: {}", err);
    err.failure_status()
}

/// Reap `pid` and turn its wait status into the number a shell reports for it.
///
/// One `waitpid` call per iteration, then a `match` on the bound result. Asking twice — once per
/// `if let` arm — cannot work: the second call has no child left to reap, so the `Signaled` arm
/// never fires and a killed child reports as though it had exited cleanly.
///
/// R7.6: `EINTR` is now a normal outcome rather than an impossible one. The SIGINT handler is
/// installed without `SA_RESTART`, so a keystroke arriving while the shell waits fails the wait
/// instead of restarting it — and a signal says nothing about how the child ended, so the only
/// correct response is to ask again.
///
/// R7.2: `WUNTRACED`, for the same reason [`crate::exec::simple`] uses it — a child stopped by
/// Ctrl-Z reports *termination* to nobody, so a plain `waitpid` would block the shell forever on
/// a process only `fg` could revive.
pub fn wait_for_status(pid: Pid) -> i32 {
    wait_for_child(pid).0
}

/// [`wait_for_status`], plus whether the child *stopped* rather than ended.
///
/// The two are indistinguishable from the status alone — both report 128 + the signal — and a
/// pipeline has to tell them apart: a stopped job goes into the job table for `fg` to find, while
/// a killed one is simply over.
pub(crate) fn wait_for_child(pid: Pid) -> (i32, bool) {
    loop {
        return match waitpid(pid, Some(WaitPidFlag::WUNTRACED)) {
            Ok(WaitStatus::Exited(_, code)) => (code, false),
            Ok(WaitStatus::Signaled(_, sig, _)) => (128 + sig as i32, false),
            Ok(WaitStatus::Stopped(_, sig)) => (128 + sig as i32, true),
            Ok(WaitStatus::StillAlive) => continue,
            // A SIGTERM or SIGHUP with an EXIT trap set ends the *wait*, not just this call: the
            // shell is dying, and sitting out the rest of a `sleep 20` first would leave the
            // cleanup that long undone. The signal is left standing for the command boundary
            // just after this to act on — see `job::fatal_signal_waiting`.
            Err(nix::errno::Errno::EINTR) => match crate::exec::job::fatal_signal_waiting() {
                Some(signum) => (128 + signum, false),
                None => continue,
            },
            // Nothing was reaped, so there is no status to report: 127 is what a shell says when
            // it cannot run or account for a command.
            _ => (127, false),
        };
    }
}
