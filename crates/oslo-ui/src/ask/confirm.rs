//! `ui confirm` — yes or no, as an exit status.
//!
//! The answer is the status, not the output: `ui confirm "go on?" && do_it` is the shape every
//! script wants, and printing `yes` to stdout for the caller to compare against would be a worse
//! interface wearing the same clothes.
//!
//! Two buttons rather than a `[y/N]` prompt, because a button you can see is selected tells you
//! what Enter will do. `y` and `n` still work, since that is what fingers already know.
//!
//! Laid out as gum lays it out — question, blank, buttons, blank, keys — rather than crammed onto
//! one row. Three kinds of text on one line read as a sentence that does not parse:
//! `carry on?  next  stop here  ←→ choose • enter confirm`.

use super::{Answer, Inline};
use crate::term::{Key, Keys, Restore, Screen};
use crate::theme;

/// How `confirm` is asked.
#[derive(Debug, Clone)]
pub struct Confirm {
    pub question: String,
    pub yes: String,
    pub no: String,
    /// Which button starts selected, and the answer when there is no terminal.
    ///
    /// Defaults to **no**. A confirm is asked before something you cannot undo, and a prompt that
    /// starts on `yes` turns a reflexive Enter into the destructive answer.
    pub default: bool,
    /// The legend, the border, the screen and where on it. See `super::chrome`.
    pub chrome: super::chrome::Chrome,
}

impl Default for Confirm {
    fn default() -> Self {
        Confirm {
            question: "Are you sure?".to_string(),
            yes: "Yes".to_string(),
            no: "No".to_string(),
            default: false,
            chrome: super::chrome::Chrome::default(),
        }
    }
}

pub fn confirm(spec: &Confirm) -> Answer<bool> {
    let ui = theme::current().ui;
    let depth = theme::depth();

    let Some(raw) = Restore::enter(Screen::Inline) else {
        return Answer::Given(spec.default);
    };

    let mut yes = spec.default;
    let mut keys = Keys::on(raw.fd());
    let mut panel = Inline::with_chrome(spec.chrome.clone());

    loop {
        // The chosen button takes the accent as a *background*, which is what makes it read as a
        // button rather than as coloured text.
        let button = |label: &str, on: bool| {
            let style = if on {
                theme::Style {
                    bg: ui.accent.fg,
                    fg: None,
                    bold: true,
                    ..theme::Style::default()
                }
            } else {
                ui.muted
            };
            style.paint(&format!(" {label} "), depth)
        };
        // gum's shape: the question, a blank row, the buttons, a blank row, the keys. Five rows
        // rather than one, because a question, the thing you are answering it with, and the list
        // of keys are three different kinds of text — run together on one line they read as a
        // sentence that does not parse.
        let frame = format!(
            "\r\n\r\x1b[K {}\r\n\r\x1b[K\r\n\r\x1b[K  {}  {}",
            ui.question.paint(&spec.question, depth),
            button(&spec.yes, yes),
            button(&spec.no, !yes),
        );
        panel.draw(
            &frame,
            &[("←→", "choose"), ("y/n", "answer"), ("enter", "confirm")],
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
                // Erased rather than echoed — `confirm`'s answer is its exit status, so there is
                // nothing to leave behind. See `input` for why nothing here echoes.
                panel.close();
                return Answer::Given(yes);
            }
            // The letters answer, as the legend says — that is what people type without looking,
            // and a `y` that only moved the highlight left the question waiting for an Enter.
            Key::Char('y') | Key::Char('Y') => {
                panel.close();
                return Answer::Given(true);
            }
            Key::Char('n') | Key::Char('N') => {
                panel.close();
                return Answer::Given(false);
            }
            Key::Left | Key::Right | Key::ToggleScope | Key::BackTab => yes = !yes,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A confirm defaults to **no**: it is asked before something irreversible, and a reflexive
    /// Enter must not be the destructive answer.
    #[test]
    fn the_default_is_no() {
        assert!(!Confirm::default().default);
    }

    /// With no terminal the default is the answer, so a script that confirms still runs headless.
    #[test]
    fn without_a_terminal_the_default_answers() {
        assert_eq!(confirm(&Confirm::default()), Answer::Given(false));
        let yes = Confirm {
            default: true,
            ..Confirm::default()
        };
        assert_eq!(confirm(&yes), Answer::Given(true));
    }
}
