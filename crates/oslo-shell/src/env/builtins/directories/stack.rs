//! The directory stack model, and the two builtins that change it: `pushd` and `popd`.
//!
//! The stack is presented the way `dirs` prints it — entry 0 is the *current* directory — but
//! only entries 1..n are stored. Entry 0 is read back from `$PWD` on every use, so a plain `cd`
//! between two `pushd`s cannot leave the stack describing a directory the shell has left.
//!
//! Both builtins move through [`change_directory`], which is what keeps `$OLDPWD` correct: a
//! `popd` followed by `cd -` has to return to where the `popd` was issued from.

use super::chdir::{PathMode, change_directory, logical_pwd};
use super::dirs::builtin_dirs;
use crate::env::origin_now;
use crate::env::scope::Environment;
use oslo_base::error::Result;
use std::path::PathBuf;

const PUSHD_USAGE: &str = "pushd: usage: pushd [-n] [+N | -N | dir]";
const POPD_USAGE: &str = "popd: usage: popd [-n] [+N | -N]";

/// The stack as `dirs` prints it: index 0 is the current directory, then the pushed entries,
/// most recent first.
pub fn stack(env: &Environment) -> Vec<String> {
    let mut entries = vec![logical_pwd(env)];
    entries.extend(
        env.get_dir_stack()
            .iter()
            .rev()
            .map(|path| path.to_string_lossy().into_owned()),
    );
    entries
}

/// Replace the stored stack with `entries[1..]`.
///
/// `entries[0]` is deliberately dropped: the current directory lives in `$PWD`, and storing a
/// second copy is how a stack starts disagreeing with the shell it belongs to.
pub fn store(env: &mut Environment, entries: &[String]) {
    while env.pop_dir().is_some() {}
    for entry in entries.iter().skip(1).rev() {
        env.push_dir(PathBuf::from(entry));
    }
}

/// Whether `spec` is a `+N`/`-N` stack index rather than a directory or a flag.
pub fn is_index(spec: &str) -> bool {
    let Some(digits) = spec.strip_prefix(['+', '-']) else {
        return false;
    };
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// Resolve `+N` (counting from the top) or `-N` (counting from the bottom) against a stack of
/// `len` entries. `None` when the spec names no entry.
pub fn resolve_index(spec: &str, len: usize) -> Option<usize> {
    let n: usize = spec[1..].parse().ok()?;
    if n >= len {
        return None;
    }
    if spec.starts_with('+') {
        Some(n)
    } else {
        Some(len - 1 - n)
    }
}

/// What a `pushd`/`popd` argument turned out to be.
enum Operand {
    /// `+N`/`-N`.
    Index(String),
    /// A directory name.
    Dir(String),
}

/// Split off `-n` and the single optional operand. `Err` carries the exit status after the
/// diagnostic has been printed.
fn parse_args(
    args: &[String],
    caller: &str,
    usage: &str,
    dirs_allowed: bool,
) -> std::result::Result<(bool, Option<Operand>), i32> {
    let mut no_cd = false;
    let mut operand = None;
    for arg in args.iter().skip(1) {
        let found = match arg.as_str() {
            "-n" => {
                no_cd = true;
                continue;
            }
            spec if is_index(spec) => Operand::Index(spec.to_string()),
            flag if flag.starts_with('-') || flag.starts_with('+') => {
                crate::env::complain_with_usage(
                    args,
                    flag,
                    &format!("{caller}: {flag}: invalid number"),
                    "not a stack position",
                    usage,
                );
                return Err(2);
            }
            dir if dirs_allowed => Operand::Dir(dir.to_string()),
            other => {
                crate::env::complain_with_usage(
                    args,
                    other,
                    &format!("{caller}: {other}: invalid argument"),
                    "not an argument this takes",
                    usage,
                );
                return Err(2);
            }
        };
        if operand.is_some() {
            // bash calls this a failed pushd rather than a usage error, and so does oslo: the
            // arguments parsed, there were simply too many of them.
            crate::env::complain(
                args,
                arg,
                &format!("{caller}: too many arguments"),
                "one too many",
                Some(usage),
            );
            return Err(1);
        }
        operand = Some(found);
    }
    Ok((no_cd, operand))
}

pub fn builtin_pushd(env: &mut Environment, args: &[String]) -> Result<i32> {
    let (no_cd, operand) = match parse_args(args, "pushd", PUSHD_USAGE, true) {
        Ok(parsed) => parsed,
        Err(status) => return Ok(status),
    };

    let entries = stack(env);
    let next = match operand {
        Some(Operand::Index(spec)) => {
            let Some(index) = resolve_index(&spec, entries.len()) else {
                crate::env::complain(
                    args,
                    &spec,
                    &format!("pushd: {spec}: directory stack index out of range"),
                    "no such entry",
                    Some("`dirs -v` numbers the stack; +N counts from the left, -N from the right"),
                );
                return Ok(1);
            };
            match rotate(env, entries, index, no_cd) {
                Some(rotated) => rotated,
                None => return Ok(1),
            }
        }
        Some(Operand::Dir(dir)) => {
            let mut next = entries;
            if no_cd {
                // `-n` keeps the shell where it is, so the new entry goes *below* the current
                // directory rather than becoming it — as written, the way bash stores it.
                next.insert(1, dir);
            } else {
                // CDPATH can send the shell somewhere the operand does not name, so the entry
                // is read back from `$PWD` afterwards rather than guessed from the operand.
                if change_directory(env, &dir, PathMode::Logical, "pushd").is_none() {
                    return Ok(1);
                }
                next.insert(0, logical_pwd(env));
            }
            next
        }
        None => {
            if entries.len() < 2 {
                eprintln!("{}pushd: no other directory", origin_now());
                return Ok(1);
            }
            if no_cd {
                // The exchange is the whole command, and it cannot happen without leaving the
                // current directory — which is what `-n` forbids.
                return Ok(0);
            }
            // No operand *exchanges* the top two entries rather than rotating: on a stack of
            // three or more, `pushd; pushd` returns to where it started, while `pushd +1` twice
            // would have walked two entries down.
            let mut swapped = entries;
            swapped.swap(0, 1);
            if change_directory(env, &swapped[0], PathMode::Logical, "pushd").is_none() {
                return Ok(1);
            }
            swapped
        }
    };

    store(env, &next);
    builtin_dirs(env, &[])
}

/// Rotate `entries` so that `index` becomes the top, moving there unless `-n` was given.
///
/// `None` means the move failed and the stack must be left alone.
fn rotate(
    env: &mut Environment,
    mut entries: Vec<String>,
    index: usize,
    no_cd: bool,
) -> Option<Vec<String>> {
    if no_cd {
        // Entry 0 is the process's own directory and `-n` forbids leaving it, so only the
        // stored portion turns.
        if entries.len() > 1 {
            let by = index.saturating_sub(1);
            entries[1..].rotate_left(by);
        }
        return Some(entries);
    }
    entries.rotate_left(index);
    change_directory(env, &entries[0], PathMode::Logical, "pushd")?;
    Some(entries)
}

pub fn builtin_popd(env: &mut Environment, args: &[String]) -> Result<i32> {
    let (no_cd, operand) = match parse_args(args, "popd", POPD_USAGE, false) {
        Ok(parsed) => parsed,
        Err(status) => return Ok(status),
    };

    let mut entries = stack(env);
    if entries.len() < 2 {
        eprintln!("{}popd: directory stack empty", origin_now());
        return Ok(1);
    }

    let mut index = match operand {
        Some(Operand::Index(spec)) => match resolve_index(&spec, entries.len()) {
            Some(index) => index,
            None => {
                crate::env::complain(
                    args,
                    &spec,
                    &format!("popd: {spec}: directory stack index out of range"),
                    "no such entry",
                    Some("`dirs -v` numbers the stack; +N counts from the left, -N from the right"),
                );
                return Ok(1);
            }
        },
        _ => 0,
    };
    if no_cd && index == 0 {
        // Dropping entry 0 would mean leaving the directory, which `-n` forbids; the stored top
        // goes instead.
        index = 1;
    }

    entries.remove(index);
    if index == 0 && change_directory(env, &entries[0], PathMode::Logical, "popd").is_none() {
        return Ok(1);
    }

    store(env, &entries);
    builtin_dirs(env, &[])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_specs_are_recognised() {
        assert!(is_index("+0"));
        assert!(is_index("-12"));
        assert!(!is_index("-"));
        assert!(!is_index("+"));
        assert!(!is_index("-n"));
        assert!(!is_index("dir"));
        assert!(!is_index("+1a"));
    }

    #[test]
    fn plus_counts_from_the_top_and_minus_from_the_bottom() {
        assert_eq!(resolve_index("+0", 3), Some(0));
        assert_eq!(resolve_index("+2", 3), Some(2));
        assert_eq!(resolve_index("-0", 3), Some(2));
        assert_eq!(resolve_index("-2", 3), Some(0));
        assert_eq!(resolve_index("+3", 3), None);
        assert_eq!(resolve_index("-3", 3), None);
    }
}
