//! Bringing a job back: `fg` and `bg`.
//!
//! Split from [`super::table`] because the two files answer different questions. The table is
//! bookkeeping — which jobs exist, what state each is in, which one `%%` means. This is the part
//! that *acts*: it signals a process group, moves the terminal, and blocks. Keeping them apart
//! also keeps R7.1's terminal discipline in one place, so the `fg` builtin never has to know that
//! a `tcsetpgrp` needs SIGTTOU blocked around it.

use super::control;
use super::report::describe;
use super::table::{JobState, with_jobs};
use nix::sys::signal::{Signal, killpg};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::Pid;

/// Resume a stopped job in the background: `bg`.
///
/// Returns false when there is no such job. SIGCONT goes to the whole group, because a stopped
/// pipeline has every stage stopped.
pub fn continue_in_background(id: usize) -> bool {
    with_jobs(|jobs| {
        let Some(job) = jobs.get_mut(id) else {
            return false;
        };
        job.state = JobState::Running;
        job.notified = true;
        // `bg` is what turns a stopped foreground job into a detached one, so from here on its
        // `jobs` line carries the `&`.
        job.background = true;
        let pgid = job.pgid;
        let _ = killpg(pgid, Signal::SIGCONT);
        jobs.promote(id);
        true
    })
}

/// Bring a job back to the foreground and wait for it: `fg`.
///
/// Returns the status it ended with, or `None` when there is no such job. The terminal handling
/// is here rather than in the builtin so that R7.1's rule — hand the terminal over, take it back
/// with SIGTTOU blocked — has exactly one implementation.
pub fn foreground_job(id: usize) -> Option<i32> {
    let (pgid, pids, command) = with_jobs(|jobs| {
        let job = jobs.get(id)?;
        Some((job.pgid, job.pids.clone(), job.command.clone()))
    })?;

    with_jobs(|jobs| {
        if let Some(job) = jobs.get_mut(id) {
            job.state = JobState::Running;
            job.notified = true;
            // The shell owns the terminal again, so the `Stopped` notice a later Ctrl-Z prints
            // must not claim the job is detached.
            job.background = false;
        }
        jobs.promote(id);
    });

    control::give_terminal_to(pgid);
    let _ = killpg(pgid, Signal::SIGCONT);
    let status = wait_for_foreground(id, pgid, &pids, &command);
    control::reclaim_terminal(control::left_it_deliberately(status));
    Some(status)
}

/// Wait for every process of a job that is running in the foreground.
///
/// A stop leaves the job in the table for `fg`/`bg` to find again; a clean end removes it.
fn wait_for_foreground(id: usize, pgid: Pid, pids: &[Pid], command: &str) -> i32 {
    let mut status = 0;
    for pid in pids {
        match wait_one(*pid) {
            Outcome::Ended(code) => status = code,
            Outcome::Stopped => {
                with_jobs(|jobs| {
                    if let Some(job) = jobs.get_mut(id) {
                        job.state = JobState::Stopped;
                        job.notified = true;
                    } else {
                        jobs.add_stopped(pgid, pids.to_vec(), command.to_string());
                    }
                    if let Some(job) = jobs.get(id) {
                        eprintln!("{}", describe(job, '+'));
                    }
                });
                return 128 + Signal::SIGTSTP as i32;
            }
        }
    }
    with_jobs(|jobs| {
        jobs.remove(id);
    });
    status
}

enum Outcome {
    Ended(i32),
    Stopped,
}

/// One `waitpid` per child, retried across `EINTR`, reporting a stop as well as a death.
fn wait_one(pid: Pid) -> Outcome {
    loop {
        return match waitpid(pid, Some(WaitPidFlag::WUNTRACED)) {
            Ok(WaitStatus::Exited(_, code)) => Outcome::Ended(code),
            Ok(WaitStatus::Signaled(_, sig, _)) => Outcome::Ended(128 + sig as i32),
            Ok(WaitStatus::Stopped(_, _)) => Outcome::Stopped,
            Ok(_) => continue,
            Err(nix::errno::Errno::EINTR) => continue,
            // The child is gone and someone else reaped it; there is no status left to report.
            Err(_) => Outcome::Ended(0),
        };
    }
}
