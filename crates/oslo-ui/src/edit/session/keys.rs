//! What a key was bound to, and what pressing it did.
//!
//! Two enums either side of the session: [`Bound`] is what the keymap resolved a chord *to*, and
//! [`Step`] is what the session did *about it*. They are separate because most bindings are handled
//! where they are read and a few cannot be — opening a widget needs the terminal, which belongs to
//! the loop outside — so `Step` is how the session says "this one is yours".

use super::Assist;

/// A binding the config asked for, which the session performs instead of its default.
///
/// Named for the effect rather than the key, because the same effect can be reached from a chord,
/// a config entry or a default — and the loop performing it should not care which.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bound {
    /// Switch the prompt between shell and Lua.
    ToggleLanguage,
    ClearScreen,
    SearchHistory,
    /// Take the whole ghost suggestion.
    AcceptHint,
    /// Take one word of it.
    AcceptHintWord,
    Interrupt,
    Complete,
    /// Open the tab finder. Like completion, it wants the terminal to itself, so the session only
    /// says so and the outer loop does it.
    OpenScratch,
    /// Open the macro manager, on the same terms.
    OpenMacros,
    /// Hand the line to `$EDITOR`. On the same terms again: the program wants the terminal.
    EditExternally,
    /// Replace the glob under the cursor with every match.
    ExpandGlob,
    /// Show what the glob under the cursor matches.
    ListGlob,
    /// `alt-.` / `alt-_`: the previous command's last argument; again, the one before's.
    YankLastArg,
    /// `alt-<`: the oldest history entry.
    HistoryFirst,
    /// `alt->`: back to the line being composed.
    HistoryLast,
    /// `alt-#`: comment the line out and run it — kept in history, and it does nothing.
    InsertComment,
    /// A Lua function, by the key's name.
    Lua(String),
}

/// What a keypress did to the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Keep editing. `redraw` is false for a key that changed nothing, so an unbound chord does
    /// not repaint the row.
    Continue { redraw: bool },
    /// Enter. `erase` runs the line without ever showing it — see [`crate::editor::Answer::erase`].
    Accept { erase: bool },
    /// Ctrl-C.
    Interrupted,
    /// Ctrl-D on an empty line.
    Eof,
    /// Ctrl-L: the screen should be cleared before the next draw.
    ClearScreen,
    /// Shift-Tab: the prompt should switch between shell and Lua. Handled by the read loop, which
    /// is the only thing that knows what a language *is*.
    ToggleLanguage,
    /// Open the completion modal through the outer loop's shared input reader.
    OpenCompletion { backwards: bool },
    /// Open the tab finder through the outer loop, which owns the terminal the widget needs.
    OpenScratch,
    /// Open the macro manager, likewise.
    OpenMacros,
    /// Hand the line to `$EDITOR` and take back what it wrote.
    EditExternally,
    /// Expand the glob under the cursor in place.
    ExpandGlob,
    /// List the glob's matches below the line, through the outer loop's input reader.
    ListGlob,
}

/// Carrying out a binding lives here, beside the two enums it maps between, rather than in the
/// state machine: it is a translation table and nothing else, and reading it next to `Bound` and
/// `Step` is the only way to see that every case is covered.
impl super::Session {
    /// Carry out a binding the config asked for.
    pub(super) fn perform(
        &mut self,
        bound: Bound,
        assist: &mut dyn Assist,
        chain: Option<super::recall::LastArg>,
    ) -> Step {
        let changed = |yes: bool| Step::Continue { redraw: yes };
        match bound {
            Bound::ToggleLanguage => Step::ToggleLanguage,
            Bound::ClearScreen => Step::ClearScreen,
            Bound::Interrupt => Step::Interrupted,
            Bound::Complete => Step::OpenCompletion { backwards: false },
            Bound::OpenScratch => Step::OpenScratch,
            Bound::OpenMacros => Step::OpenMacros,
            Bound::EditExternally => Step::EditExternally,
            Bound::ExpandGlob => Step::ExpandGlob,
            Bound::ListGlob => Step::ListGlob,
            Bound::YankLastArg => changed(self.yank_last_arg(chain, assist)),
            Bound::HistoryFirst => changed(self.recall_oldest(assist)),
            Bound::HistoryLast => changed(self.recall_newest(assist)),
            Bound::InsertComment => self.insert_comment(),
            Bound::SearchHistory => match assist.search_history(&self.buffer.text()) {
                Some(line) => {
                    let end = line.chars().count();
                    self.buffer.set(&line, end);
                    changed(true)
                }
                None => changed(false),
            },
            // A correction falls to the same key under the same rule as Right — the two are drawn
            // in the same place and never at once. Without the second half, a config that named its
            // own accept key got half of what the key is documented to accept.
            Bound::AcceptHint => changed(self.take_hint(true, assist) || self.take_repair(assist)),
            Bound::AcceptHintWord => changed(self.take_hint(false, assist)),
            Bound::Lua(name) => {
                match assist.lua_key(&name, &self.buffer.text(), self.buffer.cursor()) {
                    Some(placed) => {
                        self.buffer.set(&placed.text, placed.cursor);
                        // `submit = true` is zsh's `bindkey -s '…\n'`: the key runs the line
                        // rather than only typing it.
                        if placed.submit {
                            Step::Accept {
                                erase: placed.erase,
                            }
                        } else {
                            changed(true)
                        }
                    }
                    None => changed(false),
                }
            }
        }
    }
}
