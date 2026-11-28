//! Secrets the shell can hold: encrypted at rest with age, decrypted only when something asks.
//!
//! # Why a shell at all
//!
//! Every secret a person keeps ends up in a shell eventually — exported for one command, pasted
//! into a `curl`, read out of a `.env` that should never have been written. The shell is where they
//! are *used*, and it is the one program in the chain that can hold a value in memory, hand it to
//! one command, and never write it down. Anything outside the shell has to put the value somewhere
//! the shell can read it, which means a file, which means a plaintext file.
//!
//! # A key and an AEAD, not a keyring
//!
//! [age](https://github.com/str4d/rage) is a file encryption format with one key type, no options
//! to get wrong, and a Rust implementation with no C in it — which is the only kind that can go in
//! a static musl binary that is also `/bin/sh`. A system keyring is the alternative and is the
//! wrong shape here: it needs a daemon, a session bus and a desktop, none of which exist on a
//! server, in a container, or in the initramfs where this shell is supposed to work.
//!
//! # A store is three things
//!
//! A [`Store`] is a directory, an ordered list of [`KeySource`]s, and the [`Crypto`] that seals and
//! opens its files — oslo's own, another program, or a Lua hook. Every one of them is configurable
//! in [`conf`]; an install that has configured nothing gets one store called `user` and one key
//! file, and never has to be told anything.
//!
//! # What is on disk, and why the key is not next to it
//!
//! ```text
//! ~/.local/share/oslo/secrets/NAME.sealed       one secret, encrypted — safe to commit
//! ~/.local/share/oslo/secrets/.lock             held while a write reads and replaces a file
//! ~/.local/state/oslo/profiles/default.key      the profile's key, mode 0600 — never
//! ~/.local/state/oslo/secrets.conf              which stores exist — beside the key, see `conf`
//! ```
//!
//! Both directories are `0700`, every file `0600`, and every write lands through a scratch file and
//! a rename — see [`crate::private`].
//!
//! **Two directories, because they have opposite rules.** The point of encrypting a store is that
//! the store can then go where files go — a dotfiles repository, a backup, a synced directory. A
//! key sitting inside it turns that from a feature into the worst kind of accident: the ciphertext
//! and the thing that opens it, committed together, in one `git add -A`.
//!
//! So the key lives under `$XDG_STATE_HOME` — a different tree, whose whole definition is state a
//! machine keeps to itself — and `$OSLO_SECRET_IDENTITY` moves it anywhere else: a USB stick, an
//! encrypted volume, `~/.ssh` beside the other private keys. [`Store::key_paths`] names every file
//! a store reads a key from, and `in_a_repository` is the check that says so out loud when one
//! turns out to be under a `.git` anyway — because a home directory that is itself a repository is
//! a thing people do.
//!
//! The identity is a file at all because the alternative is asking for a passphrase on every read,
//! and a shell that asks fifty times a day teaches you to keep the value somewhere else. Protecting
//! it is the filesystem's job, the same as `~/.ssh/id_ed25519`.

pub mod child;
pub mod cipher;
pub mod conf;
pub mod crypto;
pub mod hooked;
pub mod key;
pub mod name;
#[cfg(feature = "crypt")]
pub mod native;
// Where the files are: the store, the key, and why those are two different directories.
pub mod place;
pub mod recipient;
// What lets a store travel: the stamp each file carries, and the merge that reads it. Deliberately
// outside the sealed body, so a machine with no key for this store can still carry it.
pub mod sync;
mod write;

use place::{default_directory, named_identity};
pub use place::{directory, forget, get, identity_path, in_a_repository, names, path, set};

use std::path::PathBuf;
use std::time::Duration;

pub use cipher::Cipher;
pub use crypto::Crypto;
pub use key::KeySource;
pub use recipient::Recipient;
use sync::Kept;

/// The most a store file may hold, and the most a cipher command may print: 16 MiB.
pub const LARGEST: u64 = 16 << 20;

/// What one secret's file is called, after its name.
///
/// Not `.age`: these have not been age files since oslo grew a mechanism of its own, and a suffix
/// that named a format it is not would send somebody to the wrong tool to open one.
pub const KEPT: &str = ".sealed";

/// The store every install has, and the one `oslo secret` means when nothing says otherwise.
pub const USER: &str = "user";

/// A plugin's own store is named for it, under a prefix Lua cannot type.
pub const PLUGIN: &str = "plugin.";

/// A directory, what opens it, and who it is written for.
#[derive(Debug, Clone)]
pub struct Store {
    pub name: String,
    pub directory: PathBuf,
    pub keys: Vec<KeySource>,
    /// Who its files are written for. Empty means "just me": the public half of the first key.
    pub recipients: Vec<Recipient>,
    /// What seals and opens its files: oslo's own age, another program, or a Lua hook.
    pub crypto: Crypto,
    /// How long a key or cipher command may run: `key timeout`, else [`child::DEFAULT_TIMEOUT`].
    pub timeout: Duration,
}

impl Store {
    /// The store called `name`, as `secrets.conf` has it, filled in with the defaults for whatever
    /// it does not say.
    pub fn named(name: &str) -> Result<Store, String> {
        conf::valid_store_name(name)?;
        let declared = conf::read()?;
        let section = declared.section(name);
        let mut keys = section.map(|s| s.keys.clone()).unwrap_or_default();
        if keys.is_empty() {
            // **The profile, not a file of its own.** A store that says nothing takes its key from
            // the profile it belongs to, so one exported profile carries the history *and* the
            // secrets, and a machine has one secret on it rather than two.
            //
            // `$OSLO_SECRET_IDENTITY` still wins, because it is somebody saying *this file* — a
            // USB stick, an encrypted volume — and a variable that quietly stopped meaning anything
            // would be worse than one that never existed.
            keys.push(match named_identity() {
                Some(path) if path.is_relative() => {
                    return Err(format!(
                        "$OSLO_SECRET_IDENTITY is {}, a relative path; give it an absolute one",
                        path.display()
                    ));
                }
                Some(path) => KeySource::File(path),
                None => KeySource::Profile,
            });
        }
        let recipients = match section {
            Some(section) => section
                .recipients
                .iter()
                .map(|text| Recipient::new(text))
                .collect::<Result<Vec<_>, _>>()?,
            None => Vec::new(),
        };
        let cipher = section.map(|s| s.cipher.clone()).unwrap_or_default();
        let crypto = Crypto::of(section.is_some_and(|s| s.hooked), &cipher)
            .map_err(|e| format!("{name}: {e}"))?;
        if name.starts_with(PLUGIN)
            && (keys.iter().any(KeySource::is_external) || cipher.is_external())
        {
            return Err(format!("{name}: a plugin's store may not run a command"));
        }
        Ok(Store {
            directory: match section.and_then(|s| s.directory.clone()) {
                Some(directory) => directory,
                None => default_directory(name)?,
            },
            name: name.to_string(),
            keys,
            recipients,
            crypto,
            timeout: section
                .and_then(|s| s.timeout)
                .unwrap_or(child::DEFAULT_TIMEOUT),
        })
    }

    /// The store this invocation means: what was asked for, else `$OSLO_SECRET_STORE`, else the
    /// `default` line, else [`USER`].
    pub fn selected(asked: Option<&str>) -> Result<Store, String> {
        if let Some(name) = asked {
            return Store::named(name);
        }
        if let Some(name) = std::env::var_os("OSLO_SECRET_STORE")
            && !name.is_empty()
        {
            return Store::named(&name.to_string_lossy());
        }
        match conf::read()?.default {
            Some(name) => Store::named(&name),
            None => Store::named(USER),
        }
    }

    /// Why this store cannot be used *in this process*, when that is the case.
    ///
    /// **Asked before anything touches a file**, or a hook-backed store in a script would report a
    /// missing file — true, and not the reason. Only `crypto hook` can be unreachable: age and a
    /// command both work wherever the binary does.
    pub fn unreachable_here(&self) -> Option<String> {
        match self.crypto {
            Crypto::Hook => hooked::unreachable_here(&self.name),
            _ => None,
        }
    }

    /// Where one secret lives.
    ///
    /// A name is a filename, so it may not reach out of the directory it is written into: `..` and
    /// `/` are refused rather than sanitised, because a secret quietly stored somewhere other than
    /// where it was asked for is worse than an error. See [`name`] for the rest of what is refused.
    pub fn path(&self, name: &str) -> Result<PathBuf, String> {
        name::check(name)?;
        Ok(self.directory.join(format!("{name}{KEPT}")))
    }

    /// Keep `value` under `name`.
    pub fn set(&self, name: &str, value: &[u8]) -> Result<(), String> {
        if let Some(why) = self.unreachable_here() {
            return Err(why);
        }
        let path = self.path(name)?;
        let sealed = self.sealed(name, value)?;
        self.put(&path, sealed, None).map(|_| ())
    }

    /// The value kept under `name`.
    pub fn get(&self, name: &str) -> Result<Vec<u8>, String> {
        if let Some(why) = self.unreachable_here() {
            return Err(why);
        }
        let path = self.path(name)?;
        // A tombstone is a name that is gone, and reads like one rather than like a file full of
        // nothing; so does a file that is not there at all.
        match sync::read_kept(&path)? {
            Some(kept) if !kept.stamp.deleted => self.opened(name, &kept.sealed),
            _ => Err(self.missing(name)),
        }
    }

    /// Every name kept here, sorted. Tombstones are not names anybody has, and nor is a file that is
    /// not a regular one or whose header is damaged.
    pub fn names(&self) -> Vec<String> {
        sync::every_name(self)
            .into_iter()
            .filter(|name| {
                self.path(name)
                    .ok()
                    .and_then(|path| sync::read_kept(&path).ok().flatten())
                    .is_some_and(|kept| !kept.stamp.deleted)
            })
            .collect()
    }

    /// Forget the secret kept under `name`.
    ///
    /// **A tombstone, not an erasure**, so that the removal survives the next sync instead of the
    /// other machine putting it back. The sealed body is dropped — there is no reason to keep
    /// ciphertext for something deleted — and what remains is the name and a stamp saying it is
    /// gone. See [`crate::track::stamp`]. Success means the tombstone is on disk.
    pub fn forget(&self, name: &str) -> Result<(), String> {
        let path = self.path(name)?;
        let alive = |kept: &Kept| !kept.stamp.deleted;
        // Looked at before the lock, so forgetting a name nobody has makes no directory for it.
        if !sync::read_kept(&path)?.is_some_and(|kept| alive(&kept)) {
            return Err(self.missing(name));
        }
        let _held = self.lock()?;
        let Some(kept) = sync::read_kept(&path)?.filter(alive) else {
            return Err(self.missing(name));
        };
        let mut stamp = kept.stamp;
        stamp
            .bury()
            .ok_or_else(|| write::pinned(&path, &kept.stamp))?;
        let tombstone = Kept {
            stamp,
            sealed: Vec::new(),
        };
        sync::write_kept(&path, &tombstone)
    }

    /// Encrypt to every recipient, without storing it anywhere.
    ///
    /// **A store with an `encrypt command` never reaches oslo's own age**: who that program
    /// encrypts to, and in what format, is its business. That is what makes a key oslo cannot
    /// compute — one held in hardware — usable at all.
    pub fn seal(&self, value: &[u8]) -> Result<Vec<u8>, String> {
        self.seal_named("", value)
    }

    /// The same, telling whatever does the work which secret this is.
    ///
    /// The name is empty for an anonymous seal — `oslo.secret.seal`, which encrypts something the
    /// caller keeps somewhere else of its own.
    ///
    /// **A store with only a `decrypt command` cannot write**, rather than writing with oslo's own
    /// crypto: what that store's files are sealed to is the other program's business.
    pub fn seal_named(&self, name: &str, value: &[u8]) -> Result<Vec<u8>, String> {
        match &self.crypto {
            Crypto::Hook => hooked::encrypt(&self.name, name, value),
            Crypto::Command(cipher) => match &cipher.encrypt {
                Some(argv) => cipher::through_within(argv, value, self.timeout),
                None => Err(self.half("decrypt", "encrypt", "write")),
            },
            Crypto::Native => self.seal_natively(value),
        }
    }

    /// Why a store that hands over one half of its crypto cannot do the other.
    fn half(&self, has: &str, lacks: &str, cannot: &str) -> String {
        format!(
            "{}: it has a `{has} command` and no `{lacks} command`, so it cannot {cannot}; \
             `oslo secret cipher {lacks} -- …` names one",
            self.name
        )
    }

    /// oslo's own, in this process.
    #[cfg(feature = "crypt")]
    fn seal_natively(&self, value: &[u8]) -> Result<Vec<u8>, String> {
        native::seal(&self.encrypt_to()?, value)
    }

    /// This build carries no crypto of its own, so a store has to name one.
    #[cfg(not(feature = "crypt"))]
    fn seal_natively(&self, _value: &[u8]) -> Result<Vec<u8>, String> {
        Err(no_native(&self.name))
    }

    /// Decrypt with whichever key opens it.
    ///
    /// **Native sources first, and external ones only if none of them matched.** `age` stops at the
    /// first identity that fits, so a store whose file key opens the file never runs the program
    /// another key source names — no `$PATH` walk, no fork, and a cron job on a machine that cannot
    /// reach the other key degrades instead of hanging on it.
    pub fn unseal(&self, ciphertext: &[u8]) -> Result<Vec<u8>, String> {
        self.unseal_named("", ciphertext)
    }

    /// The same, telling whatever does the work which secret this is.
    pub fn unseal_named(&self, name: &str, ciphertext: &[u8]) -> Result<Vec<u8>, String> {
        match &self.crypto {
            Crypto::Hook => hooked::decrypt(&self.name, name, ciphertext),
            Crypto::Command(cipher) => match &cipher.decrypt {
                Some(argv) => cipher::through_within(argv, ciphertext, self.timeout),
                None => Err(self.half("encrypt", "decrypt", "read")),
            },
            Crypto::Native => self.unseal_natively(ciphertext),
        }
    }

    /// oslo's own, in this process.
    ///
    /// **File keys before program keys**, so a store that opens with a file never runs the program
    /// another key source names: no `$PATH` walk, no fork, and a cron job on a machine that cannot
    /// reach the other key degrades instead of hanging on it.
    #[cfg(feature = "crypt")]
    fn unseal_natively(&self, ciphertext: &[u8]) -> Result<Vec<u8>, String> {
        let mut tried = 0;
        let mut failed = Vec::new();
        for external in [false, true] {
            for answer in self.keys_of(external) {
                let Ok(key) = answer.map_err(|e| failed.push(e)) else {
                    continue;
                };
                tried += 1;
                if let Ok(value) = native::unseal(&key, ciphertext) {
                    return Ok(value);
                }
            }
        }
        Err(self.why_it_did_not_open(tried, &failed))
    }

    #[cfg(not(feature = "crypt"))]
    fn unseal_natively(&self, _ciphertext: &[u8]) -> Result<Vec<u8>, String> {
        Err(no_native(&self.name))
    }

    /// What each key source of one kind answers, in order, run only as far as it is read: a key, or
    /// what went wrong, labelled with the source. A source with nothing there answers nothing.
    #[cfg(feature = "crypt")]
    pub fn keys_of(&self, external: bool) -> impl Iterator<Item = Result<[u8; 32], String>> + '_ {
        self.keys
            .iter()
            .filter(move |source| source.is_external() == external)
            .filter_map(|source| {
                let answer = match source {
                    // Only the store knows its own name, and the name is the salt — so this one is
                    // resolved here rather than by the key source.
                    KeySource::Profile => self.profile_key().and_then(|profile| {
                        profile
                            .map(|profile| native::derive_store_key(&profile, &self.name))
                            .transpose()
                    }),
                    other => other.key(self.timeout),
                };
                match answer {
                    Ok(found) => found.map(Ok),
                    Err(e) => Some(Err(format!("{} {e}", source.kind()))),
                }
            })
    }

    /// The first key file this store would look in, which is where `key init` makes one.
    pub fn key_file(&self) -> Option<PathBuf> {
        self.keys.iter().find_map(|source| match source {
            KeySource::File(path) => Some(path.clone()),
            KeySource::Profile | KeySource::Command(_) => None,
        })
    }

    /// Who this store writes for.
    ///
    /// **A store with no `recipient` line is written for its own key**, which is what makes a fresh
    /// install work without being told anything — and adding the first recipient is the moment that
    /// stops being implied, which is why the command writes the implied one down first.
    #[cfg(feature = "crypt")]
    pub fn encrypt_to(&self) -> Result<Vec<[u8; 32]>, String> {
        if !self.recipients.is_empty() {
            return self.recipients.iter().map(Recipient::public).collect();
        }
        Ok(vec![native::public_of(&self.key_to_write_with()?)])
    }

    /// This profile's key, if it has one.
    ///
    /// A plugin's store belongs to the same profile as everything else: the prefix names which
    /// plugin, not which identity.
    #[cfg(feature = "crypt")]
    fn profile_key(&self) -> Result<Option<[u8; 32]>, String> {
        crate::track::profile::key::read(&crate::track::profile::current())
    }

    /// The key this store writes with, made now if this is the first time.
    ///
    /// The first that answers, file keys before program ones — so a store with a spare key on a USB
    /// stick still writes with the one it always used. **A source that fails before one answers
    /// stops the write**: writing with the spare instead would seal the file to a key its owner
    /// may not have to hand.
    ///
    /// **A key is made only when every source said "not there".** Never over a file that failed to
    /// read, and never for a store with a `key command`: that key lives somewhere oslo cannot see.
    #[cfg(feature = "crypt")]
    pub fn key_to_write_with(&self) -> Result<[u8; 32], String> {
        for external in [false, true] {
            if let Some(answer) = self.keys_of(external).next() {
                return answer.map_err(|e| {
                    format!(
                        "{}: {e}; nothing was written, and no key was made",
                        self.name
                    )
                });
            }
        }
        if self.keys.iter().any(KeySource::is_external) {
            return Err(format!(
                "{}: no key answered, and a store with a `key command` never has one made for it",
                self.name
            ));
        }
        // Nothing is there, so this is the first write. A store that takes its key from the
        // profile makes the *profile's* key rather than one of its own — which is what `oslo secret
        // set` does on a machine that has never been told anything, and is the same key
        // `oslo profile export` then carries.
        if self
            .keys
            .iter()
            .any(|source| matches!(source, KeySource::Profile))
        {
            let profile = crate::track::profile::current();
            let made = crate::track::profile::key::obtain(&profile)
                .map_err(|e| format!("{}: {e}", self.name))?;
            return native::derive_store_key(&made, &self.name);
        }
        match self.key_file() {
            Some(path) => key::generate(&path).or_else(|e| {
                // Another process made it between the look and the link: use theirs.
                KeySource::File(path)
                    .key(self.timeout)
                    .ok()
                    .flatten()
                    .ok_or(e)
            }),
            None => Err(format!(
                "{}: no key, and no key file to make one in",
                self.name
            )),
        }
    }

    /// Every key file this store reads: each `key file`, and the profile's key for `key profile`.
    pub fn key_paths(&self) -> Vec<PathBuf> {
        self.keys
            .iter()
            .filter_map(|source| match source {
                KeySource::File(path) => Some(path.clone()),
                KeySource::Profile => {
                    crate::track::profile::key::path(&crate::track::profile::current())
                }
                KeySource::Command(_) => None,
            })
            .collect()
    }

    /// Why nothing opened it, in the terms of what this store was told to try, naming every source
    /// that failed rather than answered.
    #[cfg(feature = "crypt")]
    fn why_it_did_not_open(&self, tried: usize, failed: &[String]) -> String {
        let skipped = key::no_exec() && self.keys.iter().any(KeySource::is_external);
        if skipped {
            return format!(
                "no key opened it. $OSLO_SECRET_NO_EXEC is set, so {} of {} key sources were not run",
                self.keys.iter().filter(|s| s.is_external()).count(),
                self.keys.len()
            );
        }
        let said = match tried {
            0 => format!("{}: no key to open it with", self.name),
            _ => format!("no key opened it, of the {tried} this store has"),
        };
        match failed.is_empty() {
            true => said,
            false => format!("{said}; {}", failed.join("; ")),
        }
    }
}

/// What a build with no crypto of its own says when a store did not name one.
///
/// **Named rather than implied.** A build without `age` has the whole of the filing — the store, the
/// names, `run`, the lazy variable, the Lua API — and none of the encryption, so the one thing it
/// cannot do has to say which one thing that is.
#[cfg(not(feature = "crypt"))]
fn no_native(store: &str) -> String {
    format!(
        "{store}: this oslo has no built-in crypto. Give the store an `encrypt command` and a \
         `decrypt command` — `age`, `gpg`, `systemd-creds`, whatever this machine already has — \
         or `crypto hook` to do it in Lua"
    )
}

#[cfg(test)]
#[path = "secrets/tests.rs"]
mod tests;

#[cfg(all(test, feature = "crypt"))]
#[path = "secrets/hardening_tests.rs"]
mod hardening_tests;
