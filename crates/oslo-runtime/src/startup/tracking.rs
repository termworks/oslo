//! What the interactive loop already knows, handed to the store instead of dropped on the floor.
//!
//! One turn of [`super::repl`] computes the directory the command started in, the moment it
//! started, the directory it left the shell in, how long it took and what it exited with — all
//! within sixty lines — and then keeps two of them. Nothing here is new telemetry; it is those
//! same locals reaching a table. The whole of it is one call at the end of the loop.
//!
//! # Why not the `postcmd` hook
//!
//! Because that hook fires only on `Ok(status)`, so every command that failed outright would go
//! unrecorded — and failures are exactly what the `fails` column exists to hold. This runs beside
//! the hook, not through it.
//!
//! # Why the store is opened here and nowhere else
//!
//! [`crate::startup`] is the binary. A script, an `oslo -c` or a subshell never reaches this file,
//! so `track::store()` answers `None` for them and there is no file for a CI job's command lines to
//! land in — a structural answer rather than a flag somebody has to remember to check.

use super::history;
use oslo_base::error::ShellError;
use oslo_base::track::{self, Run, Step, Track, Visit};
use std::path::Path;
use std::time::{Duration, SystemTime};

/// Whether this session keeps any record of itself at all.
///
/// `HISTFILE=""` and `HISTSIZE=0` are already documented as the way to run a session that leaves no
/// trace. A user who took that step and then found a new tracking file had appeared beside the
/// history they had just switched off would be right to be angry — so such a session is given no
/// store rather than a quieter one. Read off [`history::Settings`] rather than off `$HISTFILE`
/// again, so the tracker cannot come to a different conclusion from the history about what was
/// asked for.
///
/// **`no_trace`, not `file.is_none()`.** This asked whether a history *file* had been settled on,
/// which was the same question while one had a default. It is not any more: with no default file,
/// reading an absent one as "leave no trace" would take the store away from every shell that had
/// simply never been configured — and with it the finder, `cd` ranking and the model.
fn keeps_a_record(settings: &history::Settings) -> bool {
    !settings.no_trace && settings.max_size > 0
}

/// The two things the loop cannot re-derive after the fact: when the shell arrived where it is
/// standing, and which worktree that is.
pub(super) struct Tracker {
    /// The open dwell segment. It lives here rather than in a row with a null end, because a row
    /// with a null end is precisely the thing that cannot survive a `kill -9`; only closed
    /// segments are ever written.
    ///
    /// Wall clock rather than `Instant`, deliberately: a laptop that slept for nine hours in `~`
    /// really did spend them there, and it is the cap the store applies, not a monotonic clock,
    /// that stops those hours from counting as nine hours of interest.
    since: SystemTime,
    /// The last directory a worktree was walked for. A one-entry cache is the whole of it: a shell
    /// stands still for long stretches, and the directory a command ran in is nearly always the
    /// one the command before it left behind.
    worktree: Option<(String, Option<String>)>,
    /// `oslo.history.immediate`: count a line in the store on Enter rather than on finish.
    immediate: bool,
    /// The line counted on Enter and not yet settled, when [`Self::immediate`] is on.
    entered: Option<entered::Entered>,
}

/// A boundary's answers from the prompt thread, on their way to the thread that writes them.
struct Prepared {
    dwell: i64,
    /// Whether the command left the shell somewhere else, which is what makes it an arrival.
    moved: bool,
    here: Option<String>,
    there: Option<String>,
}

/// A [`Run`] that owns its text.
///
/// `Run` borrows the line and the mode name off the loop's locals, which are gone by the time the
/// writer thread gets to them.
struct Ran {
    argv: String,
    mode: String,
    status: Option<i32>,
    duration_ms: i64,
}

impl Ran {
    fn of(run: Run<'_>) -> Ran {
        Ran {
            argv: run.argv.to_string(),
            mode: run.mode.to_string(),
            status: run.status,
            duration_ms: run.duration_ms,
        }
    }

    fn borrow(&self) -> Run<'_> {
        Run {
            argv: &self.argv,
            mode: &self.mode,
            status: self.status,
            duration_ms: self.duration_ms,
        }
    }
}

/// The boundary itself: one transaction, and the only part that waits on a disk.
fn commit(
    track: &Track,
    prepared: &Prepared,
    before: &str,
    after: &str,
    run: Option<Run<'_>>,
    settled: Option<(u64, &[track::Outcome])>,
) {
    let step = Step {
        ran_in: Visit {
            path: before,
            root: prepared.here.as_deref(),
        },
        moved_to: prepared.moved.then_some(Visit {
            path: after,
            root: prepared.there.as_deref(),
        }),
        dwell_ms: prepared.dwell,
        run,
    };
    match settled {
        Some((history_id, rows)) => track.record_settled(&step, history_id, rows),
        None => track.record(&step),
    };
}

impl Tracker {
    /// Open the store, hand it to the process, and tell it where the shell is standing.
    pub(super) fn start(here: &str, settings: &history::Settings) -> Tracker {
        let mut tracker = Tracker {
            since: SystemTime::now(),
            worktree: None,
            immediate: settings.immediate,
            entered: None,
        };
        if !keeps_a_record(settings) {
            return tracker;
        }
        // The predictor's snapshot, read on a thread of its own — and behind the same gate, which
        // is why it is here rather than in the loop. A model is a distillation of the history, so
        // a session that keeps no history must not read one or write one either; `HISTFILE=""`
        // meaning "no trace" and leaving a file of every command behind would be worse than not
        // offering the switch.
        //
        // **Read, never rebuilt.** Measured in `bench/predict.rs`: the snapshot costs 0.1 ms and
        // stays about 31 KB whatever the history holds, while building the same model from history
        // costs 9.5 ms at ten thousand commands — several times oslo's whole startup, to produce
        // what a file already has. Detached like the sweep below: a prompt drawn before it lands
        // has nothing to predict from, and every command that runs feeds it regardless.
        #[cfg(feature = "vista")]
        if let Some(path) = oslo_base::predict::default_path(
            std::env::var("XDG_DATA_HOME").ok().as_deref(),
            std::env::var("HOME").ok().as_deref(),
        ) {
            oslo_base::predict::warm(path);
        }
        track::install(
            track::default_path(
                std::env::var("XDG_DATA_HOME").ok().as_deref(),
                std::env::var("HOME").ok().as_deref(),
            )
            .and_then(|path| Track::open(&path)),
        );
        // Priming rather than counting a visit: starting a shell somewhere is not the same act as
        // walking there, and it must not raise that directory's rank. What it buys is a `dir_id`
        // for the session's first command, which the visit statement would not have written
        // because the directory never changed.
        if let Some(track) = track::store() {
            let root = tracker.worktree_of(here);
            track.prime(&Visit {
                path: here,
                root: root.as_deref(),
            });
            // On open, and on a thread, exactly as `command_index::warm` starts the `$PATH` scan a
            // few lines earlier in the loop: whatever the shell does between here and the first
            // prompt is time the sweep gets for free, and a sweep the prompt waited on would be a
            // daily stall on the one command a person did not ask for.
            track::prune::sweep_soon(track);
        }
        tracker
    }

    /// Write down one turn of the loop, and — when the caller has one to give — what its line did.
    ///
    /// The outcome rides along because the directory it names is the one this boundary is about to
    /// resolve. Two writes that need the same answer are one write.
    ///
    /// **Prepared here and written elsewhere.** Closing the dwell segment and resolving the
    /// worktrees needs this `Tracker` and the locals the loop is holding; committing needs neither,
    /// and committing is the part that waits on a disk. See [`track::writer`].
    ///
    pub(super) fn boundary(
        &mut self,
        before: &str,
        after: &str,
        run: Option<Run<'_>>,
        settled: Option<(u64, Vec<track::Outcome>)>,
    ) {
        if track::store().is_none() {
            return;
        }
        let prepared = self.prepare(before, after);
        // Owned, because the thread that writes this down outlives every local it came from.
        let (before, after) = (before.to_string(), after.to_string());
        let run = run.map(Ran::of);
        track::writer::defer(move || {
            let Some(track) = track::store() else {
                return;
            };
            let settled = settled.as_ref().map(|(id, rows)| (*id, rows.as_slice()));
            commit(
                track,
                &prepared,
                &before,
                &after,
                run.as_ref().map(Ran::borrow),
                settled,
            );
        });
    }

    /// Record a second line for the same command boundary — a link a `pre-record` rule kept.
    ///
    /// **No movement and no dwell.** Those belong to the boundary, which the first line already
    /// recorded; crediting them again would count one command's seconds twice and make the
    /// directory look busier than it was.
    pub(super) fn also_ran(&mut self, here: &str, run: Option<Run<'_>>) {
        if track::store().is_none() {
            return;
        }
        let root = self.worktree_of(here);
        let here = here.to_string();
        let run = run.map(Ran::of);
        track::writer::defer(move || {
            let Some(track) = track::store() else {
                return;
            };
            track.record(&Step {
                ran_in: Visit {
                    path: &here,
                    root: root.as_deref(),
                },
                moved_to: None,
                dwell_ms: 0,
                run: run.as_ref().map(Ran::borrow),
            });
        });
    }

    /// A line the user asked the shell to forget as they typed it.
    ///
    /// The segment is closed and thrown away, so the time a secret command took is not credited to
    /// the directory it ran in either. The leading space is the one privacy mechanism a user
    /// operates deliberately; half-honouring it — dropping the line but keeping the place and the
    /// minutes — would be worse than not offering it.
    pub(super) fn forget_boundary(&mut self) {
        self.since = SystemTime::now();
        if let Some(entered) = self.entered.take() {
            track::writer::defer(move || {
                if let Some(track) = track::store() {
                    track.forget_entered(&entered.dir, &entered.mode, &entered.argv);
                }
            });
        }
    }

    /// [`Tracker::boundary`] without the thread: prepare, then commit, right here.
    ///
    /// For tests, which need a store in a temporary directory rather than the process-global one —
    /// that can only ever be set once — and need the write to have happened by the time the call
    /// returns rather than shortly afterwards.
    #[cfg(test)]
    fn write(
        &mut self,
        track: &Track,
        before: &str,
        after: &str,
        run: Option<Run<'_>>,
        settled: Option<(u64, &[track::Outcome])>,
    ) {
        let prepared = self.prepare(before, after);
        commit(track, &prepared, before, after, run, settled);
    }

    /// Everything about a boundary that only this `Tracker` can answer.
    ///
    /// The dwell because it closes a segment held here, the worktrees because they are cached here.
    /// Both are cheap and neither touches the store, which is what makes the rest deferrable.
    fn prepare(&mut self, before: &str, after: &str) -> Prepared {
        let dwell = self.close_segment();
        let moved = after != before;
        let here = self.worktree_of(before);
        Prepared {
            dwell,
            moved,
            here,
            there: if moved { self.worktree_of(after) } else { None },
        }
    }

    /// Close the open dwell segment and start the next one.
    ///
    /// A clock that went backwards contributes nothing rather than taking time away from a
    /// directory that had earned it.
    fn close_segment(&mut self) -> i64 {
        let now = SystemTime::now();
        let elapsed = now.duration_since(self.since).unwrap_or(Duration::ZERO);
        self.since = now;
        millis(elapsed)
    }

    /// The worktree `dir` belongs to, walked once per directory rather than once per command.
    fn worktree_of(&mut self, dir: &str) -> Option<String> {
        if let Some((cached, root)) = &self.worktree
            && cached == dir
        {
            return root.clone();
        }
        let root = oslo_ui::prompt::git_root_of(Path::new(dir))
            .map(|root| root.to_string_lossy().into_owned());
        self.worktree = Some((dir.to_string(), root.clone()));
        root
    }
}

/// The command as the store wants it, or `None` when there is nothing worth writing down.
///
/// A line that failed to *parse* is deliberately not a command. A typo is often a password typed
/// into the wrong prompt, and a store built to suggest lines back to you is the last place one
/// should come to rest.
pub(super) fn ran<'a>(
    text: &'a str,
    mode: &'a str,
    result: &Result<i32, ShellError>,
    elapsed: Duration,
) -> Option<Run<'a>> {
    let status = match result {
        Ok(status) => *status,
        Err(ShellError::SyntaxError(_)) => return None,
        // `exit 0` succeeded; every other error is the failure `fails` is counting.
        Err(ShellError::Exit(code)) => *code,
        Err(err) => err.failure_status(),
    };
    Some(Run {
        argv: text,
        mode,
        status: Some(status),
        duration_ms: millis(elapsed),
    })
}

/// A duration in whole milliseconds. Sub-millisecond resolution on a shell command is noise.
fn millis(elapsed: Duration) -> i64 {
    i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
}

/// What a `pre-record` handler decided about a finished line.
#[derive(Clone)]
pub(super) enum Recording {
    /// Nothing was attached, or the handler declined to answer. Record as typed.
    AsTyped,
    /// Record these lines instead. The first is what the log row becomes; every one of them gets a
    /// row in the aggregate.
    These(Vec<String>),
    /// Record nothing at all.
    Refused,
}

/// Ask a `pre-record` rule what to write down for the line that just finished.
///
/// **This runs where the shell's state is free** — `boundary` is called after the guard is dropped
/// — so a handler may use the whole `oslo.*` API, unlike the answering hooks that fire from inside
/// a builtin.
///
/// The handler is told the line, where and how it ran, and its links; it answers with a list of
/// lines to record, `false` for none, or nothing to leave it alone. See the README.
pub(super) fn ask_what_to_record(
    text: &str,
    cwd: &str,
    mode: &str,
    result: &Result<i32, ShellError>,
    elapsed: Duration,
) -> Recording {
    use oslo_base::value::Value;
    let status = match result {
        Ok(status) => *status,
        Err(err) => err.failure_status(),
    };
    let mut fields = vec![
        ("text", Value::str(text)),
        ("cwd", Value::str(cwd)),
        ("mode", Value::str(mode)),
        ("status", Value::int(i64::from(status))),
        ("duration_ms", Value::int(millis(elapsed))),
        ("profile", Value::str(track::profile::current())),
        ("segments", segment_table()),
    ];
    fields.sort_by_key(|(name, _)| *name);
    let answer = crate::lua::engine::answer_hook_with(
        crate::lua::api::hooks::at::PRE_RECORD,
        vec![crate::lua::LuaEngine::hook_fields(&fields)],
    );
    match answer {
        None | Some(Value::Nil) => Recording::AsTyped,
        Some(Value::Bool(false)) => Recording::Refused,
        // A bare string is the one-line shorthand for `{ line }`, which is what anyone who has
        // written a `pre-cmd` handler will reach for first.
        Some(Value::Str(one)) => Recording::These(vec![one.to_string()]),
        Some(Value::Table(list)) => {
            let lines: Vec<String> = list
                .borrow()
                .sequence()
                .iter()
                .filter_map(|value| match value {
                    Value::Str(line) => Some(line.to_string()),
                    _ => None,
                })
                .filter(|line| !line.trim().is_empty())
                .collect();
            // An empty list is not a refusal — `false` is. A handler that built a list and matched
            // nothing meant "no change", and reading it as "forget this line" would lose commands
            // to a rule that simply did not apply.
            if lines.is_empty() {
                Recording::AsTyped
            } else {
                Recording::These(lines)
            }
        }
        // Anything else is a handler mistake rather than an instruction. Leaving the line alone is
        // the only safe reading: the alternative is losing history to a typo.
        Some(_) => Recording::AsTyped,
    }
}

/// The lines a decision says to write down, given what was typed.
pub(super) fn lines_to_record<'a>(decided: &'a Recording, typed: &'a str) -> Vec<&'a str> {
    match decided {
        Recording::AsTyped => vec![typed],
        Recording::These(lines) => lines.iter().map(String::as_str).collect(),
        Recording::Refused => Vec::new(),
    }
}

/// Bring the log row at `history_id` into line with what was decided.
///
/// The row was written *before* the command ran, so by the time a rule has an opinion it is already
/// there — rewritten in place rather than re-appended, keeping the id everything else joins on.
pub(super) fn settle_log_row(history_id: u64, decided: &Recording, typed: &str) {
    let Some(track) = track::store() else {
        return;
    };
    match decided {
        Recording::AsTyped => {}
        Recording::Refused => {
            track.drop_line(history_id);
        }
        Recording::These(lines) => {
            // The first line is what the row becomes; the rest exist only in the aggregate, since
            // they were never separate things anybody typed.
            if let Some(kept) = lines.first()
                && kept != typed
            {
                track.rewrite_line(history_id, kept);
            }
        }
    }
}

/// The links of the line that just ran, as the table a handler walks.
fn segment_table() -> oslo_base::value::Value {
    use oslo_base::value::{Table, Value};
    let mut list = Table::new();
    for (i, link) in oslo_shell::exec::pipeline::segments::taken()
        .iter()
        .enumerate()
    {
        let mut row = Table::new();
        row.set_str("text", Value::str(&link.text));
        row.set_str("op", Value::str(link.join.written()));
        row.set_str("ran", Value::Bool(link.ran()));
        row.set_str("ms", Value::int(link.duration_ms));
        // Absent rather than a number when the link never ran: any number here would be read as a
        // status, and "did not run" is neither success nor failure.
        if let Some(status) = link.status {
            row.set_str("status", Value::int(i64::from(status)));
        }
        list.set(Value::int(i as i64 + 1), Value::table(row));
    }
    Value::table(list)
}

/// Write what the line logged as `history_id` did, links and all.
///
/// The log row was written *before* the command ran — that is what keeps a long command visible to
/// another terminal while it is still going — so the directory, the status and the duration can
/// only be recorded here, against the id it went in under.
///
/// The links come from `exec::pipeline::segments`, which the read loop armed for this line. A line
/// that was not a chain records one row for itself and no links, which is the common case and
/// costs one small write.
pub(super) fn outcome_rows(
    result: &Result<i32, ShellError>,
    elapsed: Duration,
) -> Vec<track::Outcome> {
    let status = outcome_status(result);
    // The predictor held this line when the log wrote it, because a command's status does not
    // exist until here. This is what lets it learn that a failure was followed by a retyping,
    // which is the whole of what repair is built on.
    //
    // **On this thread, because its other half is.** `predict::record` runs inside `append`, which
    // is on the prompt thread; the two are a strict pair through one held slot. Queue one of them
    // and the model learns a line's status against the line before it.
    #[cfg(feature = "vista")]
    oslo_base::predict::settle(status);
    // `0` for now: the directory is what the boundary resolves, and the boundary is what writes
    // these. The store fills segment zero in whichever way the rows reach it.
    let mut rows = vec![track::Outcome::line(0, status, millis(elapsed))];
    // Only when it *was* a chain. One link is the line itself, already in row zero.
    let links = oslo_shell::exec::pipeline::segments::taken();
    if links.len() > 1 {
        rows.extend(links.iter().map(|link| track::Outcome {
            segment: link.index as u32 + 1,
            join: link.join.written().to_string(),
            text: link.text.clone(),
            status: link.status,
            duration_ms: link.duration_ms,
            dir_id: 0,
        }));
    }
    rows
}

/// The outcome on its own, for the lines whose boundary could not carry it.
pub(super) fn record_outcome(history_id: u64, rows: &[track::Outcome]) {
    let Some(track) = track::store() else {
        return;
    };
    track.record_outcome_here(history_id, rows);
}

/// What the line reported, in the shape the outcome row and the model both take.
fn outcome_status(result: &Result<i32, ShellError>) -> Option<i32> {
    match result {
        Ok(status) => Some(*status),
        // A line that never reached execution has no status, and saying so is the point: `None`
        // here means the same as it does on a link that was short-circuited past.
        Err(ShellError::SyntaxError(_)) => None,
        Err(ShellError::Exit(code)) => Some(*code),
        Err(err) => Some(err.failure_status()),
    }
}

#[path = "tracking/entered.rs"]
mod entered;

#[path = "tracking/finished.rs"]
mod finished;
pub(in crate::startup) use finished::Finished;

#[cfg(test)]
#[path = "tracking/tests.rs"]
mod tests;
