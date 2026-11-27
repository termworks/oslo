//! `shopt` — the option namespace `set -o` cannot reach.
//!
//! bash has two option sets, not one. `set -o` holds the POSIX options plus a few of bash's own
//! (`pipefail`, `posix`); `shopt` holds the rest (`autocd`, `globstar`, `nullglob`, …), and the
//! two are separate namespaces bridged by `shopt -o`, which reads and writes the *`set -o`* set.
//! `shopt -s errexit` is an error and `set -o autocd` is an error; only `shopt -so errexit`
//! works.
//!
//! Every name bash 5.3 has is here, with bash's default; see [`table`]. A name bash does not have
//! is reported as an invalid option name.

mod table;

pub use table::{force, note_compat, on};

use crate::env::options::ShellOption;
use crate::env::scope::Environment;
use oslo_base::error::Result;
use table::{OPTIONS, Support, index_of, record, state_of};

const USAGE: &str = "shopt: usage: shopt [-pqsu] [-o] [optname ...]";

/// Every option name, for `compgen -A shopt`.
pub fn option_names() -> impl Iterator<Item = &'static str> {
    OPTIONS.iter().map(|o| o.name)
}

/// The state of one option, for callers that are not the builtin — `oslo.shopt` in Lua.
pub fn option_state(name: &str) -> Option<bool> {
    index_of(name).map(state_of)
}

/// `$BASHOPTS`: the names of the options that are on, colon-separated.
pub fn bashopts() -> String {
    (0..OPTIONS.len())
        .filter(|&index| state_of(index))
        .map(|index| OPTIONS[index].name)
        .collect::<Vec<_>>()
        .join(":")
}

/// Set one option exactly as `shopt -s`/`-u` would.
pub fn set_option(name: &str, on: bool) -> std::result::Result<(), String> {
    let index = index_of(name).ok_or_else(|| format!("{name}: invalid shell option name"))?;
    apply(index, on);
    Ok(())
}

/// Switch option `index`, returning the compatibility level it selected, if it is a `compatNN`.
fn apply(index: usize, on: bool) -> Option<u32> {
    match OPTIONS[index].support {
        Support::Hook(hook) => {
            hook(on);
            record(index, on);
        }
        Support::Stored => record(index, on),
        Support::ReadOnly => {}
        Support::Compat(level) => return Some(table::set_level(level, on)),
    }
    None
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
        status |= one(args, env, &flags, name);
    }
    if flags.set.is_some() {
        env.publish_option_vars();
    }
    Ok(status)
}

/// `shopt` with no names: every option, filtered by `-s`/`-u` when given.
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
fn one(args: &[String], env: &mut Environment, flags: &Flags, name: &str) -> i32 {
    let Some(index) = index_of(name) else {
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
        return i32::from(!on);
    };
    if let Some(level) = apply(index, wanted) {
        env.set_var("BASH_COMPAT", &level.to_string(), false);
    }
    0
}

/// `shopt -o` — the same three verbs applied to the `set -o` namespace.
fn bridged(args: &[String], env: &mut Environment, flags: &Flags, names: &[String]) -> i32 {
    if names.is_empty() {
        if !flags.quiet {
            let options = env.options();
            let text = if flags.print {
                options.reinputtable_listing()
            } else {
                options.long_listing()
            };
            let wanted = flags.set.map(|on| if on { "\ton\n" } else { "\toff\n" });
            let wanted_p = flags.set.map(|on| if on { "set -o " } else { "set +o " });
            for line in text.split_inclusive('\n') {
                let keep = match (flags.print, wanted, wanted_p) {
                    (false, Some(tail), _) => line.ends_with(tail),
                    (true, _, Some(head)) => line.starts_with(head),
                    _ => true,
                };
                if keep {
                    print!("{line}");
                }
            }
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
                    if flags.print {
                        println!("set {}o {}", if on { '-' } else { '+' }, name);
                    } else {
                        report(name, on, false);
                    }
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
/// bare `set -o` listing uses 15.
fn report(name: &str, on: bool, as_command: bool) {
    if as_command {
        println!("shopt -{} {}", if on { 's' } else { 'u' }, name);
    } else {
        println!("{:<20}\t{}", name, on_off(on));
    }
}

#[cfg(test)]
mod tests {
    use super::{OPTIONS, builtin_shopt};
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

        assert_eq!(run(&mut env, &["shopt", "-s", "errexit"]), 1);
        assert_eq!(run(&mut env, &["shopt", "-o", "-s", "autocd"]), 1);
    }

    /// The bitset indexes [`OPTIONS`], and bash lists them sorted.
    #[test]
    fn names_are_unique_sorted_and_the_bitset_is_wide_enough() {
        for pair in OPTIONS.windows(2) {
            assert!(pair[0].name < pair[1].name, "{} out of order", pair[1].name);
        }
        assert_eq!(OPTIONS.len(), 59, "bash 5.3 has 59");
        assert!(OPTIONS.len() <= 64, "the bitset is a u64");
    }
}
