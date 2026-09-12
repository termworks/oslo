//! A colon-separated variable as the list it is: `$PATH`, `$MANPATH`, and their relatives.
//!
//! # Why this is not in `direnv`, where it was
//!
//! These operations arrived with `PATH_add`, so they lived in the `.envrc` stdlib and were reachable
//! from Lua as `oslo.direnv.path_add` — behind the `direnv` feature. But **putting a directory on
//! `$PATH` is the single most common thing any configuration does**, directory environments
//! included and not specially. A build without `direnv` had no way to say it except string surgery
//! on `oslo.env.get("PATH")`, which is exactly the set of edge cases this exists to remove.
//!
//! So the operations are here, in every build, and `direnv`'s stdlib calls them. One implementation
//! either way, which is what keeps `PATH_add` in an `.envrc` and `oslo.env.path_add` in a config
//! from drifting apart.
//!
//! # What makes it more than `s .. ":" .. old`
//!
//! * **Idempotent.** A configuration is loaded and reloaded — on an edit, on a nested shell, on
//!   `direnv allow` — and appending unconditionally grows the variable every time until it is pages
//!   long. An entry already present moves to the front rather than appearing twice.
//! * **Deduplicating, as a consequence.** The list is rebuilt on every change, so duplicates that
//!   were already in it collapse — adding one entry to a `$PATH` holding two copies of `/usr/bin`
//!   leaves it one longer than it started, not two. Worth knowing rather than worth avoiding: a
//!   repeated entry is a slower lookup that can never change an answer.
//! * **Absolute.** `./bin` means the bin directory of wherever the caller is *now*, which is not
//!   where the shell will be standing when the entry is used.
//! * **No empty entries.** A trailing or doubled colon means "the current directory" to the dynamic
//!   linker and to some shells, which is a way to run something you did not mean to run.

use crate::env::Environment;
use std::path::{Path, PathBuf};

/// The entries of `name`, in order, without the empty ones.
pub fn entries(env: &Environment, name: &str) -> Vec<String> {
    let joined = env.get_var(name).unwrap_or_default().to_string();
    joined
        .split(':')
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

/// Put `dirs` at the front of `name`, absolute, in the order given, each appearing once.
///
/// Relative entries resolve against `base` — the caller's current directory for a config, and the
/// directory holding the file for a directory environment.
pub fn prepend(env: &mut Environment, name: &str, dirs: &[String], base: &Path) {
    let mut wanted: Vec<String> = dirs
        .iter()
        .map(|dir| absolute(dir, base).to_string_lossy().into_owned())
        .collect();
    for entry in entries(env, name) {
        if !wanted.contains(&entry) {
            wanted.push(entry);
        }
    }
    env.set_var(name, &wanted.join(":"), true);
}

/// Put `dirs` at the end of `name`, on the same terms.
///
/// **The other half of the pair, and the one people forget they want.** Prepending says "prefer
/// this to what is installed"; appending says "use this if nothing else provides it", which is what
/// a fallback directory of scripts is for. An entry already present is left where it is rather than
/// moved to the end — moving it would quietly demote a tool the caller had deliberately preferred.
pub fn append(env: &mut Environment, name: &str, dirs: &[String], base: &Path) {
    let mut kept = entries(env, name);
    for dir in dirs {
        let entry = absolute(dir, base).to_string_lossy().into_owned();
        if !kept.contains(&entry) {
            kept.push(entry);
        }
    }
    env.set_var(name, &kept.join(":"), true);
}

/// Drop every entry of `name` matching any of `patterns`. Answers how many went.
pub fn remove(env: &mut Environment, name: &str, patterns: &[String]) -> usize {
    let held = entries(env, name);
    let kept: Vec<String> = held
        .iter()
        .filter(|part| !patterns.iter().any(|pattern| glob(pattern, part)))
        .cloned()
        .collect();
    let gone = held.len() - kept.len();
    env.set_var(name, &kept.join(":"), true);
    gone
}

/// Whether `name` already holds `dir`, comparing it the way [`prepend`] would have written it.
pub fn contains(env: &Environment, name: &str, dir: &str, base: &Path) -> bool {
    let wanted = absolute(dir, base).to_string_lossy().into_owned();
    entries(env, name).contains(&wanted)
}

/// `path` against `base`, lexically — no symlink resolution and no touching the disk.
///
/// A directory that does not exist yet is still a legitimate entry: a build puts one there later,
/// and a `PATH` that refused it would be right only until the build ran.
pub fn absolute(path: &str, base: &Path) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        return path.to_path_buf();
    }
    let mut out = base.to_path_buf();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Whether `text` matches a shell pattern.
///
/// The shell's own matcher, not its globber: an entry named for removal need not exist, so nothing
/// walks the filesystem, and `*` crosses `/` — `PATH_rm "/nix/*"` is meant to take out everything
/// under it.
pub fn glob(pattern: &str, text: &str) -> bool {
    oslo_base::glob::ShellPattern::from_unquoted(pattern).matches(text)
}

/// Where a relative entry resolves from when nobody said: the shell's current directory.
pub fn here() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"))
}

#[cfg(test)]
#[path = "lists/tests.rs"]
mod tests;
