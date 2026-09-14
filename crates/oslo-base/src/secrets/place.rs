//! Where the files are, and the one store a bare `oslo secret` means.
//!
//! # Two directories with opposite rules
//!
//! The store is under `$XDG_DATA_HOME` and is meant to be committable: everything in it is sealed,
//! and the whole point of encrypting it is that it can live in a dotfiles repository. The key is
//! under `$XDG_STATE_HOME` and must never be — which is why it is not kept beside the thing it
//! opens, and why [`identity_in_a_repository`] exists to say so when somebody's state directory has
//! ended up inside a repository anyway.

use super::{PLUGIN, Store, USER};
use std::io::Write;
use std::path::{Path, PathBuf};

/// `$XDG_DATA_HOME/oslo`, or `~/.local/share/oslo`.
fn data_directory() -> Option<PathBuf> {
    base("XDG_DATA_HOME", ".local/share").map(|base| base.join("oslo"))
}

/// `$XDG_STATE_HOME/oslo`, or `~/.local/state/oslo`.
pub(super) fn state_directory() -> Option<PathBuf> {
    base("XDG_STATE_HOME", ".local/state").map(|base| base.join("oslo"))
}

fn base(variable: &str, fallback: &str) -> Option<PathBuf> {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(fallback)))
}

/// Where a store keeps its files when it does not say.
///
/// The `user` store keeps the directory it has always had. A named one gets its own under
/// `stores/`, and a plugin's under `plugins/`, beside the `.kv` that `oslo.db` makes for it — so
/// uninstalling a plugin stays an `rm -r` rather than a migration.
pub(super) fn default_directory(name: &str) -> Result<PathBuf, String> {
    let oslo = data_directory()
        .ok_or("no $XDG_DATA_HOME and no $HOME, so there is nowhere to keep secrets")?;
    Ok(match name {
        USER => oslo.join("secrets"),
        _ => match name.strip_prefix(PLUGIN) {
            Some(plugin) => oslo.join("plugins").join(format!("{plugin}.secrets")),
            None => oslo.join("stores").join(name),
        },
    })
}

/// `$XDG_DATA_HOME/oslo/secrets`, or `~/.local/share/oslo/secrets`.
pub fn directory() -> Option<PathBuf> {
    default_directory(USER).ok()
}

/// The key file `$OSLO_SECRET_IDENTITY` names, when it names one.
pub(super) fn named_identity() -> Option<PathBuf> {
    std::env::var_os("OSLO_SECRET_IDENTITY")
        .filter(|named| !named.is_empty())
        .map(PathBuf::from)
}

/// Where the private key is: `$OSLO_SECRET_IDENTITY`, else `$XDG_STATE_HOME/oslo/identity`, else
/// `~/.local/state/oslo/identity`.
///
/// **Deliberately not under [`directory`].** The store is meant to be committable, and a key inside
/// it would be committed with it.
pub fn identity_path() -> Option<PathBuf> {
    if let Some(named) = named_identity() {
        return Some(named);
    }
    Some(state_directory()?.join("key"))
}

/// The repository the identity is inside, when it is inside one.
///
/// A key under a `.git` is a key one `git add -A` away from being published, and the person it
/// happens to is never the person who chose it — it is somebody who moved their state directory
/// into a dotfiles repository a year later. Walking up costs one `exists` per parent and is only
/// done when a secret is used.
pub fn identity_in_a_repository() -> Option<PathBuf> {
    let path = identity_path()?;
    let mut here = path.parent()?;
    loop {
        if is_repository(&here.join(".git")) {
            return Some(here.to_path_buf());
        }
        here = here.parent()?;
    }
}

pub(super) use crate::repo::is_repository;

/// Write bytes where only this user can read them, before anything is in them.
///
/// **Bytes, and nothing appended.** It writes ciphertext as well as identities, and a newline added
/// to the end of a sealed file is a file the format does not accept back.
pub(super) fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    file.write_all(bytes)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Where one secret lives in the `user` store.
pub fn path(name: &str) -> Result<PathBuf, String> {
    Store::named(USER)?.path(name)
}

/// Keep `value` under `name` in the `user` store.
pub fn set(name: &str, value: &[u8]) -> Result<(), String> {
    Store::named(USER)?.set(name, value)
}

/// The value kept under `name` in the `user` store.
pub fn get(name: &str) -> Result<Vec<u8>, String> {
    Store::named(USER)?.get(name)
}

/// Every name in the `user` store.
pub fn names() -> Vec<String> {
    Store::named(USER)
        .map(|store| store.names())
        .unwrap_or_default()
}

/// Forget the secret kept under `name` in the `user` store.
pub fn forget(name: &str) -> Result<(), String> {
    Store::named(USER)?.forget(name)
}
