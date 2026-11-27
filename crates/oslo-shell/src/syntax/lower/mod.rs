//! The parts of the lowering that do not depend on which parser produced the tree.
//!
//! Everything in here works on a word's *source text*. That is what made it worth separating: the
//! rules for what a `[[ ]]` operand means, and for turning a word's text into
//! [`oslo_base::ast::WordPart`]s, are facts about shell rather than about a parser, and they
//! survived the parser being replaced without a line of them changing.

pub mod cond;
pub mod words;

pub(super) use words::{
    convert_braced_words_from_str, convert_braced_words_with_tilde, convert_words_from_str,
    single_word_from_str, single_word_with_tilde,
};
