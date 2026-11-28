//! The wall clock, formatted the way `strftime` formats it.
//!
//! **Local rather than UTC.** The rest of oslo's date handling refuses timezones on the grounds
//! that a plausible-but-wrong timestamp is worse than none, which is right for a *script*. A clock
//! read at a glance is the opposite case: it is only useful if it agrees with the wall.
//!
//! Here rather than in either crate that draws one, because both do — the prompt's `\t` and `\A`
//! escapes and the transcript's stamp — and two copies of a `localtime_r` call is two places for
//! the timezone handling to differ.

/// The current local time under a `strftime` format, or an empty string if the system cannot say.
pub fn local(format: &str) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    at(now, format)
}

/// A given epoch second under a `strftime` format.
///
/// The same call as [`local`], because a `Val::Time` in a drawn table and the prompt's `\t` are the
/// same question asked about a different second — and two `localtime_r` calls is two places for the
/// timezone handling to drift apart, which is what this module exists to prevent.
/// **`i64`, and not spelled `time_t`.** `nix::libc::time_t` is deprecated on musl — the alias is
/// changing to 64 bits and naming it warns — and the release binary is a musl one, so the cast that
/// looked like the careful thing to write was the one that broke `make build`. `i64` is what the
/// alias already is on every target oslo runs on, which is Linux, and it is what [`local`] passed
/// before this function existed.
pub fn at(seconds: i64, format: &str) -> String {
    let out = format_at(seconds, format.as_bytes()).unwrap_or_default();
    String::from_utf8(out).unwrap_or_default()
}

unsafe extern "C" {
    fn tzset();
}

/// `strftime` of `seconds` in the local zone as `TZ` names it now, into at most 127 bytes.
///
/// `None` when the second has no local time. A result that does not fit, or a format with a NUL
/// in it, is empty.
pub fn format_at(seconds: i64, format: &[u8]) -> Option<Vec<u8>> {
    let mut tm: nix::libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: `tzset` takes no arguments and only reads the environment; `seconds` is a valid
    // `time_t` and `tm` is owned here for the whole call.
    if unsafe {
        tzset();
        nix::libc::localtime_r(&seconds, &mut tm)
    }
    .is_null()
    {
        return None;
    }
    let mut out = vec![0u8; 128];
    let Ok(c_format) = std::ffi::CString::new(format) else {
        return Some(Vec::new());
    };
    // SAFETY: a buffer this call owns, its own length, and a NUL-terminated format.
    let written =
        unsafe { nix::libc::strftime(out.as_mut_ptr().cast(), out.len(), c_format.as_ptr(), &tm) };
    out.truncate(written);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_format_comes_back_filled_in() {
        let stamp = local("%H:%M:%S");
        assert_eq!(stamp.len(), 8, "hh:mm:ss, got {stamp:?}");
        assert!(
            stamp.chars().all(|c| c.is_ascii_digit() || c == ':'),
            "{stamp:?}"
        );
        // A format with no directives is itself, which is what says the buffer is not being cut.
        assert_eq!(local("plain"), "plain");
        // An empty result rather than a panic: `strftime` cannot take an interior NUL.
        assert_eq!(local("a\0b"), "");
    }
}
