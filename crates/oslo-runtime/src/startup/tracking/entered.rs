//! Counting a line in the store on Enter, before it runs.

use super::*;

/// A run counted on Enter, waiting for its outcome.
pub(super) struct Entered {
    pub(super) dir: String,
    pub(super) mode: String,
    pub(super) argv: String,
}

impl Tracker {
    /// Count a line in the store now that Enter has been pressed, before it runs.
    ///
    /// Synchronous, like the log append beside it: this is the write that has to survive a
    /// shell killed under a long command, and a queued write would die with it. Status and
    /// duration do not exist yet, so the row goes in with neither; `settle_entered` folds them
    /// in at the boundary without counting the run again, and `forget_entered` takes the run
    /// back when the finish learns it should never have been counted — a parse failure, or a
    /// `pre-record` rule refusing or replacing the line.
    pub(in crate::startup) fn entered(&mut self, before: &str, text: &str, mode: &str) {
        if !self.immediate {
            return;
        }
        let Some(track) = track::store() else {
            return;
        };
        let root = self.worktree_of(before);
        if track.record(&Step {
            ran_in: Visit {
                path: before,
                root: root.as_deref(),
            },
            moved_to: None,
            dwell_ms: 0,
            run: Some(Run {
                argv: text,
                mode,
                status: None,
                duration_ms: 0,
            }),
        }) {
            self.entered = Some(Entered {
                dir: before.to_string(),
                mode: mode.to_string(),
                argv: text.to_string(),
            });
        }
    }
}
