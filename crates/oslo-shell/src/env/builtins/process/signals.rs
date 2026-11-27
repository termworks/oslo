//! Signal names and numbers, in both directions, numbered as bash numbers them.
//!
//! nix's `Signal` only round-trips the canonical `SIGxxx` spelling and only covers the standard
//! signals. Every shell also accepts the bare name (`kill -HUP`), the number (`kill -1`) and,
//! on Linux, the realtime names (`kill -RTMIN+3`) — so the mapping lives here.
//!
//! The realtime signals are numbered 34 to 64, as bash on glibc lists them: `RTMIN` is 34 even
//! when this libc reserves more of the range. [`real`] and [`from_real`] convert between that
//! numbering and the signal the kernel is actually asked to deliver.

use nix::sys::signal::Signal;
use std::str::FromStr;

/// Signal numbers a `kill -l` listing walks before the realtime ones.
const STANDARD_RANGE: std::ops::RangeInclusive<i32> = 1..=31;

/// bash's realtime numbering.
const RT_MIN: i32 = 34;
const RT_MAX: i32 = 64;
/// The last `RTMIN+n` before the names switch to counting back from `RTMAX`.
const RT_LOW_HALF: i32 = 15;

/// Resolve a `kill` signal spec — a name, a `SIG`-prefixed name, or a number.
///
/// `0` resolves to `Some(0)`: it is not a signal but it is a valid spec, meaning "probe".
pub fn parse_spec(spec: &str) -> Option<i32> {
    if let Ok(num) = spec.parse::<i32>() {
        return (num == 0 || name_from_number(num).is_some()).then_some(num);
    }
    number_from_name(spec)
}

/// Number for a signal name, with or without the `SIG` prefix, in any case.
pub fn number_from_name(name: &str) -> Option<i32> {
    let upper = name.to_uppercase();
    let bare = upper.strip_prefix("SIG").unwrap_or(&upper);
    if bare.is_empty() {
        return None;
    }
    if let Some(rt) = realtime_from_name(bare) {
        return Some(rt);
    }
    Signal::from_str(&format!("SIG{bare}"))
        .ok()
        .map(|s| s as i32)
}

/// Name (without the `SIG` prefix) for a signal number.
pub fn name_from_number(num: i32) -> Option<String> {
    if let Some(rt) = realtime_name(num) {
        return Some(rt);
    }
    if !STANDARD_RANGE.contains(&num) {
        return None;
    }
    Signal::try_from(num)
        .ok()
        .map(|s| s.as_str().trim_start_matches("SIG").to_string())
}

/// Every signal, in number order — the operand-less `kill -l` listing.
pub fn all() -> Vec<(i32, String)> {
    STANDARD_RANGE
        .chain(RT_MIN..=RT_MAX)
        .filter_map(|n| name_from_number(n).map(|name| (n, name)))
        .collect()
}

/// bash's numbered table: five `NN) SIGNAME` cells a line, each followed by a tab.
pub fn table() -> String {
    let mut out = String::new();
    for (i, (num, name)) in all().into_iter().enumerate() {
        out.push_str(&format!("{num:2}) SIG{name}"));
        out.push(if i % 5 == 4 { '\n' } else { '\t' });
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// The signal the kernel delivers for bash's number `num`.
pub fn real(num: i32) -> i32 {
    let (min, max) = (nix::libc::SIGRTMIN(), nix::libc::SIGRTMAX());
    match num {
        RT_MIN..=RT_MAX if num - RT_MIN <= RT_LOW_HALF => (min + (num - RT_MIN)).min(max),
        RT_MIN..=RT_MAX => (max - (RT_MAX - num)).max(min),
        _ => num,
    }
}

/// bash's number for the signal the kernel delivered.
pub fn from_real(signum: i32) -> i32 {
    let (min, max) = (nix::libc::SIGRTMIN(), nix::libc::SIGRTMAX());
    if !(min..=max).contains(&signum) {
        return signum;
    }
    if signum - min <= RT_LOW_HALF {
        RT_MIN + (signum - min)
    } else {
        RT_MAX - (max - signum)
    }
}

/// `RTMIN`, `RTMAX`, `RTMIN+n` and `RTMAX-n`, the spelling `kill -l` prints and accepts back.
fn realtime_from_name(bare: &str) -> Option<i32> {
    let num = if let Some(rest) = bare.strip_prefix("RTMIN") {
        RT_MIN.checked_add(offset(rest, '+')?)?
    } else {
        let rest = bare.strip_prefix("RTMAX")?;
        RT_MAX.checked_sub(offset(rest, '-')?)?
    };
    (RT_MIN..=RT_MAX).contains(&num).then_some(num)
}

/// The `+n` / `-n` tail of a realtime name. An empty tail is offset zero.
fn offset(tail: &str, sign: char) -> Option<i32> {
    if tail.is_empty() {
        return Some(0);
    }
    tail.strip_prefix(sign)?
        .parse::<i32>()
        .ok()
        .filter(|n| *n >= 0)
}

/// Name a realtime signal the way bash does: `RTMIN` to `RTMIN+15`, then `RTMAX-14` to `RTMAX`.
fn realtime_name(num: i32) -> Option<String> {
    if !(RT_MIN..=RT_MAX).contains(&num) {
        return None;
    }
    let from_min = num - RT_MIN;
    Some(match from_min {
        0 => "RTMIN".to_string(),
        n if n <= RT_LOW_HALF => format!("RTMIN+{n}"),
        _ if num == RT_MAX => "RTMAX".to_string(),
        _ => format!("RTMAX-{}", RT_MAX - num),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_resolve_with_and_without_the_sig_prefix() {
        assert_eq!(number_from_name("HUP"), Some(1));
        assert_eq!(number_from_name("SIGHUP"), Some(1));
        assert_eq!(number_from_name("hup"), Some(1));
        assert_eq!(number_from_name("sigusr1"), number_from_name("USR1"));
        assert_eq!(number_from_name("NOSUCHSIG"), None);
        assert_eq!(number_from_name(""), None);
        assert_eq!(number_from_name("SIG"), None);
    }

    #[test]
    fn numbers_round_trip_to_names() {
        assert_eq!(name_from_number(9).as_deref(), Some("KILL"));
        assert_eq!(name_from_number(15).as_deref(), Some("TERM"));
        assert_eq!(name_from_number(0), None);
        assert_eq!(name_from_number(32), None);
        assert_eq!(name_from_number(4096), None);
        for (num, name) in all() {
            assert_eq!(number_from_name(&name), Some(num), "{name} -> {num}");
        }
        assert_eq!(all().len(), 62);
    }

    #[test]
    fn zero_is_a_valid_spec_but_not_a_signal() {
        assert_eq!(parse_spec("0"), Some(0));
        assert_eq!(parse_spec("9"), Some(9));
        assert_eq!(parse_spec("99"), None);
        assert_eq!(parse_spec("-1"), None);
        assert_eq!(parse_spec("KILL"), Some(9));
        assert_eq!(parse_spec("NOSUCHSIG"), None);
    }

    /// bash's numbers, whatever this libc reserves.
    #[test]
    fn realtime_names_use_bashs_numbers() {
        assert_eq!(number_from_name("RTMIN"), Some(34));
        assert_eq!(number_from_name("RTMAX"), Some(64));
        assert_eq!(number_from_name("SIGRTMIN+1"), Some(35));
        assert_eq!(number_from_name("RTMAX-14"), Some(50));
        assert_eq!(name_from_number(49).as_deref(), Some("RTMIN+15"));
        assert_eq!(number_from_name("RTMIN+999"), None);
        assert_eq!(real(34), nix::libc::SIGRTMIN());
        assert_eq!(real(64), nix::libc::SIGRTMAX());
        assert_eq!(from_real(nix::libc::SIGRTMIN()), 34);
        assert_eq!(real(15), 15);
    }

    #[test]
    fn the_table_is_five_to_a_line() {
        let table = table();
        assert!(table.starts_with(" 1) SIGHUP\t 2) SIGINT\t"));
        assert!(table.contains("5) SIGTRAP\n 6) SIGABRT"));
        assert!(table.ends_with("63) SIGRTMAX-1\t64) SIGRTMAX\t\n"));
    }
}
