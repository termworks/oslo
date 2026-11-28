//! What the table does when a child ends.
//!
//! Split from the table itself because it answers a different question. [`super`] is about *naming*
//! a job — `%1`, `%+`, the substring you typed — and about what `jobs` prints. This is the moment a
//! process reports, which is where the accounting has to be exactly right: a status consumed twice
//! is a `wait` that lies, and one never consumed is a zombie for the rest of the session.
//!
//! Nothing here announces anything. Every transition is queued on the table and fired by
//! [`crate::exec::job::reap`] once its lock is released — a Lua handler may ask the shell about its
//! jobs, so firing from in here would be a handler waiting for a lock its own caller holds.

use super::{JobState, LIVE_CHILDREN, Transition};
use nix::sys::wait::WaitStatus;
use nix::unistd::Pid;
use std::sync::atomic::Ordering;

/// How many statuses to remember for a `wait` that may never come.
///
/// Unbounded growth is a real leak in a long-lived interactive shell that backgrounds jobs and
/// never waits for them; a fan-out of more than this many un-waited children is not a pattern a
/// shell script has.
const REMEMBERED_STATUSES: usize = 64;

/// How many finished jobs a *non-interactive* shell keeps for a `jobs` that may never run.
const REMEMBERED_COMPLETIONS: usize = 64;

impl super::JobTable {
    /// Record a status somebody else's `waitpid` collected, so a later `wait` still has an answer.
    ///
    /// **For a child reaped on the way to another one.** `wait -n` waits with `waitpid(-1)` and
    /// gets whichever ends first; the kernel will not offer that one again, so unless the status is
    /// kept here the child's exit code is gone for good. The job accounting is the same as the
    /// reaper's — the process leaves its job, and the job ends when its last one does.
    /// `signal` because the encoded status cannot be decoded back: `128 + n` is also a status a
    /// program may exit with of its own accord, so a handler told only the number cannot tell a
    /// killed child from one that chose to say so.
    pub fn keep_status(&mut self, pid: Pid, code: i32, signal: Option<(i32, bool)>) {
        let job_id = self.job_of_pid(pid);
        self.finish(job_id, pid, code, signal);
    }

    /// Drop the oldest finished jobs a script never asked about.
    ///
    /// A non-interactive shell has no prompt to report at, so a `Done` entry waits for a `jobs`
    /// or a `wait` that may never come. bash bounds its list the same way rather than growing it
    /// for the life of the shell; the cap is high enough that a script which *does* report its
    /// jobs always finds them.
    pub(in crate::exec::job) fn forget_stale_completions(&mut self) {
        let done: Vec<usize> = self
            .jobs
            .iter()
            .filter(|job| matches!(job.state, JobState::Completed(_)))
            .map(|job| job.id)
            .collect();
        for id in done
            .iter()
            .take(done.len().saturating_sub(REMEMBERED_COMPLETIONS))
        {
            self.remove(*id);
        }
    }

    /// Every pid the reaper should ask about: a stopped job's processes are still alive, and a
    /// stopped job is exactly the one that may be resumed and finish while nobody is watching.
    pub(in crate::exec::job) fn reapable_pids(&self) -> Vec<Pid> {
        self.jobs
            .iter()
            .filter(|j| !matches!(j.state, JobState::Completed(_)))
            .flat_map(|j| j.pids.iter().copied())
            // The disowned, who have no job to be found under and are still this process's to bury.
            .chain(self.orphans.iter().copied())
            .filter(|pid| !self.foreign.contains(pid))
            .collect()
    }

    /// The table a forked child starts with: none of the parent's jobs are its children. A
    /// command substitution and a simple pipeline stage still list them, as they last were; every
    /// other subshell starts empty, as in bash.
    pub(in crate::exec::job) fn enter_child(&mut self, keep: bool) {
        LIVE_CHILDREN.store(0, Ordering::SeqCst);
        if !keep {
            *self = Self::default();
            return;
        }
        self.orphans.clear();
        self.foreign = self.jobs.iter().flat_map(|j| j.pids.clone()).collect();
    }

    /// Keep `status` as the answer a later `wait` for `pid` gets.
    pub fn remember_status(&mut self, pid: Pid, status: i32) {
        self.statuses.retain(|(p, _)| *p != pid);
        self.statuses.push((pid, status));
        if self.statuses.len() > REMEMBERED_STATUSES {
            self.statuses.remove(0);
        }
    }

    /// Fold one `waitpid` result into the table.
    pub(in crate::exec::job) fn record(&mut self, status: WaitStatus) {
        let Some(pid) = status.pid() else { return };
        let job_id = self.job_of_pid(pid);
        match status {
            WaitStatus::Exited(_, code) => self.finish(job_id, pid, code, None),
            WaitStatus::Signaled(_, sig, core) => {
                self.finish(job_id, pid, 128 + sig as i32, Some((sig as i32, core)))
            }
            WaitStatus::Stopped(_, _) => {
                if let Some(id) = job_id {
                    self.promote(id);
                    let was = self.get(id).map(|job| job.state.clone());
                    if let Some(job) = self.get_mut(id) {
                        job.state = JobState::Stopped;
                        job.notified = false;
                    }
                    if let Some(was) = was {
                        self.note_state(id, &was, &JobState::Stopped);
                    }
                }
            }
            WaitStatus::Continued(_) => {
                if let Some(id) = job_id {
                    let was = self.get(id).map(|job| job.state.clone());
                    if let Some(job) = self.get_mut(id) {
                        job.state = JobState::Running;
                    }
                    if let Some(was) = was {
                        self.note_state(id, &was, &JobState::Running);
                    }
                }
            }
            _ => {}
        }
    }

    /// Drop `pid` from the disowned, answering whether it was one of them.
    fn forget_orphan(&mut self, pid: Pid) -> bool {
        match self.orphans.iter().position(|p| *p == pid) {
            Some(at) => {
                self.orphans.remove(at);
                true
            }
            None => false,
        }
    }

    /// Stop expecting anything from `pid`: it is gone and left no status behind.
    pub(in crate::exec::job) fn abandon(&mut self, pid: Pid) {
        forget_children(1);
        if self.forget_orphan(pid) {
            return;
        }
        let Some(job) = self.job_of_pid(pid).and_then(|id| self.get_mut(id)) else {
            return;
        };
        job.pids.retain(|p| *p != pid);
        job.ended.push(pid);
        if job.pids.is_empty() {
            job.state = JobState::Completed(0);
            job.notified = false;
        }
    }

    /// A child of this job has ended; the job itself ends when its last process does.
    fn finish(&mut self, job_id: Option<usize>, pid: Pid, code: i32, signal: Option<(i32, bool)>) {
        forget_children(1);
        // **A disowned child is buried and not remembered.** There is nobody who may ask: `wait`
        // on a disowned pid is "not a child of this shell" in every shell, so keeping its status
        // would only crowd out one somebody can still use.
        if self.forget_orphan(pid) {
            return;
        }
        self.remember_status(pid, code);
        let stage = job_id
            .and_then(|id| self.get(id))
            .and_then(|job| job.stages.iter().position(|p| *p == pid))
            .map(|at| at + 1);
        self.transitions.push(Transition::ProcessExit {
            pid,
            job: job_id,
            status: code,
            stage,
            signal: signal.map(|(signal, _)| signal),
        });
        let Some(job) = job_id.and_then(|id| self.get_mut(id)) else {
            return;
        };
        if let Some((signal, core)) = signal {
            crate::exec::job::note_background_death(pid.as_raw(), signal, core, &job.command);
        }
        job.pids.retain(|p| *p != pid);
        job.ended.push(pid);
        job.outcomes.push((pid, code));
        if job.pids.is_empty() {
            // A pipeline's status is its last stage's, and the last stage is the last pid left.
            let was = job.state.clone();
            job.state = JobState::Completed(code);
            job.signal = signal;
            job.notified = false;
            if let Some(id) = job_id {
                self.note_state(id, &was, &JobState::Completed(code));
            }
        }
    }
}

/// Drop `n` from the live-child count without ever wrapping below zero.
///
/// The count is a hint for the fast path, not a fact: a child the shell forked but never recorded
/// — a command substitution whose reader raced the reaper — would otherwise take it negative and
/// disable reaping for the rest of the session.
fn forget_children(n: usize) {
    let _ = LIVE_CHILDREN.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |live| {
        Some(live.saturating_sub(n))
    });
}

#[cfg(test)]
mod tests {
    use super::super::{JobTable, Transition};
    use nix::unistd::Pid;

    /// **Each stage of a pipeline reports as itself**, with a number that does not move.
    ///
    /// The shell owns several processes for one job when a foreground pipeline is stopped and
    /// resumed, and that is the case `stage` exists for. Reaching it through the executor would
    /// mean typing Ctrl-Z at a terminal, so the table is driven directly here — the numbering is
    /// its own, not the reaper's.
    ///
    /// The middle stage is reaped **first** on purpose: the stage list is what the job started
    /// with, so a number derived from what is *left* would call the last one stage two.
    #[test]
    fn a_pipelines_stages_are_numbered_as_they_started() {
        use nix::sys::wait::WaitStatus;

        let mut table = JobTable::default();
        let pids: Vec<Pid> = (11..14).map(Pid::from_raw).collect();
        let id = table.add_stopped(pids[0], pids.clone(), "a | b | c".into());

        for (pid, code) in [(pids[1], 5), (pids[2], 6), (pids[0], 4)] {
            table.record(WaitStatus::Exited(pid, code));
        }

        let stages: Vec<(Option<usize>, i32)> = table
            .take_transitions()
            .into_iter()
            .filter_map(|t| match t {
                Transition::ProcessExit { stage, status, .. } => Some((stage, status)),
                _ => None,
            })
            .collect();
        assert_eq!(stages, vec![(Some(2), 5), (Some(3), 6), (Some(1), 4)]);

        // And the job kept every one of them, in the order the pipeline was written.
        let job = table.get(id).expect("the job");
        assert_eq!(job.stages, pids);
        assert_eq!(job.outcomes.len(), 3);
    }

    /// R7.5's half of the bargain: a status the reaper collected is still available once, by pid.
    ///
    /// Once, not twice: `bash --posix` — the oracle the differential corpus runs against — answers
    /// a repeated `wait $p` with 127. Default bash keeps the status instead, so this assertion is
    /// a deliberate choice of POSIX over bash, not an accident.
    #[test]
    fn a_reaped_status_is_available_to_wait_exactly_once() {
        let mut table = JobTable::default();
        table.remember_status(Pid::from_raw(42), 7);
        assert_eq!(table.take_status(Pid::from_raw(42)), Some(7));
        assert_eq!(table.take_status(Pid::from_raw(42)), None);
    }

    /// The remembered-status list is bounded, or a shell that never waits leaks one entry per job.
    #[test]
    fn remembered_statuses_are_capped() {
        let mut table = JobTable::default();
        for pid in 0..(super::REMEMBERED_STATUSES as i32 + 10) {
            table.remember_status(Pid::from_raw(pid + 1), pid);
        }
        assert_eq!(table.statuses.len(), super::REMEMBERED_STATUSES);
        // The oldest were dropped, the newest kept.
        assert_eq!(table.take_status(Pid::from_raw(1)), None);
        assert!(
            table
                .take_status(Pid::from_raw(super::REMEMBERED_STATUSES as i32 + 10))
                .is_some()
        );
    }
}
