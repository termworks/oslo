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
    if matches!(err, ShellError::Abandoned) {
        return 1;
    }
    eprintln!("oslo: {}", err);
    err.failure_status()
}

thread_local! {
    /// The last child reaped because a signal killed it: its pid, the signal, and a core dump.
    static DEATH: std::cell::Cell<Option<(Pid, i32, bool)>> = const { std::cell::Cell::new(None) };
    /// Set in a command substitution and in a pipeline stage before the last, which report no
    /// deaths, as bash's do not.
    static QUIET_DEATHS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Report no signal deaths from this process on.
pub(crate) fn quiet_deaths() {
    QUIET_DEATHS.with(|q| q.set(true));
}

/// Record that `pid` died of `signal`, for [`announce_death`].
pub(crate) fn note_death(pid: Pid, signal: i32, core: bool) {
    DEATH.with(|d| d.set(Some((pid, signal, core))));
}

/// Forget any death recorded so far, before waiting on a command that may report one.
pub(crate) fn forget_death() {
    DEATH.with(|d| d.set(None));
}

/// Say on stderr that a foreground command was killed by a signal, as a non-interactive bash
/// does: `Killed`, `Segmentation fault (core dumped)` and the rest, but not SIGINT or SIGPIPE.
///
/// Only `of`, when it is given: a pipeline reports the death of its last stage alone.
pub(crate) fn announce_death(env: &Environment, of: Option<Pid>, command: impl FnOnce() -> String) {
    let Some((pid, signal, core)) = DEATH.with(|d| d.take()) else {
        return;
    };
    let quiet = QUIET_DEATHS.with(std::cell::Cell::get) || is_interactive() || env.posix();
    if quiet || signal == nix::libc::SIGINT || signal == nix::libc::SIGPIPE {
        return;
    }
    if of.is_some_and(|wanted| wanted != pid) {
        return;
    }
    // SAFETY: `strsignal` returns a pointer to a static string, read before anything else runs.
    let name = unsafe { std::ffi::CStr::from_ptr(nix::libc::strsignal(signal)) };
    let name = name.to_string_lossy();
    let core = if core { "(core dumped) " } else { "" };
    let text = command();
    if signal == nix::libc::SIGTERM {
        eprintln!("{name:<27}{core}{text}");
    } else {
        eprintln!("{}{pid} {name:<27}{core}{text}", env.origin());
    }
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
            Ok(WaitStatus::Signaled(_, sig, core)) => {
                note_death(pid, sig as i32, core);
                (128 + sig as i32, false)
            }
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
