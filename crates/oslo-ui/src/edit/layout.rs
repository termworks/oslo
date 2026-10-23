//! Computes the cell layout of an edited row.
//!
//! Given a prompt, the text, where the cursor is in it, a ghost hint and the terminal width:
//! how many rows the block occupies, and which row and column the cursor sits in. The caller
//! turns that into escapes with [`crate::paint`]'s discipline — reserve first, then
//! draw, never the shared save slot.
//!
//! # Cells, not characters
//!
//! A character is not a cell: `日` is two, a combining mark is none, and an emoji is two. Widths
//! come from [`crate::dropdown::display_width`], which already handles those and
//! skips SGR escapes — so a coloured prompt measures as what it prints.
//!
//! # The pending wrap
//!
//! A row filled exactly to the last column keeps a pending wrap. Row counts use
//! [`super::super::dropdown::physical_rows`] to preserve that state.

use crate::dropdown::{display_width, physical_rows};
use crate::edit::display::DisplayMap;

/// Everything the row is drawn from.
#[derive(Debug, Clone, Default)]
pub struct Row<'a> {
    /// The prompt, escapes and all. Measured by what it prints.
    pub prompt: &'a str,
    /// The line as it should appear — already highlighted, if it is going to be.
    pub text: &'a str,
    /// The same line without colour, for measuring and for slicing at the cursor.
    ///
    /// Kept separate rather than stripping `text`, because the highlighter is the thing that knows
    /// where its own escapes are and re-deriving that here would be a second, disagreeing parser.
    pub plain: &'a str,
    /// Cursor position as a **character** index into `plain`.
    pub cursor: usize,
    /// Ghost text shown after the line, already styled. Never counted as part of the answer.
    pub hint: &'a str,
    /// Drawn hard against the right edge of the first row, when it fits.
    pub right: &'a str,
    pub cols: usize,
    /// Blank rows above the prompt, drawn as **part of this block**.
    ///
    /// **Not printed before it, which is the whole point.** A block is replaced by being redrawn
    /// over from its own first row, and anything written outside it survives that — so a blank row
    /// printed ahead of the prompt left the old prompt stranded above the new one every time a key
    /// bound with `erase` ran a line. Rows the editor draws are rows the editor can take back, and
    /// that stays true whether this is one or five.
    pub lead: usize,
    /// `OSC 133;A`, drawn on the block's first row. Empty when the terminal takes no marks.
    ///
    /// Part of the frame for the same reason `lead` is: rows the editor draws are rows it takes
    /// back, and a mark belongs to the row it was written on.
    pub prompt_mark: &'a str,
    /// `OSC 133;B`, drawn where the prompt ends and the text begins.
    pub input_mark: &'a str,
}

/// Where the row's cells landed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    /// What to write, starting from column 0 of the prompt's row.
    pub text: String,
    /// Screen rows the whole block occupies, at least 1.
    pub rows: usize,
    /// Which of those rows the cursor is on, counting from the first.
    pub cursor_row: usize,
    /// Which column, counting from 0.
    pub cursor_col: usize,
}

/// How wide the prompt prints.
pub fn prompt_width(prompt: &str) -> usize {
    display_width(prompt)
}

fn advance_cells(start: usize, text: &str, cols: usize) -> usize {
    let plain = crate::dropdown::width::without_escapes(text);
    super::display::advance_cells(start, &plain, cols)
}

pub fn cursor_for_cell(
    prompt: &str,
    raw: &str,
    cols: usize,
    target_row: usize,
    target_col: usize,
) -> usize {
    let prompt_cells = advance_cells(0, prompt, cols.max(1));
    DisplayMap::new(raw).cursor_for_cell(prompt_cells, cols, target_row, target_col)
}

/// Lay the row out.
pub fn place(row: &Row) -> Placed {
    let cols = row.cols.max(1);
    let prompt_cells = advance_cells(0, row.prompt, cols);

    // Where the cursor sits, in cells from the start of the prompt. Measured from the *plain*
    // text so a colour escape between two characters cannot move it.
    let before: String = row.plain.chars().take(row.cursor).collect();
    let cursor_cells = advance_cells(prompt_cells, &before, cols);

    let line_cells = advance_cells(prompt_cells, row.plain, cols);
    let used = advance_cells(line_cells, row.hint, cols);

    let mut out = String::new();
    // `\r\n` and not `\n`: the terminal is in raw mode, so a bare newline steps down without
    // returning and the prompt would start under the end of whatever was above it.
    for _ in 0..row.lead {
        out.push_str("\r\n");
    }
    // After the lead, so the mark lands on the row the prompt is actually on. Neither mark
    // measures: both are escapes, and the cell counts above come from the text, not from `out`.
    out.push_str(row.prompt_mark);
    out.push_str(row.prompt);
    out.push_str(row.input_mark);
    out.push_str(row.text);
    out.push_str(row.hint);

    // The right prompt goes on the first row only, and only when there is a gap to put it in.
    // One column of breathing room, so it never abuts the text — and if it does not fit it is
    // simply not drawn, which is the only honest answer for something optional.
    if !row.right.is_empty() && used < cols {
        let right_cells = display_width(row.right);
        // Strictly less, so there is always at least one blank column between the line and the
        // right prompt. Equal would let them touch, which reads as one run of text.
        if right_cells < cols - used {
            out.push_str(&" ".repeat(cols - used - right_cells));
            out.push_str(row.right);
        }
    }

    // The block is as tall as the *content* needs — the hint is part of it, since it is drawn and
    // therefore occupies cells, even though the cursor never goes there.
    let rows = physical_rows(used, cols) + row.lead;

    // **The cursor is counted the way the rows are, and that is the whole of this.** A line filled
    // to exactly the width occupies *one* row — the terminal leaves the cursor in the last column
    // with a wrap pending rather than moving down — and `physical_rows` says so. Dividing gave the
    // cursor row *after* it: with 40 cells in 40 columns, `rows` was 1 and `cursor_row` was 1, a row
    // outside the block. `session` feeds that back as the next frame's `from_row`, so the redraw
    // moved up one row too many and its `ESC[J` erased the line above — the previous command's
    // output, gone on the next keystroke, at every width the line happened to be a multiple of.
    let (cursor_row, cursor_col) = match cursor_cells {
        0 => (row.lead, 0),
        n => (physical_rows(n, cols) - 1 + row.lead, (n - 1) % cols + 1),
    };

    Placed {
        text: out,
        rows,
        cursor_row,
        cursor_col,
    }
}

#[cfg(test)]
#[path = "layout/tests.rs"]
mod tests;
