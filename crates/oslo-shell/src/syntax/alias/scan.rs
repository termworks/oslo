//! The character-level half of alias substitution: where a word ends, what a name may be, and
//! how to copy a construct through untouched.
//!
//! Split from the scanner above it because these answer questions about *text* — quoting, word
//! boundaries, balanced brackets — while the scanner answers a question about *grammar*: whether
//! the word it is looking at begins a command.

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Quote {
    None,
    Single,
    Double,
}

/// Where the word starting at `start` ends: at a blank or an unquoted shell metacharacter.
pub(super) fn word_end(chars: &[char], start: usize) -> usize {
    let mut i = start;
    let mut quote = Quote::None;
    while i < chars.len() {
        let c = chars[i];
        match quote {
            Quote::Single => {
                if c == '\'' {
                    quote = Quote::None;
                }
            }
            Quote::Double => {
                if c == '"' {
                    quote = Quote::None;
                }
            }
            Quote::None => match c {
                '\'' => quote = Quote::Single,
                '"' => quote = Quote::Double,
                '\\' => i += 1,
                // A word *ends* at an expansion, so that the scanner's own `$` handling copies
                // it through — that copy is the one that can span lines. Splitting the word at
                // `(` instead, as this did first, left `x=$` and a bare `(`, and the `ll` inside
                // was then read as a command and substituted twice over.
                //
                // The caller must not substitute a word that stopped here: `ll${x}` is not the
                // alias `ll`, however much its first two characters look like one.
                '$' if matches!(chars.get(i + 1), Some('(') | Some('{')) => return i,
                // `#` is deliberately absent: it only begins a comment at the *start* of a word,
                // so inside one it is an ordinary character. Ending the word at it split `$#`
                // into `$` and a comment, and the scanner then copied the rest of the line
                // verbatim — so every alias after a `$#` on that line went unsubstituted.
                // Leaving it in the word is also what makes `a#b` one word, as it is in bash.
                ' ' | '\t' | ';' | '&' | '|' | '(' | ')' | '<' | '>' | '`' => return i,
                _ => {}
            },
        }
        i += 1;
    }
    chars.len()
}

/// Whether `text` ends inside a single- or double-quoted string.
pub(super) fn leaves_quote_open(text: &str) -> bool {
    let mut quote = Quote::None;
    let mut word_start = true;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Quote::Single, '\'') | (Quote::Double, '"') => quote = Quote::None,
            (Quote::Double | Quote::None, '\\') => {
                chars.next();
            }
            (Quote::None, '\'') => quote = Quote::Single,
            (Quote::None, '"') => quote = Quote::Double,
            (Quote::None, '#') if word_start => break,
            _ => {}
        }
        word_start = quote == Quote::None && matches!(c, ' ' | '\t' | '\n' | ';');
    }
    quote != Quote::None
}

/// Whether the next non-blank characters are `()`, which makes the word a function's name.
pub(super) fn is_function_definition(chars: &[char], from: usize) -> bool {
    let mut i = from;
    let skip_blanks = |i: &mut usize| {
        while *i < chars.len() && matches!(chars[*i], ' ' | '\t') {
            *i += 1;
        }
    };
    skip_blanks(&mut i);
    if chars.get(i) != Some(&'(') {
        return false;
    }
    // The parens must be *empty*. Requiring only the `(` read `not (cmd)` as a definition of a
    // function called `not`, so modernish's `alias not='! '` was never substituted and
    // `not (readonly foo; …)` stayed a syntax error — which is what the raw text is until the
    // alias expands.
    i += 1;
    skip_blanks(&mut i);
    chars.get(i) == Some(&')')
}

/// Whether a word can be an alias name at all.
///
/// POSIX allows more than this, but a name containing a quote, an expansion or a metacharacter
/// cannot be looked up without expanding it first — and expanding it is not this pass's job.
/// A reserved word such as `if` can be one: bash expands aliases before it looks for those.
pub(super) fn is_plain_name(word: &str) -> bool {
    !word.is_empty()
        && !word.contains(['\'', '"', '$', '`', '\\', '=', '/'])
        && word != "!"
        && (word.chars().all(|c| c.is_ascii_alphabetic()) || !super::INTRODUCERS.contains(&word))
}

/// `name=value`, the shape of a command prefix.
pub(super) fn is_assignment(word: &str) -> bool {
    match word.split_once('=') {
        Some((name, _)) => {
            !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '[' || c == ']')
        }
        None => false,
    }
}

/// Split a line into words, keeping quotes attached, for reading `alias` operands.
pub(super) fn split_words(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut words = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            ' ' | '\t' => i += 1,
            '#' => break,
            ';' | '&' | '|' => {
                let start = i;
                while i < chars.len() && chars[i] == chars[start] {
                    i += 1;
                }
                words.push(chars[start..i].iter().collect());
            }
            _ => {
                let end = word_end(&chars, i);
                if end == i {
                    i += 1;
                    continue;
                }
                words.push(chars[i..end].iter().collect());
                i = end;
            }
        }
    }
    words
}

/// Strip one layer of surrounding quotes from an alias body, as the `alias` builtin would.
pub(super) fn unquote(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    let mut quote = Quote::None;
    while let Some(c) = chars.next() {
        match quote {
            Quote::Single => {
                if c == '\'' {
                    quote = Quote::None;
                } else {
                    out.push(c);
                }
            }
            Quote::Double => match c {
                '"' => quote = Quote::None,
                '\\' => {
                    if let Some(next) = chars.next() {
                        out.push(next);
                    }
                }
                _ => out.push(c),
            },
            Quote::None => match c {
                '\'' => quote = Quote::Single,
                '"' => quote = Quote::Double,
                '\\' => {
                    if let Some(next) = chars.next() {
                        out.push(next);
                    }
                }
                _ => out.push(c),
            },
        }
    }
    out
}

/// A balanced `open`/`close` run being copied through, which may span lines.
///
/// Resumable because a command substitution is routinely written across several lines, and the
/// scanner reads a line at a time. A copy that stopped at the end of the line left the rest of the
/// substitution's body to be scanned as ordinary text — and since that body is *also* parsed when
/// the substitution runs, every alias in it was substituted twice.
pub(super) struct Balance {
    open: char,
    close: char,
    depth: usize,
    quote: Quote,
    escaped: bool,
    /// Whether a `#` here would begin a comment, which is true at a word boundary.
    at_word_start: bool,
}

impl Balance {
    pub(super) fn new(open: char, close: char) -> Self {
        Self {
            open,
            close,
            depth: 0,
            quote: Quote::None,
            escaped: false,
            at_word_start: true,
        }
    }

    /// Tell the balance that a new line is beginning, so a `#` on it can start a comment.
    pub(super) fn start_line(&mut self) {
        self.at_word_start = true;
    }

    /// Copy from `from` until the run closes, appending to `out`.
    ///
    /// `Some(i)` is the index just past the closing character; `None` means the line ended first
    /// and this `Balance` must be kept and resumed on the next one.
    pub(super) fn consume(
        &mut self,
        out: &mut String,
        chars: &[char],
        from: usize,
    ) -> Option<usize> {
        let mut i = from;
        while i < chars.len() {
            let c = chars[i];
            // A comment runs to the end of the line, and nothing in it is shell. Without this a
            // lone apostrophe in one — `# many shells don't check for no arguments here` — opened
            // a quote that swallowed the rest of the construct, so the `)` that closed it was
            // never seen and everything after it went unsubstituted. modernish's `builtin.t` has
            // exactly that comment inside a `$( … )`.
            if self.quote == Quote::None && !self.escaped && c == '#' && self.at_word_start {
                out.extend(&chars[i..]);
                return None;
            }
            out.push(c);
            i += 1;
            let was_escaped = std::mem::take(&mut self.escaped);
            self.at_word_start = matches!(c, ' ' | '\t' | ';' | '&' | '|' | '(' | ')');
            if was_escaped {
                continue;
            }
            match self.quote {
                Quote::Single => {
                    if c == '\'' {
                        self.quote = Quote::None;
                    }
                    continue;
                }
                Quote::Double => {
                    match c {
                        '"' => self.quote = Quote::None,
                        // Bounded by the loop rather than by skipping ahead: a backslash as the
                        // last character of a line must not push the index past the end, which
                        // panicked the shell when the caller sliced with it.
                        '\\' => self.escaped = true,
                        _ => {}
                    }
                    continue;
                }
                Quote::None => {}
            }
            match c {
                '\'' => self.quote = Quote::Single,
                '"' => self.quote = Quote::Double,
                '\\' => self.escaped = true,
                c if c == self.open => self.depth += 1,
                c if c == self.close => {
                    self.depth -= 1;
                    if self.depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        None
    }
}
