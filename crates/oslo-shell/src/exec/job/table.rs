//! The job table: what the shell remembers about the children it is not currently waiting for.
//!
//! R7.2/R7.4. Before this existed, `JobManager::add_job` had no callers, [`JobState::Stopped`] was
//! unreachable, and a background child that finished stayed a zombie for the shell's lifetime
//! because nothing ever waited for it. Both halves are fixed here: every background job and every
//! stopped foreground job is recorded, and [`reap_background_jobs`] — called at every command
//! boundary by [`crate::exec::pipeline::eval_command_list`] — collects the ones that have ended.
//!
//! # The reaping invariant
//!
//! [`reap_background_jobs`] runs at command boundaries and asks after each *known* pid with
//! `WNOHANG`, never `waitpid(-1)`. Both would collect the same zombies, but `-1` also collects
//! children this table has never heard of — and the statuses it collects are kept in
//! [`JobTable::take_status`] so that a later `wait` can still report them, which is what keeps
//! opportunistic reaping from stealing `wait $!`'s answer.

use nix::unistd::Pid;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

/// Where a job is in its life.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobState {
    Running,
    Stopped,
    Completed(i32),
}

/// One entry of the job table: a process group the shell started and can still name.
#[derive(Debug, Clone)]
pub struct Job {
    /// The `%n` a user types. Reused once the table empties, as bash's are.
    pub id: usize,
    pub pgid: Pid,
    /// The processes still expected to report. Empty once the job has ended.
    pub pids: Vec<Pid>,
    /// Every process the job started, in pipeline order, and never shortened.
    ///
    /// **`pids` cannot answer "which stage was that?"** — it is what is *left*, so a pid's position
    /// in it changes every time another one ends. A stage number that moved with the reaping order
    /// would be worse than none.
    pub stages: Vec<Pid>,
    /// What each stage exited with, as it is learned.
    ///
    /// Kept per job rather than read back out of the shell's status list, which is bounded and
    /// shared: a wide pipeline could push its own early stages out of it before the last one ends.
    pub outcomes: Vec<(Pid, i32)>,
    /// The processes that already have, kept so the job stays findable by any of its pids.
    ///
    /// `wait $!` names a *pid*, and `$!` is a pipeline's last stage rather than its leader, so a
    /// job cannot be looked up by pgid instead. Dropping the pid outright left the finished entry
    /// unreachable and so never removed, and a later bare `wait -n` handed the same child back.
    pub ended: Vec<Pid>,
    /// The text the user typed, for `jobs` and for the `[1]+ Done` notice.
    pub command: String,
    pub state: JobState,
    /// Whether the user has already been told about the state this entry is in.
    ///
    /// A finished job is not dropped the instant it is reaped: bash reports it once — `jobs`
    /// shows a `Done` line, or the shell prints one — and only then forgets it.
    pub notified: bool,
    /// Whether the job is running detached from the terminal, which decides the trailing `&`.
    ///
    /// Not derivable from `state`: a job stopped with Ctrl-Z and one backgrounded with `&` are
    /// both `Running` after `bg`, but only a job the shell never had in the foreground prints
    /// `sleep 1 &`. `bg` sets this and `fg` clears it, which is exactly when bash's line changes.
    pub background: bool,
}

/// Every job the shell can still name, plus the statuses of the ones it has already reaped.
#[derive(Debug, Default)]
pub struct JobTable {
    jobs: Vec<Job>,
    /// `%%`/`%+`: the job `fg` and `bg` act on by default.
    current: Option<usize>,
    /// `%-`: the one before that.
    previous: Option<usize>,
    /// Reaped exit statuses, newest last, so `wait PID` still has an answer for a child the
    /// opportunistic reaper collected first.
    statuses: Vec<(Pid, i32)>,
    /// Children that left the job table while still alive, and must be reaped anyway.
    ///
    /// **`disown` gives up managing a job, not being its parent.** The kernel does not consult the
    /// job table: a child of this process stays a zombie until this process waits for it, and
    /// nothing else can. Dropping the pid with the job left a `<defunct>` entry for the rest of the
    /// session — reproduced, not theorised.
    ///
    /// Nothing is remembered about them but the pid. They have no job, no status anybody can ask
    /// for, and no report: the only obligation left is the one the kernel imposes.
    orphans: Vec<Pid>,
    /// What has happened to jobs and their processes since anybody last asked.
    ///
    /// **Recorded here and fired elsewhere, deliberately.** Everything that mutates this table does
    /// so holding its mutex, and a Lua handler is entitled to ask the shell about its jobs — so
    /// calling one from inside would be a handler waiting for a lock its own caller holds. The
    /// transitions queue instead, and [`super::reap`] drains and fires them once the lock is gone.
    transitions: Vec<Transition>,
    /// The processes of jobs a command substitution inherited from its parent, which it lists but
    /// cannot wait for.
    foreign: Vec<Pid>,
}

/// Something that happened to a job or one of its processes, waiting to be announced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transition {
    /// One process of a job ended.
    ProcessExit {
        pid: Pid,
        job: Option<usize>,
        status: i32,
        /// Which stage of the pipeline it was, counting from one. `None` for a process with no job.
        stage: Option<usize>,
        /// The signal that killed it, if one did.
        signal: Option<i32>,
    },
    /// A job moved between running, stopped and ended.
    JobState {
        id: usize,
        pgid: Pid,
        text: String,
        from: &'static str,
        to: &'static str,
        background: bool,
    },
}

impl JobState {
    /// The word a hook is given for this state.
    fn word(&self) -> &'static str {
        match self {
            JobState::Running => "running",
            JobState::Stopped => "stopped",
            JobState::Completed(_) => "ended",
        }
    }
}

/// Children the shell believes are still alive, as a lock-free fast path.
///
/// [`reap_background_jobs`] runs at every command boundary, including every iteration of a
/// shell-level loop. Taking a mutex and entering the kernel there would tax the common case —
/// no background jobs at all — for nothing.
pub(super) static LIVE_CHILDREN: AtomicUsize = AtomicUsize::new(0);

fn table() -> &'static Mutex<JobTable> {
    static TABLE: OnceLock<Mutex<JobTable>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(JobTable::default()))
}

/// Borrow the process-wide job table.
///
/// The entry point for everything outside this module — `jobs`, `fg`, `bg`, `disown`, `wait`.
/// A poisoned lock is recovered rather than propagated: a panic in one evaluation must not make
/// the shell unable to see its own jobs.
pub fn with_jobs<R>(f: impl FnOnce(&mut JobTable) -> R) -> R {
    let mut guard: MutexGuard<'_, JobTable> = match table().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    f(&mut guard)
}

impl JobTable {
    /// Every job, in the order they were started.
    pub fn jobs(&self) -> &[Job] {
        &self.jobs
    }

    pub fn get(&self, id: usize) -> Option<&Job> {
        self.jobs.iter().find(|j| j.id == id)
    }

    pub fn get_mut(&mut self, id: usize) -> Option<&mut Job> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }

    /// The `+` job: what `fg`/`bg`/`%%` mean with no argument.
    pub fn current_id(&self) -> Option<usize> {
        self.current
    }

    /// The `-` job: what `%-` means.
    pub fn previous_id(&self) -> Option<usize> {
        self.previous
    }

    /// The marker `jobs` prints after the job number: `+`, `-`, or a space.
    pub fn marker(&self, id: usize) -> char {
        match (self.current, self.previous) {
            (Some(cur), _) if cur == id => '+',
            (_, Some(prev)) if prev == id => '-',
            _ => ' ',
        }
    }

    /// Record a job the shell started in the background.
    pub fn add_background(&mut self, pgid: Pid, pids: Vec<Pid>, command: String) -> usize {
        self.add(pgid, pids, command, JobState::Running, true)
    }

    /// Record a foreground job that a stop signal took away from the shell.
    ///
    /// R7.2: this is the only way [`JobState::Stopped`] is ever reached. Without it, Ctrl-Z left
    /// a process suspended with nothing in the shell able to name it, so `fg` could not exist.
    pub fn add_stopped(&mut self, pgid: Pid, pids: Vec<Pid>, command: String) -> usize {
        self.add(pgid, pids, command, JobState::Stopped, false)
    }

    fn add(
        &mut self,
        pgid: Pid,
        pids: Vec<Pid>,
        command: String,
        state: JobState,
        background: bool,
    ) -> usize {
        // Job numbers restart at 1 once the table empties, as bash's do: `%1` should mean "the
        // one job I have", not "the 4001st thing this session ever backgrounded".
        let id = self.jobs.iter().map(|j| j.id).max().unwrap_or(0) + 1;
        LIVE_CHILDREN.fetch_add(pids.len(), Ordering::SeqCst);
        self.jobs.push(Job {
            id,
            pgid,
            stages: pids.clone(),
            pids,
            outcomes: Vec::new(),
            ended: Vec::new(),
            command,
            state,
            notified: false,
            background,
        });
        self.promote(id);
        id
    }

    /// Make `id` the current job, demoting the old current one to `%-`.
    pub fn promote(&mut self, id: usize) {
        if self.current == Some(id) {
            return;
        }
        self.previous = self.current;
        self.current = Some(id);
    }

    /// Forget a job without waiting for it — `disown`, or a job the user has been told about.
    pub fn remove(&mut self, id: usize) -> Option<Job> {
        let index = self.jobs.iter().position(|j| j.id == id)?;
        let job = self.jobs.remove(index);
        if self.current == Some(id) {
            self.current = self.previous.take();
        } else if self.previous == Some(id) {
            self.previous = None;
        }
        // **Still ours to reap, even when it is no longer ours to manage.** A completed job's pids
        // were reaped as they ended and are already accounted for; anything still live here is a
        // child of this process that only this process can collect, so it moves to [`orphans`]
        // rather than being forgotten. `LIVE_CHILDREN` keeps counting it for the same reason: it is
        // the fast path's "is there anything to reap", and the answer is still yes.
        if !matches!(job.state, JobState::Completed(_)) {
            for pid in &job.pids {
                if !self.orphans.contains(pid) {
                    self.orphans.push(*pid);
                }
            }
        }
        Some(job)
    }

    /// Take responsibility for a child that is not a job and never will be.
    ///
    /// **Somebody has to bury it.** SIGCHLD is *caught* rather than ignored, so the kernel does not
    /// reap for us; the reaper asks only about pids this table names; and the unrestricted
    /// `waitpid(-1)` sweep is gated on being PID 1. A child spawned and dropped outside all three —
    /// the slow-command notification is the one that exists — became a permanent `<defunct>`, one
    /// per slow command, holding a pid slot against `RLIMIT_NPROC` until something typed `wait`.
    ///
    /// `LIVE_CHILDREN` is bumped for the same reason [`Self::remove`] keeps counting: it is the fast
    /// path's "is there anything to reap at all", and a shell with no jobs still has this one.
    pub fn adopt_stray(&mut self, pid: Pid) {
        if self.orphans.contains(&pid) {
            return;
        }
        self.orphans.push(pid);
        LIVE_CHILDREN.fetch_add(1, Ordering::SeqCst);
    }

    /// Resolve a job spec — `%%`, `%+`, `%-`, `%n`, `%prefix`, `%?substring` — or a bare number.
    ///
    /// Returns the job id, not an index: ids are what the user sees and what survives a removal.
    pub fn lookup(&self, spec: &str) -> Option<usize> {
        let body = spec.strip_prefix('%').unwrap_or(spec);
        match body {
            "" | "%" | "+" => self.current,
            "-" => self.previous,
            _ => {
                if let Ok(n) = body.parse::<usize>() {
                    return self.get(n).map(|j| j.id);
                }
                if let Some(needle) = body.strip_prefix('?') {
                    return self
                        .jobs
                        .iter()
                        .find(|j| j.command.contains(needle))
                        .map(|j| j.id);
                }
                self.jobs
                    .iter()
                    .find(|j| j.command.starts_with(body))
                    .map(|j| j.id)
            }
        }
    }

    /// The job that owns `pid`, if the shell still remembers one.
    ///
    /// Processes that have already ended count: a job is not un-named by finishing, and the `wait`
    /// that reports it has to reach the same entry in order to retire it.
    pub fn job_of_pid(&self, pid: Pid) -> Option<usize> {
        self.jobs
            .iter()
            .find(|j| j.pids.contains(&pid) || j.ended.contains(&pid))
            .map(|j| j.id)
    }

    /// Take the exit status the reaper collected for `pid`, if it collected one.
    ///
    /// R7.5 depends on this: once `waitpid(-1, WNOHANG)` has reaped a background child there is
    /// nothing left for `wait $!` to wait for, and the kernel answers `ECHILD`. The status has to
    /// come from here instead, and it is removed as it is read so that a second `wait` on the same
    /// pid reports "no such job" rather than the same answer twice.
    ///
    /// Forgetting on read is the *POSIX* rule, and it is a real fork in behaviour: `bash --posix`
    /// answers the second `wait $p` with 127 while default bash answers with the status again.
    /// oslo follows the POSIX side, which is also the oracle the differential corpus runs against.
    pub fn take_status(&mut self, pid: Pid) -> Option<i32> {
        let index = self.statuses.iter().position(|(p, _)| *p == pid)?;
        Some(self.statuses.remove(index).1)
    }

    /// The remembered status of `pid`, left in place for the next `wait` to ask about.
    pub fn peek_status(&self, pid: Pid) -> Option<i32> {
        self.statuses
            .iter()
            .find(|(p, _)| *p == pid)
            .map(|(_, status)| *status)
    }

    /// Take everything that has happened since the last drain, for a caller that no longer holds
    /// the lock and can therefore afford to run Lua.
    pub fn take_transitions(&mut self) -> Vec<Transition> {
        std::mem::take(&mut self.transitions)
    }

    /// Note a job's move from one state to another, if it is a move at all.
    fn note_state(&mut self, id: usize, from: &JobState, to: &JobState) {
        if from.word() == to.word() {
            return;
        }
        let (from, to) = (from.word(), to.word());
        if let Some(job) = self.get(id) {
            self.transitions.push(Transition::JobState {
                id,
                pgid: job.pgid,
                text: job.command.clone(),
                from,
                to,
                background: job.background,
            });
        }
    }

    /// Every pid the shell still expects to outlive the current command.
    pub fn live_pids(&self) -> Vec<Pid> {
        self.jobs
            .iter()
            .filter(|j| j.state != JobState::Stopped)
            .filter(|j| !matches!(j.state, JobState::Completed(_)))
            .flat_map(|j| j.pids.iter().copied())
            .collect()
    }
}

mod ending;

#[cfg(test)]
mod tests {
    use super::JobTable;
    use nix::unistd::Pid;

    fn table_with(names: &[&str]) -> JobTable {
        let mut table = JobTable::default();
        for (i, name) in names.iter().enumerate() {
            table.add_background(
                Pid::from_raw(1000 + i as i32),
                vec![Pid::from_raw(1000 + i as i32)],
                (*name).to_string(),
            );
        }
        table
    }

    /// `%%`, `%+` and `%-` are the specs a user types most; they follow the *order jobs were
    /// started*, with the newest current.
    #[test]
    fn current_and_previous_track_the_two_newest_jobs() {
        let table = table_with(&["sleep 1", "sleep 2", "sleep 3"]);
        assert_eq!(table.current_id(), Some(3));
        assert_eq!(table.previous_id(), Some(2));
        assert_eq!(table.lookup("%%"), Some(3));
        assert_eq!(table.lookup("%+"), Some(3));
        assert_eq!(table.lookup("%-"), Some(2));
        assert_eq!(table.marker(3), '+');
        assert_eq!(table.marker(2), '-');
        assert_eq!(table.marker(1), ' ');
    }

    /// The other spec forms: a number, a command prefix, and a `?substring`.
    #[test]
    fn job_specs_name_a_job_by_number_prefix_or_substring() {
        let table = table_with(&["find / -name x", "grep foo bar"]);
        assert_eq!(table.lookup("%1"), Some(1));
        assert_eq!(table.lookup("1"), Some(1));
        assert_eq!(table.lookup("%grep"), Some(2));
        assert_eq!(table.lookup("%?foo"), Some(2));
        assert_eq!(table.lookup("%nosuch"), None);
        assert_eq!(table.lookup("%9"), None);
    }

    /// Removing the current job promotes the previous one, so `%%` never dangles.
    #[test]
    fn removing_the_current_job_promotes_the_previous() {
        let mut table = table_with(&["a", "b"]);
        table.remove(2);
        assert_eq!(table.current_id(), Some(1));
        assert_eq!(table.previous_id(), None);
    }

    /// Job numbers are reused once the table empties: a session that backgrounds one job at a
    /// time should keep calling it `%1`.
    #[test]
    fn job_numbers_restart_when_the_table_empties() {
        let mut table = table_with(&["a"]);
        table.remove(1);
        let id = table.add_background(Pid::from_raw(9), vec![Pid::from_raw(9)], "b".into());
        assert_eq!(id, 1);
    }
}
