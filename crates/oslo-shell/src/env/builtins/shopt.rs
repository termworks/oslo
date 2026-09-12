//! `shopt` — the option namespace `set -o` cannot reach.
//!
//! bash has two option sets, not one. `set -o` holds the POSIX options plus a few of bash's own
//! (`pipefail`, `posix`); `shopt` holds the rest (`autocd`, `globstar`, `nullglob`, …), and the
//! two are separate namespaces that happen to be bridged by `shopt -o`, which reads and writes
//! the *`set -o`* set. `shopt -s errexit` is an error and `set -o autocd` is an error; only
//! `shopt -so errexit` works. oslo keeps them separate for the same reason: merging them would
//! make `set -o globstar` succeed, which is a spelling no other shell accepts.
//!
//! # What oslo promises about a shopt option
//!
//! Every option here is one of two kinds, and the difference is the whole design:
//!
//! * [`Support::Hook`] — oslo implements both states, and `shopt -s`/`-u` really switch it.
//! * [`Support::Fixed`] — oslo behaves as if the option were permanently on or permanently off.
//!   Asking for the state it is already in succeeds and does nothing; asking for the other one
//!   *fails, loudly*. `shopt -s globstar` returning 0 while `**` kept meaning `*` would be a lie
//!   that only shows up as a wrong file list, which is exactly the failure mode a shell must not
//!   have.
//!
//! A name bash has but this table does not is reported as an invalid option name rather than
//! guessed at, so a typo and a gap are never confused.

use crate::env::options::ShellOption;
use crate::env::scope::Environment;
use oslo_base::error::Result;
use std::sync::atomic::{AtomicU32, Ordering};

const USAGE: &str = "shopt: usage: shopt [-pqsu] [-o] [optname ...]";

/// How much of an option oslo actually implements. See the module docs.
enum Support {
    /// Switchable: the hook is called with the new state.
    Hook(fn(bool)),
    /// Not switchable: oslo's behaviour is this state, always.
    Fixed(bool),
}

struct ShoptOption {
    name: &'static str,
    support: Support,
    /// Why the state is fixed, in one clause, for the diagnostic. Empty for a `Hook`.
    because: &'static str,
}

const fn hook(name: &'static str, apply: fn(bool)) -> ShoptOption {
    ShoptOption {
        name,
        support: Support::Hook(apply),
        because: "",
    }
}

const fn fixed(name: &'static str, state: bool, because: &'static str) -> ShoptOption {
    ShoptOption {
        name,
        support: Support::Fixed(state),
        because,
    }
}

/// The options oslo can answer for, in the alphabetical order bash lists them in.
///
/// Every `Fixed` entry is a claim about oslo's behaviour that has been checked against the code
/// that implements it — `expand_aliases` against the unconditional alias substitution in
/// `exec::simple`, the glob options against `expand::glob`, `interactive_comments` against the
/// lexer. A claim that stops being true belongs in the same commit as the change that broke it.
#[rustfmt::skip]
const OPTIONS: &[ShoptOption] = &[
    hook("autocd", crate::exec::simple::set_autocd),
    fixed("cdspell", false, "cd does not correct spelling"),
    hook("dotglob", crate::expand::glob::set_dotglob),
    fixed("expand_aliases", true, "oslo expands aliases in every shell, not only interactive ones"),
    fixed("extglob", false, "the extended pattern operators are not implemented"),
    hook("failglob", crate::expand::glob::set_failglob),
    hook("globstar", crate::expand::glob::set_globstar),
    fixed("huponexit", false, "the shell does not signal its jobs on exit"),
    fixed("interactive_comments", true, "`#` starts a comment in every shell"),
    fixed("lastpipe", false, "every stage of a pipeline runs in its own process"),
    hook("nocaseglob", crate::expand::glob::set_nocaseglob),
    hook("nocasematch", crate::expand::glob::set_nocasematch),
    hook("nullglob", crate::expand::glob::set_nullglob),
    fixed("shift_verbose", false, "`shift` past the end is silent"),
    fixed("xpg_echo", false, "`echo` expands escapes only under `-e`"),
    // **The ones a `.bashrc` sets without thinking about it.**
    //
    // Every name below is an option oslo has a real answer for, and the answer happens to be the
    // one bash defaults to — so `shopt -s promptvars` is a request oslo already satisfies and
    // returns 0 for. They are here because the alternative is what a login shell used to do: print
    // `invalid shell option name` and a usage block, twice, on a line the user did not write and
    // for a setting that was already true.
    //
    // A name oslo has no answer for is still refused. This is not a list of things to ignore; it is
    // a list of questions oslo can answer.
    fixed("checkwinsize", true, "the terminal is measured every frame, so a resize needs no signal"),
    fixed("cmdhist", true, "a multi-line command is one history entry, with its newlines escaped"),
    fixed("histappend", true, "the history file is only ever appended to, never rewritten"),
    fixed("progcomp", true, "completion specs and providers are how oslo completes an argument"),
    fixed("promptvars", true, "parameters expand in `$PS1`"),
    fixed("checkjobs", false, "the shell keeps no table of running jobs to check"),
];

/// Which options are on. Bit `n` is `OPTIONS[n]`; the `Fixed` ones are seeded by [`state_of`].
///
/// Process-global rather than a field on [`Environment`], like the `autocd` flag it drives: a
/// forked subshell inherits the settings as they stand, which is what bash does too.
static ENABLED: AtomicU32 = AtomicU32::new(0);

fn state_of(index: usize) -> bool {
    match OPTIONS[index].support {
        Support::Fixed(state) => state,
        Support::Hook(_) => ENABLED.load(Ordering::Relaxed) & (1 << index) != 0,
    }
}

fn record(index: usize, on: bool) {
    let bit = 1u32 << index;
    if on {
        ENABLED.fetch_or(bit, Ordering::Relaxed);
    } else {
        ENABLED.fetch_and(!bit, Ordering::Relaxed);
    }
}

/// The state of one option, for callers that are not the builtin — `oslo.shopt` in Lua.
pub fn option_state(name: &str) -> Option<bool> {
    OPTIONS.iter().position(|o| o.name == name).map(state_of)
}

/// Set one option exactly as `shopt -s`/`-u` would, refusing a fixed one with its reason.
pub fn set_option(name: &str, on: bool) -> std::result::Result<(), String> {
    let Some(index) = OPTIONS.iter().position(|o| o.name == name) else {
        return Err(format!("{name}: invalid shell option name"));
    };
    match OPTIONS[index].support {
        Support::Hook(apply) => {
            apply(on);
            record(index, on);
            Ok(())
        }
        Support::Fixed(state) if state == on => Ok(()),
        Support::Fixed(_) => Err(format!(
            "{name}: cannot be turned {}: {}",
            on_off(on),
            OPTIONS[index].because
        )),
    }
}

/// Everything the option run decided.
struct Flags {
    /// `-s` or `-u`: the state being asked for. `None` is a query.
    set: Option<bool>,
    /// `-p`: print as the command that would restore the setting.
    print: bool,
    /// `-q`: no output, the status is the answer.
    quiet: bool,
    /// `-o`: act on the `set -o` namespace instead of this one.
    bridge: bool,
}

/// `shopt [-pqsu] [-o] [optname ...]`.
pub fn builtin_shopt(env: &mut Environment, args: &[String]) -> Result<i32> {
    let mut flags = Flags {
        set: None,
        print: false,
        quiet: false,
        bridge: false,
    };

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
        for c in arg[1..].chars() {
            match c {
                's' => flags.set = Some(true),
                'u' => flags.set = Some(false),
                'p' => flags.print = true,
                'q' => flags.quiet = true,
                'o' => flags.bridge = true,
                other => {
                    crate::env::complain_option(
                        args,
                        other,
                        &format!("shopt: -{other}: invalid option"),
                        USAGE,
                    );
                    return Ok(2);
                }
            }
        }
        i += 1;
    }

    let names = &args[i.min(args.len())..];
    if flags.bridge {
        return Ok(bridged(args, env, &flags, names));
    }
    if names.is_empty() {
        return Ok(list_all(&flags));
    }

    let mut status = 0;
    for name in names {
        status |= one(args, &flags, name);
    }
    Ok(status)
}

/// `shopt` with no names: every option this shell knows, filtered by `-s`/`-u` when given.
fn list_all(flags: &Flags) -> i32 {
    for (index, option) in OPTIONS.iter().enumerate() {
        let on = state_of(index);
        if flags.set.is_some_and(|wanted| wanted != on) {
            continue;
        }
        if !flags.quiet {
            report(option.name, on, flags.print);
        }
    }
    0
}

/// One named option: set it, or report it.
fn one(args: &[String], flags: &Flags, name: &str) -> i32 {
    let Some(index) = OPTIONS.iter().position(|o| o.name == name) else {
        crate::env::complain_with_usage(
            args,
            name,
            &format!("shopt: {name}: invalid shell option name"),
            "not a shell option",
            USAGE,
        );
        return 1;
    };
    let Some(wanted) = flags.set else {
        let on = state_of(index);
        if !flags.quiet {
            report(name, on, flags.print);
        }
        // A query's status is the answer: `shopt -q autocd` is how a script asks.
        return i32::from(!on);
    };

    match OPTIONS[index].support {
        Support::Hook(apply) => {
            apply(wanted);
            record(index, wanted);
            0
        }
        // Already in the requested state: nothing to do, and nothing to complain about.
        Support::Fixed(state) if state == wanted => 0,
        Support::Fixed(state) => {
            eprintln!(
                "oslo: shopt: {}: cannot be turned {}: {}",
                name,
                if wanted { "on" } else { "off" },
                OPTIONS[index].because
            );
            eprintln!(
                "oslo: shopt: {} is permanently {} in this shell",
                name,
                on_off(state)
            );
            1
        }
    }
}

/// `shopt -o` — the same three verbs applied to the `set -o` namespace.
///
/// bash's bridge, and the reason the two tables can stay apart: a script that wants `errexit`
/// through `shopt` writes `shopt -so errexit`, and it reaches exactly the option `set -o errexit`
/// would have.
fn bridged(args: &[String], env: &mut Environment, flags: &Flags, names: &[String]) -> i32 {
    if names.is_empty() {
        if !flags.quiet {
            print!("{}", env.options().long_listing());
        }
        return 0;
    }

    let mut status = 0;
    for name in names {
        let Some(option) = ShellOption::from_name(name) else {
            crate::env::complain(
                args,
                name,
                &format!("shopt: {name}: invalid option name"),
                "not a set -o option",
                Some("`set -o` lists them"),
            );
            status = 1;
            continue;
        };
        match flags.set {
            Some(wanted) => env.set_option(option, wanted),
            None => {
                let on = env.option(option);
                if !flags.quiet {
                    report(name, on, flags.print);
                }
                status |= i32::from(!on);
            }
        }
    }
    status
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

/// One line of output, in whichever of bash's two shapes was asked for.
///
/// The column is 20 wide followed by a tab, which is what bash pads a *named* report to — its
/// bare `set -o` listing uses 15, and the two really do differ.
fn report(name: &str, on: bool, as_command: bool) {
    if as_command {
        println!("shopt -{} {}", if on { 's' } else { 'u' }, name);
    } else {
        println!("{:<20}\t{}", name, on_off(on));
    }
}

#[cfg(test)]
mod tests {
    use super::{OPTIONS, Support, builtin_shopt};
    use crate::env::Environment;
    use crate::env::options::ShellOption;

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|s| s.to_string()).collect()
    }

    fn run(env: &mut Environment, words: &[&str]) -> i32 {
        builtin_shopt(env, &argv(words)).unwrap()
    }

    /// The switchable option's round trip, and the query status that goes with each state.
    #[test]
    fn a_switchable_option_can_be_set_and_queried() {
        let mut env = Environment::new();
        assert_eq!(run(&mut env, &["shopt", "-s", "autocd"]), 0);
        assert_eq!(run(&mut env, &["shopt", "-q", "autocd"]), 0);
        assert_eq!(run(&mut env, &["shopt", "-u", "autocd"]), 0);
        assert_eq!(run(&mut env, &["shopt", "-q", "autocd"]), 1);
    }

    /// The rule this builtin exists to keep: an option oslo does not implement must not report
    /// success when asked to turn it on. Reporting 0 for `extglob` would mean every later `@(a|b)`
    /// silently matched the wrong files.
    ///
    /// `globstar` was this test's example until it was implemented — which is the outcome the rule
    /// is for. It is now a real option and lives in the test below.
    #[test]
    fn an_option_oslo_cannot_honour_is_refused_not_faked() {
        let mut env = Environment::new();
        assert_eq!(run(&mut env, &["shopt", "-s", "extglob"]), 1);
        assert_eq!(run(&mut env, &["shopt", "-q", "extglob"]), 1);
        // Asking for the state it is already in is not a failure: there is nothing to do.
        assert_eq!(run(&mut env, &["shopt", "-u", "extglob"]), 0);
    }

    /// An option oslo *does* implement is switchable both ways and reports what it is.
    #[test]
    fn globstar_is_a_real_option() {
        let mut env = Environment::new();
        assert_eq!(run(&mut env, &["shopt", "-s", "globstar"]), 0);
        assert_eq!(run(&mut env, &["shopt", "-q", "globstar"]), 0);
        assert_eq!(run(&mut env, &["shopt", "-u", "globstar"]), 0);
        assert_eq!(run(&mut env, &["shopt", "-q", "globstar"]), 1, "off again");
    }

    #[test]
    fn an_unknown_name_is_reported_and_an_unknown_flag_is_a_usage_error() {
        let mut env = Environment::new();
        assert_eq!(run(&mut env, &["shopt", "-s", "no_such_option"]), 1);
        assert_eq!(run(&mut env, &["shopt", "-Z"]), 2);
    }

    /// `-o` is a different namespace, and it is the *only* way `shopt` reaches `set -o`.
    #[test]
    fn the_o_bridge_reads_and_writes_the_set_o_options() {
        let mut env = Environment::new();
        assert_eq!(run(&mut env, &["shopt", "-o", "-q", "errexit"]), 1);
        assert_eq!(run(&mut env, &["shopt", "-s", "-o", "errexit"]), 0);
        assert!(env.option(ShellOption::ErrExit));
        assert_eq!(run(&mut env, &["shopt", "-o", "-q", "errexit"]), 0);

        // The two namespaces do not leak into each other.
        assert_eq!(run(&mut env, &["shopt", "-s", "errexit"]), 1);
        assert_eq!(run(&mut env, &["shopt", "-o", "-s", "autocd"]), 1);
    }

    /// The bitset indexes [`OPTIONS`], so a duplicate name would alias two options.
    #[test]
    fn names_are_unique_and_the_bitset_is_wide_enough() {
        for (i, option) in OPTIONS.iter().enumerate() {
            assert_eq!(
                OPTIONS.iter().position(|o| o.name == option.name),
                Some(i),
                "{} is listed twice",
                option.name
            );
        }
        assert!(OPTIONS.len() <= 32, "the bitset is a u32");
        assert!(
            OPTIONS
                .iter()
                .any(|o| matches!(o.support, Support::Hook(_))),
            "a table with nothing switchable would make `shopt -s` meaningless"
        );
    }
}
