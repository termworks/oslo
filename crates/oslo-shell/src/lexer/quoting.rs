//! Word scanning and quote handling.
//!
//! A shell word is a sequence of parts — literal text, single-quoted runs, double-quoted runs
//! (which may themselves contain expansions) — which the evaluator later expands in order.

use super::scanner::{Lexer, is_blank, is_operator_char};
use crate::lexer::token::Token;
use oslo_base::ast::{Word, WordPart};
use oslo_base::error::{Result, ShellError};

/// Re-lex an operand that is already known to be exactly one word.
///
/// `${x:-$HOME}` stores its default as source text, and the expander needs it as parts. The
/// difference from [`Lexer::scan_word`] is that nothing here terminates the word: a space in
/// `${x:-a b}` is part of the operand, not a separator between two of them.
pub(super) fn parse_word_source(source: &str) -> Result<Word> {
    parse_operand(source, false)
}

/// [`parse_word_source`], told whether the `${…}` it came from was written inside double quotes.
///
/// Only the payload of `${x-word}` and its relatives cares. There, the enclosing quotes govern —
/// `"${x-'q'}"` is `'q'` and `"${x:-~}"` is a tilde — because the payload is part of the quoted
/// word rather than a word of its own. A *pattern* is not: `"${v#'a'}"` still strips the `a`.
pub(super) fn parse_operand(source: &str, inside_quotes: bool) -> Result<Word> {
    let mut lexer = Lexer::new(source);
    Ok(Word {
        parts: lexer.scan_word_parts(true, inside_quotes)?,
    })
}

/// Split an unquoted here-document body into the parts expansion will run over.
///
/// A body is not a word and must not be scanned like one. The word scanner terminates nothing
/// when asked to run to end of input, but it still treats `'` and `"` as quotes and `~` as a
/// tilde prefix; inside a heredoc all three are ordinary characters, so `echo "it's fine"` in a
/// body would raise "unterminated single quote" on a script bash runs without complaint.
///
/// The live set is exactly the one double quotes have — `$`, `` ` ``, and a backslash before
/// `$`, `` ` ``, `\` or a newline — which is why this shares the double-quote scanner's shape
/// and differs only in having no closing delimiter to look for.
pub fn parse_heredoc_body(source: &str) -> Result<Word> {
    let mut lexer = Lexer::new(source);
    Ok(Word {
        parts: lexer.scan_heredoc_parts()?,
    })
}

impl Lexer<'_> {
    pub(super) fn scan_word(&mut self) -> Result<Token> {
        let start = self.offset();
        let parts = self.scan_word_parts(false, false)?;

        // A word with no parts that also consumed nothing is not a token, it is a stall. Refusing
        // it here bounds every loop over `next()` no matter which scanner disagreed; see
        // `Lexer::stalled_at`. `''` and `""` are not this case — both produce a part.
        if parts.is_empty() && self.offset() == start {
            return Err(self.stalled_at());
        }

        // A bare run of digits immediately followed by `<` or `>` is an fd number, as in `2>`.
        // One literal part and nothing else is what rules out `\2>` and `"2">`: any quoting at
        // all splits the run into something other than a single `Literal`.
        if let [WordPart::Literal(lit)] = parts.as_slice()
            && !lit.is_empty()
            && lit.bytes().all(|b| b.is_ascii_digit())
            && let Some(op_ch) = self.current_char()
            && (op_ch == '<' || op_ch == '>')
            && let Ok(num) = lit.parse::<i32>()
        {
            return Ok(Token::IoNumber(num));
        }

        // **No reserved words here.** This lexer is not the shell's grammar — rune is, and
        // it recognises `if`/`do`/`in` in the only place they mean anything, which is command
        // position. What reaches this function is an array literal or a declaration payload, where
        // both callers treat anything but a `Word` as failure: `declare -a a=(x do y)` was refused
        // as a bad array value while a bare `a=(x do y)` accepted it, so one shell gave two answers
        // for the same literal.
        Ok(Token::Word(Word { parts }))
    }

    /// Scan word parts up to the first separator, or to end of input when `to_eof`.
    ///
    /// `inside_quotes` is for the payload of `${x-word}` written inside double quotes, where the
    /// enclosing context governs: a single quote and a tilde are ordinary characters there, and a
    /// backslash escapes only what it escapes inside double quotes. A nested double quote still
    /// opens a quoted run — `"${1+"$@"}"` forwards an argument list and depends on it.
    fn scan_word_parts(&mut self, to_eof: bool, inside_quotes: bool) -> Result<Vec<WordPart>> {
        let mut parts = Vec::new();
        let mut current_lit = String::new();

        while let Some(ch) = self.current_char() {
            // `is_blank`, not `char::is_whitespace`: the two must name the same set as
            // `skip_whitespace` or a character in the gap ends the word here and is then skipped
            // by nobody, which is a lexer that cannot advance. `\n` is covered by
            // `is_operator_char`.
            if !to_eof && (is_blank(ch) || is_operator_char(ch)) {
                break;
            }

            match ch {
                // Inside double quotes a backslash escapes only these four; before anything else it
                // is a character in its own right, which is why `"${x:-a\ b}"` keeps it.
                '\\' if inside_quotes
                    && !matches!(self.peek_char(), Some('$' | '`' | '\\' | '"')) =>
                {
                    self.advance();
                    current_lit.push('\\');
                }
                // **`\` before a newline is a line continuation: both go.** Every other character
                // is escaped *to* itself, and treating a newline the same way spliced a literal
                // newline into the middle of the word — `P=/usr/bin:\` + newline + `/usr/local/bin`
                // became a `$P` with a line break in it, where bash and dash both join the two
                // halves. POSIX 2.2.1 names this as the one exception to the escape rule.
                '\\' if self.peek_char() == Some('\n') => {
                    self.advance();
                    self.advance();
                }
                '\\' => {
                    self.advance();
                    if let Some(escaped) = self.current_char() {
                        if !current_lit.is_empty() {
                            parts.push(WordPart::Literal(std::mem::take(&mut current_lit)));
                        }
                        // Not folded into `current_lit`: an escaped character is quoted, and the
                        // expander has to be able to tell it apart from text the user typed bare
                        // so that `echo \*` prints `*` instead of globbing.
                        parts.push(WordPart::Escaped(escaped.to_string()));
                        self.advance();
                    }
                }
                // Inside double quotes a single quote is an ordinary character, which is why
                // `"${x-'q'}"` keeps both of them.
                '\'' if inside_quotes => {
                    self.advance();
                    current_lit.push('\'');
                }
                '\'' => {
                    self.advance();
                    if !current_lit.is_empty() {
                        parts.push(WordPart::Literal(std::mem::take(&mut current_lit)));
                    }
                    let sq = self.scan_single_quote()?;
                    parts.push(WordPart::SingleQuoted(sq));
                }
                '"' => {
                    self.advance();
                    if !current_lit.is_empty() {
                        parts.push(WordPart::Literal(std::mem::take(&mut current_lit)));
                    }
                    let dq_parts = self.scan_double_quote()?;
                    parts.push(WordPart::DoubleQuoted(dq_parts));
                }
                '$' => {
                    self.advance();
                    if !current_lit.is_empty() {
                        parts.push(WordPart::Literal(std::mem::take(&mut current_lit)));
                    }
                    let exp = self.scan_dollar_expansion(false)?;
                    parts.push(exp);
                }
                '`' => {
                    if !current_lit.is_empty() {
                        parts.push(WordPart::Literal(std::mem::take(&mut current_lit)));
                    }
                    parts.push(self.scan_backquote_substitution()?);
                }
                // And a tilde is a tilde: `"${x:-~}"` is a literal one in bash.
                '~' if !inside_quotes && opens_tilde(&parts, &current_lit) => {
                    self.advance();
                    // The text before it goes down first. Every other branch here does this and
                    // this one did not have to, because it only ever fired on an empty buffer —
                    // now that a tilde can follow an `=`, `a=~/x` would otherwise come back as
                    // `/home/youa=/x`, with the expansion in front of the text it followed.
                    if !current_lit.is_empty() {
                        parts.push(WordPart::Literal(std::mem::take(&mut current_lit)));
                    }
                    let mut user = String::new();
                    while let Some(c) = self.current_char() {
                        // `+` is here for `~+` (the current directory); `-` doubles as `~-` (the
                        // previous one) and as a character usernames are allowed to contain, so
                        // which of the two a run means is decided by expansion, not by the lexer.
                        if c.is_alphanumeric() || c == '_' || c == '-' || c == '+' {
                            user.push(c);
                            self.advance();
                        } else {
                            break;
                        }
                    }
                    parts.push(WordPart::Tilde(user));
                }
                _ => {
                    current_lit.push(ch);
                    self.advance();
                }
            }
        }

        if !current_lit.is_empty() {
            parts.push(WordPart::Literal(current_lit));
        }

        Ok(parts)
    }

    fn scan_single_quote(&mut self) -> Result<String> {
        let mut content = String::new();
        while let Some(ch) = self.current_char() {
            if ch == '\'' {
                self.advance();
                return Ok(content);
            }
            content.push(ch);
            self.advance();
        }
        Err(ShellError::SyntaxError(
            "Unterminated single quote".to_string(),
        ))
    }

    pub(super) fn scan_double_quote(&mut self) -> Result<Vec<WordPart>> {
        self.scan_quoted_run(Some('"'))
    }

    /// Scan a heredoc body: the same live set, no closing delimiter, EOF is the end.
    fn scan_heredoc_parts(&mut self) -> Result<Vec<WordPart>> {
        self.scan_quoted_run(None)
    }

    /// The run shared by double quotes and here-document bodies.
    ///
    /// `closing` is the character that ends the run, and it doubles as the extra character a
    /// backslash may escape — `\"` is a quote inside `"…"`, while in a heredoc body there is no
    /// such character and `\"` stays two characters, exactly as bash writes it out.
    ///
    /// `None` also means running off the end is not an error: a body ends where the delimiter
    /// line was, and the caller has already cut it there.
    fn scan_quoted_run(&mut self, closing: Option<char>) -> Result<Vec<WordPart>> {
        let mut parts = Vec::new();
        let mut current_lit = String::new();

        while let Some(ch) = self.current_char() {
            if Some(ch) == closing {
                self.advance();
                if !current_lit.is_empty() {
                    parts.push(WordPart::Literal(current_lit));
                }
                return Ok(parts);
            }
            match ch {
                '\\' => {
                    self.advance();
                    if let Some(next) = self.current_char() {
                        if matches!(next, '$' | '`' | '\\' | '\n') || Some(next) == closing {
                            if next != '\n' {
                                current_lit.push(next);
                            }
                            self.advance();
                        } else {
                            current_lit.push('\\');
                        }
                    } else {
                        current_lit.push('\\');
                    }
                }
                '$' => {
                    self.advance();
                    if !current_lit.is_empty() {
                        parts.push(WordPart::Literal(std::mem::take(&mut current_lit)));
                    }
                    // `$'…'` and `$"…"` are inert inside double quotes — bash echoes `$'a\tb'`
                    // back unchanged — so the flag says so.
                    let exp = self.scan_dollar_expansion(true)?;
                    parts.push(exp);
                }
                // Backquotes stay live inside double quotes, unlike single quotes.
                '`' => {
                    if !current_lit.is_empty() {
                        parts.push(WordPart::Literal(std::mem::take(&mut current_lit)));
                    }
                    parts.push(self.scan_backquote_substitution()?);
                }
                _ => {
                    current_lit.push(ch);
                    self.advance();
                }
            }
        }

        if closing.is_some() {
            return Err(ShellError::SyntaxError(
                "Unterminated double quote".to_string(),
            ));
        }
        if !current_lit.is_empty() {
            parts.push(WordPart::Literal(current_lit));
        }
        Ok(parts)
    }
}

#[cfg(test)]
#[path = "quoting/tests.rs"]
mod tests;

/// Whether a `~` at this point in a word opens a tilde prefix.
///
/// At the start of a word, which is the obvious case — and **immediately after an unquoted `=` or
/// `:`**, which is the one that was missing. POSIX names it for assignment words; bash applies it
/// to any word, and the differential corpus is compared against bash:
///
/// ```text
///   export HOME_BIN=~/bin        the value of an assignment that is an argument
///   local p=~/x                  the same inside a function
///   PATH=$PATH:~/bin             after a `:`, which is what makes it worth having
///   echo a=~/x                   any word at all, in bash
/// ```
///
/// A plain `a=~/x` already worked, because an assignment splits its value off and the `~` then
/// opens that value. Everything above is the same expansion arriving by a different route, and
/// answering it here means one rule rather than one per route.
///
/// Quoting never reaches this: `"~/x"` is read by the double-quote branch and stays literal, which
/// is what bash does too.
fn opens_tilde(parts: &[WordPart], current_lit: &str) -> bool {
    if parts.is_empty() && current_lit.is_empty() {
        return true;
    }
    matches!(current_lit.as_bytes().last(), Some(b'=' | b':'))
}
