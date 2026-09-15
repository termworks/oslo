//! Keys that take something from history without walking it — `alt-.`, `alt-<` and `alt->` — and
//! `alt-#`, which puts the line *into* history without running anything.
//!
//! bash's `yank-last-arg`, `beginning-of-history`, `end-of-history` and `insert-comment`, on the
//! keys readline gives them, because they are the ones in an old hand's fingers.

use super::{Assist, Session, Step};

/// What the last `alt-.` put on the line, so a press straight after can take it back and look one
/// command further. Any other key drops it: see `Session::apply`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LastArg {
    /// How many commands back the word came from; `1` is the previous one.
    back: usize,
    /// Where it was inserted, in characters, and how many it is.
    start: usize,
    len: usize,
}

impl Session {
    /// `yank-last-arg`: the previous command's last word at the cursor. Pressed again at once, the
    /// word is replaced by the last word of the command before that, and so on back.
    pub(super) fn yank_last_arg(
        &mut self,
        chain: Option<LastArg>,
        assist: &mut dyn Assist,
    ) -> bool {
        let mut back = chain.map_or(1, |prev| prev.back + 1);
        let word = loop {
            let Some(line) = assist.history_line(back) else {
                // Out of history: what the last press put in stays, and so does the chain.
                self.last_arg = chain;
                return false;
            };
            let word = last_word(&line);
            if !word.is_empty() {
                break word;
            }
            back += 1;
        };
        let mut chars: Vec<char> = self.buffer.text().chars().collect();
        let start = match chain {
            Some(prev) => {
                chars.drain(prev.start..prev.start + prev.len);
                prev.start
            }
            None => self.buffer.cursor(),
        };
        let len = word.chars().count();
        chars.splice(start..start, word.chars());
        self.buffer
            .set(&chars.into_iter().collect::<String>(), start + len);
        self.last_arg = Some(LastArg { back, start, len });
        true
    }

    /// `beginning-of-history`: the oldest entry, with the walk moved there.
    pub(super) fn recall_oldest(&mut self, assist: &mut dyn Assist) -> bool {
        let text = self.buffer.text();
        self.put(assist.history_oldest(&text))
    }

    /// `end-of-history`: the line being composed, back from wherever the walk had got to.
    pub(super) fn recall_newest(&mut self, assist: &mut dyn Assist) -> bool {
        self.put(assist.history_newest())
    }

    fn put(&mut self, line: Option<String>) -> bool {
        let Some(line) = line else {
            return false;
        };
        let end = line.chars().count();
        self.buffer.set(&line, end);
        true
    }

    /// `insert-comment`: a `#` in front and the line run — kept in history, and it does nothing.
    pub(super) fn insert_comment(&mut self) -> Step {
        let text = self.buffer.text();
        if text.is_empty() {
            return Step::Continue { redraw: false };
        }
        let commented = format!("#{text}");
        let end = commented.chars().count();
        self.buffer.set(&commented, end);
        Step::Accept { erase: false }
    }
}

/// The last word of a history line as it was typed — quotes and all, which is what bash inserts.
fn last_word(line: &str) -> String {
    let line = line.trim_end();
    let word = crate::words::current_word(line, line.len());
    format!("{}{}", word.prefix, word.text)
}

#[cfg(test)]
mod tests {
    use super::super::{Assist, Bound, Key, Session, Step};

    /// A history, newest first, walked the way oslo's is, with the alt+symbol keys bound as oslo
    /// binds them by default.
    #[derive(Default)]
    struct History {
        lines: Vec<String>,
        at: usize,
        typed: Option<String>,
    }

    impl Assist for History {
        fn history_next(&mut self) -> Option<String> {
            match self.at {
                0 => None,
                1 => {
                    self.at = 0;
                    self.typed.take()
                }
                _ => {
                    self.at -= 1;
                    self.lines.get(self.at - 1).cloned()
                }
            }
        }
        fn history_line(&mut self, back: usize) -> Option<String> {
            self.lines.get(back.checked_sub(1)?).cloned()
        }
        fn history_oldest(&mut self, line: &str) -> Option<String> {
            let oldest = self.lines.last()?.clone();
            if self.at == 0 {
                self.typed = Some(line.to_string());
            }
            self.at = self.lines.len();
            Some(oldest)
        }
        fn history_newest(&mut self) -> Option<String> {
            if self.at == 0 {
                return None;
            }
            self.at = 0;
            self.typed.take()
        }
        fn binding(&mut self, key: Key) -> Option<Bound> {
            match key {
                Key::Alt('.') | Key::Alt('_') => Some(Bound::YankLastArg),
                Key::Alt('<') => Some(Bound::HistoryFirst),
                Key::Alt('>') => Some(Bound::HistoryLast),
                Key::Alt('#') => Some(Bound::InsertComment),
                _ => None,
            }
        }
    }

    fn history(lines: &[&str]) -> History {
        History {
            lines: lines.iter().map(|line| line.to_string()).collect(),
            ..History::default()
        }
    }

    fn at(text: &str) -> Session {
        Session {
            vi: None,
            ..Session::new(text, text.chars().count())
        }
    }

    /// **`alt-.` is bash's `yank-last-arg`**: the previous command's last word at the cursor, and
    /// each press straight after replaces it with the last word of the command before.
    #[test]
    fn alt_dot_inserts_the_last_argument_and_walks_back() {
        let mut a = history(&["vim src/main.rs", "ls", "cp a 'b c'"]);
        let mut s = at("git add ");
        s.apply(Key::Alt('.'), &mut a);
        assert_eq!(s.buffer.text(), "git add src/main.rs");
        s.apply(Key::Alt('.'), &mut a);
        assert_eq!(
            s.buffer.text(),
            "git add ls",
            "the second press takes the one before"
        );
        s.apply(Key::Alt('_'), &mut a);
        assert_eq!(
            s.buffer.text(),
            "git add 'b c'",
            "alt-_ is the same key, quotes as typed"
        );
        assert_eq!(
            s.apply(Key::Alt('.'), &mut a),
            Step::Continue { redraw: false },
            "out of history"
        );
        assert_eq!(s.buffer.text(), "git add 'b c'", "and the last word stays");

        // Anything else in between starts again from the previous command.
        s.apply(Key::Char(' '), &mut a);
        s.apply(Key::Alt('.'), &mut a);
        assert_eq!(s.buffer.text(), "git add 'b c' src/main.rs");
    }

    /// `alt-<` goes to the oldest entry and `alt->` back to what was being typed; Down carries on
    /// from wherever `alt-<` left the walk.
    #[test]
    fn alt_angle_brackets_jump_to_the_ends_of_history() {
        let mut a = history(&["newest", "middle", "oldest"]);
        let mut s = at("draft");
        s.apply(Key::Alt('<'), &mut a);
        assert_eq!(s.buffer.text(), "oldest");
        s.apply(Key::Down, &mut a);
        assert_eq!(s.buffer.text(), "middle", "Down goes on from the oldest");
        s.apply(Key::Alt('>'), &mut a);
        assert_eq!(
            s.buffer.text(),
            "draft",
            "and alt-> brings the typed line back"
        );
    }

    /// `alt-#` is bash's `insert-comment`: the line commented out and run — in history, and inert.
    #[test]
    fn alt_hash_comments_the_line_out_and_runs_it() {
        let mut a = History::default();
        let mut s = at("rm -rf build");
        assert_eq!(
            s.apply(Key::Alt('#'), &mut a),
            Step::Accept { erase: false }
        );
        assert_eq!(s.buffer.text(), "#rm -rf build");
        assert_eq!(
            at("").apply(Key::Alt('#'), &mut a),
            Step::Continue { redraw: false }
        );
    }
}
