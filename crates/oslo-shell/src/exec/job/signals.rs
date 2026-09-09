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
const RESET_IN_CHILD: [Signal; 6] = [
    Signal::SIGPIPE,
    Signal::SIGINT,
    Signal::SIGQUIT,
    Signal::SIGTSTP,
    Signal::SIGTTIN,
    Signal::SIGTTOU,
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
fn ignored_on_purpose(sig: Signal) -> bool {
    let signum = sig as i32;
    (1..=64).contains(&signum)
        && IGNORED_ON_PURPOSE.load(Ordering::SeqCst) & (1u64 << (signum - 1)) != 0
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
/// [`handle_sigint`] — the flag and self-pipe the whole interrupt path depends on. The `trap`
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
mod tests {
    use super::{
        RESET_IN_CHILD, interrupt_pending, note_interrupt, reset_signals_for_child, without_sigttou,
    };
    use nix::libc;
    use nix::sys::signal::{SigSet, Signal};
    use nix::sys::wait::{WaitStatus, waitpid};
    use nix::unistd::{ForkResult, fork};

    /// Exercised in a forked child, not in the test process.
    ///
    /// Setting SIGPIPE back to `SIG_DFL` here would arm the whole test binary to be killed by any
    /// write to a closed pipe, and libtest runs these on shared threads. The child never
    /// allocates — only `sigaction`, `sigprocmask` and `_exit`, all async-signal-safe — so it is
    /// safe in the post-fork window even though the parent is multi-threaded.
    /// Both tests below drive the same process-wide mask, so they may not overlap.
    static ALONE: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn every_ignored_signal_comes_back_as_sig_dfl() {
        let _alone = ALONE.lock().unwrap_or_else(|held| held.into_inner());
        let child = unsafe { fork() }.expect("fork");
        match child {
            ForkResult::Child => {
                let status = unsafe { child_checks_dispositions() };
                unsafe { libc::_exit(status) };
            }
            ForkResult::Parent { child } => {
                assert_eq!(
                    waitpid(child, None).expect("waitpid"),
                    WaitStatus::Exited(child, 0),
                    "a signal was left ignored, or the mask was left blocked"
                );
            }
        }
    }

    /// Ignore and block everything the helper is supposed to undo, undo it, then read the state
    /// back out of the kernel. Exit 0 means the helper did its job.
    ///
    /// # Safety
    ///
    /// Only for use in a forked child: it changes process-wide signal state.
    unsafe fn child_checks_dispositions() -> i32 {
        unsafe {
            let mut ign: libc::sigaction = std::mem::zeroed();
            ign.sa_sigaction = libc::SIG_IGN;
            let mut full: libc::sigset_t = std::mem::zeroed();
            libc::sigfillset(&mut full);

            for sig in RESET_IN_CHILD {
                if libc::sigaction(sig as i32, &ign, std::ptr::null_mut()) != 0 {
                    return 10;
                }
            }
            if libc::sigprocmask(libc::SIG_SETMASK, &full, std::ptr::null_mut()) != 0 {
                return 11;
            }

            reset_signals_for_child();

            for sig in RESET_IN_CHILD {
                let mut cur: libc::sigaction = std::mem::zeroed();
                if libc::sigaction(sig as i32, std::ptr::null(), &mut cur) != 0 {
                    return 12;
                }
                if cur.sa_sigaction != libc::SIG_DFL {
                    return 13;
                }
            }

            let mut mask: libc::sigset_t = std::mem::zeroed();
            if libc::sigprocmask(libc::SIG_SETMASK, std::ptr::null(), &mut mask) != 0 {
                return 14;
            }
            for sig in RESET_IN_CHILD {
                if libc::sigismember(&mask, sig as i32) != 0 {
                    return 15;
                }
            }
            0
        }
    }

    /// **`trap '' INT` has to survive into the child**, or it protects nothing.
    ///
    /// POSIX: a child inherits its parent's dispositions, save that *caught* signals go back to the
    /// default. bash and dash both hand SIGINT on as ignored — `SigIgn: …2` in the child's
    /// `/proc/<pid>/status`. oslo reset it, so `trap '' INT; ./long-job` left the job killable by
    /// the very keystroke the trap was written to survive.
    #[test]
    fn a_deliberate_ignore_reaches_the_child() {
        let _alone = ALONE.lock().unwrap_or_else(|held| held.into_inner());
        super::note_deliberate_ignore(libc::SIGINT, true);
        let child = unsafe { fork() }.expect("fork");
        match child {
            ForkResult::Child => {
                let status = unsafe { child_checks_the_kept_ignore() };
                unsafe { libc::_exit(status) };
            }
            ForkResult::Parent { child } => {
                let seen = waitpid(child, None).expect("waitpid");
                super::note_deliberate_ignore(libc::SIGINT, false);
                assert_eq!(
                    seen,
                    WaitStatus::Exited(child, 0),
                    "the trap's ignore was undone, or another signal kept one it was not given"
                );
            }
        }
    }

    /// Ignore everything, reset, and read back: SIGINT stays ignored, the rest do not.
    ///
    /// # Safety
    ///
    /// Only for use in a forked child: it changes process-wide signal state.
    unsafe fn child_checks_the_kept_ignore() -> i32 {
        unsafe {
            let mut ign: libc::sigaction = std::mem::zeroed();
            ign.sa_sigaction = libc::SIG_IGN;
            for sig in RESET_IN_CHILD {
                if libc::sigaction(sig as i32, &ign, std::ptr::null_mut()) != 0 {
                    return 10;
                }
            }

            reset_signals_for_child();

            for sig in RESET_IN_CHILD {
                let mut cur: libc::sigaction = std::mem::zeroed();
                if libc::sigaction(sig as i32, std::ptr::null(), &mut cur) != 0 {
                    return 11;
                }
                let want = if sig == Signal::SIGINT {
                    libc::SIG_IGN
                } else {
                    libc::SIG_DFL
                };
                if cur.sa_sigaction != want {
                    return 12;
                }
            }
            0
        }
    }

    /// The flag is edge-triggered: one keystroke aborts one evaluation, not every later one.
    #[test]
    fn an_interrupt_is_reported_once() {
        // Drain anything an earlier test left, so this reads its own signal and not a stale one.
        let _ = interrupt_pending();
        assert!(!interrupt_pending());
        note_interrupt();
        assert!(interrupt_pending());
        assert!(!interrupt_pending());
    }

    /// SIGTTOU has to be blocked *and* restored — a shell that leaked the block would hand it to
    /// every command it forked, and `reset_signals_for_child` is the only other thing that clears
    /// it.
    #[test]
    fn sigttou_is_blocked_only_for_the_call() {
        let before = SigSet::thread_get_mask().expect("mask");
        let seen = without_sigttou(|| {
            SigSet::thread_get_mask()
                .expect("mask")
                .contains(Signal::SIGTTOU)
        });
        assert!(seen, "SIGTTOU was not blocked inside the guard");
        let after = SigSet::thread_get_mask().expect("mask");
        assert_eq!(
            before.contains(Signal::SIGTTOU),
            after.contains(Signal::SIGTTOU),
            "the mask was not restored"
        );
    }

    /// **A non-interactive shell's `trap - INT` really does mean the system default**, which is
    /// what bash gives a script — so the restore must decline there and let `SIG_DFL` through.
    #[test]
    fn a_script_gets_the_system_default_back() {
        assert!(
            !crate::exec::pipeline::is_interactive(),
            "a test binary is not a REPL"
        );
        assert!(
            !super::restore_shell_signal(libc::SIGINT),
            "nothing is restored where the shell installed nothing"
        );
    }

    /// And only the four the shell claims for itself — everything else is the trap's to set.
    #[test]
    fn only_the_shells_own_signals_are_restorable() {
        for other in [libc::SIGTERM, libc::SIGHUP, libc::SIGUSR1, libc::SIGQUIT] {
            assert!(
                !super::restore_shell_signal(other),
                "signal {other} is not one the shell installs"
            );
        }
    }

    /// **Ctrl-\ must not kill the session.** POSIX says an interactive shell ignores SIGQUIT, and
    /// both bash and dash do — `SigIgn` in `/proc/<pid>/status` carries it for a real pty session.
    /// oslo left it at the system default, so the key that dumps core killed the shell and took the
    /// terminal with it.
    #[test]
    fn the_shell_ignores_quit_and_hands_the_default_to_children() {
        assert!(
            super::IGNORED_AT_A_PROMPT.contains(&Signal::SIGQUIT),
            "the shell ignores it for itself"
        );
        assert!(
            super::RESET_IN_CHILD.contains(&Signal::SIGQUIT),
            "and every child gets SIG_DFL back, or Ctrl-\\ would stop quitting the job"
        );
        // Everything the prompt ignores has to be undone in a child, or a program oslo starts
        // inherits a disposition it never asked for.
        for ignored in super::IGNORED_AT_A_PROMPT {
            assert!(
                super::RESET_IN_CHILD.contains(&ignored),
                "{ignored:?} is ignored at the prompt and never reset in a child"
            );
        }
    }
}
