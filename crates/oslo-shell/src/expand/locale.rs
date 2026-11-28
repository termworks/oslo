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

/// `value` with every byte a character of its own, as the C locale reads it: a substring, a `?`
/// and a bracket expression then count bytes.
pub fn bytewise(value: &str) -> String {
    oslo_base::lossless::decode(value)
        .into_iter()
        .map(|byte| match byte.is_ascii() {
            true => char::from(byte),
            false => oslo_base::lossless::raw_char(byte),
        })
        .collect()
}

/// The text a [`bytewise`] string stands for.
pub fn from_bytewise(value: &str) -> String {
    oslo_base::lossless::from_bytes(&oslo_base::lossless::decode(value))
}

/// `value` as a pattern operator reads it: [`bytewise`] when `bytes`, as it is otherwise.
pub fn in_locale(value: &str, bytes: bool) -> String {
    match bytes {
        true => bytewise(value),
        false => value.to_string(),
    }
}

/// The text an [`in_locale`] result stands for.
pub fn out_of_locale(value: String, bytes: bool) -> String {
    match bytes {
        true => from_bytewise(&value),
        false => value,
    }
}

/// [`in_locale`] for each run of a pattern, keeping its quoting.
pub fn runs_in_locale(runs: &mut [crate::expand::word::Run], bytes: bool) {
    if bytes {
        for run in runs {
            run.text = bytewise(&run.text);
        }
    }
}

/// The length `${#x}` reports: characters in a UTF-8 locale, bytes otherwise.
pub fn length(env: &Environment, value: &str) -> usize {
    match is_utf8(env) {
        true => value.chars().count(),
        false => oslo_base::lossless::byte_len(value),
    }
}
