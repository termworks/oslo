//! What the shell plugs into an editing session.
//!
//! Split out of the state machine because it is a different kind of thing: [`super::Session`] is
//! logic with no dependencies, and this is the seam the shell reaches through. Everything
//! oslo-specific — highlighting, ghost hints, the completion dropdown, history, the Lua hooks —
//! arrives here, which is what lets the state machine be tested with [`NoAssist`] and nothing else.

use super::{Bound, Key, KeyHook, Placed};
use crate::term::Keys;

/// What the shell supplies to an editing session.
///
/// Every method has a default that does nothing, so a test — or an early integration — can
/// implement none of them and still get a working editor.
pub trait Assist {
    /// The line as it should be drawn. Must print the same *characters*: the layout measures the
    /// plain text and draws this, so adding or removing anything but escapes moves the cursor.
    fn highlight(&mut self, line: &str) -> String {
        line.to_string()
    }

    /// The same suggestion **without** styling, for accepting it into the line.
    fn hint_text(&mut self, _line: &str, _cursor: usize) -> Option<String> {
        None
    }

    /// Apply trusted styling to terminal-safe suggestion text.
    fn paint_hint(&mut self, text: &str) -> String {
        text.to_string()
    }

    /// The whole line as it should probably read, when what is typed looks mistyped.
    ///
    /// A *replacement* for the line rather than a continuation of it, which is what separates it
    /// from [`Assist::hint_text`] and why it is drawn differently: one is text you may be about to
    /// have, the other is the shell disagreeing with text you already have.
    fn repair_text(&mut self, _line: &str, _cursor: usize) -> Option<String> {
        None
    }

    /// Draw the correction that goes after the line, given what was typed and what it should say.
    ///
    /// **Both, because the drawing is a comparison.** Only the words that changed are marked, so
    /// this cannot be a function of the correction alone — and doing the diff here rather than in
    /// the layout keeps the editor unaware of what a word is.
    fn paint_repair(&mut self, _typed: &str, fixed: &str) -> String {
        fixed.to_string()
    }

    /// Run completion through the editor's shared terminal event stream.
    fn complete(
        &mut self,
        _line: &str,
        _cursor: usize,
        _back: bool,
        _keys: &mut Keys,
    ) -> Option<(String, usize)> {
        None
    }

    /// `expand-glob`: the word under the cursor replaced by every match, quoted.
    fn expand_glob(&mut self, _line: &str, _cursor: usize) -> Option<(String, usize)> {
        None
    }

    /// `list-glob`: what the word under the cursor matches, below the line until the next key.
    fn list_glob(&mut self, _line: &str, _cursor: usize, _keys: &mut Keys) {}

    /// Open the tab finder, and say whether the terminal was handed to something else while it
    /// was open.
    ///
    /// **The editor cannot do this itself**: a tab is a process holding a pty, which is the shell's
    /// business and not the line editor's. All the editor knows is that a key was pressed and that
    /// whatever happened may have repainted the screen.
    fn open_scratch(&mut self) -> bool {
        false
    }

    /// Open the macro manager, and answer whether the screen was taken over.
    ///
    /// The same shape and the same reason as `open_scratch`: the editor knows a key was pressed,
    /// not what a macro is or where they are kept.
    fn open_macros(&mut self) -> bool {
        false
    }

    /// Hand the line to `$EDITOR` and take back whatever comes out.
    ///
    /// `None` leaves the line exactly as it was — the editor was cancelled, failed, or there was
    /// nowhere to run one. **Not vi mode's `v`.** A long line is hard to edit on one row whichever
    /// keymap you use, so this is a key like any other and works in both.
    ///
    /// The same seam as `open_scratch`: what `$EDITOR` is, and how to run a program with the
    /// terminal handed over to it, is the shell's business. All the editor knows is that a key was
    /// pressed and that the screen probably belongs to something else now.
    fn edit_externally(&mut self, _line: &str) -> Option<String> {
        None
    }

    /// The previous history entry, given what is on the line now.
    fn history_prev(&mut self, _line: &str) -> Option<String> {
        None
    }

    fn history_next(&mut self) -> Option<String> {
        None
    }

    /// Ctrl-R. Answers a whole line to put in place, or `None` to leave things alone.
    fn search_history(&mut self, _line: &str) -> Option<String> {
        None
    }

    /// A word has been ended: expand an abbreviation if this is one.
    ///
    /// `ending` is what the keystroke would have typed — `Some(' ')` for the space that ends a
    /// word mid-line, and `None` for Enter, which ends the word by ending the line and supplies
    /// nothing. The expansion and the space are one act when there is one: `gco ` becomes
    /// `git checkout ` in a single step, so what you see is a finished command rather than a word
    /// waiting to be finished.
    ///
    /// **Enter counts.** An abbreviation that only expanded on space meant `gco⏎` ran `gco`, which
    /// is the one word an abbreviation exists so you never type — and the failure is silent, since
    /// `gco` is a perfectly good command name for the shell to report as missing.
    fn abbreviation(
        &mut self,
        _line: &str,
        _cursor: usize,
        _ending: Option<char>,
    ) -> Option<(String, usize)> {
        None
    }

    /// A key the config bound to a Lua handler. Answers the line the handler asked for.
    ///
    /// The name is oslo's spelling — `ctrl-s`, `alt-u`, `shift-tab` — so a config's key table can
    /// be looked up directly.
    fn lua_key(&mut self, _name: &str, _line: &str, _cursor: usize) -> Option<Placed> {
        None
    }

    /// oslo's name for a key, when the config could have bound it. `None` means never ask.
    fn key_name(&mut self, _key: Key) -> Option<String> {
        None
    }

    /// What the config bound this key to, if anything.
    fn binding(&mut self, _key: Key) -> Option<Bound> {
        None
    }

    /// Whether anything is attached to the `key` hook.
    ///
    /// **Asked before the line is built**, and that is the entire reason it is a separate method.
    /// [`Assist::key_hook`] needs the text and the cursor, and producing the text means collecting
    /// the buffer into a `String`. A session with no `key` handler must not pay for that on every
    /// keystroke, so the question "is anyone listening" is answered first and cheaply — oslo's
    /// implementation is one atomic load.
    fn watches_keys(&mut self) -> bool {
        false
    }

    /// The `key` hook: a Lua handler that sees every keystroke before anything else does.
    ///
    /// `None` means the handler declined — or that there was none — and the key goes on to do
    /// whatever it would have done.
    fn key_hook(&mut self, _key: Key, _line: &str, _cursor: usize) -> Option<KeyHook> {
        None
    }
}

/// An `Assist` that does nothing, for tests and for a shell that has not wired one yet.
#[derive(Debug, Default)]
pub struct NoAssist;
impl Assist for NoAssist {}
