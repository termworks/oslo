//! Merging two secret stores, both ways at once.
//!
//! # The stamp is outside the ciphertext, on purpose
//!
//! **Syncing must not need the key.** A store whose crypto is a YubiKey works on the machine the
//! key is plugged into and nowhere else — but the *other* machine still has to be able to carry its
//! files, or the one store you most want backed up is the one that cannot travel. So each file
//! carries a plaintext header before its sealed body:
//!
//! ```text
//! OSLOSEC1 3 live 9f1c…            (32 hex characters)
//! <the sealed bytes, exactly as the crypto produced them>
//! ```
//!
//! A revision, a tombstone flag and a tie-breaker — [`crate::track::stamp`]'s three fields, and
//! nothing about what the secret is. A file with no header is one written before there were any,
//! and is read as revision one.
//!
//! # What that costs, said plainly
//!
//! The header is not authenticated, because authenticating it would mean holding the key to read
//! it. Somebody who can *write* to your store can therefore roll a revision back, or flip a
//! tombstone, and make a sync carry the wrong answer. They cannot read anything — the body is still
//! sealed — and somebody with write access to the store could already delete the file outright. The
//! store is the thing you may commit and copy about; the key is not, and that is the boundary that
//! matters.
//!
//! # A tombstone keeps its name and drops its body
//!
//! Deleting writes a header with `dead` and no body. The name stays visible to a sync and invisible
//! to everything else, which is what makes the deletion travel — see [`crate::track::stamp`] for why
//! removing the file instead would let the other machine hand it back.

use super::{KEPT, Store};
pub use crate::track::stamp::Stamp;
use crate::track::stamp::{Verdict, settle};
use std::path::Path;

/// What a stamped file starts with.
const HEADER: &str = "OSLOSEC1 ";

/// One secret as it sits on disk: what it is worth in a sync, and the bytes nobody here can read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kept {
    pub stamp: Stamp,
    /// The sealed body. Empty for a tombstone.
    pub sealed: Vec<u8>,
}

/// What a merge did, in each direction.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SecretReport {
    pub added_left: usize,
    pub updated_left: usize,
    pub deleted_left: usize,
    pub added_right: usize,
    pub updated_right: usize,
    pub deleted_right: usize,
    pub unchanged: usize,
}

impl SecretReport {
    pub fn quiet(&self) -> bool {
        self.added_left == 0
            && self.updated_left == 0
            && self.deleted_left == 0
            && self.added_right == 0
            && self.updated_right == 0
            && self.deleted_right == 0
    }
}

/// A stamp and a body, as they are written to a file.
pub fn wrap(kept: &Kept) -> Vec<u8> {
    let mut bytes = format!(
        "{HEADER}{} {} {}\n",
        kept.stamp.revision,
        if kept.stamp.deleted { "dead" } else { "live" },
        hex(&kept.stamp.tie_breaker)
    )
    .into_bytes();
    bytes.extend_from_slice(&kept.sealed);
    bytes
}

/// The other way. A file with no header of ours is a body written before there were stamps, and
/// gets revision one — which is the truth about it: it has never been synced.
pub fn unwrap(bytes: &[u8]) -> Kept {
    let unstamped = |sealed: &[u8]| Kept {
        stamp: Stamp::first().unwrap_or(Stamp {
            revision: 1,
            deleted: false,
            tie_breaker: [0; 16],
        }),
        sealed: sealed.to_vec(),
    };
    if !bytes.starts_with(HEADER.as_bytes()) {
        return unstamped(bytes);
    }
    let Some(end) = bytes.iter().position(|byte| *byte == b'\n') else {
        return unstamped(bytes);
    };
    let Ok(header) = std::str::from_utf8(&bytes[HEADER.len()..end]) else {
        return unstamped(bytes);
    };
    let mut fields = header.split(' ');
    let stamp = match (fields.next(), fields.next(), fields.next()) {
        (Some(revision), Some(state), Some(tie)) => Stamp {
            revision: revision.parse().unwrap_or(1),
            deleted: state == "dead",
            tie_breaker: unhex(tie).unwrap_or([0; 16]),
        },
        _ => return unstamped(bytes),
    };
    Kept {
        stamp,
        sealed: bytes[end + 1..].to_vec(),
    }
}

fn hex(bytes: &[u8; 16]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Option<[u8; 16]> {
    if text.len() != 32 {
        return None;
    }
    let mut bytes = [0; 16];
    for (at, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(at * 2..at * 2 + 2)?, 16).ok()?;
    }
    Some(bytes)
}

/// What is in `path`, or `None` when there is no file there.
pub fn read_kept(path: &Path) -> Option<Kept> {
    std::fs::read(path).ok().map(|bytes| unwrap(&bytes))
}

/// Write one, privately, through a rename so a reader sees the old file or the new one.
pub fn write_kept(path: &Path, kept: &Kept) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let scratch = path.with_extension("new");
    super::write_private(&scratch, &wrap(kept))?;
    std::fs::rename(&scratch, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Every name a store holds, tombstones included.
pub fn every_name(store: &Store) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(&store.directory) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.strip_suffix(KEPT).map(str::to_string)
        })
        .collect();
    names.sort();
    names
}

/// Merge two stores so that both hold the same secrets.
///
/// **Nothing is decrypted.** The sealed bodies are moved as they stand, which is what lets a machine
/// carry a store it has no key for — and means a sync never has the plaintext in memory at all.
///
/// The two stores must be the same store on two machines: their files are sealed to a key derived
/// from the profile key, so a merge across two *different* profiles would deliver bytes the far end
/// cannot open. `oslo sync` checks the fingerprints before it gets here.
pub fn merge(left: &Store, right: &Store, dry_run: bool) -> Result<SecretReport, String> {
    let mut report = SecretReport::default();

    let mut every = every_name(left);
    every.extend(every_name(right));
    every.sort();
    every.dedup();

    for name in every {
        let ours = left.path(&name).ok().and_then(|path| read_kept(&path));
        let theirs = right.path(&name).ok().and_then(|path| read_kept(&path));
        match settle(
            ours.as_ref().map(|kept| &kept.stamp),
            theirs.as_ref().map(|kept| &kept.stamp),
        ) {
            Verdict::Agreed => report.unchanged += 1,
            Verdict::Ours => {
                let Some(kept) = ours else { continue };
                count(&mut report, theirs.is_none(), kept.stamp.deleted, false);
                if !dry_run {
                    write_kept(&right.path(&name)?, &kept)?;
                }
            }
            Verdict::Theirs => {
                let Some(kept) = theirs else { continue };
                count(&mut report, ours.is_none(), kept.stamp.deleted, true);
                if !dry_run {
                    write_kept(&left.path(&name)?, &kept)?;
                }
            }
        }
    }
    Ok(report)
}

fn count(report: &mut SecretReport, absent: bool, deleted: bool, on_the_left: bool) {
    let (added, updated, removed) = match on_the_left {
        true => (
            &mut report.added_left,
            &mut report.updated_left,
            &mut report.deleted_left,
        ),
        false => (
            &mut report.added_right,
            &mut report.updated_right,
            &mut report.deleted_right,
        ),
    };
    match (deleted, absent) {
        (true, true) => *updated += 1,
        (true, false) => *removed += 1,
        (false, true) => *added += 1,
        (false, false) => *updated += 1,
    }
}

// **Needs `crypt`, not just `secrets`.** Every test here seals and opens for real, so the shared
// `store` helper calls `key::generate` and asks for `Crypto::Native` — both of which exist only
// with the built-in crypto. Without this the module does not *compile* under `--features secrets`,
// which is a configuration the feature graph allows: `crypt` implies `secrets`, never the reverse.
#[cfg(all(test, feature = "crypt"))]
#[path = "sync/tests.rs"]
mod tests;
