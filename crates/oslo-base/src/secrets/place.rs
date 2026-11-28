//! Where the files are, and the one store a bare `oslo secret` means.
//!
//! # Two directories with opposite rules
//!
//! The store is under `$XDG_DATA_HOME` and is meant to be committable: everything in it is sealed,
//! and the whole point of encrypting it is that it can live in a dotfiles repository. The key is
//! under `$XDG_STATE_HOME` and must never be — which is why it is not kept beside the thing it
//! opens, and why [`in_a_repository`] exists to say so when a key file has ended up inside a
//! repository anyway.
//!
//! Both trees are private: a store directory oslo chose, `$XDG_STATE_HOME/oslo` and its
//! `profiles/` are made at `0700` and tightened to it when oslo next writes there. A `directory`
//! somebody named in `secrets.conf` is made `0700` when missing and otherwise left alone, with a
//! warning when other users can list it.

use super::{PLUGIN, Store, USER};
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

/// Where the key a store with no `key` line uses is: `$OSLO_SECRET_IDENTITY`, else this profile's
/// key, `$XDG_STATE_HOME/oslo/profiles/NAME.key`.
///
/// **Deliberately not under [`directory`].** The store is meant to be committable, and a key inside
/// it would be committed with it.
pub fn identity_path() -> Option<PathBuf> {
    if let Some(named) = named_identity() {
        return Some(named);
    }
    crate::track::profile::key::path(&crate::track::profile::current())
}

/// The repository `path` is inside, when it is inside one.
///
/// A key under a `.git` is a key one `git add -A` away from being published, and the person it
/// happens to is never the person who chose it — it is somebody who moved their state directory
/// into a dotfiles repository a year later. Walking up costs one `exists` per parent and is only
/// done when a secret is used.
pub fn in_a_repository(path: &Path) -> Option<PathBuf> {
    let mut here = path.parent()?;
    loop {
        if is_repository(&here.join(".git")) {
            return Some(here.to_path_buf());
        }
        here = here.parent()?;
    }
}

pub(super) use crate::repo::is_repository;

/// Make `store`'s directory ready to write into, and say so when it is not private.
///
/// The directory oslo chose for the store is made, or tightened to, `0700`. One named in
/// `secrets.conf`, or reached through a symbolic link, is only made when missing: changing the
/// mode of somebody's own directory is theirs to do, so a loose one gets a warning instead.
pub(super) fn prepare(store: &Store) -> Result<(), String> {
    let chosen = default_directory(&store.name).ok() != Some(store.directory.clone());
    match chosen {
        true => crate::private::directory(&store.directory)?,
        false => crate::private::own_directory(&store.directory)?,
    }
    if crate::private::loose(&store.directory) {
        crate::messages::warn(
            "secret",
            format!(
                "{}: other users can list this store's names; `chmod 700` it",
                store.directory.display()
            ),
        );
    }
    Ok(())
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
pub fn get(name: &str) -> Result<zeroize::Zeroizing<Vec<u8>>, String> {
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
