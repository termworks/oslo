//! The one place the shell changes directory.
//!
//! `cd`, `pushd` and `popd` all land in [`attempt_directory`], so the four things that are easy
//! to get subtly wrong are decided once: which reading of a path `$PWD` keeps, that `$OLDPWD` is
//! written on *every* move (a later `cd -` is only as good as the last builtin that moved), the
//! `CDPATH` search with its POSIX echo, and that the move is entered in the directory ring.
//!
//! Moving and complaining are two functions, not one. [`attempt_directory`] answers with the error
//! rather than printing it, because `cd` has something else to try — a remembered directory of that
//! name — and a shell that printed `No such file or directory` and then jumped somewhere would be
//! contradicting itself. [`change_directory`] is that pair put back together for the callers with
//! nothing else to try.
//!
//! The logical/physical distinction is the substance of the module. A shell that only ever
//! stores `getcwd()` — which is what oslo did — cannot answer `cd /tmp/link; pwd` the way every
//! other shell does: the answer is the path the user walked, not the path the kernel resolved.

use crate::env::scope::Environment;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// What a shell says when asked to move to nowhere. `cd ""` is a usage mistake rather than a
/// missing directory, so it does not get the `{operand}: {errno}` shape the others do.
const NULL_DIRECTORY: &str = "null directory";

/// Which reading of a path the shell keeps in `$PWD`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PathMode {
    /// `-L`, the default: symlinked components stay in `$PWD` and `..` is resolved lexically,
    /// so `cd link; cd ..` returns to where `link` was named, not to the target's parent.
    Logical,
    /// `-P`: every symlink is resolved, so `$PWD` is what `getcwd()` reports.
    Physical,
}

/// The shell's logical working directory.
///
/// `$PWD` is authoritative — that is the entire point of `-L` — but only while it still names
/// the directory the process is really in. An inherited `$PWD` from a parent that has since
/// moved (every `oslo -c` the differential harness spawns inherits one) would otherwise make
/// `pwd` lie about a directory this shell never visited, so it is validated against `getcwd()`
/// before it is trusted, exactly as bash validates it at startup.
pub fn logical_pwd(env: &Environment) -> String {
    let physical = env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    if let Some(pwd) = env.get_var("PWD")
        && Path::new(pwd).is_absolute()
        && fs::canonicalize(pwd).is_ok_and(|resolved| resolved == physical)
    {
        return pwd.to_string();
    }
    physical.to_string_lossy().into_owned()
}

/// Where an operand landed the shell.
struct Landing {
    /// The value `$PWD` takes — physical under `-P`, the logical path under `-L`.
    pwd: String,
    /// The same destination named logically. This is what `cd -` and a `CDPATH` hit echo, in
    /// either mode: bash prints the path it was *asked* for, resolved against `$PWD`, even when
    /// `-P` then stores something else.
    display: String,
}

/// Join `operand` onto `base` unless it is already absolute. `base` is always absolute.
fn absolute_against(base: &str, operand: &str) -> String {
    if operand.starts_with('/') {
        operand.to_string()
    } else {
        format!("{}/{}", base.trim_end_matches('/'), operand)
    }
}

/// Resolve `.` and `..` textually, without touching the filesystem.
///
/// This is what makes `-L` logical: `..` cancels the *name* to its left, so a symlinked
/// component is stepped back out of rather than followed. Input must be absolute.
fn lexical_canon(path: &str) -> String {
    let mut kept: Vec<&str> = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                kept.pop();
            }
            other => kept.push(other),
        }
    }
    format!("/{}", kept.join("/"))
}

/// Name `operand` the way `-L` would, relative to `base`, without going anywhere.
///
/// `pushd -n dir` needs a stack entry for a directory the shell is not moving to, and a stack
/// full of relative paths would break the moment the shell moved.
pub fn logical_join(base: &str, operand: &str) -> String {
    lexical_canon(&absolute_against(base, operand))
}

/// Try to move to `operand`, interpreted relative to `base`. Nothing is printed and no shell
/// variable is touched; only the process's working directory changes.
fn attempt(base: &str, operand: &str, mode: PathMode) -> io::Result<Landing> {
    let absolute = absolute_against(base, operand);
    let logical = lexical_canon(&absolute);

    match mode {
        PathMode::Physical => {
            let resolved = fs::canonicalize(&absolute)?;
            env::set_current_dir(&resolved)?;
            Ok(Landing {
                pwd: resolved.to_string_lossy().into_owned(),
                display: logical,
            })
        }
        PathMode::Logical => {
            // The lexical path is only used when the path as written also exists. Cancelling
            // `..` first would make `cd nosuch/..` a silent success that leaves the shell where
            // it started, and bash rejects it for the same reason.
            let intact = fs::metadata(&absolute).is_ok_and(|meta| meta.is_dir());
            if intact {
                env::set_current_dir(&logical)?;
                Ok(Landing {
                    pwd: logical.clone(),
                    display: logical,
                })
            } else {
                env::set_current_dir(&absolute)?;
                // A path that stat() refused but chdir() accepted is too strange to describe
                // logically; fall back to what the kernel says.
                let pwd = env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from(&logical))
                    .to_string_lossy()
                    .into_owned();
                Ok(Landing {
                    pwd,
                    display: logical,
                })
            }
        }
    }
}

/// Whether `CDPATH` gets a say. A path that already says where it starts from — absolute, or
/// explicitly `.`/`..`-anchored — is never searched for.
fn searches_cdpath(operand: &str) -> bool {
    !(operand.starts_with('/')
        || operand == "."
        || operand == ".."
        || operand.starts_with("./")
        || operand.starts_with("../"))
}

/// `CDPATH` split on `:`. An empty element means the current directory, and is kept because the
/// difference between an empty and a non-empty element decides whether the hit is echoed.
fn cdpath_entries(env: &Environment) -> Vec<String> {
    match env.get_var("CDPATH") {
        Some(value) if !value.is_empty() => value.split(':').map(str::to_string).collect(),
        _ => Vec::new(),
    }
}

/// Change the shell's working directory and keep `$PWD`/`$OLDPWD` in step, saying nothing about a
/// failure.
///
/// Silent about *failure* only: a `CDPATH` hit still echoes, because that echo is POSIX output on
/// the success path rather than a diagnostic. The error is handed back instead of printed so that
/// a caller may try something else first — `cd` answers a failure here with a frecency jump, and a
/// jump preceded by `No such file or directory` would be a shell contradicting itself.
///
/// Returns the destination as it should be echoed on success. [`report_failure`] turns the error
/// into the diagnostic every caller used to get from here.
pub fn attempt_directory(
    env: &mut Environment,
    operand: &str,
    mode: PathMode,
) -> io::Result<String> {
    if operand.is_empty() {
        // Refused here rather than left to `chdir`, which would take the empty string as "where I
        // already am" and report a move that never happened.
        return Err(io::Error::new(io::ErrorKind::InvalidInput, NULL_DIRECTORY));
    }

    let origin = logical_pwd(env);

    let mut landing = None;
    if searches_cdpath(operand) {
        for entry in cdpath_entries(env) {
            let candidate = if entry.is_empty() {
                operand.to_string()
            } else {
                format!("{}/{}", entry, operand)
            };
            if let Ok(found) = attempt(&origin, &candidate, mode) {
                // POSIX: when a *non-empty* CDPATH element supplies the directory, the new
                // directory is written to stdout, so a script can tell it did not land where
                // the bare operand would have taken it. An empty element is the current
                // directory, which is where the operand pointed anyway — no surprise, no echo.
                if !entry.is_empty() {
                    println!("{}", found.display);
                }
                landing = Some(found);
                break;
            }
        }
    }

    let landing = match landing {
        Some(found) => found,
        None => attempt(&origin, operand, mode)?,
    };

    // **`pre-change-dir` may refuse the move**, and this is the only place it could: every `cd`,
    // `pushd`, `popd` and frecency jump lands here, and by now the destination is resolved — so a
    // handler is told where it would end up rather than what was typed, which is the difference
    // between guarding `/srv` and guarding the word `..`.
    //
    // Refused as `Permission denied` rather than a status of its own: a caller that already knows
    // how to report a failed move reports this one the same way, and `cd` keeps its contract that
    // a non-zero status means the directory did not change.
    if refused(&origin, &landing.pwd) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "refused by a pre-change-dir hook",
        ));
    }

    env.set_var("OLDPWD", &origin, true);
    env.set_var("PWD", &landing.pwd, true);
    // Every move the shell makes passes through here, so this is where "the directories you have
    // actually been in" is written down. Recorded from `cd`'s own arm it silently omitted every
    // `pushd` and `popd`, and every `cd` inside a shell function.
    super::ring::record(&landing.pwd);
    oslo_base::hooks::fire_at_here(
        oslo_base::hooks::at::POST_CHANGE_DIR,
        &[("from", &origin), ("to", &landing.pwd)],
    );
    Ok(landing.display)
}

/// Whether a `pre-change-dir` handler said no.
///
/// The move has already been resolved and not yet made, which is the only moment where refusing it
/// means anything. `false` when nothing is attached — one relaxed load — so a shell with no such
/// hook does not pay for the question on every `cd`.
fn refused(from: &str, to: &str) -> bool {
    matches!(
        oslo_base::hooks::answer_hook_with(
            oslo_base::hooks::at::PRE_CHANGE_DIR,
            vec![oslo_base::hooks::fields(&[
                ("from", oslo_base::value::Value::str(from)),
                ("to", oslo_base::value::Value::str(to)),
            ])],
        ),
        Some(oslo_base::value::Value::Bool(false))
    )
}

/// The diagnostic a failed move prints, naming `caller`, after `origin` — see
/// [`Environment::origin`](crate::env::Environment::origin).
///
/// Split out so that a caller which tried something else after the failure can still emit the
/// original error, unchanged, when the something else came to nothing too.
pub fn report_failure(origin: &str, caller: &str, operand: &str, e: &io::Error) {
    if operand.is_empty() {
        eprintln!("{origin}{caller}: {NULL_DIRECTORY}");
    } else {
        eprintln!(
            "{origin}{caller}: {}: {}",
            oslo_base::shown::shown(operand),
            oslo_base::error::reason(e)
        );
    }
}

/// Change the shell's working directory, printing a diagnostic naming `caller` if it fails.
///
/// Returns the destination as it should be echoed on success — callers decide whether to print
/// it — or `None`. The status a builtin reports for `None` is 1: the shell has not moved.
pub fn change_directory(
    env: &mut Environment,
    operand: &str,
    mode: PathMode,
    caller: &str,
) -> Option<String> {
    match attempt_directory(env, operand, mode) {
        Ok(destination) => Some(destination),
        Err(e) => {
            report_failure(&env.origin(), caller, operand, &e);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexical_canon_cancels_names_not_targets() {
        assert_eq!(lexical_canon("/a/b/.."), "/a");
        assert_eq!(lexical_canon("/a/./b"), "/a/b");
        assert_eq!(lexical_canon("/a//b/"), "/a/b");
        assert_eq!(lexical_canon("/a/b/../.."), "/");
        // `..` at the root stays at the root, as chdir would.
        assert_eq!(lexical_canon("/../.."), "/");
    }

    #[test]
    fn absolute_against_does_not_double_the_root_slash() {
        assert_eq!(absolute_against("/", "tmp"), "/tmp");
        assert_eq!(absolute_against("/a", "b"), "/a/b");
        assert_eq!(absolute_against("/a", "/b"), "/b");
    }

    #[test]
    fn cdpath_is_skipped_for_anchored_paths() {
        assert!(searches_cdpath("sub"));
        assert!(searches_cdpath("a/b"));
        assert!(!searches_cdpath("/abs"));
        assert!(!searches_cdpath("."));
        assert!(!searches_cdpath(".."));
        assert!(!searches_cdpath("./sub"));
        assert!(!searches_cdpath("../sub"));
    }
}
