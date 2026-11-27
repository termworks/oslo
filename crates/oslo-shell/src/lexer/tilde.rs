//! Where a `~` opens a tilde prefix, and where the prefix ends.
//!
//! bash decides per word, from the word's shape and the place it stands in:
//!
//! ```text
//!   ~/x  ~root            a leading tilde, in every word
//!   a=~/x  --x=~ (no)     after the `=` of a word shaped like `name=…`, then after each `:`
//!   PATH=$P:~/bin         the value of an assignment: leading, and after each `:`
//!   arr=(x=~ [1]=~)       an array element: leading, and in the value of `[i]=`
//!   ${v:-~}  <<< ~        only a leading one
//! ```
//!
//! A prefix runs to the first `/` (or `:`), and is not a prefix at all when a quote or an
//! expansion falls inside it: `~"/x"` and `~$USER` keep their tilde.

use super::scanner::{Lexer, is_blank, is_operator_char};
use oslo_base::ast::WordPart;

/// Where the words a lexer scans may open a tilde prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TildeMode {
    /// An ordinary word: a leading `~`, and in a word shaped like `name=…` one right after that
    /// `=` and one after each later unquoted `:`.
    Word,
    /// The value of an assignment: a leading `~` and one after each unquoted `:`.
    Value,
    /// An element of an array literal: a leading `~`, and in `[i]=…` the value's as in `Value`.
    Element,
    /// Only a leading `~`.
    Leading,
}

impl Lexer<'_> {
    /// Whether a `~` at this point of the word opens a tilde prefix.
    ///
    /// `eq_at` is the length the literal had just after the word's assignment `=`, if it has one.
    pub(super) fn opens_tilde(&self, parts: &[WordPart], lit: &str, eq_at: Option<usize>) -> bool {
        if parts.is_empty() && (lit.is_empty() || eq_at == Some(lit.len())) {
            return true;
        }
        self.colon_tilde && lit.ends_with(':')
    }

    /// Whether an `=` that follows `lit` is the assignment `=` of a `name=…` word.
    pub(super) fn is_assignment_eq(&self, parts: &[WordPart], lit: &str) -> bool {
        if !parts.is_empty() || self.colon_tilde {
            return false;
        }
        let name = lit.strip_suffix('+').unwrap_or(lit);
        match self.tilde {
            TildeMode::Word => {
                let base = match name.find('[') {
                    Some(open) if name.ends_with(']') => &name[..open],
                    Some(_) => return false,
                    None => name,
                };
                crate::env::scope::is_valid_identifier(base)
            }
            TildeMode::Element => name.len() > 2 && name.starts_with('[') && name.ends_with(']'),
            TildeMode::Value | TildeMode::Leading => false,
        }
    }

    /// Scan a tilde prefix, the cursor on the `~`, and return the user name it holds.
    ///
    /// `None` leaves the cursor where it was: a quote, an expansion or a glob character inside
    /// the prefix means there is no prefix, and the `~` is an ordinary character.
    pub(super) fn scan_tilde_prefix(&mut self, to_eof: bool) -> Option<String> {
        let mut user = String::new();
        let mut ahead = 1;
        let mut ended = false;
        while let Some(c) = self.char_ahead(ahead) {
            if c == '/' || (!to_eof && (is_blank(c) || is_operator_char(c))) {
                break;
            }
            if matches!(c, '\\' | '\'' | '"' | '$' | '`') {
                return None;
            }
            if c == ':' || (c == '=' && self.char_ahead(ahead + 1) == Some('~')) {
                if self.colon_tilde {
                    break;
                }
                ended = true;
            } else if !ended && matches!(c, '*' | '?' | '[') {
                return None;
            }
            if !ended {
                user.push(c);
            }
            ahead += 1;
        }
        for _ in 0..=user.chars().count() {
            self.advance();
        }
        Some(user)
    }
}
