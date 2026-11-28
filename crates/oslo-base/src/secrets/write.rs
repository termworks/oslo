//! How a store changes a file: under its lock, with the stamp carried forward.
//!
//! Sealing happens before the lock is taken, so a `crypto hook` that calls back into the same
//! store cannot wait on itself. The lock then covers only what must not interleave: reading the
//! stamp there, advancing it, and writing the new file.

use std::path::Path;

use super::sync::{self, Kept, Stamp};
use super::{Store, hooked, place};

impl Store {
    /// This store's directory made ready, and its `.lock` held until the answer is dropped.
    pub fn lock(&self) -> Result<crate::private::Lock, String> {
        place::prepare(self)?;
        crate::private::lock(&self.directory, false)
    }

    /// Seal `value` for `name`, telling anything watching before and after.
    pub(super) fn sealed(&self, name: &str, value: &[u8]) -> Result<Vec<u8>, String> {
        hooked::touched(true, &self.name, name, true);
        let sealed = self.seal_named(name, value)?;
        hooked::touched(false, &self.name, name, true);
        Ok(sealed)
    }

    /// Open `sealed` as `name`'s, telling anything watching before and after.
    pub(super) fn opened(&self, name: &str, sealed: &[u8]) -> Result<Vec<u8>, String> {
        hooked::touched(true, &self.name, name, false);
        let value = self.unseal_named(name, sealed);
        hooked::touched(false, &self.name, name, false);
        value
    }

    /// Write `sealed` at `path` under the store's lock, with the stamp there carried forward.
    ///
    /// **Carried forward and advanced**, tombstone included: writing a name somebody deleted brings
    /// it back, and the new revision has to clear the tombstone's or the next sync would hand the
    /// deletion back. With `expected`, only while the stamp there is still that one: `Ok(false)`
    /// means somebody else wrote the file since it was read.
    pub(super) fn put(
        &self,
        path: &Path,
        sealed: Vec<u8>,
        expected: Option<&Stamp>,
    ) -> Result<bool, String> {
        let _held = self.lock()?;
        let before = sync::read_kept(path)?.map(|kept| kept.stamp);
        if expected.is_some() && before.as_ref() != expected {
            return Ok(false);
        }
        let mut stamp = before.unwrap_or(Stamp {
            revision: 0,
            deleted: false,
            tie_breaker: [0; 16],
        });
        stamp.deleted = false;
        stamp.advance().ok_or_else(|| pinned(path, &stamp))?;
        sync::write_kept(path, &Kept { stamp, sealed })?;
        Ok(true)
    }

    /// What a name that is not here, or is a tombstone, reads as.
    pub(super) fn missing(&self, name: &str) -> String {
        format!("{}: {name}: no such secret", self.name)
    }

    /// Re-encrypt everything here to the recipients as they now stand, and answer how many moved.
    ///
    /// Every value is opened before any is written, so a file no key opens stops the rotation with
    /// nothing changed. A file written by somebody else between the two is left as they wrote it,
    /// and is not counted: it was sealed to the same list.
    pub fn rotate(&self) -> Result<usize, String> {
        let mut values = Vec::new();
        for name in self.names() {
            let path = self.path(&name)?;
            let Some(kept) = sync::read_kept(&path)?.filter(|kept| !kept.stamp.deleted) else {
                continue;
            };
            let value = self.opened(&name, &kept.sealed)?;
            values.push((name, path, kept.stamp, value));
        }
        let mut moved = 0;
        for (name, path, stamp, value) in values {
            let sealed = self.sealed(&name, &value)?;
            if self.put(&path, sealed, Some(&stamp))? {
                moved += 1;
            }
        }
        Ok(moved)
    }
}

/// Why a stamp could not move: it is at the bound, or the system gave no randomness.
pub(super) fn pinned(path: &Path, stamp: &Stamp) -> String {
    format!(
        "{}: revision {} cannot go higher (the bound is 2^53), so nothing was written",
        path.display(),
        stamp.revision
    )
}
