//! A run counted on Enter, and what happens to it when the command finishes.

use super::db::{Track, capped, lookup_dir};
use super::kv::Tree;
use super::redact;
use super::row::{RunRow, key};

impl Track {
    /// Fold a finished status into a run counted on Enter, without counting it again.
    ///
    /// The provisional row went in with no status and no duration, which is all that is known
    /// while a command is still running. This is the other half of that write: one more
    /// failure when it failed, its status, and the time it took. `runs` and `last_at` stay
    /// where Enter put them — the run happened once, when it was typed.
    pub fn settle_entered(
        &self,
        dir: &str,
        mode: &str,
        argv: &str,
        status: i32,
        duration_ms: i64,
    ) -> bool {
        if !self.writable {
            return false;
        }
        self.store
            .write(|writer| {
                let id = lookup_dir(writer, dir)?;
                let (argv, _) = redact::prepare(argv);
                if argv.is_empty() {
                    return Some(());
                }
                let primary = key::run(id, mode, &argv);
                let mut row = RunRow::decode(&writer.get(Tree::Run, &primary)?)?;
                row.fails += i64::from(status != 0);
                row.last_status = Some(i64::from(status));
                let ms = capped(duration_ms);
                row.total_ms += ms;
                row.max_ms = row.max_ms.max(ms);
                writer.put(Tree::Run, primary, row.encode())
            })
            .is_some()
    }

    /// Take back a run counted on Enter that should not have been counted at all.
    ///
    /// A line that failed to parse, or that a `pre-record` rule refused or replaced, was
    /// provisionally counted before either fact could be known. One run comes back off; a
    /// row that was only ever that run goes away with its index entry.
    pub fn forget_entered(&self, dir: &str, mode: &str, argv: &str) -> bool {
        if !self.writable {
            return false;
        }
        self.store
            .write(|writer| {
                let id = lookup_dir(writer, dir)?;
                let (argv, _) = redact::prepare(argv);
                if argv.is_empty() {
                    return Some(());
                }
                let primary = key::run(id, mode, &argv);
                let Some(mut row) = writer
                    .get(Tree::Run, &primary)
                    .and_then(|bytes| RunRow::decode(&bytes))
                else {
                    return Some(());
                };
                row.runs -= 1;
                if row.runs > 0 {
                    return writer.put(Tree::Run, primary, row.encode());
                }
                writer.delete(Tree::RunByArgv, &key::by_argv(mode, &argv, id));
                writer.delete(Tree::Run, &primary);
                Some(())
            })
            .is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::super::db::fixture::*;
    use super::super::db::{Run, Step, Visit};
    use super::super::kv::Tree;

    /// A run counted on Enter is visible before it finishes, and settling it adds its
    /// outcome without counting it a second time.
    #[test]
    fn an_entered_run_settles_without_counting_twice() {
        let (_dir, track) = store();
        let entered = Step {
            ran_in: Visit::at("/w/alpha"),
            moved_to: None,
            dwell_ms: 0,
            run: Some(Run {
                argv: "sleep 60",
                mode: SH,
                status: None,
                duration_ms: 0,
            }),
        };
        assert!(track.record(&entered));
        assert_eq!(
            track.suggestion_here("/w/alpha", SH, "sleep"),
            Some("sleep 60".to_string()),
            "a command still running is already in the history the suggestion reads"
        );

        assert!(track.settle_entered("/w/alpha", SH, "sleep 60", 3, 2_500));
        let row = run_row(&track, "/w/alpha", SH, "sleep 60").expect("one row");
        assert_eq!(row.runs, 1, "settling did not count the run again");
        assert_eq!(row.fails, 1);
        assert_eq!(row.last_status, Some(3));
        assert_eq!(row.total_ms, 2_500);
    }

    /// Taking back the only run a row ever had removes the row and its index entry.
    #[test]
    fn a_forgotten_entered_run_leaves_nothing_behind() {
        let (_dir, track) = store();
        track.record(&ran("/w/alpha", "cargo build", 0));
        assert!(track.record(&Step {
            ran_in: Visit::at("/w/alpha"),
            moved_to: None,
            dwell_ms: 0,
            run: Some(Run {
                argv: "carg build",
                mode: SH,
                status: None,
                duration_ms: 0,
            }),
        }));
        assert!(track.forget_entered("/w/alpha", SH, "carg build"));
        assert_eq!(rows(&track, Tree::Run), 1, "only the real run is left");
        assert_eq!(rows(&track, Tree::RunByArgv), 1);
        assert_eq!(run_row(&track, "/w/alpha", SH, "carg build"), None);

        // That was its only counted run too, so the row goes with it.
        assert!(track.forget_entered("/w/alpha", SH, "cargo build"));
        assert_eq!(run_row(&track, "/w/alpha", SH, "cargo build"), None);
        assert_eq!(rows(&track, Tree::Run), 0);
    }
}
