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
//!
//! # A damaged header is refused, not guessed at
//!
//! A header must have its three fields, a state of `live` or `dead`, 32 hex characters, and a
//! revision no higher than [`MAX_REVISION`]. Anything else is reported as damaged: a revision near
//! `u64::MAX` would otherwise pin a name, since nothing could ever be written over it.
//!
//! # Concurrent writers
//!
//! Every read-modify-write here holds the store's `.lock` (see [`crate::private::lock`]), so two
//! `set`s, a `set` and a `rm`, or a write and a sync cannot interleave.

use super::{KEPT, Store};
use crate::private::{Links, read};
pub use crate::track::stamp::{MAX_REVISION, Stamp};
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
/// gets revision one — which is the truth about it: it has never been synced. A header that is
/// there and malformed is an error, naming what is wrong with it.
pub fn unwrap(bytes: &[u8]) -> Result<Kept, String> {
    let Some(rest) = bytes.strip_prefix(HEADER.as_bytes()) else {
        return Ok(Kept {
            stamp: Stamp::first().unwrap_or(Stamp {
                revision: 1,
                deleted: false,
                tie_breaker: [0; 16],
            }),
            sealed: bytes.to_vec(),
        });
    };
    let end = rest
        .iter()
        .position(|byte| *byte == b'\n')
        .ok_or("a header with no end")?;
    let header = std::str::from_utf8(&rest[..end]).map_err(|_| "a header that is not text")?;
    let fields: Vec<&str> = header.split(' ').collect();
    let [revision, state, tie] = fields[..] else {
        return Err(format!("a header of {} fields, not 3", fields.len()));
    };
    let revision = revision
        .parse::<u64>()
        .ok()
        .filter(|revision| *revision <= MAX_REVISION)
        .ok_or_else(|| format!("revision {} is not one from 0 to 2^53", clipped(revision)))?;
    let deleted = match state {
        "live" => false,
        "dead" => true,
        _ => return Err(format!("state {} is neither live nor dead", clipped(state))),
    };
    let tie_breaker = unhex(tie).ok_or("a tie-breaker that is not 32 hex characters")?;
    Ok(Kept {
        stamp: Stamp {
            revision,
            deleted,
            tie_breaker,
        },
        sealed: rest[end + 1..].to_vec(),
    })
}

/// A field from a header as an error may print it: quoted, escaped, and short.
fn clipped(field: &str) -> String {
    format!("{:?}", field.chars().take(24).collect::<String>())
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
///
/// Never through a symbolic link, only from a regular file, and at most [`super::LARGEST`] bytes:
/// a store is a directory other people's commits reach, so a link to `/dev/zero` or a FIFO named
/// like a secret is refused rather than read.
pub fn read_kept(path: &Path) -> Result<Option<Kept>, String> {
    match read(path, super::LARGEST, Links::Refuse)? {
        Some(bytes) => unwrap(&bytes)
            .map(Some)
            .map_err(|e| format!("{}: damaged header: {e}", path.display())),
        None => Ok(None),
    }
}

/// Write one, privately, through a fsynced scratch file and a rename, so a reader sees the old file
/// or the new one and a machine that stops leaves one of them whole.
///
/// The caller holds the store's lock, which is also what made its directory.
pub fn write_kept(path: &Path, kept: &Kept) -> Result<(), String> {
    crate::private::replace(path, &wrap(kept))
}

/// Every name a store holds, tombstones included: each regular `NAME.sealed` whose name is usable.
///
/// Entries are looked at without following links, and anything that is not a regular file — a
/// link, a FIFO, a device, a directory — is passed over without being opened.
pub fn every_name(store: &Store) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(&store.directory) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let name = name.strip_suffix(KEPT)?;
            super::name::check(name).ok()?;
            Some(name.to_string())
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
///
/// **Both stores are locked, and every file is read before any is written**, so a damaged header on
/// either side stops the merge with nothing changed, and a `set` cannot land in the middle of it.
pub fn merge(left: &Store, right: &Store, dry_run: bool) -> Result<SecretReport, String> {
    let mut report = SecretReport::default();
    let mut held = Vec::new();
    if !dry_run {
        held.push(left.lock()?);
        if right.directory != left.directory {
            held.push(right.lock()?);
        }
    }

    let mut every = every_name(left);
    every.extend(every_name(right));
    every.sort();
    every.dedup();

    let mut both = Vec::with_capacity(every.len());
    for name in every {
        let ours = read_kept(&left.path(&name)?)?;
        let theirs = read_kept(&right.path(&name)?)?;
        both.push((name, ours, theirs));
    }

    for (name, ours, theirs) in both {
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
    drop(held);
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
