//! Vi editing modes, motions, operators and counts.
//!
//! Motions `h l 0 ^ $ w W b B e E f F t T ; ,`, with counts. Operators `d c y` over any of those,
//! doubled (`dd`, `cc`, `yy`), and over a text object (`ciw`, `da"`, `yi(` — see
//! [`super::object`]). The single-key edits `x X D C s S r p P ~`. Entering insert with `i I a A`,
//! replace with `R`, and undo with `u`.

use super::buffer::Buffer;
use crate::term::Key;
pub use crate::vi::Mode;

/// What is waiting for its next key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    None,
    /// `d`, `c` or `y`, waiting for a motion.
    Operator(char),
    /// `f`, `F`, `t` or `T`, waiting for the character to search for.
    Find(char),
    /// `r`, waiting for the replacement character.
    Replace,
    /// An operator that has read `f`/`F`/`t`/`T` and is waiting for the character to search for —
    /// `df/`, which keeps two keys' worth of intent alive at once.
    OperatorFind(char, char),
    /// An operator that has read `i` or `a` and is waiting for which object — `ciw`, `da"`. The
    /// second field is the `i` or the `a`, which is the only difference between the two forms.
    OperatorObject(char, char),
}

/// The vi keymap's state between keys.
#[derive(Debug)]
pub struct Vi {
    pub mode: Mode,
    pending: Pending,
    /// Digits typed so far. `None` means no count, which is not the same as `Some(1)` for `0`.
    count: Option<usize>,
    /// The count that was typed *before* an operator, kept while its motion is read.
    ///
    /// Separate from `count` because vi allows one on each side and **multiplies** them: `2d3w`
    /// is six words. Reusing one field made the second count overwrite the first, and made a
    /// digit after an operator look like a motion — so `d2w` moved the cursor instead of deleting.
    operator_count: usize,
    /// The last `f`/`t` search, for `;` and `,`.
    last_find: Option<(char, char)>,
}

impl Default for Vi {
    fn default() -> Self {
        // Every line starts in insert, as vi's own command line does and as every shell that has
        // a vi mode does. Starting in normal would make the common case — type a command — need a
        // keystroke first.
        Vi {
            mode: Mode::Insert,
            pending: Pending::None,
            count: None,
            operator_count: 1,
            last_find: None,
        }
    }
}

/// What the caller should do about the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Handled here. `redraw` says whether anything visible changed.
    Handled { redraw: bool },
    /// Not a vi concern — the caller should run its ordinary (emacs) handling. This is how insert
    /// mode keeps every readline binding working, which is what vi users expect of a shell.
    Passthrough,
}

impl Vi {
    /// The count typed before a command, defaulting to 1, and consumed.
    fn take_count(&mut self) -> usize {
        self.count.take().unwrap_or(1).max(1)
    }

    /// Where a motion lands from `at`, and whether it **includes** the character it lands on.
    ///
    /// The inclusive flag is vi's central subtlety and the thing that is wrong in every
    /// half-implementation: `dw` stops *before* the next word, but `de` and `df/` take the
    /// character they land on. A bare motion ignores the flag — it only moves the cursor — so it
    /// matters exactly when an operator is reading the range.
    fn motion(&mut self, key: char, buf: &Buffer, at: usize, count: usize) -> Option<Aim> {
        let len = buf.len();
        let mut to = at;
        let mut inclusive = false;
        match key {
            'h' => to = buf.move_graphemes_from(to, -(count as isize)),
            'l' => to = buf.move_graphemes_from(to, count as isize),
            '0' => to = 0,
            '$' => to = len,
            '^' => {
                to = (0..len)
                    .find(|i| !buf.char_at(*i).is_some_and(char::is_whitespace))
                    .unwrap_or(0)
            }
            'w' | 'W' => {
                for _ in 0..count {
                    to = next_word(buf, to, key == 'W');
                }
            }
            'b' | 'B' => {
                for _ in 0..count {
                    to = prev_word(buf, to, key == 'B');
                }
            }
            // `e` lands *on* the last character of the word, not after it.
            'e' | 'E' => {
                for _ in 0..count {
                    to = buf.previous_grapheme(word_end(buf, to, key == 'E'));
                }
                inclusive = true;
            }
            // `;` and `,` repeat the last `f`/`t`, `,` in the other direction.
            ';' | ',' => {
                let (op, target) = self.last_find?;
                let op = if key == ',' { flip(op) } else { op };
                to = find(buf, to, op, target, count)?;
                inclusive = op == 'f' || op == 't';
            }
            _ => return None,
        }
        to = super::display::clamp_char_boundary(&buf.text(), to);
        Some(Aim { to, inclusive })
    }

    /// Apply `key`. `buf` is mutated in place.
    pub fn apply(&mut self, key: Key, buf: &mut Buffer) -> Outcome {
        let redrew = |yes: bool| Outcome::Handled { redraw: yes };

        // **Esc glued to the next key.** A terminal spells `M-x` as `ESC x`, so an Esc pressed
        // quickly enough before another key arrives as one two-byte sequence and decodes as Alt —
        // and vi users press Esc-then-key fast constantly. The timeout that tells the two apart
        // cannot help when both bytes are already in the buffer.
        //
        // Vi mode binds no Alt chord, so reading it as Esc followed by that key is unambiguous
        // here and is what was meant. Without this, `Esc0w` typed at speed does nothing at all.
        if let Key::Alt(c) = key
            && !c.is_control()
        {
            if self.mode != Mode::Normal {
                self.mode = Mode::Normal;
                buf.move_left();
            }
            self.pending = Pending::None;
            self.count = None;
            return self.normal(Key::Char(c), buf);
        }

        // Esc always returns to normal mode, and steps left as vi does — the cursor sits *on* a
        // character in normal mode, not after it.
        if key == Key::Cancel {
            if self.mode != Mode::Normal {
                self.mode = Mode::Normal;
                buf.move_left();
                self.pending = Pending::None;
                self.count = None;
                return redrew(true);
            }
            self.pending = Pending::None;
            self.count = None;
            return redrew(false);
        }

        match self.mode {
            // Insert and replace both defer to the ordinary keymap for everything except the one
            // character they treat differently.
            Mode::Insert => Outcome::Passthrough,
            Mode::Replace => match key {
                Key::Char(c) => {
                    if !buf.replace_at_cursor(c) {
                        buf.insert(c);
                    } else {
                        buf.move_right();
                    }
                    redrew(true)
                }
                _ => Outcome::Passthrough,
            },
            Mode::Normal => self.normal(key, buf),
        }
    }

    fn normal(&mut self, key: Key, buf: &mut Buffer) -> Outcome {
        let redrew = |yes: bool| Outcome::Handled { redraw: yes };
        // Enter and the interrupt keys belong to the caller in every mode, or a line could not be
        // run from normal mode.
        let Key::Char(c) = key else {
            return match key {
                Key::Accept | Key::Abort | Key::Delete => Outcome::Passthrough,
                // Arrows and Home/End work in normal mode too: a terminal is not a vi tutorial.
                Key::Left => {
                    buf.move_left();
                    redrew(true)
                }
                Key::Right => {
                    buf.move_right();
                    redrew(true)
                }
                Key::Up | Key::Down | Key::Insert => Outcome::Passthrough,
                Key::Home => {
                    buf.move_home();
                    redrew(true)
                }
                Key::End => {
                    buf.move_end();
                    redrew(true)
                }
                _ => redrew(false),
            };
        };

        // `r` and `f` take the very next key whatever it is — a digit is the character to find,
        // not a count. Checked before the count, which is the only reason `f2` works.
        match self.pending {
            Pending::Replace => {
                self.pending = Pending::None;
                buf.snapshot();
                let ok = buf.replace_at_cursor(c);
                return redrew(ok);
            }
            Pending::Find(op) => {
                self.pending = Pending::None;
                self.last_find = Some((op, c));
                let count = self.take_count();
                return match find(buf, buf.cursor(), op, c, count) {
                    Some(to) => {
                        buf.set_cursor(to);
                        redrew(true)
                    }
                    None => redrew(false),
                };
            }
            // `d` then `f` then the character: the operator waited through two keys, which is why
            // it needs a state of its own rather than being folded into `Find`.
            Pending::OperatorFind(op, find_op) => {
                self.pending = Pending::None;
                self.last_find = Some((find_op, c));
                let count = self.operator_count * self.take_count();
                self.operator_count = 1;
                let Some(to) = find(buf, buf.cursor(), find_op, c, count) else {
                    return redrew(false);
                };
                let inclusive = find_op == 'f' || find_op == 't';
                return self.operate_over(op, buf.cursor(), Aim { to, inclusive }, buf);
            }
            // `c` then `i` then `w`. Like the find above, this key is the object and never a
            // count: `ci2` is nothing, not "two of something".
            //
            // **An object answers the range outright**, so it does not go through `operate_over` —
            // there is no motion here to decide an inclusive end from, which is the whole reason
            // objects exist. A count is dropped rather than applied: `2ciw` in vi means two words,
            // and taking one while claiming to take two is worse than taking one and saying so.
            Pending::OperatorObject(op, form) => {
                self.pending = Pending::None;
                self.count = None;
                self.operator_count = 1;
                let Some(span) = super::object::find(buf, buf.cursor(), form == 'a', c) else {
                    return redrew(false);
                };
                return self.apply_range(op, span.from, span.to, buf);
            }
            _ => {}
        }

        // A count. `0` is a motion when no count has started and a digit when one has.
        //
        // **Before the operator check**, so the `2` in `d2w` is a count rather than a motion the
        // operator cannot understand.
        if c.is_ascii_digit() && !(c == '0' && self.count.is_none()) {
            let digit = c.to_digit(10).unwrap_or(0) as usize;
            self.count = Some(self.count.unwrap_or(0) * 10 + digit);
            return redrew(false);
        }

        if let Pending::Operator(op) = self.pending {
            self.pending = Pending::None;
            return self.operate(op, c, buf);
        }

        // A motion on its own only moves the cursor, so the inclusive flag does not apply.
        let count = self.count.unwrap_or(1).max(1);
        if let Some(aim) = self.motion(c, buf, buf.cursor(), count) {
            self.count = None;
            buf.set_cursor(aim.to);
            return redrew(true);
        }

        let count = self.take_count();
        match c {
            'i' => {
                self.mode = Mode::Insert;
                buf.snapshot();
                redrew(true)
            }
            'a' => {
                self.mode = Mode::Insert;
                buf.snapshot();
                buf.move_right();
                redrew(true)
            }
            'I' => {
                self.mode = Mode::Insert;
                buf.snapshot();
                buf.move_home();
                redrew(true)
            }
            'A' => {
                self.mode = Mode::Insert;
                buf.snapshot();
                buf.move_end();
                redrew(true)
            }
            'R' => {
                self.mode = Mode::Replace;
                buf.snapshot();
                redrew(true)
            }
            'd' | 'c' | 'y' => {
                self.pending = Pending::Operator(c);
                // Kept aside rather than left in `count`, so a count typed *after* the operator
                // starts from nothing and the two multiply — vi's rule for `2d3w`.
                self.operator_count = count;
                redrew(false)
            }
            'f' | 'F' | 't' | 'T' => {
                self.pending = Pending::Find(c);
                self.count = Some(count);
                redrew(false)
            }
            'r' => {
                self.pending = Pending::Replace;
                redrew(false)
            }
            'x' => {
                buf.snapshot();
                let to = buf.move_graphemes_from(buf.cursor(), count as isize);
                redrew(buf.cut(buf.cursor(), to))
            }
            'X' => {
                buf.snapshot();
                let from = buf.move_graphemes_from(buf.cursor(), -(count as isize));
                redrew(buf.cut(from, buf.cursor()))
            }
            'D' => {
                buf.snapshot();
                let end = buf.len();
                redrew(buf.cut(buf.cursor(), end))
            }
            'C' => {
                buf.snapshot();
                let end = buf.len();
                buf.cut(buf.cursor(), end);
                self.mode = Mode::Insert;
                redrew(true)
            }
            's' => {
                buf.snapshot();
                let to = buf.move_graphemes_from(buf.cursor(), count as isize);
                buf.cut(buf.cursor(), to);
                self.mode = Mode::Insert;
                redrew(true)
            }
            'S' => {
                buf.snapshot();
                buf.clear();
                self.mode = Mode::Insert;
                redrew(true)
            }
            // Paste after / before the cursor. One register, which is the kill buffer `C-y` uses,
            // so yanking in vi mode and pasting with `C-y` agree.
            'p' => {
                buf.snapshot();
                buf.move_right();
                let put = buf.yank();
                buf.move_left();
                redrew(put)
            }
            'P' => {
                buf.snapshot();
                redrew(buf.yank())
            }
            '~' => {
                buf.snapshot();
                let mut any = false;
                for _ in 0..count {
                    let Some(ch) = buf.at_cursor() else { break };
                    let flipped = if ch.is_uppercase() {
                        ch.to_lowercase().next().unwrap_or(ch)
                    } else {
                        ch.to_uppercase().next().unwrap_or(ch)
                    };
                    buf.replace_at_cursor(flipped);
                    buf.move_right();
                    any = true;
                }
                redrew(any)
            }
            'u' => redrew(buf.undo()),
            _ => redrew(false),
        }
    }

    /// An operator with its motion: `dw`, `c$`, `y2b`, and the doubled whole-line forms.
    fn operate(&mut self, op: char, motion: char, buf: &mut Buffer) -> Outcome {
        // A find is two more keys away, so the operator has to keep waiting.
        if matches!(motion, 'f' | 'F' | 't' | 'T') {
            self.pending = Pending::OperatorFind(op, motion);
            return Outcome::Handled { redraw: false };
        }
        // So is a text object: `ci` has said nothing yet about *what*. Checked here, beside the
        // find, because both are an operator that cannot act until another key arrives — and
        // before the count is taken, which neither of them has read yet.
        if matches!(motion, 'i' | 'a') {
            self.pending = Pending::OperatorObject(op, motion);
            return Outcome::Handled { redraw: false };
        }

        // The two counts multiply: `2d3w` is six words, as vi has it.
        let count = self.operator_count * self.take_count();
        self.operator_count = 1;
        let at = buf.cursor();

        // `dd`, `cc`, `yy` — the whole line, because a shell line has no other lines to take.
        if motion == op {
            let end = buf.len();
            return self.apply_range(op, 0, end, buf);
        }
        let Some(aim) = self.motion(motion, buf, at, count) else {
            return Outcome::Handled { redraw: false };
        };
        // `cw` is `ce`: vi's own special case, and the one everybody notices missing, because
        // `cw` on a word otherwise takes the space after it too.
        let aim = if op == 'c' && (motion == 'w' || motion == 'W') {
            Aim {
                to: word_end(buf, at, motion == 'W'),
                inclusive: false,
            }
        } else {
            aim
        };
        self.operate_over(op, at, aim, buf)
    }

    /// Turn a motion into a range and apply the operator over it.
    ///
    /// An inclusive motion takes the character it landed on, which is only ever true going
    /// forward: `dF x` already reaches back *to* the `x` by making it the start of the range.
    fn operate_over(&mut self, op: char, at: usize, aim: Aim, buf: &mut Buffer) -> Outcome {
        let (from, to) = if aim.to >= at {
            (
                at,
                if aim.inclusive {
                    buf.next_grapheme(aim.to)
                } else {
                    aim.to
                },
            )
        } else {
            (aim.to, at)
        };
        self.apply_range(op, from, to.min(buf.len()), buf)
    }

    fn apply_range(&mut self, op: char, from: usize, to: usize, buf: &mut Buffer) -> Outcome {
        buf.snapshot();
        let changed = match op {
            'd' => buf.cut(from, to),
            'c' => {
                buf.cut(from, to);
                self.mode = Mode::Insert;
                true
            }
            'y' => {
                let copied = buf.copy(from, to);
                buf.set_cursor(from);
                copied
            }
            _ => false,
        };
        Outcome::Handled { redraw: changed }
    }
}

/// Where a motion lands, and whether the character it lands on is part of the range.
#[derive(Debug, Clone, Copy)]
struct Aim {
    to: usize,
    inclusive: bool,
}

fn flip(op: char) -> char {
    match op {
        'f' => 'F',
        'F' => 'f',
        't' => 'T',
        'T' => 't',
        other => other,
    }
}

/// `f`/`F`/`t`/`T`: the `count`-th occurrence of `target`, `t` stopping one short.
fn find(buf: &Buffer, at: usize, op: char, target: char, count: usize) -> Option<usize> {
    let forward = op == 'f' || op == 't';
    let mut found = at;
    for _ in 0..count {
        found = if forward {
            (found + 1..buf.len()).find(|i| buf.char_at(*i) == Some(target))?
        } else {
            (0..found).rev().find(|i| buf.char_at(*i) == Some(target))?
        };
    }
    let found = super::display::clamp_char_boundary(&buf.text(), found);
    Some(match op {
        't' => buf.previous_grapheme(found),
        'T' => buf.next_grapheme(found),
        _ => found,
    })
}

/// Whether two characters belong to the same vi word class.
///
/// `big` is `W`/`B`/`E`, where a word is anything without whitespace — so `/usr/local/bin` is one
/// word. The small forms split on punctuation as well, which is why `w` walks a path a component
/// at a time.
fn same_class(a: char, b: char, big: bool) -> bool {
    if big {
        return !a.is_whitespace() && !b.is_whitespace();
    }
    let class = |c: char| {
        if c.is_whitespace() {
            0
        } else if c.is_alphanumeric() || c == '_' {
            1
        } else {
            2
        }
    };
    class(a) == class(b) && class(a) != 0
}

fn next_word(buf: &Buffer, from: usize, big: bool) -> usize {
    let len = buf.len();
    let mut at = from;
    if let Some(here) = buf.char_at(at) {
        while at < len && buf.char_at(at).is_some_and(|c| same_class(here, c, big)) {
            at += 1;
        }
    }
    while at < len && buf.char_at(at).is_some_and(char::is_whitespace) {
        at += 1;
    }
    at
}

fn prev_word(buf: &Buffer, from: usize, big: bool) -> usize {
    let mut at = from;
    while at > 0 && buf.char_at(at - 1).is_some_and(char::is_whitespace) {
        at -= 1;
    }
    let Some(here) = (at > 0).then(|| buf.char_at(at - 1)).flatten() else {
        return at;
    };
    while at > 0
        && buf
            .char_at(at - 1)
            .is_some_and(|c| same_class(here, c, big))
    {
        at -= 1;
    }
    at
}

/// Where `e` lands: the last character of the word ahead, not the one after it.
fn word_end(buf: &Buffer, from: usize, big: bool) -> usize {
    let len = buf.len();
    let mut at = from + 1;
    while at < len && buf.char_at(at).is_some_and(char::is_whitespace) {
        at += 1;
    }
    let Some(here) = buf.char_at(at) else {
        return len;
    };
    while at + 1 < len
        && buf
            .char_at(at + 1)
            .is_some_and(|c| same_class(here, c, big))
    {
        at += 1;
    }
    (at + 1).min(len)
}

#[cfg(test)]
#[path = "vi/tests.rs"]
mod tests;
