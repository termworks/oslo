//! What the shell *tells the user* about its jobs.
//!
//! Split from [`super::table`] because the two answer different questions. The table is
//! bookkeeping — which jobs exist, what state each is in, which one `%%` means. This is the
//! rendering and the announcing: the exact column layout `jobs` prints, and the rule about who is
//! allowed to print a `Done` line so the user never sees it twice.
//!
//! Both halves of that rule are here on purpose. The layout is only right if `describe` and the
//! change notice agree, and they only agree if one function produces both.

use super::table::{Job, JobState, JobTable};

/// One line of `jobs` output, and the notice printed when a job changes state.
///
/// bash's column layout, because that is what scripts and eyes both expect: the state is padded
/// to 27 columns and a job running detached keeps the `&` the user typed, so the line reads
/// `[1]+  Running                    sleep 1 &`.
pub fn describe(job: &Job, marker: char) -> String {
    let state = match (job.state.clone(), job.signal) {
        (JobState::Running, _) => "Running".to_string(),
        (JobState::Stopped, _) => "Stopped".to_string(),
        (JobState::Completed(_), Some((signal, core))) => {
            let core = if core { "(core dumped) " } else { "" };
            format!(
                "{:<width$}{core}",
                signal_description(signal),
                width = STATE_COLUMN
            )
        }
        (JobState::Completed(0), None) => "Done".to_string(),
        (JobState::Completed(code), None) => format!("Exit {}", code),
    };
    // The `&` says "still running detached", not "was started with `&`": bash prints
    // `sleep 1 &` while it runs and plain `sleep 1` on the `Done` line, because a finished job is
    // not in the background any more.
    let amp = match job.background && job.state == JobState::Running {
        true => " &",
        false => "",
    };
    format!(
        "[{}]{}  {:<width$}{}{}",
        job.id,
        marker,
        state,
        job.command,
        amp,
        width = STATE_COLUMN
    )
}

/// Width of the state column in `jobs` output, measured against bash 5: `Done` is followed by 23
/// spaces, `Running` by 20, so the command always starts in column 34.
const STATE_COLUMN: usize = 27;

/// What `strsignal` calls `signal`: `Killed`, `Segmentation fault`, `User defined signal 1`.
pub fn signal_description(signal: i32) -> String {
    // SAFETY: `strsignal` returns a pointer to a static string, read before anything else runs.
    let name = unsafe { std::ffi::CStr::from_ptr(nix::libc::strsignal(signal)) };
    name.to_string_lossy().into_owned()
}

/// A background process a signal ended, waiting for its notice.
struct Death {
    pid: i32,
    signal: i32,
    core: bool,
    command: String,
}

thread_local! {
    static DEATHS: std::cell::RefCell<Vec<Death>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Queue the notice for a background process that `signal` ended.
pub fn note_background_death(pid: i32, signal: i32, core: bool, command: &str) {
    let death = Death {
        pid,
        signal,
        core,
        command: command.to_string(),
    };
    DEATHS.with(|deaths| deaths.borrow_mut().push(death));
}

/// Drop the queued notices, for a subshell whose parent will give them.
pub fn forget_background_deaths() {
    DEATHS.with(|deaths| deaths.borrow_mut().clear());
}

/// Say what non-interactive bash says, at a top-level command or a `wait`, of a background job a
/// signal ended: `line 4: 1234 Segmentation fault         (core dumped) sleep 5`.
///
/// Not for SIGINT, SIGTERM or SIGPIPE, nor for a trapped signal, nor in a POSIX-mode `-c`.
pub fn announce_background_deaths(env: &crate::env::Environment) {
    let deaths = DEATHS.with(|deaths| std::mem::take(&mut *deaths.borrow_mut()));
    let command_string = env.option(crate::env::options::ShellOption::CommandString);
    if crate::exec::pipeline::is_interactive() || (env.posix() && command_string) {
        return;
    }
    use nix::libc::{SIGINT, SIGPIPE, SIGTERM};
    for death in deaths {
        let trapped = nix::sys::signal::Signal::try_from(death.signal)
            .ok()
            .and_then(|signal| signal.as_str().strip_prefix("SIG"))
            .and_then(|name| env.get_trap(name))
            .is_some_and(|action| action != "-");
        if matches!(death.signal, SIGINT | SIGTERM | SIGPIPE) || trapped {
            continue;
        }
        let core = if death.core { "(core dumped) " } else { "" };
        let name = signal_description(death.signal);
        eprintln!(
            "{}{} {name:<width$}{core}{}",
            env.origin(),
            death.pid,
            death.command,
            width = STATE_COLUMN
        );
        // Told once: `jobs` has nothing left to say about it.
        super::with_jobs(|jobs| {
            let pid = nix::unistd::Pid::from_raw(death.pid);
            if let Some(id) = jobs.job_of_pid(pid)
                && jobs
                    .get(id)
                    .is_some_and(|job| matches!(job.state, JobState::Completed(_)))
            {
                jobs.remove(id);
            }
        });
    }
}

/// Tell an interactive user what changed, and forget the jobs that have nothing left to say.
///
/// Exactly one thing may report a finished job, or the user sees the `Done` line twice. bash
/// resolves that by having whichever printer gets there first delete the entry, and so does this:
/// an interactive shell announces a job at the command boundary after it ends and drops it in the
/// same breath, which leaves the `jobs` builtin nothing to repeat.
///
/// A non-interactive shell announces nothing — the notice would land in the middle of a script's
/// output and inside every `$( )` that contains a `&` — so there the entry is left alone and
/// `jobs` (or `wait`) is the one printer, reporting it once and removing it.
///
/// A *stopped* job is announced but kept: `fg` and `bg` exist to act on it, and `jobs` lists it
/// every time until it is resumed.
/// One job's notice, taken out of the table so it can be told without holding the lock.
pub(super) struct Notice {
    id: usize,
    pid: i32,
    command: String,
    status: i32,
    ended: bool,
    /// What the built-in line would say, when no handler draws it instead.
    line: String,
    pipestatus: String,
}

/// Tell the hooks and the user what [`announce_changes`] found.
///
/// **Outside the job table's lock, and that is the whole point.** A handler is entitled to ask the
/// shell about its jobs — `oslo.job.list()` takes that same lock, which is a plain non-reentrant
/// `Mutex` — so firing from inside was a handler waiting for a lock its own caller held. The rule
/// was already written down one function along, for `announce`; the `on-report` and `on-job-finish`
/// hooks here were simply on the wrong side of it, and an `oslo.on.job_finish` that listed jobs
/// wedged the shell the first time any background command finished.
pub(super) fn tell(notices: Vec<Notice>) {
    for notice in notices {
        // **Asked before the line is printed**, or a handler could only ever add to the notice
        // rather than replace it. `on-report` is the "how should this look" question; the
        // `on-job-finish` hook below is the "this happened" one, and both fire.
        if !drawn_by_config(
            notice.id,
            notice.pid,
            &notice.command,
            notice.status,
            notice.ended,
        ) {
            eprintln!("{}", notice.line);
        }
        if notice.ended {
            // `on-job-finish`, beside the `[1]+ Done` line rather than instead of it, and only for
            // a job that *ended* — a stopped job is announced here too but it has not finished,
            // and `fg` may yet resume it.
            oslo_base::hooks::fire_at_here(
                oslo_base::hooks::at::JOB_FINISH,
                &[
                    ("id", &notice.id.to_string()),
                    ("pid", &notice.pid.to_string()),
                    ("text", &notice.command),
                    ("status", &notice.status.to_string()),
                    ("pipestatus", &notice.pipestatus),
                ],
            );
        }
    }
}

pub(super) fn announce_changes(jobs: &mut JobTable) -> Vec<Notice> {
    if !crate::exec::pipeline::is_interactive() {
        jobs.forget_stale_completions();
        return Vec::new();
    }
    let ids: Vec<usize> = jobs.jobs().iter().map(|job| job.id).collect();
    let mut retire = Vec::new();
    let mut notices = Vec::new();

    for id in ids {
        let marker = jobs.marker(id);
        let Some(job) = jobs.get_mut(id) else {
            continue;
        };
        if job.notified {
            continue;
        }
        let ended = matches!(job.state, JobState::Completed(_));
        if !ended && job.state != JobState::Stopped {
            continue;
        }
        job.notified = true;
        let status = match job.state {
            JobState::Completed(code) => code,
            _ => 0,
        };
        // Everything the notice needs is taken here, while the table is in hand; saying it is the
        // caller's job, once the lock is gone. See [`tell`].
        notices.push(Notice {
            id,
            pid: job.pgid.as_raw(),
            command: job.command.clone(),
            status,
            ended,
            line: describe(job, marker),
            pipestatus: pipestatus(job),
        });
        if ended {
            retire.push(id);
        }
    }
    for id in retire {
        jobs.remove(id);
    }
    notices
}

/// Every stage's status, in pipeline order, space-separated — the shape `$PIPESTATUS` has.
///
/// **`status` alone is the last stage's**, which for `grep x file | head` says nothing about
/// whether `grep` found anything. A stage that left no status behind — disowned, or reaped by
/// somebody who did not record it — is written as `?` rather than as a plausible number.
fn pipestatus(job: &Job) -> String {
    job.stages
        .iter()
        .map(|pid| match job.outcomes.iter().find(|(p, _)| p == pid) {
            Some((_, code)) => code.to_string(),
            None => "?".to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether an `on-report` handler drew this job's notice instead.
///
/// `ended` separates the two things this line can be: a job that finished, and one that merely
/// stopped and which `fg` may yet resume. A handler that only wants to announce completions checks
/// it rather than guessing from the status, which is zero for both.
fn drawn_by_config(id: usize, pid: i32, command: &str, status: i32, ended: bool) -> bool {
    use oslo_ui::report::{self, int, text};
    if !report::watched() {
        return false;
    }
    report::handled(
        "job",
        vec![
            ("id", int(id as i64)),
            ("pid", int(i64::from(pid))),
            ("text", text(command)),
            ("status", int(i64::from(status))),
            ("ended", oslo_base::value::Value::Bool(ended)),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use nix::unistd::Pid;

    fn running(command: &str) -> Job {
        Job {
            id: 1,
            pgid: Pid::from_raw(5),
            pids: vec![Pid::from_raw(5)],
            stages: vec![Pid::from_raw(5)],
            outcomes: Vec::new(),
            ended: Vec::new(),
            command: command.to_string(),
            state: JobState::Running,
            signal: None,
            notified: false,
            background: false,
        }
    }

    /// **Every stage's status, in the order the pipeline was written.**
    ///
    /// `status` is the last stage's, which is exactly the thing a pipeline's failure usually is
    /// not: `grep x file | head` succeeds whether or not `grep` found anything.
    #[test]
    fn pipestatus_is_every_stage_in_order() {
        let mut job = running("a | b | c");
        job.stages = (11..14).map(Pid::from_raw).collect();
        // Reported out of order, and the middle one never reported at all.
        job.outcomes = vec![(Pid::from_raw(13), 6), (Pid::from_raw(11), 4)];

        assert_eq!(pipestatus(&job), "4 ? 6");
    }

    /// The line `jobs` prints, and the notice a finished job produces.
    ///
    /// Every column here was measured against bash 5 rather than guessed: `bash -c 'sleep 1 &
    /// jobs'` starts the command in column 34, and the state column used to be three characters
    /// short of that.
    #[test]
    fn a_job_line_reads_like_bashs() {
        let job = running("sleep 10");
        assert_eq!(
            describe(&job, '+'),
            "[1]+  Running                    sleep 10"
        );

        let done = Job {
            state: JobState::Completed(3),
            ..job.clone()
        };
        assert_eq!(
            describe(&done, '-'),
            "[1]-  Exit 3                     sleep 10"
        );

        let finished = Job {
            state: JobState::Completed(0),
            ..job.clone()
        };
        assert_eq!(
            describe(&finished, ' '),
            "[1]   Done                       sleep 10"
        );

        let stopped = Job {
            state: JobState::Stopped,
            ..job.clone()
        };
        assert_eq!(
            describe(&stopped, ' '),
            "[1]   Stopped                    sleep 10"
        );
    }

    /// A job *running* detached carries the `&`; one stopped out of the foreground never had it,
    /// and one that has finished has stopped being in the background. All three measured against
    /// bash 5, which prints `sleep 1 &` for `Running` and a bare `sleep 1` on the `Done` line.
    #[test]
    fn only_a_running_detached_job_shows_an_ampersand() {
        let detached = Job {
            background: true,
            ..running("sleep 10")
        };
        assert_eq!(
            describe(&detached, '+'),
            "[1]+  Running                    sleep 10 &"
        );

        let finished = Job {
            state: JobState::Completed(0),
            ..detached.clone()
        };
        assert_eq!(
            describe(&finished, '+'),
            "[1]+  Done                       sleep 10"
        );

        let stopped = Job {
            state: JobState::Stopped,
            ..running("vim")
        };
        assert_eq!(
            describe(&stopped, '+'),
            "[1]+  Stopped                    vim"
        );
    }
}
