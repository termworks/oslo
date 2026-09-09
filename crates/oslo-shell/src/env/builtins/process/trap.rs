//! The `trap` builtin: what the shell should do when a condition fires.
//!
//! Only the operand grammar lives here. Actually delivering a trap — installing the kernel
//! disposition, draining the pending set, running the EXIT handler — is [`super::handlers`].
//!
//! The grammar is small but every clause of it is load-bearing:
//!
//! * `trap ACTION cond...` installs, `trap - cond...` resets, `trap '' cond...` ignores. The
//!   reset form is the one oslo used to get wrong in the most damaging way available: `-` was
//!   stored as the *handler text*, so `trap - EXIT` installed a handler that would try to run a
//!   command called `-`.
//! * `trap N cond...`, where the first operand is an unsigned integer, resets as well. POSIX
//!   requires it so that `trap $(trap -p INT)`-style round-trips cannot be misread as installing
//!   a handler whose text happens to be a number.
//! * a condition the system does not name is an error with status 1, not a silently ignored
//!   operand. A trap that was never installed is a cleanup that will never happen.

use super::handlers::{self, DEFAULT_ACTION};
use super::signals;
use crate::env::origin_now;
use crate::env::scope::Environment;
use oslo_base::error::Result;

/// What a trap condition may be, for the help line under one that is not.
///
/// The three shapes, because the failure is almost always a fourth: `SIGINT` where `INT` was meant
/// is fine, `EXIT0` is not, and a lower-case `exit` is the commonest of all.
const TRAP_CONDITIONS: &str =
    "a condition is EXIT, ERR, DEBUG, RETURN, a signal name (INT, TERM) or a signal number";

/// A `trap` operand, once resolved.
enum Condition {
    /// `EXIT` or `0`: the shell is ending.
    Exit,
    Signal {
        number: i32,
        name: String,
    },
    /// `DEBUG`: the handler runs before each simple command, with `$BASH_COMMAND` set to what is
    /// about to run. This is the hook every shell integration is built on — starship, atuin and
    /// hexe all install a preexec through it — so it is a condition rather than a signal, fired
    /// from `exec::simple` rather than armed with the kernel.
    Debug,
    /// `ERR`: the handler runs for a command that failed, under exactly the exemptions `set -e`
    /// uses. Fired from `pipeline::run_and_record` rather than armed with the kernel.
    Err,
    /// `RETURN` — named by bash, not implemented here. Kept as a distinct case so the
    /// diagnostic can say *why* rather than claiming the name does not exist.
    Unsupported(String),
}

impl Condition {
    /// The key the trap table stores this condition under.
    /// How a listing spells this condition: `EXIT`, or a signal named the way the current mode
    /// names it.
    ///
    /// The spelling is part of the interface, not decoration — `trap` output is fed back to
    /// `eval` and grepped by save-and-restore wrappers — and bash prints it two ways: `SIGINT`
    /// normally, bare `INT` under `set -o posix`. Following only the first broke every
    /// posix-mode script, which is how the corpus caught it. `EXIT` carries no prefix either way.
    fn display_name(&self, posix: bool) -> String {
        match self {
            Condition::Exit => "EXIT".to_string(),
            Condition::Debug => "DEBUG".to_string(),
            Condition::Err => "ERR".to_string(),
            Condition::Signal { name, .. } if posix => name.to_string(),
            Condition::Signal { name, .. } => format!("SIG{name}"),
            Condition::Unsupported(name) => name.to_string(),
        }
    }

    fn key(&self) -> &str {
        match self {
            Condition::Exit => "EXIT",
            Condition::Debug => "DEBUG",
            Condition::Err => "ERR",
            Condition::Signal { name, .. } => name,
            Condition::Unsupported(name) => name,
        }
    }

    /// Sort position for a listing: EXIT first, then signals in number order.
    fn order(&self) -> i32 {
        match self {
            Condition::Exit => 0,
            // After EXIT and before every signal: it is not a signal, and a listing that sorted it
            // among them by an invented number would imply one.
            Condition::Debug => 1,
            Condition::Err => 2,
            Condition::Signal { number, .. } => *number + 2,
            Condition::Unsupported(_) => i32::MAX,
        }
    }
}

/// The usage line, printed under the diagnostic that caused it. Unprefixed, as bash leaves its
/// own: the line above already said where, and this one is a reminder rather than a report.
const USAGE: &str = "trap: usage: trap [-lp] [[action] condition ...]";

/// bash's non-POSIX conditions that oslo still does not run. Recognised so the diagnostic can be
/// truthful rather than claiming the name does not exist.
const UNSUPPORTED: [&str; 1] = ["RETURN"];

fn resolve(spec: &str) -> Option<Condition> {
    let upper = spec.to_uppercase();
    let bare = upper.strip_prefix("SIG").unwrap_or(&upper);
    if bare == "EXIT" {
        return Some(Condition::Exit);
    }
    if bare == "ERR" {
        return Some(Condition::Err);
    }
    if bare == "DEBUG" {
        return Some(Condition::Debug);
    }
    if UNSUPPORTED.contains(&bare) {
        return Some(Condition::Unsupported(bare.to_string()));
    }
    let number = signals::parse_spec(spec)?;
    if number == 0 {
        return Some(Condition::Exit);
    }
    signals::name_from_number(number).map(|name| Condition::Signal { number, name })
}

pub fn builtin_trap(env: &mut Environment, args: &[String]) -> Result<i32> {
    let operands = &args[1..];

    match operands.first().map(String::as_str) {
        None => return Ok(list(args, env, &[])),
        Some("-l") => return Ok(list_signal_names()),
        Some("-p") => return Ok(list(args, env, &operands[1..])),
        _ => {}
    }

    // `--` ends the options, so `trap -- - EXIT` can reset a condition without `-` being read as
    // an option letter — and so `trap -- -x EXIT` can install an action that starts with one.
    let (operands, options_ended) = match operands.first().map(String::as_str) {
        Some("--") => (&operands[1..], true),
        _ => (operands, false),
    };

    // **An option this does not know is an error, not an action.** Everything after `-l`, `-p` and
    // `--` used to fall through to "the first operand is the handler text", so `trap -z EXIT`
    // installed a handler that ran the command `-z`: status 0 at the time, and a `command not
    // found` at exit, when the cleanup the script was relying on did not happen. That is the same
    // failure the `-` case at the top of this file was written to stop, one operand along.
    if !options_ended
        && let Some(first) = operands.first()
        && first.starts_with('-')
        && first.as_str() != "-"
    {
        crate::env::complain_with_usage(
            args,
            first,
            &format!("trap: {first}: invalid option"),
            "not an option here",
            "trap: usage: trap [-lp] [[action] condition ...]",
        );
        eprintln!("{USAGE}");
        return Ok(2);
    }

    let Some(first) = operands.first() else {
        return Ok(list(args, env, &[]));
    };

    // POSIX: an unsigned integer first operand means every operand is a condition, and the
    // request is a reset. `-` is the ordinary spelling of the same thing.
    let resets_only = first.chars().all(|c| c.is_ascii_digit()) && !first.is_empty();
    let (action, conditions) = if first == "-" {
        (Some(DEFAULT_ACTION), &operands[1..])
    } else if resets_only {
        (Some(DEFAULT_ACTION), operands)
    } else {
        (Some(first.as_str()), &operands[1..])
    };

    if conditions.is_empty() {
        eprintln!(
            "{}trap: an action needs a condition to run on",
            origin_now()
        );
        eprintln!("{USAGE}");
        return Ok(2);
    }

    let action = action.unwrap_or(DEFAULT_ACTION);
    let mut status = 0;
    for spec in conditions {
        if !apply(args, env, spec, action) {
            status = 1;
        }
    }
    Ok(status)
}

/// Record one condition's new disposition, and tell the kernel about it. False on a bad operand.
fn apply(args: &[String], env: &mut Environment, spec: &str, action: &str) -> bool {
    let Some(condition) = resolve(spec) else {
        crate::env::complain(
            args,
            spec,
            &format!("trap: {spec}: invalid signal specification"),
            "not a condition",
            Some(TRAP_CONDITIONS),
        );
        return false;
    };

    if let Condition::Unsupported(name) = &condition {
        // Honest refusal rather than a silent no-op: a script that sets an ERR trap and gets
        // status 0 back is entitled to believe the handler will run.
        crate::env::complain(
            args,
            name,
            &format!("trap: {name}: condition not supported; no handler was installed"),
            "known, but not wired up",
            Some(TRAP_CONDITIONS),
        );
        return false;
    }

    env.set_trap(condition.key(), action);

    if let Condition::Signal { number, .. } = condition {
        let disposition = handlers::disposition(env, condition.key());
        // A refusal from the kernel is SIGKILL or SIGSTOP, which no process may catch. bash also
        // reports success here; the trap is recorded and simply never fires, exactly as in bash.
        let _ = handlers::arm(number, &disposition);
    }
    true
}

/// `trap` and `trap -p`: the set traps, as commands that would restore them.
///
/// Only conditions that are actually trapped are printed. bash's `-p` also lists every untrapped
/// signal as `trap -- - NAME`; that is 70 lines of noise saying nothing, and POSIX asks for the
/// re-inputtable form of the *current* traps, which is this.
fn list(args: &[String], env: &Environment, conditions: &[String]) -> i32 {
    let mut status = 0;
    let mut wanted: Vec<Condition> = Vec::new();

    if conditions.is_empty() {
        for key in env.listable_traps().keys() {
            if let Some(condition) = resolve(key) {
                wanted.push(condition);
            }
        }
    } else {
        for spec in conditions {
            match resolve(spec) {
                Some(condition) => wanted.push(condition),
                None => {
                    crate::env::complain(
                        args,
                        spec,
                        &format!("trap: {spec}: invalid signal specification"),
                        "not a condition",
                        Some(TRAP_CONDITIONS),
                    );
                    status = 1;
                }
            }
        }
    }

    wanted.sort_by_key(Condition::order);
    // Read from the *listable* view, not from `disposition`: in a subshell the inherited handlers
    // no longer run, so `disposition` correctly answers Default for them — and listing them is
    // exactly what `saved=$(trap)` needs. The two questions have different answers here.
    let listable = env.listable_traps();
    for condition in wanted {
        let text = match listable.get(condition.key()).map(String::as_str) {
            None | Some(DEFAULT_ACTION) => continue,
            Some(text) => text.to_string(),
        };
        println!(
            "trap -- {} {}",
            single_quote(&text),
            condition.display_name(env.posix())
        );
    }
    status
}

/// `trap -l`: the signal names, in the same form `kill -l` prints them.
fn list_signal_names() -> i32 {
    let names: Vec<String> = signals::all().into_iter().map(|(_, name)| name).collect();
    println!("{}", names.join(" "));
    0
}

/// Wrap handler text so the printed line can be fed back to the shell verbatim.
fn single_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    fn run(env: &mut Environment, args: &[&str]) -> i32 {
        builtin_trap(env, &words(args)).expect("trap never unwinds")
    }

    /// The bug this file was written for: `-` is a reset, not the name of a command to run.
    #[test]
    fn a_dash_resets_instead_of_installing_a_handler_called_dash() {
        let mut env = Environment::new();
        assert_eq!(run(&mut env, &["trap", "echo bye", "EXIT"]), 0);
        assert_eq!(env.get_trap("EXIT"), Some("echo bye"));

        // `-` *is* what the table now stores, but it means "default disposition" rather than a
        // command to run — which is the whole distinction the old code did not draw.
        assert_eq!(run(&mut env, &["trap", "-", "EXIT"]), 0);
        assert_eq!(
            handlers::disposition(&env, "EXIT"),
            handlers::Disposition::Default
        );
    }

    #[test]
    fn an_empty_action_is_ignore_and_a_missing_one_is_default() {
        let mut env = Environment::new();
        run(&mut env, &["trap", "", "URG"]);
        assert_eq!(
            handlers::disposition(&env, "URG"),
            handlers::Disposition::Ignore
        );
        run(&mut env, &["trap", "-", "URG"]);
        assert_eq!(
            handlers::disposition(&env, "URG"),
            handlers::Disposition::Default
        );
    }

    /// Every spelling of the same signal has to land on one key, or `trap - int` would fail to
    /// reset what `trap 'x' SIGINT` installed.
    #[test]
    fn every_spelling_of_a_signal_resolves_to_one_key() {
        for spec in ["INT", "SIGINT", "sigint", "int", "2"] {
            let mut env = Environment::new();
            run(&mut env, &["trap", "echo x", spec]);
            assert_eq!(env.get_trap("INT"), Some("echo x"), "{spec}");
        }
        for spec in ["EXIT", "exit", "0"] {
            let mut env = Environment::new();
            run(&mut env, &["trap", "echo x", spec]);
            assert_eq!(env.get_trap("EXIT"), Some("echo x"), "{spec}");
        }
    }

    #[test]
    fn an_unknown_condition_is_an_error() {
        let mut env = Environment::new();
        assert_eq!(run(&mut env, &["trap", "echo x", "NOSUCHSIG"]), 1);
        assert_eq!(run(&mut env, &["trap", "echo x", "99"]), 1);
        assert_eq!(run(&mut env, &["trap", "-p", "NOSUCHSIG"]), 1);
    }

    /// Not implemented is reported as not implemented. The previous behaviour — store it, return
    /// 0, never run it — is the failure mode a cleanup handler cannot survive.
    ///
    /// `DEBUG` and `ERR` both used to be on this list and are now real; they are asserted below
    /// rather than removed, so that implementing `RETURN` cannot quietly leave this test passing
    /// for the wrong reason.
    #[test]
    fn the_bash_only_conditions_refuse_rather_than_pretend() {
        let mut env = Environment::new();
        for name in ["RETURN"] {
            assert_eq!(run(&mut env, &["trap", "echo x", name]), 1, "{name}");
            assert_eq!(env.get_trap(name), None, "{name}");
        }
    }

    /// `DEBUG` and `ERR` are conditions the shell fires itself, so they have to be *stored* like
    /// any other — the firing is in `exec::simple` and `exec::pipeline`, but nothing can fire what
    /// `trap` refused to record.
    #[test]
    fn debug_is_recorded_like_any_other_condition() {
        let mut env = Environment::new();
        for name in ["DEBUG", "ERR"] {
            assert_eq!(run(&mut env, &["trap", "echo x", name]), 0, "{name}");
            assert_eq!(env.get_trap(name), Some("echo x"), "{name}");
            assert_eq!(run(&mut env, &["trap", "-", name]), 0, "{name}");
            assert_eq!(
                handlers::disposition(&env, name),
                handlers::Disposition::Default,
                "{name}"
            );
        }
    }

    /// An unsigned-integer first operand is a list of conditions to reset, not an action.
    #[test]
    fn a_numeric_first_operand_resets() {
        let mut env = Environment::new();
        run(&mut env, &["trap", "echo x", "INT"]);
        run(&mut env, &["trap", "echo x", "TERM"]);
        assert_eq!(run(&mut env, &["trap", "2", "15"]), 0);
        assert_eq!(
            handlers::disposition(&env, "INT"),
            handlers::Disposition::Default
        );
        assert_eq!(
            handlers::disposition(&env, "TERM"),
            handlers::Disposition::Default
        );
    }

    #[test]
    fn an_action_with_no_condition_is_a_usage_error() {
        let mut env = Environment::new();
        assert_eq!(run(&mut env, &["trap", "echo x"]), 2);
    }

    /// **A typo in a flag must not become the handler.** `trap -z EXIT` reported success and
    /// installed `-z` as the EXIT action, so the failure surfaced at exit as `-z: command not
    /// found` — by which point the cleanup the script wanted had not run and could not.
    #[test]
    fn an_unknown_option_is_refused_rather_than_installed() {
        let mut env = Environment::new();
        for bad in ["-z", "--nosuch", "-Z", "-lp"] {
            assert_eq!(run(&mut env, &["trap", bad, "EXIT"]), 2, "{bad}");
            assert_eq!(env.get_trap("EXIT"), None, "{bad} was installed anyway");
        }
    }

    /// `-` is still a reset and not an option, which is the case the check has to step around.
    #[test]
    fn a_bare_dash_is_not_read_as_an_option() {
        let mut env = Environment::new();
        run(&mut env, &["trap", "echo x", "EXIT"]);
        assert_eq!(run(&mut env, &["trap", "-", "EXIT"]), 0);
        assert_eq!(
            handlers::disposition(&env, "EXIT"),
            handlers::Disposition::Default
        );
    }

    /// And `--` still ends them, so an action may legitimately start with a dash.
    #[test]
    fn a_double_dash_lets_an_action_start_with_one() {
        let mut env = Environment::new();
        assert_eq!(run(&mut env, &["trap", "--", "-z", "EXIT"]), 0);
        assert_eq!(env.get_trap("EXIT"), Some("-z"));
    }
}
