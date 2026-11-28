//! `times` — accumulated user and system time for the shell and its children.
//!
//! Two lines, `user system`: the shell's own times first, its waited-for children's second.
//!
//! The numbers come from `getrusage(2)`, which is what `times(2)` is built on. They used to come
//! from `/proc/self/stat`, parsed by hand, because `nix` only exposes the call behind a feature
//! this crate did not enable — `ulimit` turned that feature on, so the reason for the workaround
//! outlived the workaround itself.

use crate::env::scope::Environment;
use nix::sys::resource::{UsageWho, getrusage};
use nix::sys::time::TimeVal;
use oslo_base::error::Result;

/// `times` — print the shell's and its children's CPU time.
pub fn builtin_times(_env: &mut Environment, args: &[String]) -> Result<i32> {
    // `times` takes no options, so an option given to it is named; an operand is ignored, as in bash.
    if let Some(extra) = args
        .get(1)
        .filter(|arg| arg.len() > 1 && arg.starts_with('-') && *arg != "--")
    {
        crate::env::complain_with_usage(
            args,
            extra,
            &format!("times: {extra}: invalid option"),
            "times takes no options",
            "times: usage: times",
        );
        return Ok(2);
    }
    // A shell's `times` cannot fail in any other shell, so an unreadable clock prints zeroes
    // rather than aborting a script on a diagnostic.
    let t = cpu_times().unwrap_or([0.0; 4]);
    println!("{} {}", format_seconds(t[0]), format_seconds(t[1]));
    println!("{} {}", format_seconds(t[2]), format_seconds(t[3]));
    Ok(0)
}

/// `[self user, self system, children user, children system]`, in seconds.
///
/// `RUSAGE_CHILDREN` counts only children that have been *waited for*, which is precisely the set
/// POSIX says the second line reports.
fn cpu_times() -> Option<[f64; 4]> {
    let own = getrusage(UsageWho::RUSAGE_SELF).ok()?;
    let children = getrusage(UsageWho::RUSAGE_CHILDREN).ok()?;
    Some([
        seconds(own.user_time()),
        seconds(own.system_time()),
        seconds(children.user_time()),
        seconds(children.system_time()),
    ])
}

fn seconds(t: TimeVal) -> f64 {
    t.tv_sec() as f64 + t.tv_usec() as f64 / 1_000_000.0
}

/// Render a duration the way every shell's `times` does: `<minutes>m<seconds>s`.
fn format_seconds(total: f64) -> String {
    let minutes = (total / 60.0) as u64;
    format!("{}m{:.3}s", minutes, total - (minutes * 60) as f64)
}

#[cfg(test)]
mod tests {
    use super::{cpu_times, format_seconds};

    #[test]
    fn durations_render_as_minutes_and_seconds() {
        assert_eq!(format_seconds(0.0), "0m0.000s");
        assert_eq!(format_seconds(0.01), "0m0.010s");
        assert_eq!(format_seconds(60.0), "1m0.000s");
        assert_eq!(format_seconds(61.5), "1m1.500s");
    }

    /// The real clock, through the same call the builtin uses.
    #[test]
    fn the_running_process_reports_its_own_times() {
        let t = cpu_times().expect("getrusage must work on this platform");
        assert!(
            t.iter().all(|v| *v >= 0.0),
            "cpu times must not be negative: {t:?}"
        );
    }
}
