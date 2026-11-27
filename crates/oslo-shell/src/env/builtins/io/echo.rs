//! `echo`: the leading option run and the `-e` escape table.

use super::escape::expand_echo;
use crate::env::scope::Environment;
use oslo_base::error::Result;

/// What the leading `-n`/`-e`/`-E` run left behind.
struct EchoOptions {
    /// `-n` clears this.
    newline: bool,
    /// `-e` sets it, `-E` clears it again; the last one in the run wins.
    escapes: bool,
}

/// Whether an argument is part of the option run.
///
/// Only a `-` followed by one or more of `neE` counts. `echo -x`, `echo --` and `echo -` are all
/// data in bash, and the scan stops at the first of them — which is why the check is
/// all-or-nothing rather than per-character.
fn is_option(arg: &str) -> bool {
    match arg.strip_prefix('-') {
        Some(flags) => !flags.is_empty() && flags.chars().all(|c| matches!(c, 'n' | 'e' | 'E')),
        None => false,
    }
}

/// `echo [-neE] [arg…]` — arguments joined by a single space.
pub fn builtin_echo(env: &mut Environment, args: &[String]) -> Result<i32> {
    let mut opts = EchoOptions {
        newline: true,
        escapes: false,
    };
    let mut idx = 1;
    while idx < args.len() && is_option(&args[idx]) {
        for flag in args[idx][1..].chars() {
            match flag {
                'n' => opts.newline = false,
                'e' => opts.escapes = true,
                'E' => opts.escapes = false,
                _ => unreachable!("is_option admits only neE"),
            }
        }
        idx += 1;
    }

    let utf8 = super::utf8_locale(env);
    let mut output = Vec::new();
    // Each argument is decoded on its own, and a `\c` in one ends the output and the newline.
    for (at, arg) in args[idx..].iter().enumerate() {
        if at > 0 {
            output.push(b' ');
        }
        let bytes = oslo_base::lossless::decode(arg);
        if !opts.escapes {
            output.extend_from_slice(&bytes);
            continue;
        }
        let (decoded, stop) = expand_echo(&bytes, utf8);
        output.extend_from_slice(&decoded);
        if stop {
            opts.newline = false;
            break;
        }
    }
    if opts.newline {
        output.push(b'\n');
    }

    Ok(super::write_raw("echo", &output))
}

#[cfg(test)]
mod tests {
    use super::is_option;

    #[test]
    fn only_runs_of_n_e_and_capital_e_are_options() {
        assert!(is_option("-n"));
        assert!(is_option("-neE"));
        assert!(!is_option("-x"));
        assert!(!is_option("--"));
        assert!(!is_option("-"));
        assert!(!is_option("n"));
    }
}
