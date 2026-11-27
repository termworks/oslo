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
    /// Scan whatever follows a `$`, the cursor sitting on the character after it.
    ///
    /// `in_double_quotes` is not cosmetic: `$'…'` and `$"…"` are only special *outside* quotes.
    /// bash prints `$'a\tb'` for `echo "$'a\tb'"`, so inside a double-quoted run the `$` has to
    /// stay the literal it always was.
    pub(super) fn scan_dollar_expansion(&mut self, in_double_quotes: bool) -> Result<WordPart> {
        match self.current_char() {
            Some('{') => {
                self.advance();
                self.scan_braced_param(in_double_quotes)
            }
            Some('(') => {
                self.advance();
                if self.current_char() == Some('(') {
                    self.advance();
                    // `$((` is ambiguous: arithmetic, or a substitution whose first command is a
                    // subshell. Scan one balanced paren group and let the character after it
                    // decide — `$((1+2))` closes twice in a row, `$((cd /) ; pwd)` does not.
                    let inner = self.scan_raw_delimited('(', ')', "arithmetic expansion", 0)?;
                    if self.current_char() == Some(')') {
                        self.advance();
                        return Ok(WordPart::Arithmetic(inner));
                    }
                    let rest = self.scan_raw_delimited('(', ')', "command substitution", 0)?;
                    return Ok(WordPart::CommandSubstitution(format!("({inner}){rest}")));
                }
                let cmd = self.scan_raw_delimited('(', ')', "command substitution", 0)?;
                Ok(WordPart::CommandSubstitution(cmd))
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
                    while let Some(c) = self.current_char() {
                        if is_var_name_char(c) {
                            name.push(c);
                            self.advance();
                        } else {
                            break;
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
    pub(super) fn scan_backquote_substitution(&mut self) -> Result<WordPart> {
        self.advance();
        let mut cmd = String::new();

        while let Some(ch) = self.current_char() {
            match ch {
                '`' => {
                    self.advance();
                    return Ok(WordPart::CommandSubstitution(cmd));
                }
                '\\' if matches!(self.peek_char(), Some('`') | Some('\\') | Some('$')) => {
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

    fn scan_braced_param(&mut self, in_double_quotes: bool) -> Result<WordPart> {
        // Depth- and quote-aware, so `${x:-${y}}` keeps its whole payload and `${x:-a}b}` does
        // not end early on the brace inside the quotes.
        let content = self.scan_raw_delimited('{', '}', "parameter expansion", 0)?;
        // A payload inside an assignment value takes the value's tilde rule.
        let tilde = if self.colon_tilde {
            super::TildeMode::Value
        } else {
            super::TildeMode::Leading
        };
        param::parse_braced_body(&content, in_double_quotes, tilde)
    }
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
