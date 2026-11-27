//! `enable [-a] [-dnps] [-f filename] [name …]` — turn builtins on and off, or list them.
//!
//! A disabled builtin stays registered but is invisible to dispatch, `type` and `command`, so the
//! name falls through to `$PATH`. oslo loads no builtins from shared objects: `-f` and `-d` are
//! refused the way a bash built without dynamic loading refuses them.

use crate::env::origin_now;
use crate::env::scope::Environment;
use oslo_base::error::Result;

const USAGE: &str = "enable: usage: enable [-a] [-dnps] [-f filename] [name ...]";

/// bash's special builtins, which `enable -s` restricts a listing to.
const SPECIAL: &[&str] = &[
    ".", ":", "break", "continue", "eval", "exec", "exit", "export", "readonly", "return", "set",
    "shift", "source", "times", "trap", "unset",
];

#[derive(Default)]
struct Flags {
    all: bool,
    disable: bool,
    special: bool,
    print: bool,
}

pub fn builtin_enable(env: &mut Environment, args: &[String]) -> Result<i32> {
    let mut flags = Flags::default();
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
                'a' => flags.all = true,
                'n' => flags.disable = true,
                's' => flags.special = true,
                'p' => flags.print = true,
                'f' | 'd' => {
                    eprintln!("{}enable: dynamic loading not available", origin_now());
                    return Ok(2);
                }
                other => {
                    crate::env::complain_option(
                        args,
                        other,
                        &format!("enable: -{other}: invalid option"),
                        USAGE,
                    );
                    return Ok(2);
                }
            }
        }
        i += 1;
    }

    let names = &args[i.min(args.len())..];
    if names.is_empty() || flags.print {
        list(env, &flags);
        return Ok(0);
    }

    let mut status = 0;
    for name in names {
        if name == "[[" || !env.set_builtin_enabled(name, !flags.disable) {
            crate::env::complain(
                args,
                name,
                &format!("enable: {name}: not a shell builtin"),
                "no builtin of that name",
                Some("`enable -a` lists every builtin"),
            );
            status = 1;
        }
    }
    Ok(status)
}

/// Every builtin `enable` and `compgen -b` know of, sorted, with whether it is on.
///
/// `[[` is a builtin to oslo's dispatcher and a keyword to bash, so it is left out here.
pub fn builtins(env: &Environment) -> Vec<(String, bool)> {
    env.builtin_states()
        .into_iter()
        .filter(|(name, _)| name != "[[")
        .collect()
}

fn list(env: &Environment, flags: &Flags) {
    for (name, on) in builtins(env) {
        if flags.special && !SPECIAL.contains(&name.as_str()) {
            continue;
        }
        let shown = flags.all || on != flags.disable;
        if !shown {
            continue;
        }
        match on {
            true => println!("enable {name}"),
            false => println!("enable -n {name}"),
        }
    }
}
