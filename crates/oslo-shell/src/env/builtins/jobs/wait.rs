//! `wait` — block on the shell's children and report what happened to them.
//!
//! The status is the whole point. `sh -c 'exit 7' & wait $!` is how a script fans work out and
//! collects the results, and the `wait` this replaces discarded every `WaitStatus` it got and
//! answered 0 — turning each one of those into a silent success. Every path here ends in a real
//! number instead: the child's exit code, `128 + signo` if a signal killed it, 127 for a pid or
//! job the shell has no record of.
//!
//! Two sources have to be consulted, in this order:
//!
//! 1. the job table's remembered statuses ([`JobTable::take_status`]). The shell reaps background
//!    children opportunistically at every command boundary, and once `waitpid` has returned for a
//!    pid the kernel has no memory of it. That status only exists in the table.
//! 2. `waitpid`, for a child that is still outstanding.
//!
//! Getting the order wrong is how `wait $!` regresses to "not a child of this shell" the moment
//! the shell starts reaping eagerly — which it now does.
//!
//! # A status is reported once
//!
//! Every form of `wait` consumes what it reports, so a second `wait` for the same child is 127.
//! That is the POSIX rule and what `bash --posix` does; *default* bash keeps the status and
//! answers the same number again. oslo follows POSIX, which is also the corpus oracle — see
//! [`JobTable::take_status`].

use nix::errno::Errno;
use nix::sys::wait::{WaitStatus, waitpid};
use nix::unistd::Pid;

use crate::env::scope::Environment;
use crate::exec::job::{JobState, with_jobs};
use oslo_base::error::Result;

/// Status for a pid or job the shell cannot account for. POSIX fixes this number.
const NO_SUCH_CHILD: i32 = 127;

/// A trapped signal arrived, and the wait is over whatever the child is doing.
///
/// **POSIX: `wait` is interrupted by a signal the shell has a trap for.** The trap runs and `wait`
/// returns above 128; it does not go back to sleep. Every loop here retried on `EINTR` instead, so
/// `trap 'echo caught' USR1; sleep 5 & wait $!` sat out the full five seconds and answered 0 —
/// bash and dash both come back in the moment the signal lands, with 138.
///
/// `EINTR` on its own is not this. The shell's own SIGINT handler and the reaper both interrupt a
/// `waitpid` without any trap being involved, and abandoning the wait for those would turn a
/// stray reap into a child whose status is never collected — hence the check for a *pending trap*
/// rather than for the errno alone.
struct Interrupted(i32);

/// What a `waitpid` returning `EINTR` means: a trap to run, or nothing worth stopping for.
fn a_trap_cut_it_short() -> Option<Interrupted> {
    crate::env::builtins::pending_signal().map(Interrupted)
}

/// The status a `wait` ended by a trapped signal reports.
fn interrupted_status(env: &mut Environment, Interrupted(signum): Interrupted) -> Result<i32> {
    // The body first, then the status — the order bash prints them in, and the reason the trap is
    // worth setting at all.
    crate::env::builtins::run_pending_traps(env)?;
    Ok(128 + signum)
}

/// `wait [-fn] [-p var] [id …]`.
pub fn builtin_wait(env: &mut Environment, args: &[String]) -> Result<i32> {
    let opts = match Options::parse(&args[1..]) {
        Ok(opts) => opts,
        Err(bad) => {
            crate::env::complain_with_usage(
                args,
                &bad,
                &format!("wait: {bad}: invalid option"),
                "not an option here",
                "wait: usage: wait [-fn] [-p var] [id ...]",
            );
            return Ok(2);
        }
    };

    if opts.ids.is_empty() {
        if opts.next_only {
            return match wait_any() {
                Ok(outcome) => Ok(report(env, &opts, outcome)),
                Err(cut) => interrupted_status(env, cut),
            };
        }
        // Bare `wait` has no status to report — POSIX makes it 0 whatever the children did.
        return match wait_for_everything() {
            Ok(()) => Ok(0),
            Err(cut) => interrupted_status(env, cut),
        };
    }

    if opts.next_only {
        // **A target that has already finished is the first to finish.** Its status was collected
        // by the opportunistic reaper and is being held for exactly this question; going to the
        // kernel for a pid it has already forgotten answers `ECHILD`, which became 127 — while a
        // plain `wait` on the same child answered correctly. The two must not disagree.
        let (targets, finished) = collect_targets(&opts.ids);
        if let Some(done) = finished {
            return Ok(report(env, &opts, Some(done)));
        }
        return match wait_first_of(&targets) {
            Ok(outcome) => Ok(report(env, &opts, outcome)),
            Err(cut) => interrupted_status(env, cut),
        };
    }

    // Several operands: bash waits for each in turn and reports the *last* one, so a bad operand
    // in the middle contributes its own status without aborting the rest —
    // `wait $! abc` is 1 and `wait abc $!` is the child's status.
    let mut status = 0;
    for id in &opts.ids {
        status = match resolve(id) {
            Ok(target) => match settle(target) {
                Ok(outcome) => report(env, &opts, outcome),
                Err(cut) => return interrupted_status(env, cut),
            },
            Err(status) => status,
        };
    }
    Ok(status)
}

/// The parsed command line.
struct Options {
    /// `-n`: return as soon as one job finishes, rather than waiting for all of them.
    next_only: bool,
    /// `-p var`: the variable that receives the pid whose status is being returned.
    pid_var: Option<String>,
    ids: Vec<String>,
}

impl Options {
    /// `Err(word)` names the offending option.
    fn parse(args: &[String]) -> std::result::Result<Self, String> {
        let mut opts = Options {
            next_only: false,
            pid_var: None,
            ids: Vec::new(),
        };
        let mut it = args.iter().peekable();
        while let Some(arg) = it.peek() {
            // A lone `-` and anything not starting with `-` are operands. So is `%-`: a job spec
            // read as an option cluster becomes a usage error, which is how naive getopt loops
            // eat `%-`.
            if arg.len() < 2 || !arg.starts_with('-') || arg.starts_with('%') {
                break;
            }
            let arg = it.next().expect("peeked").clone();
            if arg == "--" {
                break;
            }
            let mut chars = arg[1..].chars();
            while let Some(c) = chars.next() {
                match c {
                    'n' => opts.next_only = true,
                    // `-f` asks to wait for termination rather than for any status change.
                    // Nothing here waits with `WUNTRACED`, so that is already the only behaviour
                    // and accepting the flag is honest rather than a stub.
                    'f' => {}
                    'p' => {
                        let rest: String = chars.by_ref().collect();
                        opts.pid_var = match rest.is_empty() {
                            true => Some(it.next().cloned().unwrap_or_default()),
                            false => Some(rest),
                        };
                    }
                    other => return Err(format!("-{}", other)),
                }
            }
        }
        opts.ids.extend(it.cloned());
        Ok(opts)
    }
}

/// What one operand asks the shell to wait for.
enum Target {
    /// Processes still outstanding; the last one decides the status, as in a pipeline.
    Live(Vec<Pid>),
    /// A job that has already ended and whose status the table still remembers.
    Done(Pid, i32),
}

/// Publish `-p var` and collapse the outcome to a status, with "no such child" as 127.
fn report(env: &mut Environment, opts: &Options, outcome: Option<(Pid, i32)>) -> i32 {
    if let Some(name) = &opts.pid_var
        && !name.is_empty()
    {
        let value = outcome.map_or(String::new(), |(pid, _)| pid.as_raw().to_string());
        env.set_var(name, &value, false);
    }
    outcome.map_or(NO_SUCH_CHILD, |(_, status)| status)
}

/// Turn one operand into something to wait for, or into the status to report for it.
///
/// The two failure statuses are different on purpose, and bash distinguishes them: an operand
/// that is not a pid or a job spec at all is a usage error (1), while a well-formed reference to
/// a job the shell does not have is "no such child" (127), the same as an unknown pid.
fn resolve(id: &str) -> std::result::Result<Target, i32> {
    if id.starts_with('%') {
        return match resolve_job(id) {
            Some(target) => Ok(target),
            None => {
                crate::env::complain(
                    &[String::from("wait"), id.to_string()],
                    id,
                    &format!("wait: {id}: no such job"),
                    "no job by that name",
                    Some("`jobs` lists them; %1 is by number, %% the current one, %name by prefix"),
                );
                Err(NO_SUCH_CHILD)
            }
        };
    }
    match id.parse::<i32>() {
        Ok(n) if n > 0 => Ok(resolve_pid(Pid::from_raw(n))),
        _ => {
            crate::env::complain(
                &[String::from("wait"), id.to_string()],
                id,
                &format!("wait: `{id}': not a pid or valid job spec"),
                "neither a pid nor a job",
                Some("a pid is a number; a job is %1, %%, %+, %- or %name"),
            );
            Err(1)
        }
    }
}

/// `%n`, `%%`, `%+`, `%-`, `%prefix`, `%?substring` — resolved against the shell's job table.
fn resolve_job(spec: &str) -> Option<Target> {
    with_jobs(|jobs| {
        let id = jobs.lookup(spec)?;
        let job = jobs.get(id)?;
        // A job the reaper already finished is reported from the table and then forgotten, so a
        // second `wait %1` says "no such job" the way `bash --posix`'s does.
        if let JobState::Completed(status) = job.state {
            let pgid = job.pgid;
            jobs.remove(id);
            return Some(Target::Done(pgid, status));
        }
        Some(Target::Live(job.pids.clone()))
    })
}

/// A pid operand: the table first, in case the opportunistic reaper already collected it.
fn resolve_pid(pid: Pid) -> Target {
    match with_jobs(|jobs| jobs.take_status(pid)) {
        Some(status) => {
            // Symmetric with `resolve_job`, which removes the entry it reports. Without this the
            // finished job sits in the table and a later bare `wait -n` hands the same child back
            // — so a fan-in loop over `wait -n` returns the first job forever instead of draining.
            note_reaped(pid);
            Target::Done(pid, status)
        }
        None => Target::Live(vec![pid]),
    }
}

/// Block for whatever the operand named.
fn settle(target: Target) -> std::result::Result<Option<(Pid, i32)>, Interrupted> {
    match target {
        Target::Done(pid, status) => Ok(Some((pid, status))),
        Target::Live(pids) => wait_for_all_of(&pids),
    }
}

/// Every pid the `-n` form may return, and the first operand that has already finished.
///
/// **Both halves, because `-n` means "the first to finish" and one of them already has.** Resolving
/// consumes the remembered status — [`resolve_pid`] takes it out of the table — so a caller that
/// discarded it would have destroyed the only remaining answer for that child.
///
/// The pids of later operands are still collected: a finished one is returned here and the rest are
/// what the wait falls back to when there is none.
fn collect_targets(ids: &[String]) -> (Vec<Pid>, Option<(Pid, i32)>) {
    let mut pids = Vec::new();
    let mut finished = None;
    for id in ids {
        match resolve(id) {
            Ok(Target::Live(live)) => pids.extend(live),
            Ok(Target::Done(pid, status)) => {
                pids.push(pid);
                finished.get_or_insert((pid, status));
            }
            Err(_) => {}
        }
    }
    (pids, finished)
}

/// Wait for one specific child.
///
/// `None` means neither the table nor the kernel has anything to say about it — the caller
/// reports 127 and, for a pid operand, says so on stderr.
fn wait_for_pid(pid: Pid) -> std::result::Result<Option<i32>, Interrupted> {
    if let Some(status) = with_jobs(|jobs| jobs.take_status(pid)) {
        note_reaped(pid);
        return Ok(Some(status));
    }
    loop {
        match waitpid(pid, None) {
            Ok(WaitStatus::StillAlive) => return Ok(None),
            Ok(status) => {
                if let Some(code) = exit_status(status) {
                    note_reaped(pid);
                    return Ok(Some(code));
                }
                // A stop or a continue is not a termination; keep waiting for the real end.
            }
            Err(Errno::EINTR) => match a_trap_cut_it_short() {
                Some(cut) => return Err(cut),
                // Says nothing about how the child ended.
                None => continue,
            },
            Err(_) => return Ok(None),
        }
    }
}

/// Wait for every pid of a job; the last one decides, as in a pipeline.
fn wait_for_all_of(pids: &[Pid]) -> std::result::Result<Option<(Pid, i32)>, Interrupted> {
    let mut last = None;
    for pid in pids {
        match wait_for_pid(*pid)? {
            Some(status) => last = Some((*pid, status)),
            None if pids.len() == 1 => eprintln!(
                "oslo: wait: pid {} is not a child of this shell",
                pid.as_raw()
            ),
            None => {}
        }
    }
    Ok(last)
}

/// Bare `wait`: block until nothing the shell started is outstanding.
///
/// `waitpid(-1)` rather than a walk of the job table, because a shell has children the table
/// never knew about — anything forked before job control was set up — and POSIX says bare `wait`
/// waits for all of them.
fn wait_for_everything() -> std::result::Result<(), Interrupted> {
    loop {
        match waitpid(Pid::from_raw(-1), None) {
            Ok(WaitStatus::StillAlive) => break,
            Ok(status) => {
                if let (Some(pid), Some(_)) = (status.pid(), exit_status(status)) {
                    note_reaped(pid);
                }
            }
            Err(Errno::EINTR) => match a_trap_cut_it_short() {
                Some(cut) => return Err(cut),
                None => continue,
            },
            // ECHILD: nothing left to wait for, which is the successful end of a bare `wait`.
            Err(_) => break,
        }
    }
    Ok(())
}

/// `wait -n` with no operands: the next child to finish, or one that already has.
fn wait_any() -> std::result::Result<Option<(Pid, i32)>, Interrupted> {
    if let Some(found) = take_finished_job() {
        return Ok(Some(found));
    }
    wait_first_of(&[])
}

/// `wait -n id …`: the first of `targets` to finish. An empty `targets` accepts any child.
///
/// Children reaped along the way are folded into the table rather than thrown away, so a later
/// `wait` for one of them still has an answer.
fn wait_first_of(targets: &[Pid]) -> std::result::Result<Option<(Pid, i32)>, Interrupted> {
    for pid in targets {
        if let Some(status) = with_jobs(|jobs| jobs.take_status(*pid)) {
            note_reaped(*pid);
            return Ok(Some((*pid, status)));
        }
    }
    loop {
        match waitpid(Pid::from_raw(-1), None) {
            Ok(WaitStatus::StillAlive) => return Ok(None),
            Ok(status) => {
                let (Some(pid), Some(code)) = (status.pid(), exit_status(status)) else {
                    continue;
                };
                if targets.is_empty() || targets.contains(&pid) {
                    note_reaped(pid);
                    return Ok(Some((pid, code)));
                }
                // **Somebody else's child, and its status is not ours to throw away.** `waitpid(-1)`
                // collects whichever ends first; the kernel will never offer this one again, so a
                // later `wait $that` has nowhere but the table to find it. Dropping it here is what
                // made that wait answer 127 and "not a child of this shell".
                let signal = match status {
                    WaitStatus::Signaled(_, sig, _) => Some(sig as i32),
                    _ => None,
                };
                with_jobs(|jobs| jobs.keep_status(pid, code, signal));
            }
            Err(Errno::EINTR) => match a_trap_cut_it_short() {
                Some(cut) => return Err(cut),
                None => continue,
            },
            Err(_) => return Ok(None),
        }
    }
}

/// The oldest job the reaper has already finished, removed from the table as it is reported.
fn take_finished_job() -> Option<(Pid, i32)> {
    with_jobs(|jobs| {
        let (id, pgid, status) = jobs.jobs().iter().find_map(|job| match job.state {
            JobState::Completed(status) => Some((job.id, job.pgid, status)),
            _ => None,
        })?;
        jobs.remove(id);
        Some((pgid, status))
    })
}

/// Tell the job table that `wait` has accounted for `pid`, so nothing reports it twice.
///
/// A job whose last process this was leaves the table entirely: bash prints no `Done` notice for
/// a job a script explicitly waited for, `jobs` afterwards shows nothing, and `wait -n` does not
/// offer it again. Reaching the entry relies on [`Job::ended`] — by this point the reaper may
/// already have emptied the job's `pids`.
fn note_reaped(pid: Pid) {
    with_jobs(|jobs| {
        let Some(id) = jobs.job_of_pid(pid) else {
            return;
        };
        let Some(job) = jobs.get_mut(id) else { return };
        job.pids.retain(|p| *p != pid);
        if job.pids.is_empty() {
            jobs.remove(id);
        }
    });
}

/// The exit status a shell reports for a wait status, or `None` if the child has not ended.
///
/// `128 + signo` for a signal death is the convention every shell shares, and the reason
/// `wait $!` on a `kill -TERM`ed child answers 143.
pub fn exit_status(status: WaitStatus) -> Option<i32> {
    match status {
        WaitStatus::Exited(_, code) => Some(code),
        WaitStatus::Signaled(_, sig, _) => Some(128 + sig as i32),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nix::sys::signal::Signal;

    #[test]
    fn a_signal_death_is_reported_as_128_plus_the_signal() {
        let pid = Pid::from_raw(1);
        assert_eq!(exit_status(WaitStatus::Exited(pid, 7)), Some(7));
        assert_eq!(
            exit_status(WaitStatus::Signaled(pid, Signal::SIGTERM, false)),
            Some(143)
        );
        assert_eq!(
            exit_status(WaitStatus::Signaled(pid, Signal::SIGKILL, false)),
            Some(137)
        );
        assert_eq!(exit_status(WaitStatus::StillAlive), None);
        assert_eq!(
            exit_status(WaitStatus::Stopped(pid, Signal::SIGTSTP)),
            None,
            "a stopped child has not finished and has no exit status yet"
        );
    }

    fn opts(args: &[&str]) -> Options {
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        Options::parse(&owned).expect("options parse")
    }

    #[test]
    fn options_and_operands_are_told_apart() {
        let o = opts(&["-n"]);
        assert!(o.next_only && o.ids.is_empty());

        let o = opts(&["-f", "123"]);
        assert!(!o.next_only);
        assert_eq!(o.ids, vec!["123".to_string()]);

        let o = opts(&["-p", "v", "%1"]);
        assert_eq!(o.pid_var.as_deref(), Some("v"));
        assert_eq!(o.ids, vec!["%1".to_string()]);

        let o = opts(&["-pv", "9"]);
        assert_eq!(o.pid_var.as_deref(), Some("v"));
        assert_eq!(o.ids, vec!["9".to_string()]);
    }

    /// `%-` names the previous job. Read as an option cluster it is a `-` option and a usage
    /// error, which is exactly how job specs get eaten by a naive getopt loop.
    #[test]
    fn a_job_spec_is_never_mistaken_for_an_option() {
        assert_eq!(opts(&["%-"]).ids, vec!["%-".to_string()]);
        assert_eq!(opts(&["%1"]).ids, vec!["%1".to_string()]);
        assert_eq!(opts(&["--", "%2"]).ids, vec!["%2".to_string()]);
    }

    #[test]
    fn an_unknown_option_is_named_back_to_the_caller() {
        let owned = vec!["-x".to_string()];
        assert_eq!(Options::parse(&owned).err().as_deref(), Some("-x"));
    }

    /// An operand that is neither a pid nor a job spec is a usage error (1); a well-formed
    /// reference to a job that does not exist is "no such child" (127).
    #[test]
    fn a_malformed_operand_and_a_missing_job_report_different_statuses() {
        assert!(matches!(resolve("abc"), Err(1)));
        assert!(matches!(resolve("-3"), Err(1)));
        assert!(matches!(resolve("%no-such-job-xyz"), Err(NO_SUCH_CHILD)));
        assert!(matches!(resolve("1234"), Ok(Target::Live(_))));
    }

    /// A child that is not ours at all: the kernel says `ECHILD`, and that is 127, not 0.
    ///
    /// The end-to-end cases — a real child's exit code, `128 + signo`, `-n`, `%n` — are in the
    /// differential corpus (`job_wait_*.sh`) rather than here on purpose. Forking from a libtest
    /// thread puts a child into a process where other tests are also reaping, and a unit test
    /// that races the rest of the binary is worse than no unit test.
    #[test]
    fn a_pid_that_is_not_a_child_has_no_status() {
        // pid 1 is init: it exists, so this is `ECHILD` rather than `ESRCH`.
        assert!(matches!(wait_for_pid(Pid::from_raw(1)), Ok(None)));
    }
}
