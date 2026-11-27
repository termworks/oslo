//! The report the `time` keyword leaves behind.
//!
//! Split out of [`super`] because it is the only part of pipeline evaluation that asks the kernel
//! about resource usage, and because the report is purely a *side* channel: a timed pipeline runs,
//! writes and reports its status exactly as an untimed one does. The single rule this module
//! exists to enforce is that the three lines go to **stderr**, never stdout — `x=$(time cmd)` has
//! to capture `cmd`'s output alone, and a `time` that polluted a command substitution would be
//! worse than the silent drop it replaced (R8.7).

use nix::libc;
use oslo_base::ast::{Command, Pipeline, Word};
use std::time::{Duration, Instant};

/// CPU time consumed so far by this shell and by every child it has already reaped.
///
/// Both halves are needed. A builtin (`time read x`) burns only the shell's own time; an external
/// command (`time sleep 1`) only its children's; a mixed pipeline splits between them. Reporting
/// either alone makes half the pipelines a script can write look free.
///
/// `RUSAGE_CHILDREN` is cumulative over the shell's whole life, which is why callers take a
/// snapshot before the pipeline and subtract — see [`Timer`].
#[derive(Clone, Copy)]
struct CpuTimes {
    user: Duration,
    sys: Duration,
}

impl CpuTimes {
    fn now() -> Self {
        let (self_user, self_sys) = getrusage(libc::RUSAGE_SELF);
        let (child_user, child_sys) = getrusage(libc::RUSAGE_CHILDREN);
        Self {
            user: self_user + child_user,
            sys: self_sys + child_sys,
        }
    }
}

/// The user and system time `who` reports, or zero if the kernel refuses to say.
///
/// Zero rather than an error: `time` is a report, not a command, and failing the pipeline because
/// its accounting could not be read would turn a cosmetic problem into a broken script.
fn getrusage(who: libc::c_int) -> (Duration, Duration) {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: `getrusage` writes only through the pointer it is given, which points at a
    // correctly sized and aligned `rusage` owned by this frame.
    let rc = unsafe { libc::getrusage(who, usage.as_mut_ptr()) };
    if rc != 0 {
        return (Duration::ZERO, Duration::ZERO);
    }
    // SAFETY: `getrusage` returned 0, so it filled the struct.
    let usage = unsafe { usage.assume_init() };
    (
        timeval_to_duration(usage.ru_utime),
        timeval_to_duration(usage.ru_stime),
    )
}

/// A `timeval` as a [`Duration`], clamping rather than trusting the sign.
///
/// `Duration` cannot hold a negative span and `time_t`/`suseconds_t` are signed; a hostile or
/// buggy value must not panic the shell in the middle of reporting a timing.
fn timeval_to_duration(tv: libc::timeval) -> Duration {
    let secs = tv.tv_sec.max(0) as u64;
    let micros = tv.tv_usec.clamp(0, 999_999) as u32;
    Duration::new(secs, micros * 1_000)
}

/// A measurement in progress. [`Timer::report`] stops it and writes the three lines.
pub(super) struct Timer {
    wall: Instant,
    cpu: CpuTimes,
}

impl Timer {
    pub(super) fn start() -> Self {
        Self {
            wall: Instant::now(),
            cpu: CpuTimes::now(),
        }
    }

    /// Write `real`, `user` and `sys` for everything that happened since [`Timer::start`], in
    /// `format` — `$TIMEFORMAT`, or `None` for bash's default.
    pub(super) fn report(self, format: Option<&str>) {
        let real = self.wall.elapsed();
        let end = CpuTimes::now();
        // Saturating, not plain subtraction: `getrusage` is monotonic, but a failed call reports
        // zero, and a zero "after" would otherwise wrap into a 584-year `user`.
        let user = end.user.saturating_sub(self.cpu.user);
        let sys = end.sys.saturating_sub(self.cpu.sys);
        // The config gets first refusal on the three lines. Fired from `time`'s own report, which
        // is inside the executor — a handler may draw, but not change the shell. See
        // `ui::report`.
        match format {
            // An empty `TIMEFORMAT` asks for no report at all.
            Some("") => {}
            Some(format) => match render(format, [real, user, sys]) {
                Ok(text) => eprintln!("{text}"),
                Err(bad) => eprintln!(
                    "{}TIMEFORMAT: `{bad}': invalid format character",
                    crate::env::origin_now()
                ),
            },
            None if drawn_by_config(real, user, sys) => {}
            None => write_report(real, user, sys),
        }
        // `on-time-report`: only a `time`-prefixed pipeline reaches here, which is what separates
        // this from `post-cmd` — that one fires for everything and carries wall-clock only. These
        // are the three clocks, and they were asked for.
        oslo_base::hooks::fire_at_here(
            oslo_base::hooks::at::TIME_REPORT,
            &[
                ("real_ms", &real.as_millis().to_string()),
                ("user_ms", &user.as_millis().to_string()),
                ("sys_ms", &sys.as_millis().to_string()),
            ],
        );
    }
}

/// Emit bash's shape: a blank line, then one tab-separated line per clock.
///
/// One `eprint!` rather than four, so the block cannot be split by another writer — a background
/// job sharing this stderr is exactly the case where a half-printed report is unreadable.
fn write_report(real: Duration, user: Duration, sys: Duration) {
    eprint!(
        "\nreal\t{}\nuser\t{}\nsys\t{}\n",
        format_elapsed(real),
        format_elapsed(user),
        format_elapsed(sys)
    );
}

/// bash's default `TIMEFORMAT` field: whole minutes, then seconds to the millisecond.
///
/// Computed in integer milliseconds rather than through `f64` seconds so that a value just under a
/// minute cannot round up into the nonsense `0m60.000s`.
fn format_elapsed(d: Duration) -> String {
    let millis = d.as_millis();
    let minutes = millis / 60_000;
    let rest = millis % 60_000;
    format!("{}m{}.{:03}s", minutes, rest / 1_000, rest % 1_000)
}

/// The pipeline `time -p …` times, with the `-p` taken off its first command; `None` without one.
pub(super) fn without_posix_flag(pipeline: &Pipeline) -> Option<Pipeline> {
    let Some(Command::Simple(first)) = pipeline.commands.first() else {
        return None;
    };
    let flag = first.words.first()?;
    if !first.assignments.is_empty() || *flag != Word::from_literal("-p") {
        return None;
    }
    let mut stripped = pipeline.clone();
    if let Some(Command::Simple(simple)) = stripped.commands.first_mut() {
        simple.words.remove(0);
        if simple.words.is_empty() && simple.redirections.is_empty() {
            stripped.commands.remove(0);
        }
    }
    Some(stripped)
}

/// The report `time -p` writes, whatever `TIMEFORMAT` says.
pub(super) const POSIX_FORMAT: &str = "real %2R\nuser %2U\nsys %2S";

/// Expand a `TIMEFORMAT`: `%%`, and `%[p][l]R`, `U`, `S` for the real, user and system time with
/// `p` fractional digits (at most 3, 3 by default) and `l` for the `MMmSS.FFFs` form; `%P` is the
/// CPU percentage. Any other character after a `%` is refused, and answered back.
fn render(format: &str, [real, user, sys]: [Duration; 3]) -> Result<String, char> {
    let mut out = String::new();
    let mut chars = format.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let digits = chars
            .next_if(char::is_ascii_digit)
            .and_then(|d| d.to_digit(10))
            .map_or(3, |d| d.min(3));
        let long = chars.next_if_eq(&'l').is_some();
        match chars.next() {
            Some('%') => out.push('%'),
            Some('R') => out.push_str(&seconds(real, digits, long)),
            Some('U') => out.push_str(&seconds(user, digits, long)),
            Some('S') => out.push_str(&seconds(sys, digits, long)),
            Some('P') => {
                let busy = (user + sys).as_secs_f64();
                let share = if real.is_zero() { 0.0 } else { busy * 100.0 / real.as_secs_f64() };
                out.push_str(&format!("{share:.2}"));
            }
            Some(other) => return Err(other),
            None => out.push('%'),
        }
    }
    Ok(out)
}

/// One clock with `digits` fractional digits, truncated, as `S.FFF` or `MmS.FFFs`.
fn seconds(d: Duration, digits: u32, long: bool) -> String {
    let millis = d.as_millis();
    let (minutes, secs) = if long {
        (Some(millis / 60_000), (millis % 60_000) / 1_000)
    } else {
        (None, millis / 1_000)
    };
    let fraction = (millis % 1_000) / 10u128.pow(3 - digits);
    let mut text = match minutes {
        Some(m) => format!("{m}m{secs}"),
        None => secs.to_string(),
    };
    if digits > 0 {
        text.push_str(&format!(".{fraction:0width$}", width = digits as usize));
    }
    if long {
        text.push('s');
    }
    text
}

/// Whether an `on-report` handler drew the three clocks instead.
fn drawn_by_config(
    real: std::time::Duration,
    user: std::time::Duration,
    sys: std::time::Duration,
) -> bool {
    use oslo_ui::report::{self, int};
    if !report::watched() {
        return false;
    }
    report::handled(
        "time",
        vec![
            ("real_ms", int(real.as_millis() as i64)),
            ("user_ms", int(user.as_millis() as i64)),
            ("sys_ms", int(sys.as_millis() as i64)),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::{format_elapsed, timeval_to_duration};
    use std::time::Duration;

    /// The shape bash prints: minutes are never zero-padded, seconds always are.
    #[test]
    fn elapsed_is_formatted_as_minutes_and_millis() {
        assert_eq!(format_elapsed(Duration::ZERO), "0m0.000s");
        assert_eq!(format_elapsed(Duration::from_millis(7)), "0m0.007s");
        assert_eq!(format_elapsed(Duration::from_millis(1234)), "0m1.234s");
        assert_eq!(format_elapsed(Duration::from_secs(59)), "0m59.000s");
        assert_eq!(format_elapsed(Duration::from_secs(60)), "1m0.000s");
        assert_eq!(
            format_elapsed(Duration::from_millis(3_661_500)),
            "61m1.500s"
        );
    }

    /// Sub-millisecond time truncates to `0m0.000s`; it must not borrow into the minutes field.
    #[test]
    fn sub_millisecond_time_is_zero_not_negative() {
        assert_eq!(format_elapsed(Duration::from_micros(999)), "0m0.000s");
    }

    /// A negative `timeval` is nonsense the kernel should never produce, but `Duration::new` would
    /// panic on the cast, so it is clamped instead of trusted.
    #[test]
    fn negative_timeval_clamps_to_zero() {
        let tv = nix::libc::timeval {
            tv_sec: -5,
            tv_usec: -1,
        };
        assert_eq!(timeval_to_duration(tv), Duration::ZERO);
    }

    /// Microseconds are scaled, not copied: 1_500 µs is 1.5 ms, not 1.5 s.
    #[test]
    fn timeval_microseconds_are_scaled() {
        let tv = nix::libc::timeval {
            tv_sec: 2,
            tv_usec: 1_500,
        };
        assert_eq!(timeval_to_duration(tv), Duration::from_micros(2_001_500));
    }
}
