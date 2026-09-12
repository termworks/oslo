//! Turning `oslo.keys` into bindings the line editor understands.
//!
//! Keys are named — `ctrl-r`, `shift-tab`, `f2` — rather than written as escape sequences,
//! because the sequence a key produces depends on the terminal, which is the thing a user
//! rebinding it is usually trying to work around.
//!
//! An unrecognised key or action is *reported*, never ignored. A binding that silently does
//! nothing is indistinguishable from a shell that ignores the config, and it is the failure mode
//! that costs the most time to diagnose.

/// What a bound key does.
///
/// A fixed list rather than an open one: an action nothing answers to is a typo, and the point of
/// naming them is that the shell can say so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Switch the prompt between shell and Lua.
    ToggleLanguage,
    ClearScreen,
    HistorySearchBackward,
    /// Take the whole ghost suggestion.
    AcceptSuggestion,
    /// Take one word of it.
    AcceptSuggestionWord,
    Interrupt,
    Complete,
    /// Hand the line to `$EDITOR` and take back what it wrote.
    EditExternally,
    /// Replace the glob under the cursor with every match, quoted — bash's `glob-expand-word`.
    ExpandGlob,
    /// Show what the glob under the cursor matches, leaving the line alone.
    ListGlob,
    /// A function the config supplied. The function itself lives in [`super::editor`]; this only
    /// records that the key has one, because an `Action` has to stay plain data.
    LuaHandler,
    /// **Unbind.** `oslo.keys["shift-tab"] = "none"` makes the key do nothing at all.
    ///
    /// Needed because a binding table can only ever *add*, and some of oslo's keys are bound
    /// before any config runs — Shift-Tab toggles the language whether or not anybody asked. This
    /// is the only way to say no to one, and saying no has to be possible or the default is a
    /// decision the user cannot revisit.
    Nothing,
}

/// The action a name stands for, for callers outside `oslo.keys` that bind by name.
pub fn action(name: &str) -> Option<Action> {
    Action::parse(name)
}

impl Action {
    fn parse(name: &str) -> Option<Action> {
        match name {
            "toggle-language" | "toggle-mode" => Some(Action::ToggleLanguage),
            "clear-screen" => Some(Action::ClearScreen),
            super::editor::ACTION => Some(Action::LuaHandler),
            "history-search" | "history-search-backward" => Some(Action::HistorySearchBackward),
            "accept-suggestion" => Some(Action::AcceptSuggestion),
            "accept-suggestion-word" | "accept-word" => Some(Action::AcceptSuggestionWord),
            "interrupt" => Some(Action::Interrupt),
            "complete" => Some(Action::Complete),
            // readline calls this `edit-and-execute-command`; oslo's does not execute, so it is
            // named for the half it does.
            "edit-line" | "edit-command-line" => Some(Action::EditExternally),
            "expand-glob" | "glob-expand-word" => Some(Action::ExpandGlob),
            "list-glob" | "glob-list-expansions" => Some(Action::ListGlob),
            "none" | "nothing" => Some(Action::Nothing),
            _ => None,
        }
    }
}

/// Whether `name` is a key oslo can recognise, in the spelling a config writes.
///
/// A *name* check, not a parse into an editor event: the editor is oslo's own now and matches on
/// these names directly. This exists so a typo in `oslo.finder.key` is reported next to the line
/// that wrote it rather than leaving the finder quietly unreachable.
pub fn is_key_name(name: &str) -> bool {
    canonical(name).is_some()
}

/// The one spelling of `name` that a binding is stored and looked up under, or `None` when nothing
/// can produce it.
///
/// **A binding is found by comparing strings**, against the name the editor gives the key that was
/// pressed. So every spelling a config is allowed to write has to collapse to that one name before
/// it is stored, or the binding is accepted and then never matches anything: `oslo.keys["Ctrl-R"]`
/// and `oslo.keys["backtab"]` were each read without complaint and each bound nothing at all.
///
/// `meta-` is the other name for `alt-`, and collapses to it.
///
/// **What is not here is as deliberate as what is.** `shift-<letter>` is out because a terminal
/// sends the capital rather than a modifier, and `enter`, `esc`, `backspace` and `delete` are out
/// because the editor answers to those keys itself and never offers them to a binding — the `key`
/// hook is the surface that sees them, and it exists for that reason. All six were accepted before,
/// stored, and bound nothing for the rest of the session; a refusal is reported, which is the whole
/// point of checking a name at all.
pub fn canonical(name: &str) -> Option<String> {
    let name = name.trim().to_ascii_lowercase();
    let same = |name: String| Some(name);
    match name.as_str() {
        "tab" => return same(name),
        "shift-tab" | "backtab" | "s-tab" => return Some("shift-tab".to_string()),
        "up" | "down" | "left" | "right" | "home" | "end" | "pageup" | "pagedown" | "space" => {
            return same(name);
        }
        // **Ctrl+Space, spelled the way people say it.** The editor produces this name for
        // `Key::Ctrl(' ')`; without this arm the only spelling that matched was `"ctrl- "`, with a
        // literal space, because the generic `ctrl-<one char>` rule below is what accepted it.
        // Both spellings are taken, and both collapse to this one.
        "ctrl-space" | "ctrl- " | "c-space" => return Some("ctrl-space".to_string()),
        // **Ctrl+Tab and Ctrl+Enter, which only a terminal that reports modifiers ever sends.**
        // Accepted as names because they are real chords with real bindings; on a terminal that
        // cannot report them the binding is simply never reached. That is a property of the
        // terminal, not a name that binds nothing — which is what this function refuses.
        "ctrl-tab" | "c-tab" => return Some("ctrl-tab".to_string()),
        "ctrl-enter" | "ctrl-return" | "c-enter" => return Some("ctrl-enter".to_string()),
        _ => {}
    }
    if let Some(rest) = name.strip_prefix("f")
        && rest.parse::<u8>().is_ok_and(|n| (1..=12).contains(&n))
    {
        return Some(name);
    }
    if let Some(c) = name.strip_prefix("meta-")
        && c.chars().count() == 1
    {
        return Some(format!("alt-{c}"));
    }
    ["ctrl-", "alt-"]
        .iter()
        .any(|prefix| {
            name.strip_prefix(prefix)
                .is_some_and(|c| c.chars().count() == 1)
        })
        .then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The names a config may write. Checked rather than parsed into an editor event: the editor
    /// is oslo's own and matches on these names, so what matters is that a name is *recognised*.
    #[test]
    fn keys_are_named_the_way_people_say_them() {
        for name in [
            "shift-tab",
            "backtab",
            "s-tab",
            "tab",
            "ctrl-r",
            "alt-f",
            "f2",
            "f12",
            "up",
            "pagedown",
            "space",
        ] {
            assert!(is_key_name(name), "{name} should be a key name");
        }
        // Case does not matter, because a config is written by a person.
        assert!(is_key_name("Ctrl-R"));
        assert!(is_key_name("  tab  "));
    }

    /// A name oslo cannot place is refused, so a typo in `oslo.finder.key` is reported next to
    /// the line that wrote it rather than leaving the finder quietly unreachable.
    #[test]
    fn a_name_oslo_cannot_place_is_refused() {
        for name in ["", "ctrl-", "ctrl-shift-r", "f13", "wiggle", "alt-"] {
            assert!(!is_key_name(name), "{name:?} should not be a key name");
        }
        // A terminal sends the capital for shift and a letter, so there is no key a `shift-r`
        // binding could ever match. It was accepted, stored, and silently bound nothing.
        assert!(!is_key_name("shift-r"));
        assert!(!is_key_name("shift-1"));
        assert!(is_key_name("shift-tab"), "the one shift chord that arrives");

        // **The editor answers to these itself and never offers them to a binding.** The `key` hook
        // is the surface that sees them. Accepting the names here meant a config could write
        // `oslo.keys["enter"]` and be told nothing while it did nothing.
        for name in ["enter", "return", "esc", "escape", "backspace", "delete"] {
            assert!(
                !is_key_name(name),
                "{name:?} cannot be bound, so it is not a name"
            );
        }
        // Space can be, and is listed as bindable in the line-editor page.
        assert!(is_key_name("space"));
    }

    /// **Every spelling collapses to the one the editor produces.** A binding is found by comparing
    /// strings against that name, so a name stored as written matched nothing at all: `Ctrl-R` and
    /// `backtab` were each accepted and each bound nothing for the whole session.
    #[test]
    fn a_name_is_stored_in_one_spelling() {
        assert_eq!(canonical("Ctrl-R").as_deref(), Some("ctrl-r"));
        assert_eq!(canonical("  SPACE ").as_deref(), Some("space"));
        for spelling in ["backtab", "s-tab", "Shift-Tab"] {
            assert_eq!(
                canonical(spelling).as_deref(),
                Some("shift-tab"),
                "{spelling}"
            );
        }
        // `meta-` is the other name for `alt-`, and one key arrives for both.
        assert_eq!(canonical("meta-n").as_deref(), Some("alt-n"));
        assert_eq!(canonical("alt-n").as_deref(), Some("alt-n"));
    }

    /// The action vocabulary a config binds a key to. Every spelling here is one somebody has in
    /// a config already.
    #[test]
    fn action_names_resolve() {
        assert_eq!(action("toggle-language"), Some(Action::ToggleLanguage));
        assert_eq!(action("toggle-mode"), Some(Action::ToggleLanguage));
        assert_eq!(action("clear-screen"), Some(Action::ClearScreen));
        // **Unbinding is an action**, not the absence of one. `oslo.keys["shift-tab"] = "none"`
        // has to be distinguishable from a typo, because the typo is reported and this is not —
        // and it is the only way to refuse a key oslo bound before the config ran.
        assert_eq!(action("none"), Some(Action::Nothing));
        assert_eq!(action("nothing"), Some(Action::Nothing));
        assert_eq!(
            action("history-search"),
            Some(Action::HistorySearchBackward)
        );
        assert_eq!(action("accept-suggestion"), Some(Action::AcceptSuggestion));
        assert_eq!(action("accept-word"), Some(Action::AcceptSuggestionWord));
        assert_eq!(action("interrupt"), Some(Action::Interrupt));
        assert_eq!(action("complete"), Some(Action::Complete));
        assert_eq!(action("do-a-barrel-roll"), None);
    }
}
