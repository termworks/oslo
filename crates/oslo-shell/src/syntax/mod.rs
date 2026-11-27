//! Parsing: shell source text in, [`oslo_base::ast`] out.
//!
//! There is exactly one parser — `rune`, lowered in `rune_adapter`. oslo used to carry a second,
//! hand-written one as a fallback for anything the first rejected, and that design turned every
//! gap in the adapter into a silent reinterpretation of the whole program: the fallback had no
//! here-document support, so it parsed heredoc *bodies* as commands and ran them. Inert data
//! became code. A single parser that reports its errors is worth far more than a second one that
//! guesses.
pub mod alias;
pub mod lower;
pub mod rune_adapter;

pub use rune_adapter::{array_literal, parse_bash_script};

/// Parse `source` after substituting the aliases `lookup` knows about.
///
/// This is what every caller that has an [`crate::env::Environment`] should use. Alias
/// substitution belongs before parsing — an alias body is source text, not a list of arguments —
/// so it cannot be a step the executor performs afterwards; see [`alias`].
pub fn parse_with_aliases(
    source: &str,
    any_defined: bool,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> oslo_base::error::Result<oslo_base::ast::CommandList> {
    parse_bash_script(&alias::substitute(source, any_defined, lookup))
}
