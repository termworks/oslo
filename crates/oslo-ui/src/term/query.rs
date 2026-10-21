//! Bounded terminal queries that return unrelated input to the caller.

use std::os::fd::BorrowedFd;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const MAX_REPLY: usize = 4096;
const MAX_TOTAL: usize = 16384;
/// How long to keep reading after the last query has been answered.
///
/// A terminal answering for itself is already silent by now, so this is what a plain session pays
/// at startup — once. Anything emulating a terminal in between answers first and forwards second,
/// and this is the room its replies need to arrive in rather than land in the first prompt.
const SETTLE_MS: u64 = 20;
static STARTUP_PENDING: Mutex<Vec<u8>> = Mutex::new(Vec::new());
static HELD_ECHO: Mutex<Option<(i32, nix::sys::termios::Termios)>> = Mutex::new(None);

/// How the bytes collected for one possible reply should be handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyMatch {
    Prefix,
    Complete,
    Reject,
}

/// Classifies a byte stream while preserving bytes that are not the requested reply.
#[derive(Debug)]
pub struct Broker<F> {
    classify: F,
    candidate: Vec<u8>,
    pending: Vec<u8>,
    limit: usize,
}

impl<F> Broker<F>
where
    F: Fn(&[u8]) -> ReplyMatch,
{
    pub fn new(classify: F, limit: usize) -> Self {
        Self {
            classify,
            candidate: Vec::new(),
            pending: Vec::new(),
            limit: limit.min(MAX_REPLY),
        }
    }

    /// Accept one byte and return a complete reply when one has arrived.
    pub fn push(&mut self, byte: u8) -> Option<Vec<u8>> {
        self.candidate.push(byte);
        loop {
            match (self.classify)(&self.candidate) {
                ReplyMatch::Prefix if self.candidate.len() <= self.limit => return None,
                ReplyMatch::Complete if self.candidate.len() <= self.limit => {
                    return Some(std::mem::take(&mut self.candidate));
                }
                ReplyMatch::Prefix | ReplyMatch::Complete | ReplyMatch::Reject => {
                    self.pending.push(self.candidate.remove(0));
                    if self.candidate.is_empty() {
                        return None;
                    }
                }
            }
        }
    }

    /// Return every byte that was not part of the matched reply.
    pub fn finish(mut self) -> Vec<u8> {
        self.pending.append(&mut self.candidate);
        self.pending
    }
}

/// Send one request and wait for its reply without losing unrelated input.
pub fn query_sequence<F>(
    fd: i32,
    request: &[u8],
    timeout_ms: u64,
    classify: F,
) -> (Option<Vec<u8>>, Vec<u8>)
where
    F: Fn(&[u8]) -> ReplyMatch,
{
    let (mut replies, pending) = query_sequences(fd, request, timeout_ms, classify, |replies| {
        !replies.is_empty()
    });
    (replies.pop(), pending)
}

/// Send one request batch and collect replies until `complete` accepts their order.
pub fn query_sequences<F, C>(
    fd: i32,
    request: &[u8],
    timeout_ms: u64,
    classify: F,
    complete: C,
) -> (Vec<Vec<u8>>, Vec<u8>)
where
    F: Fn(&[u8]) -> ReplyMatch,
    C: Fn(&[Vec<u8>]) -> bool,
{
    let Ok(original) = termios(fd) else {
        return (Vec::new(), Vec::new());
    };
    let mut raw = original.clone();
    nix::sys::termios::cfmakeraw(&mut raw);
    if set_termios(fd, &raw).is_err() {
        return (Vec::new(), Vec::new());
    }
    let _restore = TermiosRestore { fd, original };

    if !write_all(fd, request) {
        return (Vec::new(), Vec::new());
    }

    let mut broker = Broker::new(classify, MAX_REPLY);
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut replies = Vec::new();
    let mut total = 0usize;
    // **The completion test shortens the wait; it does not end it.**
    //
    // `complete` is "someone answered the last thing we asked", and the caller uses a reply that
    // every terminal gives — so a single terminal answering it means the earlier queries are done,
    // because a terminal answers in order. Ending the read there was the obvious next step and it
    // is wrong, because *someone* is not *the terminal*.
    //
    // Anything that emulates a terminal will answer from its own emulation while the queries it
    // forwarded are still in flight: a multiplexer, a session over ssh, `script`, an editor's
    // embedded terminal. The read stopped, and the real replies arrived afterwards — into the line
    // editor, where an answer about the keyboard protocol is indistinguishable from somebody
    // pressing keys.
    //
    // So completion starts a short settle instead. A terminal on its own goes quiet immediately
    // and pays the settle once at startup; anything with a layer in between gets its late replies
    // read, classified, and — since they arrive before the settle expires — actually believed.
    // Bytes that are not replies are preserved either way, so a keystroke typed into the window is
    // never eaten.
    let mut settled: Option<Instant> = None;
    while Instant::now() < settled.unwrap_or(deadline).min(deadline) {
        let until = settled.unwrap_or(deadline).min(deadline);
        let remaining = until.saturating_duration_since(Instant::now());
        if !wait(fd, remaining) {
            break;
        }
        let mut byte = 0u8;
        // SAFETY: one live output byte is supplied for one byte read from the caller's descriptor.
        let read = unsafe { nix::libc::read(fd, (&mut byte as *mut u8).cast(), 1) };
        if read != 1 {
            break;
        }
        total += 1;
        if total > MAX_TOTAL {
            break;
        }
        if let Some(found) = broker.push(byte) {
            replies.push(found);
            if settled.is_none() && complete(&replies) {
                settled = Some(Instant::now() + Duration::from_millis(SETTLE_MS));
            }
        }
    }
    (replies, broker.finish())
}

fn termios(fd: i32) -> Result<nix::sys::termios::Termios, nix::errno::Errno> {
    // SAFETY: the descriptor is borrowed only for this call.
    let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
    nix::sys::termios::tcgetattr(borrowed)
}

fn set_termios(fd: i32, value: &nix::sys::termios::Termios) -> Result<(), nix::errno::Errno> {
    // SAFETY: the descriptor is borrowed only for this call.
    let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
    nix::sys::termios::tcsetattr(borrowed, nix::sys::termios::SetArg::TCSANOW, value)
}

struct TermiosRestore {
    fd: i32,
    original: nix::sys::termios::Termios,
}

impl Drop for TermiosRestore {
    fn drop(&mut self) {
        let _ = set_termios(self.fd, &self.original);
    }
}

fn write_all(fd: i32, mut bytes: &[u8]) -> bool {
    while !bytes.is_empty() {
        // SAFETY: the slice is live for the duration of this write.
        let written = unsafe { nix::libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if written > 0 {
            bytes = &bytes[written as usize..];
        } else if written < 0
            && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
        {
            continue;
        } else {
            return false;
        }
    }
    true
}

fn wait(fd: i32, remaining: Duration) -> bool {
    let timeout = i32::try_from(remaining.as_millis()).unwrap_or(i32::MAX);
    let mut pollfd = nix::libc::pollfd {
        fd,
        events: nix::libc::POLLIN,
        revents: 0,
    };
    loop {
        // SAFETY: one valid poll descriptor is supplied for this call.
        let ready = unsafe { nix::libc::poll(&mut pollfd, 1, timeout) };
        if ready >= 0 {
            return ready > 0;
        }
        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return false;
        }
    }
}

/// What the terminal said its background is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Background {
    Dark,
    Light,
}

/// Read the zero-round-trip background hint.
pub fn background_from_environment() -> Option<Background> {
    from_colorfgbg(std::env::var("COLORFGBG").ok().as_deref())
}

/// Query OSC 11 on an owned terminal descriptor.
pub fn background_on(fd: i32, timeout_ms: u64) -> (Option<Background>, Vec<u8>) {
    if let Some(background) = background_from_environment() {
        return (Some(background), Vec::new());
    }
    let (reply, pending) = query_sequence(fd, b"\x1b]11;?\x1b\\", timeout_ms, osc11_match);
    (reply.as_deref().and_then(parse_background_bytes), pending)
}

/// Stop the line discipline echoing a reply that arrives after the exchange ended.
///
/// Held from the end of the startup exchange to the first prompt, where the editor
/// takes the terminal in raw mode and drops a reply nothing is waiting for.
pub fn hold_echo(fd: i32) {
    // Only stdin, whose descriptor outlives the hold; a borrowed /dev/tty is closed before it.
    if fd != nix::libc::STDIN_FILENO {
        return;
    }
    let Ok(original) = termios(fd) else {
        return;
    };
    let mut quiet = original.clone();
    quiet
        .local_flags
        .remove(nix::sys::termios::LocalFlags::ECHO);
    if set_termios(fd, &quiet).is_err() {
        return;
    }
    *HELD_ECHO.lock().unwrap_or_else(|error| error.into_inner()) = Some((fd, original));
}

/// Give echo back, or do nothing when it was never held.
pub fn release_echo() {
    let held = HELD_ECHO
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take();
    if let Some((fd, original)) = held {
        let _ = set_termios(fd, &original);
    }
}

/// Preserve input consumed by a startup query until the first editor owns the terminal.
pub fn preserve_startup_input(mut bytes: Vec<u8>) {
    STARTUP_PENDING
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .append(&mut bytes);
}

/// Take input preserved by startup queries.
pub fn take_startup_input() -> Vec<u8> {
    std::mem::take(
        &mut *STARTUP_PENDING
            .lock()
            .unwrap_or_else(|error| error.into_inner()),
    )
}

fn osc11_match(bytes: &[u8]) -> ReplyMatch {
    const PREFIX: &[u8] = b"\x1b]11;rgb:";
    if bytes.len() <= PREFIX.len() {
        return if PREFIX.starts_with(bytes) {
            ReplyMatch::Prefix
        } else {
            ReplyMatch::Reject
        };
    }
    if !bytes.starts_with(PREFIX) {
        return ReplyMatch::Reject;
    }
    if bytes.ends_with(b"\x07") || bytes.ends_with(b"\x1b\\") {
        ReplyMatch::Complete
    } else {
        ReplyMatch::Prefix
    }
}

fn from_colorfgbg(value: Option<&str>) -> Option<Background> {
    let number: u8 = value?.rsplit(';').next()?.trim().parse().ok()?;
    Some(match number {
        0..=6 | 8 => Background::Dark,
        _ => Background::Light,
    })
}

pub fn parse_background(reply: &str) -> Option<Background> {
    parse_background_bytes(reply.as_bytes())
}

fn parse_background_bytes(reply: &[u8]) -> Option<Background> {
    let reply = std::str::from_utf8(reply).ok()?;
    let body = reply.split("rgb:").nth(1)?;
    let mut parts = body.split('/');
    let mut channel = || -> Option<f64> {
        let raw: String = parts
            .next()?
            .chars()
            .take_while(|c| c.is_ascii_hexdigit())
            .collect();
        if raw.is_empty() {
            return None;
        }
        let value = u32::from_str_radix(&raw, 16).ok()?;
        let max = 16u32.pow(raw.len() as u32) - 1;
        Some(f64::from(value) / f64::from(max))
    };
    let (r, g, b) = (channel()?, channel()?, channel()?);
    let luma = 0.299 * r + 0.587 * g + 0.114 * b;
    Some(if luma < 0.5 {
        Background::Dark
    } else {
        Background::Light
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(bytes: &[u8]) -> (Option<Vec<u8>>, Vec<u8>) {
        let mut broker = Broker::new(osc11_match, 64);
        let mut reply = None;
        for &byte in bytes {
            if let Some(found) = broker.push(byte) {
                reply = Some(found);
                break;
            }
        }
        (reply, broker.finish())
    }

    #[test]
    fn split_reply_waits_for_bel_or_st() {
        let mut broker = Broker::new(osc11_match, 64);
        for &byte in b"\x1b]11;rgb:ff/ff/ff" {
            assert_eq!(broker.push(byte), None);
        }
        assert_eq!(
            broker.push(0x07),
            Some(b"\x1b]11;rgb:ff/ff/ff\x07".to_vec())
        );
    }

    #[test]
    fn no_reply_preserves_an_incomplete_candidate() {
        let (reply, pending) = feed(b"\x1b]11;rgb:12/34");
        assert_eq!(reply, None);
        assert_eq!(pending, b"\x1b]11;rgb:12/34");
    }

    #[test]
    fn typed_bytes_before_a_reply_are_preserved() {
        let (reply, pending) = feed(b"x\x1b]11;rgb:00/00/00\x1b\\");
        assert_eq!(reply, Some(b"\x1b]11;rgb:00/00/00\x1b\\".to_vec()));
        assert_eq!(pending, b"x");
    }

    #[test]
    fn an_escape_key_before_a_reply_does_not_hide_the_reply_prefix() {
        let (reply, pending) = feed(b"\x1b\x1b]11;rgb:00/00/00\x07");
        assert_eq!(reply, Some(b"\x1b]11;rgb:00/00/00\x07".to_vec()));
        assert_eq!(pending, b"\x1b");
    }

    #[test]
    fn oversized_candidate_is_returned_as_input() {
        let bytes = [b"\x1b]11;rgb:".as_slice(), &[b'a'; 80]].concat();
        let (reply, pending) = feed(&bytes);
        assert_eq!(reply, None);
        assert_eq!(pending, bytes);
    }

    #[test]
    fn background_parser_accepts_bel_and_st_forms() {
        assert_eq!(
            parse_background("\x1b]11;rgb:1e/1e/2e\x07"),
            Some(Background::Dark)
        );
        assert_eq!(
            parse_background("\x1b]11;rgb:f5f5/f5f5/f5f5\x1b\\"),
            Some(Background::Light)
        );
    }

    #[test]
    fn colorfgbg_uses_its_last_field() {
        assert_eq!(from_colorfgbg(Some("15;0")), Some(Background::Dark));
        assert_eq!(from_colorfgbg(Some("0;15")), Some(Background::Light));
        assert_eq!(from_colorfgbg(Some("15;default")), None);
    }

    #[test]
    fn startup_input_is_taken_once() {
        preserve_startup_input(b"typed".to_vec());
        assert_eq!(take_startup_input(), b"typed");
        assert!(take_startup_input().is_empty());
    }
}

#[cfg(test)]
mod settle_tests {
    use super::*;
    use std::io::Write;

    /// A pty pair, so the read loop is driven by a real descriptor rather than a mock.
    fn pty() -> (std::fs::File, std::fs::File) {
        let pair = nix::pty::openpty(None, None).expect("openpty");
        (pair.master.into(), pair.slave.into())
    }

    /// Every reply is a complete `\x1b[…c`, so `classify` stays trivial and the test is about the
    /// loop rather than about parsing.
    fn da1(bytes: &[u8]) -> ReplyMatch {
        match (bytes.starts_with(b"\x1b["), bytes.ends_with(b"c")) {
            (true, true) => ReplyMatch::Complete,
            (true, false) => ReplyMatch::Prefix,
            _ => match b"\x1b[".starts_with(bytes) {
                true => ReplyMatch::Prefix,
                false => ReplyMatch::Reject,
            },
        }
    }

    /// **The reply that satisfies `complete` does not end the read.**
    ///
    /// This is the whole bug. Something between the shell and the terminal answers first, from its
    /// own emulation, and the terminal's real replies are still in flight. Stopping there left them
    /// to arrive during the first prompt, where a report about the keyboard protocol is
    /// indistinguishable from somebody typing.
    #[test]
    fn a_late_reply_is_still_collected() {
        let (mut master, slave) = pty();
        let fd = std::os::fd::AsRawFd::as_raw_fd(&slave);
        // The impostor answers at once; the real terminal answers a few milliseconds later.
        master.write_all(b"\x1b[?1;2c").expect("write");
        master.flush().expect("flush");
        // **A millisecond, not five.** The reply only has to arrive inside the settle window, and
        // the assertion holds however early it lands — so the margin is what matters and the delay
        // is not. At five, the whole suite running sixteen tests wide could overshoot `SETTLE_MS`
        // and the late reply became input, which is a correct outcome for a reply that really was
        // that late and a wrong one for a test claiming otherwise.
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1));
            let _ = master.write_all(b"\x1b[?62;4c");
            let _ = master.flush();
            master
        });

        let (replies, pending) = query_sequences(fd, b"\x1b[c", 200, da1, |seen| !seen.is_empty());
        let _ = writer.join();

        assert_eq!(replies.len(), 2, "the late reply was dropped: {replies:?}");
        assert_eq!(replies[1], b"\x1b[?62;4c", "{replies:?}");
        assert!(
            pending.is_empty(),
            "nothing was mistaken for input: {pending:?}"
        );
    }

    /// A reply arriving after the settle has expired is *input*, not a reply — and is handed back
    /// rather than swallowed. Losing a keystroke would be the opposite bug.
    #[test]
    fn anything_after_the_settle_is_returned_as_input() {
        let (mut master, slave) = pty();
        let fd = std::os::fd::AsRawFd::as_raw_fd(&slave);
        master.write_all(b"\x1b[?1;2c").expect("write");
        master.flush().expect("flush");
        let (replies, _) = query_sequences(fd, b"\x1b[c", 200, da1, |seen| !seen.is_empty());
        assert_eq!(replies.len(), 1);

        // Typed after the shell stopped listening; the next read must still see it.
        master.write_all(b"ls\n").expect("write");
        master.flush().expect("flush");
        let mut buffer = [0u8; 3];
        let read = unsafe { nix::libc::read(fd, buffer.as_mut_ptr().cast(), 3) };
        assert_eq!(read, 3, "the keystrokes were consumed");
        assert_eq!(&buffer, b"ls\n");
    }

    /// A terminal answering only for itself goes quiet, so a plain session pays the settle once
    /// and no more — it must not sit out the whole timeout.
    #[test]
    fn a_quiet_terminal_does_not_wait_out_the_timeout() {
        let (mut master, slave) = pty();
        let fd = std::os::fd::AsRawFd::as_raw_fd(&slave);
        master.write_all(b"\x1b[?1;2c").expect("write");
        master.flush().expect("flush");

        let start = Instant::now();
        let (replies, _) = query_sequences(fd, b"\x1b[c", 2_000, da1, |seen| !seen.is_empty());
        let took = start.elapsed();

        assert_eq!(replies.len(), 1);
        assert!(
            took < Duration::from_millis(500),
            "waited {took:?} for a terminal that had finished"
        );
    }
}
