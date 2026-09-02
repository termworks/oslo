//! The line-editing state machine and terminal loop.
//!
//! [`Session::apply`] updates editor state without terminal access. [`Assist`] supplies
//! highlighting, hints, completion and history.

use super::buffer::{Buffer, Case};
use super::keymap::{Action, action};

use super::screen;
pub use crate::term::Key;
use crate::term::{InputEvent, Keys, PasteError, Restore, Screen};
use std::io::Write;

mod answer;
pub use answer::{KeyHook, Placed};

mod assist;
pub use assist::{Assist, NoAssist};

mod preview;

mod shortcuts;

/// The line being edited, and where in history it came from.
#[derive(Debug, Default)]
pub struct Session {
    pub buffer: Buffer,
    /// Whether the key before this one was a Tab on an empty line. See [`shortcuts::tab`].
    tab_armed: bool,
    /// Vi mode, when `oslo.vi.enabled` asked for it.
    ///
    /// `None` is emacs, and then nothing vi-shaped is consulted at all. With it on, insert mode
    /// still passes every key through to the ordinary keymap — a vi user at a shell still expects
    /// `C-w` and `C-a` to work, because those are the shell's keys and not vi's.
    pub vi: Option<super::vi::Vi>,
}

impl Session {
    pub fn new(text: &str, cursor: usize) -> Session {
        let mut buffer = Buffer::new();
        buffer.set(text, cursor);
        Session {
            buffer,
            tab_armed: false,
            vi: crate::vi::enabled().then(super::vi::Vi::default),
        }
    }

    /// Return the current vi mode stored by this session.
    pub fn mode(&self) -> Option<super::vi::Mode> {
        self.vi.as_ref().map(|vi| vi.mode)
    }

    pub fn paste(&mut self, text: &str) -> Step {
        self.buffer.insert_str(text);
        Step::Continue {
            redraw: !text.is_empty(),
        }
    }

    /// Apply one key.
    pub fn apply(&mut self, key: Key, assist: &mut dyn Assist) -> Step {
        let changed = |yes: bool| Step::Continue { redraw: yes };

        // **A resize is not a keystroke and never reaches a binding.** It arrives through the key
        // reader only because that is where the editor is waiting; letting it fall through would
        // hand `oslo.keys` and vi a key nobody pressed.
        //
        // The prompt is invalidated as well as the row redrawn: a prompt that measures the
        // terminal — one with a right-aligned segment, or any external one told `$cols` — is
        // wrong at the new width, and redrawing the old string would just re-place a stale one.
        if key == Key::Resized {
            crate::prompt::invalidate();
            return changed(true);
        }

        // **The `key` hook sees the keystroke before anything else, ordinary characters included.**
        //
        // First, because a hook that cannot see a key before its binding runs cannot implement the
        // thing it exists for — deciding what a key *means*. That is also why it can answer with a
        // whole line: an observer would not need one.
        //
        // Gated on `watches_keys` so a session with no handler attached pays one atomic load per
        // keystroke rather than building the line to hand over. It is the only method on `Assist`
        // that runs for every key including the ones nobody bound, so it is the only one where
        // that distinction is worth drawing.
        if assist.watches_keys()
            && let Some(hook) = assist.key_hook(key, &self.buffer.text(), self.buffer.cursor())
        {
            return match hook {
                // Nothing happened and nothing is redrawn — the key is simply gone.
                KeyHook::Swallow => changed(false),
                KeyHook::Line(placed) => {
                    self.buffer.set(&placed.text, placed.cursor);
                    if placed.submit {
                        Step::Accept {
                            erase: placed.erase,
                        }
                    } else {
                        changed(true)
                    }
                }
            };
        }

        // **A key the config bound wins over everything, vi included.**
        //
        // Before vi rather than after, and that ordering is load-bearing: vi mode is on by
        // default, and it reads `Alt(x)` as Esc-then-`x` so that leaving insert mode at speed
        // works. That would make every `oslo.keys["alt-…"]` binding unreachable for most users —
        // an explicit binding has to beat a heuristic about what someone probably meant.
        if let Some(bound) = assist.binding(key) {
            return self.perform(bound, assist);
        }

        // Tab twice on an empty line switches language — the fallback for a terminal that cannot
        // deliver Shift+Tab. See `shortcuts::tab`.
        //
        // **After the hook and after the bindings**, both deliberately: the `key` hook is
        // documented to see every keystroke before anything acts on it, and a config that bound
        // Tab to something meant it. This is oslo's own default, and a default is the thing that
        // gives way.
        if let Some(step) = shortcuts::tab(self, key) {
            return step;
        }

        // **Right at the end of the line takes the ghost suggestion**, and moves the cursor
        // everywhere else. One key doing both jobs is fish's `forward-char`, and it is the key
        // people reach for — Tab opens the dropdown when there is a choice to make, Right says
        // "yes, that one". `hint_text` answers `None` unless the cursor is already at the end, so
        // Right mid-line is unaffected and needs no check here.
        //
        // **Not in vi's normal mode.** There `l` and Right are a motion, and a motion has to be a
        // motion — `d<Right>` deletes a character, and a Right that inserted text instead would
        // make the operator do something no vi user asked for. Insert and replace are where text
        // gets added, so that is where a key can add some.
        //
        // Above vi rather than in the keymap because vi sees the key first and would move the
        // cursor before the keymap ever ran.
        //
        // A correction is taken by the same key under the same rule, and only when there was no
        // suggestion to take — the two are drawn in the same place and never at once, so one key
        // accepting whichever is showing is the only thing that could be meant.
        if key == Key::Right
            && self.mode() != Some(super::vi::Mode::Normal)
            && (self.take_hint(true, assist) || self.take_repair(assist))
        {
            return changed(true);
        }

        // Vi gets next refusal. `Passthrough` means the key is not vi's business — insert mode,
        // or Enter in any mode — and falls through to the ordinary keymap below.
        if let Some(vi) = self.vi.as_mut()
            && let super::vi::Outcome::Handled { redraw } = vi.apply(key, &mut self.buffer)
        {
            return Step::Continue { redraw };
        }

        match action(key) {
            // A space may end an abbreviation. Tried before the space is inserted, because the
            // expansion supplies its own — the two are one step.
            Action::Insert(' ') => {
                if let Some((line, cursor)) =
                    assist.abbreviation(&self.buffer.text(), self.buffer.cursor(), Some(' '))
                {
                    self.buffer.set(&line, cursor);
                    return changed(true);
                }
                self.buffer.insert(' ');
                changed(true)
            }
            // A bracket or a quote may bring its partner with it, or step over one already
            // there. See `super::pair`; every other character falls straight through.
            Action::Insert(c) => {
                super::pair::insert(&mut self.buffer, c);
                changed(true)
            }

            Action::Left => {
                self.buffer.move_left();
                changed(true)
            }
            Action::Right => {
                self.buffer.move_right();
                changed(true)
            }
            Action::WordLeft => {
                self.buffer.move_word_left();
                changed(true)
            }
            Action::WordRight => {
                self.buffer.move_word_right();
                changed(true)
            }
            Action::Home => {
                self.buffer.move_home();
                changed(true)
            }
            Action::End => {
                self.buffer.move_end();
                changed(true)
            }

            // **One gesture made both halves, so one gesture removes both** — see `pair`.
            Action::Backspace => changed(super::pair::backspace(&mut self.buffer)),
            // **Ctrl-D is two keys in one.** On a line with text it deletes forward; on an empty
            // one it is end of input, which is how every shell has ended a session since v7.
            // Only here is the line known to be empty, which is why the keymap cannot decide it.
            Action::Delete => {
                if self.buffer.is_empty() {
                    Step::Eof
                } else {
                    changed(self.buffer.delete())
                }
            }
            Action::KillToEnd => changed(self.buffer.kill_to_end()),
            Action::KillToStart => changed(self.buffer.kill_to_start()),
            Action::KillWordLeft => changed(self.buffer.kill_word_left()),
            Action::KillWordRight => changed(self.buffer.kill_word_right()),
            Action::KillSpaceWordLeft => changed(self.buffer.kill_space_word_left()),
            Action::Yank => changed(self.buffer.yank()),
            Action::Transpose => changed(self.buffer.transpose()),
            Action::Upper => changed(self.buffer.case_word(Case::Upper)),
            Action::Lower => changed(self.buffer.case_word(Case::Lower)),
            Action::Capitalise => changed(self.buffer.case_word(Case::Title)),

            Action::Complete | Action::CompleteBack => Step::OpenCompletion {
                backwards: matches!(action(key), Action::CompleteBack),
            },
            Action::HistoryPrev => match assist.history_prev(&self.buffer.text()) {
                Some(line) => {
                    let end = line.chars().count();
                    self.buffer.set(&line, end);
                    changed(true)
                }
                None => changed(false),
            },
            Action::HistoryNext => match assist.history_next() {
                Some(line) => {
                    let end = line.chars().count();
                    self.buffer.set(&line, end);
                    changed(true)
                }
                None => changed(false),
            },
            // Said rather than done: the program wants the terminal, and the outer loop is what
            // owns it. Same shape as completion and the tab finder.
            Action::EditExternally => Step::EditExternally,

            Action::SearchHistory => match assist.search_history(&self.buffer.text()) {
                Some(line) => {
                    let end = line.chars().count();
                    self.buffer.set(&line, end);
                    changed(true)
                }
                None => changed(false),
            },

            // **Enter ends a word too, and ends it before the line is taken.** So the expansion is
            // what runs, what is written to history, and what the transcript row above the output
            // shows — all three read the buffer, and all three used to be handed the abbreviation
            // rather than the command it stands for.
            Action::Accept => {
                if let Some((line, cursor)) =
                    assist.abbreviation(&self.buffer.text(), self.buffer.cursor(), None)
                {
                    self.buffer.set(&line, cursor);
                }
                Step::Accept { erase: false }
            }
            Action::Abort => Step::Interrupted,
            Action::Eof => Step::Eof,
            Action::Redraw => Step::ClearScreen,
            // Esc on its own does nothing in emacs mode. It is not "cancel": a shell prompt has
            // nothing to cancel *to*, and abandoning the line is Ctrl-C's job.
            Action::Escape | Action::None => changed(false),
        }
    }
}

#[path = "session/outcome.rs"]
mod outcome;
pub use outcome::Outcome;

/// Read one line.
///
/// `initial` is text to start with and where to put the cursor in it — how a reopened line, a
/// history recall or a finder choice comes back.
///
/// **`render` is a function, not a string, and that is the whole reason a vi-mode indicator can
/// work.** The prompt used to be handed in already rendered, so it was fixed for the life of the
/// line: pressing Esc changed the mode and nothing could redraw it. Asking for it means the loop
/// can rebuild it — but only when [`crate::prompt::generation`] says an input
/// changed, so a prompt that runs another program still runs it once per line while typing, and
/// once more per mode change. It is never called per keystroke.
pub fn read_line(
    render: &mut dyn FnMut() -> (String, String),
    initial: (&str, usize),
    assist: &mut dyn Assist,
) -> Outcome {
    // **`TERM=dumb` turns the editor off, exactly as readline does.**
    //
    // A dumb terminal is one that cannot address a cursor, and every keystroke the editor answers
    // is answered by redrawing a row in place — so the whole mechanism has nothing to write to.
    // What it produces instead is noise: measured against a program driving oslo over a pty,
    // ~7 KB of cursor movement and repainting per command, into a stream being read as output.
    //
    // Nothing is lost that the terminal could have shown. History recall, completion, the vi
    // keymap and the ghost suggestion are all *drawn*, and a terminal that says it cannot draw has
    // said it cannot have them. bash answers the same variable the same way, so a program that
    // sets it already expects this shell.
    //
    // A *pipe* takes the same road, one line below, for the older reason: there is no terminal at
    // all. The two arrive at one reader from opposite directions — no screen, and no screen worth
    // drawing on.
    if !crate::term::draws() {
        return read_plain(&render().0);
    }
    let Some(mut raw) = Restore::enter(Screen::Line) else {
        // No terminal: the line comes off stdin with no editing, which is what a piped script
        // needs and what `read` already does elsewhere.
        //
        // No terminal, so nothing will ever redraw it — one render is all it can use.
        return read_plain(&render().0);
    };
    let mut session = Session::new(initial.0, initial.1);
    let mouse_events = raw.mouse_events();
    // **Before the first frame, or the frame lands on somebody else's row.** A command that ended
    // without a trailing newline leaves the cursor partway along a row that has its output on it,
    // and a prompt drawn from there overwrites the output outright. See `term::freshline`.
    //
    // Whatever was typed while the query was in flight is prepended to the key reader's buffer,
    // after the bytes that were already waiting — those arrived first.
    let mut pending = raw.take_pending();
    pending.extend(crate::term::freshline::ensure(raw.fd()));
    let mut keys = Keys::editor(raw.fd(), pending, mouse_events);
    let synchronized = crate::term::capability::snapshot().synchronized_output;
    let mut at_row = 0usize;
    let mut out = std::io::stderr();
    let mut drawn = false;
    let mut idle = false;
    // Whether the next frame has to be built. A key that moved nothing — a cursor already at the
    // end pressed End again, a refused vi key — used to repaint anyway, and building the frame is
    // where the hint lookup and the layout pass live. The first frame always draws.
    let mut repaint = true;
    // Rendered once up front, then only when an input to it has moved.
    let (mut prompt, mut right) = render();
    let mut seen = crate::prompt::generation();

    loop {
        // The cursor shape says which mode you are in, as fish's vi mode does. `observe` publishes
        // the mode for the prompt to read and answers with an escape only when it actually
        // changed, so an unchanged mode costs nothing per keystroke.
        //
        // **Before the prompt is rebuilt, and that order is the whole thing.** `observe` is what
        // bumps the generation, so asking first meant every mode change was noticed one keystroke
        // late: Esc moved the cursor and left the prompt saying `I`, and only the *next* key made
        // it say `N`. Pressing Esc twice appeared to work, which is what made it look intermittent
        // rather than simply delayed.
        if let Some(mode) = session.mode()
            && let Some(shape) = crate::vi::observe(mode, &crate::settings::current().vi.cursors)
        {
            let _ = out.write_all(shape.as_bytes());
        }

        // **A frame of an animation is a rebuild, not just a repaint.** The moving part is inside
        // the prompt, so the string itself has to be made again — but without saying anything
        // changed, which would drag every other segment through `git` with it. See
        // `crate::prompt::animation`.
        let ticked = crate::prompt::tick_due();
        // Cheap enough to ask every frame: one relaxed load, and equal almost always.
        let now = crate::prompt::generation();
        if now != seen || ticked {
            seen = now;
            (prompt, right) = render();
            // A prompt that rebuilt itself has to reach the screen even if the last key moved
            // nothing — an async prompt arriving is exactly that case.
            repaint = true;
        }
        let (prompt, right) = (prompt.as_str(), right.as_str());

        if repaint {
            let placed = draw(prompt, right, &session, assist, true);
            let frame = screen::redraw(at_row, &placed.text, into_at(&placed));
            let _ = out.write_all(crate::paint::Frame::new(&frame, synchronized).as_bytes());
            let _ = out.flush();
            at_row = placed.cursor_row;
        }

        // **`post-prompt`: the prompt is now on the screen.** Once per line, not once per frame —
        // the loop redraws on every keystroke, and a hook firing there would be an `on-key` with a
        // worse name. This is the first moment anything could be seen, which is what "after the
        // prompt is displayed" has to mean.
        if !drawn {
            drawn = true;
            let _ = out.write_all(crate::marks::input_start().as_bytes());
            // The cursor is shown again here, not on entry. A language switch hides it while the
            // other language's prompt is being built — otherwise it sits at column one in plain
            // sight for however long that takes — and this is the first moment there is something
            // for it to sit *in*. Unconditional, and harmless when it was never hidden.
            let _ = out.write_all(b"\x1b[?25h");
            let _ = out.flush();
            oslo_base::hooks::fire_at_here(oslo_base::hooks::at::POST_PROMPT, &[]);
        }

        // Anything that is not an ordinary key redraws; only `Step::Continue` below may say no.
        repaint = true;
        let Some(input) = next_input(&mut keys, &mut idle) else {
            return Outcome::Eof;
        };
        let key = match input {
            InputEvent::Key(key) => key,
            InputEvent::Resized => Key::Resized,
            InputEvent::Paste(text) => {
                let _ = session.paste(&text);
                continue;
            }
            InputEvent::PasteRejected(error) => {
                let reason = match error {
                    PasteError::TooLarge => "paste exceeds 1 MiB",
                    PasteError::InvalidUtf8 => "paste is not valid UTF-8",
                };
                let _ = out.write_all(format!("\r\noslo: {reason}\r\n").as_bytes());
                let _ = out.flush();
                at_row = 0;
                continue;
            }
            // Decoded, and until now dropped. A status line that dims when you look away, or a
            // plugin that pauses polling while the window is in the background, needs exactly this
            // and had no way to hear it. An observer: the focus has already changed somewhere oslo
            // does not control, so there is nothing for a handler to refuse.
            InputEvent::Focus(focused) => {
                oslo_base::hooks::fire_at_here(
                    oslo_base::hooks::at::FOCUS_CHANGE,
                    &[("focused", if focused { "1" } else { "" })],
                );
                continue;
            }
            // A prompt rebuilt itself behind the editor. `repaint` is already true and the
            // generation check at the top of the loop picks the new text up.
            InputEvent::Refreshed => continue,
            InputEvent::Mouse(event) => {
                if event.pressed
                    && event.button == crate::term::mouse::Button::Left
                    && !event.shift
                    && !event.alt
                    && !event.ctrl
                {
                    let capabilities = crate::term::capability::snapshot();
                    if capabilities.semantic_clicks {
                        let cursor = super::layout::cursor_for_cell(
                            prompt,
                            &session.buffer.text(),
                            crate::dropdown::terminal_cols(),
                            event.row,
                            event.column,
                        );
                        session.buffer.set_cursor(cursor);
                    } else if capabilities.legacy_clicks {
                        let (position, pending) = crate::term::mouse::cursor_position(raw.fd());
                        keys.extend_pending(pending);
                        if let Some((cursor_row, _)) = position {
                            let top = cursor_row.saturating_sub(at_row);
                            if let Some(row) = event.row.checked_sub(top) {
                                let cursor = super::layout::cursor_for_cell(
                                    prompt,
                                    &session.buffer.text(),
                                    crate::dropdown::terminal_cols(),
                                    row,
                                    event.column,
                                );
                                session.buffer.set_cursor(cursor);
                            }
                        }
                    }
                }
                continue;
            }
        };
        match session.apply(key, assist) {
            Step::Continue { redraw } => repaint = redraw,
            Step::OpenCompletion { backwards } => {
                if let Some((line, cursor)) = assist.complete(
                    &session.buffer.text(),
                    session.buffer.cursor(),
                    backwards,
                    &mut keys,
                ) {
                    session.buffer.set(&line, cursor);
                }
            }
            // A tab may have owned the terminal in the meantime, so the prompt is rebuilt rather
            // than the row repainted: what is on the screen now was written by something else.
            Step::OpenScratch => {
                if assist.open_scratch() {
                    crate::prompt::invalidate();
                }
                repaint = true;
            }
            // The manager owns the whole screen while it is open, so what is underneath when it
            // closes was written by something else — the prompt is rebuilt rather than repainted.
            Step::OpenMacros => {
                if assist.open_macros() {
                    crate::prompt::invalidate();
                }
                repaint = true;
            }
            // `$EDITOR` had the whole terminal, so the prompt is rebuilt rather than repainted —
            // what is on the screen now was written by something else. The cursor lands at the end
            // of whatever came back, because that is where you were when you saved.
            Step::EditExternally => {
                if let Some(line) = assist.edit_externally(&session.buffer.text()) {
                    let end = line.chars().count();
                    session.buffer.set(&line, end);
                }
                crate::prompt::invalidate();
                repaint = true;
            }
            Step::ToggleLanguage => {
                // **No blank frame here any more.** This used to erase the block and return, and
                // the caller then rendered the other language's prompt and re-entered — so the
                // screen held an empty row for however long that render took. With a prompt built
                // by running another program that is milliseconds, and it read as the whole prompt
                // flashing dark.
                //
                // The cursor still has to come home, because the caller draws from the top of the
                // block. Writing the *current* frame does that and leaves the row looking exactly
                // as it did, so there is nothing to see between the two draws.
                let placed = draw(prompt, right, &session, assist, true);
                let frame = screen::redraw(at_row, &placed.text, into_at(&placed));
                let _ = out.write_all(crate::paint::Frame::new(&frame, synchronized).as_bytes());
                // Up to the first row of the block and to column one, which is where the caller's
                // next draw begins counting from.
                if placed.cursor_row > 0 {
                    let _ = out.write_all(format!("\x1b[{}A", placed.cursor_row).as_bytes());
                }
                let _ = out.write_all(b"\r");
                let _ = out.flush();
                return Outcome::ToggleLanguage {
                    text: session.buffer.text(),
                    cursor: session.buffer.cursor(),
                };
            }
            Step::ClearScreen => {
                // Home the cursor and clear, then fall through to a normal redraw from row 0.
                let _ = out.write_all(b"\x1b[H\x1b[2J");
                at_row = 0;
                // **Said out loud, because this is the one blank screen oslo does not have to guess
                // about.** The block that redraws below opens with `transcript::lead()` blank rows,
                // and without this it opened with one — putting the prompt on row two of a screen
                // that was just cleared to get it to row one.
                crate::transcript::blanked();
            }
            // Every way out of the loop draws one last frame and leaves. See `ending::leave`.
            step @ (Step::Accept { .. } | Step::Interrupted | Step::Eof) => {
                return ending::leave(
                    step,
                    &mut session,
                    ending::Leaving {
                        out: &mut out,
                        prompt,
                        right,
                        assist,
                        at_row,
                        synchronized,
                    },
                );
            }
        }
    }
}
mod accept;
mod frame;
use frame::{draw, first_word, into_at, next_input, read_plain};

#[path = "session/keys.rs"]
mod keys;
pub use keys::{Bound, Step};

#[cfg(test)]
#[path = "session/tests.rs"]
mod tests;

#[path = "session/ending.rs"]
mod ending;
