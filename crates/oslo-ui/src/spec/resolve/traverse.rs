//! Where `$chdir` can be told to go.
//!
//! carapace lets a directory be named by a macro rather than written out, because the interesting
//! directories are the ones whose path nobody knows: the root of the repository you are in, the
//! config directory for this platform, the first parent holding a `Cargo.toml`.
//!
//! ```yaml
//! ["$files", "$chdir($gitworktree)"]
//! ```

use std::path::{Path, PathBuf};

/// The directory `arg` names, resolved from `from`.
///
/// A plain path is itself. `None` when a macro names nothing that is there — a `$gitdir` outside a
/// repository is not an error, it is a position with no answer.
pub fn target(arg: &str, from: &str) -> Option<String> {
    let Some(name) = arg.strip_prefix('$') else {
        return Some(oslo_base::tilde::expand_prefix(
            arg,
            &oslo_base::tilde::from_process,
        ));
    };
    let (name, inner) = match name.find('(') {
        Some(at) if name.ends_with(')') => (&name[..at], &name[at + 1..name.len() - 1]),
        _ => (name, ""),
    };
    match name {
        "gitdir" => upwards(from, &[".git"]),
        "gitworktree" => upwards(from, &[".git"]).and_then(parent_of),
        "parent" => upwards(from, &names(inner)),
        "tempdir" => Some(std::env::temp_dir().display().to_string()),
        "userhomedir" => home(),
        "nixprofile" => home().map(|h| format!("{h}/.nix-profile")),
        "userconfigdir" | "xdgconfighome" => under("XDG_CONFIG_HOME", ".config"),
        "usercachedir" | "xdgcachehome" => under("XDG_CACHE_HOME", ".cache"),
        _ => None,
    }
}

/// The names `$parent([a, b])` was given.
fn names(arg: &str) -> Vec<String> {
    super::super::action::bracketed(arg)
}

fn home() -> Option<String> {
    std::env::var("HOME").ok().filter(|h| !h.is_empty())
}

/// An XDG directory: the variable when it is set to an absolute path, else the default under `$HOME`.
fn under(variable: &str, default: &str) -> Option<String> {
    match std::env::var(variable) {
        Ok(path) if path.starts_with('/') => Some(path),
        _ => home().map(|h| format!("{h}/{default}")),
    }
}

/// The nearest directory at or above `from` that holds any of `wanted`, and the entry itself.
///
/// The entry rather than the directory, because `$gitdir` means the `.git` folder and
/// `$gitworktree` means the directory holding it — one walk answering both.
fn upwards(from: &str, wanted: &[impl AsRef<str>]) -> Option<String> {
    if wanted.is_empty() {
        return None;
    }
    let start = match from.is_empty() {
        true => std::env::current_dir().ok()?,
        false => PathBuf::from(from),
    };
    let mut at: Option<&Path> = Some(start.as_path());
    while let Some(dir) = at {
        for name in wanted {
            let candidate = dir.join(name.as_ref());
            // A `.git` has to be a repository, not an empty directory of that name.
            let real = name.as_ref() != ".git" || oslo_base::repo::is_repository(&candidate);
            if candidate.exists() && real {
                return Some(candidate.display().to_string());
            }
        }
        at = dir.parent();
    }
    None
}

fn parent_of(path: String) -> Option<String> {
    Path::new(&path)
        .parent()
        .map(|p| p.display().to_string())
        .filter(|p| !p.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_path_is_itself() {
        assert_eq!(target("/tmp", ""), Some("/tmp".to_string()));
    }

    #[test]
    fn a_macro_that_names_nothing_here_answers_nothing() {
        assert_eq!(target("$parent([definitely-not-a-real-name])", "/"), None);
        assert_eq!(target("$nonsense", "/"), None);
    }

    #[test]
    fn the_repository_is_found_by_walking_up() {
        let root = env!("CARGO_MANIFEST_DIR");
        let deep = format!("{root}/src/spec/resolve");
        let git = target("$gitdir", &deep).expect("oslo is a checkout");
        assert!(git.ends_with(".git"), "{git}");
        let tree = target("$gitworktree", &deep).expect("and so has a worktree");
        assert!(!tree.ends_with(".git"), "{tree}");
    }
}
