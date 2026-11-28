//! Word expansion.
//!
//! Split by expansion step so each stage owns one file: [`word`] runs the pipeline and defines the
//! quoting provenance every other stage consults, [`param`] resolves `${...}`, [`fields`] applies
//! IFS, [`glob`] applies pathname expansion, [`tilde`] resolves `~`.
//!
//! `oslo_base::brace` is the odd one out and does not run in that pipeline at all: it splits one word into
//! several *words* rather than fields, and it does so on the word's source text before the lexer
//! has seen it, which is where bash runs it and the only place the answer comes out the same. Its
//! callers are therefore the parser and the two places that lex a word list themselves, not
//! [`word::expand_word`].

pub mod arithmetic;
pub mod fields;
pub mod glob;
pub mod join;
pub mod locale;
pub mod param;
pub mod prompt;
pub mod sugar;
pub mod tilde;
pub mod word;

pub use word::{
    Field, Origin, Run, expand_command_word, expand_word, expand_word_fields, expand_word_part,
    expand_word_to_string,
};
