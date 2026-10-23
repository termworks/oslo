use super::*;

#[test]
fn pasted_newlines_wait_for_an_explicit_enter() {
    let mut shell = PtyShell::spawn("xterm-256color");
    shell.wait_for_marks(2);
    shell.send(b"\x1b[200~echo first\necho second\x1b[201~");
    shell.drain_for(Duration::from_millis(100));
    assert_eq!(kinds(&blocks(&shell.transcript)), "AB");

    shell.send(b"\n");
    let complete = shell.wait_for_marks(6);
    assert_eq!(kinds(&complete[..6]), "ABCDAB");
}

/// Typing re-stamps the prompt's marks, because typing erases the rows they were on.
///
/// Each keystroke redraws the block from its first row, and a row the redraw recreates has lost
/// the mark it wore. So each frame carries the pair again, and the block still reads as one.
#[test]
fn the_prompt_marks_survive_typing() {
    let mut shell = PtyShell::spawn("xterm-256color");
    shell.wait_for_marks(2);
    let before = marks(&shell.transcript).len();

    // One key at a time: each is a repaint, and each repaint has to put the marks back.
    for byte in b"echo hi" {
        shell.send(&[*byte]);
    }
    shell.drain_for(Duration::from_millis(200));

    let after = marks(&shell.transcript);
    assert!(
        after.len() > before,
        "typing drew no marks at all: {} then {}, {:?}",
        before,
        after.len(),
        visible(&shell.transcript)
    );
    // Still one block, however many times it was drawn.
    assert_eq!(
        kinds(&blocks(&shell.transcript)),
        "AB",
        "{:?}",
        visible(&shell.transcript)
    );
}

#[test]
fn pasted_terminal_controls_are_visible_but_inert() {
    let mut shell = PtyShell::spawn("xterm-256color");
    shell.wait_for_marks(2);
    shell.send(b"\x1b[200~echo \x1b]0;owned\x07 \x1b]52;c;YWJj\x1b\\ \x1b[2J\x1b[201~");
    shell.wait_for_plain_text("^[]0;owned^G");
    shell.wait_for_plain_text("^[]52;c;YWJj^[\\");
    shell.wait_for_plain_text("^[[2J");
    assert!(!shell.transcript.windows(10).any(|w| w == b"\x1b]0;owned"));
    assert!(!shell.transcript.windows(6).any(|w| w == b"\x1b[2J"));
    shell.send(b"\x03\x04");
    shell.wait_for_exit();
}

#[test]
fn grapheme_movement_and_deletion_work_through_the_real_editor() {
    let config = r#"
oslo.misc.welcome = false
oslo.prompt.left = function() return "> " end
"#;
    let mut shell = PtyShell::configured("xterm-256color", false, config);
    shell.wait_for_text("> ");

    shell.send("printf '<%s>\\n' 'e\u{301}👍🏽".as_bytes());
    shell.send(b"\x7f'\n");
    shell.wait_for_plain_text("<e\u{301}>");

    shell.send("printf '<%s>\\n' 'a👨‍👩‍👧‍👦b'".as_bytes());
    shell.send(b"\x1b[D\x1b[D\x1b[D\x1b[3~\n");
    shell.wait_for_plain_text("<ab>");
    shell.send(b"exit\n");
    shell.wait_for_exit();
}

#[test]
fn negotiated_keyboard_modes_are_balanced_on_editor_exits() {
    let mut shell = PtyShell::spawn("xterm-256color");
    shell.send(b"\x1b[?1u\x1b[?62;52;c");
    shell.wait_for_occurrences(oslo::ui::term::keyboard::PUSH_ENHANCEMENTS.as_bytes(), 1);

    shell.send(b"true\n");
    shell.wait_for_occurrences(oslo::ui::term::keyboard::PUSH_ENHANCEMENTS.as_bytes(), 2);
    shell.send(b"\x1b[Z");
    shell.wait_for_occurrences(oslo::ui::term::keyboard::PUSH_ENHANCEMENTS.as_bytes(), 3);
    shell.send(b"\x03");
    shell.wait_for_occurrences(oslo::ui::term::keyboard::PUSH_ENHANCEMENTS.as_bytes(), 4);
    shell.send(b"\x04");
    shell.wait_for_exit();

    let pushes = shell
        .transcript
        .windows(oslo::ui::term::keyboard::PUSH_ENHANCEMENTS.len())
        .filter(|window| *window == oslo::ui::term::keyboard::PUSH_ENHANCEMENTS.as_bytes())
        .count();
    let pops = shell
        .transcript
        .windows(oslo::ui::term::keyboard::POP.len())
        .filter(|window| *window == oslo::ui::term::keyboard::POP.as_bytes())
        .count();
    assert_eq!(pushes, 4);
    assert_eq!(pops, pushes);
}

#[test]
fn f2_binding_works_through_the_real_editor() {
    let config = r#"
oslo.misc.welcome = false
oslo.prompt.left = function(p) return p.language .. "> " end
    oslo.keys["f2"] = "toggle-language"
"#;
    let mut shell = PtyShell::configured("xterm-256color", false, config);
    shell.send(b"\x1b[?1u\x1b[?62;52;c");
    shell.wait_for_text("sh> ");
    shell.send(b"\x1b[Q");
    shell.wait_for_text("lua> ");
    shell.send(b"\x04");
    shell.wait_for_exit();
}

#[test]
fn function_key_lua_binding_works_through_the_real_editor() {
    let config = r#"
oslo.misc.welcome = false
oslo.keys["f3"] = function(line) return "printf f3-lua" end
"#;
    let mut shell = PtyShell::configured("xterm-256color", false, config);
    shell.wait_for_marks(2);
    shell.send(b"\x1bOR");
    shell.wait_for_plain_text("printf f3-lua");
    shell.send(b"\x03\x04");
    shell.wait_for_exit();
}

#[test]
fn startup_negotiates_sync_and_notifications_before_one_barrier() {
    let mut shell = PtyShell::spawn("xterm-256color");
    shell.wait_for_text("\x1b]99;i=");
    let transcript = String::from_utf8_lossy(&shell.transcript);
    let id = transcript
        .split("\x1b]99;i=")
        .nth(1)
        .and_then(|rest| rest.split(':').next())
        .expect("query id")
        .to_string();
    shell.send(
        format!(
            "\x1b[?1u\x1b[?2026;1$y\x1b]99;i={id}:p=?;p=title,body:o=unfocused\x1b\\\x1b[?62;4c"
        )
        .as_bytes(),
    );
    shell.wait_for_marks(2);
    shell.send(b"status terminal\n");
    shell.wait_for_text("synchronized-output on verified");
    shell.wait_for_text("osc99-notifications on verified");
    shell.send(b"exit\n");
    shell.wait_for_exit();
}

#[test]
fn completion_replays_the_character_that_dismisses_it() {
    let mut shell = PtyShell::spawn("xterm-256color");
    shell.wait_for_marks(2);
    std::fs::write(shell.home.path().join("apple"), "").expect("apple");
    std::fs::write(shell.home.path().join("apricot"), "").expect("apricot");
    shell.send(b"cat a\t");
    shell.wait_for_plain_text("apple");
    shell.send(b"x");
    shell.wait_for_plain_text("cat ax");
    shell.send(b"\x03\x04");
    shell.wait_for_exit();
}

#[test]
fn paste_and_mouse_modes_are_balanced_on_editor_exits() {
    let mut shell =
        PtyShell::spawn_with_environment("xterm-256color", &[("OSLO_CLICK_EVENTS", "legacy")]);
    shell.wait_for_occurrences(oslo::ui::term::mouse::ENABLE, 1);
    shell.send(b"true\n");
    shell.wait_for_occurrences(oslo::ui::term::mouse::ENABLE, 2);
    shell.send(b"\x04");
    shell.wait_for_exit();

    for (enable, disable) in [
        (
            oslo::ui::term::mouse::ENABLE,
            oslo::ui::term::mouse::DISABLE,
        ),
        (
            oslo::ui::term::BRACKETED_PASTE_ENABLE,
            oslo::ui::term::BRACKETED_PASTE_DISABLE,
        ),
    ] {
        let enabled = shell
            .transcript
            .windows(enable.len())
            .filter(|w| *w == enable)
            .count();
        let disabled = shell
            .transcript
            .windows(disable.len())
            .filter(|w| *w == disable)
            .count();
        assert_eq!(enabled, 2);
        assert_eq!(disabled, enabled);
    }
}

/// Ctrl-R the instant the prompt comes back still finds the command you just ran.
///
/// The log append happens on the writer thread now, so the store is a fraction of a millisecond
/// behind the prompt. The finder is the one reader that cannot tolerate that: asking for your
/// history and not being shown the line you just typed reads as the shell having lost it — and in
/// a fresh session, where that line is the only one there is, the finder declines to open at all.
///
/// **No pause between the two sends**, because the pause is the bug. The mouse test above catches
/// this by accident; this one is about the thing itself.
#[test]
fn the_finder_shows_a_command_typed_a_moment_ago() {
    // The *first* thing a new prompt writes, not the last. `OSC 133;B` closes the frame, by which
    // point the writer thread has had the whole redraw to catch up and the race no longer shows.
    const PROMPT_START: &[u8] = b"\x1b[?2004h";

    let mut shell = PtyShell::spawn("xterm-256color");
    shell.wait_for_occurrences(PROMPT_START, 1);
    shell.send(b"true zqxwvmarker\n");
    shell.wait_for_occurrences(PROMPT_START, 2);
    shell.send(b"\x12");
    // The finder's match counter, which is the assertion: it says one row is being shown, and in a
    // session one command old that row can only be the command. Not the line's own text — the
    // terminal echoed that back when it was typed, so finding it proves nothing about the finder.
    // `wait_for_text` panics on timeout, and timing out is exactly what an empty store causes:
    // `open_finder` declines rather than opening on nothing.
    shell.wait_for_text("1/1");
}

#[test]
fn legacy_mouse_pauses_while_the_finder_owns_the_terminal() {
    let mut shell =
        PtyShell::spawn_with_environment("xterm-256color", &[("OSLO_CLICK_EVENTS", "legacy")]);
    shell.wait_for_occurrences(oslo::ui::term::mouse::ENABLE, 1);
    shell.send(b"true\n");
    shell.wait_for_occurrences(oslo::ui::term::mouse::ENABLE, 2);

    shell.send(b"\x12");
    shell.wait_for_occurrences(oslo::ui::term::mouse::DISABLE, 2);
    shell.send(b"\x1b");
    shell.wait_for_occurrences(oslo::ui::term::mouse::ENABLE, 3);

    shell.send(b"\x03");
    shell.wait_for_occurrences(oslo::ui::term::mouse::ENABLE, 4);
    shell.send(b"\x04");
    shell.wait_for_exit();

    let enabled = shell
        .transcript
        .windows(oslo::ui::term::mouse::ENABLE.len())
        .filter(|window| *window == oslo::ui::term::mouse::ENABLE)
        .count();
    let disabled = shell
        .transcript
        .windows(oslo::ui::term::mouse::DISABLE.len())
        .filter(|window| *window == oslo::ui::term::mouse::DISABLE)
        .count();
    assert_eq!(enabled, disabled);
}

#[test]
fn semantic_clicks_do_not_enable_global_mouse_reporting() {
    let mut shell =
        PtyShell::spawn_with_environment("xterm-256color", &[("OSLO_CLICK_EVENTS", "1")]);
    shell.wait_for_text("click_events=2");
    shell.drain_for(Duration::from_millis(50));
    assert!(
        !shell
            .transcript
            .windows(oslo::ui::term::mouse::ENABLE.len())
            .any(|w| { w == oslo::ui::term::mouse::ENABLE })
    );
    shell.send(b"\x1b[<0;1;1M");
    shell.drain_for(Duration::from_millis(50));
    assert!(
        !shell.transcript.windows(5).any(|w| w == b"\x1b[?6n"),
        "{}",
        visible(&shell.transcript)
    );
    shell.send(b"\x04");
    shell.wait_for_exit();
}

#[test]
fn vscode_selects_one_rich_lifecycle() {
    let mut shell = PtyShell::spawn_with_options("xterm-256color", false, Some("vscode"));
    shell.wait_for_text("\x1b]633;B\x1b\\");
    shell.send(b"true\n");
    shell.wait_for_text("\x1b]633;D;0\x1b\\");
    // The *second* `B`: one per prompt, and the first is still in the transcript, so waiting for
    // it by text alone returns immediately and leaves the trailing marks to a 50 ms race.
    shell.wait_for_text_count("\x1b]633;B\x1b\\", 2);
    shell.drain_for(Duration::from_millis(50));

    assert_eq!(vscode_blocks(&shell.transcript), "ABECDAB");
    assert!(marks(&shell.transcript).is_empty());
}

#[test]
fn dumb_and_noninteractive_sessions_emit_no_semantic_marks() {
    let mut dumb = PtyShell::spawn("dumb");
    dumb.wait_for_text("Type 'exit'");
    dumb.send(b"exit\n");
    dumb.wait_for_exit();
    assert!(
        marks(&dumb.transcript).is_empty(),
        "{}",
        visible(&dumb.transcript)
    );

    let output = Command::new(common::oslo_bin())
        .args(["-c", "true"])
        .env("TERM", "xterm-256color")
        .output()
        .expect("run noninteractive oslo");
    let mut transcript = output.stdout;
    transcript.extend(output.stderr);
    assert!(marks(&transcript).is_empty(), "{}", visible(&transcript));
}

#[test]
fn noninteractive_terminal_status_is_inert() {
    let output = Command::new(common::oslo_bin())
        .args(["-c", "status terminal"])
        .env("TERM", "xterm-256color")
        .env("OSLO_SYNC_OUTPUT", "1")
        .env("OSLO_CLICK_EVENTS", "1")
        .output()
        .expect("run terminal diagnostics");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("semantic-protocol none disabled\n"));
    let mut transcript = output.stdout;
    transcript.extend(output.stderr);
    assert!(!transcript.contains(&0x1b), "{}", visible(&transcript));
}

#[test]
fn nav_escape_after_navigation_changes_the_shell_directory() {
    let config = r#"
oslo.misc.welcome = false
oslo.prompt.left = function() return "> " end
oslo.builtin.nav.fullscreen = false
oslo.builtin.nav.scanner = false
oslo.builtin.nav.height = 8
"#;
    let mut shell = PtyShell::configured("xterm-256color", false, config);
    let first = shell.home.path().join("first");
    let second = first.join("second");
    std::fs::create_dir_all(&second).expect("create navigation tree");
    shell.wait_for_marks(2);

    shell.send(b"nav\n");
    shell.wait_for_plain_text("first/");
    shell.send(b"\x1b[C");
    shell.wait_for_plain_text("second/");
    shell.send(b"\x1b[C");
    shell.wait_for_plain_text(&second.to_string_lossy());
    shell.send(b"\x1b");
    shell.wait_for_marks(6);
    shell.send(b"pwd\n");
    shell.wait_for_plain_text(&second.to_string_lossy());
    shell.send(b"exit\n");
    shell.wait_for_exit();
}

#[test]
fn nav_escape_changes_to_its_starting_directory() {
    let config = r#"
oslo.misc.welcome = false
oslo.builtin.nav.fullscreen = false
oslo.builtin.nav.scanner = false
"#;
    let mut shell = PtyShell::configured("xterm-256color", false, config);
    let target = shell.home.path().join("target");
    std::fs::create_dir(&target).expect("create target");
    shell.wait_for_marks(2);

    shell.send(format!("nav {}\n", target.display()).as_bytes());
    shell.wait_for_plain_text(&target.to_string_lossy());
    shell.send(b"\x1b");
    shell.wait_for_marks(6);
    shell.send(b"pwd\n");
    shell.wait_for_plain_text(&target.to_string_lossy());
    shell.send(b"exit\n");
    shell.wait_for_exit();
}

#[test]
fn nav_typing_filters_and_delete_uses_the_internal_rm() {
    let config = r#"
oslo.misc.welcome = false
oslo.prompt.left = function() return "> " end
oslo.builtin.nav.scanner = false
-- This is about the filter and about `rm`. With type-and-navigate on, `q` would walk straight
-- into the only match and there would be no filter left to look at — which is its own test.
oslo.builtin.nav.type_nav = { enabled = false }
oslo.builtin.rm.to_tmp = true
oslo.builtin.rm.trash = os.getenv("HOME") .. "/trash"
"#;
    let mut shell = PtyShell::configured("xterm-256color", false, config);
    let quick = shell.home.path().join("quick");
    std::fs::create_dir(&quick).expect("create entry to navigate into");
    std::fs::write(quick.join("inside"), "move this to trash").expect("create removable file");
    shell.wait_for_marks(2);

    shell.send(b"nav\n");
    shell.wait_for_plain_text("quick/");
    assert!(!String::from_utf8_lossy(&shell.transcript).contains("filter @"));
    assert!(!String::from_utf8_lossy(&shell.transcript).contains("type filter"));

    shell.send(b"?");
    shell.wait_for_plain_text("type filter");
    shell.send(b"?q");
    shell.wait_for_plain_text("filter @");
    shell.send(b"\n");
    shell.wait_for_plain_text(&quick.to_string_lossy());

    shell.send(b"\x1b[3~");
    shell.wait_for_plain_text("delete inside?");
    shell.send(b"y");
    shell.drain_for(Duration::from_millis(50));
    shell.send(b"\x1b");
    shell.wait_for_marks(6);
    shell.send(b"test ! -e inside && test -e ../trash/inside && echo trashed\n");
    shell.wait_for_plain_text("trashed");
    shell.send(b"exit\n");
    shell.wait_for_exit();
}

/// Typing a name until nothing else matches walks into it, with no Enter — and the rest of the
/// word typed behind it is dropped rather than becoming a filter in the directory just entered.
#[test]
fn nav_walks_into_the_only_match_and_swallows_the_rest_of_the_word() {
    let config = r#"
oslo.misc.welcome = false
oslo.prompt.left = function() return "> " end
oslo.builtin.nav.scanner = false
oslo.builtin.nav.type_nav = { enabled = true, settle_ms = 2000 }
"#;
    let mut shell = PtyShell::configured("xterm-256color", false, config);
    let fuzz = shell.home.path().join("fuzz");
    std::fs::create_dir(&fuzz).expect("create the directory to walk into");
    // Shares `fu`, so `fuzz` is only unambiguous at `fuz` — one character short of the word.
    std::fs::create_dir(shell.home.path().join("fudge")).expect("create the near miss");
    std::fs::write(fuzz.join("landed"), "here").expect("create a file to see inside");
    shell.wait_for_marks(2);

    shell.send(b"nav\n");
    shell.wait_for_plain_text("fuzz/");

    // The whole word at once, as it would arrive from a keyboard.
    shell.send(b"fuzz");
    shell.wait_for_plain_text(&fuzz.to_string_lossy());
    // Inside, and unfiltered: the trailing `z` did not become a search for `z`.
    shell.wait_for_plain_text("landed");

    shell.send(b"\x1b");
    shell.wait_for_marks(6);
    shell.send(b"pwd\n");
    shell.wait_for_plain_text(&fuzz.to_string_lossy());
    shell.send(b"exit\n");
    shell.wait_for_exit();
}

/// `.` shows and hides the dotfiles. It is a browse-mode shortcut: once a filter is being typed a
/// dot is just a character, because `Cargo.toml` has to be typeable.
#[test]
fn nav_dot_toggles_hidden_entries_while_browsing() {
    let config = r#"
oslo.misc.welcome = false
oslo.prompt.left = function() return "> " end
oslo.builtin.nav.scanner = false
oslo.builtin.nav.type_nav = { enabled = false }
"#;
    let mut shell = PtyShell::configured("xterm-256color", false, config);
    std::fs::write(shell.home.path().join("visible"), "x").expect("a plain file");
    std::fs::write(shell.home.path().join(".secret"), "x").expect("a hidden file");
    shell.wait_for_marks(2);

    shell.send(b"nav\n");
    shell.wait_for_plain_text("visible");
    assert!(!String::from_utf8_lossy(&shell.transcript).contains(".secret"));

    shell.send(b".");
    shell.wait_for_plain_text(".secret");
    // And it is a toggle, not a one-way door: the filter never opened for it.
    assert!(!String::from_utf8_lossy(&shell.transcript).contains("filter @"));

    shell.send(b"\x1b");
    shell.wait_for_marks(6);
    shell.send(b"exit\n");
    shell.wait_for_exit();
}
