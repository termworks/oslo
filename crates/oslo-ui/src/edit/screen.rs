//! Turning a laid-out row into the escapes that put it on the terminal.
//!
//! Pure, like everything else here: [`redraw`] takes where the cursor is now and where the frame
//! wants it, and answers with a string. No terminal, so the sequences are asserted in tests rather
//! than discovered by watching a prompt march up the screen.
//!
//! # Every move is relative
//!
//! Never `ESC 7` / `ESC 8`. There is one cursor-save slot per terminal and the multiplexer hosting
//! the session shares it, so a restore lands wherever somebody else's save left the cursor. This
//! is the same rule [`crate::paint`] follows and for the same reason.
//!
//! # Why relative moves survive scrolling
//!
//! Writing a frame at the bottom of the screen makes the terminal scroll, so the row the prompt
//! started on moves up. Every position here is measured from *the cursor*, which the scroll moves
//! too — so the arithmetic stays true. An absolute row number would not.
//!
//! # The order of the erase
//!
//! `ESC [ J` erases from the cursor to the end of the screen, so it has to happen at the top-left
//! of the block **before** anything is drawn. Doing it after — at the end of the content — runs
//! into the pending wrap: on a row filled to the last column the cursor is still *in* that column,
//! and the erase would take back the character just written.

/// The semantic marks a drawn block wears.
///
/// Carried rather than looked up, so this module stays pure. Empty strings when the terminal takes
/// no marks. Every path here reopens the block and writes new rows, so each one stamps them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Marks<'a> {
    /// `OSC 133;A`, for the block's first row.
    pub prompt: &'a str,
    /// `OSC 133;B`, for the row the command itself is on.
    pub input: &'a str,
}

/// Where the frame wants the cursor, and how tall it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct At {
    pub rows: usize,
    pub cursor_row: usize,
    pub cursor_col: usize,
}

/// The escapes that replace the block on screen with `frame`.
///
/// `from_row` is which row of the *previous* block the cursor is sitting on — 0 for the first
/// draw, since nothing has been drawn yet and the cursor is already where the prompt goes.
pub fn redraw(from_row: usize, frame: &str, at: At) -> String {
    let mut out = String::new();

    // Back to the top-left of the block. Up first, then carriage return: a `\r` on the wrong row
    // would leave the erase below to eat a line that is not ours.
    if from_row > 0 {
        out.push_str(&format!("\x1b[{from_row}A"));
    }
    out.push('\r');
    // Everything from here down is the block's, so erasing it all and redrawing is both correct
    // and the only thing that removes a frame that has become shorter.
    //
    // **Never `ESC[J` from the screen's own origin.** When the block starts at row 0 — which
    // `Ctrl-L` guarantees, since it homes the cursor and clears — an erase-to-end from there is an
    // erase of the whole screen, and a terminal keeping scrollback moves what was visible into
    // history first. One duplicate prompt line per keystroke, ten for `echo hello`.
    //
    // **And never off this row.** `ESC[B` on the last row of the screen does not move, so the
    // matching `ESC[A` landed a row *above* the block and the frame was drawn over the last line
    // of the previous command's output — invisible until the screen filled up, and then `ls` on a
    // full screen printed nothing at all. So the dodge is horizontal: clear the row, step one
    // column right, and erase from there, which is off the origin without leaving the row.
    out.push_str("\x1b[K\x1b[C\x1b[J\r");

    out.push_str(frame);

    // The content ends on the last row of the block. `\r` commits the position — with a pending
    // wrap the cursor is still on that row, and column 0 of it is unambiguous.
    out.push('\r');
    let last = at.rows.saturating_sub(1);
    if last > at.cursor_row {
        out.push_str(&format!("\x1b[{}A", last - at.cursor_row));
    }
    if at.cursor_col > 0 {
        out.push_str(&format!("\x1b[{}C", at.cursor_col));
    }
    out
}

/// The escapes that leave a finished line behind and put the cursor on a fresh row below it.
///
/// Used when Enter is pressed: the block stays on screen as part of the transcript, and the
/// command's output starts under it.
pub fn finish(cursor_row: usize, rows: usize) -> String {
    let mut out = String::new();
    // Down to the last row of the block, wherever in it the cursor happened to be.
    let last = rows.saturating_sub(1);
    if last > cursor_row {
        out.push_str(&format!("\x1b[{}B", last - cursor_row));
    }
    out.push_str("\r\n");
    out
}

/// The escapes that leave the cursor at the top of the block without touching what is drawn there.
///
/// The counterpart to [`finish`], for a line nobody typed. It clears nothing: the prompt stays on
/// screen for as long as the command runs — which is the point when the command opens a floating
/// pane beside it — and the *next* prompt is drawn over these same rows, because [`redraw`] erases
/// from the top of the block before it draws.
///
/// An earlier version cleared here instead. That took the shell off the screen the instant the key
/// was pressed and left a hole until the browser exited.
///
/// **What the command prints therefore lands on the prompt.** True, and the reason this is opt-in:
/// a key bound to something that prints wants [`finish`], which keeps the line as the record of
/// what produced the output below it.
pub fn park(cursor_row: usize) -> String {
    let mut out = String::new();
    if cursor_row > 0 {
        out.push_str(&format!("\x1b[{cursor_row}A"));
    }
    out.push('\r');
    out
}

/// Back to the top of the block, erase it, and put its blank rows back.
///
/// What [`transcript`] and [`given`] both open with: the prompt is being *replaced*, so the block
/// has to be taken back whole before anything is drawn over it.
///
/// **`lead` is redrawn, not inherited.** The blank rows a block opens with are part of the block —
/// `layout::place` draws them, which is what lets them be taken back — so erasing the block erases
/// them too. An ending that then started at the rule moved the whole transcript up onto the row its
/// own gap was meant to be, and the frame came out with a blank under it and none above however
/// symmetric the drawing was.
///
/// **And never `ESC[J` from the screen's own origin**, which is [`redraw`]'s rule and there for the
/// same reason: on a cleared screen the block starts at row 0, and an erase-to-end from there is an
/// erase of *the whole screen* — which a terminal keeping scrollback answers by copying what was
/// visible into history first. So the dodge is horizontal: clear the row, step one column right,
/// and erase from there.
fn reopen(cursor_row: usize, lead: usize) -> String {
    let mut out = park(cursor_row);
    out.push_str("\x1b[K\x1b[C\x1b[J\r");
    for _ in 0..lead {
        out.push_str("\r\n");
    }
    out
}

#[cfg(test)]
#[path = "screen/tests.rs"]
mod tests;

/// A finished line's transcript: the command at the head of a rule, and how the last one ended at
/// the far end of it.
///
/// ```text
/// ---[ cargo test --lib ]---------------------------------------[ 0 ]---
/// ```
///
/// The third ending a line can have, beside [`finish`] and [`park`]. The block is cleared rather
/// than kept, because the point is that the prompt is *not* what scrolls back — see
/// [`crate::settings::Transcript`].
///
/// **The command leads, because it is what the block is read for.** A column of commands down the
/// left of the scrollback is a list of what was run, found by eye at the margin where every other
/// list starts; the status of the one before it is a footnote and sits where a footnote goes.
pub fn transcript(
    cursor_row: usize,
    lead: usize,
    rows: &[String],
    unit: &str,
    cols: usize,
    style: &str,
    marks: Marks<'_>,
) -> String {
    // Everything from the top of the block down was the prompt's and is being replaced.
    let mut out = reopen(cursor_row, lead);
    out.push_str(marks.prompt);
    for (at, row) in framed(rows, unit, cols, style, &crate::transcript::stamp())
        .iter()
        .enumerate()
    {
        if at == 0 {
            out.push_str(marks.input);
        }
        out.push_str(row);
        out.push_str("\r\n");
    }
    out.push_str(BREATH);
    out
}

/// A blank row under the block.
///
/// The block sits between one command's output and the next, and without a gap it reads as another
/// line of whichever it is nearer. The row *above* is the block's own `lead`, which [`reopen`] puts
/// back after erasing — so this writes only the one below, and the two together are the gap on each
/// side. On a screen the last command blanked there is no `lead` and no gap above, which is the
/// point: the top of a cleared screen is not something to be separated from.
const BREATH: &str = "\r\n";

/// How much rule is left past the bracket. Enough to read as a rule that continues, short enough
/// that the command still ends the line.
const TAIL: usize = 3;

/// The rows of a transcript, laid out but not yet placed on the screen.
///
/// One row per line of the command — a paste, a continuation, a heredoc — each in its own brackets,
/// and only the first carrying the rule that leads into it:
///
/// ```text
/// ---[ for f in *.rs; do ]---------------------------------[ 0 ]---
///    [ echo "$f" ]
///    [ done ]
/// ```
///
/// **Brackets on every row rather than a tree.** A stem says "this belongs to the thing above",
/// which is what output does; a bracket says "this is a command", which is what these are. Every
/// row of a multi-line command was typed at a prompt, so every row gets the same mark.
///
/// The rule is the first row's alone: repeated down the block it would read as three commands
/// rather than one, and the block's job is to be found by eye in a screen of output. One lead-in
/// does that, and the rows under it hang from where it stopped.
///
/// Split out from the drawing so the arithmetic can be checked without a terminal, which is the
/// same reason everything else in this file is a pure function.
fn framed(rows: &[String], unit: &str, cols: usize, style: &str, stamp: &str) -> Vec<String> {
    let paint = |text: &str| match style.is_empty() {
        true => text.to_string(),
        false => format!("{style}{text}\x1b[0m"),
    };

    let (first, rest) = match rows.split_first() {
        Some(split) => split,
        None => return Vec::new(),
    };

    // The run of rule that leads into the command. The same length trails the status at the other
    // end, so the row is a rule with a bracket let into each end rather than one that starts at a
    // bracket and ends at another.
    let lead = fill_width(unit, TAIL);

    // When the line was run, at the far end of the rule. Passed in rather than read here, so the
    // arithmetic below stays a pure function of its arguments — and so a test can say what time it
    // is.
    let closed = match stamp.is_empty() {
        true => String::new(),
        false => format!("[ {stamp} ]{}", fill_width(unit, TAIL)),
    };

    // `[ ` and ` ]` are four cells the command does not get to use.
    let bracketed = crate::prompt::printed_width(first) + 4;
    let fill = cols.saturating_sub(bracketed + lead.chars().count() + closed.chars().count());

    let mut out = vec![format!(
        "{}{first}{}",
        paint(&format!("{lead}[ ")),
        paint(&format!(" ]{}{closed}", fill_width(unit, fill))),
    )];
    for line in rest {
        out.push(format!(
            "{}{}{line}{}",
            " ".repeat(lead.chars().count()),
            paint("[ "),
            paint(" ]"),
        ));
    }
    out
}

/// `unit` repeated until it reaches `cols`, cut to fit.
///
/// Counted in characters rather than bytes: a rule of `─` is three bytes a cell, and a byte count
/// would draw a third of a line.
fn fill_width(unit: &str, cols: usize) -> String {
    if unit.is_empty() || cols == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut width = 0;
    while width < cols {
        for c in unit.chars() {
            if width >= cols {
                break;
            }
            out.push(c);
            width += 1;
        }
    }
    out
}

/// Rows another program drew, put where the prompt was.
///
/// The counterpart to [`transcript`] for `oslo.transcript.command`. Only two things stay oslo's:
/// clearing the prompt, and the indent that lines a continuation row up under the first. That
/// indent cannot be the renderer's — it is where the *first* row's rule stopped, and a tool asked
/// for one row at a time has not seen the others.
///
/// It is computed the same way the built-in drawing computes its own, from the plain command and
/// the width, so the two agree as long as a renderer right-aligns its brackets the way it does.
pub fn given(
    cursor_row: usize,
    lead: usize,
    rows: &[String],
    cols: usize,
    first_command: Option<&&str>,
    marks: Marks<'_>,
) -> String {
    let indent = first_command.map_or(0, |text| {
        cols.saturating_sub(crate::prompt::printed_width(text) + 4 + TAIL)
    });
    let mut out = reopen(cursor_row, lead);
    out.push_str(marks.prompt);
    for (at, row) in rows.iter().enumerate() {
        if at == 0 {
            out.push_str(marks.input);
        } else {
            out.push_str(&" ".repeat(indent));
        }
        out.push_str(row.trim_end_matches(['\r', '\n']));
        out.push_str("\r\n");
    }
    out.push_str(BREATH);
    out
}
