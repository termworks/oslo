//! What a key is called.
//!
//! Its own file because the name is a fact about the key and not about reading one: nothing here
//! touches a descriptor, a mode or a byte, and everything in `input.rs` beside it does.

use super::Key;

impl Key {
    /// oslo's name for this key, in the spelling a binding is written with.
    ///
    /// **The point of it is that a person can find out what to type.** `oslo.keys` is keyed by
    /// name, so a binding cannot be written without knowing that this key is called `ctrl-g` and
    /// not `C-g`; the `ui key` widget prints this and the question is answered. It lives on `Key`
    /// rather than beside the config that reads it because the name is a fact about the key.
    ///
    /// A superset of what `oslo.keys` will *accept*: an ordinary character has a name here, and
    /// binding one is refused, because a widget reporting what was pressed has to answer for every
    /// key while a binding may only offer names that can fire.
    ///
    /// **The names are the decoded ones.** `term` folds Ctrl-A into [`Key::Home`] before this
    /// runs, so that chord is reported as `home` — which is the name that binds it.
    pub fn name(self) -> String {
        match self {
            // Named before the general character case, or it comes back as a literal space: a
            // name that works, that nobody would guess, and that no documentation could print.
            Self::Char(' ') => "space".to_string(),
            Self::Char(c) => c.to_string(),
            Self::Ctrl(' ') => "ctrl-space".to_string(),
            Self::Ctrl(c) => format!("ctrl-{c}"),
            Self::Alt(c) if !c.is_control() => format!("alt-{c}"),
            Self::Alt(_) => "alt-backspace".to_string(),
            Self::CtrlTab => "ctrl-tab".to_string(),
            Self::Submit => "ctrl-enter".to_string(),
            Self::Accept => "enter".to_string(),
            Self::Cancel => "esc".to_string(),
            Self::Abort => "ctrl-c".to_string(),
            Self::Clear => "ctrl-u".to_string(),
            Self::Backspace => "backspace".to_string(),
            Self::Delete => "delete".to_string(),
            Self::ToggleScope => "tab".to_string(),
            Self::BackTab => "shift-tab".to_string(),
            Self::Up => "up".to_string(),
            Self::Down => "down".to_string(),
            Self::Left => "left".to_string(),
            Self::Right => "right".to_string(),
            Self::Home => "home".to_string(),
            Self::End => "end".to_string(),
            Self::PageUp => "pageup".to_string(),
            Self::PageDown => "pagedown".to_string(),
            Self::Insert => "insert".to_string(),
            Self::Function(number) => format!("f{number}"),
            Self::Resized => "resized".to_string(),
            Self::Ignored => "ignored".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The names `ui key` prints are the names a binding is written with.
    ///
    /// That is the whole contract: a name that does not bind is worse than no name, because it
    /// looks like it worked. Space is spelled out for the same reason — a literal space would be a
    /// name nobody could type and no page could print.
    #[test]
    fn a_key_is_named_the_way_a_binding_spells_it() {
        assert_eq!(Key::Ctrl('g').name(), "ctrl-g");
        assert_eq!(Key::Ctrl(' ').name(), "ctrl-space");
        assert_eq!(Key::Alt('f').name(), "alt-f");
        assert_eq!(Key::Char(' ').name(), "space");
        assert_eq!(Key::Char('a').name(), "a");
        assert_eq!(Key::Function(5).name(), "f5");
        assert_eq!(Key::BackTab.name(), "shift-tab");
        assert_eq!(Key::PageUp.name(), "pageup");
        // Folded chords come back under the name that binds them, not the one that was pressed.
        assert_eq!(Key::Clear.name(), "ctrl-u");
        // Keys another widget would obey are reported here, because they are bindable.
        assert_eq!(Key::Cancel.name(), "esc");
        assert_eq!(Key::Accept.name(), "enter");
    }

    /// No key is nameless, or `ui key` would have nothing to answer with.
    #[test]
    fn every_key_has_a_name() {
        let every = [
            Key::Char('x'),
            Key::Backspace,
            Key::Delete,
            Key::Up,
            Key::Down,
            Key::Left,
            Key::Right,
            Key::Home,
            Key::End,
            Key::PageUp,
            Key::PageDown,
            Key::Insert,
            Key::Function(1),
            Key::Accept,
            Key::Cancel,
            Key::Abort,
            Key::Clear,
            Key::ToggleScope,
            Key::BackTab,
            Key::Submit,
            Key::CtrlTab,
            Key::Resized,
            Key::Ctrl('a'),
            Key::Alt('a'),
            Key::Ignored,
        ];
        for key in every {
            assert!(!key.name().is_empty(), "{key:?} has no name");
        }
    }
}
