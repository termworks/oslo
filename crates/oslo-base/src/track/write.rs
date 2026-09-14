//! One transaction, once per prompt.
//!
//! The REPL loop already computes every input this needs within sixty lines of each other and then
//! throws all but two of them away. So the whole write path is one call at the end of the loop
//! taking a struct built from locals that already exist; nothing is threaded through anything, and
//! nothing here runs on a worker thread. Each prompt commits its tracking changes synchronously.
//!
//! # Contract item 3: the upsert is a read, an add and a put
//!
//! `INSERT ... ON CONFLICT DO UPDATE SET runs = runs + 1` was one statement and it is three lines
//! of Rust now: `RunRow::absorb` does the arithmetic, and the read and the write
//! happen inside one [`super::kv::Store::write`], which is one transaction.
//!
//! That is the whole of the atomicity argument and it is worth stating, because the SQL comment it
//! replaces said the opposite. Under SQLite the increment had to be written *in SQL* — `dwell_ms =
//! dwell_ms + ?` rather than a read-then-write in Rust — precisely so that two shells in one
//! directory would add up instead of clobbering each other. Here a read-then-write is safe for a
//! reason SQLite could not offer: Tagdata permits one writer transaction at a time, so no other
//! terminal can write the row between this read and this write.
//!
//! A crash between two of these writes cannot leave a run attributed to a directory whose arrival
//! was never recorded: Tagdata publishes no transaction until `commit`, so an unfinished
//! transaction writes nothing.
//!
//! # One open interval, and it is not in the store
//!
//! Time in a directory accumulates from *closed* segments only. A row with a null end is the thing
//! that cannot survive a kill, so there isn't one: the open segment is a clock mark held in the
//! shell process, and the store is only ever told about time that has already elapsed.
//!
//! Note the unit honestly: this is shell-milliseconds, not wall-clock. Two shells sitting in one
//! directory for an hour record two hours. That is right for a ranking signal and wrong for a
//! report, and deduplicating it would need a session table and an interval-overlap computation on
//! read.
//!
//! # Indexes are maintained by hand, here and in [`super::prune`]
//!
//! There are no triggers and no foreign keys. Every write that creates a row creates its index
//! entries in the same transaction, and every delete removes them — see `db::put_dir` for
//! the directory side and `record_run` for `run_argv`. An index this file forgets is not an error
//! anywhere; it is a suggestion that quietly stops appearing.

use super::db::{Run, Step, Track, Visit, capped, lookup_dir, now, put_dir, read_dir, resolve_dir};
use super::kv::{Tree, Writer};
use super::outcome::{Outcome, settled, write_outcomes};
use super::redact;
use super::row::{DirRow, RunRow, key};

impl Track {
    /// Tell the store where the shell is standing, without counting it as a visit.
    ///
    /// Called once at startup with `$PWD`. Starting a shell somewhere is not the same act as
    /// walking there, so it must not raise that directory's rank — but the first command of the
    /// session still needs a directory to be attributed to, and the visit only happens when the
    /// directory *changes*.
    ///
    /// A directory the store already knows is resolved by a read, so the overwhelmingly common
    /// case of starting a shell somewhere familiar costs no `fsync` at all. Only the first time
    /// anybody has ever run a command in a directory does this write.
    pub fn prime(&self, at: &Visit<'_>) -> bool {
        if !self.writable {
            self.forget_current();
            return false;
        }
        if let Some(id) = self.store.read(|reader| lookup_dir(reader, at.path)) {
            self.remember_current(id, at.path);
            return true;
        }
        match self.store.write(|writer| resolve_dir(writer, at)) {
            Some(id) => {
                self.remember_current(id, at.path);
                true
            }
            None => false,
        }
    }

    /// Write down one turn of the loop.
    pub fn record(&self, step: &Step<'_>) -> bool {
        if !self.writable {
            return false;
        }
        let moved_to = step.moved_to.filter(|to| to.path != step.ran_in.path);

        let cached = self.cached_id(step.ran_in.path);
        // The lock is deliberately not held across the transaction: where the shell ended up comes
        // back out of the closure and is stored afterwards.
        let Some(next) = self
            .store
            .write(|writer| write_step(writer, step, cached, moved_to))
        else {
            return false;
        };
        match next {
            Some((id, path)) => self.remember_current(id, &path),
            None => self.forget_current(),
        }
        true
    }

    /// Write down one turn of the loop *and* what its line did, in a single transaction.
    ///
    /// The two were always one write pretending to be two: the outcome's only unknown is the
    /// directory, and the directory is exactly what the boundary has just resolved — so the second
    /// transaction existed to read back what the first already had in hand. Merging them takes a
    /// typed command from three commits to two, and a commit costs a pair of `fsync`s on the thread
    /// the next prompt is waiting on.
    ///
    /// A step this refuses to record is still a line with an outcome, so the halves fall back
    /// separately rather than the outcome being lost with the step.
    pub fn record_settled(&self, step: &Step<'_>, history_id: u64, rows: &[Outcome]) -> bool {
        if rows.is_empty() {
            return self.record(step);
        }
        if !self.writable {
            return false;
        }
        let moved_to = step.moved_to.filter(|to| to.path != step.ran_in.path);

        let cached = self.cached_id(step.ran_in.path);
        let Some(next) = self.store.write(|writer| {
            let next = write_step(writer, step, cached, moved_to)?;
            let here = next.as_ref().map_or(0, |(id, _)| *id);
            write_outcomes(writer, history_id, &settled(rows, here))?;
            Some(next)
        }) else {
            // The shared transaction is all-or-nothing, and a boundary that would not write is no
            // reason to throw the outcome away with it — that is what the second write bought when
            // there was one. It is bought back here, on the path nothing takes twice a second.
            return self.record_outcome_here(history_id, rows);
        };
        match next {
            Some((id, path)) => self.remember_current(id, &path),
            None => self.forget_current(),
        }
        true
    }

    /// Forget every command line, and keep every directory.
    ///
    /// `history -c` means "forget the history", and a store that went on suggesting the lines it
    /// had just been told to forget would be lying in exactly the way the recall set would. But
    /// "forget my command lines" is not "forget where I work", so the directories are left standing
    /// — with them go the visit counts a `cd` ranks on, which nobody asked to lose.
    ///
    /// Both buckets, or the secondary index would go on naming rows that are not there.
    pub fn forget_runs(&self) -> bool {
        if !self.writable {
            return false;
        }
        self.store
            .write(|writer| {
                writer.clear(Tree::Run);
                writer.clear(Tree::RunByArgv);
                Some(())
            })
            .is_some()
    }

    /// The cached id, but only if it is still the directory being asked about.
    fn cached_id(&self, path: &str) -> Option<u64> {
        self.current
            .lock()
            .ok()
            .and_then(|current| match current.as_ref() {
                Some((id, cached)) if cached == path => Some(*id),
                _ => None,
            })
    }

    /// The id of the directory the store last wrote a step for, or `0`.
    ///
    /// For the outcome row, which is written straight after `record` and wants the same directory
    /// without paying for a second lookup — `record` has just cached it. `0` means "not known",
    /// which is what a shell whose store would not open has anyway.
    pub fn current_dir_id(&self) -> u64 {
        self.current
            .lock()
            .ok()
            .and_then(|current| current.as_ref().map(|(id, _)| *id))
            .unwrap_or(0)
    }

    fn remember_current(&self, id: u64, path: &str) {
        if let Ok(mut current) = self.current.lock() {
            *current = Some((id, path.to_string()));
        }
    }

    fn forget_current(&self) {
        if let Ok(mut current) = self.current.lock() {
            *current = None;
        }
    }
}

/// The whole of one boundary. Answers where the shell now is, or `None` if anything failed — and a
/// `None` discards the transaction, so a half-written boundary is never a boundary.
fn write_step(
    writer: &Writer<'_, '_>,
    step: &Step<'_>,
    cached: Option<u64>,
    moved_to: Option<Visit<'_>>,
) -> Option<Option<(u64, String)>> {
    let at = now();
    // The cached id is checked against the store rather than trusted. It is one point lookup on
    // eight fixed bytes, and it is what makes a directory another terminal's prune sweep dropped
    // between two commands cost a re-resolve instead of a run row filed under an id that no longer
    // names anything.
    let id = match cached.filter(|id| writer.has(Tree::Dir, &key::dir(*id))) {
        Some(id) => id,
        None => resolve_dir(writer, &step.ran_in)?,
    };

    let dwell = capped(step.dwell_ms);
    if dwell > 0 {
        add_dwell(writer, id, dwell)?;
    }
    if let Some(run) = step.run {
        record_run(writer, id, &run, at)?;
    }

    let Some(to) = moved_to else {
        return Some(Some((id, step.ran_in.path.to_string())));
    };
    Some(Some((arrive(writer, &to, at)?, to.path.to_string())))
}

/// Count one arrival, or record the first — the `VISIT` upsert, in Rust.
///
/// `root` is taken from the arrival every time rather than only on the insert, which is what makes
/// a directory that has just become a repository, or has moved to another one, correct in the
/// worktree index. `missing_since` is cleared for the same reason it was in SQL: a disk that came
/// back is not a directory that was deleted, and walking into it is the proof.
fn arrive(writer: &Writer<'_, '_>, to: &Visit<'_>, at: i64) -> Option<u64> {
    let Some(id) = lookup_dir(writer, to.path) else {
        let mut row = DirRow::unvisited(to.path, to.base(), to.root);
        row.visits = 1;
        row.last_visit = at;
        return super::db::insert_dir(writer, &row);
    };
    // A row that will not decode is treated as one that is not there, and rewritten under the same
    // id. Keeping the id is what stops the index rows it is named by from being orphaned.
    let was = read_dir(writer, id);
    let mut row = was
        .clone()
        .unwrap_or_else(|| DirRow::unvisited(to.path, to.base(), to.root));
    row.visits += 1;
    row.last_visit = at;
    row.root = to.root.map(str::to_string);
    row.missing_since = None;
    put_dir(writer, id, was.as_ref(), &row)?;
    Some(id)
}

/// Close the segment that just ended.
///
/// A directory that has gone missing while the shell stood in it is not created again by this: it
/// is time spent somewhere the store has forgotten, which is worth nothing and is not an error.
fn add_dwell(writer: &Writer<'_, '_>, id: u64, ms: i64) -> Option<()> {
    let Some(was) = read_dir(writer, id) else {
        return Some(());
    };
    let mut row = was.clone();
    row.dwell_ms += ms;
    put_dir(writer, id, Some(&was), &row)
}

/// Fold one execution into the row for that line in that directory.
///
/// The row is written under `(dir_id, mode, argv)`; the first time a line is seen it also gets a
/// `run_argv` entry under `(mode, argv, dir_id)`, and after that it does not, because the key has
/// not changed and a `put` of the same bytes would be a page written for nothing.
fn record_run(writer: &Writer<'_, '_>, dir: u64, run: &Run<'_>, at: i64) -> Option<()> {
    let (argv, head) = redact::prepare(run.argv);
    if argv.is_empty() {
        // Not a failure: a line with no command in it has nothing to remember.
        return Some(());
    }
    let fresh = RunRow::first(head, run.status.map(i64::from), at, capped(run.duration_ms));
    let primary = key::run(dir, run.mode, &argv);
    let known = writer
        .get(Tree::Run, &primary)
        .and_then(|bytes| RunRow::decode(&bytes));
    match known {
        Some(mut row) => {
            row.absorb(&fresh);
            writer.put(Tree::Run, primary, row.encode())?;
        }
        None => {
            writer.put(Tree::Run, primary, fresh.encode())?;
            writer.put(
                Tree::RunByArgv,
                key::by_argv(run.mode, &argv, dir),
                Vec::new(),
            )?;
        }
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::super::db::CAP_MS;
    use super::super::db::fixture::*;
    use super::*;

    /// Somewhere the shell has been twice is one row that says two, not two rows.
    #[test]
    fn a_second_visit_counts_rather_than_duplicates() {
        let (_dir, track) = store();
        let alpha = Visit::at("/w/alpha");
        let beta = Visit::at("/w/beta");

        assert!(track.prime(&alpha));
        assert_eq!(
            visits_of(&track, "/w/alpha"),
            0,
            "starting a shell somewhere is not walking there"
        );

        for _ in 0..2 {
            assert!(track.record(&Step {
                ran_in: alpha,
                moved_to: Some(beta),
                dwell_ms: 0,
                run: None,
            }));
            assert!(track.record(&Step {
                ran_in: beta,
                moved_to: Some(alpha),
                dwell_ms: 0,
                run: None,
            }));
        }

        assert_eq!(rows(&track, Tree::Dir), 2, "two directories, four arrivals");
        assert_eq!(visits_of(&track, "/w/beta"), 2);
        assert_eq!(visits_of(&track, "/w/alpha"), 2);
        assert!(
            dir_row(&track, "/w/beta").expect("a row").last_visit > 0,
            "and each arrival is stamped"
        );
    }

    /// Going nowhere is not an arrival, however many commands are run standing still.
    #[test]
    fn staying_put_is_not_a_visit() {
        let (_dir, track) = store();
        for _ in 0..3 {
            track.record(&Step {
                ran_in: Visit::at("/w/alpha"),
                moved_to: Some(Visit::at("/w/alpha")),
                dwell_ms: 0,
                run: None,
            });
        }
        assert_eq!(visits_of(&track, "/w/alpha"), 0);
    }

    /// Time is added from closed segments, and one contribution can never be a suspended laptop's
    /// worth. The addition is a read and a write in Rust now rather than an increment in SQL, and
    /// it is still safe between terminals because it happens inside one transaction.
    #[test]
    fn dwell_accumulates_and_is_capped() {
        let (_dir, track) = store();
        let here = Visit::at("/w/alpha");
        for dwell in [1_000, 2_500, 9 * 60 * 60 * 1000] {
            assert!(track.record(&Step {
                ran_in: here,
                moved_to: None,
                dwell_ms: dwell,
                run: None,
            }));
        }
        assert_eq!(
            dir_row(&track, "/w/alpha").expect("a row").dwell_ms,
            1_000 + 2_500 + CAP_MS,
            "nine hours of suspend counts as fifteen minutes"
        );
    }

    /// Two stores against one file are two terminals, and the increment neither may lose.
    #[test]
    fn two_terminals_in_one_directory_add_up_rather_than_clobber_each_other() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("track.kv");
        let first = Track::open(&path).expect("the first opens");
        let second = Track::open(&path).expect("the second opens too");

        for shell in [&first, &second] {
            for _ in 0..5 {
                assert!(shell.record(&Step {
                    ran_in: Visit::at("/w/alpha"),
                    moved_to: None,
                    dwell_ms: 1_000,
                    run: Some(Run {
                        argv: "cargo build",
                        mode: SH,
                        status: Some(0),
                        duration_ms: 3,
                    }),
                }));
            }
        }

        let row = run_row(&first, "/w/alpha", SH, "cargo build").expect("one row");
        assert_eq!(row.runs, 10, "ten runs, and one row to hold them");
        assert_eq!(rows(&first, Tree::Run), 1);
        assert_eq!(
            dir_row(&second, "/w/alpha").expect("a row").dwell_ms,
            10_000,
            "and neither terminal's time was overwritten by the other's"
        );
    }

    /// The aggregate: repeats are the entire point and cost nothing after the first.
    #[test]
    fn a_repeated_command_aggregates_into_one_row() {
        let (_dir, track) = store();
        track.record(&ran("/w/alpha", "cargo build", 0));
        track.record(&ran("/w/alpha", "cargo build", 1));
        track.record(&ran("/w/alpha", "cargo build", 0));

        assert_eq!(rows(&track, Tree::Run), 1);
        let row = run_row(&track, "/w/alpha", SH, "cargo build").expect("one row");
        assert_eq!(row.runs, 3);
        assert_eq!(row.fails, 1);
        assert_eq!(row.last_status, Some(0), "the newest status wins");
        assert_eq!(row.total_ms, 15);
        assert_eq!(row.max_ms, 5);
        assert_eq!(
            row.head, "cargo build",
            "the tool and what it was doing, not the whole line"
        );
        assert_eq!(
            rows(&track, Tree::RunByArgv),
            1,
            "and the secondary index gained one entry, not three"
        );
    }

    /// A command whose exit was never seen is not a command that succeeded.
    #[test]
    fn an_unobserved_exit_is_stored_as_unknown() {
        let (_dir, track) = store();
        track.record(&Step {
            ran_in: Visit::at("/w/alpha"),
            moved_to: None,
            dwell_ms: 0,
            run: Some(Run {
                argv: "sleep 100",
                mode: SH,
                status: None,
                duration_ms: 1,
            }),
        });
        let row = run_row(&track, "/w/alpha", SH, "sleep 100").expect("one row");
        assert_eq!(row.last_status, None);
        assert_eq!(row.fails, 0);
    }

    /// A secret in the arguments costs the arguments. The directory, the count and the timing all
    /// survive, which is what makes an imperfect filter acceptable.
    #[test]
    fn a_redacted_line_still_leaves_its_timing_behind() {
        let (_dir, track) = store();
        track.record(&ran("/w/alpha", "curl --token abcdef https://x", 0));

        assert_eq!(lines_in(&track, "/w/alpha"), vec!["curl".to_string()]);
        assert_eq!(
            run_row(&track, "/w/alpha", SH, "curl")
                .expect("one row")
                .total_ms,
            5
        );
        assert!(dir_row(&track, "/w/alpha").is_some());
    }

    /// A place kept out of `cd` is still a place things ran: what ran in `/tmp` or a
    /// `node_modules` is in the finder, and only the jump never offers the directory.
    #[test]
    fn an_excluded_directory_is_recorded_but_never_a_jump_target() {
        let (_dir, track) = store();
        assert!(track.record(&ran("/w/p/node_modules/react", "npm test", 0)));
        assert!(track.record(&ran("/tmp", "wing template apply python ./x", 0)));
        assert!(track.record(&ran("/w/p", "ls", 0)));
        assert_eq!(rows(&track, Tree::Run), 3, "every command is remembered");

        let listed: Vec<String> = track.commands(10).into_iter().map(|c| c.line).collect();
        assert!(listed.iter().any(|line| line == "npm test"), "{listed:?}");
        assert!(
            listed.iter().any(|line| line.starts_with("wing")),
            "{listed:?}"
        );

        let targets: Vec<String> = track
            .directories_ranked("", 10)
            .into_iter()
            .map(|c| c.path)
            .collect();
        assert!(targets.iter().any(|path| path == "/w/p"), "{targets:?}");
        assert!(
            !targets
                .iter()
                .any(|path| path == "/tmp" || path.contains("node_modules")),
            "{targets:?}"
        );
    }

    /// What `history -c` costs and what it does not: the lines go, the places stay — and the index
    /// that names the lines goes with them.
    #[test]
    fn forgetting_the_lines_keeps_the_directories() {
        let (_dir, track) = store();
        track.record(&Step {
            ran_in: Visit::at("/w"),
            moved_to: Some(Visit::at("/w/alpha")),
            dwell_ms: 0,
            run: None,
        });
        track.record(&ran("/w/alpha", "cargo build", 0));
        assert!(track.forget_runs());

        assert_eq!(track.suggestion_here("/w/alpha", SH, "cargo"), None);
        assert_eq!(rows(&track, Tree::Run), 0);
        assert_eq!(
            rows(&track, Tree::RunByArgv),
            0,
            "or the secondary index would go on naming rows that are not there"
        );
        assert_eq!(
            visits_of(&track, "/w/alpha"),
            1,
            "where you work is not one of the lines you asked to forget"
        );
    }

    /// The bug the naive version has: the run needs a directory whose arrival was never recorded,
    /// because the shell started there.
    #[test]
    fn the_first_command_of_a_session_has_somewhere_to_be_attributed_to() {
        let (_dir, track) = store();
        assert!(track.record(&ran("/w/alpha", "cargo build", 0)));
        assert_eq!(rows(&track, Tree::Run), 1);
        assert_eq!(visits_of(&track, "/w/alpha"), 0, "resolved, not visited");
    }

    /// A cached directory that has since been forgotten must not become a run row filed under an
    /// id that names nothing — the shape a store with no foreign keys fails in silently.
    #[test]
    fn a_directory_forgotten_under_the_cache_is_resolved_again_rather_than_trusted() {
        let (_dir, track) = store();
        assert!(track.prime(&Visit::at("/w/alpha")));

        // Whatever the cache says, the row is gone: another terminal's sweep took it.
        track
            .store
            .write(|writer| {
                writer.delete(Tree::Dir, &key::dir(1));
                writer.delete(Tree::DirByPath, &key::by_path("/w/alpha"));
                Some(())
            })
            .expect("the row is dropped");

        assert!(track.record(&ran("/w/alpha", "cargo build", 0)));
        let id = track
            .store
            .read(|reader| lookup_dir(reader, "/w/alpha"))
            .expect("resolved again");
        assert_eq!(
            track
                .store
                .read(|reader| Some(reader.has(Tree::Run, &key::run(id, SH, "cargo build")))),
            Some(true),
            "the run is filed under the directory that exists, not the one that did"
        );
    }
}
