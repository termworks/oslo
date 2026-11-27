//! A pipeline of bytes: one process per stage, and with `lastpipe` the last stage in this shell.

use super::{errexit, eval_command, jobs, segments, wait_for_child};
use crate::env::Environment;
use crate::env::builtins::run_exit_trap;
use crate::exec::compound::flush_stdout;
use crate::exec::job;
use crate::exec::pipeline::status_of;
use nix::unistd::{ForkResult, Pid, close, dup2, fork, pipe};
use oslo_base::ast::{Command, Pipeline};
use oslo_base::error::{Result, ShellError};
use std::os::fd::{AsRawFd, IntoRawFd, OwnedFd};

/// The pipeline as oslo has always run it: one process per stage, bytes on every descriptor.
pub(super) fn run_byte_stages(env: &mut Environment, pipeline: &Pipeline) -> Result<i32> {
    if let [command] = pipeline.commands.as_slice() {
        let status = eval_command(env, command)?;
        // A one-command pipeline still has a stage vector, but a compound leaves the one its last
        // command published: `if false; then :; fi` reports `PIPESTATUS` 1.
        if errexit::command_reports_own_status(command) {
            env.set_pipeline_status(vec![status]);
        }
        return Ok(status);
    }

    let num_cmds = pipeline.commands.len();
    // `shopt -s lastpipe` with job control off keeps the last stage in this shell.
    let last_here = crate::env::builtins::shopt::on("lastpipe") && !env.monitor();
    let forked = if last_here { num_cmds - 1 } else { num_cmds };
    let mut pipes = Vec::new();
    for _ in 0..num_cmds - 1 {
        let p = pipe()
            .map_err(|e| ShellError::ExecutionError(format!("Pipe creation failed: {}", e)))?;
        pipes.push(p);
    }

    let mut pids = Vec::new();
    // Every stage of a foreground pipeline shares one process group, led by the first stage, so
    // Ctrl-C reaches the whole pipe and only the pipe, and Ctrl-Z stops all of it at once.
    let mut pgid: Option<Pid> = None;

    for (idx, cmd) in pipeline.commands.iter().enumerate().take(forked) {
        // bash runs a stage's DEBUG trap in the shell itself, before forking it.
        let traced = stage_debug_trap(env, cmd);
        unsafe {
            match oslo_base::forksafe::quiesced(|| fork()) {
                Ok(ForkResult::Child) => {
                    // Before the signal reset: the parent is about to hand the terminal to this
                    // group, and a stage outside it would be a background process on the tty.
                    job::join_foreground_group(pgid);
                    // A stage gets a fresh signal state, so SIGPIPE kills it on a closed pipe.
                    crate::exec::job::reset_signals_for_child();
                    if idx > 0 {
                        let _ = dup2(pipes[idx - 1].0.as_raw_fd(), 0);
                    }
                    if idx < num_cmds - 1 {
                        let _ = dup2(pipes[idx].1.as_raw_fd(), 1);
                    }
                    close_all(&pipes);
                    let counted = super::stage::stage_is_nested(env, cmd);
                    env.enter_pipeline_stage(counted);
                    if traced {
                        crate::env::builtins::skip_next_debug_trap();
                    }
                    // `echo x | exit 3` exits the stage with 3, not with 1.
                    let status = status_of(eval_command(env, cmd));
                    flush_stdout();
                    std::process::exit(run_exit_trap(env, status));
                }
                Ok(ForkResult::Parent { child }) => {
                    // The same `setpgid` the child just made, from the other side.
                    pgid = job::place_foreground_child(child, pgid).or(pgid);
                    pids.push(child);
                }
                Err(e) => return Err(ShellError::ExecutionError(format!("Fork failed: {}", e))),
            }
        }
    }

    let last = if last_here {
        Some(run_last_here(env, &pipeline.commands[num_cmds - 1], pipes))
    } else {
        for p in pipes {
            let _ = close(p.0.into_raw_fd());
            let _ = close(p.1.into_raw_fd());
        }
        None
    };

    // The pipeline is the foreground job now, so it owns the terminal until it is done.
    if let Some(pgid) = pgid {
        job::give_terminal_to(pgid);
    }

    // Every stage's status is kept, so `false | true` leaves a trace for `PIPESTATUS` and
    // `pipefail` to read.
    let mut stage_statuses = Vec::with_capacity(num_cmds);
    let mut stopped = false;
    for pid in &pids {
        let (status, was_stopped) = wait_for_child(*pid);
        stopped |= was_stopped;
        stage_statuses.push(status);
    }
    job::reclaim_terminal(
        !stopped
            && stage_statuses
                .iter()
                .all(|status| job::left_it_deliberately(*status)),
    );
    if stopped && let Some(pgid) = pgid {
        jobs::remember_stopped_pipeline(pgid, &pids, pipeline);
    }
    if let Some(last) = last {
        stage_statuses.push(last?);
    }
    segments::note_pipeline(&pipeline.commands, &stage_statuses);
    let final_status = pipeline_status(env, &stage_statuses);
    env.set_pipeline_status(stage_statuses);
    Ok(final_status)
}

/// Run the DEBUG trap for a simple-command stage before it forks; true when it did.
fn stage_debug_trap(env: &mut Environment, cmd: &Command) -> bool {
    let Command::Simple(simple) = cmd else {
        return false;
    };
    if !crate::env::builtins::in_trap() {
        env.set_bash_command(crate::env::builtins::control::format_simple(simple));
    }
    crate::env::builtins::run_debug_trap(env);
    true
}

fn close_all(pipes: &[(OwnedFd, OwnedFd)]) {
    for p in pipes {
        let _ = close(p.0.as_raw_fd());
        let _ = close(p.1.as_raw_fd());
    }
}

/// Run the last stage in this shell, reading the pipe, with stdin put back afterwards.
fn run_last_here(env: &mut Environment, cmd: &Command, pipes: Vec<(OwnedFd, OwnedFd)>) -> Result<i32> {
    use nix::fcntl::{FcntlArg, fcntl};
    let saved = fcntl(0, FcntlArg::F_DUPFD_CLOEXEC(10)).ok();
    if let Some((read, _)) = pipes.last() {
        let _ = dup2(read.as_raw_fd(), 0);
    }
    drop(pipes);
    let result = eval_command(env, cmd);
    flush_stdout();
    match saved {
        Some(fd) => {
            let _ = dup2(fd, 0);
            let _ = close(fd);
        }
        None => {
            let _ = close(0);
        }
    }
    result
}

/// The status a pipeline reports, given what each of its stages returned.
///
/// `set -o pipefail` swaps "the last stage" for "the rightmost stage that failed", so
/// `false | true` reports 1 while `true | false | true` still reports the 1 in the middle.
pub(crate) fn pipeline_status(env: &Environment, stages: &[i32]) -> i32 {
    if env.pipefail() {
        // Rightmost, not first: `exit 3 | exit 4` is 4 in bash, the failure closest to the output.
        if let Some(failed) = stages.iter().rev().find(|s| **s != 0) {
            return *failed;
        }
    }
    stages.last().copied().unwrap_or(0)
}
