//! Delivering a trap: the signal handlers, the pending set, and the EXIT trap.
//!
//! A shell cannot run a trap handler *in* a signal handler. The handler body is arbitrary shell
//! code — it allocates, it forks, it takes the same locks the interrupted code was holding — and
//! none of that is async-signal-safe. Every shell therefore does the same two-step: the real
//! signal handler records that the signal arrived and returns, and the evaluator runs the body
//! at the next command boundary, where it is just ordinary shell code again.
//!
//! So this module has an async-signal-safe half and an ordinary half:
//!
//! * [`handle_trapped_signal`] sets one bit in [`PENDING`], an atomic. That is all it does, and
//!   all it is allowed to do.
//! * [`run_pending_traps`] drains that set from the evaluator, with an [`Environment`] in hand.
//!
//! The handler is installed **without** `SA_RESTART`, deliberately. With it, a shell parked in
//! `waitpid` or `read` never notices the signal until whatever it was waiting for finishes on its
//! own — so `trap 'echo caught' INT` on a shell waiting for a long-running child prints nothing
//! at the moment the user pressed Ctrl-C, which is the only moment it is worth anything. The
//! cost is `EINTR` on blocking calls, which the wait path already retries.
//!
//! # What is stored where
//!
//! The trap table itself is [`Environment`]'s, because POSIX resets traps in a subshell and that
//! is where the subshell reset lives. The value stored per condition is bash's own notation:
//! `-` for the default disposition, the empty string for "ignore", and anything else is the
//! handler text. `trap - EXIT` therefore stores `-`, which is what `trap -p` would print — and
//! not, as it used to, a handler literally named `-`.

use super::signals;
use crate::env::Environment;
use crate::env::origin_now;
use nix::libc;
use oslo_base::error::{Result, ShellError};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// The value the trap table stores for a condition left at its default disposition.
///
/// A real handler can never collide with it: POSIX gives `-` as the first operand of `trap` the
/// fixed meaning "reset", so there is no way to *write* a handler whose text is `-`.
pub const DEFAULT_ACTION: &str = "-";

/// What is to happen when a condition fires.
#[derive(Debug, PartialEq, Eq)]
pub enum Disposition<'a> {
    /// Whatever the system would do on its own — terminate, stop, nothing.
    Default,
    /// `trap '' SIG`: the signal is discarded, and stays discarded across `exec`.
    Ignore,
    /// `trap 'cmd' SIG`: run this text as a shell command.
    Run(&'a str),
}

/// Which signals have arrived and not yet been handled, one bit per signal number.
///
/// Bit `n - 1` is signal `n`, so signals 1..=64 fit — realtime signals included, which is why
/// this is a `u64` and not a bitset keyed by nix's `Signal` enum (that enum has no realtime
/// variants at all).
static PENDING: AtomicU64 = AtomicU64::new(0);

/// Whether a trap body is currently running, so the drain does not re-enter itself.
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Whether the EXIT trap has already run. It fires once per shell, however the shell ends.
static EXIT_TRAP_DONE: AtomicBool = AtomicBool::new(false);

/// The one thing a signal handler in a shell may do.
///
/// # Safety of what it touches
///
/// A single `fetch_or` on a lock-free atomic: no allocation, no locks, no reentrancy hazard.
extern "C" fn handle_trapped_signal(signum: libc::c_int) {
    if (1..=64).contains(&signum) {
        PENDING.fetch_or(1u64 << (signum - 1), Ordering::SeqCst);
    }
}

/// The disposition currently recorded for `condition` (a canonical name such as `INT` or `EXIT`).
pub fn disposition<'a>(env: &'a Environment, condition: &str) -> Disposition<'a> {
    match env.get_trap(condition) {
        None => Disposition::Default,
        Some(DEFAULT_ACTION) => Disposition::Default,
        Some("") => Disposition::Ignore,
        Some(text) => Disposition::Run(text),
    }
}

/// Point the kernel at the right disposition for `signum`.
///
/// Returns false when the system refuses, which is the honest answer for SIGKILL and SIGSTOP:
/// they cannot be caught or ignored by anything, and a shell that claimed otherwise would be
/// promising cleanup it can never perform.
pub fn arm(signum: i32, disposition: &Disposition<'_>) -> bool {
    // **"Back to how it was" is not the system default in an interactive shell.** The shell
    // installs its own SIGINT handler once, at REPL start, and writing `SIG_DFL` over it left the
    // session with none — the next Ctrl-C killed the shell. See `job::signals::restore_shell_signal`.
    if matches!(disposition, Disposition::Default) && crate::exec::job::restore_shell_signal(signum)
    {
        return true;
    }
    let handler: libc::sighandler_t = match disposition {
        Disposition::Default => libc::SIG_DFL,
        Disposition::Ignore => libc::SIG_IGN,
        // `sighandler_t` is an integer-shaped slot the kernel calls back through, so the function
        // item has to be flattened to a pointer first; a direct `as usize` on a function item is
        // a different (and lint-worthy) conversion.
        Disposition::Run(_) => handle_trapped_signal as *const () as usize,
    };

    // SAFETY: `sigaction` is handed a fully initialised `struct sigaction` and a null pointer for
    // the (unwanted) old disposition. The handler is an `extern "C"` function with the C
    // signature the kernel will call it with.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = handler;
        libc::sigemptyset(&mut action.sa_mask);
        // No SA_RESTART: see the module docs. A trap that only fires once the blocking call it
        // interrupted has finished is not a trap, it is a delay.
        action.sa_flags = 0;
        libc::sigaction(signum, &action, std::ptr::null_mut()) == 0
    }
}

/// Run the handler of every signal that has arrived since the last check.
///
/// Called at command boundaries, where the shell is between commands and running arbitrary code
/// is safe. Cheap enough to call unconditionally: the common case is one relaxed atomic load.
///
/// A trap body that runs `exit` propagates its [`ShellError::Exit`] to the caller, which is how
/// `trap 'exit 130' INT` ends the shell rather than merely printing something.
pub fn run_pending_traps(env: &mut Environment) -> Result<()> {
    if PENDING.load(Ordering::Relaxed) == 0 {
        return Ok(());
    }
    // A handler body is shell code, so it hits the very command boundary that called us. Without
    // this a signal arriving during a handler would recurse until the stack ran out.
    if RUNNING.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    let result = drain(env);
    RUNNING.store(false, Ordering::SeqCst);
    result
}

/// Take the whole pending set and run what it names.
///
/// Taken with a single `swap`, not read-then-clear: a signal that arrives between the two would
/// otherwise be dropped, and a dropped SIGINT is a Ctrl-C the user has to press twice.
fn drain(env: &mut Environment) -> Result<()> {
    let mut pending = PENDING.swap(0, Ordering::SeqCst);
    while pending != 0 {
        let bit = pending.trailing_zeros();
        pending &= !(1u64 << bit);
        let Some(name) = signals::name_from_number(bit as i32 + 1) else {
            continue;
        };
        let Disposition::Run(text) = disposition(env, &name) else {
            continue;
        };
        let action = text.to_string();
        run_handler(env, &action)?;
    }
    Ok(())
}

/// Run one trap body, leaving `$?` as the interrupted code left it.
///
/// A trap fires *between* two commands rather than as one of them, so the status the next command
/// sees has to be the one the previous command produced: `false; trap-fires; echo $?` must print
/// 1, not whatever the handler's last command returned.
fn run_handler(env: &mut Environment, action: &str) -> Result<i32> {
    let saved = env.last_status;
    let result = parse_and_run(env, action);
    env.last_status = saved;
    result
}

fn parse_and_run(env: &mut Environment, action: &str) -> Result<i32> {
    let ast = crate::syntax::parse_bash_script(action)?;
    crate::exec::eval_command_list(env, &ast)
}

/// Whether a DEBUG handler is on the stack, so its own commands do not fire it again.
static IN_DEBUG_TRAP: AtomicBool = AtomicBool::new(false);

/// Run the DEBUG trap, before the command about to execute.
///
/// bash fires this before each *simple* command, and with `$PROMPT_COMMAND` it forms the
/// preexec/precmd pair every bash integration is built on: this one starts a timer, that one
/// draws the prompt with the elapsed time.
///
/// **`$BASH_COMMAND` is deliberately not set.** bash names the command about to run in it, which
/// needs the parsed command rendered back to shell text — 234 lines of renderer for one variable,
/// which is not a trade oslo makes. A hook that only needs to know *that* a command is starting
/// works as it does under bash; one that needs to know *which* does not.
///
/// Three things make it safe to call from the execution path:
///
/// * **it does not recurse.** The handler is shell code and its commands are simple commands too,
///   so without the guard `trap 'date' DEBUG` would fire the trap for `date`, forever. bash has
///   the same rule and spells the exception `set -o functrace`, which oslo does not have;
/// * **it cannot change `$?`.** The handler runs between two commands, so the status the next one
///   sees must still be the previous one's. `run_handler` restores it;
/// * **a failing handler is not the command's failure.** An error inside a hook is reported and
///   the command still runs, because a broken prompt integration must not make the shell unusable.
///
pub fn run_debug_trap(env: &mut Environment) {
    if IN_DEBUG_TRAP.load(Ordering::SeqCst) {
        return;
    }
    let Disposition::Run(text) = disposition(env, "DEBUG") else {
        return;
    };
    let action = text.to_string();

    IN_DEBUG_TRAP.store(true, Ordering::SeqCst);
    let outcome = run_handler(env, &action);
    IN_DEBUG_TRAP.store(false, Ordering::SeqCst);

    if let Err(e) = outcome {
        eprintln!("{}trap: DEBUG: {e}", origin_now());
    }
}

/// Run the EXIT trap and give the status the shell should finally exit with.
///
/// Every path out of a shell goes through here — falling off the end of the script, `exit N`, a
/// fatal error, and end-of-input in the REPL — because a cleanup handler that fires on only some
/// of those is worse than none: the script's author will assume the temp file is gone.
///
/// Three rules, all observable:
///
/// * the handler sees `$?` of whatever ended the shell, so `trap 'echo $?' EXIT` after a failure
///   prints the failure's status;
/// * the shell still exits with that status unless the handler itself calls `exit`, which
///   overrides it (`trap 'exit 9' EXIT; exit 3` exits 9);
/// * it runs at most once. The handler is cleared before it runs, so an `exit` inside it — which
///   comes straight back through this function — cannot loop.
pub fn run_exit_trap(env: &mut Environment, status: i32) -> i32 {
    if EXIT_TRAP_DONE.swap(true, Ordering::SeqCst) {
        return status;
    }
    let Disposition::Run(text) = disposition(env, "EXIT") else {
        return status;
    };
    let action = text.to_string();
    env.set_trap("EXIT", DEFAULT_ACTION);
    env.last_status = status;

    // What a bare `exit` inside the action means; see [`exit_trap_status`].
    EXIT_TRAP_STATUS.with(|s| s.set(Some(status)));
    let outcome = parse_and_run(env, &action);
    EXIT_TRAP_STATUS.with(|s| s.set(None));

    match outcome {
        Ok(_) => status,
        Err(ShellError::Exit(code)) => code,
        Err(e) => {
            eprintln!("{}{}", origin_now(), e);
            status
        }
    }
}

thread_local! {
    /// The status the shell is leaving with, while the EXIT trap runs.
    ///
    /// Thread-local for the same reason as the errexit counter: the test binaries evaluate scripts
    /// on several threads, and one shell's exit must not be visible to another's.
    static EXIT_TRAP_STATUS: std::cell::Cell<Option<i32>> = const { std::cell::Cell::new(None) };
}

/// The status a bare `exit` should carry out, when one is running inside the EXIT trap.
///
/// `None` anywhere else, so `cmd; exit` keeps meaning `cmd; exit $?`.
pub fn exit_trap_status() -> Option<i32> {
    EXIT_TRAP_STATUS.with(|s| s.get())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stored_notation_round_trips_through_disposition() {
        let mut env = Environment::new();
        assert_eq!(disposition(&env, "INT"), Disposition::Default);
        env.set_trap("INT", DEFAULT_ACTION);
        assert_eq!(disposition(&env, "INT"), Disposition::Default);
        env.set_trap("INT", "");
        assert_eq!(disposition(&env, "INT"), Disposition::Ignore);
        env.set_trap("INT", "echo hi");
        assert_eq!(disposition(&env, "INT"), Disposition::Run("echo hi"));
    }

    /// SIGKILL is the case where "did the kernel accept it?" is the only honest answer available.
    #[test]
    fn arming_reports_whether_the_system_agreed() {
        assert!(!arm(nix::libc::SIGKILL, &Disposition::Run("echo never")));
        assert!(!arm(nix::libc::SIGSTOP, &Disposition::Ignore));
        assert!(arm(nix::libc::SIGURG, &Disposition::Default));
    }

    /// The drain must clear what it took even when nothing is trapped, or the next command
    /// boundary walks the same bits again forever.
    #[test]
    fn a_signal_with_no_handler_is_still_drained() {
        let mut env = Environment::new();
        PENDING.fetch_or(1u64 << (nix::libc::SIGURG - 1), Ordering::SeqCst);
        run_pending_traps(&mut env).expect("drain");
        assert_eq!(PENDING.load(Ordering::SeqCst), 0);
    }
}
