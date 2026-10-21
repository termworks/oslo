//! Interactive terminal initialization before the first prompt.

/// Ask the terminal again what it can do.
///
/// **Detection is one exchange with a 100 ms budget, and it happens once.** A terminal that misses
/// that window — on a machine busy enough to lose it, or one whose emulator was still starting —
/// left the shell with the degraded answer for the rest of its life. `reset` could not help: the
/// stale answer is the shell's own memory, not the terminal's state, so resetting the terminal
/// changed nothing about what the shell believed.
///
/// Called after a command that resets the terminal, because that is both the moment the old answer
/// stops being trustworthy and the moment somebody is asking for things to be put right.
pub fn renegotiate() {
    oslo_ui::term::capability::forget();
    initialize();
}

pub fn initialize() {
    oslo_ui::term::watch_for_resize();
    let mut background = oslo_ui::term::query::background_from_environment();
    let mut verified = oslo_ui::term::capability::Verified::default();
    let query_terminal = std::env::var("TERM").as_deref() != Ok("dumb");
    if query_terminal && let Some(tty) = oslo_ui::term::Tty::open() {
        let negotiated = oslo_ui::term::negotiate::on(tty.fd(), background.is_none());
        background = background.or(negotiated.background);
        verified = negotiated.verified;
        oslo_ui::term::query::preserve_startup_input(negotiated.pending);
        // A reply that misses the budget lands during the rest of startup, where the
        // line discipline would print it. The first prompt gives echo back.
        oslo_ui::term::query::hold_echo(tty.fd());
    }
    oslo_ui::term::keyboard::remember_support(verified.kitty_keyboard);
    oslo_ui::term::capability::initialize_with_verified(verified);
    oslo_ui::marks::enable(true);
    if let Some(background) = background {
        oslo_ui::theme::set_background(background);
    }
}

/// Whether a finished line was one that resets the terminal.
///
/// **A heuristic, and named so it reads as one.** The shell cannot see the bytes a program wrote, so
/// "was the terminal reset" has to be inferred from what was run. These three are what people
/// actually type for it, and the cost of being wrong is one 100 ms exchange nobody asked for —
/// against a session that stays degraded for ever when the guess is not made at all.
///
/// The first word only, so a line that merely *mentions* one of them — `echo reset`, `grep reset
/// notes.txt` — does not trigger it.
///
/// **`clear` is deliberately not here.** It clears the screen and changes no mode, so re-probing
/// after one would put a 100 ms exchange on the most frequently typed command there is, to learn
/// nothing.
pub fn resets_the_terminal(line: &str) -> bool {
    let mut words = line.split_whitespace();
    match words.next() {
        Some("reset") => true,
        // `tput reset` and `stty sane`, where the second word is what decides.
        Some("tput") => words.next() == Some("reset"),
        Some("stty") => words.next() == Some("sane"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::resets_the_terminal;

    /// The lines people actually type to put a terminal right.
    #[test]
    fn the_lines_that_reset_a_terminal_are_recognised() {
        assert!(resets_the_terminal("reset"));
        assert!(resets_the_terminal("  reset  "));
        assert!(resets_the_terminal("tput reset"));
        assert!(resets_the_terminal("stty sane"));
    }

    /// **A line that merely mentions one is not one.** The first word decides, or `grep reset log`
    /// would spend 100 ms asking the terminal about itself.
    #[test]
    fn merely_naming_one_does_not_count() {
        assert!(!resets_the_terminal("echo reset"));
        assert!(!resets_the_terminal("grep reset notes.txt"));
        assert!(!resets_the_terminal("git reset --hard"));
        assert!(!resets_the_terminal("tput cols"));
        assert!(!resets_the_terminal("stty -a"));
        assert!(!resets_the_terminal(""));
    }

    /// `clear` changes no mode, and is typed constantly.
    #[test]
    fn clearing_the_screen_is_not_resetting_the_terminal() {
        assert!(!resets_the_terminal("clear"));
    }
}
