//! Reading keys out of bytes a terminal split however it liked.
//!
//! These moved here with the reader when the history finder stopped being the only thing that
//! needed one. The mouse-report cases are the reason the reader exists: see the module note.

use super::*;

#[test]
fn line_mode_has_balanced_bracketed_paste_controls() {
    assert_eq!(BRACKETED_PASTE_ENABLE, b"\x1b[?2004h");
    assert_eq!(BRACKETED_PASTE_DISABLE, b"\x1b[?2004l");
}

#[test]
fn line_mode_blocks_for_at_least_one_input_byte() {
    let pty = nix::pty::openpty(None, None).expect("open pty");
    let mut inherited = nix::sys::termios::tcgetattr(&pty.slave).expect("read termios");
    inherited.control_chars[nix::libc::VMIN] = 0;
    inherited.control_chars[nix::libc::VTIME] = 0;
    let raw = editor_termios(&inherited);
    assert_eq!(raw.control_chars[nix::libc::VMIN], 1);
    assert_eq!(raw.control_chars[nix::libc::VTIME], 0);
}

#[test]
fn the_arrows_and_the_keys_beside_them() {
    assert_eq!(key(b"\x1b[A"), Key::Up);
    assert_eq!(key(b"\x1b[B"), Key::Down);
    assert_eq!(key(b"\x1b[C"), Key::Right);
    assert_eq!(key(b"\x1b[D"), Key::Left);
    assert_eq!(key(b"\x1b[H"), Key::Home);
    assert_eq!(key(b"\x1b[F"), Key::End);
    assert_eq!(key(b"\x1b[3~"), Key::Delete);
    assert_eq!(key(b"\x1b[5~"), Key::PageUp);
    assert_eq!(key(b"\x1b[6~"), Key::PageDown);
    assert_eq!(key(b"\x1b[Z"), Key::BackTab);
    // The application-cursor spellings a terminal switches to inside a full-screen program.
    assert_eq!(key(b"\x1bOA"), Key::Up);
    assert_eq!(key(b"\x1bOD"), Key::Left);
}

/// The four control bytes above the alphabet, which a terminal sends bare when it has not been
/// asked for the Kitty protocol. Without them `^\` is not a chord at all — it falls through to
/// `text_key`, which has no character to make of a control byte and returns `Ignored`, so the key
/// does nothing and nothing says why.
#[test]
fn the_control_chords_above_the_alphabet() {
    assert_eq!(key(b"\x1c"), Key::Ctrl('\\'));
    assert_eq!(key(b"\x1d"), Key::Ctrl(']'));
    assert_eq!(key(b"\x1e"), Key::Ctrl('^'));
    assert_eq!(key(b"\x1f"), Key::Ctrl('_'));
    // The same chords once the terminal is speaking Kitty, which is the other half of the pair.
    assert_eq!(super::keyboard::decode(b"\x1b[92;5u"), Key::Ctrl('\\'));
    // Esc is 0x1b and sits just below the range: it keeps its own meaning.
    assert_eq!(key(b"\x1b"), Key::Cancel);
}

#[test]
fn function_keys_decode_without_colliding_with_reports() {
    let legacy = [
        (b"\x1bOP".as_slice(), 1),
        (b"\x1bOQ".as_slice(), 2),
        (b"\x1bOR".as_slice(), 3),
        (b"\x1bOS".as_slice(), 4),
        (b"\x1b[15~".as_slice(), 5),
        (b"\x1b[17~".as_slice(), 6),
        (b"\x1b[18~".as_slice(), 7),
        (b"\x1b[19~".as_slice(), 8),
        (b"\x1b[20~".as_slice(), 9),
        (b"\x1b[21~".as_slice(), 10),
        (b"\x1b[23~".as_slice(), 11),
        (b"\x1b[24~".as_slice(), 12),
    ];
    for (sequence, number) in legacy {
        assert_eq!(key(sequence), Key::Function(number), "F{number}");
        assert_eq!(
            parse(sequence),
            Parsed::Took(sequence.len(), Key::Function(number))
        );
        for at in 1..sequence.len() {
            assert_eq!(parse(&sequence[..at]), Parsed::Partial, "F{number} at {at}");
        }
        let mut adjacent = sequence.to_vec();
        adjacent.push(b'x');
        assert_eq!(
            parse(&adjacent),
            Parsed::Took(sequence.len(), Key::Function(number))
        );
    }
    for number in 1..=4 {
        let sequence = format!("\x1b[{}~", number + 10);
        assert_eq!(key(sequence.as_bytes()), Key::Function(number));
    }
    for (sequence, number) in [
        (b"\x1b[P".as_slice(), 1),
        (b"\x1b[Q".as_slice(), 2),
        (b"\x1b[S".as_slice(), 4),
    ] {
        assert_eq!(key(sequence), Key::Function(number), "Kitty F{number}");
        assert_eq!(
            parse(sequence),
            Parsed::Took(sequence.len(), Key::Function(number))
        );
        for at in 1..sequence.len() {
            assert_eq!(
                parse(&sequence[..at]),
                Parsed::Partial,
                "Kitty F{number} at {at}"
            );
        }
        let mut adjacent = sequence.to_vec();
        adjacent.push(b'x');
        assert_eq!(
            parse(&adjacent),
            Parsed::Took(sequence.len(), Key::Function(number))
        );
    }
    for (number, parameters, final_byte) in [
        (1, "1", 'P'),
        (2, "1", 'Q'),
        (3, "13", '~'),
        (4, "1", 'S'),
        (5, "15", '~'),
        (6, "17", '~'),
        (7, "18", '~'),
        (8, "19", '~'),
        (9, "20", '~'),
        (10, "21", '~'),
        (11, "23", '~'),
        (12, "24", '~'),
    ] {
        let sequence = format!("\x1b[{parameters};1{final_byte}");
        assert_eq!(key(sequence.as_bytes()), Key::Function(number));
        let press = format!("\x1b[{parameters};1:1{final_byte}");
        assert_eq!(key(press.as_bytes()), Key::Function(number));
    }
    assert_eq!(key(b"\x1b[R"), Key::Ignored, "CSI R is a report final");
    assert_eq!(key(b"\x1b[15;2~"), Key::Ignored, "modified F5");
    assert_eq!(key(b"\x1b[15;1:2~"), Key::Ignored, "repeat F5");
    assert_eq!(key(b"\x1b[15;1:3~"), Key::Ignored, "release F5");
    assert_eq!(key(b"\x1b[10~"), Key::Ignored, "F0 is unsupported");
    assert_eq!(key(b"\x1b[25~"), Key::Ignored, "F13 is unsupported");
}

/// The control characters every readline-shaped thing binds, so a widget does not have to.
#[test]
fn the_control_keys() {
    assert_eq!(key(b"\x10"), Key::Up);
    assert_eq!(key(b"\x0e"), Key::Down);
    assert_eq!(key(b"\x02"), Key::Left);
    assert_eq!(key(b"\x06"), Key::Right);
    assert_eq!(key(b"\x01"), Key::Home);
    assert_eq!(key(b"\x05"), Key::End);
    assert_eq!(key(b"\x04"), Key::Delete);
    assert_eq!(key(b"\x15"), Key::Clear);
    assert_eq!(key(b"\t"), Key::ToggleScope);
}

#[test]
fn the_keys_that_leave() {
    assert_eq!(key(b"\x1b"), Key::Cancel);
    // Ctrl-C is its own key: the pager treats an abort differently from an ordinary quit.
    assert_eq!(key(b"\x03"), Key::Abort);
    assert_eq!(key(b"\r"), Key::Accept);
    assert_eq!(key(b"\n"), Key::Accept);
    assert_eq!(key(b"\x7f"), Key::Backspace);
    assert_eq!(key(b"\x08"), Key::Backspace);
}

#[test]
fn text_is_text() {
    assert_eq!(key(b"g"), Key::Char('g'));
    assert_eq!(key(b" "), Key::Char(' '));
    assert_eq!(key("é".as_bytes()), Key::Char('é'));
    assert_eq!(key("→".as_bytes()), Key::Char('→'));
    // A control character nothing binds is not text. `0x1c`..`0x1f` are chords and are asserted in
    // `the_control_chords_above_the_alphabet`; `0x1b` alone is Cancel.
    assert_eq!(key(b"\x11"), Key::Ctrl('q'));
    assert_eq!(key(&[]), Key::Ignored);
}

/// A mouse report is consumed whole and means nothing.
///
/// The bug this prevents: read in eight-byte chunks, a thirteen-byte report is cut in two and the
/// tail arrives looking like typed text. tmux and hexe both have mouse reporting on, so every
/// movement of the pointer typed into whatever was reading.
#[test]
fn a_mouse_report_is_swallowed() {
    let report = b"\x1b[<35;56;12M";
    match parse(report) {
        Parsed::Mouse(used, _) => assert_eq!(used, report.len(), "the whole report must go"),
        Parsed::Took(used, Key::Ignored) | Parsed::Discard(used) => {
            assert_eq!(used, report.len(), "the whole report must go")
        }
        other => panic!("mouse parse: {other:?}"),
    }
    // The X10 encoding, whose three trailing bytes are not part of the sequence by any rule.
    match parse(b"\x1b[M\x20\x21\x22") {
        Parsed::Discard(used) => assert_eq!(used, 6),
        other => panic!("x10 mouse: {other:?}"),
    }
}

#[test]
fn unsolicited_mouse_is_swallowed_unless_enabled() {
    use std::os::fd::AsRawFd;

    let (reader, writer) = nix::unistd::pipe().expect("a pipe");
    nix::unistd::write(&writer, b"\x1b[<0;12;7Mx").expect("mouse and text");
    let mut keys = Keys::on(reader.as_raw_fd());
    assert_eq!(keys.read_event(), Some(InputEvent::Key(Key::Char('x'))));

    let (reader, writer) = nix::unistd::pipe().expect("a pipe");
    nix::unistd::write(&writer, b"\x1b[<0;12;7M").expect("mouse");
    let mut keys = Keys::editor(reader.as_raw_fd(), Vec::new(), true);
    assert!(matches!(keys.read_event(), Some(InputEvent::Mouse(_))));
}

#[test]
fn unread_events_are_returned_in_fifo_order() {
    use std::os::fd::AsRawFd;

    let (reader, _writer) = nix::unistd::pipe().expect("a pipe");
    let mut keys = Keys::on(reader.as_raw_fd());
    keys.unread_event(InputEvent::Key(Key::Char('x')));
    keys.unread_event(InputEvent::Paste("two".to_string()));
    assert_eq!(keys.read_event(), Some(InputEvent::Key(Key::Char('x'))));
    assert_eq!(
        keys.read_event(),
        Some(InputEvent::Paste("two".to_string()))
    );
}

#[test]
fn bracketed_paste_markers_are_swallowed() {
    assert_eq!(parse(b"\x1b[200~"), Parsed::PasteStart(6));
    match parse(b"\x1b[201~") {
        Parsed::Took(used, Key::Ignored) | Parsed::Discard(used) => assert_eq!(used, 6),
        other => panic!("paste end leaked: {other:?}"),
    }
}

#[test]
fn bracketed_paste_is_one_owned_event() {
    use std::os::fd::AsRawFd;

    let (reader, writer) = nix::unistd::pipe().expect("a pipe");
    nix::unistd::write(&writer, b"\x1b[200~echo one\necho two\x1b[201~x").expect("paste");
    let mut keys = Keys::on(reader.as_raw_fd());
    assert_eq!(
        keys.read_event(),
        Some(InputEvent::Paste("echo one\necho two".to_string()))
    );
    assert_eq!(keys.read_event(), Some(InputEvent::Key(Key::Char('x'))));
}

#[test]
fn empty_and_escape_filled_pastes_are_owned_events() {
    use std::os::fd::AsRawFd;

    let (reader, writer) = nix::unistd::pipe().expect("a pipe");
    nix::unistd::write(
        &writer,
        b"\x1b[200~\x1b[201~\x1b[200~a\x1b[31mb\x1b[0m\x1b[201~",
    )
    .expect("pastes");
    let mut keys = Keys::on(reader.as_raw_fd());
    assert_eq!(keys.read_event(), Some(InputEvent::Paste(String::new())));
    assert_eq!(
        keys.read_event(),
        Some(InputEvent::Paste("a\x1b[31mb\x1b[0m".to_string()))
    );
}

#[test]
fn every_split_paste_marker_waits() {
    for at in 1..b"\x1b[200~".len() {
        assert_eq!(parse(&b"\x1b[200~"[..at]), Parsed::Partial, "split {at}");
    }
    let mut paste = paste::Paste::new();
    for byte in b"line one\nline two\x1b[201~" {
        let done = paste.push(*byte);
        if *byte != b'~' {
            assert!(!done);
        }
    }
    assert_eq!(paste.finish(), Ok("line one\nline two".to_string()));
}

#[test]
fn an_osc_reply_is_swallowed() {
    match parse(b"\x1b]11;rgb:1e1e/1e1e/2e2e\x1b\\") {
        Parsed::Discard(used) => assert_eq!(used, 25),
        other => panic!("osc leaked: {other:?}"),
    }
    match parse(b"\x1b]0;title\x07") {
        Parsed::Discard(used) => assert_eq!(used, 10),
        other => panic!("osc with BEL leaked: {other:?}"),
    }
}

#[test]
fn a_late_csi_query_reply_is_swallowed_whole() {
    use std::os::fd::AsRawFd;

    let (reader, writer) = nix::unistd::pipe().expect("a pipe");
    nix::unistd::write(&writer, b"\x1b[?1ux").expect("late reply and text");
    let mut keys = Keys::on(reader.as_raw_fd());
    assert_eq!(keys.read_event(), Some(InputEvent::Key(Key::Ignored)));
    assert_eq!(keys.read_event(), Some(InputEvent::Key(Key::Char('x'))));
}

/// A sequence that has not all arrived waits instead of being read as text. This is the half of
/// the fix that matters: the old code could not wait, so it guessed.
#[test]
fn a_split_sequence_waits_for_the_rest() {
    assert!(matches!(parse(b"\x1b"), Parsed::Partial));
    assert!(matches!(parse(b"\x1b["), Parsed::Partial));
    assert!(matches!(parse(b"\x1b[<35;56"), Parsed::Partial));
    assert!(matches!(parse(b"\x1b]11;rgb:1e"), Parsed::Partial));
    assert!(matches!(parse(&"é".as_bytes()[..1]), Parsed::Partial));
}

#[test]
fn text_is_taken_a_character_at_a_time() {
    assert!(matches!(parse(b"abc"), Parsed::Took(1, Key::Char('a'))));
    assert!(matches!(
        parse("éx".as_bytes()),
        Parsed::Took(2, Key::Char('é'))
    ));
}

/// An arrow arriving in the same read as the text before it must not swallow that text.
#[test]
fn a_key_after_text_is_not_lost() {
    let buf = b"ab\x1b[A";
    assert!(matches!(parse(buf), Parsed::Took(1, Key::Char('a'))));
    assert!(matches!(parse(&buf[1..]), Parsed::Took(1, Key::Char('b'))));
    assert!(matches!(parse(&buf[2..]), Parsed::Took(3, Key::Up)));
}

/// Alt chords, which a terminal spells `ESC` then the character.
///
/// Worth pinning because a config may bind any of them and the decoding is easy to lose: an
/// earlier version discarded `ESC x` outright, so `oslo.keys["alt-p"]` could never fire.
#[test]
fn esc_then_a_character_is_an_alt_chord() {
    assert_eq!(key(&[0x1b, b'p']), Key::Alt('p'));
    assert_eq!(key(&[0x1b, b'b']), Key::Alt('b'));
    assert_eq!(key(&[0x1b, 0x7f]), Key::Alt('\x7f'), "M-DEL");
    // And the whole two-byte sequence is consumed, so the character is not also read as text.
    assert_eq!(parse(&[0x1b, b'p']), Parsed::Took(2, Key::Alt('p')));
}

/// Every control chord decodes, because a config may bind any of them. The ones with a shared
/// meaning keep it.
#[test]
fn control_chords_decode_by_letter() {
    assert_eq!(key(&[0x07]), Key::Ctrl('g'));
    assert_eq!(key(&[0x0f]), Key::Ctrl('o'));
    assert_eq!(key(&[0x01]), Key::Home, "C-a keeps its shared meaning");
    assert_eq!(key(&[0x09]), Key::ToggleScope, "Tab is not C-i here");
    // Ctrl-Space arrives as NUL and used to fall through to the text path, where it became
    // nothing at all — which is why the finder could not be given it as a key.
    assert_eq!(key(&[0x00]), Key::Ctrl(' '), "C-Space");
}

/// **What follows the Enter is left where it was, not swallowed.**
///
/// A `Keys` lives for one line. It used to read 64 bytes at a time, so a burst carrying more than
/// one line had the rest sitting in its buffer when the line ended — and the buffer died with it.
/// Pasting three commands ran one and silently dropped two, where bash, zsh and dash run three.
///
/// The fix is not to carry the leftovers into the next line: after `cat`, the rest of a paste
/// belongs to `cat`'s stdin. It is to never take them, which is what this asserts — the reader
/// stops at the key it was asked for, and the remaining bytes are still there for whoever is next.
#[test]
fn the_reader_stops_at_the_key_it_needed() {
    use std::os::fd::AsRawFd;

    let (reader, writer) = nix::unistd::pipe().expect("a pipe");
    nix::unistd::write(&writer, b"ab\ncd\n").expect("write the burst");

    let mut keys = Keys::on(reader.as_raw_fd());
    assert_eq!(keys.read(), Some(Key::Char('a')));
    assert_eq!(keys.read(), Some(Key::Char('b')));
    assert_eq!(keys.read(), Some(Key::Accept), "the line ends here");

    assert!(
        keys.buf.is_empty(),
        "nothing may be held once the line has ended: {:?}",
        keys.buf
    );

    // Still in the pipe, and readable by anyone — the next line, or the command about to run.
    drop(keys);
    let mut rest = [0u8; 8];
    let n = nix::unistd::read(reader.as_raw_fd(), &mut rest).expect("the rest is still there");
    assert_eq!(
        &rest[..n],
        b"cd\n",
        "the bytes after Enter were never taken"
    );
}

/// A multi-byte sequence still assembles, which is the thing the chunked read was mistakenly
/// believed to be doing. The buffer and `parse` do it, at any read size.
#[test]
fn an_escape_sequence_assembles_one_byte_at_a_time() {
    use std::os::fd::AsRawFd;

    let (reader, writer) = nix::unistd::pipe().expect("a pipe");
    nix::unistd::write(&writer, b"\x1b[Ax").expect("an arrow and a letter");

    let mut keys = Keys::on(reader.as_raw_fd());
    assert_eq!(keys.read(), Some(Key::Up), "three bytes, one key");
    assert_eq!(keys.read(), Some(Key::Char('x')));
}

#[test]
fn a_nonblocking_terminal_waits_instead_of_becoming_eof() {
    use std::os::fd::AsRawFd;

    let (reader, writer) = nix::unistd::pipe().expect("a pipe");
    // SAFETY: the descriptor is live and the command changes only its file status flags.
    unsafe {
        let flags = nix::libc::fcntl(reader.as_raw_fd(), nix::libc::F_GETFL);
        assert!(flags >= 0);
        assert_eq!(
            nix::libc::fcntl(
                reader.as_raw_fd(),
                nix::libc::F_SETFL,
                flags | nix::libc::O_NONBLOCK,
            ),
            0
        );
    }
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(20));
        nix::unistd::write(&writer, b"x").expect("write after poll");
    });

    let mut keys = Keys::on(reader.as_raw_fd());
    assert_eq!(keys.read_event(), Some(InputEvent::Key(Key::Char('x'))));
}

/// **Ctrl-S reaches the editor rather than the line discipline.**
///
/// `IXON` makes the terminal read Ctrl-S as XOFF and stop painting, so the documented
/// `oslo.keys["ctrl-s"]` binding never saw the byte and the screen looked frozen — with Ctrl-Q,
/// unbindable for the same reason, the only way out.
#[test]
fn the_editor_owns_ctrl_s() {
    use nix::sys::termios::{InputFlags, LocalFlags};
    let Ok(pty) = nix::pty::openpty(None, None) else {
        return;
    };
    let handle = unsafe { std::os::fd::BorrowedFd::borrow_raw(pty.slave.as_raw_fd()) };
    let original = nix::sys::termios::tcgetattr(handle).expect("a pty answers tcgetattr");
    assert!(
        original.input_flags.contains(InputFlags::IXON),
        "the default line discipline does hold Ctrl-S"
    );

    let raw = super::editor_termios(&original);
    assert!(!raw.input_flags.contains(InputFlags::IXON));
    // The flags that were already right have not moved.
    assert!(!raw.local_flags.contains(LocalFlags::ICANON));
    assert!(!raw.local_flags.contains(LocalFlags::ECHO));
    assert!(!raw.local_flags.contains(LocalFlags::ISIG));
}

/// **The editor draws through whatever the last program left behind, unless it undoes it first.**
///
/// A program that exits badly can leave the G0 charset in line drawing (`\e(0`), so every letter
/// after it is a box character; autowrap off (`\e[?7l`), so a long line piles up in the last
/// column; or a stray SGR, so everything is bold. None is the shell's doing and all make its output
/// look broken — and having to run `reset` to clear them is the complaint this answers.
#[test]
fn the_editor_undoes_what_the_last_program_left_behind() {
    assert_eq!(SANITISE, b"\x1b[0m\x1b(B\x1b[?7h");

    // Each half named, so a future edit cannot quietly drop one.
    let text = String::from_utf8(SANITISE.to_vec()).expect("ascii");
    assert!(text.contains("\x1b[0m"), "attributes off");
    assert!(
        text.contains("\x1b(B"),
        "ASCII into G0, undoing line drawing"
    );
    assert!(text.contains("\x1b[?7h"), "autowrap on");

    // **Not a full reset.** `RIS` would clear the screen and drop the scrollback, which is a thing
    // to do when asked and never a thing to do before every prompt.
    assert!(!text.contains("\x1bc"), "must not be RIS");
    assert!(!text.contains("\x1b[2J"), "must not clear the screen");
}

/// **`dumb` and empty mean no screen; anything else, including nothing, means there is one.**
///
/// The asymmetry is the point. `TERM=dumb` is a statement that the terminal cannot draw, and
/// everything gated on it is work whose only product is something drawn — a colour, a mark, a frame
/// of an animation, and with an external prompt a *process per frame*. An unset `TERM` is not that
/// statement: it is the ordinary state of a capable terminal nobody has told, and reading it as
/// "no screen" would turn oslo monochrome wherever the variable was forgotten.
#[test]
fn a_dumb_terminal_draws_nothing_and_an_unset_one_is_not_dumb() {
    assert!(!super::drawn_by(Some("dumb")));
    assert!(!super::drawn_by(Some("")));
    assert!(super::drawn_by(None), "unset is not dumb");
    assert!(super::drawn_by(Some("xterm-256color")));
    assert!(super::drawn_by(Some("screen")));
    // Not a prefix test: `dumb-emacs-ansi` is a real terminal that does draw.
    assert!(super::drawn_by(Some("dumb-emacs-ansi")));
}
