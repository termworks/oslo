//! Scanning `$` expansions.
//!
//! Covers `$var`, `${...}` in all its parameter-expansion forms, `$(...)` and backtick command
//! substitution, and `$((...))` arithmetic.

use super::ansi_c;
use super::param;
use super::scanner::{Lexer, is_special_param, is_var_name_char};
use oslo_base::ast::{ParamExpansion, WordPart};
use oslo_base::error::{Result, ShellError};

impl Lexer<'_> {
    /// Step over any `\` + newline pairs at the cursor: a line continuation is nothing at all.
    fn skip_line_continuations(&mut self) {
        while self.current_char() == Some('\\') && self.peek_char() == Some('\n') {
            self.advance();
            self.advance();
        }
    }

    /// Scan whatever follows a `$`, the cursor sitting on the character after it.
    ///
    /// `in_double_quotes` is not cosmetic: `$'…'` and `$"…"` are only special *outside* quotes.
    /// bash prints `$'a\tb'` for `echo "$'a\tb'"`, so inside a double-quoted run the `$` has to
    /// stay the literal it always was.
    pub(super) fn scan_dollar_expansion(&mut self, in_double_quotes: bool) -> Result<WordPart> {
        self.skip_line_continuations();
        match self.current_char() {
            Some('{' | '(') => self.scan_enclosed(in_double_quotes),
            // `$[expr]` is the old spelling of `$((expr))`.
            Some('[') => {
                self.advance();
                Ok(WordPart::Arithmetic(self.scan_old_arithmetic()?))
            }
            Some('\'') if !in_double_quotes => {
                self.advance();
                let decoded = self.scan_ansi_c_quote()?;
                // Single-quoted, not literal: the decoded text is data. A decoded `*` must not
                // glob and a decoded newline must not field-split.
                Ok(WordPart::SingleQuoted(decoded))
            }
            Some('"') if !in_double_quotes => {
                self.advance();
                // `$"…"` marks a translatable string. With no message catalogue there is nothing
                // to translate, so it is exactly a double-quoted run — which is also what bash
                // does when the string is not in the catalogue.
                Ok(WordPart::DoubleQuoted(self.scan_double_quote()?))
            }
            Some(ch) if is_var_name_char(ch) || is_special_param(ch) => {
                let mut name = String::new();
                if is_special_param(ch) {
                    name.push(ch);
                    self.advance();
                } else {
                    // A line continuation inside the name joins its halves: `$fo\⏎o` is `$foo`.
                    loop {
                        self.skip_line_continuations();
                        match self.current_char() {
                            Some(c) if is_var_name_char(c) => {
                                name.push(c);
                                self.advance();
                            }
                            _ => break,
                        }
                    }
                }
                Ok(WordPart::Variable {
                    name,
                    expansion_type: ParamExpansion::Normal,
                })
            }
            _ => Ok(WordPart::Literal("$".to_string())),
        }
    }

    /// Scan a `` `cmd` `` substitution, the cursor on the opening backquote.
    ///
    /// Shared by [`Lexer::scan_word`] and [`Lexer::scan_double_quote`], because backquotes are
    /// live in both. The body is *not* the raw source: inside backquotes only `` \` ``, `\\` and
    /// `\$` are escapes, and the shell strips those backslashes before parsing what is left.
    /// Every other backslash is data, which is why this cannot reuse the raw `$( )` scanner.
    ///
    /// Inside double quotes `\"` is an escape too: ``"`echo \"q\"`"`` runs `echo "q"`.
    pub(super) fn scan_backquote_substitution(
        &mut self,
        in_double_quotes: bool,
    ) -> Result<WordPart> {
        self.advance();
        let mut cmd = String::new();

        while let Some(ch) = self.current_char() {
            match ch {
                '`' => {
                    self.advance();
                    return Ok(WordPart::CommandSubstitution(cmd));
                }
                '\\' if matches!(self.peek_char(), Some('`' | '\\' | '$'))
                    || (in_double_quotes && self.peek_char() == Some('"')) =>
                {
                    self.advance();
                    if let Some(escaped) = self.advance() {
                        cmd.push(escaped);
                    }
                }
                _ => {
                    cmd.push(ch);
                    self.advance();
                }
            }
        }

        Err(ShellError::SyntaxError(
            "Unterminated backquote".to_string(),
        ))
    }

    /// Scan the body of `$'…'`, the cursor just past the opening quote.
    ///
    /// Only `\'` matters while scanning: it is the one escape that stops the string from ending.
    /// Everything else is left for [`ansi_c::decode`].
    fn scan_ansi_c_quote(&mut self) -> Result<String> {
        let mut raw = String::new();

        while let Some(ch) = self.current_char() {
            match ch {
                '\'' => {
                    self.advance();
                    return Ok(ansi_c::decode(&raw));
                }
                '\\' => {
                    self.advance();
                    raw.push('\\');
                    if let Some(next) = self.advance() {
                        raw.push(next);
                    }
                }
                _ => {
                    raw.push(ch);
                    self.advance();
                }
            }
        }

        Err(ShellError::SyntaxError("unterminated $' quote".to_string()))
    }

    /// The body of `$[ … ]`, the cursor just past the `[`: everything up to the matching `]`.
    fn scan_old_arithmetic(&mut self) -> Result<String> {
        let mut depth = 0usize;
        let mut body = String::new();
        while let Some(ch) = self.advance() {
            match ch {
                '[' => depth += 1,
                ']' if depth == 0 => return Ok(body),
                ']' => depth -= 1,
                _ => {}
            }
            body.push(ch);
        }
        Err(ShellError::SyntaxError(
            "unterminated arithmetic expansion".to_string(),
        ))
    }

    /// The payload of `${ … }`: a command for `${ cmd; }` and `${| cmd; }`, a parameter otherwise.
    fn braced_body(&self, raw: &str, in_double_quotes: bool) -> Result<WordPart> {
        let content = without_line_continuations(raw);
        if content.starts_with([' ', '\t', '\n']) {
            return Ok(WordPart::FunctionSubstitution {
                body: content,
                reply: false,
            });
        }
        if let Some(body) = content.strip_prefix('|') {
            return Ok(WordPart::FunctionSubstitution {
                body: body.to_string(),
                reply: true,
            });
        }
        // A payload inside an assignment value takes the value's tilde rule.
        let tilde = if self.colon_tilde {
            super::TildeMode::Value
        } else {
            super::TildeMode::Leading
        };
        param::parse_braced_body(&content, in_double_quotes, tilde)
    }

    /// Scan a `$( … )`, `$(( … ))` or `${ … }`, the cursor on the character after the `$`.
    ///
    /// Where it ends is asked of rune, the parser that read the word: a here-document body, a
    /// `case` pattern, a comment, `$'…'` or a regex group can each hold a closer that closes
    /// nothing. `$((` is arithmetic when its two parentheses close together, so `$((cd /) ; pwd)`
    /// is a substitution whose first command is a subshell.
    fn scan_enclosed(&mut self, in_double_quotes: bool) -> Result<WordPart> {
        let text = self.rest_from(1);
        let Some((kind, len)) = rune::lex::enclosed(text) else {
            let what = match self.current_char() {
                Some('{') => "parameter expansion",
                _ => "command substitution",
            };
            return Err(ShellError::SyntaxError(format!("unterminated {what}")));
        };
        let construct = &text[..len];
        self.advance_bytes(len - 1);
        let inner = |open: usize, close: usize| {
            construct
                .get(open..len - close)
                .unwrap_or_default()
                .to_string()
        };
        match kind {
            rune::SyntaxKind::DollarParenParen => Ok(WordPart::Arithmetic(inner(3, 2))),
            rune::SyntaxKind::DollarParen => Ok(WordPart::CommandSubstitution(inner(2, 1))),
            _ => self.braced_body(&inner(2, 1), in_double_quotes),
        }
    }
}

/// `text` with every `\` + newline outside single quotes removed, as bash reads its input.
fn without_line_continuations(text: &str) -> String {
    if !text.contains("\\\n") {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut single = false;
    while let Some(ch) = chars.next() {
        match ch {
            '\'' => single = !single,
            '\\' if !single && chars.peek() == Some(&'\n') => {
                chars.next();
                continue;
            }
            '\\' if !single => {
                out.push(ch);
                out.extend(chars.next());
                continue;
            }
            _ => {}
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use crate::lexer::{Lexer, Token};
    use oslo_base::ast::{ParamExpansion, WordPart};

    fn parts(src: &str) -> Vec<WordPart> {
        match Lexer::new(src).next() {
            Ok(Token::Word(w)) => w.parts,
            other => panic!("expected a word from {src:?}, got {other:?}"),
        }
    }

    fn one_part(src: &str) -> WordPart {
        let mut p = parts(src);
        assert_eq!(p.len(), 1, "expected one part from {src:?}, got {p:?}");
        p.remove(0)
    }

    // --- R2.8: `$'…'` and `$"…"` ---

    /// The finding that motivated this: `IFS=$'\n'` used to set IFS to `$`, `\`, `n`.
    #[test]
    fn ansi_c_quoting_decodes_to_one_quoted_part() {
        assert_eq!(one_part("$'\\n'"), WordPart::SingleQuoted("\n".into()));
        assert_eq!(one_part("$'a\\tb'"), WordPart::SingleQuoted("a\tb".into()));
        assert_eq!(one_part("$'\\x41'"), WordPart::SingleQuoted("A".into()));
    }

    /// Single-quoted, not literal: a decoded `*` is data and must not reach the globber.
    #[test]
    fn a_decoded_escape_is_quoted_data() {
        assert!(matches!(
            one_part("$'\\052'"),
            WordPart::SingleQuoted(ref s) if s == "*"
        ));
    }

    /// `\'` is the one escape the *scanner* has to know about: it must not end the string.
    #[test]
    fn an_escaped_quote_does_not_end_the_string() {
        assert_eq!(one_part("$'a\\'b'"), WordPart::SingleQuoted("a'b".into()));
    }

    /// bash prints `$'a\tb'` for `echo "$'a\tb'"`: inside double quotes the `$` is just a `$`.
    #[test]
    fn ansi_c_quoting_is_inert_inside_double_quotes() {
        assert_eq!(
            parts("\"$'a\\tb'\""),
            vec![WordPart::DoubleQuoted(vec![
                WordPart::Literal("$".into()),
                WordPart::Literal("'a\\tb'".into()),
            ])]
        );
    }

    /// `$"…"` is a translatable string; with no catalogue it is an ordinary double-quoted run.
    #[test]
    fn dollar_double_quote_is_a_plain_double_quoted_run() {
        assert_eq!(
            one_part("$\"a $b\""),
            WordPart::DoubleQuoted(vec![
                WordPart::Literal("a ".into()),
                WordPart::Variable {
                    name: "b".into(),
                    expansion_type: ParamExpansion::Normal,
                },
            ])
        );
    }
}
