//! `compgen [-V name] [-abcdefgjksuv] [-o option] [-A action] [-G glob] [-W words] [-F function]
//! [-C command] [-X filter] [-P prefix] [-S suffix] [word]`.
//!
//! The completions a spec built from these options would offer for `word`, one per line, or into
//! the array `-V` names. Status 1 when there are none.

use super::complete::{generate, parse};
use crate::env::origin_now;
use crate::env::scope::{Environment, ShellArray};
use oslo_base::error::Result;

const USAGE: &str = "compgen: usage: compgen [-V varname] [-abcdefgjksuv] [-o option] [-A action] \
     [-G globpat] [-W wordlist] [-F function] [-C command] [-X filterpat] [-P prefix] [-S suffix] \
     [word]";

pub fn builtin_compgen(env: &mut Environment, args: &[String]) -> Result<i32> {
    let parsed = match parse("compgen", args, "abcdefgjksuvoAGWPSXFCV", USAGE) {
        Ok(parsed) => parsed,
        Err(status) => return Ok(status),
    };
    let spec = &parsed.spec;
    for (flag, given) in [
        ("-F", spec.function.is_some()),
        ("-C", spec.command.is_some()),
    ] {
        if given {
            eprintln!(
                "{}compgen: warning: {flag} option may not work as you expect",
                origin_now()
            );
        }
    }
    let word = parsed.operands.first().cloned().unwrap_or_default();
    let mut found = generate::completions(env, spec, &word, "compgen", "")?;
    if found.is_empty() && (spec.option("bashdefault") || spec.option("default")) {
        found = generate::paths(&word, false);
    }

    match &parsed.var {
        Some(name) => {
            if !env.set_array(name, ShellArray::from_values(found.clone())) {
                return Ok(1);
            }
        }
        None => {
            for line in &found {
                println!("{line}");
            }
        }
    }
    Ok(i32::from(found.is_empty()))
}
