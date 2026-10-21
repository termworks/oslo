//! What the shell learns from being used: where you work, and what you run there.
//!
//! oslo's REPL loop already computes everything this store holds — the line, its language, whether
//! it was secret, the directory before and after, the wall-clock duration and the exit status — and
//! then discards all but the line and the language. This is not new telemetry; it is the same
//! sixty lines of `repl.rs` not throwing their inputs away. What it buys is a `cd` that can find a
//! directory by name and a suggestion that knows `cargo run --example` meant something different in
//! the last project.
//!
//! # It is an aggregate, not a log
//!
//! The `.db` of the same profile is already the event log, one row per line typed. A second log
//! would double the
//! write cost and double the privacy surface for a chronology that already exists — and the store
//! underneath has no `VACUUM`, so a file that grows is a permanent high-water mark. That argument
//! got sharper rather than weaker when the engine changed: Tagdata grows in one 8 MiB step and
//! never gives it back, so the aggregate and the bounds in [`prune`] are what stand between oslo
//! and 8.5 MiB for ever. An aggregate is bounded by distinct *behaviour* rather than by time:
//! repeats, which are the entire point of the ranking, cost nothing after the first. What it gives
//! up is the order of individual executions, which is recoverable by joining the event log if
//! anything ever needs it.
//!
//! # Why it lives in the library
//!
//! `src/startup/` is private to the binary (`main.rs` declares `mod startup;`), and `builtin_cd` is
//! library code that has to be able to ask this store where to jump. So it is here, and the handle
//! is a process-global installed by the interactive loop and by nothing else.
//!
//! That is the privacy design as much as the plumbing one. A script, an `oslo -c`, or a subshell
//! never calls [`install`], so [`store`] answers `None` and every clever path is *structurally*
//! unreachable rather than gated by a flag someone has to remember to check. A CI job's command
//! lines cannot reach the file because there is no file to reach.

pub mod db;
pub mod entered;
pub mod history;
pub mod log;
pub mod nested;
pub mod profile;
pub mod session;
// The rule that decides which copy of a record wins, shared by everything that syncs. History,
// macros and secrets keep their own storage and their own encodings; what they must not each invent
// is a second idea of what "newer" means.
pub mod stamp;
pub mod sync;
// The one module that knows which key-value engine is underneath. Read its note before touching
// it: nothing else may `use tagdata`, so that moving engines again is a day of rewriting one
// directory. Named `kv` rather than `store` because `store()` below is already the accessor for
// this module's process-global, and a module and a function sharing a name is a thing to read
// twice.
pub mod kv;
pub mod outcome;
pub mod prune;
pub mod query;
pub mod redact;
pub(crate) mod row;
pub mod score;
pub mod write;
pub mod writer;

pub use db::{Run, Step, Track, Visit};
pub use outcome::Outcome;
pub use prune::PrunePreview;
pub use redact::head_of;
pub use sync::{
    EventId, HistoryCompletion, HistoryEvent, HistoryFilter, HistoryMatch, HistorySegment,
    HistoryStatus, ImportReport, SyncReport, status_file, sync_files, verify_file,
};

use std::path::PathBuf;
use std::sync::OnceLock;

/// The one store, or `None` in every shell that is not an interactive session.
///
/// A `OnceLock` rather than a `Mutex` for the reason `autocd::AUTOCD` gives for its own global: this
/// is a property of the invocation, decided once before the first prompt and never afterwards.
static TRACK: OnceLock<Option<Track>> = OnceLock::new();

/// Hand the process its store.
///
/// **Two callers, and no more.** The interactive loop, which is where commands are normally
/// recorded; and a `-c` command under `$OSLO_ALLHIST`, which is the one non-interactive shell with
/// something to record. Everything else — a script, a subshell, a `#!/bin/sh` maintainer script —
/// runs with no store at all, which is what keeps a `/bin/sh` from logging the whole system.
///
/// `None` is a legitimate argument: a shell whose store would not open is a working shell with a
/// dumber `cd`, and installing the absence says so explicitly rather than leaving the slot to be
/// filled by whoever calls next.
///
/// Later calls are ignored, so nothing can swap the store out from under a running shell.
pub fn install(track: Option<Track>) {
    let _ = TRACK.set(track);
}

/// The store, if this shell has one.
pub fn store() -> Option<&'static Track> {
    TRACK.get()?.as_ref()
}

/// Where the store is kept, given the environment.
///
/// `<data>/oslo/history/<profile>/hist.db` — state the user accumulates, not configuration they
/// wrote. `None` when neither `$XDG_DATA_HOME` nor `$HOME` is knowable — a container's `nobody` —
/// which must run without a store rather than fail.
///
/// One store per profile, holding everything: the event log, the aggregate, the directories. It used
/// to be `<profile>.kv` flat in `<data>/oslo/`; see [`profile::store_path`] for why it moved.
/// Nothing adopts a store written under the old name — a file this did not write is yours.
pub fn default_path(xdg_data: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    profile::store_path(xdg_data, home, "db")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// The store is one process-wide slot that can only ever be written once, so every test that
    /// touches it is this one.
    static SERIAL: Mutex<()> = Mutex::new(());

    #[test]
    fn the_store_lives_in_the_profiles_own_directory() {
        // A directory per profile, named after the profile — see `track::profile::store_path`.
        let named =
            |dir: &str| PathBuf::from(format!("{dir}/oslo/history/{}/hist.db", profile::current()));
        assert_eq!(
            default_path(Some("/x/data"), Some("/home/u")),
            Some(named("/x/data"))
        );
        assert_eq!(
            default_path(None, Some("/home/u")),
            Some(named("/home/u/.local/share"))
        );
        // An empty XDG is unset, not a relative path from the root.
        assert_eq!(
            default_path(Some("  "), Some("/home/u")),
            Some(named("/home/u/.local/share"))
        );
        // Nowhere to put it is not an error; it is a shell without a store.
        assert_eq!(default_path(None, None), None);
    }

    /// The whole privacy argument in one assertion: until the interactive loop installs a store
    /// there is none, so a script cannot record anything however hard it tries.
    #[test]
    fn nothing_is_tracked_until_the_interactive_loop_installs_a_store() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        assert!(store().is_none(), "a process starts with no store at all");

        let dir = tempfile::tempdir().expect("a temp dir");
        let track = Track::open(&dir.path().join("track.kv")).expect("the store opens");
        install(Some(track));
        assert!(store().is_some(), "and has one once it is installed");

        // A second install cannot swap the store out from under a running shell.
        install(None);
        assert!(store().is_some());
    }
}
