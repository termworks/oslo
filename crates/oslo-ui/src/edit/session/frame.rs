//! Turning the session's state into the bytes a terminal draws, and reading the key that changes
//! it next.
//!
//! Split from the loop when `session.rs` crossed the 600-line limit. These are the four things the
//! loop delegates whole rather than inlines: waiting for a key, laying out a frame, converting a
//! layout into a cursor position, and the no-terminal path that does none of it.

use super::super::{layout, screen};
use super::{Assist, Outcome, Session};
use crate::dropdown::terminal_cols;
use crate::edit::display::DisplayMap;
use crate::term::{InputEvent, Keys};

/// The next key, firing `on-idle-timeout` if the prompt sits untouched long enough.
///
/// **The blocking read is the default and stays the default.** A timed read is only asked for when
/// `oslo.misc.idle_timeout` is set *and* something is attached to the hook — otherwise the editor
/// would wake up on a timer for the rest of the session to ask a question nobody is listening for.
///
/// `reported` is what stops it firing over and over: idleness is a state you enter once, not a
/// tick. It resets the moment a key arrives, so walking away twice reports twice.
pub(super) fn next_input(keys: &mut Keys, reported: &mut bool) -> Option<InputEvent> {
    let seconds = crate::settings::current().misc.idle_timeout;
    let idle_hook = seconds != 0 && oslo_base::hooks::watched(oslo_base::hooks::at::IDLE_TIMEOUT);
    // **A blocking wait is only safe when nothing else can change the screen.** An asynchronous
    // prompt is rebuilt off this thread, and a wait with no deadline cannot notice it finishing —
    // so the fresh prompt sat in the cache until the next keystroke, and the prompt on screen went
    // on describing the command before last.
    if !idle_hook && !crate::pending::waiting() {
        return keys.read_event();
    }

    // Short enough that a prompt lands as soon as it exists, long enough to be nothing: this only
    // runs while an answer is outstanding, which is a few tens of milliseconds after a command.
    const REFRESH_SLICE_MS: i32 = 15;
    let idle_ms = seconds.saturating_mul(1000).min(i32::MAX as u64) as i32;
    let seen = crate::pending::generation();
    let mut waited: i64 = 0;
    loop {
        let refreshing = crate::pending::waiting();
        let slice = match (refreshing, idle_hook) {
            // Never longer than the moment somebody asked for a turn at, so a provider's debounce
            // expires when it said it would rather than up to a slice late.
            (true, _) => crate::pending::slice_ms(REFRESH_SLICE_MS),
            (false, true) => idle_ms,
            // Nothing left to wait for but a key, and the idle hook is not installed.
            (false, false) => return keys.read_event(),
        };
        match keys.read_event_within(slice) {
            crate::term::EventPressed::Event(event) => {
                *reported = false;
                return Some(event);
            }
            crate::term::EventPressed::Ended => return None,
            crate::term::EventPressed::Timeout => {
                // Something landed and the frame is stale — a prompt rebuilt behind the line, a
                // suggestion that had to be asked for. Hand the loop a turn so it can redraw; it is
                // not a key and nothing binds it.
                // A turn was asked for and its moment has come — a debounce expiring, which is a
                // redraw rather than a key: the frame asks the providers again on the way past.
                if crate::pending::generation() != seen || crate::pending::due() {
                    return Some(InputEvent::Refreshed);
                }
                waited = waited.saturating_add(slice as i64);
                if idle_hook && !*reported && waited >= idle_ms as i64 {
                    *reported = true;
                    oslo_base::hooks::fire_at_here(
                        oslo_base::hooks::at::IDLE_TIMEOUT,
                        &[("seconds", &seconds.to_string())],
                    );
                }
            }
        }
    }
}

/// Build the frame for the current state.
/// Lay the line out. `ghost` is whether the suggestion is drawn with it.
///
/// **Off for the last frame of a line.** The ghost is a proposal, not text you typed — so once the
/// line is finished it has to go, or the transcript shows a command that was never run. Typing
/// `cat ~/` with `lis/` suggested and pressing Enter left `cat ~/lis/` on screen above the output
/// of `cat ~/`, which is a scrollback that lies about what happened.
pub(super) fn draw(
    prompt: &str,
    right: &str,
    session: &Session,
    assist: &mut dyn Assist,
    ghost: bool,
) -> layout::Placed {
    let raw = session.buffer.text();
    let display = DisplayMap::new(&raw);
    let plain = display.plain();
    let painted = assist.highlight(plain);
    let hint = if ghost {
        assist
            .hint_text(&raw, session.buffer.cursor())
            .map(|hint| {
                let safe = DisplayMap::new(&hint);
                assist.paint_hint(safe.plain())
            })
            // **One of the three, never two.** They are drawn in the same place and are three
            // different futures for the same line: a continuation exists when what was typed is the
            // *start* of something, a preview when it holds a coordinate, a repair when it is a
            // near-miss of something else.
            //
            // The preview goes above the repair, and that order was decided by watching it:
            // `wc -l {-1:0:1}` drew `-> wc [-9]`, a guess at a word that was not a mistake, and the
            // preview never got a turn. A repair is a proposal about what you *meant*; a preview is
            // a fact about what the line will do, and it only exists when there is a coordinate to
            // be certain about. Between a guess and a fact, the fact.
            //
            // The preview is reached only here, in the drawing. `Bound::AcceptHint` asks
            // `take_hint` and `take_repair` for their own text, so no key can put one in the buffer.
            .or_else(|| coordinate(session, assist))
            .or_else(|| repair(session, assist))
            .unwrap_or_default()
    } else {
        String::new()
    };
    let marks = crate::marks::block_marks();
    layout::place(&layout::Row {
        prompt,
        text: &painted,
        plain,
        cursor: display.rendered_cursor(session.buffer.cursor()),
        hint: &hint,
        right,
        // Read every frame rather than cached, so a resized terminal lays out correctly on the
        // next keystroke without a `SIGWINCH` handler to get wrong.
        cols: terminal_cols(),
        lead: crate::transcript::lead(),
        prompt_mark: &marks.prompt,
        input_mark: &marks.input,
    })
}

/// The correction, drawn after the line as ` -> [lsblk]` rather than appended to it.
///
/// The leading space is outside the styling and the arrow is inside it: the gap belongs to the line
/// rather than to the annotation, and a reversed block that starts flush against the last character
/// typed would read as part of that word.
fn repair(session: &Session, assist: &mut dyn Assist) -> Option<String> {
    let raw = session.buffer.text();
    let fixed = assist.repair_text(&raw, session.buffer.cursor())?;
    let safe = DisplayMap::new(&fixed);
    Some(format!(" {}", assist.paint_repair(&raw, safe.plain())))
}

/// What the line's last stream coordinate will become, drawn after it as ` ↳ one.txt`.
///
/// The arrow rather than the repair's `->`: they sit in the same place and mean different things,
/// and one of them can be accepted. Painted as a hint, because it is the same kind of grey
/// not-yet-real text — see [`super::preview`] for why only the session axis can be shown.
fn coordinate(session: &Session, assist: &mut dyn Assist) -> Option<String> {
    let text = super::preview::preview(&session.buffer.text())?;
    let safe = DisplayMap::new(&text);
    Some(format!(
        " {}",
        assist.paint_hint(&format!("↳ {}", safe.plain()))
    ))
}

pub(super) fn into_at(placed: &layout::Placed) -> screen::At {
    screen::At {
        rows: placed.rows,
        cursor_row: placed.cursor_row,
        cursor_col: placed.cursor_col,
    }
}

/// A line from stdin, for when there is no terminal to edit on.
pub(super) fn read_plain(prompt: &str) -> Outcome {
    // The prompt is written only for a terminal that cannot be edited on — `TERM=dumb`, a serial
    // console, a `screen` session someone has told to be simple. Down an ordinary pipe it is not
    // written at all: the shell is being driven by a script, and a prompt interleaved with the
    // output would be noise in the middle of the data.
    if std::env::var("TERM").as_deref() == Ok("dumb") {
        print!("{prompt}");
        let _ = std::io::Write::flush(&mut std::io::stdout());
    }
    let mut line = String::new();
    match std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut line) {
        Ok(0) | Err(_) => Outcome::Eof,
        Ok(_) => {
            while line.ends_with('\n') || line.ends_with('\r') {
                line.pop();
            }
            Outcome::Line(line)
        }
    }
}

/// The first word of a suggestion, with the whitespace that follows it.
///
/// Accepting "one word" of `--example foo` should leave the cursor after the space, ready for the
/// next word — stopping before it would make the second press insert a leading space.
pub(super) fn first_word(hint: &str) -> String {
    let trimmed = hint.trim_start();
    let lead = hint.len() - trimmed.len();
    let end = trimmed
        .find(char::is_whitespace)
        .map(|at| {
            let rest = &trimmed[at..];
            at + rest.len() - rest.trim_start().len()
        })
        .unwrap_or(trimmed.len());
    hint[..lead + end].to_string()
}
