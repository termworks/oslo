//! What the shell's locale says about characters.
//!
//! bash re-reads `LC_ALL`, `LC_CTYPE` and `LANG` whenever one is assigned, so a script that sets
//! `LC_ALL=C` changes how `${#x}` counts and how `${x^^}` maps from that line on. With none of
//! them set the locale is C, where every byte is a character and only ASCII has a case.

use crate::env::Environment;

/// Whether the locale in force is a UTF-8 one: the first of `LC_ALL`, `LC_CTYPE` and `LANG`
/// that is set and not empty names it.
pub fn is_utf8(env: &Environment) -> bool {
    let name = ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .find_map(|var| env.get_var(var).filter(|value| !value.is_empty()))
        .unwrap_or("C")
        .to_ascii_lowercase();
    name.contains("utf-8") || name.contains("utf8")
}

/// The length `${#x}` reports: characters in a UTF-8 locale, bytes otherwise.
pub fn length(env: &Environment, value: &str) -> usize {
    match is_utf8(env) {
        true => value.chars().count(),
        false => oslo_base::lossless::byte_len(value),
    }
}
