//! What a forked pipeline stage is, for `$BASH_SUBSHELL`.

use crate::env::Environment;
use oslo_base::ast::{Command, WordPart};

/// Whether a stage counts as one more subshell level.
///
/// bash forks a simple command in a pipeline without entering a subshell, so only a compound
/// command or a call to a shell function makes `$BASH_SUBSHELL` one deeper there.
pub(super) fn stage_is_nested(env: &Environment, cmd: &Command) -> bool {
    let Command::Simple(simple) = cmd else {
        return true;
    };
    match simple.words.first().map(|word| word.parts.as_slice()) {
        Some([WordPart::Literal(name)]) => env.get_function(name).is_some(),
        _ => false,
    }
}
