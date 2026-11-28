//! Where a store's keys come from.
//!
//! # Two kinds, in a deliberate order
//!
//! A [`KeySource::File`] is read; a [`KeySource::Command`] is *run*, and its standard output is the
//! key. The second exists because the alternative is compiling every way a person might hold a
//! key into a shell that is meant to be `/bin/sh` — a password manager, a smartcard wrapper,
//! whatever they already use. It costs nothing when it is not configured.
//!
//! Native sources are always tried first, and a store that decrypts with a file key never runs
//! anything: `age`'s decryptor stops at the first identity that matches, so the ordering is a
//! latency property and a security one at once.
//!
//! # What is fenced, and how
//!
//! * **argv, never a shell string.** Nothing reaches `/bin/sh`, so there is no quoting layer to get
//!   wrong and no `$(…)` in a configuration file.
//! * **Never in a `plugin.*` store.** A plugin's own store cannot fork; that is a decision its
//!   owner makes at a command line.
//! * **`$OSLO_SECRET_NO_EXEC`** set to anything non-empty skips every command source and names it
//!   in the failure. Exported once by a cron job or a container, inherited by every child, it makes
//!   "this will not fork" something to assert rather than infer.
//! * **Standard error is the terminal's**, so a "touch your key" prompt is seen, and the run is
//!   bounded by the store's `key timeout` and a cap on what it prints. See [`super::child`].
//!
//! # A file is a path that means one place
//!
//! `key file` takes an absolute path or one that starts with `~/`, which is `$HOME`. A relative
//! one is refused: it would mean a different file in every directory the shell is run from.

use std::path::PathBuf;
#[cfg(feature = "crypt")]
use std::time::Duration;
#[cfg(feature = "crypt")]
use zeroize::Zeroizing;

/// The most a key file, or a key command's output, may hold.
#[cfg(feature = "crypt")]
const SMALL: u64 = crate::private::SMALL;

/// Where one key comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeySource {
    /// The profile's own key, with this store's key derived from it.
    ///
    /// **The default, and the reason a fresh install needs to be told nothing.** A profile already
    /// has a key — it is what lets two machines agree they share a history — so a store derives its
    /// own from that rather than inventing a second secret to keep track of. Carrying the profile
    /// to another machine therefore carries the secrets with it, which is the same one step.
    Profile,
    File(PathBuf),
    Command(Vec<String>),
}

impl KeySource {
    /// `file PATH`, or `command ARG…`, as written after `key` in `secrets.conf`.
    pub fn parse(rest: &str) -> Result<Self, String> {
        let (kind, rest) = super::conf::split_word(rest);
        match kind {
            "profile" => Ok(KeySource::Profile),
            "file" if !rest.is_empty() => {
                super::conf::absolute("key file", rest).map(KeySource::File)
            }
            "command" if !rest.is_empty() => {
                let argv: Vec<String> = rest.split_whitespace().map(str::to_string).collect();
                Ok(KeySource::Command(argv))
            }
            "file" | "command" => Err(format!("`key {kind}` needs something after it")),
            other => Err(format!(
                "a key is `profile`, `file` or `command`, not {:?}",
                super::recipient::shown(other)
            )),
        }
    }

    /// How it is written in the file.
    pub fn line(&self) -> String {
        match self {
            KeySource::Profile => "key profile".to_string(),
            KeySource::File(path) => format!("key file {}", path.display()),
            KeySource::Command(argv) => format!("key command {}", argv.join(" ")),
        }
    }

    /// What kind it is, as the start of its line: `key profile`, `key file` or `key command`.
    pub fn kind(&self) -> &'static str {
        match self {
            KeySource::Profile => "key profile",
            KeySource::File(_) => "key file",
            KeySource::Command(_) => "key command",
        }
    }

    /// Whether reaching this key means running another program.
    pub fn is_external(&self) -> bool {
        matches!(self, KeySource::Command(_))
    }

    /// The key, or why not, giving a command `timeout` to answer.
    ///
    /// A file that is not there is `Ok(None)` rather than an error: several key sources are a list
    /// of places to look, and a laptop that has one of them is not misconfigured. Anything else —
    /// unreadable, not a regular file, not a key — is an error, and is never read as "not there".
    #[cfg(feature = "crypt")]
    pub fn key(&self, timeout: Duration) -> Result<Option<super::native::SecretKey>, String> {
        use crate::private::{Links, read};
        match self {
            KeySource::Profile => Ok(None),
            KeySource::File(path) => match read(path, SMALL, Links::Follow)?.map(Zeroizing::new) {
                Some(bytes) => std::str::from_utf8(&bytes)
                    .map_err(|_| "not text".to_string())
                    .and_then(super::native::read_secret)
                    .map(Some)
                    .map_err(|e| format!("{}: {e}", path.display())),
                None => Ok(None),
            },
            KeySource::Command(argv) => KeySource::run(argv, timeout).map(Some),
        }
    }

    #[cfg(feature = "crypt")]
    fn run(argv: &[String], timeout: Duration) -> Result<super::native::SecretKey, String> {
        if no_exec() {
            return Err(format!(
                "$OSLO_SECRET_NO_EXEC is set, so `{}` was not run",
                argv.join(" ")
            ));
        }
        let program = argv.first().map_or("", String::as_str);
        let output = super::child::run(argv, None, timeout, SMALL as usize)?;
        let text = Zeroizing::new(
            String::from_utf8(output)
                .map_err(|_| format!("{program}: what it printed is not a key"))?,
        );
        super::native::read_secret(&text).map_err(|e| format!("{program}: {e}"))
    }
}

/// Whether every external source is to be skipped.
pub fn no_exec() -> bool {
    std::env::var_os("OSLO_SECRET_NO_EXEC").is_some_and(|value| !value.is_empty())
}

/// Make one where `path` says, mode `0600` from the moment it exists.
///
/// **Never over anything.** The file is linked into place without replacing, so a key that is
/// already there — readable or not, a file or a link — is refused rather than destroyed. Missing
/// directories are made at `0700`; an existing one is left as its owner made it.
#[cfg(feature = "crypt")]
pub fn generate(path: &std::path::Path) -> Result<super::native::SecretKey, String> {
    let directory = path
        .parent()
        .ok_or_else(|| format!("{}: has no directory to be in", path.display()))?;
    crate::private::directory(directory)?;
    let fresh = super::native::generate_secret()?;
    match crate::private::create(path, super::native::write_secret(&fresh).as_bytes())? {
        true => Ok(fresh),
        false => Err(format!(
            "{}: something is already there, so no key was made over it",
            path.display()
        )),
    }
}
