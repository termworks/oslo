//! The reads the write path pays for.
//!
//! Four questions, and every one of them is a **seek to a lower bound and a walk to an upper one**.
//! That is the single most performance-relevant decision in `src/track/`, and it survives the move
//! off SQL unchanged: `argv LIKE 'cargo run --ex%'` and `for row in bucket { if starts_with(..) }`
//! are the same mistake in two languages and both are O(rows in the bucket). Indexed range reads
//! keep the per-keystroke path independent of the total bucket size.
//!
//! # Contract item 1: what did I run *here*
//!
//! A range on `run(dir_id, mode, argv)`, which is one composite key with the typed text as its
//! final, unterminated field. The directory is resolved through `dir_path` first — one point
//! lookup — so the range is pinned to the handful of lines run in one directory in one language.
//!
//! # Contract item 2: what did I run anywhere in this repository
//!
//! Two ranges and a join, exactly as the SQL did it. `dir_root` gives every directory of the
//! worktree — a prefix scan whose keys arrive in id order, so the result is a sorted array and the
//! membership test is a binary search rather than a hash. `run_argv`, the secondary index on
//! `(mode, argv, dir_id)`, then drives the range over the line, and the worktree filter is applied
//! to whatever it produces.
//!
//! The alternative is one narrow range per directory of the worktree, which reads strictly fewer
//! rows. It is not taken because the count of directories under one root is unbounded — a monorepo
//! somebody has worked all over is hundreds of seeks — where the secondary range is always one.
//! The cost profile is therefore the one `ui::recall::nearby` measured and built its memo
//! against, and that memo stays correct.
//!
//! # Contract item 9: there is one ranking, and it is in Rust
//!
//! `score::score_sql` is gone. It existed because the ordering happened inside the database and
//! had to be kept in step with the ordering that happened in Rust; a store with no query language
//! cannot express an ordering at all, so the drift it guarded against is now impossible rather
//! than merely tested for. Every ordering in this file goes through [`score`], which is the same
//! function `cd`'s ranker calls.
//!
//! # No read happens inside a live cursor
//!
//! Where a scan needs a row from another bucket — the directory behind an index entry, the
//! counters behind a `run_argv` key — the scan collects keys first and the rows are read after it
//! returns. The lists are small (the directories of one worktree, the rows of one prefix), and
//! keeping a cursor and a second bucket lookup out of each other's way is worth more than the
//! `Vec`.

use super::db::{Track, lookup_dir, now, read_dir};
use super::kv::{Reader, Span, Tree, Walk};
use super::row::{DirRow, RunRow, field, key, span};
use super::score::{Candidate, score};
use std::cmp::Ordering;

impl Track {
    /// Remembered directories whose final component starts with `needle`, best first.
    ///
    /// This is both indexed tiers at once — naming a directory exactly is also naming its own
    /// prefix — as one range over `dir_base`. Which of the two a row actually earned is decided in
    /// [`super::score`] against the path, so that the tier and the ordering are read off the same
    /// text in the same place.
    ///
    /// `exclude` is where the shell is standing, and it is dropped here rather than by the caller
    /// afterwards. That is the one thing zoxide gets structurally wrong: it excludes `$PWD` from
    /// its *shell function*, by string equality against `pwd`, and so silently fails whenever
    /// `pwd -L` and `pwd -P` disagree.
    pub fn directories_named(&self, needle: &str, exclude: &str, limit: usize) -> Vec<Candidate> {
        let needle = needle.to_lowercase();
        // An empty needle names every directory, which is not what a `cd` with nothing typed
        // means. The SQL answered the same way, by way of an upper bound it could not compute.
        if needle.is_empty() {
            return Vec::new();
        }
        let home = self.not_a_target();
        let mut found = self
            .store
            .read(|reader| {
                let ids = reader.collect(Tree::DirByBase, &span::bases_like(&needle), |key, _| {
                    field::trailing_id(key)
                });
                Some(
                    ids.into_iter()
                        .filter_map(|id| candidate(read_dir(reader, id)?, exclude, &home))
                        .collect::<Vec<Candidate>>(),
                )
            })
            .unwrap_or_default();
        best_first(&mut found, limit);
        found
    }

    /// The best-scoring remembered directories, for the tiers no index can serve.
    ///
    /// zoxide does this scan on every query; here it is only reached when naming the directory
    /// found nothing, and only for a `cd` a person typed — once per press of Enter, never per
    /// keystroke.
    pub fn directories_ranked(&self, exclude: &str, limit: usize) -> Vec<Candidate> {
        let home = self.not_a_target();
        let mut found = self
            .store
            .read(|reader| {
                Some(reader.collect(Tree::Dir, &Span::all(), |_, value| {
                    candidate(DirRow::decode(value)?, exclude, &home)
                }))
            })
            .unwrap_or_default();
        best_first(&mut found, limit);
        found
    }

    /// The line to suggest for `typed` in `dir`, or `None`. Contract item 1.
    pub fn suggestion_here(&self, dir: &str, mode: &str, typed: &str) -> Option<String> {
        if typed.is_empty() {
            return None;
        }
        self.store.read(|reader| {
            let id = lookup_dir(reader, dir)?;
            let mut best = Best::default();
            reader.scan(
                Tree::Run,
                &span::runs_like(id, mode, typed),
                |key, value| {
                    if let Some(argv) = field::argv_of_run(key)
                        && argv != typed
                        && let Some(row) = RunRow::decode(value)
                        && row.worked()
                    {
                        best.offer(&row, &argv);
                    }
                    Walk::On
                },
            );
            best.line
        })
    }

    /// The same question widened to the whole worktree. Contract item 2.
    ///
    /// You ran `cargo run --example abc` in the repository root and you are now in `crates/api`;
    /// asking about the exact directory finds nothing. `dir.root` was written at visit time from
    /// the caller's git-root walk, so there is no `git` subprocess anywhere near the keystroke
    /// path.
    pub fn suggestion_in_workspace(&self, root: &str, mode: &str, typed: &str) -> Option<String> {
        if typed.is_empty() {
            return None;
        }
        self.store.read(|reader| {
            let inside = dirs_of_root(reader, root);
            if inside.is_empty() {
                return None;
            }
            let matched: Vec<(String, u64)> =
                reader.collect(Tree::RunByArgv, &span::argv_like(mode, typed), |key, _| {
                    let (argv, dir) = field::argv_and_dir(key)?;
                    (argv != typed && inside.binary_search(&dir).is_ok())
                        .then(|| (argv.into_owned(), dir))
                });

            let mut best = Best::default();
            for (argv, dir) in &matched {
                if let Some(row) = reader
                    .get(Tree::Run, &key::run(*dir, mode, argv))
                    .and_then(|bytes| RunRow::decode(&bytes))
                    && row.worked()
                {
                    best.offer(&row, argv);
                }
            }
            best.line
        })
    }

    /// The one remembered path that is never a jump destination.
    ///
    /// `$HOME` is recorded like anywhere else — what you run there is worth suggesting — but `cd`
    /// with no operand already goes there, so offering it as a frecency candidate wins nothing.
    /// Empty when there is no `$HOME` to compare against, which no real path equals.
    fn not_a_target(&self) -> String {
        self.home
            .as_deref()
            .map(|home| home.trim_end_matches('/').to_string())
            .unwrap_or_default()
    }
}

/// The best row a scan has seen, and what it takes to beat it.
///
/// The order is the SQL's: most net successes, then most recent. Strictly *better* wins, so a tie
/// is settled by key order and the same store answers the same way twice — which the SQL, ordering
/// by two columns and taking one row, did not guarantee.
///
/// It holds two integers and one string so that a scan over a range allocates once for the winner
/// rather than once per row it walks past.
#[derive(Default)]
struct Best {
    standing: Option<(i64, i64)>,
    line: Option<String>,
}

impl Best {
    fn offer(&mut self, row: &RunRow, argv: &str) {
        let standing = row.standing();
        if self.standing.is_none_or(|best| standing > best) {
            self.standing = Some(standing);
            self.line = Some(argv.to_string());
        }
    }
}

/// A directory row as the ranker wants it, unless it is never offered: where the shell stands,
/// `$HOME`, or a directory `redact::is_excluded` keeps out of `cd` — `/tmp`, anything in a `.git`.
fn candidate(row: DirRow, exclude: &str, home: &str) -> Option<Candidate> {
    let offered = row.path != exclude && row.path != home && !super::redact::is_excluded(&row.path);
    offered.then_some(Candidate {
        path: row.path,
        visits: row.visits,
        last_visit: row.last_visit,
    })
}

/// Order by the house frecency, then by the shorter path, and cut to `limit`.
///
/// `ORDER BY score DESC, length(path) ASC LIMIT n`, in the one place it can now be written. The
/// path itself settles the rest: two candidates equal in every ranking term must still be cut in
/// the same place on every run, and a `LIMIT` that dropped a different row each time would make
/// `cd` answer differently for no reason a person could see.
fn best_first(found: &mut Vec<Candidate>, limit: usize) {
    let at = now();
    found.sort_by(|a, b| {
        score(b.visits, b.last_visit, at)
            .partial_cmp(&score(a.visits, a.last_visit, at))
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.path.len().cmp(&b.path.len()))
            .then_with(|| a.path.cmp(&b.path))
    });
    found.truncate(limit);
}

/// Every directory of one worktree, in id order — the first half of contract item 2's join.
///
/// The order is the index's, not a sort: `dir_root` is keyed `(root, dir_id)` and a range walks in
/// key order, so what comes back is already sorted and the membership test below it is a binary
/// search over an array nobody had to sort.
fn dirs_of_root(reader: &Reader<'_, '_>, root: &str) -> Vec<u64> {
    reader.collect(Tree::DirByRoot, &span::dirs_of_root(root), |key, _| {
        field::trailing_id(key)
    })
}

#[cfg(test)]
mod tests {
    use super::super::db::fixture::*;
    use super::super::db::{Run, Step, Visit};
    use super::*;

    /// Being worth remembering and being worth jumping to are different questions.
    ///
    /// The design excludes `$HOME` from the store outright (Privacy and size, item 6, agreeing with
    /// zoxide's default). Its stated reason — "your home directory is never a jump target" — is an
    /// argument about *candidates*, and applying it at write time also throws away every command
    /// run at home, silently killing directory-aware suggestion where many people spend the day.
    /// So the row is written and only the jump is refused.
    #[test]
    fn home_is_remembered_but_never_jumped_to() {
        let (_dir, track) = store_with_home("/home/u");
        track.record(&ran("/home/u", "cargo run --example home", 0));
        track.record(&ran("/home/u/src", "cargo run --example child", 0));

        assert_eq!(
            track.suggestion_here("/home/u", SH, "cargo run --ex"),
            Some("cargo run --example home".to_string()),
            "what you run at home is still suggested at home"
        );

        let ranked = paths(track.directories_ranked("/elsewhere", 10));
        assert!(
            !ranked.iter().any(|path| path == "/home/u"),
            "`cd` with no operand already goes home, so it is not a frecency candidate: {ranked:?}"
        );
        assert!(
            ranked.iter().any(|path| path == "/home/u/src"),
            "but its children are ordinary directories: {ranked:?}"
        );

        let named = paths(track.directories_named("u", "/elsewhere", 10));
        assert!(
            !named.iter().any(|path| path == "/home/u"),
            "and naming it does not reach it either: {named:?}"
        );
    }

    /// The same line in two directories is two rows, and that difference is the whole feature.
    #[test]
    fn the_directory_decides_which_line_is_offered() {
        let (_dir, track) = store();
        track.record(&ran("/w/alpha", "cargo run --example xyz", 0));
        track.record(&ran("/w/beta", "cargo run --example abc", 0));

        assert_eq!(rows(&track, Tree::Run), 2);
        assert_eq!(
            track.suggestion_here("/w/alpha", SH, "cargo run --ex"),
            Some("cargo run --example xyz".to_string())
        );
        assert_eq!(
            track.suggestion_here("/w/beta", SH, "cargo run --ex"),
            Some("cargo run --example abc".to_string())
        );
    }

    /// The half-open range must find every line that starts with what was typed and nothing that
    /// merely contains it — including at the boundary where the range ends.
    #[test]
    fn the_prefix_scan_finds_only_what_starts_with_it() {
        let (_dir, track) = store();
        for line in [
            "cargo test",
            "cargo tesseract",
            "cargo tf",
            "cargo tests --all",
            "make cargo test",
            "cargo t",
        ] {
            track.record(&ran("/w/alpha", line, 0));
        }

        // `cargo tf` sorts immediately above the range and `cargo t` immediately below it; `make
        // cargo test` contains the prefix but does not start with it.
        let offered = track.suggestion_here("/w/alpha", SH, "cargo te");
        assert!(
            matches!(
                offered.as_deref(),
                Some("cargo test" | "cargo tesseract" | "cargo tests --all")
            ),
            "got {offered:?}"
        );

        for _ in 0..3 {
            track.record(&ran("/w/alpha", "cargo test", 0));
        }
        assert_eq!(
            track.suggestion_here("/w/alpha", SH, "cargo te"),
            Some("cargo test".to_string()),
            "the one that has worked most often wins"
        );

        // A longer line is still worth offering; the line already typed in full never is.
        assert_eq!(
            track.suggestion_here("/w/alpha", SH, "cargo test"),
            Some("cargo tests --all".to_string())
        );
        assert_eq!(track.suggestion_here("/w/alpha", SH, "cargo tf"), None);

        // Nor does a suggestion cross languages, or reach a directory with nothing in it.
        assert_eq!(track.suggestion_here("/w/alpha", "lua", "cargo te"), None);
        assert_eq!(track.suggestion_here("/w/nowhere", SH, "cargo te"), None);
        assert_eq!(track.suggestion_here("/w/alpha", SH, ""), None);
    }

    /// A line that has only ever failed is not a suggestion — the defect this fixes in passing.
    #[test]
    fn a_command_that_never_worked_is_never_offered() {
        let (_dir, track) = store();
        track.record(&ran("/w/alpha", "carg build", 127));
        assert_eq!(track.suggestion_here("/w/alpha", SH, "carg"), None);

        // Once it has worked more often than not, it is a real command again.
        track.record(&ran("/w/alpha", "carg build", 0));
        track.record(&ran("/w/alpha", "carg build", 0));
        assert_eq!(
            track.suggestion_here("/w/alpha", SH, "carg"),
            Some("carg build".to_string())
        );
    }

    /// You typed it in the repository root and you are now three directories down.
    #[test]
    fn the_worktree_answers_when_the_exact_directory_cannot() {
        let (_dir, track) = store();
        let root = "/w/beta";
        assert!(track.record(&Step {
            ran_in: Visit {
                path: root,
                root: Some(root),
            },
            moved_to: None,
            dwell_ms: 0,
            run: Some(Run {
                argv: "cargo run --example abc",
                mode: SH,
                status: Some(0),
                duration_ms: 12,
            }),
        }));

        assert_eq!(
            track.suggestion_here("/w/beta/crates/api", SH, "cargo run"),
            None,
            "nothing was ever run down here"
        );
        assert_eq!(
            track.suggestion_in_workspace(root, SH, "cargo run"),
            Some("cargo run --example abc".to_string())
        );
        assert_eq!(
            track.suggestion_in_workspace("/w/alpha", SH, "cargo run"),
            None,
            "and it is this repository, not any repository"
        );
    }

    /// The join contract item 2 is written in: the secondary range answers about every directory
    /// at once, and the worktree index is what cuts it back down to this one.
    #[test]
    fn the_widened_question_reaches_the_whole_worktree_and_stops_at_its_edge() {
        let (_dir, track) = store();
        let inside = |path: &'static str| Visit {
            path,
            root: Some("/w/beta"),
        };
        for (place, argv, times) in [
            (inside("/w/beta"), "cargo run --example abc", 3),
            (inside("/w/beta/crates/api"), "cargo run --example api", 1),
            (Visit::at("/w/alpha"), "cargo run --example far", 40),
        ] {
            for _ in 0..times {
                assert!(track.record(&Step {
                    ran_in: place,
                    moved_to: None,
                    dwell_ms: 0,
                    run: Some(Run {
                        argv,
                        mode: SH,
                        status: Some(0),
                        duration_ms: 1,
                    }),
                }));
            }
        }

        let inside_the_tree = track
            .store
            .read(|reader| Some(super::dirs_of_root(reader, "/w/beta")))
            .expect("the read succeeds");
        assert_eq!(
            inside_the_tree.len(),
            2,
            "both directories of the worktree, and neither of anybody else's"
        );

        assert_eq!(
            track.suggestion_in_workspace("/w/beta", SH, "cargo run --ex"),
            Some("cargo run --example abc".to_string()),
            "the most-run line of this worktree, not the most-run line in the store"
        );
        assert_eq!(
            track.suggestion_in_workspace("/w/beta", SH, "cargo run --example api"),
            None,
            "the line already typed in full is never the answer"
        );
        assert_eq!(
            track.suggestion_in_workspace("/w/beta", "lua", "cargo"),
            None
        );
    }

    /// Naming a directory is an indexed range over the folded final component, and where you are
    /// standing is never among the answers.
    #[test]
    fn directories_are_found_by_the_name_they_end_in() {
        let (_dir, track) = store();
        for path in ["/w/Rust", "/w/rustlings", "/w/prust", "/w/go/src"] {
            track.record(&Step {
                ran_in: Visit::at("/w"),
                moved_to: Some(Visit::at(path)),
                dwell_ms: 0,
                run: None,
            });
        }

        let found = paths(track.directories_named("rust", "/w/nowhere", 10));
        assert_eq!(found.len(), 2, "got {found:?}");
        assert!(
            found.contains(&"/w/Rust".to_string()),
            "case is folded at write time"
        );
        assert!(
            found.contains(&"/w/rustlings".to_string()),
            "a prefix of the name counts"
        );
        assert!(
            !found.contains(&"/w/prust".to_string()),
            "but a substring of it does not — that is a weaker tier and a different query"
        );

        // Where you already are is never an answer, however well it matched.
        assert_eq!(
            paths(track.directories_named("rust", "/w/Rust", 10)),
            vec!["/w/rustlings".to_string()]
        );
        // And nothing typed is not a query at all.
        assert!(track.directories_named("", "/w/nowhere", 10).is_empty());

        // The tiers no index can serve get every directory. `/w` is among them with nothing
        // counted: it is where the commands ran, so a row had to exist, but nobody walked there.
        let all = track.directories_ranked("/w/nowhere", 10);
        assert_eq!(all.len(), 5);
        assert!(all.iter().all(|candidate| {
            if candidate.path == "/w" {
                candidate.visits == 0
            } else {
                candidate.visits == 1 && candidate.last_visit > 0
            }
        }));

        // A limit cuts the list where the ranker would have cut it, not arbitrarily.
        assert_eq!(track.directories_ranked("/w/nowhere", 2).len(), 2);
    }

    /// Contract item 9: the ordering the database used to do is done here, by the one function
    /// that also orders `cd`'s candidates. The cut a `limit` makes has to fall in the same place.
    #[test]
    fn the_candidates_come_back_in_the_order_the_ranker_would_have_put_them_in() {
        let (_dir, track) = store();
        for (path, visits) in [("/w/one", 1), ("/w/three", 3), ("/w/two", 2)] {
            for _ in 0..visits {
                track.record(&Step {
                    ran_in: Visit::at("/w"),
                    moved_to: Some(Visit::at(path)),
                    dwell_ms: 0,
                    run: None,
                });
            }
        }

        let ranked = paths(track.directories_ranked("/w", 10));
        assert_eq!(
            ranked,
            vec![
                "/w/three".to_string(),
                "/w/two".to_string(),
                "/w/one".to_string()
            ],
            "most visited first, every one of them just now"
        );
        assert_eq!(
            paths(track.directories_ranked("/w", 2)),
            vec!["/w/three".to_string(), "/w/two".to_string()],
            "and the limit takes the top of that order, not an arbitrary two"
        );
    }

    fn paths(found: Vec<Candidate>) -> Vec<String> {
        found.into_iter().map(|candidate| candidate.path).collect()
    }
}
