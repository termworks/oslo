//! What sync last knew about each script, so that an edit and a deletion can travel.
//!
//! ```text
//! <data>/oslo/macros/scripts.ledger
//! NAME REVISION live|dead TIE-BREAKER FINGERPRINT
//! ```
//!
//! The files are the scripts; this holds only their stamps. A sync turns the directory into
//! records with [`stamp_into`], merges them with the far end's by the shared stamp rule, and lays
//! the winners back out as files.
//!
//! # The stamps
//!
//! | file, against the ledger | stamp |
//! |---|---|
//! | unchanged | the ledger's |
//! | changed, or new | revision `max(ledger + 1, mtime)`, tie-breaker from the contents |
//! | gone | the ledger's revision + 1, buried |
//!
//! A revision is a modification time, so of two edits the newer one wins. Both rules are pure,
//! which keeps a sync that is run twice from changing anything the second time.

use super::super::{Entry, Kind, Store, encode, key, stored};
use crate::track::stamp::Stamp;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One script as the last sync left it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Line {
    stamp: Stamp,
    fingerprint: String,
}

/// `<data>/oslo/macros/scripts.ledger`.
pub fn path() -> Option<PathBuf> {
    Some(super::super::directory()?.join("scripts.ledger"))
}

fn load(path: &Path) -> BTreeMap<String, Line> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    text.lines().filter_map(parse).collect()
}

fn parse(line: &str) -> Option<(String, Line)> {
    let mut fields = line.split(' ');
    let name = fields.next()?.to_string();
    let revision = fields.next()?.parse().ok()?;
    let deleted = fields.next()? == "dead";
    let tie_breaker = super::super::unhex(fields.next()?)?;
    let fingerprint = fields.next()?.to_string();
    let stamp = Stamp {
        revision,
        deleted,
        tie_breaker,
    };
    Some((name, Line { stamp, fingerprint }))
}

fn save(path: &Path, lines: &BTreeMap<String, Line>) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let text: String = lines
        .iter()
        .map(|(name, line)| {
            format!(
                "{name} {} {} {} {}\n",
                line.stamp.revision,
                if line.stamp.deleted { "dead" } else { "live" },
                super::super::hex(&line.stamp.tie_breaker),
                line.fingerprint
            )
        })
        .collect();
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// A digest of what a script is: on or off, and its body.
fn digest(entry: &Entry) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update([u8::from(entry.active)]);
    hasher.update(entry.body.as_bytes());
    hasher.finalize().into()
}

fn fingerprint(entry: &Entry) -> String {
    digest(entry)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn tie_breaker(entry: &Entry) -> [u8; 16] {
    let mut tie = [0; 16];
    tie.copy_from_slice(&digest(entry)[..16]);
    tie
}

/// Every script with the stamp it carries now, the deleted ones as tombstones.
fn reconcile(ledger: &BTreeMap<String, Line>) -> Vec<Entry> {
    let present = super::all();
    let mut out: Vec<Entry> = Vec::new();
    for entry in &present {
        let modified = super::path_of(&entry.name)
            .map(|path| super::modified(&path))
            .unwrap_or(0);
        let stamp = match ledger.get(&entry.name) {
            Some(line) if !line.stamp.deleted && line.fingerprint == fingerprint(entry) => {
                line.stamp
            }
            known => Stamp {
                revision: known
                    .map(|line| line.stamp.revision.saturating_add(1))
                    .unwrap_or(1)
                    .max(modified),
                deleted: false,
                tie_breaker: tie_breaker(entry),
            },
        };
        out.push(Entry {
            stamp,
            ..entry.clone()
        });
    }
    for (name, line) in ledger {
        if present.iter().any(|entry| &entry.name == name) {
            continue;
        }
        let mut stamp = line.stamp;
        if !stamp.deleted {
            stamp.revision = stamp.revision.saturating_add(1);
            stamp.deleted = true;
        }
        out.push(Entry {
            stamp,
            ..Entry::new(Kind::Script, name, "")
        });
    }
    out
}

/// Make the script records in `store` exactly the directory's, stamped against the ledger.
pub fn stamp_into(store: &Store) -> Result<(), String> {
    stamp_into_with(store, &path().ok_or(NOWHERE)?)
}

pub(in crate::macros) const NOWHERE: &str =
    "no $XDG_DATA_HOME and no $HOME, so there is nowhere to keep the ledger";

/// [`stamp_into`], against the ledger at `ledger`.
pub(in crate::macros) fn stamp_into_with(store: &Store, ledger: &Path) -> Result<(), String> {
    let prefix = format!("{}/", Kind::Script.word());
    for stale in crate::store::keys(store, &prefix) {
        crate::store::delete(store, &stale);
    }
    for entry in reconcile(&load(ledger)) {
        crate::store::set(
            store,
            &key(Kind::Script, &entry.name),
            encode(&entry).as_bytes(),
        )?;
    }
    Ok(())
}

/// Lay the script records in `store` out as files, and save their stamps as the ledger at `path`.
pub(in crate::macros) fn absorb_with(store: &Store, path: &Path) -> Result<(), String> {
    let prefix = format!("{}/", Kind::Script.word());
    let mut ledger: BTreeMap<String, Line> = BTreeMap::new();
    for full in crate::store::keys(store, &prefix) {
        let Some(name) = full.strip_prefix(&prefix) else {
            continue;
        };
        let Some(entry) = stored(store, Kind::Script, name) else {
            continue;
        };
        if entry.stamp.deleted {
            super::remove(name);
        } else if super::get(name).map(|here| (here.body, here.active))
            != Some((entry.body.clone(), entry.active))
        {
            super::write(&entry)?;
        }
        ledger.insert(
            name.to_string(),
            Line {
                stamp: entry.stamp,
                fingerprint: fingerprint(&entry),
            },
        );
    }
    save(path, &ledger)
}

#[cfg(test)]
#[path = "ledger/tests.rs"]
mod tests;
