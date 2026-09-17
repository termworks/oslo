//! Whether an entry may go — the three ways `rm` decides, and the prompt behind two of them.
//!
//! `-f` never asks, `-i` always asks, and in between there is the write-protected prompt: the one
//! GNU raises for a file the user cannot write to, and only when someone is at the terminal to
//! answer it. A script's stdin is not a tty, so it is silent everywhere it would otherwise hang.
//!
//! Split from the walk when the file crossed the 600-line limit. It is one idea — asking — and the
//! walk beside it is another.

use super::{Level, Walk};
use nix::fcntl::AtFlags;
use nix::libc;
use nix::unistd::{AccessFlags, faccessat};
use std::ffi::OsString;
use std::io::IsTerminal;
use std::os::fd::AsRawFd;
use std::path::Path;

/// Whether this entry may go, for something named inside an open directory.
///
/// **`-f` never asks, `-i` always asks, and in between there is the write-protected prompt** — the
/// one GNU raises for a file the user cannot write to, and only when someone is at the terminal to
/// answer it. A script's stdin is not a tty, so this is silent everywhere it would otherwise hang.
pub(super) fn ask_at(
    walk: &Walk,
    shown: &str,
    kind: &str,
    parent: &Level,
    name: &OsString,
) -> bool {
    if walk.force {
        return true;
    }
    if walk.interactive {
        return ask(
            walk.prompt,
            &walk.origin,
            &format!("remove {kind} '{shown}'"),
        );
    }
    if !std::io::stdin().is_terminal() {
        return true;
    }
    let writable = faccessat(
        Some(parent.as_raw_fd()),
        name.as_os_str(),
        AccessFlags::W_OK,
        AtFlags::AT_EACCESS,
    )
    .is_ok();
    writable || protected(walk, shown, kind)
}

/// The same question about the operand, which has a path and no parent descriptor.
pub(super) fn ask_path(walk: &Walk, shown: &str, kind: &str, path: &Path) -> bool {
    if walk.force {
        return true;
    }
    if walk.interactive {
        return ask(
            walk.prompt,
            &walk.origin,
            &format!("remove {kind} '{shown}'"),
        );
    }
    if !std::io::stdin().is_terminal() || nix::unistd::access(path, AccessFlags::W_OK).is_ok() {
        return true;
    }
    protected(walk, shown, kind)
}

/// Ask `question`: oslo's own yes/no at a prompt, the `rm` line on stdin anywhere else.
pub fn ask(prompt: bool, origin: &str, question: &str) -> bool {
    if prompt && let Some(yes) = widget(&format!("rm: {question}?"), "Yes", "No") {
        return yes;
    }
    confirm(origin, question)
}

/// The write-protected question — once per `rm` at a prompt, and the answer holds for the rest.
///
/// **One question, not one per file.** A git repository's objects are all read-only, so GNU's
/// question per file is two hundred questions to remove one checkout. Nobody answers them: they
/// press Ctrl-C and reach for `sudo`, which is not even what is needed, because removing a
/// read-only file only needs its directory to be writable.
fn protected(walk: &Walk, shown: &str, kind: &str) -> bool {
    let line = || format!("remove write-protected {kind} '{shown}'");
    if !walk.prompt {
        return confirm(&walk.origin, &line());
    }
    if let Some(answer) = walk.protected.get() {
        return answer;
    }
    // The tail of the path, so the question fits one row: a git object's whole path wrapped, and
    // the wrapped half was left on the screen when the question closed.
    let short = match shown.char_indices().rev().nth(29) {
        Some((at, _)) => format!("…{}", &shown[at..]),
        None => shown.to_string(),
    };
    let question = format!("rm: '{short}' is write-protected. Remove all such files?");
    match widget(&question, "Remove them", "Skip them") {
        Some(answer) => {
            walk.protected.set(Some(answer));
            answer
        }
        None => confirm(&walk.origin, &line()),
    }
}

/// oslo's yes/no, or `None` when there is no terminal to draw it on.
///
/// Esc and Ctrl-C are a no that also stops the `rm`, as Ctrl-C at the stdin prompt does.
pub fn widget(question: &str, yes: &str, no: &str) -> Option<bool> {
    let spec = oslo_ui::ask::Confirm {
        question: question.to_string(),
        yes: yes.to_string(),
        no: no.to_string(),
        ..Default::default()
    };
    match oslo_ui::ask::confirm(&spec) {
        oslo_ui::ask::Answer::Given(yes) => Some(yes),
        oslo_ui::ask::Answer::Cancelled => {
            crate::exec::job::note_interrupt();
            Some(false)
        }
        oslo_ui::ask::Answer::NoTerminal => None,
    }
}

/// Anything but a `y` answer means no, as it does in `rm` and in `find -ok`.
pub fn confirm(origin: &str, question: &str) -> bool {
    eprint!("{origin}rm: {question}? ");
    let _ = std::io::Write::flush(&mut std::io::stderr());
    match answer() {
        Some(answer) => matches!(answer.trim_start().chars().next(), Some('y') | Some('Y')),
        // Interrupted, or end of input: either way this entry does not go. The walk stops at its
        // next poll of `job::interrupt_waiting`, which is what turns one refused prompt into the
        // whole `rm` ending.
        None => false,
    }
}

/// One line from the terminal, or `None` if a Ctrl-C arrived while waiting for it.
///
/// **`read_line` cannot be used here, and that was the bug.** SIGINT is installed without
/// `SA_RESTART` precisely so a blocking read fails with `EINTR` rather than resuming — see
/// `exec::job::signals`. But `BufRead::read_line` is built on `read_until`, which treats
/// `ErrorKind::Interrupted` as "try again" and loops. So the keystroke set the interrupt flag, the
/// read went straight back to waiting, and nothing looked at the flag until an answer arrived that
/// was never coming:
///
/// ```text
/// oslo: rm: remove write-protected regular file '…/objects/a6/b653…'? ^C^C^C^C^C^C
/// ```
///
/// `rm` is a *builtin*, so there is no child for the terminal driver to kill — the shell itself is
/// the foreground process, and a prompt it will not abandon is a prompt nothing can escape. The
/// real `rm` dies on the first Ctrl-C because it is somebody else's process; this has to arrange
/// the same ending for itself.
///
/// So the bytes are read one at a time and `EINTR` is answered rather than retried.
///
/// # `EINTR` alone is not enough, and testing the flag first is not either
///
/// `EINTR` only reports a signal that arrived **while the process was inside `read(2)`**. One
/// delivered a moment earlier — between printing the question and entering the read — runs the
/// handler, sets the flag, and leaves nothing pending: the read then blocks on a keystroke nobody
/// is going to type. Same hang, narrower door.
///
/// Testing the flag before reading does not close that window either, it only narrows it: a signal
/// can still land between the test and the `read`. That is a plain time-of-check race, and no
/// re-ordering fixes it.
///
/// So the wait is on **two** descriptors — the keystroke and the signal self-pipe
/// ([`crate::exec::job::interrupt_fd`]) — and there is no window at all. A SIGINT that arrived
/// before the wait began has already left its byte, so `poll` returns at once; one that arrives
/// during it wakes the same `poll`. The flag is still tested first, because a `note_interrupt` from
/// this thread writes no byte.
///
/// # The descriptor is read directly, and that is not incidental
///
/// `std::io::stdin()` is a `BufReader`. Reading one byte through it pulls **everything available**
/// into an 8 KiB buffer, so the *next* prompt in the same `rm -ri` polls a descriptor that is empty
/// while its answer sits in that buffer, and waits for input that has already arrived. That is not
/// a hypothetical: mixing `poll` with the buffered handle takes `rm_race_tests` from 0.12s to never
/// finishing.
///
/// Reading the descriptor a byte at a time keeps `poll` and `read` talking about the same thing.
/// Nothing is lost by skipping the buffer, because nothing else leaves anything in it — every other
/// reader of stdin in the shell (`run_stdin`, `copy`, the list widgets) takes it to EOF in one
/// `read_to_string`.
fn answer() -> Option<String> {
    let stdin = std::io::stdin();
    let mut line = Vec::new();
    loop {
        // A local interrupt writes no byte to the pipe, so the flag is still the first question.
        if crate::exec::job::interrupt_waiting() {
            return None;
        }
        if wait_for_input(&stdin) == Waited::Interrupted {
            return None;
        }
        match read_one(&stdin) {
            // End of input: `rm` treats that as "no", which is what a script piping nothing means.
            Read1::Eof => break,
            Read1::Byte(b'\n') => break,
            Read1::Byte(byte) => line.push(byte),
            Read1::Again => continue,
            Read1::Failed => return None,
        }
    }
    Some(String::from_utf8_lossy(&line).into_owned())
}

enum Read1 {
    Byte(u8),
    Eof,
    /// Interrupted or momentarily empty — go round, which tests the flag again.
    Again,
    Failed,
}

fn read_one(stdin: &std::io::Stdin) -> Read1 {
    use std::os::fd::AsRawFd;
    let mut byte = 0u8;
    let read = unsafe { libc::read(stdin.as_raw_fd(), std::ptr::from_mut(&mut byte).cast(), 1) };
    match read {
        0 => Read1::Eof,
        1 => Read1::Byte(byte),
        _ => match nix::errno::Errno::last() {
            nix::errno::Errno::EINTR | nix::errno::Errno::EAGAIN => Read1::Again,
            _ => Read1::Failed,
        },
    }
}

#[derive(PartialEq, Eq)]
enum Waited {
    Ready,
    Interrupted,
}

/// Wait until stdin has something, or a SIGINT arrives. No window between the two.
fn wait_for_input(stdin: &std::io::Stdin) -> Waited {
    use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
    use std::os::fd::AsFd;

    let Some(interrupt) = crate::exec::job::interrupt_fd() else {
        // No self-pipe means no interactive shell installed one — a test, or `-i` from a script.
        // There is no signal to wait for, so waiting on stdin alone is the whole of it.
        return Waited::Ready;
    };
    let mut fds = [
        PollFd::new(stdin.as_fd(), PollFlags::POLLIN),
        PollFd::new(interrupt, PollFlags::POLLIN),
    ];
    match poll(&mut fds, PollTimeout::NONE) {
        // `EINTR` is the signal arriving with the handler already run: the flag says so, and the
        // caller tests it at the top of the next round.
        Err(_) => Waited::Interrupted,
        Ok(_) => match fds[1].revents().is_some_and(|r| !r.is_empty()) {
            true => {
                crate::exec::job::drain_interrupt_fd();
                Waited::Interrupted
            }
            false => Waited::Ready,
        },
    }
}
