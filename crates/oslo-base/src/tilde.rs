//! Tilde expansion — `~`, `~user`, `~+` and `~-`.
//!
//! **Here rather than in the shell because four callers need it and three of them are the prompt.**
//! The shell expands `~root/bin` and `~+/src` exactly as bash does; completion, the ghost suggestion
//! and the highlighter each knew only `~` and `~/…`, so `ls ~+/` offered nothing, `~root` never lit
//! as a path that is there, and a form the shell understands perfectly read at the prompt as a
//! mistake. One expander answering all four is the only arrangement in which they cannot disagree.
//!
//! Every failure path returns the text unchanged, which is what POSIX asks for and also the only
//! safe answer: `~nosuchuser` as a literal is a path that will fail loudly, while a guessed home
//! directory is a path that will succeed on the wrong files.

/// How a caller reads a shell variable. The shell passes its own; the prompt passes the process's.
pub type Lookup<'a> = &'a dyn Fn(&str) -> Option<String>;

/// Expand the text between a leading `~` and the first `/`.
pub fn expand(user: &str, var: Lookup<'_>) -> String {
    match user {
        "" => home_directory(var),
        // `~+` and `~-` are the shell's own notion of where it is, not a user's: they read the
        // variables `cd` maintains, so they follow symlinks exactly the way `cd` left them.
        "+" => var("PWD")
            .or_else(current_directory)
            .unwrap_or_else(|| "~+".to_string()),
        // With no previous directory there is nothing to name, and bash leaves `~-` literal.
        "-" => var("OLDPWD").unwrap_or_else(|| "~-".to_string()),
        name => passwd_home(name).unwrap_or_else(|| format!("~{name}")),
    }
}

/// Expand a leading tilde in a whole path, leaving the rest alone.
///
/// `~root/bin/` becomes `/root/bin/`; a path with no tilde comes back untouched. What the prompt
/// wants, where the tilde is always at the front of a word and never in the middle.
pub fn expand_prefix(path: &str, var: Lookup<'_>) -> String {
    let Some(rest) = path.strip_prefix('~') else {
        return path.to_string();
    };
    let cut = rest.find('/').unwrap_or(rest.len());
    let (user, tail) = rest.split_at(cut);
    // A name that resolved to nothing comes back carrying its own `~`, so the two cases join here:
    // either way what precedes the tail is the whole of the answer.
    format!("{}{tail}", expand(user, var))
}

/// A lookup that reads this process's environment — what the prompt uses when it has no shell to
/// ask. `PWD` and `OLDPWD` are exported by every shell that maintains them, `cd` included.
pub fn from_process(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// `$HOME`, falling back to the password database.
///
/// The fallback matters for `env -u HOME sh`: a login without `HOME` still has a home directory,
/// and expanding `~` to `/` there would point every script at the filesystem root.
fn home_directory(var: Lookup<'_>) -> String {
    if let Some(home) = var("HOME") {
        return home;
    }
    passwd_home_of_uid().unwrap_or_else(|| "/".to_string())
}

fn current_directory() -> Option<String> {
    std::env::current_dir()
        .ok()
        .map(|p| crate::lossless::encode(p.as_os_str()))
}

/// The home directory recorded for `name`, or `None` if there is no such user.
/// The user names `~name` could stand for, for a completion to offer.
///
/// **Read from `/etc/passwd` rather than enumerated through `getpwent`.** `nix` exposes no safe
/// iterator, and `getpwent` is not thread-safe — this runs on the keystroke path beside a writer
/// thread. What it misses is a directory service (LDAP, SSSD) that answers `getpwnam` without
/// having a line in the file; those names still *expand*, because expansion goes through
/// `User::from_name`. They simply are not offered before they are typed, which is the same bargain
/// bash's own `~` completion makes.
///
/// Nothing is cached: this is reached only when a word begins with `~` and has no `/` in it yet,
/// which is a keystroke or two per session.
pub fn user_names() -> Vec<String> {
    let Ok(passwd) = std::fs::read_to_string("/etc/passwd") else {
        return Vec::new();
    };
    let mut names: Vec<String> = passwd
        .lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.split(':').next())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect();
    names.sort();
    names.dedup();
    names
}

fn passwd_home(name: &str) -> Option<String> {
    let user = nix::unistd::User::from_name(name).ok().flatten()?;
    Some(crate::lossless::encode(user.dir.as_os_str()))
}

fn passwd_home_of_uid() -> Option<String> {
    let user = nix::unistd::User::from_uid(nix::unistd::getuid())
        .ok()
        .flatten()?;
    Some(crate::lossless::encode(user.dir.as_os_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn shell(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// The four forms, and the whole-path wrapper the prompt uses.
    #[test]
    fn every_form_expands_the_way_the_shell_does() {
        let vars = shell(&[("HOME", "/home/u"), ("PWD", "/here"), ("OLDPWD", "/there")]);
        let var = |name: &str| vars.get(name).cloned();

        assert_eq!(expand("", &var), "/home/u");
        assert_eq!(expand("+", &var), "/here");
        assert_eq!(expand("-", &var), "/there");

        assert_eq!(expand_prefix("~/bin", &var), "/home/u/bin");
        assert_eq!(expand_prefix("~+/src/", &var), "/here/src/");
        assert_eq!(expand_prefix("~-", &var), "/there");
        // No tilde, no change — the overwhelming case.
        assert_eq!(expand_prefix("/etc/", &var), "/etc/");
        assert_eq!(expand_prefix("rel/path", &var), "rel/path");
    }

    /// **A name nobody can resolve stays exactly as it was.** A guessed home directory is a path
    /// that succeeds on the wrong files, which is worse than one that fails loudly.
    #[test]
    fn an_unresolvable_tilde_is_left_alone() {
        let vars = shell(&[("HOME", "/home/u")]);
        let var = |name: &str| vars.get(name).cloned();

        assert_eq!(expand("nosuchuser-xyzzy", &var), "~nosuchuser-xyzzy");
        assert_eq!(
            expand_prefix("~nosuchuser-xyzzy/bin", &var),
            "~nosuchuser-xyzzy/bin"
        );
        // `~-` with nothing to name is bash's literal, not an empty path.
        assert_eq!(expand("-", &var), "~-");
        assert_eq!(expand_prefix("~-/x", &var), "~-/x");
    }

    /// `~root` is a real account on every Linux system, so the password lookup is exercised for
    /// real rather than mocked.
    #[test]
    fn a_real_account_resolves_through_the_password_database() {
        let var = |_: &str| None;
        let home = expand("root", &var);
        assert!(home.starts_with('/'), "root has a home directory: {home:?}");
    }
}
