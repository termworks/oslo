//! The tracker, against a store in a temporary directory.

use super::*;
const SH: &str = "sh";

fn store() -> (tempfile::TempDir, Track) {
    let dir = tempfile::tempdir().expect("a temp dir");
    let track = Track::open(&dir.path().join("track.db")).expect("the database opens");
    (dir, track)
}

fn tracker() -> Tracker {
    Tracker {
        since: SystemTime::now(),
        worktree: None,
        immediate: true,
        entered: None,
    }
}

/// The whole point of the write path, asserted through the read side: the same prefix means a
/// different line depending on where it is typed.
#[test]
fn a_command_is_attributed_to_where_it_started_not_where_it_left_you() {
    let (_dir, track) = store();
    let mut tracker = tracker();

    // `cd /w/beta` ran in /w and left the shell in /w/beta.
    tracker.write(
        &track,
        "/w",
        "/w/beta",
        ran("cd /w/beta", SH, &Ok(0), Duration::from_millis(1)),
        None,
    );
    tracker.write(
        &track,
        "/w/beta",
        "/w/beta",
        ran(
            "cargo run --example abc",
            SH,
            &Ok(0),
            Duration::from_millis(20),
        ),
        None,
    );

    assert_eq!(
        track.suggestion_here("/w/beta", SH, "cargo run --ex"),
        Some("cargo run --example abc".to_string())
    );
    assert_eq!(
        track.suggestion_here("/w", SH, "cargo run --ex"),
        None,
        "the cd was attributed to /w; what it ran into was not"
    );
    assert_eq!(
        track.suggestion_here("/w", SH, "cd /w/b"),
        Some("cd /w/beta".to_string())
    );
}

/// A command that never finished parsing is not a command, and a command that failed is
/// recorded as having failed rather than not recorded at all.
#[test]
fn what_reaches_the_store_and_what_does_not() {
    let syntax = Err(ShellError::SyntaxError("unexpected token".to_string()));
    assert!(ran("mypassword )", SH, &syntax, Duration::ZERO).is_none());

    let failed = ran("cargo buidl", SH, &Ok(101), Duration::from_millis(3));
    assert_eq!(failed.map(|run| run.status), Some(Some(101)));

    // `exit 0` succeeded, however the loop learned about it.
    let quit = ran("exit", SH, &Err(ShellError::Exit(0)), Duration::ZERO);
    assert_eq!(quit.map(|run| run.status), Some(Some(0)));
}

/// A session that was told to leave no trace leaves none of this either.
#[test]
fn a_session_that_keeps_no_history_opens_no_store() {
    let kept = |file: Option<&str>, no_trace, max_size| {
        keeps_a_record(&history::Settings {
            ignore_space: true,
            ignore_dups: false,
            immediate: true,
            file: file.map(std::path::PathBuf::from),
            no_trace,
            max_size,
        })
    };
    assert!(kept(Some("/home/u/.oslo_history"), false, 10_000));
    assert!(
        !kept(None, true, 10_000),
        "HISTFILE= disables the store with it"
    );
    assert!(
        !kept(Some("/home/u/.oslo_history"), false, 0),
        "and so does HISTSIZE=0"
    );
    // **The regression this pair exists to catch.** No history file is the default now, and a
    // shell nobody has configured must still keep a store — the finder, `cd` ranking and the
    // model all live in it.
    assert!(
        kept(None, false, 10_000),
        "no history file is not a request to leave no trace"
    );
}

/// A secret line leaves nothing behind — not the line, not the directory, not the minutes.
#[test]
fn a_secret_line_is_not_a_boundary_at_all() {
    let (_dir, track) = store();
    let mut tracker = tracker();
    tracker.forget_boundary();

    // Nothing was written, so the directory the secret command ran in is not even known.
    assert_eq!(track.suggestion_here("/w/alpha", SH, "pass"), None);
    assert!(track.directories_named("alpha", "/w", 10).is_empty());

    // The next ordinary command still records normally.
    tracker.write(
        &track,
        "/w",
        "/w/alpha",
        ran("cd alpha", SH, &Ok(0), Duration::ZERO),
        None,
    );
    assert_eq!(
        track
            .directories_named("alpha", "/w", 10)
            .into_iter()
            .map(|found| found.path)
            .collect::<Vec<_>>(),
        vec!["/w/alpha".to_string()]
    );
}
