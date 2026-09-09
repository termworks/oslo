//! Job control: process groups, the terminal, signal dispositions, and the job table.
//!
//! Split three ways because the three concerns share almost no vocabulary:
//!
//! * `signals` — what the shell ignores and what every forked child gets back (R7.6);
//! * `control` — process groups and who owns the terminal (R7.1);
//! * `table` — the jobs the shell can still name, and reaping the ones that end (R7.2, R7.4);
//! * `report` — how a job is rendered and who is allowed to announce it;
//! * `resume` — `fg` and `bg`, the two ways a job comes back.
//!
//! # The API `jobs`/`fg`/`bg`/`disown`/`wait` are built on
//!
//! Everything a builtin needs is re-exported here:
//!
//! * [`with_jobs`] borrows the process-wide [`JobTable`]. Inside it: [`JobTable::jobs`],
//!   [`JobTable::get`], [`JobTable::lookup`] (`%%`, `%+`, `%-`, `%n`, `%prefix`, `%?substring`),
//!   [`JobTable::marker`], [`JobTable::remove`] (`disown`), [`JobTable::take_status`] (the reaped
//!   status `wait PID` needs) and [`JobTable::live_pids`] (what bare `wait` must wait for).
//! * [`describe`] renders one `jobs` line in bash's column layout.
//! * [`foreground_job`] and [`continue_in_background`] are `fg` and `bg`: they own the terminal
//!   handover so that no builtin has to reimplement R7.1's SIGTTOU discipline.
//! * [`reap_background_jobs`] is called by the evaluator at every command boundary; a builtin
//!   should never need it.

mod control;
mod reap;
mod report;
mod resume;
mod sentinel;
mod signals;
mod table;

pub use control::{
    enable_job_control, init_job_control, job_control_active, leave_job_control, shell_pgid,
};
pub use reap::reap_background_jobs;
pub use report::describe;
pub use resume::{continue_in_background, foreground_job};
pub use signals::{
    drain_interrupt_fd, forget_interrupt, install_shell_signals, interrupt_fd, interrupt_pending,
    interrupt_waiting, note_deliberate_ignore, note_interrupt, reset_signals_for_child,
    restore_shell_signal,
};
pub use table::{Job, JobState, JobTable, with_jobs};

pub use sentinel::started as watcher_started;

pub(crate) use sentinel::{Orders, stand_down, take_events, watch};

pub(crate) use control::{
    give_terminal_to, join_foreground_group, join_group_in_child, left_it_deliberately,
    place_child, place_foreground_child, reclaim_terminal,
};

/// The interactive shell's one-time claim on the terminal and on its own signal policy.
///
/// A handle, not a container. It used to hold `jobs`, `next_id` and an `add_job` nobody called,
/// which is how R7.2's finding — an unreachable [`JobState::Stopped`] — came about: a job outlives
/// the evaluation that started it and is reached by builtins several call frames down, so the
/// table has to be process-global ([`with_jobs`]) rather than owned by whoever happens to hold
/// this. What is left here is the part that genuinely is once-per-process.
#[derive(Debug, Default)]
pub struct JobManager;

impl JobManager {
    pub fn new() -> Self {
        Self
    }

    /// Install the interactive shell's signal policy and claim the terminal.
    ///
    /// The two halves have to happen in this order: SIGTTOU must already be ignored before
    /// [`init_job_control`] calls `tcsetpgrp`, or the very first handover would stop the shell.
    pub fn setup_signals(&mut self) {
        install_shell_signals();
        init_job_control();
    }
}
