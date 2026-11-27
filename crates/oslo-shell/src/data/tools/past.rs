//! `history` as rows — what this shell has already been asked to do.
//!
//! ```text
//! history | where 'worked == false' | sort-by last | final 10
//! history | where 'places == 1'                      -- what belongs to one project
//! history | group-by dir | sort-by count
//! ```
//!
//! # Why this one is worth being a producer
//!
//! Forty verbs could be pointed at three things — `ls`, `ps` and `df`. Meanwhile the shell was
//! already writing down, for every command anyone ran, how long it took, whether it worked, where
//! it ran, how many *distinct* places it has run in, and which worktree it belonged to. None of it
//! was answerable: `oslo history` prints, and printing is where a question about the data stops.
//!
//! The store is folded per *line*, not per run — the same shape the finder reads — so a command run
//! four hundred times is one row that says four hundred. That is what makes `sort-by runs` mean
//! "what do I actually do" rather than "what did I do most recently".
//!
//! # The columns are typed, so the questions are arithmetic
//!
//! `last` is a [`Val::Time`] and not a rendering, so it draws as a date and compares as a number:
//! `where 'last > 2days'` is the units syntax working on it, and `sort-by last` is chronological
//! rather than alphabetical. `runs` and `places` are counts. This is the whole argument for the
//! typed kinds, applied to the one table the shell writes for itself.
//!
//! # `history` alone is still `history`
//!
//! The name is a shell builtin everywhere, so this is the fourth deliberate collision after `ls`,
//! `ps` and `df`, and it follows their rule exactly: a lone `history` is the builtin, printing
//! numbered lines as bash does, because a single stage has no edge and no edge can carry rows.
//! Structure is what you get by piping it somewhere.

use super::super::{Record, Val};

/// What a row of the past has. Declared here, beside the code that fills it.
pub const COLUMNS: &[&str] = &[
    "line", "runs", "last", "dir", "places", "worked", "mode", "host",
];

/// How many distinct command lines to fold out of the store.
///
/// The same bound the finder uses, and for the same reason: the scan is one pass over the run
/// table, and what is worth keeping is not known until it is done. A store with a hundred thousand
/// runs folds to far fewer lines than this.
const MOST: usize = 20_000;

/// The commands this shell has been asked to run, most recent first.
pub fn rows() -> Vec<Record> {
    // **Opened here when nothing has opened it**, which is what makes this work in a script. The
    // interactive loop opens the store because it writes to it; `oslo -c 'history | …'` does not,
    // and an empty answer there would be a wrong one — the past exists, this shell simply had no
    // reason to look at it yet. The 2.6 ms is paid by the caller who asked for it and by nobody
    // else, which is the same bargain `history::record_command` already strikes.
    if oslo_base::track::store().is_none() {
        oslo_base::track::install(
            oslo_base::track::default_path(
                std::env::var("XDG_DATA_HOME").ok().as_deref(),
                std::env::var("HOME").ok().as_deref(),
            )
            .and_then(|path| oslo_base::track::Track::open(&path)),
        );
    }
    let Some(track) = oslo_base::track::store() else {
        // Still nothing: no `$HOME`, or a store that will not open. An empty table is the honest
        // answer — there is nothing to report and nothing went wrong.
        return Vec::new();
    };
    let mut commands = track.commands(MOST);
    // Most recent first, which is the order somebody reading their own history expects. A `sort-by`
    // downstream overrides it, and the drawn table shows whichever order arrived.
    commands.sort_by_key(|command| std::cmp::Reverse(command.last_at));
    commands.into_iter().map(row).collect()
}

/// One folded command as a row.
fn row(command: oslo_base::track::history::Command) -> Record {
    Record::from_pairs([
        ("line", Val::Str(command.line)),
        ("runs", Val::Int(command.runs)),
        // Nanoseconds, because that is what the kind holds: the store keeps seconds.
        (
            "last",
            Val::Time(command.last_at.saturating_mul(1_000_000_000)),
        ),
        ("dir", Val::Str(command.dir)),
        ("places", Val::Int(command.places as i64)),
        ("worked", Val::Bool(command.worked)),
        ("mode", Val::Str(command.mode)),
        ("host", Val::Str(command.host)),
    ])
}

#[cfg(test)]
#[path = "past/tests.rs"]
mod tests;
