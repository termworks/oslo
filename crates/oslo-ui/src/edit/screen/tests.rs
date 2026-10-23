//! The escapes, asserted rather than observed.
//!
//! Cursor bugs in a line editor are invisible until they are not: the prompt walks up the screen,
//! or a character of the last line disappears. Every one of those is an off-by-one in a sequence
//! that can be written down, so it is written down here.

use super::*;

fn at(rows: usize, cursor_row: usize, cursor_col: usize) -> At {
    At {
        rows,
        cursor_row,
        cursor_col,
    }
}

/// The first draw has nothing above it, so it does not move up — an `ESC [ 0 A` would be a move
/// of one row on some terminals, which is how the prompt starts climbing.
#[test]
fn the_first_draw_does_not_move_up() {
    let out = redraw(0, "$ ls", at(1, 0, 4));
    assert!(!out.contains("\x1b[0A"), "a zero-row move: {out:?}");
    assert!(out.starts_with("\r\x1b[K"), "{out:?}");
    assert!(out.contains("$ ls"));
    assert!(out.ends_with("\r\x1b[4C"), "{out:?}");
}

/// **The erase comes before the content, never after.** After the content the cursor may be
/// sitting in the last column with the wrap pending, and erasing from there takes back the
/// character just drawn.
#[test]
fn the_erase_happens_before_anything_is_drawn() {
    let out = redraw(0, "$ ls", at(1, 0, 4));
    let erase = out.find("\x1b[J").expect("erases");
    let content = out.find("$ ls").expect("draws");
    assert!(erase < content, "erased after drawing: {out:?}");
}

/// A redraw from a lower row walks back up to the top of the block first.
#[test]
fn a_redraw_returns_to_the_top_of_the_block() {
    let out = redraw(2, "frame", at(3, 1, 5));
    assert!(out.starts_with("\x1b[2A\r\x1b[K"), "{out:?}");
}

/// **The erase never begins at the screen's own origin.**
///
/// `ESC[J` from row 0, column 0 is an erase of the whole screen, and a terminal that keeps
/// scrollback moves what was visible into history before clearing it — so every keystroke after a
/// `Ctrl-L` pushed another copy of the half-typed prompt into the buffer. Ten keystrokes, ten
/// lines, measured in tmux. The column, not the row, is what steps off the origin.
#[test]
fn the_erase_never_starts_at_the_screen_origin() {
    // Every ending that replaces the block, not just the redraw. A transcript drawn on a cleared
    // screen starts at row 0 with no lead, which is exactly the case this dodge exists for.
    let endings = [
        redraw(0, "$ ls", at(1, 0, 4)),
        transcript(0, 0, &["ls".into()], "-", 20, "", Marks::default()),
        given(0, 0, &["ls".into()], 20, Some(&"ls"), Marks::default()),
    ];
    for out in endings {
        let erase = out.find("\x1b[J").expect("erases");
        assert!(
            out[..erase].ends_with("\x1b[K\x1b[C"),
            "the erase must run from column one of this row: {out:?}"
        );
    }
}

/// **The erase never leaves the block's own row.**
///
/// `ESC[B` at the bottom of the screen is a no-op, so the `ESC[A` that used to pair with it moved
/// the cursor one row above the block and the frame overwrote the line above — the last line of
/// the previous command's output. On a full screen `ls` printed its one line and the prompt ate
/// it, which read as a builtin that had stopped working.
#[test]
fn the_erase_makes_no_vertical_move() {
    let out = redraw(0, "$ ls", at(1, 0, 4));
    let erase = out.find("\x1b[J").expect("erases");
    let head = &out[..erase];
    assert!(!head.contains("\x1b[B"), "steps down to erase: {out:?}");
    assert!(!head.contains("\x1b[A"), "steps up to erase: {out:?}");
}

/// The cursor is placed by walking up from the *last* row, because that is where writing the
/// content leaves it.
#[test]
fn the_cursor_is_placed_from_the_bottom_of_the_block() {
    // Three rows, cursor on the first: come up two.
    let out = redraw(0, "frame", at(3, 0, 6));
    assert!(out.ends_with("\r\x1b[2A\x1b[6C"), "{out:?}");
    // Cursor already on the last row: no vertical move at all.
    let bottom = redraw(0, "frame", at(3, 2, 1));
    assert!(bottom.ends_with("\r\x1b[1C"), "{bottom:?}");
    assert!(!bottom.contains("\x1b[0A"));
}

/// Column zero needs no horizontal move, for the same reason row zero needs no vertical one.
#[test]
fn column_zero_emits_no_move() {
    let out = redraw(0, "frame", at(1, 0, 0));
    assert!(out.ends_with('\r'), "{out:?}");
    assert!(!out.contains("\x1b[0C"), "{out:?}");
}

/// A `\r` after the content, so the pending wrap cannot make the position ambiguous.
#[test]
fn the_position_is_committed_after_the_content() {
    let out = redraw(0, "$ x", at(1, 0, 3));
    let content = out.find("$ x").expect("draws");
    assert!(
        out[content..].starts_with("$ x\r"),
        "no carriage return after the content: {out:?}"
    );
}

/// The shared cursor-save slot belongs to the multiplexer. Touching it means a restore lands
/// wherever somebody else's save left the cursor.
#[test]
fn the_shared_save_slot_is_never_touched() {
    for out in [redraw(3, "f", at(4, 2, 9)), finish(1, 3)] {
        assert!(!out.contains("\x1b7"), "saves the cursor: {out:?}");
        assert!(!out.contains("\x1b8"), "restores the cursor: {out:?}");
    }
}

/// Accepting a line steps down to the bottom of the block before the newline, so the command's
/// output starts below the whole thing rather than inside it.
#[test]
fn finishing_steps_past_the_last_row() {
    assert_eq!(finish(0, 3), "\x1b[2B\r\n");
    assert_eq!(finish(2, 3), "\r\n", "already on the last row");
    assert_eq!(finish(0, 1), "\r\n", "a one-row block");
}

/// A frame that has shrunk must leave nothing of the taller one behind — which is what the erase
/// at the top of the block is for.
#[test]
fn a_shorter_frame_clears_what_the_taller_one_left() {
    let out = redraw(1, "short", at(1, 0, 5));
    assert!(
        out.contains("\x1b[J"),
        "nothing clears the rows below: {out:?}"
    );
}

/// The other ending: back to the top of the block and *nothing else*. The prompt has to stay on
/// screen while the command runs — it is the shell you are still looking at when the browser opens
/// beside it — and the next prompt overwrites it, because `redraw` erases before it draws.
#[test]
fn parking_returns_to_the_top_without_clearing() {
    assert_eq!(park(0), "\r", "already on the first row");
    assert_eq!(park(2), "\x1b[2A\r");
    for row in 0..4 {
        let out = park(row);
        assert!(!out.contains("\x1b[J"), "clears the screen: {out:?}");
        assert!(!out.contains("\x1b[K"), "clears the row: {out:?}");
        assert!(!out.contains('\n'), "must not scroll: {out:?}");
        assert!(!out.contains("\x1b[B"), "must not step down: {out:?}");
    }
}

/// The third ending: the prompt is replaced by what was run, at the head of a rule.
///
/// Driven through `transcript`, which reads the wall clock, so what is pinned here is the shape
/// around the stamp rather than the stamp — `framed` is where the arithmetic is checked against a
/// time that holds still.
#[test]
fn a_transcript_puts_the_command_at_the_left_edge() {
    let out = transcript(0, 0, &["ls".into()], "-", 60, "", Marks::default());
    assert!(
        out.contains("\r---[ ls ]---"),
        "the command leads the rule: {out:?}"
    );
    let row = out
        .split("\r\n")
        .find(|part| part.contains("[ ls ]"))
        .expect("a row");
    let row = row.rsplit('\r').next().expect("past the cursor moves");
    assert_eq!(
        crate::prompt::printed_width(row),
        60,
        "the row is exactly the width it was given: {row:?}"
    );

    // **Cleared, not stepped past.** The whole point is that the prompt is not what scrolls back.
    assert!(out.contains("\x1b[J"), "the block has to go: {out:?}");
    // A blank row under it, so the block sits apart from the output below.
    assert!(
        out.ends_with("\r\n\r\n"),
        "a blank row under the block: {out:?}"
    );

    // **And the block's own blank rows are put back.** They are drawn by `layout::place` as part of
    // the block, so erasing the block erases them — and an ending that started at the rule moved
    // the whole transcript up onto the row its own gap was meant to be. One above and one below is
    // what a frame between two commands' output has to have; before this it had only the one below.
    let led = transcript(1, 1, &["ls".into()], "-", 60, "", Marks::default());
    assert!(
        led.starts_with("\x1b[1A\r\x1b[K\x1b[C\x1b[J\r\r\n---[ ls ]"),
        "up to the top of the block, erase, then the blank row it opened with: {led:?}"
    );

    // On a screen the last command blanked there is no lead and no gap above — the top of a cleared
    // screen is not something that needs separating from what is not there.
    assert!(
        !transcript(0, 0, &["ls".into()], "-", 60, "", Marks::default())
            .starts_with("\r\x1b[K\x1b[C\x1b[J\r\r\n")
    );

    // The rule and the brackets are styled; the command between them is not — it is either what
    // was typed or what another program drew, and neither wants a second opinion.
    let painted = transcript(0, 0, &["ls".into()], "-", 60, "\x1b[2m", Marks::default());
    assert!(
        painted.contains("\x1b[2m---[ \x1b[0mls\x1b[2m ]"),
        "{painted:?}"
    );
}

/// A command too wide for the row still gets its brackets, with nothing but the lead-in left of the
/// rule that would have trailed it.
#[test]
fn a_long_command_loses_the_rule_and_not_itself() {
    let long = "cargo test --all-targets --no-run";
    let out = transcript(0, 0, &[long.to_string()], "-", 10, "", Marks::default());
    assert!(out.contains(long), "the command is never cut: {out:?}");
    assert!(!out.contains("--------"), "no room to trail it: {out:?}");
}

/// **Every row of a multi-line command gets its own brackets.** A paste, a continuation, a heredoc —
/// each line was typed at a prompt, so each carries the same mark. Only the first has the rule
/// leading into it: repeated down the block it would read as three commands rather than one.
#[test]
fn every_row_is_bracketed_and_only_the_first_has_a_rule() {
    let rows = framed(
        &rows_of("for f in *.rs; do\necho \"$f\"\ndone"),
        "-",
        40,
        "",
        "",
    );
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0], "---[ for f in *.rs; do ]----------------");
    assert_eq!(rows[1], "   [ echo \"$f\" ]");
    assert_eq!(rows[2], "   [ done ]");

    // The rule is the first row's alone, and the rest hang from where it stopped.
    let indent = |row: &str| row.len() - row.trim_start().len();
    assert_eq!(indent(&rows[1]), indent(&rows[2]), "the brackets line up");
    assert_eq!(
        indent(&rows[1]),
        rows[0].find("[ ").expect("a bracket"),
        "and they line up under the first one"
    );

    // A long line takes the whole row: it loses the lead-in, never itself, and the rows under it
    // then start at the margin because there is nowhere else for them to start.
    let long = framed(
        &rows_of("a-very-long-command-that-fills-the-row\nsecond"),
        "-",
        12,
        "",
        "",
    );
    assert_eq!(long[0], "---[ a-very-long-command-that-fills-the-row ]");
    assert_eq!(long[1], "   [ second ]");
}

/// A rule is a *unit* repeated to the width, because two characters in the corner is not a rule.
#[test]
fn a_rule_is_repeated_to_the_width_and_cut() {
    assert_eq!(fill_width("- ", 7), "- - - -");
    assert_eq!(fill_width("-", 3), "---");
    assert_eq!(fill_width("", 10), "", "nothing repeats to nothing");
    assert_eq!(fill_width("-", 0), "", "and no width is no rule");
    // Counted in characters, not bytes: a box-drawing rule is three bytes a cell.
    assert_eq!(fill_width("─", 4).chars().count(), 4);
}

/// One row per line, as `ending` splits a command before the renderer sees it.
fn rows_of(command: &str) -> Vec<String> {
    command.split('\n').map(str::to_string).collect()
}

/// **The frame closes with the time the line was run.** A transcript is drawn between Enter and the
/// command starting, so nothing about how the command went exists yet — what does is the moment,
/// which is what makes a scrollback readable later.
///
/// The same run of rule leads into the command as trails the stamp, so the row reads as a rule with
/// a bracket let into each end.
#[test]
fn a_frame_closes_with_the_time() {
    let rows = framed(&rows_of("ls"), "-", 27, "", "14:54:48");
    assert_eq!(rows[0], "---[ ls ]---[ 14:54:48 ]---");
    assert_eq!(
        crate::prompt::printed_width(&rows[0]),
        27,
        "the stamp comes out of the rule, not out of the width"
    );

    let long = framed(&rows_of("cargo test"), "-", 40, "", "09:00:01");
    assert!(long[0].starts_with("---[ cargo test ]"), "{:?}", long[0]);
    assert!(long[0].ends_with("[ 09:00:01 ]---"), "{:?}", long[0]);

    // No stamp is no bracket at all, rather than an empty one.
    let bare = framed(&rows_of("ls"), "-", 20, "", "");
    assert_eq!(bare[0], "---[ ls ]-----------");

    // The rows of a multi-line command hang under the command's bracket, not the stamp's.
    let two = framed(&rows_of("one\ntwo"), "-", 30, "", "14:54:48");
    assert_eq!(
        two[1].find("[ ").expect("a bracket"),
        two[0].find("[ one").expect("a bracket"),
        "the brackets line up under the first one"
    );
}
