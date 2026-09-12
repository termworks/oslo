//! Process groups and who owns the terminal (R7.1).
//!
//! Job control is a property of the *session*, not of a command, so the two things it needs — the
//! shell's own process group and a descriptor for the controlling terminal — are process-global
//! and set up once by [`init_job_control`]. Everything else in this file is a small, failure-
//! tolerant wrapper around `setpgid`/`tcsetpgrp`: when job control is off (a script, a `-c`
//! command, a pipe on stdin) every one of them is a no-op, so the non-interactive execution path
//! is byte-for-byte what it was before.
//!
//! The invariant the rest of the shell relies on: **only the session's interactive shell process
//! ever calls the functions here**. Every forked child renounces job control through
//! [`leave_job_control`], reached from [`super::reset_signals_for_child`].

use nix::fcntl::{FcntlArg, fcntl};
use nix::sys::termios::{SetArg, Termios, tcgetattr, tcsetattr};
use nix::unistd::{Pid, getpgrp, getpid, isatty, setpgid, tcsetpgrp};
use std::os::fd::{BorrowedFd, RawFd};
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, Ordering};

/// A dup of the controlling terminal, or `-1` when this process is not doing job control.
///
/// A *private* descriptor rather than fd 0/1/2: a builtin's redirection can replace any of the
/// three in the shell itself (`jobs > file`), and `exec 2>log` replaces one permanently. The
/// terminal has to stay reachable after that, or the shell could never take its terminal back.
static TERMINAL_FD: AtomicI32 = AtomicI32::new(NO_JOB_CONTROL);

const NO_JOB_CONTROL: RawFd = -1;

/// The shell's own process group — the one the terminal must be handed back to.
static SHELL_PGID: AtomicI32 = AtomicI32::new(0);

/// Claim the terminal and the shell's own process group, if there is a terminal to claim.
///
/// Called from `JobManager::setup_signals`, which only the REPL reaches. Any failure leaves job
/// control off rather than half-configured: a shell that had moved itself into a new process
/// group but could not get the terminal would be unable to read from it at all.
///
/// The shell puts *itself* in a group it leads first. Without that step `tcsetpgrp` would hand
/// the terminal to whatever group the shell was started in — typically its parent shell's — and
/// every child of it would then look like a foreground process to the tty driver.
pub fn init_job_control() {
    if !crate::exec::pipeline::is_interactive() {
        return;
    }
    enable_job_control();
}

/// Turn job control on without asking whether the shell is interactive: `set -m`.
///
/// POSIX makes `-m` an option a *script* can set, and bash honours it — `set -m` in a script gives
/// each job its own process group and makes `fg`, `bg` and `%1` work. oslo enabled job control
/// only from the REPL, so a script that said `set -m` got half of it: jobs did land in their own
/// process groups, but nothing owned the terminal, so `bg` answered `no job control` and left the
/// job stopped forever. Found by running the job-control suite in the Alpine VM, which is the only
/// place a shell that is not a REPL still has a controlling terminal.
///
/// Returns whether job control is on afterwards. It stays off when there is no terminal to claim,
/// which is not an error: `set -m` in a pipeline is legal and simply cannot do anything.
pub fn enable_job_control() -> bool {
    if job_control_active() {
        return true;
    }
    let Some(fd) = duplicate_terminal() else {
        return false;
    };

    let shell = getpid();
    // EPERM here means the shell is already a session leader, which is exactly the state the call
    // is trying to reach; anything else and job control stays off.
    if getpgrp() != shell && setpgid(shell, shell).is_err() {
        close_raw(fd);
        return false;
    }
    if super::signals::without_sigttou(|| set_foreground(fd, shell)).is_err() {
        close_raw(fd);
        return false;
    }

    SHELL_PGID.store(shell.as_raw(), Ordering::SeqCst);
    TERMINAL_FD.store(fd, Ordering::SeqCst);
    true
}

/// Whether this process arbitrates the terminal — the one question every caller here asks first.
pub fn job_control_active() -> bool {
    TERMINAL_FD.load(Ordering::SeqCst) != NO_JOB_CONTROL
}

/// The process group the terminal belongs to when no job is running.
pub fn shell_pgid() -> Pid {
    Pid::from_raw(SHELL_PGID.load(Ordering::SeqCst))
}

/// Renounce job control in a forked child.
///
/// A subshell, a pipeline stage or a background job is not the session's shell. If it kept the
/// terminal descriptor it would go on to put *its* children into fresh process groups and call
/// `tcsetpgrp`, taking the terminal away from the job its parent is still waiting for — the
/// classic way a shell wedges itself when `( a | b )` runs in the foreground.
///
/// Reached from [`super::reset_signals_for_child`], so every fork site in the shell gets it
/// without having to remember.
pub fn leave_job_control() {
    let fd = TERMINAL_FD.swap(NO_JOB_CONTROL, Ordering::SeqCst);
    if fd != NO_JOB_CONTROL {
        close_raw(fd);
    }
}

/// Put a freshly forked child into `pgid`, or into a group of its own when `pgid` is `None`.
///
/// Called in the *child*, before it execs or starts evaluating. The parent makes the same call
/// ([`place_child`]) because either process may be scheduled first: the child must be in its group
/// before it can be signalled, and the parent must not hand the terminal to a group that does not
/// exist yet. `setpgid` is idempotent, so doing it twice is the documented way to close that race.
pub(crate) fn join_group_in_child(pgid: Option<Pid>) {
    let target = pgid.unwrap_or_else(|| Pid::from_raw(0));
    // Unreportable and not worth failing over: the worst case is that the job shares the shell's
    // group, which is where every job lived before R7.1.
    let _ = setpgid(Pid::from_raw(0), target);
}

/// The parent half of [`join_group_in_child`]; returns the group the child ended up in.
///
/// `None` means "make this child a group leader", which is what the first process of a pipeline
/// is: its pid becomes the pgid every later stage joins.
pub(crate) fn place_child(child: Pid, pgid: Option<Pid>) -> Pid {
    let target = pgid.unwrap_or(child);
    // ESRCH/EACCES here mean the child has already exec'd or exited; it is in the right group or
    // beyond caring either way.
    let _ = setpgid(child, target);
    target
}

/// [`join_group_in_child`] for a *foreground* job, which only moves when job control is on.
///
/// A foreground command must stay in the shell's process group when there is no job control,
/// because that group is what the tty driver signals: a `oslo -c 'sleep 100'` whose `sleep` had
/// been moved out of it would go on sleeping through Ctrl-C. bash makes the same distinction, and
/// it is why a script's execution path is unchanged by R7.1.
pub(crate) fn join_foreground_group(pgid: Option<Pid>) {
    if !job_control_active() {
        return;
    }
    join_group_in_child(pgid);
}

/// The parent half of [`join_foreground_group`]; `None` means "left in the shell's group".
pub(crate) fn place_foreground_child(child: Pid, pgid: Option<Pid>) -> Option<Pid> {
    if !job_control_active() {
        return None;
    }
    Some(place_child(child, pgid))
}

/// Hand the terminal to `pgid` so the job can read from it and receive Ctrl-C.
///
/// A no-op without job control, which is what keeps scripts and `-c` commands on exactly the code
/// path they had before.
pub(crate) fn give_terminal_to(pgid: Pid) {
    let fd = TERMINAL_FD.load(Ordering::SeqCst);
    if fd == NO_JOB_CONTROL {
        return;
    }
    // Remember how the terminal is set up before letting go of it. Handing it over is the only
    // moment the shell can be sure the modes are still its own — see `reclaim_terminal`.
    if pgid != shell_pgid() {
        remember_terminal_modes(fd);
    }
    super::signals::without_sigttou(|| {
        let _ = set_foreground(fd, pgid);
    });
}

/// The terminal's settings as they were before the last foreground job got them.
static SAVED_MODES: Mutex<Option<Termios>> = Mutex::new(None);

fn remember_terminal_modes(fd: RawFd) {
    let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
    if let Ok(modes) = tcgetattr(borrowed)
        && let Ok(mut slot) = SAVED_MODES.lock()
    {
        *slot = Some(modes);
    }
}

/// Put the terminal back the way the shell had it.
///
/// **A program that dies badly does not tidy up.** A killed TUI, a crashed editor, a visualiser
/// stopped with Ctrl-C — any of them can leave the terminal with echo off, in raw mode, or on the
/// alternate screen. The shell then reads the *broken* state as its starting point and restores
/// that after every line, so the damage outlives the program that caused it and "my terminal is
/// broken until I run `reset`" becomes the shell's fault.
///
/// Only the modes, not the screen: a program that switched to the alternate screen and did not
/// switch back has lost the user's scrollback either way, and issuing the escape here would erase
/// output the shell did not write.
fn restore_terminal_modes(fd: RawFd) {
    let Ok(slot) = SAVED_MODES.lock() else {
        return;
    };
    let Some(modes) = slot.as_ref() else {
        return;
    };
    let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
    // `TCSADRAIN`, not `TCSANOW`: whatever the job last wrote should reach the screen before the
    // settings change under it.
    let _ = tcsetattr(borrowed, SetArg::TCSADRAIN, modes);
}

/// Take the terminal back once a foreground job has stopped or finished.
///
/// SIGTTOU is blocked for the call by [`give_terminal_to`]: at this moment the shell is *not* the
/// foreground group, so an unguarded `tcsetpgrp` is precisely the call that would stop it.
///
/// **`tidy` says whether the job left the terminal the way it meant to.** A job that exited on its
/// own did: `stty -echo` is a program whose whole purpose is to change these settings, and putting
/// the snapshot back one instant later made it — and `stty sane`, and `reset` — a no-op at an oslo
/// prompt. Measured against dash, which keeps the change; oslo undid it.
///
/// A job that was killed or stopped did not, and that is the case [`restore_terminal_modes`] exists
/// for: a TUI cut down by a signal has no chance to tidy, and the damage would otherwise outlive it.
pub(crate) fn reclaim_terminal(tidy: bool) {
    if !job_control_active() {
        return;
    }
    give_terminal_to(shell_pgid());
    let fd = TERMINAL_FD.load(Ordering::SeqCst);
    if !tidy && fd != NO_JOB_CONTROL {
        restore_terminal_modes(fd);
    }
}

/// `tcsetpgrp` on a borrowed descriptor.
///
/// # Safety of the borrow
///
/// `fd` is the descriptor stored in [`TERMINAL_FD`], which is only ever closed by
/// [`leave_job_control`] — and that clears the slot in the same operation, so no caller can reach
/// this with a closed descriptor.
fn set_foreground(fd: RawFd, pgid: Pid) -> nix::Result<()> {
    let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
    tcsetpgrp(borrowed, pgid)
}

/// A private, close-on-exec dup of whichever standard descriptor is still a terminal.
///
/// Starting the search at fd 10 keeps it clear of the low numbers a script redirects by hand, and
/// `F_DUPFD_CLOEXEC` keeps it out of every program the shell execs — a command that inherited the
/// shell's terminal descriptor could hand the terminal to a group of its own choosing.
fn duplicate_terminal() -> Option<RawFd> {
    let tty = [0, 1, 2]
        .into_iter()
        .find(|fd| isatty(*fd).unwrap_or(false))?;
    fcntl(tty, FcntlArg::F_DUPFD_CLOEXEC(10)).ok()
}

fn close_raw(fd: RawFd) {
    // Safety: `fd` came from `fcntl(F_DUPFD_CLOEXEC)` in this module and is not owned elsewhere.
    unsafe {
        let _ = nix::libc::close(fd);
    }
}

#[cfg(test)]
mod tests {
    use super::{job_control_active, leave_job_control, place_child, shell_pgid};
    use nix::unistd::Pid;

    /// The default for every non-interactive oslo — a script, `-c`, and the test binaries — is
    /// that nothing in this module touches the terminal.
    #[test]
    fn job_control_is_off_until_a_repl_asks_for_it() {
        assert!(!job_control_active());
        assert_eq!(shell_pgid(), Pid::from_raw(0));
    }

    /// Renouncing job control twice must not close a descriptor twice; the second call has
    /// nothing to close.
    #[test]
    fn leaving_job_control_is_idempotent() {
        leave_job_control();
        leave_job_control();
        assert!(!job_control_active());
    }

    /// The parent's placement decides the pipeline's group: the first stage leads, every later
    /// stage joins the leader.
    #[test]
    fn the_first_stage_becomes_the_group_leader() {
        // A pid that cannot exist, so `setpgid` fails and only the bookkeeping is under test.
        let first = Pid::from_raw(i32::MAX);
        assert_eq!(place_child(first, None), first);
        assert_eq!(place_child(Pid::from_raw(i32::MAX - 1), Some(first)), first);
    }
}

/// Whether a job that ended with `status` left the terminal the way it meant to.
///
/// A shell reports a signal death as `128 + signo`, so anything below that is a program that
/// returned on its own — and a program that returned chose whatever it did to the terminal.
/// `stty -echo` is exactly such a program, and restoring over it made `stty`, `stty sane` and
/// `reset` no-ops at an oslo prompt.
///
/// At or above 128 the job was killed or stopped and had no chance to tidy, which is the case
/// [`restore_terminal_modes`] exists for.
pub(crate) fn left_it_deliberately(status: i32) -> bool {
    status < 128
}

#[cfg(test)]
mod deliberate_tests {
    /// **`stty` is a program whose whole purpose is to change the terminal**, and it exits 0.
    /// Restoring the snapshot taken a moment earlier made it — and `stty sane`, and `reset` — a
    /// no-op at an oslo prompt. Measured against dash, which keeps the change.
    #[test]
    fn a_program_that_returned_chose_what_it_left() {
        for status in [0, 1, 2, 127] {
            assert!(
                super::left_it_deliberately(status),
                "status {status} is a program that returned on its own"
            );
        }
    }

    /// A shell reports a signal death as `128 + signo`. Those had no chance to tidy, and repairing
    /// after them is what the snapshot is for: a TUI cut down by SIGKILL leaves echo off.
    #[test]
    fn a_job_that_was_killed_did_not() {
        for signo in [1, 2, 9, 15, 19] {
            assert!(
                !super::left_it_deliberately(128 + signo),
                "128+{signo} was killed or stopped"
            );
        }
    }
}
