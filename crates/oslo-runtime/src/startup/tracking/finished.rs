//! Everything that gets written down once a command has finished.
//!
//! One place, because the four writes a command produces are one decision taken four ways: a
//! `pre-record` rule is asked once, and its answer decides what the aggregate learns, what the log
//! row ends up saying, and whether the outcome can ride along with the boundary or has to follow it.
//! Split across the loop, those would be four call sites each re-deriving the same answer.

use super::*;

/// One finished command, as the loop already knows it.
pub(in crate::startup) struct Finished<'a> {
    pub text: &'a str,
    /// Where it started, which is what it is attributed to.
    pub before: &'a str,
    /// Where it left the shell, which is what makes it an arrival.
    pub after: &'a str,
    pub mode: &'a str,
    pub result: &'a Result<i32, ShellError>,
    pub elapsed: Duration,
    /// The log row it went in under, if it was logged at all.
    pub logged_as: Option<u64>,
}

impl Tracker {
    /// Record a finished command: the boundary, the aggregate, and what the line did.
    ///
    /// Beside the `postcmd` hook rather than through it — see this module's parent. Every argument
    /// is a local the loop already had and used to drop.
    pub(in crate::startup) fn write_down(&mut self, done: &Finished<'_>) {
        // Asked once, before anything is written down. A rule may keep one link of a chain instead
        // of the whole line, or refuse it.
        let decided =
            ask_what_to_record(done.text, done.before, done.mode, done.result, done.elapsed);
        let lines = lines_to_record(&decided, done.text);
        // What the line did, joined to the log row it went in under.
        //
        // **Handed to the boundary rather than written after it**, because the boundary is what
        // resolves the directory these rows name: two writes asking the same question are one
        // write. Only when nothing is going to rewrite the log row underneath them — a rule that
        // refuses or replaces the line does that *after* the loop, and then the outcome has to wait
        // its turn as it always did.
        let rows = done
            .logged_as
            .filter(|_| !lines.is_empty())
            .map(|id| (id, outcome_rows(done.result, done.elapsed)));
        let rides_along = rows.is_some() && matches!(decided, Recording::AsTyped);

        // The run counted on Enter, if any. It is settled rather than recorded when the finish
        // is about the same line, and taken back when it is not — a rule replacing or refusing
        // the line, or a line that never ran at all.
        let mut pending = self.entered.take();
        for (first, line) in lines.iter().enumerate().map(|(i, l)| (i == 0, l)) {
            let run = ran(line, done.mode, done.result, done.elapsed);
            // Only the first arrival records the *movement* and the dwell; a second one would
            // credit this directory twice for the same command.
            if first {
                let carried = rows
                    .as_ref()
                    .filter(|_| rides_along)
                    .map(|(id, rows)| (*id, rows.clone()));
                match pending.take() {
                    Some(entered)
                        if rides_along
                            && entered.dir == done.before
                            && entered.mode == done.mode
                            && entered.argv == *line
                            && run.as_ref().and_then(|run| run.status).is_some() =>
                    {
                        let run = run.expect("checked above");
                        let (status, duration_ms) =
                            (run.status.expect("checked above"), run.duration_ms);
                        self.boundary(done.before, done.after, None, carried);
                        track::writer::defer(move || {
                            if let Some(track) = track::store() {
                                track.settle_entered(
                                    &entered.dir,
                                    &entered.mode,
                                    &entered.argv,
                                    status,
                                    duration_ms,
                                );
                            }
                        });
                    }
                    Some(entered) => {
                        track::writer::defer(move || {
                            if let Some(track) = track::store() {
                                track.forget_entered(&entered.dir, &entered.mode, &entered.argv);
                            }
                        });
                        self.boundary(done.before, done.after, run, carried);
                    }
                    None => self.boundary(done.before, done.after, run, carried),
                }
            } else {
                self.also_ran(done.before, run);
            }
        }
        if lines.is_empty() {
            self.forget_boundary();
        }
        if let Some(entered) = pending.take() {
            track::writer::defer(move || {
                if let Some(track) = track::store() {
                    track.forget_entered(&entered.dir, &entered.mode, &entered.argv);
                }
            });
        }

        // Behind the boundary in the same queue, so they still see the directory it resolved and
        // still land after the log row they rewrite.
        if let Some(id) = done.logged_as {
            let (decided, text) = (decided.clone(), done.text.to_string());
            track::writer::defer(move || settle_log_row(id, &decided, &text));
        }
        if let Some((id, rows)) = rows.filter(|_| !rides_along) {
            track::writer::defer(move || record_outcome(id, &rows));
        }
    }
}
