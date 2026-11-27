//! `coproc [NAME] command`: a background job whose stdin and stdout are pipes to the shell.
//!
//! The shell keeps the read end of the coprocess's stdout in `NAME[0]` and the write end of its
//! stdin in `NAME[1]`, both close-on-exec, and its pid in `NAME_PID` and `$!`. Once the job is
//! known to have ended, the next command boundary closes both ends and unsets the two variables,
//! as bash does.

use crate::env::Environment;
use crate::env::builtins::run_exit_trap;
use crate::env::scope::ShellArray;
use crate::exec::compound::flush_stdout;
use crate::exec::job;
use crate::exec::pipeline::{eval_command, is_interactive, status_of};
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use nix::unistd::{ForkResult, Pid, close, dup2, fork, pipe2};
use oslo_base::ast::Command;
use oslo_base::error::{Result, ShellError};
use std::os::fd::{IntoRawFd, RawFd};
use std::sync::Mutex;

/// The descriptors are parked just under this, highest first, where bash puts its own.
const HIGH_FD_CEILING: RawFd = 64;

/// The coprocess this shell last started and has not yet seen end.
struct Active {
    pid: Pid,
    name: String,
    read: RawFd,
    write: RawFd,
    /// The shell process that started it; a subshell inherits the ends and never reaps them.
    owner: u32,
}

static ACTIVE: Mutex<Vec<Active>> = Mutex::new(Vec::new());

pub(crate) fn start(env: &mut Environment, name: &str, body: &Command) -> Result<i32> {
    let (out_read, out_write) = pipe()?;
    let (in_read, in_write) = pipe()?;
    let [out_read, out_write, in_read, in_write] =
        [out_read, out_write, in_read, in_write].map(move_high);
    let label = crate::exec::pipeline::describe::describe_coproc(name, body);
    flush_stdout();
    match unsafe { oslo_base::forksafe::quiesced(|| fork()) } {
        Ok(ForkResult::Child) => {
            job::join_group_in_child(None);
            job::reset_signals_for_child();
            env.enter_subshell();
            let _ = dup2(in_read, 0);
            let _ = dup2(out_write, 1);
            for fd in [out_read, out_write, in_read, in_write] {
                let _ = close(fd);
            }
            if let Ok(mut active) = ACTIVE.lock() {
                for previous in active.drain(..) {
                    let _ = close(previous.read);
                    let _ = close(previous.write);
                }
            }
            let status = status_of(eval_command(env, body));
            flush_stdout();
            std::process::exit(run_exit_trap(env, status));
        }
        Ok(ForkResult::Parent { child }) => {
            let _ = close(out_write);
            let _ = close(in_read);
            let pgid = job::place_child(child, None);
            env.last_bg_pid = Some(child.as_raw() as u32);
            let id = job::with_jobs(|jobs| jobs.add_background(pgid, vec![child], label));
            if is_interactive() {
                eprintln!("[{id}] {child}");
            }
            env.set_array(
                name,
                ShellArray::from_values([out_read.to_string(), in_write.to_string()]),
            );
            env.set_var(&format!("{name}_PID"), &child.to_string(), false);
            if let Ok(mut active) = ACTIVE.lock() {
                active.push(Active {
                    pid: child,
                    name: name.to_string(),
                    read: out_read,
                    write: in_write,
                    owner: std::process::id(),
                });
            }
            Ok(0)
        }
        Err(e) => {
            for fd in [out_read, out_write, in_read, in_write] {
                let _ = close(fd);
            }
            Err(ShellError::ExecutionError(format!(
                "coproc: fork failed: {e}"
            )))
        }
    }
}

/// Close the ends and unset `NAME` and `NAME_PID` once the coprocess has ended.
pub(crate) fn reap(env: &mut Environment) {
    let Ok(mut active) = ACTIVE.lock() else {
        return;
    };
    let me = std::process::id();
    if active.iter().all(|coproc| coproc.owner != me) {
        return;
    }
    let live = job::with_jobs(|jobs| jobs.live_pids());
    let (ended, running): (Vec<Active>, Vec<Active>) = active
        .drain(..)
        .partition(|coproc| coproc.owner == me && !live.contains(&coproc.pid));
    *active = running;
    drop(active);
    for coproc in ended {
        let _ = close(coproc.read);
        let _ = close(coproc.write);
        env.unset_var(&coproc.name);
        env.unset_var(&format!("{}_PID", coproc.name));
    }
}

fn pipe() -> Result<(RawFd, RawFd)> {
    let (read, write) = pipe2(OFlag::O_CLOEXEC)
        .map_err(|e| ShellError::ExecutionError(format!("coproc: cannot make pipe: {e}")))?;
    Ok((read.into_raw_fd(), write.into_raw_fd()))
}

/// Move `fd` to the highest free descriptor below [`HIGH_FD_CEILING`], close-on-exec.
fn move_high(fd: RawFd) -> RawFd {
    let free = (super::redirect::SAVE_FD_FLOOR..HIGH_FD_CEILING)
        .rev()
        .find(|n| fcntl(*n, FcntlArg::F_GETFD).is_err());
    let Some(target) = free else {
        return fd;
    };
    match fcntl(fd, FcntlArg::F_DUPFD_CLOEXEC(target)) {
        Ok(moved) => {
            let _ = close(fd);
            moved
        }
        Err(_) => fd,
    }
}
