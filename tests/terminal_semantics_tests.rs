//! Terminal semantic marks observed through the real interactive binary.
mod common;

use std::time::Duration;

#[path = "terminal_semantics/terminal_input.rs"]
mod terminal_input;

#[path = "terminal_semantics/sessions.rs"]
mod sessions;

#[path = "terminal_semantics/veto.rs"]
mod veto;

#[path = "terminal_semantics/pty.rs"]
mod pty;

use pty::{PtyShell, kinds, marks, visible, vscode_kinds};
// `terminal_input` reads this through `use super::*`.
use std::process::Command;

#[test]
fn successful_and_failed_commands_have_balanced_stable_marks() {
    let mut shell = PtyShell::spawn("xterm-256color");
    shell.wait_for_marks(2);
    shell.send(b"true\nfalse\n");
    let marks = shell.wait_for_marks(10);

    assert_eq!(kinds(&marks[..10]), "ABCDABCDAB");
    assert_eq!(marks[3].status(), Some(0));
    assert_eq!(marks[7].status(), Some(1));
    let aid = marks[0].aid().expect("session aid");
    assert!(marks[..10].iter().all(|mark| mark.aid() == Some(aid)));
}

#[test]
fn blank_partial_interrupt_and_eof_close_without_command_start() {
    let mut shell = PtyShell::spawn("xterm-256color");
    shell.wait_for_marks(2);
    shell.send(b"\n");
    shell.wait_for_marks(5);
    shell.send(b"partial\x03");
    shell.wait_for_marks(8);
    shell.send(b"\x03");
    shell.wait_for_marks(11);
    shell.send(b"\x04");
    shell.wait_for_exit();
    let marks = marks(&shell.transcript);

    assert_eq!(kinds(&marks), "ABDABDABDABD");
    assert!(
        marks
            .iter()
            .filter(|mark| mark.kind == b'D')
            .all(|mark| mark.status().is_none())
    );
    assert!(!marks.iter().any(|mark| mark.kind == b'C'));
}

#[test]
fn multiline_input_has_secondary_prompt_marks_in_one_interaction() {
    let mut shell = PtyShell::configured("xterm-256color", true, "oslo.autopair.enabled = false\n");
    shell.wait_for_marks(2);
    shell.send(b"printf '%s\\n' \"left\n");
    let continued = shell.wait_for_marks(4);
    assert_eq!(kinds(&continued[..4]), "ABAB");
    assert!(continued[2].body.contains("k=s"), "{:?}", continued[2]);
    shell.send(b"middle\n");
    let continued = shell.wait_for_marks(6);
    assert_eq!(kinds(&continued[..6]), "ABABAB");
    assert!(continued[4].body.contains("k=s"), "{:?}", continued[4]);
    shell.send(b"right\"\n");
    let complete = shell.wait_for_marks(10);

    assert_eq!(kinds(&complete[..10]), "ABABABCDAB");
    let aid = complete[0].aid().expect("session aid");
    assert!(complete[..10].iter().all(|mark| mark.aid() == Some(aid)));
}

#[test]
fn lua_continuations_and_language_redraw_keep_one_interaction() {
    let config = r#"
oslo.misc.welcome = false
oslo.env.set("OSLO_DEFAULT_MODE", "lua")
"#;
    let mut shell = PtyShell::configured("xterm-256color", true, config);
    shell.wait_for_marks(2);
    shell.send(b"if true then\n");
    shell.wait_for_marks(4);
    shell.send(b"if true then\n");
    shell.wait_for_marks(6);
    // **`end` does not run it; an empty line does.** A Lua block that has asked for more keeps
    // asking until a blank line, which is Python's rule and there for Python's reason: after
    // `local function f()` the parser is satisfied again at `end`, so running the moment it parses
    // would mean no line after `end` could ever be typed. So this is one more continuation prompt,
    // not the command — see `docs/features/two-languages-one-prompt.md`.
    shell.send(b"end end\n");
    shell.wait_for_marks(8);
    shell.send(b"\n");
    let complete = shell.wait_for_marks(12);
    assert_eq!(kinds(&complete[..12]), "ABABABABCDAB");
}

#[test]
fn right_transient_and_vi_redraws_do_not_repeat_boundaries() {
    let config = r#"
oslo.misc.welcome = false
oslo.vi.enabled = true
oslo.prompt.left = function() return "LEFT> " end
oslo.prompt.right = function() return "RIGHT" end
oslo.prompt.transient = function() return "SHORT> " end
"#;
    let mut shell = PtyShell::configured("xterm-256color", false, config);
    shell.wait_for_marks(2);
    shell.wait_for_text("RIGHT");
    shell.send(b"\x1b[Z");
    shell.drain_for(Duration::from_millis(50));
    assert_eq!(kinds(&marks(&shell.transcript)), "AB");
    shell.send(b"\x1b[Z");
    shell.drain_for(Duration::from_millis(50));
    assert_eq!(kinds(&marks(&shell.transcript)), "AB");
    shell.send(b"\x1b");
    shell.drain_for(Duration::from_millis(50));
    assert_eq!(kinds(&marks(&shell.transcript)), "AB");

    shell.send(b"itrue\n");
    let complete = shell.wait_for_marks(6);
    assert_eq!(kinds(&complete[..6]), "ABCDAB");
    shell.wait_for_text("SHORT> ");
}

#[test]
fn post_hook_metadata_is_exact_private_and_cancel_safe() {
    let config = r#"
oslo.misc.welcome = false
oslo.on.pre_cmd(function(command)
  if command.text == "replace-me" then return "printf replaced" end
  if command.text == "cancel-me" then return false end
end)
"#;
    let mut shell = PtyShell::configured("xterm-256color", true, config);
    shell.wait_for_marks(2);
    shell.send(b"replace-me\n");
    let replaced = shell.wait_for_marks(6);
    assert!(
        replaced[2].body.contains("cmdline_url=printf%20replaced"),
        "{:?}",
        replaced[2]
    );

    shell.send(b" true\n");
    let private = shell.wait_for_marks(10);
    assert_eq!(private[6].kind, b'C');
    assert!(!private[6].body.contains("cmdline_url"), "{:?}", private[6]);

    shell.send(b"cancel-me\n");
    let cancelled = shell.wait_for_marks(13);
    assert_eq!(kinds(&cancelled[..13]), "ABCDABCDABDAB");
    assert_eq!(cancelled[10].kind, b'D');
    assert_eq!(cancelled[10].status(), None);
}

#[test]
fn disabled_marks_and_separate_processes_are_isolated() {
    let config = r#"
oslo.misc.welcome = false
oslo.feature.set("marks", false)
oslo.prompt.left = function() return "READY> " end
"#;
    let mut disabled = PtyShell::configured("xterm-256color", false, config);
    disabled.wait_for_text("READY> ");
    disabled.send(b"exit\n");
    disabled.wait_for_exit();
    assert!(marks(&disabled.transcript).is_empty());

    let mut first = PtyShell::spawn("xterm-256color");
    let first_aid = first.wait_for_marks(2)[0]
        .aid()
        .expect("first session aid")
        .to_string();
    let mut second = PtyShell::spawn("xterm-256color");
    let second_aid = second.wait_for_marks(2)[0]
        .aid()
        .expect("second session aid")
        .to_string();
    assert_ne!(first_aid, second_aid);
}

#[test]
fn iterm_nested_shells_keep_distinct_stable_session_ids() {
    // An interactive oslo started inside one asks whether that is what you meant, and this test
    // nests on purpose — so it says so in the config it starts with. See `startup::nested`.
    let mut shell = PtyShell::spawn_with_config(
        "xterm-256color",
        false,
        Some("iTerm.app"),
        Some("oslo.misc.nested_ask = false\n"),
    );
    shell.wait_for_marks(2);
    shell.send(format!("{} -i\n", common::oslo_bin().display()).as_bytes());
    let nested = shell.wait_for_marks(5);
    let outer = nested[0].aid().expect("outer aid").to_string();
    let child = nested[3].aid().expect("child aid").to_string();
    assert_ne!(outer, child);
    assert!(nested[..3].iter().all(|mark| mark.aid() == Some(&outer)));
    assert!(nested[3..5].iter().all(|mark| mark.aid() == Some(&child)));

    shell.send(b"exit 7\n");
    let finished = shell.wait_for_marks(10);
    assert!(finished[3..7].iter().all(|mark| mark.aid() == Some(&child)));
    assert_eq!(finished[6].status(), Some(7));
    assert!(
        finished[7..10]
            .iter()
            .all(|mark| mark.aid() == Some(&outer))
    );
}
