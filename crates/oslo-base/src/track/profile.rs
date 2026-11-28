//! Whose history this is.
//!
//! The store is named after a **profile** rather than after what it contains: `default.kv`, not
//! `history.db` or `track.kv`.
//!
//! # Why a name at all
//!
//! Because more than one thing runs commands through this shell. An agent that shells out — and
//! they all do — writes thousands of lines into the same history a person is trying to search, and
//! into the same frecency table that decides what `cd` and Tab suggest. `oslo --profile=claude` gives it
//! its own store: its history is still recorded, still searchable by pointing another
//! shell at the same profile, and no longer mixed into yours.
//!
//! It is a *profile*, not a lock. Nothing stops two shells sharing one, which is what makes
//! `oslo --profile=claude` twice in a row accumulate rather than start over.
//!
//! A profile also carries a **key**, in [`key`]: the thing that says two machines mean the same
//! history rather than two histories that share a word. Nothing needs it until `oslo profile sync`
//! does.
//!
//! Either `--profile=NAME` or `$OSLO_PROFILE` names it — the flag for one invocation, the variable
//! for a whole session, and the flag wins when both are given.

pub mod key;

use std::path::PathBuf;

/// The environment variable that names a profile.
pub const ENV: &str = "OSLO_PROFILE";

/// The profile in force: `$OSLO_PROFILE`, or `default`.
///
/// **The variable and nothing else.** There was a `--profile` flag; it is gone, because a profile
/// is a property of a *session* rather than of one command. Export it once and every `oslo` that
/// anything spawns inherits it — which is the whole point when the thing spawning shells is an
/// agent running thousands of commands you would rather keep out of your own history. A flag
/// covers only the invocation you remembered to put it on.
///
/// Read rather than cached, so exporting it mid-session takes effect on the next shell without
/// anything having to be told.
pub fn current() -> String {
    match std::env::var(ENV) {
        Ok(name) if valid(&name) => name,
        // Named but unusable. Said once rather than per call, and then the default is used: a
        // shell that refused to start over a stray variable would be worse than one that tells you
        // and carries on.
        Ok(name) if !name.trim().is_empty() => {
            static COMPLAINED: std::sync::Once = std::sync::Once::new();
            COMPLAINED.call_once(|| {
                eprintln!(
                    "oslo: {ENV}: {name:?} is not a profile name; \
                     using {}. A name is a letter, then letters, digits, _ or -",
                    default_name()
                );
            });
            default_name()
        }
        _ => default_name(),
    }
}

/// What a shell uses when nothing asked for anything else.
///
/// A fixed name rather than the user's, because the profile is about *what is running the shell*,
/// not who owns it — the store already lives under that user's home. `default` also means a
/// dotfile repo can name it without knowing whose machine it is on.
fn default_name() -> String {
    "default".to_string()
}

/// Whether `name` is a profile name.
///
/// **A letter, then letters, digits, `_` or `-`.** Nothing else, and nothing that starts with a
/// digit or a punctuation mark.
///
/// Restrictive on purpose. The name becomes a file name, so anything looser has to be *escaped*
/// rather than checked — and an escape is a thing that can be got wrong, whereas a name that
/// cannot contain a separator cannot name a file outside the directory however it is handled. It
/// also keeps a profile something a person can say out loud and type again.
pub fn valid(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    // A leading digit would let a profile look like a number, and a leading `-` would look like a
    // flag everywhere the name is passed on.
    first.is_ascii_alphabetic()
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        // Long enough for anything anybody means, short enough to stay a file name everywhere.
        && name.len() <= 64
}

/// `<data>/oslo/history/<profile>/hist.<extension>`.
///
/// **A directory per profile, not a file per profile per kind.** A profile started as one `.kv` and
/// had grown a `.model` and a `.lock` beside it, all three flat in `<data>/oslo/` next to `plugins/`
/// and `direnv/` — so six profiles meant fifteen files in one directory and every new per-profile
/// artifact multiplied by the number of profiles. A directory makes a profile a thing you can copy
/// to another machine or delete outright, which `rm claude.*` only approximated.
pub fn store_path(xdg_data: Option<&str>, home: Option<&str>, extension: &str) -> Option<PathBuf> {
    Some(profile_dir(xdg_data, home, &current())?.join(format!("hist.{extension}")))
}

/// Where one named profile keeps everything: `<data>/oslo/history/<name>/`.
pub fn profile_dir(xdg_data: Option<&str>, home: Option<&str>, name: &str) -> Option<PathBuf> {
    Some(history_dir(xdg_data, home)?.join(name))
}

/// The directory every profile lives under.
pub fn history_dir(xdg_data: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    Some(store_dir(xdg_data, home)?.join("history"))
}

/// oslo's own directory under the data home. `plugins/`, `direnv/` and `history/` are inside it.
pub fn store_dir(xdg_data: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    let base = match xdg_data {
        Some(dir) if !dir.trim().is_empty() => PathBuf::from(dir),
        _ => PathBuf::from(home?).join(".local/share"),
    };
    Some(base.join("oslo"))
}

/// Every profile that has a tracking store, sorted, with the current one always present.
///
/// Found by listing rather than recorded anywhere: a profile *is* a directory with a store in it, so
/// the filesystem is the only thing that could be authoritative. The current profile is included
/// even with nothing written yet, or a brand-new shell would have nothing to switch away from.
pub fn available() -> Vec<String> {
    let mut found: Vec<String> = history_dir(
        std::env::var("XDG_DATA_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    )
    .and_then(|dir| std::fs::read_dir(dir).ok())
    .map(|entries| {
        entries
            .flatten()
            .filter_map(|entry| {
                // The store is what must exist for a profile to be worth showing: a directory
                // somebody made by hand, or one left behind, has nothing the finder can rank.
                entry
                    .path()
                    .join("hist.db")
                    .is_file()
                    .then(|| entry.file_name().to_str().map(str::to_string))
                    .flatten()
            })
            .collect()
    })
    .unwrap_or_default();
    let now = current();
    if !found.contains(&now) {
        found.push(now);
    }
    found.sort();
    found.dedup();
    found
}

/// The profile after `name` in the list, wrapping. `None` when there is only one.
pub fn after(name: &str) -> Option<String> {
    let all = available();
    if all.len() < 2 {
        return None;
    }
    let at = all.iter().position(|found| found == name).unwrap_or(0);
    all.get((at + 1) % all.len()).cloned()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The profile is process-wide state, so the tests that set or read it cannot run beside each
    /// other.
    pub(crate) static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A profile name is a letter, then letters, digits, `_` or `-`. Everything else is refused
    /// rather than cleaned: the name *is* the file, so turning a typo into a different valid name
    /// would write somewhere nobody asked for.
    #[test]
    fn a_name_cannot_escape_the_directory() {
        for hostile in [
            "../../etc/passwd",
            "a/b",
            "..",
            ".",
            "~/x",
            "a\\b",
            "with space",
            "dot.name",
            "9lives",
            "-dash",
            "_under",
            "",
            "   ",
            "emoji-🎉",
        ] {
            assert!(!valid(hostile), "{hostile:?} must not be a profile name");
        }
    }

    /// The ordinary names are accepted — this must not refuse what people actually type.
    #[test]
    fn ordinary_names_are_accepted() {
        for name in ["bresilla", "claude", "agent-1", "test_run", "v2", "a"] {
            assert!(valid(name), "{name:?} should be a profile name");
        }
    }

    /// A name too long to be a comfortable file name is refused rather than truncated.
    #[test]
    fn an_absurd_name_is_refused() {
        assert!(valid(&"a".repeat(64)));
        assert!(!valid(&"a".repeat(65)));
    }

    /// Nothing chosen means `default`, which is what an ordinary shell writes to.
    #[test]
    fn the_default_profile_is_called_default() {
        assert_eq!(default_name(), "default");
    }

    /// **`$OSLO_PROFILE` is the only way to name one.** A hostile or empty value falls back to
    /// the default rather than being cleaned into a different valid name.
    #[test]
    fn the_environment_names_the_profile() {
        // SAFETY: as elsewhere in this crate's tests — see `env::scope::environ`. The lock below
        // is what keeps this from racing the other tests that read the profile.
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());

        unsafe { std::env::set_var(ENV, "from-the-env") };
        assert_eq!(current(), "from-the-env");

        unsafe { std::env::set_var(ENV, "../escape") };
        assert_eq!(current(), "default", "a hostile value is refused");

        // Empty means unset: fall through rather than writing to a file called nothing.
        unsafe { std::env::set_var(ENV, "   ") };
        assert_eq!(current(), "default");

        unsafe { std::env::remove_var(ENV) };
        assert_eq!(current(), "default");
    }

    /// The path is the profile with the store's extension, under the data directory.
    #[test]
    fn the_path_is_the_profile_and_the_extension() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        unsafe { std::env::set_var(ENV, "claude") };
        // The directory carries the profile; the file says only what it is.
        assert_eq!(
            store_path(Some("/x/data"), None, "db"),
            Some(PathBuf::from("/x/data/oslo/history/claude/hist.db"))
        );
        assert_eq!(
            store_path(None, Some("/home/u"), "model"),
            Some(PathBuf::from(
                "/home/u/.local/share/oslo/history/claude/hist.model"
            ))
        );
        unsafe { std::env::remove_var(ENV) };
    }

    /// With nowhere to put it, there is no path — not a path in the wrong place.
    #[test]
    fn no_home_is_no_path() {
        assert_eq!(store_path(None, None, "db"), None);
    }
}
