//! `kill`: signal delivery that resolves the spec before it sends anything.
//!
//! The previous implementation seeded `sig` with SIGTERM and kept that default when the name
//! failed to parse — and nix only accepts the `SIG`-prefixed spelling, so `-HUP`, `-USR1` and
//! `-stop` all quietly sent TERM, and `kill -0 $pid`, the canonical "is it still alive?" probe,
//! *terminated the process it was asked about*. The order here is deliberate: the spec is
//! resolved to a number first, and an unresolvable spec returns before a single signal is sent.

use super::signals;
use crate::env::origin_now;
use crate::env::scope::Environment;
use crate::exec::job::with_jobs;
use nix::errno::Errno;
use oslo_base::error::Result;

/// The help line under a rejected signal spec.
///
/// The three spellings `parse_spec` accepts, in the order somebody would try them. Worth saying
/// because the failure is almost always the fourth: a name that is close but not one — `KILL9`,
/// `sighup`, `-9x` — and the message alone does not say what shape a right one has.
const SIGNAL_SYNTAX: &str = "a signal is a name (TERM), a number (15), or SIG-prefixed (SIGTERM)";

/// What the argument scan produced. `spec` is unresolved on purpose — parsing it is the caller's
/// job, so that "no operands" and "bad signal" stay two distinguishable failures.
struct Invocation<'a> {
    list: bool,
    spec: Option<&'a str>,
    operands: &'a [String],
}

pub fn builtin_kill(_env: &mut Environment, args: &[String]) -> Result<i32> {
    // `kill -s` with nothing after it: bash names the option and answers 1, with no usage line.
    if let [_, flag] = args
        && matches!(flag.as_str(), "-s" | "-n")
    {
        let body = format!("kill: {flag}: option requires an argument");
        crate::env::complain(args, flag, &body, "needs a signal", None);
        return Ok(1);
    }
    let Some(inv) = parse_args(args.get(1..).unwrap_or(&[])) else {
        return Ok(usage());
    };

    if inv.list {
        return Ok(list(args, inv.operands));
    }

    // Default only when nothing was asked for. A spec that was given and did not parse is an
    // error, never a silent fallback to TERM.
    let spec = inv.spec.unwrap_or("TERM");
    let Some(signum) = signals::parse_spec(spec) else {
        crate::env::complain(
            args,
            spec,
            &format!("kill: {spec}: invalid signal specification"),
            "not a signal",
            Some(SIGNAL_SYNTAX),
        );
        return Ok(1);
    };

    if inv.operands.is_empty() {
        return Ok(usage());
    }

    // bash reports success when at least one operand was signalled, so a dead pid in a list does
    // not mask the live ones. Nothing here stops at the first failure.
    let mut any_succeeded = false;
    for operand in inv.operands {
        // A `%`-form is a job, not a number, and it was reaching the `parse` below and failing.
        // The symptom was not a diagnostic anybody saw: `kill %1` printed to stderr, exited 1, and
        // the script that had just backgrounded something went on to `wait` for a job nothing had
        // signalled — one corpus script sat there for a full second where bash took two
        // milliseconds. A shell that cannot signal its own jobs by name is not a shell.
        if operand.starts_with('%') {
            match signal_job(operand, signum) {
                Ok(()) => any_succeeded = true,
                Err(JobSignal::NoSuchJob) => {
                    crate::env::complain(
                        args,
                        operand,
                        &format!("kill: {operand}: no such job"),
                        "no job by that name",
                        Some(
                            "`jobs` lists them; %1 is by number, %% the current one, %name by prefix",
                        ),
                    );
                }
                Err(JobSignal::Failed(e)) => {
                    eprintln!("{}kill: ({operand}) - {}", origin_now(), e.desc());
                }
            }
            continue;
        }
        match operand.parse::<i32>() {
            Ok(pid) => match send(pid, signum) {
                Ok(()) => any_succeeded = true,
                Err(e) => eprintln!("{}kill: ({operand}) - {}", origin_now(), e.desc()),
            },
            Err(_) => {
                crate::env::complain(
                    args,
                    operand,
                    &format!("kill: `{operand}': not a pid or valid job spec"),
                    "neither a pid nor a job",
                    Some("a pid is a number; a job is %1, %%, %+, %- or %name"),
                );
            }
        }
    }

    Ok(if any_succeeded { 0 } else { 1 })
}

/// Split options from operands. Returns `None` when the invocation is malformed enough that the
/// only useful answer is the usage line.
fn parse_args(rest: &[String]) -> Option<Invocation<'_>> {
    let mut list = false;
    let mut spec: Option<&str> = None;
    let mut idx = 0;

    while idx < rest.len() {
        let arg = rest[idx].as_str();
        if arg == "--" {
            idx += 1;
            break;
        }
        // Once a signal is chosen, a leading `-` belongs to an operand: `kill -TERM -1234`
        // signals process group 1234.
        if spec.is_some() || !arg.starts_with('-') || arg == "-" {
            break;
        }
        match arg {
            "-l" | "-L" => {
                list = true;
                idx += 1;
            }
            "-s" | "-n" => {
                spec = Some(rest.get(idx + 1)?.as_str());
                idx += 2;
            }
            _ => {
                spec = Some(&arg[1..]);
                idx += 1;
            }
        }
    }

    let operands = &rest[idx..];
    if !list && spec.is_none() && operands.is_empty() {
        return None;
    }
    Some(Invocation {
        list,
        spec,
        operands,
    })
}

fn usage() -> i32 {
    eprintln!(
        "oslo: kill: usage: kill [-s sigspec | -n signum | -sigspec] pid | jobspec ... or kill -l [sigspec]"
    );
    2
}

/// `kill -l`: the whole table, or the other half of each spec given.
fn list(args: &[String], specs: &[String]) -> i32 {
    if specs.is_empty() {
        print!("{}", signals::table());
        return 0;
    }

    let mut status = 0;
    for spec in specs {
        match describe(spec) {
            Some(line) => println!("{line}"),
            None => {
                crate::env::complain(
                    args,
                    spec,
                    &format!("kill: {spec}: invalid signal specification"),
                    "not a signal",
                    Some(SIGNAL_SYNTAX),
                );
                status = 1;
            }
        }
    }
    status
}

/// A number becomes a name, a name becomes a number.
fn describe(spec: &str) -> Option<String> {
    let Ok(num) = spec.parse::<i32>() else {
        if spec.eq_ignore_ascii_case("EXIT") || spec.eq_ignore_ascii_case("SIGEXIT") {
            return Some("0".to_string());
        }
        return signals::number_from_name(spec).map(|n| n.to_string());
    };
    if num == 0 {
        // Shells name signal 0 EXIT here: `kill -l $?` on a trap-driven status should read back.
        return Some("EXIT".to_string());
    }
    // `kill -l $?` on a signalled child is given 128 + signo, so fall back to the wait-status
    // encoding rather than calling a perfectly meaningful status invalid.
    signals::name_from_number(num).or_else(|| signals::name_from_number(num - 128))
}

/// `libc::kill` rather than nix's wrapper: nix's `Signal` enum has no realtime signals and so
/// cannot express `kill -RTMIN+3`. Signal 0 passes straight through — that is the existence
/// probe, and the kernel delivers nothing for it.
/// Why signalling a job spec did not work.
enum JobSignal {
    /// The table has no job by that name — a different message from a failed `kill(2)`, because
    /// they are different mistakes: one is a typo, the other is a process that has gone.
    NoSuchJob,
    Failed(Errno),
}

/// Signal the job named by a `%`-spec: `%1`, `%%`, `%+`, `%-`, `%prefix`, `%?substring`.
///
/// **The whole process group, not the leader.** bash signals `-pgid`, and the difference is the
/// point of job control: `sleep 10 | cat &` is one job of two processes, and signalling only the
/// leader leaves the other running with its stdin closed. The negation is what `kill(2)` reads as
/// "every process in this group".
///
/// Resolution goes through the same `JobTable::lookup` that `wait` and `fg` use, so every spelling
/// of a job spec means the same thing to every builtin — a second parser here is how `%prefix`
/// ends up meaning one thing to `wait` and another to `kill`.
fn signal_job(spec: &str, signum: i32) -> std::result::Result<(), JobSignal> {
    let pgid = with_jobs(|jobs| {
        let id = jobs.lookup(spec)?;
        jobs.get(id).map(|job| job.pgid)
    })
    .ok_or(JobSignal::NoSuchJob)?;
    send(-pgid.as_raw(), signum).map_err(JobSignal::Failed)
}

fn send(pid: i32, signum: i32) -> std::result::Result<(), Errno> {
    // SAFETY: kill(2) takes two scalars, reads no memory owned by this process and cannot
    // invalidate any Rust invariant. Failure is reported through errno, which is what
    // `Errno::result` reads.
    let res = unsafe { nix::libc::kill(pid, signals::real(signum)) };
    Errno::result(res).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    /// The regression that named this finding: an unparseable spec must send nothing at all.
    /// Signal 0 against our own pid is the only send this suite performs, precisely because it
    /// is the one that must be inert.
    #[test]
    fn a_bad_signal_spec_sends_nothing_and_fails() {
        let mut env = Environment::new();
        let self_pid = std::process::id().to_string();
        for bad in ["-NOSUCHSIG", "-99", "-sig"] {
            assert_eq!(
                builtin_kill(&mut env, &words(&["kill", bad, &self_pid])).unwrap(),
                1,
                "{bad} should be an invalid signal specification"
            );
        }
        assert_eq!(
            builtin_kill(&mut env, &words(&["kill", "-s", "NOSUCHSIG", &self_pid])).unwrap(),
            1
        );
    }

    #[test]
    fn signal_zero_probes_a_live_process() {
        let mut env = Environment::new();
        let self_pid = std::process::id().to_string();
        assert_eq!(
            builtin_kill(&mut env, &words(&["kill", "-0", &self_pid])).unwrap(),
            0
        );
        // Still running to make the assertion, which is the whole point.
        assert_eq!(
            builtin_kill(&mut env, &words(&["kill", "-s", "0", &self_pid])).unwrap(),
            0
        );
    }

    #[test]
    fn a_non_numeric_operand_is_diagnosed() {
        let mut env = Environment::new();
        assert_eq!(
            builtin_kill(&mut env, &words(&["kill", "-0", "abc"])).unwrap(),
            1
        );
        assert_eq!(builtin_kill(&mut env, &words(&["kill", "abc"])).unwrap(), 1);
    }

    #[test]
    fn missing_operands_are_a_usage_error() {
        let mut env = Environment::new();
        assert_eq!(builtin_kill(&mut env, &words(&["kill"])).unwrap(), 2);
        assert_eq!(
            builtin_kill(&mut env, &words(&["kill", "-TERM"])).unwrap(),
            2
        );
        assert_eq!(builtin_kill(&mut env, &words(&["kill", "-s"])).unwrap(), 1);
    }

    #[test]
    fn list_translates_both_directions() {
        assert_eq!(describe("9").as_deref(), Some("KILL"));
        assert_eq!(describe("KILL").as_deref(), Some("9"));
        assert_eq!(describe("SIGKILL").as_deref(), Some("9"));
        assert_eq!(describe("0").as_deref(), Some("EXIT"));
        assert_eq!(describe("137").as_deref(), Some("KILL"));
        assert_eq!(describe("NOSUCHSIG"), None);
    }

    /// Options must be recognised where a shell puts them, and stop where operands begin.
    #[test]
    fn argument_scan_separates_options_from_pids() {
        let args = words(&["-s", "HUP", "123", "456"]);
        let inv = parse_args(&args).expect("parses");
        assert_eq!(inv.spec, Some("HUP"));
        assert_eq!(inv.operands, &args[2..]);

        // A negative operand after a signal is a process group, not another option.
        let args = words(&["-TERM", "-1234"]);
        let inv = parse_args(&args).expect("parses");
        assert_eq!(inv.spec, Some("TERM"));
        assert_eq!(inv.operands.len(), 1);

        // `--` ends the options: everything after it is an operand.
        let args = words(&["--", "-1234"]);
        let inv = parse_args(&args).expect("parses");
        assert!(inv.spec.is_none());
        assert_eq!(inv.operands.len(), 1);

        let args = words(&["-l"]);
        assert!(parse_args(&args).expect("parses").list);
    }
}
