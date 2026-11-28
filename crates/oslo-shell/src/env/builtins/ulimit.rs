//! `ulimit` — report and change the resource limits the shell is running under.
//!
//! Both directions go through `getrlimit(2)`/`setrlimit(2)`, reached via `nix`'s `resource`
//! feature. Reading used to parse `/proc/self/limits` by hand instead — the same numbers, at the
//! cost of a text format to keep in step with the kernel.
//!
//! Values are converted into the units every shell's `ulimit` reports in: 512-byte blocks for file
//! sizes, kilobytes for memory sizes, plain counts for everything else.
//!
//! A limit that cannot be applied is reported rather than passed over in silence: a script told it
//! has headroom it does not have will happily open the files it cannot open, which is why this
//! builtin refused the set direction outright until it could implement it.

use crate::env::origin_now;
use crate::env::scope::Environment;
use nix::sys::resource::{RLIM_INFINITY, Resource, getrlimit, rlim_t, setrlimit};
use oslo_base::error::Result;

/// One selectable limit: its option letter, its description and unit, and the divisor that turns
/// the kernel's units into the unit `ulimit` prints.
struct Limit {
    flag: char,
    label: &'static str,
    unit: Option<&'static str>,
    /// 1 for a plain count or a time in seconds, 512 for a size in blocks, 1024 for a size in kB.
    divisor: u64,
}

/// One row per limit, in the order `ulimit -a` lists them on Linux.
#[rustfmt::skip]
const LIMITS: &[Limit] = &[
    Limit { flag: 'R', label: "real-time non-blocking time", unit: Some("microseconds"), divisor: 1 },
    Limit { flag: 'c', label: "core file size", unit: Some("blocks"), divisor: 512 },
    Limit { flag: 'd', label: "data seg size", unit: Some("kbytes"), divisor: 1024 },
    Limit { flag: 'e', label: "scheduling priority", unit: None, divisor: 1 },
    Limit { flag: 'f', label: "file size", unit: Some("blocks"), divisor: 512 },
    Limit { flag: 'i', label: "pending signals", unit: None, divisor: 1 },
    Limit { flag: 'l', label: "max locked memory", unit: Some("kbytes"), divisor: 1024 },
    Limit { flag: 'm', label: "max memory size", unit: Some("kbytes"), divisor: 1024 },
    Limit { flag: 'n', label: "open files", unit: None, divisor: 1 },
    Limit { flag: 'p', label: "pipe size", unit: Some("512 bytes"), divisor: 512 },
    Limit { flag: 'q', label: "POSIX message queues", unit: Some("bytes"), divisor: 1 },
    Limit { flag: 'r', label: "real-time priority", unit: None, divisor: 1 },
    Limit { flag: 's', label: "stack size", unit: Some("kbytes"), divisor: 1024 },
    Limit { flag: 't', label: "cpu time", unit: Some("seconds"), divisor: 1 },
    Limit { flag: 'u', label: "max user processes", unit: None, divisor: 1 },
    Limit { flag: 'v', label: "virtual memory", unit: Some("kbytes"), divisor: 1024 },
    Limit { flag: 'x', label: "file locks", unit: None, divisor: 1 },
];

const USAGE: &str = "ulimit: usage: ulimit [-SHabcdefiklmnpqrstuvxPRT] [limit]";

/// The pipe buffer `-p` reports, which is not a resource limit and cannot be changed.
const PIPE_SIZE: u64 = 4096;

/// `value`, with the description and unit in front when several limits are printed.
fn line(limit: &Limit, value: &str, described: bool) -> String {
    if !described {
        return value.to_string();
    }
    let unit = match limit.unit {
        Some(unit) => format!("({unit}, -{}) ", limit.flag),
        None => format!("(-{}) ", limit.flag),
    };
    format!("{:<20} {:>20}{value}", limit.label, unit)
}

/// Which of the two limits a `-H`/`-S` run selected.
///
/// Three states, not a `bool`: reporting needs to know *which* limit to print, and setting needs
/// to know that neither flag was given, because a bare `ulimit -n 100` moves the hard limit down
/// with the soft one and `ulimit -S -n 100` does not.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Which {
    Soft,
    Hard,
    Both,
}

impl Which {
    /// Whether a report should print the hard limit. `Both` cannot occur while reporting — no
    /// flag means the soft limit, which is the one a process is actually running under.
    fn reports_hard(self) -> bool {
        self == Which::Hard
    }
}

/// `ulimit [-SHa] [-RcdefilmnpqrstuvxR [limit]] … [limit]`.
///
/// Each limit letter takes an optional value: the rest of its word, or the next word when that
/// does not start with `-`. A letter with no value is printed, one with a value is set.
pub fn builtin_ulimit(_env: &mut Environment, args: &[String]) -> Result<i32> {
    let (mut soft, mut hard) = (false, false);
    let mut all = false;
    let mut selected: Vec<(char, Option<String>)> = Vec::new();

    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            i += 1;
            break;
        }
        if arg.len() < 2 || !arg.starts_with('-') {
            break;
        }
        for (at, c) in arg[1..].char_indices() {
            match c {
                'H' => hard = true,
                'S' => soft = true,
                'a' => all = true,
                c if LIMITS.iter().any(|l| l.flag == c) => {
                    let rest = &arg[1 + at + c.len_utf8()..];
                    let value = match rest.is_empty() {
                        false => Some(rest.to_string()),
                        true => match args.get(i + 1) {
                            Some(next) if !next.starts_with('-') => {
                                i += 1;
                                Some(next.clone())
                            }
                            _ => None,
                        },
                    };
                    selected.push((c, value));
                    break;
                }
                other => {
                    crate::env::complain_option(
                        args,
                        other,
                        &format!("ulimit: -{other}: invalid option"),
                        USAGE,
                    );
                    return Ok(2);
                }
            }
        }
        i += 1;
    }
    let which = match (soft, hard) {
        (true, false) => Which::Soft,
        (false, true) => Which::Hard,
        _ => Which::Both,
    };
    // Printing reads the soft limit unless only `-H` was given.
    let report_hard = which.reports_hard();

    if all {
        for limit in LIMITS {
            if let Some(value) = report(limit, report_hard) {
                println!("{}", line(limit, &value, true));
            }
        }
        return Ok(0);
    }

    // With no limit selected, `ulimit` means the file-size limit, as POSIX specifies — for
    // setting as much as for reporting.
    if selected.is_empty() {
        selected.push(('f', args.get(i).cloned()));
    }

    let described = selected.len() > 1;
    let mut status = 0;
    for (flag, value) in selected {
        let limit = LIMITS
            .iter()
            .find(|l| l.flag == flag)
            .expect("validated above");
        if let Some(value) = value {
            status |= set_one(flag, &value, which);
            continue;
        }
        match report(limit, report_hard) {
            Some(value) => println!("{}", line(limit, &value, described)),
            None => {
                eprintln!("{}ulimit: {}: cannot get limit", origin_now(), limit.label);
                status = 1;
            }
        }
    }
    Ok(status)
}

/// The `getrlimit`/`setrlimit` resource a flag selects.
///
/// Every flag in [`LIMITS`] maps, so an unknown one is a bug in that table rather than a property
/// of the machine — the option parser has already rejected anything not in it.
#[rustfmt::skip]
fn resource_for(flag: char) -> Resource {
    match flag {
        'R' => Resource::RLIMIT_RTTIME,
        'c' => Resource::RLIMIT_CORE,
        'd' => Resource::RLIMIT_DATA,
        'e' => Resource::RLIMIT_NICE,
        'f' => Resource::RLIMIT_FSIZE,
        'i' => Resource::RLIMIT_SIGPENDING,
        'l' => Resource::RLIMIT_MEMLOCK,
        'm' => Resource::RLIMIT_RSS,
        'n' => Resource::RLIMIT_NOFILE,
        'q' => Resource::RLIMIT_MSGQUEUE,
        'r' => Resource::RLIMIT_RTPRIO,
        's' => Resource::RLIMIT_STACK,
        't' => Resource::RLIMIT_CPU,
        'u' => Resource::RLIMIT_NPROC,
        'v' => Resource::RLIMIT_AS,
        'x' => Resource::RLIMIT_LOCKS,
        other => unreachable!("ulimit -{other} is not in the LIMITS table"),
    }
}

/// What a `ulimit` operand means, in the units the flag is reported in.
///
/// `hard` and `soft` are not numbers and must not be parsed as any: they name whichever value
/// the limit currently holds, which is how `ulimit -n hard` raises the soft limit as far as it
/// is allowed to go.
enum Requested {
    Value(rlim_t),
    Unlimited,
    Hard,
    Soft,
}

fn parse_operand(operand: &str, divisor: u64) -> Option<Requested> {
    match operand {
        "unlimited" => Some(Requested::Unlimited),
        "hard" => Some(Requested::Hard),
        "soft" => Some(Requested::Soft),
        // Multiplied into bytes, since the flag reports blocks or kilobytes. A value so large
        // that the multiplication would wrap is not a limit any kernel can hold.
        n => n
            .parse::<u64>()
            .ok()?
            .checked_mul(divisor)
            .map(|v| Requested::Value(v as rlim_t)),
    }
}

/// Apply one `ulimit -FLAG value`. Returns the exit status, having reported any failure.
fn set_one(flag: char, operand: &str, which: Which) -> i32 {
    let limit = LIMITS
        .iter()
        .find(|l| l.flag == flag)
        .expect("the option run only accepts flags from the table");
    if flag == 'p' {
        if parse_operand(operand, limit.divisor).is_none() {
            let words = [
                String::from("ulimit"),
                format!("-{flag}"),
                operand.to_string(),
            ];
            let body = format!("ulimit: {operand}: invalid number");
            crate::env::complain(&words, operand, &body, "not a limit", None);
        } else {
            eprintln!(
                "{}ulimit: pipe size: cannot modify limit: Invalid argument",
                origin_now()
            );
        }
        return 1;
    }
    let Ok((soft, hard)) = getrlimit(resource_for(flag)) else {
        crate::env::complain(
            &[String::from("ulimit"), format!("-{flag}")],
            &format!("-{flag}"),
            &format!("ulimit: -{flag}: cannot read the current limit"),
            "the kernel refused it",
            Some("this limit may not exist on this kernel"),
        );
        return 1;
    };
    let Some(requested) = parse_operand(operand, limit.divisor) else {
        crate::env::complain(
            &[
                String::from("ulimit"),
                format!("-{flag}"),
                operand.to_string(),
            ],
            operand,
            &format!("ulimit: {operand}: invalid number"),
            "not a limit",
            Some("a limit is a whole number, or `unlimited`"),
        );
        return 1;
    };
    let value = match requested {
        Requested::Value(v) => v,
        // `ulimit -S -n unlimited` means "as high as I am allowed", not "past the ceiling":
        // bash raises the soft limit to the hard one and exits 0, where a literal
        // `RLIM_INFINITY` would be `EINVAL` whenever the hard limit is finite. Raising the
        // *hard* limit is a different request and is still allowed to fail.
        Requested::Unlimited if which == Which::Soft => hard,
        Requested::Unlimited => RLIM_INFINITY,
        Requested::Hard => hard,
        Requested::Soft => soft,
    };

    // Whichever half was not selected keeps the value it had. The kernel, not this code, decides
    // whether the result is legal: lowering a hard limit below the soft one is `EINVAL` and
    // raising a hard limit without privilege is `EPERM`, and both are worth reporting exactly.
    let (new_soft, new_hard) = match which {
        Which::Soft => (value, hard),
        Which::Hard => (soft, value),
        Which::Both => (value, value),
    };
    match setrlimit(resource_for(flag), new_soft, new_hard) {
        Ok(()) => 0,
        Err(errno) => {
            eprintln!(
                "{}ulimit: {}: cannot modify limit: {}",
                origin_now(),
                limit.label,
                errno.desc()
            );
            1
        }
    }
}

/// Read one limit and convert it to `ulimit`'s units, or `None` if the kernel refuses to report
/// it at all.
fn report(limit: &Limit, hard: bool) -> Option<String> {
    if limit.flag == 'p' {
        return Some(format_limit(PIPE_SIZE, limit.divisor));
    }
    let (soft_value, hard_value) = getrlimit(resource_for(limit.flag)).ok()?;
    let value = if hard { hard_value } else { soft_value };
    Some(format_limit(value, limit.divisor))
}

/// Render one raw limit in the unit its flag reports.
///
/// Split from [`report`] so the conversion can be tested without `setrlimit`: the limits belong to
/// the whole test process, and a test that moves them races every test running beside it.
fn format_limit(value: rlim_t, divisor: u64) -> String {
    if value == RLIM_INFINITY {
        return "unlimited".to_string();
    }
    // Truncating division, matching bash: a limit that is not a whole number of blocks is
    // reported as the number of whole blocks it covers. `rlim_t` is `u64` on every target in the
    // matrix, so this divides without a conversion — clippy rejects one as useless either way.
    (value / divisor).to_string()
}

#[cfg(test)]
mod tests {
    use super::{LIMITS, builtin_ulimit, format_limit, report};
    use crate::env::Environment;
    use nix::sys::resource::RLIM_INFINITY;

    fn limit_for(flag: char) -> &'static super::Limit {
        LIMITS.iter().find(|l| l.flag == flag).expect("known flag")
    }

    /// Sizes are reported in the units every shell's `ulimit` uses, not in bytes: kilobytes for
    /// memory, 512-byte blocks for files.
    #[test]
    fn sizes_are_converted_to_the_units_shells_report() {
        assert_eq!(format_limit(8_388_608, 1024), "8192");
        assert_eq!(format_limit(4096 * 512, 512), "4096");
        assert_eq!(format_limit(1024, 1), "1024");
        assert_eq!(format_limit(0, 512), "0");
        assert_eq!(format_limit(RLIM_INFINITY, 1024), "unlimited");
    }

    /// Every flag in the table has to be readable through the same path a script uses. The
    /// `/proc`-based reader this replaced answered for only some of them and said nothing about
    /// the rest.
    #[test]
    fn every_listed_limit_is_readable() {
        for limit in LIMITS {
            assert!(
                report(limit, false).is_some(),
                "-{} could not be read",
                limit.flag
            );
        }
    }

    /// A limit really is applied: lower the soft core-file limit and read it back through the
    /// same path a script would. Restored afterwards, because this is the test process's own
    /// limit and every later test inherits it.
    #[test]
    fn setting_a_limit_changes_what_the_query_reports() {
        use nix::sys::resource::{Resource, getrlimit, setrlimit};
        let (soft, hard) = getrlimit(Resource::RLIMIT_CORE).expect("core limit");

        let mut env = Environment::new();
        let args = vec!["ulimit".to_string(), "-Sc".to_string(), "0".to_string()];
        assert_eq!(builtin_ulimit(&mut env, &args).unwrap(), 0);
        assert_eq!(report(limit_for('c'), false).as_deref(), Some("0"));

        setrlimit(Resource::RLIMIT_CORE, soft, hard).expect("restore");
    }

    /// A value that is not a number is refused rather than rounded to zero, which would silently
    /// forbid what the author meant to allow.
    #[test]
    fn a_non_numeric_limit_is_refused() {
        let mut env = Environment::new();
        let args = vec!["ulimit".to_string(), "-c".to_string(), "abc".to_string()];
        assert_eq!(builtin_ulimit(&mut env, &args).unwrap(), 1);
    }

    /// `hard`, `soft` and `unlimited` name limits; everything else is a count in the flag's own
    /// unit, so a file-size operand is multiplied into bytes before it reaches the kernel.
    #[test]
    fn operands_are_named_limits_or_counts_in_the_flags_unit() {
        use super::{Requested, parse_operand};
        assert!(matches!(
            parse_operand("unlimited", 1),
            Some(Requested::Unlimited)
        ));
        assert!(matches!(parse_operand("hard", 512), Some(Requested::Hard)));
        assert!(matches!(parse_operand("soft", 512), Some(Requested::Soft)));
        assert!(matches!(
            parse_operand("2", 512),
            Some(Requested::Value(1024))
        ));
        assert!(parse_operand("abc", 1).is_none());
        assert!(parse_operand("-1", 1).is_none());
        assert!(
            parse_operand(&u64::MAX.to_string(), 1024).is_none(),
            "a value that overflows its own unit is not a limit"
        );
    }
}
