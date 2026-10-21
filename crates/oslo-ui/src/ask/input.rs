//! `ui input` — one line, typed.
//!
//! Inline rather than full-screen: the question and the answer stay in the scrollback where you
//! can scroll back and see what you agreed to. A picker that takes the whole screen and restores
//! it leaves nothing behind, which is right for browsing history and wrong for a question a script
//! asked you.
//!
//! # What it is not
//!
//! It is not the line editor. There is no completion, no history, no syntax colour, and the
//! editing keys stop at the ones people reach for without thinking: the arrows, Home and End,
//! backspace and forward delete, and Ctrl-U. A script asking for a branch name does not need
//! oslo's whole `readline`, and hosting it here would mean two things to keep in step for the rest
//! of the shell's life.

use super::{Answer, Inline, with_caret};
use crate::term::{Key, Keys, Restore, Screen};
use crate::theme;

/// How `input` is asked.
#[derive(Debug, Clone, Default)]
pub struct Input {
    /// Shown before the cursor.
    pub prompt: String,
    /// Greyed text shown while the line is empty. Not a value — pressing Enter on it answers the
    /// empty string, which is the difference between a placeholder and a default.
    pub placeholder: String,
    /// The line starts with this, and Enter on an untouched line answers it. Also the answer when
    /// there is no terminal.
    pub default: Option<String>,
    /// Draw `•` instead of the characters. For a password, which is the one case where the
    /// scrollback keeping what you typed is exactly wrong.
    pub password: bool,
    /// Refuse to answer with an empty line.
    pub required: bool,
    /// The legend, the border, the screen and where on it. See `super::chrome`.
    pub chrome: super::chrome::Chrome,
    /// The colours the row takes, and whether it sits on a surface. See `super::look`.
    ///
    /// Only the parts of a look that mean anything to a single row: `accent` paints the prompt,
    /// `surface` tints the row, `surface_rows` turns it into a panel. There is no list here, so
    /// the stripe and the marker have nothing to colour.
    pub look: super::look::Look,
}

/// Ask for one line.
pub fn input(spec: &Input) -> Answer<String> {
    let ui = theme::current().ui;
    let depth = theme::depth();

    let Some(raw) = Restore::enter(Screen::Inline) else {
        // No terminal. A default is the script's own answer for exactly this, and is why one of
        // these in a CI pipeline need not hang.
        return match &spec.default {
            Some(value) => Answer::Given(value.clone()),
            None => Answer::NoTerminal,
        };
    };

    let mut line: Vec<char> = spec.default.clone().unwrap_or_default().chars().collect();
    let mut cursor = line.len();
    let mut keys = Keys::on(raw.fd());
    // Drawn through `Inline` like every other widget, so a border is a border here too. It used to
    // write its own row directly, which is why `ui input --border` took the flag and drew nothing.
    let mut panel = Inline::with_chrome(spec.chrome.clone());

    loop {
        let shown: String = if spec.password {
            "•".repeat(line.len())
        } else {
            line.iter().collect()
        };
        // The placeholder carries the caret too, on its first character, so an empty field still
        // shows where typing will go. The block is drawn first and the rest dimmed after it —
        // painting the whole string muted and then reversing part of it would nest the escapes.
        let body = if line.is_empty() && !spec.placeholder.is_empty() {
            let mut chars = spec.placeholder.chars();
            let first = chars.next().unwrap_or(' ');
            format!(
                "{}{}",
                with_caret(&first.to_string(), 0),
                ui.muted.paint(chars.as_str(), depth)
            )
        } else {
            with_caret(&shown, cursor)
        };
        panel.draw(
            &spec.look.one_row(&spec.prompt, &body, spec.chrome.room()),
            &[],
        );

        let Some(pressed) = keys.read() else {
            panel.close();
            return Answer::Cancelled;
        };
        match pressed {
            // An abort is a cancel here: there is an answer to decline either way.
            Key::Cancel | Key::Abort => {
                panel.close();
                return Answer::Cancelled;
            }
            Key::Accept => {
                let answer: String = line.iter().collect();
                if spec.required && answer.is_empty() {
                    // Say why, on the same row, and go on asking. Refusing silently reads as the
                    // key not having worked.
                    panel.draw(
                        &spec.look.one_row(
                            &spec.prompt,
                            &ui.error.paint("required", depth),
                            spec.chrome.room(),
                        ),
                        &[],
                    );
                    continue;
                }
                // Erased, not echoed: the caller prints the answer to stdout and that is the one
                // record of it. Echoing here as well put the answer on screen twice, once from
                // each stream — and made `x=$(ui input)` leave a stray copy behind.
                panel.close();
                return Answer::Given(answer);
            }
            Key::Char(c) => {
                line.insert(cursor, c);
                cursor += 1;
            }
            Key::Backspace => {
                if cursor > 0 {
                    cursor -= 1;
                    line.remove(cursor);
                }
            }
            Key::Delete => {
                if cursor < line.len() {
                    line.remove(cursor);
                }
            }
            Key::Left => cursor = cursor.saturating_sub(1),
            Key::Right => cursor = (cursor + 1).min(line.len()),
            Key::Home | Key::PageUp => cursor = 0,
            Key::End | Key::PageDown => cursor = line.len(),
            Key::Clear => {
                line.clear();
                cursor = 0;
            }
            // Nothing here scrolls or toggles.
            // `Ctrl`/`Alt` chords belong to the line editor, not to a one-line prompt: listed
            // rather than caught by a wildcard so a new key has to be considered here too.
            Key::Up
            | Key::Down
            | Key::Insert
            | Key::ToggleScope
            | Key::BackTab
            | Key::Submit
            | Key::CtrlTab
            | Key::Function(_)
            | Key::Ctrl(_)
            | Key::Alt(_)
            | Key::Resized
            | Key::Ignored => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With no terminal, a default is the answer and its absence is a distinct status — which is
    /// what lets the same script run under CI and at a prompt.
    #[test]
    fn without_a_terminal_the_default_answers() {
        let with = Input {
            default: Some("fallback".to_string()),
            ..Input::default()
        };
        // Under `cargo test` stdin is not a terminal, so this exercises the headless path.
        assert_eq!(input(&with), Answer::Given("fallback".to_string()));

        let without = Input::default();
        assert_eq!(input(&without), Answer::NoTerminal);
    }

    /// The status is the whole interface for `x=$(ui input) || exit`.
    #[test]
    fn the_statuses_are_distinct() {
        assert_eq!(Answer::Given(()).status(), 0);
        assert_eq!(Answer::<()>::Cancelled.status(), 1);
        assert_eq!(Answer::<()>::NoTerminal.status(), 2);
    }
}
