//! Merging two macro stores, both ways at once.
//!
//! # The same rule as history, on a different shape
//!
//! History merges *events*, which are immutable and identified by a random id. Macros are the
//! opposite: mutable records identified by something a person chose, `alias/gs`. So there is no
//! merge of two histories to do here — for any one name, one of the two copies wins outright, and
//! [`crate::track::stamp`] is what picks it.
//!
//! That the rule lives elsewhere is the whole point: history, macros and secrets each keep their
//! own storage and their own encoding, and share the one definition of *newer*.
//!
//! # Why both sides are written
//!
//! A sync that only pulled would leave the far end missing everything this machine has, and the
//! person would have to remember to run it again from over there. Both stores come out holding the
//! union, so it does not matter which end you type it on.

use super::scripts::ledger;
use super::{Entry, Kind, Store, encode, key, stored};
use crate::track::stamp::{Verdict, settle};

/// What a merge did, in each direction.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MacroReport {
    pub added_left: usize,
    pub updated_left: usize,
    pub deleted_left: usize,
    pub added_right: usize,
    pub updated_right: usize,
    pub deleted_right: usize,
    pub unchanged: usize,
}

impl MacroReport {
    /// Whether anything at all moved, for a caller deciding if it needs to say so.
    pub fn quiet(&self) -> bool {
        self.added_left == 0
            && self.updated_left == 0
            && self.deleted_left == 0
            && self.added_right == 0
            && self.updated_right == 0
            && self.deleted_right == 0
    }

    /// Two reports as one.
    pub fn plus(self, other: MacroReport) -> MacroReport {
        MacroReport {
            added_left: self.added_left + other.added_left,
            updated_left: self.updated_left + other.updated_left,
            deleted_left: self.deleted_left + other.deleted_left,
            added_right: self.added_right + other.added_right,
            updated_right: self.updated_right + other.updated_right,
            deleted_right: self.deleted_right + other.deleted_right,
            unchanged: self.unchanged + other.unchanged,
        }
    }
}

/// Every name of the given kinds a store knows about, tombstones included.
fn names(store: &Store, kinds: &[Kind]) -> Vec<(Kind, String)> {
    let mut found = Vec::new();
    for kind in kinds.iter().copied() {
        let prefix = format!("{}/", kind.word());
        for full in crate::store::keys(store, &prefix) {
            if let Some(name) = full.strip_prefix(&prefix) {
                found.push((kind, name.to_string()));
            }
        }
    }
    found
}

/// Merge `left` and `right` so that both end up holding the same thing.
///
/// With `dry_run` neither store is written and the report says what would have happened, which is
/// the only way to look before syncing something you cannot un-sync.
///
/// **The snapshot is the caller's job.** A starting shell reads the flat file rather than the
/// database — see [`mod@super::snapshot`] — so a sync that stopped here would leave every arriving
/// unusable in every new shell. It is not done here because only the caller knows which of these two
/// stores is *this machine's*: the far end's snapshot is written on the far end, by the oslo running
/// over there, and writing it from here would put its aliases in our file.
///
/// Every kind but the scripts, which are files and are merged by [`merge_scripts`].
pub fn merge(left: &Store, right: &Store, dry_run: bool) -> Result<MacroReport, String> {
    let kinds: Vec<Kind> = Kind::every()
        .iter()
        .copied()
        .filter(|kind| *kind != Kind::Script)
        .collect();
    merge_kinds(left, right, &kinds, dry_run)
}

/// Merge this machine's scripts with the script records in `right`.
///
/// `scratch` is an empty store the caller provides. The scripts are stamped into it, merged, and —
/// unless this is a dry run — laid back out as files. `right` ends up holding the union.
pub fn merge_scripts(scratch: &Store, right: &Store, dry_run: bool) -> Result<MacroReport, String> {
    let ledger = ledger::path().ok_or(ledger::NOWHERE)?;
    merge_scripts_at(scratch, right, &ledger, dry_run)
}

/// [`merge_scripts`], against the ledger at `path`.
pub(in crate::macros) fn merge_scripts_at(
    scratch: &Store,
    right: &Store,
    path: &std::path::Path,
    dry_run: bool,
) -> Result<MacroReport, String> {
    ledger::stamp_into_with(scratch, path)?;
    let report = merge_kinds(scratch, right, &[Kind::Script], dry_run)?;
    if !dry_run {
        ledger::absorb_with(scratch, path)?;
    }
    Ok(report)
}

fn merge_kinds(
    left: &Store,
    right: &Store,
    kinds: &[Kind],
    dry_run: bool,
) -> Result<MacroReport, String> {
    let mut report = MacroReport::default();

    // Both sides' names, each once. A name only one of them has is exactly the case the merge is
    // for, so the walk cannot be over either store alone.
    let mut every = names(left, kinds);
    every.extend(names(right, kinds));
    every.sort();
    every.dedup();

    for (kind, name) in every {
        let ours = stored(left, kind, &name);
        let theirs = stored(right, kind, &name);
        let verdict = settle(
            ours.as_ref().map(|entry| &entry.stamp),
            theirs.as_ref().map(|entry| &entry.stamp),
        );
        match verdict {
            Verdict::Agreed => report.unchanged += 1,
            // Ours wins, so the far end takes it.
            Verdict::Ours => {
                let Some(entry) = ours else { continue };
                count(&mut report, theirs.is_none(), entry.stamp.deleted, false);
                if !dry_run {
                    write(right, &entry)?;
                }
            }
            Verdict::Theirs => {
                let Some(entry) = theirs else { continue };
                count(&mut report, ours.is_none(), entry.stamp.deleted, true);
                if !dry_run {
                    write(left, &entry)?;
                }
            }
        }
    }

    Ok(report)
}

/// Tally one record against the side that is about to change.
fn count(report: &mut MacroReport, absent: bool, deleted: bool, on_the_left: bool) {
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
        // A tombstone arriving where there was nothing is not a deletion anybody sees.
        (true, true) => *updated += 1,
        (true, false) => *removed += 1,
        (false, true) => *added += 1,
        (false, false) => *updated += 1,
    }
}

/// Put a record in exactly as it stands.
///
/// **Not [`super::put`]**, which advances the stamp: that is right for an edit somebody made and
/// wrong here, where the whole job is to carry a record across unchanged. Advancing it would make
/// the copy beat the original, and the next sync would push it back the other way for ever.
fn write(store: &Store, entry: &Entry) -> Result<(), String> {
    crate::store::set(
        store,
        &key(entry.kind, &entry.name),
        encode(entry).as_bytes(),
    )
}

#[cfg(test)]
#[path = "sync/tests.rs"]
mod tests;
