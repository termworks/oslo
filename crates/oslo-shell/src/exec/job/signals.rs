//! What the shell does about signals, and what it undoes in every process it forks.
//!
//! Two halves that have to agree: [`install_shell_signals`] decides what the *shell* ignores so a
//! keystroke aimed at a foreground job cannot kill the REPL, and [`reset_signals_for_child`]
//! undoes exactly that in every process the shell forks.

use nix::libc;
use nix::sys::signal::{self, SaFlags, SigAction, SigHandler, SigSet, SigmaskHow, Signal};
use std::sync::atomic::{AtomicBool, Ordering};

/// Every signal whose disposition the shell may have changed for its own benefit.
///
/// SIGPIPE is in the list even though the shell never touches it deliberately: the Rust runtime
/// sets it to `SIG_IGN` before `main` so a write to a closed socket returns `EPIPE` instead of
/// killing the process. An ignored disposition survives `execv` (only *handled* signals are reset
/// by exec), so without this every command oslo runs inherits it — which is why `yes | head -1`
/// printed `yes: standard output: Broken pipe` instead of dying quietly on the closed pipe.
/// SIGHUP and SIGTERM are here for [`catch_fatal_for_exit_trap`]: `exec` would reset a *handled*
/// signal on its own, but a forked subshell never execs, and one carrying the parent's handler
/// would record a fatal signal instead of dying of it.
const RESET_IN_CHILD: [Signal; 8] = [
    Signal::SIGPIPE,
    Signal::SIGINT,
    Signal::SIGQUIT,
    Signal::SIGTSTP,
    Signal::SIGTTIN,
    Signal::SIGTTOU,
    Signal::SIGHUP,
    Signal::SIGTERM,
];

/// The signals a `trap '' SIG` has deliberately ignored, one bit per signal number.
///
/// **An ignored signal is inherited, and that is the point of ignoring it.** POSIX says a child
/// starts with the dispositions its parent had, save that *caught* signals become the default;
/// `trap '' INT` is how a script makes a long job immune to a stray Ctrl-C, and bash and dash both
/// pass it on (`SigIgn` carries SIGINT in `/proc/<pid>/status`). oslo reset every signal in
/// [`RESET_IN_CHILD`] unconditionally, which wiped the one thing the user asked for.
///
/// A bitmask rather than the trap table because the read happens between `fork` and `execv`, where
/// a `HashMap` behind a lock the parent's other threads may hold is not touchable. An atomic load
/// is; bit `n - 1` is signal `n`, the same shape [`super::super::super::env::builtins::process`]
/// uses for pending signals.
static IGNORED_ON_PURPOSE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Record whether `signum` is ignored because a trap said so. Called by the `trap` builtin.
pub fn note_deliberate_ignore(signum: i32, deliberate: bool) {
    if !(1..=64).contains(&signum) {
        return;
    }
    let bit = 1u64 << (signum - 1);
    if deliberate {
        IGNORED_ON_PURPOSE.fetch_or(bit, Ordering::SeqCst);
    } else {
        IGNORED_ON_PURPOSE.fetch_and(!bit, Ordering::SeqCst);
    }
}

/// Whether a child should keep `sig` ignored rather than get `SIG_DFL` back.
///
/// Two ways that happens. A `trap '' SIG` in this shell is the recorded one. The other is an
/// ignore this shell was *started* with and never touched: **`nohup` works by ignoring SIGHUP and
/// exec'ing**, and an ignore is inherited, so `nohup oslo -c 'sh -c …'` must hand it to the child
/// too — bash and dash both do. Only the signals the shell changes for its own reasons are in
/// [`RESET_IN_CHILD`] unconditionally; the two in [`FATAL_TO_A_SHELL`] are there because
/// [`catch_fatal_for_exit_trap`] may have *handled* them, which is not a reason to overwrite an
/// ignore nobody here installed.
fn ignored_on_purpose(sig: Signal) -> bool {
    let signum = sig as i32;
    if (1..=64).contains(&signum)
        && IGNORED_ON_PURPOSE.load(Ordering::SeqCst) & (1u64 << (signum - 1)) != 0
    {
        return true;
    }
    FATAL_TO_A_SHELL.contains(&sig) && currently_ignored(signum)
}

/// Whether the kernel has `SIG_IGN` for `signum` right now.
///
/// `sigaction` is async-signal-safe, which is what makes this legal between `fork` and `execv`.
fn currently_ignored(signum: i32) -> bool {
    // SAFETY: a zeroed `sigaction` to read into, and a null pointer for the new disposition.
    unsafe {
        let mut current: libc::sigaction = std::mem::zeroed();
        libc::sigaction(signum, std::ptr::null(), &mut current) == 0
            && current.sa_sigaction == libc::SIG_IGN
    }
}

/// Restore the signal state a freshly-started program is entitled to assume.
///
/// Call this in the child between `fork` and `execv`, and in any forked subshell before it starts
/// running commands. It is the counterpart of [`install_shell_signals`]: the REPL ignores
/// SIGTSTP/SIGTTIN/SIGTTOU so that job-control keystrokes and terminal access from a background
/// process cannot stop the shell itself, but a child that inherits those cannot be suspended at
/// all — Ctrl-Z on anything oslo launched did nothing.
///
/// R7.1: it also renounces job control for the child (`control::leave_job_control`). A
/// forked subshell is *not* the session's shell: if it kept the terminal descriptor it would put
/// its own pipeline stages into fresh process groups and hand them the terminal, stealing it from
/// the job its parent is still waiting on.
///
/// Only `sigaction`, `sigprocmask` and `close` are used, all async-signal-safe, so this is legal
/// in the window after `fork` where almost nothing else is.
pub fn reset_signals_for_child() {
    // This process is not the one that cached whether it is init. See `table::is_init`.
    super::reap::forgot_which_process_i_am();
    let dfl = SigAction::new(SigHandler::SigDfl, SaFlags::empty(), SigSet::empty());
    for sig in RESET_IN_CHILD {
        if ignored_on_purpose(sig) {
            continue;
        }
        // Errors are unreportable here (the child has not exec'd yet and stderr may belong to a
        // pipe the parent is about to close); a failure leaves the inherited disposition, which
        // is no worse than not trying.
        unsafe {
            let _ = signal::sigaction(sig, &dfl);
        }
    }

    // A blocked signal also survives exec. Nothing in oslo blocks signals for longer than a
    // `tcsetpgrp` pair, but a mask inherited from whatever started the shell — or caught mid-swap
    // by a fork — would be passed on to every command it runs.
    let _ = signal::sigprocmask(SigmaskHow::SIG_SETMASK, Some(&SigSet::empty()), None);

    super::control::leave_job_control();
}

/// The signals that would kill the shell outright, and so leave an EXIT trap unrun.
///
/// SIGHUP and SIGTERM only. SIGINT and SIGQUIT already reach the shell as something it can act on —
/// SIGINT through [`handle_sigint`], SIGQUIT ignored at a prompt — and SIGKILL cannot be caught by
/// anything.
const FATAL_TO_A_SHELL: [Signal; 2] = [Signal::SIGHUP, Signal::SIGTERM];

/// The fatal signal that has arrived, or 0. Read by [`fatal_signal_pending`].
static FATAL_RECEIVED: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

extern "C" fn handle_fatal(signum: libc::c_int) {
    // One store on a lock-free atomic, which is all a handler may do. The shell notices at its
    // next command boundary or interrupted wait and ends itself there, where running the EXIT
    // trap's shell code is legal.
    FATAL_RECEIVED.store(signum, std::sync::atomic::Ordering::SeqCst);

    // **The second one kills outright.** Deferring the first is what buys the EXIT trap its chance
    // to run, but a shell parked somewhere that never reaches a command boundary — idle at a
    // prompt — would otherwise have absorbed a signal meant to end it. Restoring the default here
    // means `kill` twice always works, without anything having to be reachable to arrange it.
    //
    // `sigaction` is async-signal-safe, which is why this is legal in a handler at all.
    let dfl = libc::sigaction {
        sa_sigaction: libc::SIG_DFL,
        ..unsafe { std::mem::zeroed() }
    };
    unsafe {
        let _ = libc::sigaction(signum, &dfl, std::ptr::null_mut());
    }
}

/// Whether a fatal signal has arrived, and which. Cleared as it is taken.
pub fn fatal_signal_pending() -> Option<i32> {
    match FATAL_RECEIVED.swap(0, std::sync::atomic::Ordering::SeqCst) {
        0 => None,
        signum => Some(signum),
    }
}

/// Whether a fatal signal is waiting, **without** taking it.
///
/// For the foreground wait, which has no channel to report one through: it stops waiting so the
/// shell does not sit out a `sleep 20` it is supposed to be dying during, and leaves the signal for
/// the command boundary immediately after to act on. The same peek-versus-drain split as
/// [`interrupt_waiting`] and for the same reason — a reader that cleared it would leave the
/// boundary with no evidence the signal arrived.
pub fn fatal_signal_waiting() -> Option<i32> {
    match FATAL_RECEIVED.load(Ordering::SeqCst) {
        0 => None,
        signum => Some(signum),
    }
}

/// Catch — or stop catching — the signals that would otherwise kill the shell before its EXIT
/// trap could run.
///
/// **`trap cleanup EXIT` did not survive `kill`.** A shell terminated by SIGTERM or SIGHUP dies in
/// the kernel, so the handler every script writes to remove its temporary directory never ran:
/// `timeout` on a script, a session hanging up, a service manager stopping a job — all of them left
/// the cleanup undone. bash catches these, runs the trap, and still exits `128 + signo`; measured,
/// it also dies *promptly* rather than waiting for the foreground child, which is what makes this
/// safe to do at all.
///
/// **Armed only while an EXIT trap is actually set**, which is the containment that matters: a
/// shell with no EXIT trap keeps the default disposition and dies the instant it is signalled, so
/// nothing about an ordinary session changes. Nothing here can make a shell unkillable — the
/// handler only records, and the two places that read it are the command boundary and an
/// interrupted wait, both of which then exit.
///
/// # Where this is still short of bash
///
/// A shell blocked somewhere that reaches neither of those places absorbs the first signal and
/// needs a second. `read` was the common one and now ends its own wait — see
/// `read_input` — which leaves a redirection whose *open* blocks:
/// `read x < a-fifo-nobody-writes-to` parks in `open(2)`, and `File::open` retries `EINTR` inside
/// the standard library, so the signal is never seen. bash dies on the first there.
///
/// The second always works, because `handle_fatal` puts the default disposition back as it runs.
/// So the residue is a cleanup skipped, never a process that will not stop.
pub fn catch_fatal_for_exit_trap(catching: bool) {
    let action = match catching {
        true => SigAction::new(
            SigHandler::Handler(handle_fatal),
            SaFlags::empty(),
            SigSet::empty(),
        ),
        false => SigAction::new(SigHandler::SigDfl, SaFlags::empty(), SigSet::empty()),
    };
    for sig in FATAL_TO_A_SHELL {
        // SAFETY: an `extern "C"` handler with the signature the kernel calls it with, or the
        // system default.
        unsafe {
            let _ = signal::sigaction(sig, &action);
        }
    }
    if !catching {
        FATAL_RECEIVED.store(0, Ordering::SeqCst);
    }
}

/// Set by the SIGINT handler; drained by [`interrupt_pending`].
///
/// An `AtomicBool` because that is all a handler may safely touch: the evaluator polls it at
/// command boundaries rather than the handler unwinding anything itself.
static SIGINT_RECEIVED: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_sigint(_: libc::c_int) {
    SIGINT_RECEIVED.store(true, Ordering::SeqCst);
    // **The self-pipe**, so a caller can *wait* for an interrupt rather than only ask about one
    // afterwards. See [`interrupt_fd`] for why a flag alone cannot close the race.
    //
    // `write(2)` is async-signal-safe, which almost nothing else is; the byte's value carries
    // nothing and the failure is ignored because a full pipe already says what this is trying to.
    let fd = SIGINT_PIPE.load(Ordering::SeqCst);
    if fd >= 0 {
        let byte = b"\x01";
        unsafe {
            let _ = libc::write(fd, byte.as_ptr().cast(), 1);
        }
    }
}

/// The write end of the self-pipe, or `-1` before one exists.
///
/// An `AtomicI32` because a signal handler may read it and may touch nothing else.
static SIGINT_PIPE: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

/// The read end, for a caller to poll. `None` until [`install_shell_signals`] has run.
///
/// # Why a flag is not enough
///
/// [`interrupt_waiting`] answers "has one arrived", which is all a *polling* caller needs. A caller
/// that has to **block** — `rm`'s prompt waiting for a keystroke — cannot use it: testing the flag
/// and then blocking is a time-of-check race, and a signal landing in between leaves the flag set
/// with nothing pending to interrupt the wait. The read blocks on an answer that is never coming.
///
/// A descriptor has no such window. The handler writes a byte; a caller waits on the keystroke and
/// this together. A signal that arrived *before* the wait began has already left its byte, so the
/// wait returns immediately — which is exactly the case a flag cannot express.
///
/// Both remain: the flag is what a loop polls between entries, and this is what a blocking wait
/// selects on. They report the same event by the two means it can be observed.
pub fn interrupt_fd() -> Option<std::os::fd::BorrowedFd<'static>> {
    let fd = SIGINT_READ.load(Ordering::SeqCst);
    // SAFETY: set once by `install_shell_signals` from a pipe this process owns for its lifetime,
    // and never closed. `BorrowedFd` does not close it.
    (fd >= 0).then(|| unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) })
}

/// The read end of the self-pipe, or `-1`.
static SIGINT_READ: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

/// Take the bytes the handler has written, so one keystroke does not answer twice.
///
/// The flag is left alone: draining it is [`interrupt_pending`]'s, and a builtin that cleared it
/// would leave the evaluator with no evidence the keystroke happened — the same rule
/// [`interrupt_waiting`] states.
pub fn drain_interrupt_fd() {
    let fd = SIGINT_READ.load(Ordering::SeqCst);
    if fd < 0 {
        return;
    }
    let mut scratch = [0u8; 64];
    loop {
        let read = unsafe { libc::read(fd, scratch.as_mut_ptr().cast(), scratch.len()) };
        if read <= 0 {
            return;
        }
    }
}

/// Make the pipe, once. Non-blocking on both ends so neither the handler nor a drain can stall.
fn open_self_pipe() {
    if SIGINT_READ.load(Ordering::SeqCst) >= 0 {
        return;
    }
    let Ok((read, write)) = nix::unistd::pipe2(nix::fcntl::OFlag::O_CLOEXEC) else {
        return;
    };
    // **Non-blocking, and the write end above all.** A handler that blocked on a full pipe would
    // stop the process inside a signal, which is the one place it must never wait.
    use std::os::fd::AsRawFd;
    for fd in [read.as_raw_fd(), write.as_raw_fd()] {
        let flags = nix::fcntl::fcntl(fd, nix::fcntl::F_GETFL).unwrap_or(0);
        let mut flags = nix::fcntl::OFlag::from_bits_truncate(flags);
        flags.insert(nix::fcntl::OFlag::O_NONBLOCK);
        let _ = nix::fcntl::fcntl(fd, nix::fcntl::F_SETFL(flags));
    }
    // Leaked deliberately: these live as long as the process, and a `BorrowedFd` handed to a
    // handler must not be closed under it.
    use std::os::fd::IntoRawFd;
    SIGINT_READ.store(read.into_raw_fd(), Ordering::SeqCst);
    SIGINT_PIPE.store(write.into_raw_fd(), Ordering::SeqCst);
}

/// Install the dispositions an interactive shell needs. Called once, from `JobManager::setup_signals`.
///
/// R7.6: SIGINT is installed **without** `SA_RESTART`. With the flag, a blocking `read`/`write`
/// restarts itself after the handler returns, so Ctrl-C during a long write in the REPL was
/// invisible until the write finished on its own. Without it those calls fail with `EINTR`
/// instead, which is why every wait loop in the evaluator retries on `EINTR` explicitly — see
/// [`crate::exec::pipeline::wait_for_status`].
///
/// SIGTSTP/SIGTTIN/SIGTTOU are ignored so that a job-control keystroke, or a background job
/// touching the terminal, cannot stop the shell that is supposed to arbitrate it. Ignoring
/// SIGTTOU also makes the shell's own `tcsetpgrp` safe; `without_sigttou` blocks it as well,
/// because a `trap` may legitimately replace the disposition later.
/// Put back the disposition an interactive shell installed for itself, if it owns one for `signum`.
///
/// **`trap - INT` means "back to how it was", and how it was is not the system default.**
/// [`install_shell_signals`] runs once at REPL start and is the only thing that installs
/// `handle_sigint` — the flag and self-pipe the whole interrupt path depends on. The `trap`
/// builtin wrote `SIG_DFL` straight into the kernel for `Disposition::Default`, so the universal
/// idiom
///
/// ```sh
/// trap 'cleanup' INT ; … ; trap - INT
/// ```
///
/// left the session with no SIGINT handler at all: the next Ctrl-C killed the shell instead of
/// returning to the prompt, and the terminal went with it.
///
/// Only for an interactive shell. A script's `trap - INT` really does mean the system default, and
/// that is what bash gives it.
///
/// Answers whether it installed anything, so the caller can fall through to `SIG_DFL`.
pub fn restore_shell_signal(signum: i32) -> bool {
    if !crate::exec::pipeline::is_interactive() {
        return false;
    }
    let interruptible = SigAction::new(
        SigHandler::Handler(handle_sigint),
        SaFlags::empty(),
        SigSet::empty(),
    );
    let ignored = SigAction::new(SigHandler::SigIgn, SaFlags::empty(), SigSet::empty());
    // The four the shell claims for itself, and nothing else — see `install_shell_signals`.
    let (signal, action) = match signum {
        libc::SIGINT => (Signal::SIGINT, interruptible),
        libc::SIGQUIT => (Signal::SIGQUIT, ignored),
        libc::SIGTSTP => (Signal::SIGTSTP, ignored),
        libc::SIGTTIN => (Signal::SIGTTIN, ignored),
        libc::SIGTTOU => (Signal::SIGTTOU, ignored),
        _ => return false,
    };
    // SAFETY: the same call `install_shell_signals` makes, with the same handler.
    unsafe { signal::sigaction(signal, &action).is_ok() }
}

pub fn install_shell_signals() {
    // Before the handler is installed, so a signal delivered a moment later already has somewhere
    // to write. See `interrupt_fd`.
    open_self_pipe();
    let interruptible = SigAction::new(
        SigHandler::Handler(handle_sigint),
        SaFlags::empty(),
        SigSet::empty(),
    );
    let ignored = SigAction::new(SigHandler::SigIgn, SaFlags::empty(), SigSet::empty());
    unsafe {
        let _ = signal::sigaction(Signal::SIGINT, &interruptible);
        for sig in IGNORED_AT_A_PROMPT {
            let _ = signal::sigaction(sig, &ignored);
        }
    }
}

/// The signals an interactive shell ignores for itself.
///
/// **SIGQUIT is here because Ctrl-\ killed the session.** POSIX says an interactive shell shall
/// ignore it, and bash and dash both do — read out of `/proc/<pid>/status` for a real pty session,
/// `SigIgn` carries it in both. oslo left it at the system default, so the key that dumps core
/// killed the shell and took the terminal with it, from any prompt.
///
/// Safe to ignore here precisely because [`RESET_IN_CHILD`] already lists it: a program the shell
/// starts still gets `SIG_DFL`, so Ctrl-\ still quits the thing that is running.
const IGNORED_AT_A_PROMPT: [Signal; 4] = [
    Signal::SIGQUIT,
    Signal::SIGTSTP,
    Signal::SIGTTIN,
    Signal::SIGTTOU,
];

thread_local! {
    /// An interrupt raised by *this* thread rather than delivered by the kernel.
    ///
    /// Separate from [`SIGINT_RECEIVED`] so that one evaluation cannot cancel another's: the test
    /// binaries run a script per thread, and a process-wide flag set by one of them would abort
    /// whichever unrelated evaluation polled first. `const`-initialised, so touching it from a
    /// signal handler allocates nothing.
    static LOCAL_INTERRUPT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether a SIGINT arrived since this was last asked, clearing the flag.
///
/// R7.2: polled by [`crate::exec::pipeline::eval_command_list`] at every command boundary, which
/// is what makes a shell-level `while true; do :; done` interruptible — nothing in that loop ever
/// enters the kernel, so the handler is the only evidence the keystroke happened.
pub fn interrupt_pending() -> bool {
    // Both are drained, not short-circuited: leaving one set would make the *next* command
    // boundary report an interrupt that has already been acted on.
    let local = LOCAL_INTERRUPT.with(|flag| flag.replace(false));
    let delivered = SIGINT_RECEIVED.swap(false, Ordering::SeqCst);
    local || delivered
}

/// Whether a SIGINT is waiting, **without** taking it.
///
/// For the builtins that run long enough to need interrupting from the inside — `rm -r` over a
/// large tree is the one this was written for. A builtin runs in the shell process, so the
/// keystroke sets the flag and then nothing looks at it until the command is already over; a walk
/// that polls this can stop between entries instead.
///
/// **Peeking rather than draining** is the whole point. [`interrupt_pending`] is the command
/// boundary's, and a builtin that cleared the flag would leave the evaluator with no evidence the
/// keystroke happened — the `rm` would stop, and the `&&` after it would run anyway.
pub fn interrupt_waiting() -> bool {
    LOCAL_INTERRUPT.with(std::cell::Cell::get) || SIGINT_RECEIVED.load(Ordering::SeqCst)
}

/// Drop an interrupt that arrived before the shell asked for the next command.
///
/// A SIGINT is only ever *cleared* by [`interrupt_pending`], and the only caller of that is the
/// command boundary in [`crate::exec::pipeline::eval_command_list`]. So a signal delivered while
/// the previous command was finishing, or while the prompt was being drawn, stayed set until the
/// *next* command reached its first boundary — which then reported it as interrupted and returned
/// 130 without running anything, leaving the flag clear so the retry worked. A keystroke from
/// before the command was typed cannot sensibly cancel it, so the REPL forgets it at the prompt.
pub fn forget_interrupt() {
    let _ = interrupt_pending();
}

/// Ask the evaluator running on *this* thread to unwind at its next command boundary.
///
/// Exists so the interrupt path can be tested without signalling a multi-threaded test binary,
/// and so a `trap`-driven caller can interrupt the evaluation it is part of.
pub fn note_interrupt() {
    LOCAL_INTERRUPT.with(|flag| flag.set(true));
}

/// Run `f` with SIGTTOU blocked, restoring the previous mask afterwards.
///
/// R7.1: `tcsetpgrp` from a process that is *not* in the terminal's foreground group raises
/// SIGTTOU at the caller, whose default action is to stop it. Handing the terminal to a job and
/// taking it back are both such calls, so a shell that did them unguarded would suspend itself
/// the moment it started a foreground pipeline.
pub(crate) fn without_sigttou<R>(f: impl FnOnce() -> R) -> R {
    let mut blocked = SigSet::empty();
    blocked.add(Signal::SIGTTOU);
    let mut previous = SigSet::empty();
    // `pthread_sigmask`, not `sigprocmask`: the latter is unspecified in a process with more than
    // one thread, and the test binaries evaluate scripts on several at once.
    let masked =
        signal::pthread_sigmask(SigmaskHow::SIG_BLOCK, Some(&blocked), Some(&mut previous));

    let result = f();

    // Only restore what was actually saved: on the error path `previous` is the empty set we
    // initialised it to, and installing that would unblock signals the caller had blocked.
    if masked.is_ok() {
        let _ = signal::pthread_sigmask(SigmaskHow::SIG_SETMASK, Some(&previous), None);
    }
    result
}

#[cfg(test)]
#[path = "signals/tests.rs"]
mod tests;
