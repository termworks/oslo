//! Process substitution: `<(cmd)` and `>(cmd)`.
//!
//! The command runs in a subshell connected to a pipe, and the *word* becomes the name of that
//! pipe — `/dev/fd/N` — so the program receiving it opens an ordinary file. That is what makes
//! `diff <(sort a) <(sort b)` work without either temporary file existing.
//!
//! Not POSIX, and deferred on that basis until the evidence said otherwise: of 740 real shell
//! scripts on a working system, 74 — one in ten — failed to parse in oslo, and every one of them
//! failed on this. "Not in POSIX" is the wrong test for a shell that intends to be the only one a
//! distribution has.
//!
//! Three things decide the design:
//!
//! * **The descriptor must outlive the expansion.** The word is built during expansion and used
//!   when the command runs, so closing the pipe at the end of expansion would hand the program a
//!   descriptor that is already gone. The shell holds them on `Environment` until the command
//!   that asked for them has finished, and `crate::exec::simple` closes them on every exit path.
//! * **The child must not inherit the descriptor it is named by.** A `<(cmd)` child holding the
//!   read end open means the reader never sees EOF, and `cat <(echo hi)` hangs forever.
//! * **`/dev/fd/N` is a Linux interface**, via `/proc/self/fd`. bash uses the same one here, and
//!   oslo is Linux-only, so there is no fallback path to maintain.

use crate::env::Environment;
use crate::exec::compound::flush_stdout;
use crate::exec::pipeline::status_of;
use nix::fcntl::{FcntlArg, FdFlag, fcntl};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::{ForkResult, Pid, close, dup2, fork, pipe};
use oslo_base::ast::CommandList;
use oslo_base::error::{Result, ShellError};
use std::os::fd::{IntoRawFd, RawFd};
use std::sync::Mutex;

/// Children of process substitutions that have ended their command but not yet been reaped.
///
/// A substitution's child is **asynchronous**, and blocking on one is a deadlock waiting to
/// happen: `exec 8< <(generator)` copies the read end into descriptor 8, so the generator is
/// *meant* to go on running after the `exec` finishes. modernish's `LOOP` is built exactly that
/// way — an endless generator feeding the loop over descriptor 8, which stops when the consumer
/// closes it — and waiting for that child never returned. bash does not wait for one either.
///
/// They are still reaped, just never waited on: every attempt is `WNOHANG`, and anything still
/// running is tried again the next time a substitution finishes.
static UNREAPED: Mutex<Vec<Pid>> = Mutex::new(Vec::new());

/// One running substitution: the descriptor the caller was given, and the child feeding it.
///
/// Declared beside the list that holds it, in [`crate::env::scope`], so that the store depends on
/// nothing above it. Opened and closed here, where the forking is.
pub use crate::env::scope::Substitution;

/// Start `command` on a pipe and return the `/dev/fd/N` path naming it, plus the handle that
/// keeps it alive.
///
/// A free function rather than a method on the list, because opening one needs `&mut Environment`
/// (the child evaluates in it) and the list lives *on* the environment — the caller records the
/// returned handle once the borrow has ended.
///
/// `reads_from_command` is `<(…)`: the caller reads what the command writes.
pub fn open(
    env: &mut Environment,
    command: &str,
    reads_from_command: bool,
) -> Result<(String, Substitution)> {
    let ast = crate::syntax::parse_with_aliases(command, !env.get_aliases().is_empty(), &|n| {
        env.get_alias(n).map(str::to_string)
    })?;
    let (reader, writer) =
        pipe().map_err(|e| ShellError::ExecutionError(format!("process substitution: {e}")))?;
    let (ours, theirs) = if reads_from_command {
        (reader.into_raw_fd(), writer.into_raw_fd())
    } else {
        (writer.into_raw_fd(), reader.into_raw_fd())
    };

    // Anything buffered belongs to this shell, not to the child.
    flush_stdout();
    match oslo_base::forksafe::quiesced(|| unsafe { fork() }) {
        Ok(ForkResult::Child) => {
            crate::exec::job::reset_signals_for_child();
            // The child must not keep the end the *caller* was named by: while it holds the
            // read end of its own output pipe open, nothing ever sees EOF and the reader hangs.
            let _ = close(ours);
            let target = if reads_from_command { 1 } else { 0 };
            let _ = dup2(theirs, target);
            if theirs != target {
                let _ = close(theirs);
            }
            env.enter_subshell();
            let status = status_of(run(env, &ast));
            flush_stdout();
            std::process::exit(crate::env::builtins::run_exit_trap(env, status));
        }
        Ok(ForkResult::Parent { child }) => {
            let _ = close(theirs);
            // **Moved out of the script's own range first.** `pipe()` hands back the lowest free
            // numbers, which is 3 in a plain shell — and 3..9 belongs to the script: `cat <(echo
            // hi) 3>/dev/null` had the command's own redirection land on top of the substitution
            // it was reading, so it silently read nothing. bash allocates from 63 downward for
            // this reason; here the floor the rest of the shell already respects is enough.
            let ours = above_the_scripts_range(ours);
            // `/dev/fd/N` is opened by the *program*, which is a different process image after
            // `exec` — so the descriptor has to survive it. Rust opens everything `O_CLOEXEC`,
            // and a pipe from `nix` is no exception, so clearing it is what makes the path
            // resolvable on the other side of the `exec`.
            let _ = fcntl(ours, FcntlArg::F_SETFD(FdFlag::empty()));
            Ok((format!("/dev/fd/{ours}"), Substitution { fd: ours, child }))
        }
        Err(e) => {
            let _ = close(ours);
            let _ = close(theirs);
            Err(ShellError::ExecutionError(format!(
                "process substitution: fork failed: {e}"
            )))
        }
    }
}

/// Move a descriptor above the range a script is entitled to redirect, if it is not already there.
///
/// The original is closed on success. On failure the original is kept: a substitution on a low
/// number still works for every command that does not redirect that number, which is a great deal
/// better than refusing to run.
fn above_the_scripts_range(fd: RawFd) -> RawFd {
    use crate::exec::redirect::SAVE_FD_FLOOR;
    if fd >= SAVE_FD_FLOOR {
        return fd;
    }
    match fcntl(fd, FcntlArg::F_DUPFD(SAVE_FD_FLOOR)) {
        Ok(moved) => {
            let _ = close(fd);
            moved
        }
        Err(_) => fd,
    }
}

/// Close every descriptor in `open` and reap what can be reaped without blocking.
///
/// Closing before reaping is not tidiness: a `>(cmd)` child is *reading* from the pipe, and only
/// the last writer closing gives it EOF. Reaping first would deadlock against a child that is
/// waiting for us.
///
/// Reaping never blocks: a substitution's child is asynchronous, and `exec 8< <(gen)` keeps one
/// running on purpose. See the `UNREAPED` list above.
pub fn finish(open: &mut Vec<Substitution>) {
    for sub in open.iter() {
        let _ = close(sub.fd);
    }
    let mut unreaped = UNREAPED.lock().unwrap_or_else(|e| e.into_inner());
    unreaped.extend(open.drain(..).map(|sub| sub.child));
    unreaped.retain(|child| !collect(*child));
}

/// Whether `child` is gone — reaped now, or already reaped by someone else.
///
/// A forked subshell inherits this list but not the children, so its `waitpid` answers `ECHILD`;
/// treating that as "gone" is what keeps the list from growing in every subshell.
fn collect(child: Pid) -> bool {
    match waitpid(child, Some(WaitPidFlag::WNOHANG)) {
        Ok(WaitStatus::StillAlive) => false,
        Ok(_) | Err(_) => true,
    }
}

fn run(env: &mut Environment, ast: &CommandList) -> Result<i32> {
    crate::exec::eval_command_list(env, ast)
}
