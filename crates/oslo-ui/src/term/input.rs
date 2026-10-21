mod idle;
mod name;

use super::child::CHILD_MARK;
use super::paste::{self, Paste};
use super::resize::RESIZE_MARK;
use idle::waiting;
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    /// The Insert key: `ESC [ 2 ~`, or kitty's functional code for it.
    Insert,
    Function(u8),
    Accept,
    Cancel,
    Abort,
    Clear,
    ToggleScope,
    BackTab,
    /// Ctrl+Enter: send the block, whatever plain Enter is set to do.
    ///
    /// Only a terminal that reports modifiers produces this — in the legacy encoding Ctrl-M *is*
    /// Enter, the same byte. Alt+Enter decodes to it as well, so a terminal without the kitty
    /// keyboard protocol still has one way to send.
    Submit,
    /// Ctrl+Tab: switch language. Reported only under the kitty keyboard protocol, for the same
    /// reason — plain Tab and Ctrl+Tab are both `0x09` otherwise.
    CtrlTab,
    Resized,
    Ctrl(char),
    Alt(char),
    Ignored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteError {
    TooLarge,
    InvalidUtf8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputEvent {
    Key(Key),
    Paste(String),
    PasteRejected(PasteError),
    Resized,
    Focus(bool),
    Mouse(super::mouse::Event),
    /// Something the frame draws landed while the editor was waiting — a prompt rebuilt off the
    /// thread, a suggestion that had to be asked for. Not a keystroke and never reaches a binding;
    /// it exists so the loop gets a turn to notice and redraw.
    Refreshed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pressed {
    Key(Key),
    Timeout,
    Ended,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventPressed {
    Event(InputEvent),
    Timeout,
    Ended,
}

pub fn key(bytes: &[u8]) -> Key {
    match bytes {
        [] => Key::Ignored,
        [RESIZE_MARK, ..] => Key::Resized,
        [0x1b] => Key::Cancel,
        [0x03] => Key::Abort,
        [0x0d] | [0x0a] => Key::Accept,
        [0x7f] | [0x08] => Key::Backspace,
        [0x09] => Key::ToggleScope,
        [0x01] => Key::Home,
        [0x05] => Key::End,
        [0x02] => Key::Left,
        [0x06] => Key::Right,
        [0x04] => Key::Delete,
        [0x15] => Key::Clear,
        [0x10] => Key::Up,
        [0x0e] => Key::Down,
        [0x0b] => Key::Ctrl('k'),
        [0x0c] => Key::Ctrl('l'),
        [0x12] => Key::Ctrl('r'),
        [0x14] => Key::Ctrl('t'),
        [0x17] => Key::Ctrl('w'),
        [0x19] => Key::Ctrl('y'),
        [0x1b, b'[', _rest @ ..] if bytes.ends_with(b"u") => super::keyboard::decode(bytes),
        [0x1b, b'[', rest @ ..] => match function_key(rest) {
            Some(function) => function,
            None => match rest {
                [b'A', ..] => Key::Up,
                [b'B', ..] => Key::Down,
                [b'C', ..] => Key::Right,
                [b'D', ..] => Key::Left,
                [b'H', ..] => Key::Home,
                [b'F', ..] => Key::End,
                [b'Z', ..] => Key::BackTab,
                [b'1', b'~', ..] | [b'7', b'~', ..] => Key::Home,
                [b'2', b'~', ..] => Key::Insert,
                [b'3', b'~', ..] => Key::Delete,
                [b'4', b'~', ..] | [b'8', b'~', ..] => Key::End,
                [b'5', b'~', ..] => Key::PageUp,
                [b'6', b'~', ..] => Key::PageDown,
                _ => Key::Ignored,
            },
        },
        [0x1b, b'O', rest @ ..] => match rest {
            [b'A', ..] => Key::Up,
            [b'B', ..] => Key::Down,
            [b'C', ..] => Key::Right,
            [b'D', ..] => Key::Left,
            [b'H', ..] => Key::Home,
            [b'F', ..] => Key::End,
            [b'P'] => Key::Function(1),
            [b'Q'] => Key::Function(2),
            [b'R'] => Key::Function(3),
            [b'S'] => Key::Function(4),
            _ => Key::Ignored,
        },
        // Ctrl-Space arrives as NUL, and there is nothing else it can be: no terminal sends a bare
        // zero byte for anything a person typed. Without this it fell through to `text_key` and was
        // inserted into the line as nothing at all.
        [0x00] => Key::Ctrl(' '),
        [byte @ 0x01..=0x1a] => Key::Ctrl((byte + b'a' - 1) as char),
        // The control bytes above the alphabet. A terminal that has not been asked for the Kitty
        // protocol sends these bare, with nothing to say they were a chord, and without this they
        // fall through to `text_key` and are inserted as text.
        [byte @ 0x1c..=0x1f] => Key::Ctrl(br"\]^_"[(byte - 0x1c) as usize] as char),
        [0x1b, 0x7f] => Key::Alt('\x7f'),
        [0x1b, rest @ ..] => text_key(rest, true),
        bytes => text_key(bytes, false),
    }
}

fn function_key(rest: &[u8]) -> Option<Key> {
    let (&final_byte, parameters) = rest.split_last()?;
    let parameters = std::str::from_utf8(parameters).ok()?;
    let mut fields = parameters.split(';');
    let first = fields.next()?;
    let mut modifier = fields.next().unwrap_or("1").split(':');
    let modifier_value = modifier.next()?.parse::<u16>().ok()?;
    let event = modifier.next();
    if !matches!(event, None | Some("1")) || modifier.next().is_some() {
        return Some(Key::Ignored);
    }
    if modifier_value != 1 || fields.next().is_some() {
        return Some(Key::Ignored);
    }
    let number = match final_byte {
        b'P' | b'Q' | b'S' if first.is_empty() => final_byte - b'P' + 1,
        b'P'..=b'S' if first == "1" => final_byte - b'P' + 1,
        b'~' => match first.parse::<u8>().ok()? {
            11 => 1,
            12 => 2,
            13 => 3,
            14 => 4,
            15 => 5,
            17 => 6,
            18 => 7,
            19 => 8,
            20 => 9,
            21 => 10,
            23 => 11,
            24 => 12,
            _ => return None,
        },
        _ => return None,
    };
    Some(Key::Function(number))
}

fn text_key(bytes: &[u8], alt: bool) -> Key {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Key::Ignored;
    };
    match text.chars().next() {
        // **Alt+Enter, the stand-in for Ctrl+Enter where that cannot arrive.** A terminal without
        // the kitty protocol sends it as `ESC` then `CR`, which landed on the control-character
        // arm below and was discarded. It decodes to `Submit` — the same key Ctrl+Enter produces —
        // so nothing downstream has to know which terminal it is on, and a prompt where Enter
        // inserts a newline always has a way to send. See `startup::mode`.
        Some('\r' | '\n') if alt => Key::Submit,
        Some(c) if !c.is_control() && alt => Key::Alt(c),
        Some(c) if !c.is_control() => Key::Char(c),
        _ => Key::Ignored,
    }
}

pub struct Keys {
    pub(crate) buf: Vec<u8>,
    fd: i32,
    paste: Option<Paste>,
    mouse_events: bool,
    unread: VecDeque<InputEvent>,
}

impl Keys {
    pub fn on(fd: i32) -> Keys {
        Keys {
            buf: Vec::new(),
            fd,
            paste: None,
            mouse_events: false,
            unread: VecDeque::new(),
        }
    }

    pub fn editor(fd: i32, pending: Vec<u8>, mouse_events: bool) -> Keys {
        Keys {
            buf: pending,
            fd,
            paste: None,
            mouse_events,
            unread: VecDeque::new(),
        }
    }

    pub fn fd(&self) -> i32 {
        self.fd
    }

    pub fn unread_event(&mut self, event: InputEvent) {
        self.unread.push_back(event);
    }

    pub fn extend_pending(&mut self, pending: Vec<u8>) {
        self.buf.extend(pending);
    }

    pub fn read_event(&mut self) -> Option<InputEvent> {
        if let Some(event) = self.unread.pop_front() {
            return Some(event);
        }
        loop {
            if self.paste.is_some() {
                if self.buf.is_empty() && !self.fill() {
                    self.paste = None;
                    return None;
                }
                let byte = self.buf.remove(0);
                if self.paste.as_mut().is_some_and(|paste| paste.push(byte)) {
                    let paste = self.paste.take().expect("active paste");
                    return Some(match paste.finish() {
                        Ok(text) => InputEvent::Paste(text),
                        Err(paste::Error::TooLarge) => {
                            InputEvent::PasteRejected(PasteError::TooLarge)
                        }
                        Err(paste::Error::InvalidUtf8) => {
                            InputEvent::PasteRejected(PasteError::InvalidUtf8)
                        }
                    });
                }
                continue;
            }
            match parse(&self.buf) {
                Parsed::Took(used, key) => {
                    self.buf.drain(..used);
                    return Some(if key == Key::Resized {
                        InputEvent::Resized
                    } else {
                        InputEvent::Key(key)
                    });
                }
                Parsed::Serviced(used) => {
                    self.buf.drain(..used);
                    return Some(InputEvent::Refreshed);
                }
                Parsed::Focus(used, focused) => {
                    self.buf.drain(..used);
                    return Some(InputEvent::Focus(focused));
                }
                Parsed::Mouse(used, event) => {
                    self.buf.drain(..used);
                    if self.mouse_events {
                        return Some(InputEvent::Mouse(event));
                    }
                }
                Parsed::PasteStart(used) => {
                    self.buf.drain(..used);
                    self.paste = Some(Paste::new());
                }
                Parsed::Discard(used) => {
                    self.buf.drain(..used);
                }
                Parsed::Partial => {
                    let delay = crate::settings::current().misc.escape_delay as i32;
                    if self.buf == [0x1b] && !waiting(self.fd, delay) {
                        self.buf.clear();
                        return Some(InputEvent::Key(Key::Cancel));
                    }
                    if !self.fill() {
                        return None;
                    }
                }
            }
        }
    }

    pub fn read(&mut self) -> Option<Key> {
        loop {
            match self.read_event()? {
                InputEvent::Key(key) => return Some(key),
                InputEvent::Resized => return Some(Key::Resized),
                InputEvent::Paste(_)
                | InputEvent::PasteRejected(_)
                | InputEvent::Focus(_)
                | InputEvent::Mouse(_)
                | InputEvent::Refreshed => {}
            }
        }
    }

    pub fn read_within(&mut self, ms: i32) -> Pressed {
        if !waiting(self.fd, ms) && !self.took_resize() && self.buf.is_empty() {
            return Pressed::Timeout;
        }
        match self.read() {
            Some(key) => Pressed::Key(key),
            None => Pressed::Ended,
        }
    }

    pub fn read_event_within(&mut self, ms: i32) -> EventPressed {
        if !waiting(self.fd, ms)
            && !self.took_resize()
            && self.buf.is_empty()
            && self.unread.is_empty()
        {
            return EventPressed::Timeout;
        }
        match self.read_event() {
            Some(event) => EventPressed::Event(event),
            None => EventPressed::Ended,
        }
    }

    /// Take a resize that arrived during the *timed* wait, and queue it as a marker.
    ///
    /// **The two waits have to agree, and this is the half that did not.** `waiting` polls, and a
    /// `SIGWINCH` interrupts the poll — but its `EINTR` is indistinguishable from a timeout in a
    /// `bool`, so it answered "nothing happened" and the resize sat in its flag until the next
    /// keystroke. The blocking wait has taken the flag since the marker was fixed; this one had not,
    /// so whether a resize redrew the line depended on which wait the editor happened to be sitting
    /// in — an idle hook armed, or a prompt being rebuilt off the thread, and it was the timed one.
    /// That is what made losing the prompt on resize feel random.
    ///
    /// Taken *here* rather than inside `waiting`, which has nowhere to put it: a flag consumed by
    /// something that cannot report it is worse than one left set.
    fn took_resize(&mut self) -> bool {
        if !super::resize::take_resize() {
            return false;
        }
        self.buf.push(RESIZE_MARK);
        true
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Parsed {
    Took(usize, Key),
    PasteStart(usize),
    Focus(usize, bool),
    Mouse(usize, super::mouse::Event),
    Discard(usize),
    /// Background work was done rather than a key produced. The loop gets a turn to repaint.
    Serviced(usize),
    Partial,
}

pub(crate) fn parse(buf: &[u8]) -> Parsed {
    let Some(&first) = buf.first() else {
        return Parsed::Partial;
    };
    // A child ended while the editor was waiting. Serviced here rather than handed onward as a key:
    // it is not one, no binding may claim it, and what the loop needs afterwards is a repaint —
    // which is exactly what `Refreshed` already means.
    if first == CHILD_MARK {
        oslo_base::background::service();
        return Parsed::Serviced(1);
    }
    // **The window changed size.** Taken here, beside the other marker, because the path below
    // cannot: `RESIZE_MARK` is `0xFF`, which is not a legal UTF-8 leading byte, so it failed
    // `from_utf8` and was thrown away as an undecodable character — and the `[RESIZE_MARK, ..]`
    // arm in [`key`] was never reached from here at all. A resize therefore set the flag, woke the
    // read, pushed the marker and vanished, leaving the line laid out for the old width until the
    // next command.
    if first == RESIZE_MARK {
        return Parsed::Took(1, Key::Resized);
    }
    if first != 0x1b {
        let len = utf8_len(first);
        if buf.len() < len {
            return Parsed::Partial;
        }
        return match std::str::from_utf8(&buf[..len]) {
            Ok(text) => Parsed::Took(len, key(text.as_bytes())),
            Err(_) => Parsed::Discard(len),
        };
    }
    match buf.get(1) {
        None => Parsed::Partial,
        Some(b'[') => parse_csi(buf),
        Some(b'O') => match buf.get(2) {
            None => Parsed::Partial,
            Some(_) => Parsed::Took(3, key(&buf[..3])),
        },
        Some(b']') => parse_osc(buf),
        Some(&next) => {
            let len = 1 + utf8_len(next);
            if buf.len() < len {
                Parsed::Partial
            } else {
                Parsed::Took(len, key(&buf[..len]))
            }
        }
    }
}

fn parse_csi(buf: &[u8]) -> Parsed {
    let mut at = 2;
    while at < buf.len() && (0x30..=0x3f).contains(&buf[at]) {
        at += 1;
    }
    while at < buf.len() && (0x20..=0x2f).contains(&buf[at]) {
        at += 1;
    }
    let Some(&final_byte) = buf.get(at) else {
        return Parsed::Partial;
    };
    if !(0x40..=0x7e).contains(&final_byte) {
        return Parsed::Discard(at + 1);
    }
    let used = at + 1;
    if &buf[..used] == paste::START {
        return Parsed::PasteStart(used);
    }
    if &buf[..used] == b"\x1b[I" {
        return Parsed::Focus(used, true);
    }
    if &buf[..used] == b"\x1b[O" {
        return Parsed::Focus(used, false);
    }
    if buf[..used].starts_with(b"\x1b[<")
        && let Some(event) = super::mouse::decode(&buf[..used])
    {
        return Parsed::Mouse(used, event);
    }
    if final_byte == b'M' && at == 2 {
        return if buf.len() >= 6 {
            Parsed::Discard(6)
        } else {
            Parsed::Partial
        };
    }
    Parsed::Took(used, key(&buf[..used]))
}

fn parse_osc(buf: &[u8]) -> Parsed {
    let mut at = 2;
    while at < buf.len() {
        if buf[at] == 0x07 {
            return Parsed::Discard(at + 1);
        }
        if buf[at] == 0x1b && buf.get(at + 1) == Some(&b'\\') {
            return Parsed::Discard(at + 2);
        }
        at += 1;
    }
    Parsed::Partial
}

fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A resize marker is taken, not thrown away.** `RESIZE_MARK` is `0xFF`, which is not a legal
    /// UTF-8 leading byte — so before this it fell into the character path below, failed
    /// `from_utf8` and was discarded as an undecodable byte. The `[RESIZE_MARK, ..]` arm in [`key`]
    /// was unreachable from here, and a resize set its flag, woke the read, pushed its marker and
    /// vanished: the line stayed laid out for the old width until the next command.
    #[test]
    fn a_resize_marker_is_taken_rather_than_discarded() {
        assert_eq!(parse(&[RESIZE_MARK]), Parsed::Took(1, Key::Resized));
        // With a keystroke queued behind it, the marker takes exactly its own byte.
        assert_eq!(parse(&[RESIZE_MARK, b'x']), Parsed::Took(1, Key::Resized));
    }

    /// The other marker keeps its own path: it is serviced here rather than handed on as a key.
    #[test]
    fn a_child_marker_is_serviced() {
        assert_eq!(parse(&[CHILD_MARK]), Parsed::Serviced(1));
    }

    /// Insert arrives as `ESC [ 2 ~` and is its own key, not an ignored sequence.
    #[test]
    fn insert_is_a_key() {
        assert_eq!(key(b"\x1b[2~"), Key::Insert);
        assert_eq!(key(b"\x1b[3~"), Key::Delete, "its neighbour is untouched");
    }
}
