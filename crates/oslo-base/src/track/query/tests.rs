//! Which matching line a suggestion offers: the newest one.

use super::super::db::fixture::*;
use super::*;

/// Age one row by an hour, as if its runs were this morning's habit.
fn aged(track: &Track, dir: &str, argv: &str) {
    let done = track.store.write(|writer| {
        let id = lookup_dir(writer, dir)?;
        let primary = key::run(id, SH, argv);
        let mut row = RunRow::decode(&writer.get(Tree::Run, &primary)?)?;
        row.last_at -= 3600;
        writer.put(Tree::Run, primary, row.encode())
    });
    assert!(done.is_some(), "the row was rewritten");
}

/// The latest matching line wins over a much-run older one: a ghost answers what you were
/// just doing, and frequency only breaks ties inside the same second.
#[test]
fn the_latest_line_beats_a_frequent_older_one() {
    let (_dir, track) = store();
    for _ in 0..5 {
        track.record(&ran("/w/alpha", "cargo build --release", 0));
    }
    track.record(&ran("/w/alpha", "cargo build --workspace", 0));
    aged(&track, "/w/alpha", "cargo build --release");

    assert_eq!(
        track.suggestion_here("/w/alpha", SH, "cargo b"),
        Some("cargo build --workspace".to_string()),
        "typed once just now beats five runs an hour ago"
    );
}

/// The latest line wins even though it exited non-zero: `history | grep xyz` finding
/// nothing is a command that ran, not a typo, and retyping it to change the pattern is
/// exactly what the ghost is for.
#[test]
fn the_latest_line_wins_even_when_it_exited_non_zero() {
    let (_dir, track) = store();
    for _ in 0..4 {
        track.record(&ran("/w/alpha", "history | grep alpha", 0));
    }
    track.record(&ran("/w/alpha", "history | grep xyz", 1));
    aged(&track, "/w/alpha", "history | grep alpha");

    assert_eq!(
        track.suggestion_here("/w/alpha", SH, "history | grep"),
        Some("history | grep xyz".to_string()),
        "just run and exited 1 beats four successes an hour ago"
    );
}
