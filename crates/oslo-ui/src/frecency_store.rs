//! Shared access to the frecency table, seeded from the commands you have actually run.
//!
//! [`crate::spec::FrecencyTracker`] had no callers at all: `record_use` was never invoked, so
//! `get_score` answered 0.0 for everything and the completion sort collapsed to alphabetical —
//! typing `exit` offered `exitsnoop-bpfcc`. This wraps the tracker in interior mutability, because
//! the completer only ever holds `&self`, and in a source of past counts, because a ranking that
//! resets every session never gets good.
//!
//! # There is no frecency file
//!
//! There used to be one: `$HOME/.oslo_frecency`, an append-only log of `count<TAB>time<TAB>name`.
//! It was a bolt-on — written to give the dead tracker something to load, at a moment when giving
//! it a file of its own was the smallest change that worked — and every reason for it had gone:
//!
//! - **The counts were already in the profile store.** `Tree::Run` holds one row per command line
//!   with `runs` and `last_at` on it, which is the same `(count, last used)` pair the log kept.
//! - **It was the profile leak.** Directory ranking is per profile and command ranking was not, so
//!   `oslo --profile=claude` kept an agent's `cd`s out of yours and let every command it completed
//!   into the table that ranks yours.
//! - **It was the only store outside XDG**, sitting in `$HOME` beside nothing.
//!
//! # Runs, not completions
//!
//! The log counted *completions you accepted*; this counts *commands you ran*. `git` should rank
//! high because you run it constantly, not because you press Tab at it constantly — and a command
//! typed in full, which is most of them, used to teach the ranking nothing at all.
//!
//! Wrappers come off on the way in, so `sudo git status` ranks `git`.
//!
//! # Seeded on first use, not at startup
//!
//! Folding the run table is a scan, and a shell that pays for one before its first prompt is a
//! shell that got slower to start so that Tab could be marginally better. The scan happens the
//! first time a score is asked for — the same thing the history finder does when it opens.

use super::spec::FrecencyTracker;
use std::sync::Mutex;

pub struct FrecencyStore {
    tracker: Mutex<FrecencyTracker>,
    /// Whether past runs should be folded in on first use. False for a store that must not read
    /// anything — tests, and any shell with nobody typing at it.
    seed: bool,
    seeded: Mutex<bool>,
}

impl FrecencyStore {
    /// A store that folds in what this profile has run, the first time it is asked anything.
    pub fn from_history() -> Self {
        Self {
            tracker: Mutex::new(FrecencyTracker::new()),
            seed: true,
            seeded: Mutex::new(false),
        }
    }

    /// A store that reads nothing. Used by tests and by non-interactive shells.
    pub fn in_memory() -> Self {
        Self {
            tracker: Mutex::new(FrecencyTracker::new()),
            seed: false,
            seeded: Mutex::new(true),
        }
    }

    /// Fold in past runs: `(line, runs, last_at)` as the store holds them.
    ///
    /// Takes the counts rather than the database so that what is tested is the folding, and so that
    /// this crate does not have to build a store to have an opinion about one.
    pub fn seed_from(&self, commands: impl IntoIterator<Item = (String, i64, i64)>) {
        let mut tracker = self.tracker.lock().unwrap_or_else(|held| held.into_inner());
        for (line, runs, last_at) in commands {
            // The command *name*, so `cargo build` and `cargo test` both rank `cargo` — which is
            // what the dropdown offers. `head_of` has already dropped `sudo` and `VAR=x`.
            let head = oslo_base::track::head_of(&line);
            let Some(name) = head.split_whitespace().next() else {
                continue;
            };
            if name.is_empty() || runs <= 0 {
                continue;
            }
            tracker.merge(name, runs.max(0) as u64, last_at.max(0) as u64);
        }
    }

    /// Fold in past runs once, if this store was built to.
    fn ensure_seeded(&self) {
        if !self.seed {
            return;
        }
        {
            let mut seeded = self.seeded.lock().unwrap_or_else(|held| held.into_inner());
            if *seeded {
                return;
            }
            // Marked before the scan, not after: a scan that finds nothing must not be retried on
            // every keystroke for the rest of the session.
            *seeded = true;
        }
        let Some(track) = oslo_base::track::store() else {
            return;
        };
        // A cap, because this folds to command *names* — a store with a hundred thousand runs has
        // a few hundred of those, and the tail of distinct lines cannot change the ranking.
        let commands = track.commands(20_000);
        self.seed_from(
            commands
                .into_iter()
                .map(|command| (command.line, command.runs, command.last_at)),
        );
    }

    /// Count one use of `name`, for this session.
    ///
    /// **In memory only.** The run itself is written down by the tracker when the command runs, so
    /// writing here as well would count it twice. This exists so that accepting a completion moves
    /// the ranking *now* rather than at the next start.
    pub fn record(&self, name: &str) {
        if name.is_empty() {
            return;
        }
        self.ensure_seeded();
        self.tracker
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .record_use(name);
    }

    /// This command's rank, higher being better. Zero for anything never used.
    pub fn score(&self, name: &str) -> f64 {
        self.ensure_seeded();
        self.tracker
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .get_score(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unused_command_scores_zero() {
        let store = FrecencyStore::in_memory();
        assert_eq!(store.score("git"), 0.0);
    }

    #[test]
    fn use_raises_the_score_and_more_use_raises_it_further() {
        let store = FrecencyStore::in_memory();
        store.record("git");
        let once = store.score("git");
        assert!(once > 0.0);
        store.record("git");
        assert!(store.score("git") > once);
        assert!(store.score("git") > store.score("gcc"));
    }

    fn now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    /// **What replaced the file.** Past runs come from the profile store, so a fresh session ranks
    /// what you actually use rather than starting flat.
    #[test]
    fn past_runs_rank_before_anything_is_typed() {
        let store = FrecencyStore::in_memory();
        store.seed_from([
            ("cargo build".to_string(), 40, now()),
            ("ls -la".to_string(), 2, now()),
        ]);
        assert!(store.score("cargo") > store.score("ls"));
        assert_eq!(store.score("nothing-here"), 0.0);
    }

    /// The whole line folds to its command, so every `cargo …` counts towards `cargo`.
    #[test]
    fn every_line_of_a_command_counts_towards_its_name() {
        let store = FrecencyStore::in_memory();
        store.seed_from([
            ("cargo build".to_string(), 3, now()),
            ("cargo test".to_string(), 4, now()),
            ("git status".to_string(), 5, now()),
        ]);
        assert!(store.score("cargo") > store.score("git"), "3 + 4 beats 5");
    }

    /// `sudo git status` is a use of `git`. Ranking `sudo` would put it above everything you own.
    #[test]
    fn a_wrapper_is_not_the_command() {
        let store = FrecencyStore::in_memory();
        store.seed_from([("sudo git status".to_string(), 9, now())]);
        assert!(store.score("git") > 0.0);
        assert_eq!(store.score("sudo"), 0.0);
    }

    /// Recency is half the score, so a seeded count must keep the time it happened rather than
    /// being dated to startup.
    #[test]
    fn an_old_run_ranks_below_a_recent_one_with_the_same_count() {
        let store = FrecencyStore::in_memory();
        let long_ago = now() - 60 * 60 * 24 * 90;
        store.seed_from([
            ("ancient".to_string(), 10, long_ago),
            ("current".to_string(), 10, now()),
        ]);
        assert!(store.score("current") > store.score("ancient"));
    }

    #[test]
    fn a_row_with_nothing_usable_in_it_is_skipped() {
        let store = FrecencyStore::in_memory();
        store.seed_from([
            ("".to_string(), 5, now()),
            ("   ".to_string(), 5, now()),
            ("zero".to_string(), 0, now()),
        ]);
        assert_eq!(store.score("zero"), 0.0);
    }

    /// A store that reads nothing must not go looking, and one that has read must not read again.
    #[test]
    fn seeding_happens_at_most_once() {
        let store = FrecencyStore::in_memory();
        assert!(*store.seeded.lock().unwrap_or_else(|held| held.into_inner()));
        store.score("anything");

        let live = FrecencyStore::from_history();
        assert!(!*live.seeded.lock().unwrap_or_else(|held| held.into_inner()));
        // No store is installed in a test process, so this folds nothing — and still only tries the
        // once, which is the property that matters on the keystroke path.
        live.score("anything");
        assert!(*live.seeded.lock().unwrap_or_else(|held| held.into_inner()));
    }
}
