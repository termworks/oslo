//! The state machine, driven a key at a time with no terminal anywhere.
//!
//! This is the payoff of splitting `apply` from the loop: what a key does to a line can be checked
//! exhaustively, and the only thing left to get wrong on a real terminal is the drawing — which
//! `super::screen` covers separately.

use super::ending::ending;
use super::*;

/// Feed a sequence of keys and hand back the line.
fn run(start: &str, keys: &[Key]) -> (Session, Vec<Step>) {
    // The emacs keymap, explicitly rather than by default: these assert what a key does when it is
    // *not* a vi command, and a session built from the config follows whatever `oslo.vi.enabled` says.
    let mut session = Session {
        vi: None,
        ..Session::new(start, start.chars().count())
    };
    let mut assist = NoAssist;
    let steps = keys
        .iter()
        .map(|k| session.apply(*k, &mut assist))
        .collect();
    (session, steps)
}

fn typed(text: &str) -> Vec<Key> {
    text.chars().map(Key::Char).collect()
}

#[test]
fn typing_builds_a_line() {
    let (s, _) = run("", &typed("echo hi"));
    assert_eq!(s.buffer.text(), "echo hi");
    assert_eq!(s.buffer.cursor(), 7);
}

#[test]
fn editing_keys_reach_the_buffer() {
    let (s, _) = run("echo hello", &[Key::Ctrl('w')]);
    assert_eq!(s.buffer.text(), "echo ", "C-w took the last word");

    let (s, _) = run("echo hello", &[Key::Home, Key::Ctrl('k')]);
    assert_eq!(s.buffer.text(), "", "C-k from the start takes everything");

    let (s, _) = run("abc", &[Key::Left, Key::Backspace]);
    assert_eq!(s.buffer.text(), "ac");
}

/// **Ctrl-D is end of input only on an empty line.** With text it deletes forward, and getting
/// this backwards means a stray keypress closes somebody's shell.
#[test]
fn ctrl_d_is_eof_only_when_the_line_is_empty() {
    let mut empty = Session {
        vi: None,
        ..Session::new("", 0)
    };
    assert_eq!(empty.apply(Key::Delete, &mut NoAssist), Step::Eof);

    let mut has_text = Session {
        vi: None,
        ..Session::new("ls", 0)
    };
    assert_eq!(
        has_text.apply(Key::Delete, &mut NoAssist),
        Step::Continue { redraw: true }
    );
    assert_eq!(has_text.buffer.text(), "s", "it deleted forward");
}

#[test]
fn enter_accepts_and_ctrl_c_interrupts() {
    let mut s = Session {
        vi: None,
        ..Session::new("ls", 2)
    };
    assert_eq!(
        s.apply(Key::Accept, &mut NoAssist),
        Step::Accept { erase: false }
    );
    assert_eq!(s.apply(Key::Abort, &mut NoAssist), Step::Interrupted);
}

/// A key that changes nothing must not ask for a repaint — a redraw per unbound keypress is
/// visible as flicker on a slow link.
#[test]
fn a_key_that_changes_nothing_does_not_ask_for_a_redraw() {
    let mut s = Session {
        vi: None,
        ..Session::new("", 0)
    };
    for key in [Key::Ctrl('q'), Key::Alt('z'), Key::Ignored, Key::Cancel] {
        assert_eq!(
            s.apply(key, &mut NoAssist),
            Step::Continue { redraw: false },
            "{key:?} asked for a repaint"
        );
    }
    // And nor does a kill with nothing to kill.
    assert_eq!(
        s.apply(Key::Ctrl('k'), &mut NoAssist),
        Step::Continue { redraw: false }
    );
}

/// Esc alone is not "abandon the line". A shell prompt has nothing to cancel back to, and
/// Ctrl-C is the key that abandons.
///
/// **In emacs mode**, stated explicitly: with vi mode on — which is oslo's default — Esc leaves
/// insert mode, which is very much something. Building the session with `vi: None` is what makes
/// this test about the emacs keymap rather than about whichever default is current.
#[test]
fn escape_alone_does_nothing_in_emacs_mode() {
    let mut s = Session {
        vi: None,
        ..Session::new("half typed", 10)
    };
    assert_eq!(
        s.apply(Key::Cancel, &mut NoAssist),
        Step::Continue { redraw: false }
    );
    assert_eq!(s.buffer.text(), "half typed", "the line survived Esc");
}

/// An `Assist` that answers with fixed lines, to check the wiring without oslo's real machinery.
///
/// `at` counts how many steps back into history the walk has gone, so `0` means "still on the
/// line the user was typing". Modelled properly rather than as a bare index because the
/// interesting case is walking back *out* of history.
#[derive(Default)]
struct Canned {
    history: Vec<String>,
    at: usize,
    /// What was on the line when the walk started, so coming back down can restore it.
    typed: Option<String>,
    /// What the ghost suggestion would add, if anything.
    hint: Option<String>,
    /// What the line should have said, if it looks mistyped.
    repair: Option<String>,
}

impl Assist for Canned {
    /// As the real one does: no suggestion unless the cursor is at the end of the line.
    fn hint_text(&mut self, line: &str, cursor: usize) -> Option<String> {
        (cursor >= line.chars().count()).then(|| self.hint.clone())?
    }
    fn repair_text(&mut self, line: &str, cursor: usize) -> Option<String> {
        (cursor >= line.chars().count()).then(|| self.repair.clone())?
    }
    fn history_prev(&mut self, line: &str) -> Option<String> {
        let entry = self.history.get(self.at)?.clone();
        if self.at == 0 {
            self.typed = Some(line.to_string());
        }
        self.at += 1;
        Some(entry)
    }
    fn history_next(&mut self) -> Option<String> {
        match self.at {
            0 => None,
            // Stepping out of history: the line being composed comes back, not a blank.
            1 => {
                self.at = 0;
                self.typed.take()
            }
            _ => {
                self.at -= 1;
                self.history.get(self.at - 1).cloned()
            }
        }
    }
    fn history_entry(&mut self, line: &str) -> Option<String> {
        if self.at == 0 {
            return None;
        }
        let entry = self.history.get(self.at - 1)?;
        (entry == line).then(|| entry.clone())
    }
}

#[test]
fn history_walks_through_the_assist() {
    let mut a = Canned {
        history: vec!["cargo test".into(), "ls -la".into()],
        ..Canned::default()
    };
    let mut s = Session {
        vi: None,
        ..Session::new("", 0)
    };
    s.apply(Key::Up, &mut a);
    assert_eq!(s.buffer.text(), "cargo test");
    assert_eq!(s.buffer.cursor(), 10, "the cursor lands at the end");
    s.apply(Key::Up, &mut a);
    assert_eq!(s.buffer.text(), "ls -la");
    s.apply(Key::Down, &mut a);
    assert_eq!(s.buffer.text(), "cargo test");
}

/// **Walking back down past the newest entry restores what you were typing.** Blanking the line
/// instead is the behaviour people lose work to, and oslo already promises not to.
#[test]
fn coming_back_out_of_history_restores_the_typed_line() {
    let mut a = Canned {
        history: vec!["cargo test".into()],
        ..Canned::default()
    };
    let mut s = Session {
        vi: None,
        ..Session::new("half-writ", 9)
    };
    s.apply(Key::Up, &mut a);
    assert_eq!(s.buffer.text(), "cargo test");
    s.apply(Key::Down, &mut a);
    assert_eq!(s.buffer.text(), "half-writ", "the composed line came back");
}

/// Insert on a history entry asks the outer loop to copy that entry, and nothing else:
/// not the line being composed, and not an entry that has been edited since recall.
#[test]
fn insert_copies_the_history_entry_being_shown() {
    let mut a = Canned {
        history: vec!["cargo test".into(), "ls -la".into()],
        ..Canned::default()
    };
    let mut s = Session {
        vi: None,
        ..Session::new("half-writ", 9)
    };
    assert_eq!(
        s.apply(Key::Insert, &mut a),
        Step::Continue { redraw: false },
        "composing a line: there is no entry to copy"
    );
    s.apply(Key::Up, &mut a);
    assert_eq!(
        s.apply(Key::Insert, &mut a),
        Step::CopyHistoryEntry("cargo test".to_string())
    );
    assert_eq!(s.buffer.text(), "cargo test", "copying changes nothing");
    s.apply(Key::Up, &mut a);
    assert_eq!(
        s.apply(Key::Insert, &mut a),
        Step::CopyHistoryEntry("ls -la".to_string()),
        "the entry changes as the walk moves"
    );
    s.apply(Key::Char('x'), &mut a);
    assert_eq!(
        s.apply(Key::Insert, &mut a),
        Step::Continue { redraw: false },
        "edited since recall: not an entry any more"
    );
}

/// History that runs out leaves the line alone rather than blanking it.
#[test]
fn history_that_runs_out_changes_nothing() {
    let mut a = Canned::default();
    let mut s = Session {
        vi: None,
        ..Session::new("typed", 5)
    };
    assert_eq!(s.apply(Key::Up, &mut a), Step::Continue { redraw: false });
    assert_eq!(s.buffer.text(), "typed");
}

/// Completion is delegated to the outer loop that owns the terminal reader.
#[test]
fn completion_requests_the_shared_modal() {
    let mut a = Canned::default();
    let mut s = Session {
        vi: None,
        ..Session::new("git ch", 6)
    };
    assert_eq!(
        s.apply(Key::ToggleScope, &mut a),
        Step::OpenCompletion { backwards: false }
    );
    assert_eq!(s.buffer.text(), "git ch");
}

/// Ctrl-L asks for the screen to be cleared, which the loop does — the buffer is untouched.
#[test]
fn ctrl_l_clears_the_screen_without_touching_the_line() {
    let mut s = Session {
        vi: None,
        ..Session::new("half typed", 4)
    };
    assert_eq!(s.apply(Key::Ctrl('l'), &mut NoAssist), Step::ClearScreen);
    assert_eq!(s.buffer.text(), "half typed");
    assert_eq!(s.buffer.cursor(), 4, "and the cursor stayed put");
}

/// **Right at the end of the line accepts the ghost suggestion.** The key that reads as "yes,
/// that one" — Tab is for choosing between several, Right is for taking the one on screen.
#[test]
fn right_at_the_end_of_the_line_takes_the_suggestion() {
    let mut a = Canned {
        hint: Some(" --release".into()),
        ..Canned::default()
    };
    let mut s = Session {
        vi: None,
        ..Session::new("cargo build", 11)
    };
    assert_eq!(s.apply(Key::Right, &mut a), Step::Continue { redraw: true });
    assert_eq!(s.buffer.text(), "cargo build --release");
    assert_eq!(
        s.buffer.cursor(),
        21,
        "and the cursor follows it to the end"
    );
}

/// **In vi's normal mode Right is a motion and nothing else.** There it is `l`, and `d<Right>`
/// has to delete a character — a Right that inserted the suggestion instead would turn every
/// operator that takes it into something else. Accepting belongs to the modes where text is
/// being added.
#[test]
fn right_is_only_a_motion_in_vi_normal_mode() {
    let mut a = Canned {
        hint: Some(" -la".into()),
        ..Canned::default()
    };
    let mut s = Session::new("ls", 2);
    s.vi = Some(crate::edit::vi::Vi::default());
    s.apply(Key::Cancel, &mut a); // Esc: into normal mode, and one step left as vi does
    s.apply(Key::Right, &mut a);
    assert_eq!(s.buffer.text(), "ls", "the suggestion was not taken");

    // Back to insert, and the same key takes it.
    s.apply(Key::Char('a'), &mut a);
    s.buffer.move_end();
    s.apply(Key::Right, &mut a);
    assert_eq!(s.buffer.text(), "ls -la");
}

/// Mid-line it is still an ordinary cursor move — the suggestion is only ever drawn at the end,
/// so accepting one from the middle would insert text nobody was shown.
#[test]
fn right_in_the_middle_of_a_line_only_moves() {
    let mut a = Canned {
        hint: Some(" --release".into()),
        ..Canned::default()
    };
    let mut s = Session {
        vi: None,
        ..Session::new("cargo build", 5)
    };
    s.apply(Key::Right, &mut a);
    assert_eq!(s.buffer.text(), "cargo build", "unchanged");
    assert_eq!(s.buffer.cursor(), 6);
}

/// With nothing to suggest, Right falls through to moving — it must not become a dead key at the
/// end of a line, which is where it is pressed most.
#[test]
fn right_with_no_suggestion_still_moves() {
    let mut s = Session {
        vi: None,
        ..Session::new("ls", 1)
    };
    s.apply(Key::Right, &mut Canned::default());
    assert_eq!(s.buffer.cursor(), 2);
}

/// **The finished line is drawn without its ghost.**
///
/// The suggestion is a proposal, not text you typed, so the last frame of a line must not carry
/// it: `cat ~/` with `lis/` suggested left `cat ~/lis/` in the scrollback above the output of
/// `cat ~/`, a transcript that says a command ran which never did.
#[test]
fn the_last_frame_of_a_line_has_no_ghost() {
    let mut a = Canned {
        hint: Some("lis/".into()),
        ..Canned::default()
    };
    let s = Session {
        vi: None,
        ..Session::new("cat /home/me/", 13)
    };

    let editing = super::draw("$ ", "", &s, &mut a, true);
    assert!(
        editing.text.contains("lis/"),
        "the ghost must be drawn while editing: {:?}",
        editing.text
    );

    let finished = super::draw("$ ", "", &s, &mut a, false);
    assert!(
        !finished.text.contains("lis/"),
        "the ghost survived into the finished line: {:?}",
        finished.text
    );
    // The line itself is untouched — only the proposal goes.
    assert!(
        finished.text.contains("cat /home/me/"),
        "{:?}",
        finished.text
    );
}

#[test]
fn editor_frames_encode_untrusted_controls_but_keep_raw_text() {
    let raw = "echo \x1b]52;c;owned\x07\r\t\0\x7f";
    let s = Session {
        vi: None,
        ..Session::new(raw, raw.chars().count())
    };
    let frame = super::draw("$ ", "", &s, &mut NoAssist, true);
    assert!(
        !frame.text.contains('\x1b'),
        "raw escape reached frame: {frame:?}"
    );
    assert!(
        !frame.text.contains('\x07'),
        "raw BEL reached frame: {frame:?}"
    );
    assert!(frame.text.contains("^[]52;c;owned^G^M^I^@^?"));
    assert_eq!(s.buffer.text(), raw);
}

#[test]
fn untrusted_suggestion_controls_are_encoded_before_styling() {
    let mut assist = Canned {
        hint: Some("\x1b]0;owned\x07".into()),
        ..Canned::default()
    };
    let s = Session {
        vi: None,
        ..Session::new("echo", 4)
    };
    let frame = super::draw("$ ", "", &s, &mut assist, true);
    assert!(!frame.text.contains("\x1b]0;owned"));
    assert!(frame.text.contains("^[]0;owned^G"));
}

#[test]
fn hooks_receive_raw_controls_instead_of_display_notation() {
    struct RawHook;
    impl Assist for RawHook {
        fn watches_keys(&mut self) -> bool {
            true
        }

        fn key_hook(&mut self, _key: Key, line: &str, cursor: usize) -> Option<KeyHook> {
            assert_eq!(line, "echo \x1b]0;owned\x07");
            assert_eq!(cursor, line.chars().count());
            Some(KeyHook::Swallow)
        }
    }

    let raw = "echo \x1b]0;owned\x07";
    let mut session = Session {
        vi: None,
        ..Session::new(raw, raw.chars().count())
    };
    assert_eq!(
        session.apply(Key::Function(1), &mut RawHook),
        Step::Continue { redraw: false }
    );
    assert_eq!(session.buffer.text(), raw);
}

#[test]
fn multiline_paste_inserts_without_submitting() {
    let mut session = Session {
        vi: None,
        ..Session::new("echo ", 5)
    };
    assert_eq!(
        session.paste("one\necho two"),
        Step::Continue { redraw: true }
    );
    assert_eq!(session.buffer.text(), "echo one\necho two");
}

#[path = "tests/taking.rs"]
mod taking;

#[path = "tests/handlers.rs"]
mod handlers;

/// **Which ending a finished line gets.** Three of them, and the choice is worth pinning because
/// two are opt-in and the third is what every shell has always done.
#[test]
fn a_blank_line_never_gets_a_transcript() {
    // `erase` wins over everything: a key that *is* a command was never meant to be seen, rules
    // around it least of all.
    assert_eq!(ending(true, false, "nav", 0), Some(screen::park(0)));

    // With no rule configured — the default — a line stays where it was typed.
    assert_eq!(ending(false, false, "ls -l", 0), None);

    // And a line that is only whitespace takes the plain ending whatever is configured: there is
    // no command to frame, and two rules around an empty row is a worse transcript than none.
    assert_eq!(ending(false, false, "   ", 0), None);
    assert_eq!(ending(false, false, "", 0), None);
}

/// **The wiring, not the rules.** `super::super::pair` tests what should happen to a neighbourhood
/// of characters; this checks that typing the key actually does it, through `apply`.
#[test]
fn a_bracket_typed_at_the_prompt_closes_itself() {
    let (s, _) = run("", &typed("echo ("));
    assert_eq!(s.buffer.text(), "echo ()");
    assert_eq!(s.buffer.cursor(), 6, "the cursor waits between them");

    // Typing the closer walks over the one already there rather than doubling it.
    let (s, _) = run("", &typed("echo (a)"));
    assert_eq!(s.buffer.text(), "echo (a)");
    assert_eq!(s.buffer.cursor(), 8);

    // And a whole quoted word comes out as one pair.
    let (s, _) = run("", &typed("echo \"hi\""));
    assert_eq!(s.buffer.text(), "echo \"hi\"");
}

/// One gesture made both halves, so one gesture removes both — but only while they are still a
/// pair. Deleting a character the user typed is the one thing this must never do.
#[test]
fn backspace_over_an_empty_pair_takes_both() {
    let (s, _) = run("", &[Key::Char('('), Key::Backspace]);
    assert_eq!(s.buffer.text(), "");

    let (s, _) = run("", &[Key::Char('('), Key::Char('a'), Key::Backspace]);
    assert_eq!(
        s.buffer.text(),
        "()",
        "the closer stays once something is between them"
    );

    let (s, _) = run("ab", &[Key::Backspace]);
    assert_eq!(s.buffer.text(), "a", "an ordinary backspace is untouched");
}

/// The apostrophe case, end to end: in a shell a stray quote swallows the rest of the line.
#[test]
fn a_quote_after_a_word_stays_one_character() {
    let (s, _) = run("", &typed("echo it's"));
    assert_eq!(s.buffer.text(), "echo it's");
}
