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

/// The signals the shell changes for its *own* benefit, which are the ones a child gets back as
/// `SIG_DFL` no matter what they were.
///
/// [`RESET_IN_CHILD`] minus [`super::FATAL_TO_A_SHELL`]. The two fatal ones are in that list only
/// because [`super::catch_fatal_for_exit_trap`] may have handled them; an ignore this shell was
/// *started* with is not the shell's to overwrite, which is what `nohup` depends on — see
/// [`an_inherited_ignore_is_not_the_shells_to_overwrite`].
const THE_SHELLS_OWN: [Signal; 6] = [
    Signal::SIGPIPE,
    Signal::SIGINT,
    Signal::SIGQUIT,
    Signal::SIGTSTP,
    Signal::SIGTTIN,
    Signal::SIGTTOU,
];

/// **`nohup` works by ignoring SIGHUP and exec'ing**, and an ignore is inherited — so a child of
/// `nohup oslo -c …` has to get it too. bash and dash both pass it on; adding SIGHUP and SIGTERM
/// to [`RESET_IN_CHILD`] for the EXIT-trap handler zeroed it, which is the one thing `nohup` is
/// for. Only a disposition *this* shell installed may be reset.
#[test]
fn an_inherited_ignore_is_not_the_shells_to_overwrite() {
    let _alone = ALONE.lock().unwrap_or_else(|held| held.into_inner());
    let child = unsafe { fork() }.expect("fork");
    match child {
        ForkResult::Child => {
            let status = unsafe { child_keeps_an_inherited_ignore() };
            unsafe { libc::_exit(status) };
        }
        ForkResult::Parent { child } => {
            assert_eq!(
                waitpid(child, None).expect("waitpid"),
                WaitStatus::Exited(child, 0),
                "an ignore the shell never installed was reset in the child"
            );
        }
    }
}

/// Ignore the two fatal signals the way a parent would have, reset, and require them still
/// ignored — while the shell's own still come back as `SIG_DFL`.
///
/// # Safety
///
/// Only for use in a forked child: it changes process-wide signal state.
unsafe fn child_keeps_an_inherited_ignore() -> i32 {
    unsafe {
        let mut ign: libc::sigaction = std::mem::zeroed();
        ign.sa_sigaction = libc::SIG_IGN;
        for sig in super::FATAL_TO_A_SHELL {
            if libc::sigaction(sig as i32, &ign, std::ptr::null_mut()) != 0 {
                return 10;
            }
        }
        // And one of the shell's own, to prove the exception is narrow rather than a blanket
        // "leave every ignore alone".
        if libc::sigaction(libc::SIGINT, &ign, std::ptr::null_mut()) != 0 {
            return 11;
        }

        reset_signals_for_child();

        for sig in super::FATAL_TO_A_SHELL {
            let mut cur: libc::sigaction = std::mem::zeroed();
            if libc::sigaction(sig as i32, std::ptr::null(), &mut cur) != 0 {
                return 12;
            }
            if cur.sa_sigaction != libc::SIG_IGN {
                return 13;
            }
        }
        let mut cur: libc::sigaction = std::mem::zeroed();
        if libc::sigaction(libc::SIGINT, std::ptr::null(), &mut cur) != 0 {
            return 14;
        }
        if cur.sa_sigaction != libc::SIG_DFL {
            return 15;
        }
        0
    }
}

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

        for sig in THE_SHELLS_OWN {
            if libc::sigaction(sig as i32, &ign, std::ptr::null_mut()) != 0 {
                return 10;
            }
        }
        if libc::sigprocmask(libc::SIG_SETMASK, &full, std::ptr::null_mut()) != 0 {
            return 11;
        }

        reset_signals_for_child();

        for sig in THE_SHELLS_OWN {
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
        for sig in THE_SHELLS_OWN {
            if libc::sigaction(sig as i32, &ign, std::ptr::null_mut()) != 0 {
                return 10;
            }
        }

        reset_signals_for_child();

        for sig in THE_SHELLS_OWN {
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

/// **A shell with no EXIT trap must die of SIGTERM the way it always did.** That is what makes
/// catching it safe at all, so the arming is asserted in both directions rather than only on.
#[test]
fn the_fatal_signals_are_caught_only_while_an_exit_trap_is_set() {
    let _alone = ALONE.lock().unwrap_or_else(|held| held.into_inner());
    // From a known state rather than an assumed one: the `trap` builtin's own tests set an EXIT
    // trap, which arms these process-wide and leaves them armed for whatever runs next.
    super::catch_fatal_for_exit_trap(false);
    for sig in super::FATAL_TO_A_SHELL {
        assert_eq!(
            current_handler(sig),
            libc::SIG_DFL,
            "{sig:?} was not put back by disarming"
        );
    }

    super::catch_fatal_for_exit_trap(true);
    for sig in super::FATAL_TO_A_SHELL {
        assert_ne!(current_handler(sig), libc::SIG_DFL, "{sig:?} was not armed");
    }

    super::catch_fatal_for_exit_trap(false);
    for sig in super::FATAL_TO_A_SHELL {
        assert_eq!(
            current_handler(sig),
            libc::SIG_DFL,
            "{sig:?} stayed caught after `trap - EXIT`"
        );
    }
}

/// What the kernel currently has for `sig`.
fn current_handler(sig: Signal) -> libc::sighandler_t {
    // SAFETY: a zeroed `sigaction` to read into and a null pointer for the (unwanted) new one.
    unsafe {
        let mut cur: libc::sigaction = std::mem::zeroed();
        assert_eq!(
            libc::sigaction(sig as i32, std::ptr::null(), &mut cur),
            0,
            "sigaction"
        );
        cur.sa_sigaction
    }
}

/// Peek and take, the same split [`interrupt_waiting`] and [`interrupt_pending`] have: the
/// foreground wait reads it to stop waiting and must *leave* it for the command boundary that
/// then ends the shell. A reader that cleared it would lose the exit status and the trap.
#[test]
fn a_fatal_signal_is_peeked_by_one_reader_and_taken_by_the_other() {
    let _alone = ALONE.lock().unwrap_or_else(|held| held.into_inner());
    // Whatever an earlier test left, so this reads its own.
    let _ = super::fatal_signal_pending();
    assert_eq!(super::fatal_signal_waiting(), None);

    super::FATAL_RECEIVED.store(libc::SIGTERM, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(super::fatal_signal_waiting(), Some(libc::SIGTERM));
    assert_eq!(
        super::fatal_signal_waiting(),
        Some(libc::SIGTERM),
        "the peek consumed it"
    );
    assert_eq!(super::fatal_signal_pending(), Some(libc::SIGTERM));
    assert_eq!(super::fatal_signal_pending(), None, "it was not taken");
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
