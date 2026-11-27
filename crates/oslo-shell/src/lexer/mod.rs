//! Tokenizing shell input.
//!
//! Split by what is being scanned: [`scanner`] holds the cursor and operator handling,
//! [`quoting`] scans words and quoted runs, [`expansion`] scans `$`-expansions, the private
//! `param` module splits a `${…}` body into its operator and operands, and the private `ansi_c`
//! module decodes `$'…'` escapes.

mod ansi_c;
pub mod expansion;
mod param;
pub mod quoting;
pub mod scanner;
mod tilde;
pub mod token;

pub use ansi_c::decode as decode_ansi_c;
pub use quoting::parse_heredoc_body;
pub use scanner::{Lexer, TildeMode};
pub use token::Token;

use oslo_base::ast::Word;
use oslo_base::error::Result;

/// Parse a string that is known to hold exactly one word into its parts.
///
/// Nothing terminates the word: whitespace and operator characters are ordinary text, because the
/// caller has already decided where the word ends. Arithmetic expansion needs this — POSIX runs
/// parameter expansion, command substitution and quote removal over the expression *text* before
/// any of it is arithmetic, and `$(( $(wc -l < f) * 2 ))` is one such expression, spaces and all.
pub fn parse_single_word(source: &str) -> Result<Word> {
    quoting::parse_word_source(source)
}
