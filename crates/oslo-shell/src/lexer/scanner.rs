//! The lexer core: cursor, token dispatch and operator scanning.
//!
//! Word scanning lives in [`super::quoting`] and `$`-expansions in [`super::expansion`];
//! all three write `impl Lexer` blocks against the struct defined here.

pub use super::tilde::TildeMode;
use crate::lexer::token::Token;
use oslo_base::error::{Result, ShellError};

/// How deeply `$( )`, `${ }`, `"…"` and backticks may nest inside a single word.
///
/// The raw scanners below recurse once per nesting level, so input like `$($($(…` would abort the
/// process on a stack overflow before any diagnostic could be printed — the same failure mode
/// [`crate::nesting`] exists to prevent. Real words nest a handful of levels at most.
const MAX_WORD_NESTING: usize = 64;

#[allow(clippy::should_implement_trait, clippy::collapsible_if)]
pub struct Lexer<'a> {
    _input: &'a str,
    chars: Vec<char>,

    pos: usize,
    peeked: Option<Token>,
    /// Where the words this lexer scans may open a tilde prefix.
    pub(super) tilde: TildeMode,
    /// Whether the word being scanned is past its assignment `=`, so a `:` opens a tilde.
    pub(super) colon_tilde: bool,
}

#[allow(clippy::should_implement_trait, clippy::collapsible_if)]
impl<'a> Lexer<'a> {
    pub fn new(input: &'a str) -> Self {
        Self::with_tilde(input, TildeMode::Word)
    }

    /// A lexer whose words follow the tilde rule of `mode`.
    pub fn with_tilde(input: &'a str, mode: TildeMode) -> Self {
        Self {
            _input: input,
            chars: input.chars().collect(),

            pos: 0,
            peeked: None,
            tilde: mode,
            colon_tilde: false,
        }
    }

    pub(super) fn is_eof(&self) -> bool {
        self.pos >= self.chars.len()
    }

    pub(super) fn current_char(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    pub(super) fn peek_char(&self) -> Option<char> {
        self.chars.get(self.pos + 1).copied()
    }

    /// The character `ahead` places past the cursor.
    pub(super) fn char_ahead(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.pos + ahead).copied()
    }

    /// Step over any `\` + newline pairs at the cursor: a line continuation is nothing at all.
    pub(super) fn skip_line_continuations(&mut self) {
        while self.current_char() == Some('\\') && self.peek_char() == Some('\n') {
            self.advance();
            self.advance();
        }
    }

    /// How far the cursor has moved through the input, in characters.
    ///
    /// Exposed so a caller that loops on [`Lexer::next`] can tell a token apart from no progress
    /// at all. Note that [`Lexer::peek`] moves it too — a peeked token is already scanned — so
    /// only a loop that consumes with `next` alone may read this as "bytes consumed".
    pub fn offset(&self) -> usize {
        self.pos
    }

    pub(super) fn advance(&mut self) -> Option<char> {
        if self.pos < self.chars.len() {
            let ch = self.chars[self.pos];
            self.pos += 1;
            Some(ch)
        } else {
            None
        }
    }

    pub fn peek(&mut self) -> Result<&Token> {
        if self.peeked.is_none() {
            let tok = self.next_token()?;
            self.peeked = Some(tok);
        }
        Ok(self.peeked.as_ref().unwrap())
    }

    pub fn next(&mut self) -> Result<Token> {
        if let Some(tok) = self.peeked.take() {
            Ok(tok)
        } else {
            self.next_token()
        }
    }

    fn skip_whitespace(&mut self) {
        while let Some(ch) = self.current_char() {
            if is_blank(ch) {
                self.advance();
            } else if ch == '\\' && self.peek_char() == Some('\n') {
                // Line continuation
                self.advance();
                self.advance();
            } else {
                break;
            }
        }
    }

    /// Turn a stalled scan into an error instead of an endless stream of empty tokens.
    ///
    /// [`Lexer::next`] must consume at least one character per token, or any caller looping to
    /// `Eof` never reaches it. `next_token` reaches [`Lexer::scan_word`] only after
    /// `skip_whitespace` and `scan_operator` have both declined a character that is not EOF, so
    /// word scanning consuming nothing means those three disagree about what that character is.
    /// That was a real bug once — see [`is_blank`] — and this makes the next one loud.
    pub(super) fn stalled_at(&self) -> ShellError {
        let ch = self.current_char().unwrap_or('\0');
        ShellError::SyntaxError(format!(
            "lexer made no progress at U+{:04X}: it is neither a separator, an operator nor part \
             of a word",
            ch as u32
        ))
    }

    fn next_token(&mut self) -> Result<Token> {
        self.skip_whitespace();

        if self.is_eof() {
            return Ok(Token::Eof);
        }

        let ch = self.current_char().unwrap();

        // Newline
        if ch == '\n' {
            self.advance();
            return Ok(Token::Newline);
        }

        // Comment `#`
        if ch == '#' {
            while let Some(c) = self.current_char() {
                if c == '\n' {
                    break;
                }
                self.advance();
            }
            return self.next_token();
        }

        // Operators
        if let Some(tok) = self.scan_operator()? {
            return Ok(tok);
        }

        // Word or IoNumber
        self.scan_word()
    }

    fn scan_operator(&mut self) -> Result<Option<Token>> {
        let ch = match self.current_char() {
            Some(c) => c,
            None => return Ok(None),
        };

        let next = self.peek_char();

        let token = match (ch, next) {
            ('|', Some('|')) => {
                self.advance();
                self.advance();
                Token::OrIf
            }
            ('|', _) => {
                self.advance();
                Token::Pipe
            }
            ('&', Some('&')) => {
                self.advance();
                self.advance();
                Token::AndIf
            }
            ('&', _) => {
                self.advance();
                Token::Amp
            }
            (';', Some(';')) => {
                self.advance();
                self.advance();
                Token::Dsemi
            }
            (';', _) => {
                self.advance();
                Token::Semicolon
            }
            ('<', Some('<')) => {
                self.advance();
                self.advance();
                if self.current_char() == Some('-') {
                    self.advance();
                    Token::DlessDash
                } else {
                    Token::Dless
                }
            }
            ('<', Some('&')) => {
                self.advance();
                self.advance();
                Token::LessAnd
            }
            ('<', Some('>')) => {
                self.advance();
                self.advance();
                Token::LessGreat
            }
            ('<', _) => {
                self.advance();
                Token::Less
            }
            ('>', Some('>')) => {
                self.advance();
                self.advance();
                Token::Dgreat
            }
            ('>', Some('&')) => {
                self.advance();
                self.advance();
                Token::GreatAnd
            }
            ('>', Some('|')) => {
                self.advance();
                self.advance();
                Token::Clobber
            }
            ('>', _) => {
                self.advance();
                Token::Great
            }
            ('(', _) => {
                self.advance();
                Token::LParen
            }
            (')', _) => {
                self.advance();
                Token::RParen
            }
            // `{` and `}` are deliberately absent: they are not operator characters in the shell
            // grammar, they are reserved *words*, recognised only when they stand alone. Scanning
            // them as operators cut `{a,b}` off from the rest of its word, and since the caller
            // (word re-lexing) then falls back to treating the whole text as one literal, it also
            // silently disabled every expansion in a word that happened to start with a brace.
            _ => return Ok(None),
        };

        Ok(Some(token))
    }
}

/// How far through a `case` a raw scan is, so a pattern's `)` can be told from a real closer.
#[derive(PartialEq, Eq)]
enum CaseAt {
    /// Seen `case`, waiting for `in`.
    AwaitingIn,
    /// A pattern may start here; the next `)` ends it.
    Pattern,
    /// Inside an arm's commands, until `;;`.
    Body,
}

/// Raw, quote-aware scanning of bracketed constructs.
///
/// These copy source text verbatim rather than decomposing it into [`oslo_base::ast::WordPart`]s: a
/// `$( )` body is re-parsed as a whole script and a `${ }` body is picked apart by the
/// parameter-expansion parser, and both need the quotes the user actually wrote. What the quote
/// tracking buys is knowing which delimiters *count* — the `)` in `echo "$(echo "a)b")"` is data,
/// not the end of the substitution.
impl Lexer<'_> {
    /// Copy up to the delimiter matching an `open` that was just consumed, excluding it.
    pub(super) fn scan_raw_delimited(
        &mut self,
        open: char,
        close: char,
        what: &str,
        depth: usize,
    ) -> Result<String> {
        if depth > MAX_WORD_NESTING {
            return Err(ShellError::SyntaxError(format!("{what} nested too deeply")));
        }

        let mut out = String::new();
        let mut level = 1usize;
        // How far through each open `case` this scan is, innermost last. Only meaningful for a
        // command substitution: a case pattern ends in `)`, which is otherwise indistinguishable
        // from the `)` closing the substitution, so `$(case a in a) echo Y;; esac)` stopped at
        // `$(case a in a` and the rest came back as literal text. A mistake other shells' scanners
        // of its own scanners; this is oslo's copy of it.
        let mut cases: Vec<CaseAt> = Vec::new();
        let counting_cases = open == '(';
        let mut word = String::new();
        // Whether a `#` here would start a comment, which it does only at a word boundary — a
        // `#` inside a word is an ordinary character (`ab#cd`, `${x#pat}`).
        let mut at_word_start = true;

        while let Some(ch) = self.current_char() {
            // **A comment runs to the end of the line and nothing in it is shell.** Without this,
            // an apostrophe in one — `# Don't complain about nonexistent directories` — opened a
            // quote that ran to the end of the file, so the `)` closing the substitution was
            // never seen. `/usr/bin/xdg-terminal-exec` carries exactly that comment inside a
            // `$( … )` inside a heredoc body, which is the path that reaches this scanner.
            //
            // The third scanner to need this rule, after `parser::alias::scan` and the tokenizer.
            //
            // **Only inside `$( … )`.** A command substitution holds a command list, which can
            // have comments; `${ … }` holds a parameter expansion, which cannot — and its very
            // first character is `#` in `${#name}`, the length operator. Applying the rule there
            // ate the whole expansion, which is the same "a hash inside a word is not a comment"
            // mistake this rule was written to avoid.
            if counting_cases && ch == '#' && at_word_start {
                while let Some(c) = self.current_char() {
                    if c == '\n' {
                        break;
                    }
                    out.push(c);
                    self.advance();
                }
                continue;
            }
            at_word_start = matches!(ch, ' ' | '\t' | '\n' | ';' | '&' | '|' | '(' | ')');

            if counting_cases {
                // A word ends here, so decide what it was before acting on `ch`.
                if !(ch.is_ascii_alphanumeric() || ch == '_') {
                    match word.as_str() {
                        "case" => cases.push(CaseAt::AwaitingIn),
                        "in" if cases.last() == Some(&CaseAt::AwaitingIn) => {
                            *cases.last_mut().unwrap() = CaseAt::Pattern;
                        }
                        "esac" => {
                            cases.pop();
                        }
                        _ => {}
                    }
                    word.clear();
                } else {
                    word.push(ch);
                }
                // `;;` ends an arm and a new pattern may follow.
                if ch == ';' && self.peek_char() == Some(';') && cases.last() == Some(&CaseAt::Body)
                {
                    *cases.last_mut().unwrap() = CaseAt::Pattern;
                }
                if cases.last() == Some(&CaseAt::Pattern) && (ch == close || ch == open) {
                    // Part of the pattern, not of this scan's brackets.
                    if ch == close {
                        *cases.last_mut().unwrap() = CaseAt::Body;
                    }
                    self.advance();
                    out.push(ch);
                    continue;
                }
            }

            // A nested expansion is followed into whatever brackets *this* scan is counting, so
            // the `}` in `${x:-$(echo })}` belongs to the substitution and not to the operand.
            if ch == '$' && self.copy_dollar_group(&mut out, depth)? {
                continue;
            }
            if ch == close {
                self.advance();
                level -= 1;
                if level == 0 {
                    return Ok(out);
                }
                out.push(ch);
                continue;
            }
            if ch == open {
                self.advance();
                level += 1;
                out.push(ch);
                continue;
            }

            match ch {
                '\'' => {
                    self.advance();
                    out.push('\'');
                    self.copy_single_quoted(&mut out)?;
                }
                '"' => {
                    self.advance();
                    out.push('"');
                    self.copy_double_quoted(&mut out, depth + 1)?;
                }
                '`' => {
                    self.advance();
                    out.push('`');
                    self.copy_backquoted(&mut out)?;
                }
                '\\' => {
                    self.advance();
                    out.push('\\');
                    if let Some(next) = self.advance() {
                        out.push(next);
                    }
                }
                _ => {
                    out.push(ch);
                    self.advance();
                }
            }
        }

        Err(ShellError::SyntaxError(format!("unterminated {what}")))
    }

    /// Copy a `$( … )` or `${ … }` starting at the cursor, brackets included.
    ///
    /// Returns `false` — having consumed nothing — when the `$` starts something else, so callers
    /// can fall through to their ordinary handling of it.
    fn copy_dollar_group(&mut self, out: &mut String, depth: usize) -> Result<bool> {
        let (open, close, what) = match self.peek_char() {
            Some('(') => ('(', ')', "command substitution"),
            Some('{') => ('{', '}', "parameter expansion"),
            _ => return Ok(false),
        };

        self.advance();
        self.advance();
        out.push('$');
        out.push(open);
        let inner = self.scan_raw_delimited(open, close, what, depth + 1)?;
        out.push_str(&inner);
        out.push(close);
        Ok(true)
    }

    /// Copy through the closing `'`. Nothing inside is special, not even a backslash.
    fn copy_single_quoted(&mut self, out: &mut String) -> Result<()> {
        while let Some(ch) = self.advance() {
            out.push(ch);
            if ch == '\'' {
                return Ok(());
            }
        }
        Err(ShellError::SyntaxError(
            "Unterminated single quote".to_string(),
        ))
    }

    /// Copy through the closing `"`.
    ///
    /// Expansions stay live inside double quotes, so a `$( )` or `${ }` opened here has to be
    /// followed into: `"$(echo ")")"` closes on the *third* quote, not the second.
    fn copy_double_quoted(&mut self, out: &mut String, depth: usize) -> Result<()> {
        while let Some(ch) = self.current_char() {
            match ch {
                '"' => {
                    self.advance();
                    out.push('"');
                    return Ok(());
                }
                '\\' => {
                    self.advance();
                    out.push('\\');
                    if let Some(next) = self.advance() {
                        out.push(next);
                    }
                }
                '`' => {
                    self.advance();
                    out.push('`');
                    self.copy_backquoted(out)?;
                }
                '$' if self.copy_dollar_group(out, depth)? => {}
                _ => {
                    out.push(ch);
                    self.advance();
                }
            }
        }

        Err(ShellError::SyntaxError(
            "Unterminated double quote".to_string(),
        ))
    }

    /// Copy through the closing backquote. Only `` \` ``, `\\` and `\$` are escapes here.
    fn copy_backquoted(&mut self, out: &mut String) -> Result<()> {
        while let Some(ch) = self.current_char() {
            self.advance();
            out.push(ch);
            if ch == '`' {
                return Ok(());
            }
            if ch == '\\'
                && matches!(self.current_char(), Some('`') | Some('\\') | Some('$'))
                && let Some(next) = self.advance()
            {
                out.push(next);
            }
        }
        Err(ShellError::SyntaxError(
            "Unterminated backquote".to_string(),
        ))
    }
}

/// The blanks that separate one token from the next — the *only* ones.
///
/// This is the shell's definition, not Unicode's: POSIX separates tokens on the characters in
/// `IFS`, whose default is space, tab and newline, and bash tokenizes on exactly space and tab
/// (newline is an operator, handled by [`is_operator_char`], and `\r` is accepted here so a file
/// with CRLF line endings still lexes). Every other character `char::is_whitespace` reports —
/// U+000B vertical tab, U+000C form feed, U+00A0 no-break space, U+2028, the rest of Unicode
/// `White_Space` — is an ordinary word character, and `echo a<NBSP>b` prints one word in bash.
///
/// It is a free function, and both the token scanner and the word scanner call it, because they
/// used to disagree: [`Lexer::skip_whitespace`] stepped over this set while
/// [`Lexer::scan_word_parts`] ended a word at any `char::is_whitespace()`. A character in the gap
/// was skipped by neither and consumed by neither, so `scan_word` returned a part-less word
/// without moving the cursor and every caller looping to `Eof` spun forever, allocating — a hang
/// and an out-of-memory reachable by pasting a no-break space into any script. Any future change
/// to what counts as a separator has to happen here, where it cannot apply to only one of them.
pub(super) fn is_blank(ch: char) -> bool {
    ch == ' ' || ch == '\t' || ch == '\r'
}

pub(super) fn is_operator_char(ch: char) -> bool {
    matches!(ch, '|' | '&' | ';' | '<' | '>' | '(' | ')' | '\n')
}

pub(super) fn is_var_name_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// The one-character parameters that follow `$` without braces.
///
/// `-` is one of them: `$-` is the set of shell options, not a literal dash. It used to fall
/// through to the "not an expansion" arm, so `case "$-" in *e*)` compared the two characters
/// `$-` against the pattern.
pub(super) fn is_special_param(ch: char) -> bool {
    matches!(ch, '?' | '$' | '!' | '#' | '*' | '@' | '-' | '0'..='9')
}

#[cfg(test)]
mod case_substitution_tests {
    use crate::lexer::Lexer;

    fn word_of(src: &str) -> String {
        let mut lexer = Lexer::new(src);
        match lexer.next() {
            Ok(crate::lexer::Token::Word(w)) => format!("{w:?}"),
            other => format!("{other:?}"),
        }
    }

    /// A case pattern ends in `)`, which the raw scan used to read as the `)` closing the
    /// substitution — so `$(case a in a) …)` stopped at `$(case a in a` and the rest of the
    /// command came back as literal text. Other shells have had the same bug in their scanners.
    #[test]
    fn a_case_pattern_does_not_close_a_command_substitution() {
        // The whole construct has to survive as one word, both spellings of the pattern.
        for src in [
            "$(case a in a) echo Y;; esac)",
            "$(case a in (a) echo P;; esac)",
            "$(case a in a) case b in b) echo IN;; esac;; esac)",
            "$(case b in a) echo N;; *) echo D;; esac)",
        ] {
            let word = word_of(src);
            assert!(word.contains("esac"), "{src} lost its tail: {word}");
        }
    }

    /// And the things that must keep working: a real closer still closes.
    #[test]
    fn ordinary_substitutions_are_unaffected() {
        for src in [
            "$(echo hi)",
            "$(echo $(echo deep))",
            "$(echo \"a)b\")",
            "${v:-$(echo d)}",
        ] {
            let word = word_of(src);
            assert!(!word.starts_with("Err"), "{src} failed to lex: {word}");
        }
    }
}
