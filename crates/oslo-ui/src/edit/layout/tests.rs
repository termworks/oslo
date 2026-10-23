//! Cursor and row placement without a terminal.

use super::*;

fn row<'a>(prompt: &'a str, plain: &'a str, cursor: usize, cols: usize) -> Row<'a> {
    Row {
        prompt,
        text: plain,
        plain,
        cursor,
        hint: "",
        right: "",
        cols,
        lead: 0,
        ..Default::default()
    }
}

#[test]
fn a_short_line_is_one_row_and_the_cursor_follows_the_text() {
    let p = place(&row("$ ", "echo hi", 7, 80));
    assert_eq!(p.rows, 1);
    assert_eq!(p.cursor_row, 0);
    assert_eq!(p.cursor_col, 9, "2 for the prompt plus 7 typed");
    assert_eq!(p.text, "$ echo hi");
}

/// Prompt colour escapes occupy no cells.
#[test]
fn escapes_in_the_prompt_are_not_cells() {
    let plain = place(&row("$ ", "ls", 2, 80));
    let coloured = place(&row("\x1b[32m$\x1b[0m ", "ls", 2, 80));
    assert_eq!(
        coloured.cursor_col, plain.cursor_col,
        "colour changed where the cursor sits"
    );
    assert_eq!(coloured.rows, plain.rows);
}

/// A line longer than the terminal wraps, and the cursor's row is counted from cells, not
/// characters.
#[test]
fn a_wrapped_line_puts_the_cursor_on_the_right_row() {
    // 10 columns, a 2-cell prompt, 25 characters typed: 27 cells over 3 rows.
    let text: String = "x".repeat(25);
    let p = place(&row("$ ", &text, 25, 10));
    assert_eq!(p.rows, 3, "27 cells in a 10-wide terminal");
    assert_eq!(p.cursor_row, 2);
    assert_eq!(p.cursor_col, 7, "27 % 10");
}

/// **The pending wrap.** A row filled exactly to the last column has not consumed the next row
/// yet, and counting it as though it had is what walks the cursor above the prompt.
#[test]
fn a_line_that_exactly_fills_the_row_does_not_claim_the_next_one() {
    let text: String = "x".repeat(8);
    let p = place(&row("$ ", &text, 8, 10));
    assert_eq!(p.rows, 1, "10 cells in a 10-wide terminal is still one row");
    // The cursor itself is at column 10, which the terminal shows as the pending-wrap position.
    //
    // **This is what the paragraph above always meant**, and the assertion used to say row 1,
    // column 0 — the row *after* the block, which `rows` had just said does not exist. `session`
    // hands that number back as the next frame's `from_row`, so the redraw moved up one row too
    // many and its `ESC[J` erased the line above: the previous command's output vanished on the
    // next keystroke, at every width the line happened to be an exact multiple of.
    assert_eq!(p.cursor_row, 0);
    assert_eq!(p.cursor_col, 10);
}

/// A wide character is two cells, so the cursor is two columns further on — the thing that goes
/// wrong when an editor counts characters.
#[test]
fn wide_characters_take_two_columns() {
    let p = place(&row("$ ", "日本", 2, 80));
    assert_eq!(p.cursor_col, 6, "2 prompt + 2 characters at 2 cells each");
    let mixed = place(&row("$ ", "a日", 2, 80));
    assert_eq!(mixed.cursor_col, 5);
}

/// A combining mark adds no cell, so the cursor does not move for it.
#[test]
fn a_combining_mark_is_not_a_column() {
    let p = place(&row("", "e\u{0301}", 2, 80));
    assert_eq!(p.cursor_col, 1, "e plus a combining acute is one cell");
}

#[test]
fn emoji_clusters_have_terminal_width_without_splitting() {
    for grapheme in ["👍🏽", "👨‍👩‍👧‍👦", "🇳🇱", "1️⃣", "⚙️"] {
        let cursor = grapheme.chars().count();
        let p = place(&row("", grapheme, cursor, 80));
        assert_eq!(p.cursor_col, 2, "{grapheme:?}");
    }
}

#[test]
fn a_wide_cluster_wraps_before_the_final_single_cell() {
    let p = place(&row("", "a日", 2, 2));
    assert_eq!(p.rows, 2);
    // The last row is filled exactly, so the cursor is in its pending-wrap position — row 1 of a
    // two-row block, not the row after it. See `a_line_that_exactly_fills_the_row_…`.
    assert_eq!((p.cursor_row, p.cursor_col), (1, 2));
    assert_eq!(cursor_for_cell("", "a日", 2, 0, 1), 1);
    assert_eq!(cursor_for_cell("", "a日", 2, 1, 1), 2);
}

#[test]
fn explicit_newlines_advance_to_the_next_terminal_row() {
    let p = place(&row("$ ", "one\ntwo", 7, 10));
    assert_eq!((p.cursor_row, p.cursor_col), (1, 3));
    assert_eq!(p.rows, 2);
}

#[test]
fn the_cursor_can_sit_in_the_middle_of_the_line() {
    let p = place(&row("> ", "hello world", 5, 80));
    assert_eq!(p.cursor_col, 7);
    assert_eq!(p.cursor_row, 0);
}

/// The right prompt sits against the right edge with a gap, and is drawn on the first row.
#[test]
fn the_right_prompt_is_flush_right() {
    let mut r = row("$ ", "ls", 2, 20);
    r.right = "main";
    let p = place(&r);
    // "$ ls" is 4 cells, "main" is 4, so 12 spaces between them fills 20 columns exactly.
    assert_eq!(p.text, format!("$ ls{}main", " ".repeat(12)));
    assert_eq!(display_width(&p.text), 20);
}

/// **When it does not fit it is not drawn.** Wrapping the right prompt onto the next row, or
/// letting it overlap the line, are both worse than leaving it out.
#[test]
fn a_right_prompt_that_would_not_fit_is_dropped() {
    let mut r = row("$ ", "a rather long command line", 2, 30);
    r.right = "some-long-branch-name";
    let p = place(&r);
    assert!(
        !p.text.contains("some-long-branch-name"),
        "drawn anyway: {:?}",
        p.text
    );
    assert_eq!(p.rows, 1, "and it did not push the row count up");
}

/// The ghost hint is drawn and occupies cells, but the cursor never moves into it.
#[test]
fn the_hint_takes_cells_without_taking_the_cursor() {
    let mut r = row("$ ", "car", 3, 10);
    r.hint = "go build";
    let p = place(&r);
    assert!(p.text.ends_with("go build"));
    assert_eq!(
        p.cursor_col, 5,
        "the cursor stays at the end of what was typed"
    );
    assert_eq!(p.cursor_row, 0);
    // 2 + 3 + 8 = 13 cells, which is two rows of 10.
    assert_eq!(p.rows, 2, "the hint is drawn, so it is part of the block");
}

/// A zero-width terminal must not divide by zero or collapse the layout.
#[test]
fn a_zero_width_terminal_is_survived() {
    let p = place(&row("$ ", "ls", 2, 0));
    assert!(p.rows >= 1);
}

#[test]
fn an_empty_line_still_places_the_cursor_after_the_prompt() {
    let p = place(&row("oslo> ", "", 0, 80));
    assert_eq!(p.cursor_col, 6);
    assert_eq!(p.rows, 1);
    assert_eq!(p.text, "oslo> ");
}

/// The highlighted text is what gets drawn, while the plain text is what gets measured — so a
/// syntax colour cannot move the cursor either.
#[test]
fn colour_in_the_line_is_drawn_but_not_measured() {
    let r = Row {
        lead: 0,
        prompt: "$ ",
        text: "\x1b[32mecho\x1b[0m hi",
        plain: "echo hi",
        cursor: 4,
        hint: "",
        right: "",
        cols: 80,
        ..Default::default()
    };
    let p = place(&r);
    assert_eq!(p.cursor_col, 6, "2 prompt + 4 characters");
    assert!(p.text.contains("\x1b[32m"), "the colour is still drawn");
}

/// The marks ride along with the block, which is what makes them survive a repaint: the editor
/// replaces the block by erasing from its first row down, so a mark written once beside the frame
/// is erased by the next keystroke along with the row it named.
///
/// Position matters as much as presence. `A` goes after the lead, on the row the prompt is really
/// on, and `B` goes between the prompt and the text, at the column the input starts in.
#[test]
fn the_marks_are_drawn_with_the_block() {
    let r = Row {
        prompt: "$ ",
        text: "echo hi",
        plain: "echo hi",
        cursor: 7,
        hint: "",
        right: "",
        cols: 80,
        lead: 1,
        prompt_mark: "<A>",
        input_mark: "<B>",
    };
    let p = place(&r);
    assert_eq!(p.text, "\r\n<A>$ <B>echo hi");
    assert_eq!(
        p.cursor_col, 9,
        "2 prompt + 7 characters; a mark is no cells"
    );
    assert_eq!(p.rows, 2, "one lead row and the prompt's");
}

#[test]
fn clicks_map_through_wrapping_and_prompt_cells() {
    assert_eq!(cursor_for_cell("$ ", "abcdefgh", 5, 0, 0), 0);
    assert_eq!(cursor_for_cell("$ ", "abcdefgh", 5, 0, 4), 2);
    assert_eq!(cursor_for_cell("$ ", "abcdefgh", 5, 1, 2), 5);
    assert_eq!(cursor_for_cell("$ ", "abcdefgh", 5, 9, 4), 8);
}

#[test]
fn clicks_map_wide_combining_and_multiline_text() {
    assert_eq!(cursor_for_cell("", "日x", 10, 0, 0), 0);
    assert_eq!(cursor_for_cell("", "日x", 10, 0, 1), 1);
    assert_eq!(cursor_for_cell("", "e\u{301}x", 10, 0, 1), 2);
    assert_eq!(cursor_for_cell("$ ", "one\ntwo", 10, 1, 2), 6);
}

#[test]
fn clicks_clamp_around_primary_continuation_and_right_prompts() {
    assert_eq!(cursor_for_cell("PRIMARY> ", "abc", 20, 0, 0), 0);
    assert_eq!(cursor_for_cell("PRIMARY> ", "abc", 20, 0, 19), 3);
    assert_eq!(cursor_for_cell("> ", "one\ntwo", 10, 1, 0), 4);
    assert_eq!(cursor_for_cell("> ", "one\ntwo", 10, 1, 9), 7);
}

#[test]
fn click_mapping_uses_the_current_width_after_resize() {
    let line = "abcdefghij";
    assert_eq!(cursor_for_cell("$ ", line, 8, 1, 1), 7);
    assert_eq!(cursor_for_cell("$ ", line, 5, 1, 1), 4);
}

/// **Leading blanks belong to the block.** A row drawn above the prompt has to be a row the editor
/// can take back: a blank *printed* before the prompt survives the redraw that replaces it, which
/// left the old prompt stranded above the new one every time an `erase` binding ran a line.
///
/// So `lead` moves the whole block down — its height and the cursor with it — rather than being
/// written outside it. Five behaves like one, which is the point.
#[test]
fn a_lead_moves_the_block_and_not_just_the_text() {
    let flat = place(&row("$ ", "ls", 2, 40));
    for lead in [1usize, 5] {
        let mut with = row("$ ", "ls", 2, 40);
        with.lead = lead;
        let led = place(&with);
        assert_eq!(led.rows, flat.rows + lead, "the block is taller by {lead}");
        assert_eq!(
            led.cursor_row,
            flat.cursor_row + lead,
            "and the cursor moved with it"
        );
        assert_eq!(led.cursor_col, flat.cursor_col, "but not sideways");
        assert!(
            led.text.starts_with(&"\r\n".repeat(lead)),
            "carriage return and newline, not a bare newline: {:?}",
            &led.text[..8.min(led.text.len())]
        );
    }
}
