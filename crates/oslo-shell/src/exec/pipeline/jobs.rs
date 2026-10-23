//! How a pipeline becomes an entry in the job table.
//!
//! The two ways a running command stops being the shell's foreground concern: it was started with
//! `&`, or the user suspended it. They are the same problem seen from two sides — a process group
//! the shell no longer waits for but must still be able to name — so they share a file, and both
//! end at [`crate::exec::job::with_jobs`].
//!
//! Split out of [`super`] to keep the evaluator about evaluation: nothing here decides a status
//! or runs a command, it only decides what the user is told and what the shell remembers.

use super::{describe, eval_and_or_list, is_interactive, status_of};
use crate::env::Environment;
use crate::env::builtins::run_exit_trap;
use crate::exec::compound::flush_stdout;
use crate::exec::job;
use nix::fcntl::{OFlag, open};
use nix::sys::stat::Mode;
use nix::unistd::{ForkResult, Pid, close, dup2, fork};
use oslo_base::ast::{AndOrList, Pipeline};
use oslo_base::error::{Result, ShellError};

/// Start `and_or` in the background, record it as `$!` and as a job.
pub(super) fn spawn_background(env: &mut Environment, and_or: &AndOrList) -> Result<()> {
    // R4.1: the parent's buffered output must not be duplicated by the fork.
    flush_stdout();
    // Built before the fork so both halves agree on the label, and so the allocation happens
    // where allocation is still legal.
    let label = describe::describe_and_or(and_or);
    unsafe {
        match oslo_base::forksafe::quiesced(|| fork()) {
            Ok(ForkResult::Child) => {
                // R7.1: its own process group, so a Ctrl-C aimed at the foreground job cannot
                // reach it and so `kill %1` has one group to signal. Done before the signal reset
                // because the parent may already be trying to place this child in that group.
                job::join_group_in_child(None);
                // R4.7: same fresh signal state every forked child gets. It also renounces job
                // control: a background job must never take the terminal from its shell.
                job::reset_signals_for_child();
                // R4.1: a background child is this shell in another process — it keeps
                // the environment `fork` copied instead of rebuilding one.
                env.enter_subshell();
                detach_stdin();
                let res = status_of(eval_and_or_list(env, and_or));
                flush_stdout();
                // A background job is a subshell too: `{ trap ... EXIT; ...; } &` must clean up.
                std::process::exit(run_exit_trap(env, res));
            }
            Ok(ForkResult::Parent { child }) => {
                let pgid = job::place_child(child, None);
                env.last_bg_pid = Some(child.as_raw() as u32);
                // R7.2/R7.4: the table is what `jobs`, `fg`, `bg` and `wait` read, and what lets
                // the reaper attribute a status to a job instead of dropping it.
                let id = job::with_jobs(|jobs| jobs.add_background(pgid, vec![child], label));
                announce_background_job(id, child);
                Ok(())
            }
            Err(e) => Err(ShellError::ExecutionError(format!("Fork failed: {}", e))),
        }
    }
}

/// Give a background child `/dev/null` for stdin.
///
/// Without job control there is no foreground process group to keep it off the terminal, so a
/// background `read` competes with the shell for the user's keystrokes and swallows the ones it
/// wins. POSIX asks for exactly this substitution when job control is off. An interactive shell
/// leaves stdin alone instead: R7.1 gives the job its own process group, so the tty driver stops
/// it with SIGTTIN rather than letting it steal input — which is what `fg` exists to resolve.
///
/// Called between `fork` and any exec, where only async-signal-safe calls are legal: `open`,
/// `dup2` and `close` all are.
fn detach_stdin() {
    if is_interactive() {
        return;
    }
    if let Ok(fd) = open("/dev/null", OFlag::O_RDONLY, Mode::empty()) {
        // Errors are unreportable here and leaving the inherited descriptor is no worse than
        // failing to start the job.
        let _ = dup2(fd, 0);
        if fd != 0 {
            let _ = close(fd);
        }
    }
}

/// Tell the user a job started — but only if there is a user, and never on stdout.
///
/// `println!` here was captured verbatim by every `$( )` containing a `&`, so
/// `x=$(sleep 0 & echo done)` came back as `[bg] 3881847 done`. A non-interactive shell prints
/// no notice at all, which is what bash does and what the corpus compares against.
fn announce_background_job(id: usize, child: Pid) {
    if !is_interactive() {
        return;
    }
    eprintln!("[{}] {}", id, child);
}

/// A foreground pipeline the user suspended becomes a job, so `fg` and `bg` can reach it.
///
/// R7.2: without this the processes stay stopped with nothing in the shell able to name them —
/// the state that made [`crate::exec::job::JobState::Stopped`] unreachable and Ctrl-Z a way of
/// leaking a process rather than parking one.
pub(super) fn remember_stopped_pipeline(pgid: Pid, pids: &[Pid], pipeline: &Pipeline) {
    let label = describe::describe_pipeline(pipeline);
    let line = job::with_jobs(|jobs| {
        let id = jobs.add_stopped(pgid, pids.to_vec(), label);
        // Marked as already reported: the notice goes out here, and the reaper must not repeat it
        // at the next command boundary.
        if let Some(entry) = jobs.get_mut(id) {
            entry.notified = true;
        }
        jobs.get(id).map(|entry| job::describe(entry, '+'))
    });
    if let Some(line) = line {
        eprintln!("{}", line);
    }
}
