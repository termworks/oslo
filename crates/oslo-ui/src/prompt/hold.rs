//! Keeping the prompt alive while something else runs.
//!
//! A command normally takes the screen with it: the editor has handed the line back, so the loop
//! that ticks an animated segment, services the background and repaints is gone until the command
//! ends. That is right almost always — a file browser drawn inline owns the terminal, and anything
//! oslo wrote over it would be damage.
//!
//! **Except when the command draws somewhere else.** A browser opened in a terminal mux's float
//! leaves oslo's own screen sitting there with a prompt on it, and that prompt then spends the whole
//! visit frozen: a spinner stops turning, and a directory the browser moves the shell to — over the
//! control socket, while you navigate — is not shown until you come back.
//!
//! So a command may be marked as not owning the terminal, and while it runs [`pump`] is called in
//! its wait loop. What it does is the editor's loop with the keyboard taken out: service the
//! background, re-render if that changed anything, redraw where the prompt is.
//!
//! # Why the renderer is a `fn` and not a closure
//!
//! Building a prompt means running Lua, which is `oslo-runtime`'s, and by the time a command is
//! waiting the closure the editor was given has been dropped. A plain function pointer can be
//! registered once for the session and reach the interpreter through the thread-local the runtime
//! already keeps for exactly this — hooks fire from places with no engine in hand the same way.

use std::sync::atomic::{AtomicBool, Ordering};

/// Renders the prompt as it stands now: the left and the right. `None` when it cannot be built —
/// no interpreter on this thread, or the shell state held by something else.
type Render = fn() -> Option<(String, String)>;

thread_local! {
    static RENDER: std::cell::Cell<Option<Render>> = const { std::cell::Cell::new(None) };

    /// Which row of its own block the cursor is on, carried between repaints.
    ///
    /// **The same number the editor carries between keystrokes**, and for the same reason. A block
    /// is redrawn by going back to its first row and writing it again — so the redraw has to be
    /// told where the cursor is *within* it, and a repaint that always claimed "the first row"
    /// starts one row lower every time, because that is where its own last draw left the cursor.
    ///
    /// Eight spinner frames while a browser was open therefore walked the prompt eight rows down
    /// the screen. Nothing about the prompt was wrong; it was drawn eight times, each one lower.
    static AT_ROW: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Whether a prompt is on screen to keep alive.
///
/// **Not the same question as "is a command running".** A command run from a script, from `-c`, or
/// before the first prompt has nothing to repaint, and a pump that drew one there would put a
/// prompt in the middle of somebody's output.
static SHOWING: AtomicBool = AtomicBool::new(false);

/// Register how to build a prompt. Called once, by startup.
pub fn renders_with(render: Render) {
    RENDER.with(|slot| slot.set(Some(render)));
}

/// Say whether a prompt is on screen and worth keeping alive.
pub fn showing(yes: bool) {
    SHOWING.store(yes, Ordering::Relaxed);
    // A new prompt is a new block, drawn from its first row by whoever drew it.
    AT_ROW.with(|at| at.set(0));
}

/// One turn of the editor's loop, with the keyboard taken out.
///
/// Answers whether it drew, so a caller can tell a wait that did something from one that did not.
///
/// **Redrawn in place, the way a keystroke redraws it.** The block is written again from its own
/// first row, so the redraw has to be told which row of it the cursor is on — the number carried
/// between repaints. A key bound with `erase` parks the cursor at the top of the block it left, so
/// the first repaint starts there and each one after starts where the last finished.
pub fn pump(cols: usize) -> bool {
    if !SHOWING.load(Ordering::Relaxed) {
        return false;
    }
    let Some(render) = RENDER.with(|slot| slot.get()) else {
        return false;
    };
    // The same servicing the input wait does: timers, a finished job, and a directory a peer asked
    // for over the control socket. It is what makes this worth doing at all.
    let serviced = oslo_base::background::service();
    let ticked = crate::prompt::tick_due();
    if !serviced && !ticked {
        return false;
    }
    let Some((left, right)) = render() else {
        return false;
    };
    let marks = crate::marks::block_marks();
    let placed = crate::edit::layout::place(&crate::edit::layout::Row {
        prompt: &left,
        text: "",
        plain: "",
        cursor: 0,
        hint: "",
        right: &right,
        cols,
        lead: crate::transcript::lead(),
        prompt_mark: &marks.prompt,
        input_mark: &marks.input,
    });
    let frame = crate::edit::screen::redraw(
        AT_ROW.with(|at| at.get()),
        &placed.text,
        crate::edit::screen::At {
            rows: placed.rows,
            cursor_row: placed.cursor_row,
            cursor_col: placed.cursor_col,
        },
    );
    use std::io::Write;
    let mut out = std::io::stdout();
    let _ = out.write_all(frame.as_bytes());
    let _ = out.flush();
    AT_ROW.with(|at| at.set(placed.cursor_row));
    true
}

/// Give the block back the way it was found: cursor at its first row.
///
/// **The handoff, and it is not optional.** Whoever draws next — the editor, for the prompt that
/// follows the command — starts from the row the cursor is on and writes the block from there. A
/// pump that stopped with the cursor one row *into* its own block therefore had the next prompt
/// drawn one row lower, and the screen grew by one per command however carefully each repaint had
/// been placed.
///
/// **And the row is measured here, not counted.** This is the one moment where the count is known
/// to be unreliable: something else has just had the terminal, and the number carried between
/// repaints describes a screen that may have moved underneath it. So the terminal is asked — see
/// [`crate::term::anchor`] — and only where nothing answers does the count stand in.
///
/// Nothing is erased. The rows are still the block's, and the next draw overwrites them.
pub fn settle() {
    if !SHOWING.load(Ordering::Relaxed) {
        return;
    }
    let at = measured().unwrap_or_else(|| AT_ROW.get());
    AT_ROW.set(0);
    if at == 0 {
        return;
    }
    use std::io::Write;
    let mut out = std::io::stdout();
    let _ = out.write_all(crate::edit::screen::park(at).as_bytes());
    let _ = out.flush();
}

/// What the terminal says, and `None` on any terminal that does not keep prompt marks.
///
/// Bytes that came back and were not the answer are put where the next editor session picks them
/// up: something typed while a browser was closing is a keystroke, not noise, and the query has no
/// business eating it.
fn measured() -> Option<usize> {
    if !nix::unistd::isatty(nix::libc::STDIN_FILENO).unwrap_or(false) {
        return None;
    }
    let (rows, pending) = crate::term::anchor::rows_above(nix::libc::STDIN_FILENO);
    crate::term::query::preserve_startup_input(pending);
    rows
}

#[cfg(test)]
#[path = "hold/tests.rs"]
mod tests;

/// How the last command to finish ended.
///
/// **Here because the held prompt is what needs it.** A prompt redrawn while a command runs was
/// drawn with this status and has to keep saying it: the command running now has not ended and has
/// nothing of its own to report. Every other prompt is handed the status by the loop that ran the
/// command and never asks.
///
/// Zero before anything has run, which is what `$?` reads as at a fresh prompt.
static LAST_STATUS: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

/// Record how a command ended. Called once per command, by the loop that ran it.
pub fn command_ended(status: i32) {
    LAST_STATUS.store(status, Ordering::Relaxed);
}

/// The status a prompt drawn mid-command should still be reporting.
pub fn last_status() -> i32 {
    LAST_STATUS.load(Ordering::Relaxed)
}
