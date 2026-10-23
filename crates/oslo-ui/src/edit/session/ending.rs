//! How a finished line leaves the screen.
//!
//! Split from [`super`] when that file crossed the 600-line limit, along a seam it had gained: the
//! loop draws frames while a line is being edited, and this is the one frame after it is not.

use super::Outcome;
use super::screen;

/// How a finished line leaves the screen.
///
/// Three ways, and the order they are tried in is the order of how much the caller asked for:
///
/// * `erase` — a key that *is* a command, which never wanted to be seen. See
///   [`crate::editor::Answer::erase`].
/// * `oslo.transcript.rule` — the prompt is replaced by the command between two rules. See
///   [`crate::settings::Transcript`].
/// * otherwise the line stays where it was typed and the next prompt goes under it, which is what
///   every shell has always done.
///
/// A blank line takes the plain ending whatever is configured: there is no command to frame, and a
/// pair of rules around an empty row is a worse transcript than none.
/// `None` when nothing replaces the block — the line stays where it was typed and the caller only
/// has to move past it. That distinction is the difference between a Ctrl-C that leaves one prompt
/// on screen and one that leaves two: a replacement has to be drawn over a repainted block, and a
/// repaint with nothing to replace it is simply the prompt written out a second time.
pub(super) fn ending(
    erase: bool,
    interrupted: bool,
    line: &str,
    cursor_row: usize,
) -> Option<String> {
    if erase {
        return Some(screen::park(cursor_row));
    }
    // **An abandoned empty line gives its block back.** There is no command to record and nothing
    // was typed, so the rows the prompt is standing on are still the right rows for the next one:
    // parking hands them over and `redraw` draws the next prompt over them. Moving down instead
    // left the dead prompt on screen and opened another underneath — press Ctrl-C four times and
    // there were four of them, each a snapshot of whatever the spinner was doing at the time.
    //
    // Only when it is *empty*. A line with something typed on it is the thing you might want to
    // look at or copy, so it stays and the next prompt goes below — which is every shell's answer.
    if line.is_empty() && interrupted {
        return Some(screen::park(cursor_row));
    }
    let settings = crate::settings::current();
    let rule = settings.transcript.rule.clone();
    // The rule is the switch: with none there is no transcript, whatever else is configured.
    if rule.is_empty() || line.trim().is_empty() {
        return None;
    }
    let cols = crate::dropdown::terminal_cols();
    // The same read the frame this replaces was laid out with, in the same frame — so the rows the
    // ending puts back are the rows the block actually opened with. See `screen::reopen`.
    let lead = crate::transcript::lead();
    let lines: Vec<&str> = line.split('\n').collect();

    // **The renderer draws the whole row when there is one** — rule, brackets, command and the
    // colour of all three. oslo says how wide and whether the row leads with a rule; a tool whose
    // job is how things look should not be handed only the text.
    let drawn: Option<Vec<String>> = lines
        .iter()
        .enumerate()
        .map(|(at, text)| {
            crate::transcript::rendered(&crate::transcript::Row {
                text,
                cols,
                first: at == 0,
            })
        })
        .collect();

    // The replacement stands in for the block it erases, so it carries that block's marks.
    let stamped = crate::marks::block_marks();
    let marks = screen::Marks {
        prompt: &stamped.prompt,
        input: &stamped.input,
    };
    let block = match drawn {
        // Placed as it came, but for the indent that lines a continuation row up under the first —
        // which is oslo's because only oslo knows where the first row's rule stopped.
        Some(rendered) => screen::given(cursor_row, lead, &rendered, cols, lines.first(), marks),
        // Nothing installed, or a row it declined: oslo draws them all itself.
        None => {
            let painted = crate::theme::Color::parse(&settings.transcript.style)
                .map(crate::theme::Style::fg)
                .unwrap_or(crate::theme::current().prompt.aside);
            let own: Vec<String> = lines
                .iter()
                .map(|text| format!("{}{text}", settings.transcript.prefix))
                .collect();
            screen::transcript(
                cursor_row,
                lead,
                &own,
                &rule,
                cols,
                &painted.open(crate::theme::depth()),
                marks,
            )
        }
    };
    Some(format!(
        "{}{block}{}",
        crate::transcript::mark(true),
        crate::transcript::mark(false)
    ))
}

/// Everything the last frame needs that is not the session itself.
///
/// A struct rather than seven parameters: the two ways out of the editor want exactly the same
/// set, and a list that long at two call sites is a list that drifts apart.
pub(super) struct Leaving<'a> {
    pub out: &'a mut dyn std::io::Write,
    pub prompt: &'a str,
    pub right: &'a str,
    pub assist: &'a mut dyn super::Assist,
    pub at_row: usize,
    pub synchronized: bool,
}

/// Draw the last frame and answer what the read resolved to.
///
/// **One place, because the three ways out differ in two details and used to differ in more.**
/// Accepting, Ctrl-C and Ctrl-D all draw the finished block and then its ending; what changes is
/// whether the buffer is cleared first and which ending is asked for. Ctrl-C had its own shorter
/// version of this that skipped [`ending`] entirely, so a configured `oslo.transcript.rule` — which
/// replaces a finished prompt block with one row holding the command — did not apply to an
/// abandoned line, and the whole prompt was left standing in the scrollback looking like one still
/// waiting for input.
///
/// Ctrl-D keeps the bare ending: the shell is leaving, and a transcript row is a record of a
/// command that is now never going to run.
pub(super) fn leave(step: super::Step, session: &mut super::Session, at: Leaving<'_>) -> Outcome {
    use super::Step;
    let erase = matches!(step, Step::Accept { erase: true });
    if matches!(step, Step::Accept { .. }) {
        let shape = crate::vi::back_to_insert(session.vi.is_some());
        let _ = at.out.write_all(shape.as_bytes());
    }
    // Read before the buffer is cleared. The line still runs when `erase` asked for it not to be
    // *shown*: drawing it would put `nav` on the prompt for as long as the browser is up, which is
    // the word the binding exists to spare you.
    let line = session.buffer.text();
    if erase {
        session.buffer.set("", 0);
    }

    let placed = super::draw(at.prompt, at.right, session, at.assist, false);
    let replacement = match step {
        // Ctrl-D is leaving; a transcript row records a command that will never run.
        Step::Eof => None,
        _ => ending(
            erase,
            matches!(step, Step::Interrupted),
            &line,
            placed.cursor_row,
        ),
    };

    // **The block is repainted only when something is about to replace it.**
    //
    // An accepted line is always repainted: `erase` has just emptied the buffer, and a ghost
    // suggestion drawn past the cursor is not part of what was typed. A Ctrl-C with nothing to
    // replace the block is the case that has to *not* repaint — the screen already shows exactly
    // what the reader typed, and drawing it again put a second copy of the prompt underneath the
    // first. On a two-row prompt that is unmistakable: every Ctrl-C left another one behind.
    let mut frame = String::new();
    if replacement.is_some() || matches!(step, Step::Accept { .. }) {
        frame.push_str(&screen::redraw(
            at.at_row,
            &placed.text,
            super::into_at(&placed),
        ));
    }
    // Both halves go inside the one synchronized frame, so the terminal shows the finished block
    // and its ending as one update rather than two.
    frame.push_str(&replacement.unwrap_or_else(|| screen::finish(placed.cursor_row, placed.rows)));
    let _ = at
        .out
        .write_all(crate::paint::Frame::new(&frame, at.synchronized).as_bytes());
    let _ = at.out.flush();
    // Whatever a suggestion provider still owes was for a line that no longer exists, and a count
    // left standing is the editor polling instead of waiting for a key. This is the moment
    // `pending::settle` was written for and had no caller at — and the interrupt path, which had
    // its own ending, never settled at all.
    crate::pending::settle();

    match step {
        Step::Eof => Outcome::Eof,
        Step::Accept { .. } => Outcome::Line(line),
        _ => Outcome::Interrupted(line),
    }
}
