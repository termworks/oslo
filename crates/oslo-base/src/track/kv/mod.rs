//! The Tagdata boundary for tracking storage.
//!
//! Tracking failures return `None`, leaving the shell usable without history or ranking. Database
//! transactions stay inside this module and long deletions are split into short write transactions.

mod file;
mod key;
mod range;
mod tree;

pub use file::is_a_database;
pub use key::{Fields, Key};
pub use range::{Span, upper_bound};
pub use tree::Tree;

use std::path::{Path, PathBuf};
use tagdata::{
    DB, Data, Error as TagdataError, MergeConflictPolicy, MergeOptions, OpenOptions, Tx,
};

pub(super) fn open_lock(path: &Path) -> Option<std::fs::File> {
    file::open_lock(path)
}

/// The approximate row bytes removed in one write transaction.
const DELETE_BUDGET: usize = (file::PAGE_SIZE / 4) as usize;

/// What one row costs a leaf beyond its own key and value: `size_of::<LeafElement>()`, which is a
/// tag and three `u64`s.
const LEAF_OVERHEAD: usize = 32;

/// Whether a scan wants the next row.
///
/// The reason a scan takes a closure at all: `LIMIT 1` is the commonest read in the store, and a
/// scan that collected before it stopped would pay for every row in the range to answer with one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Walk {
    /// Keep going.
    On,
    /// That was the row I wanted.
    Stop,
}

/// An open tracking store.
#[derive(Clone)]
pub struct Store {
    path: PathBuf,
    db: DB,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoreStats {
    pub file_bytes: u64,
    pub page_size: u64,
    pub allocated_pages: u64,
    pub free_pages: u64,
    pub pending_pages: u64,
    pub active_readers: u64,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Store")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl Store {
    /// Open the store at `path`, creating it and its directory if they are not there.
    ///
    /// `None` when the directory cannot be made private, when there is something at the path that
    /// is not one of our databases, or when the engine will not have it. All three are "this shell
    /// has no store", which is a supported way to run.
    pub fn open(path: &Path) -> Option<Store> {
        file::prepare_directory(path)?;
        let db = file::open(path)?;
        file::make_private(path)?;
        Some(Store {
            path: path.to_path_buf(),
            db,
        })
    }

    pub fn open_existing(path: &Path, read_only: bool) -> Result<Store, String> {
        if !path.is_file() {
            return Err(format!("{}: database does not exist", path.display()));
        }
        let info = tagdata::FormatInfo::inspect(path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if info.page_size != file::PAGE_SIZE {
            return Err(format!(
                "{}: page size {} is not supported",
                path.display(),
                info.page_size
            ));
        }
        let mut options = OpenOptions::new().pagesize(file::PAGE_SIZE);
        if read_only {
            options = options.read_only();
        }
        let db = options
            .open(path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(Store {
            path: path.to_path_buf(),
            db,
        })
    }

    /// Where the store is.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// How large the file has grown, in bytes.
    pub fn size(&self) -> u64 {
        std::fs::metadata(&self.path)
            .map(|meta| meta.len())
            .unwrap_or(0)
    }

    pub fn verify(&self) -> Result<(), String> {
        self.db
            .verify()
            .map_err(|error| format!("{}: {error}", self.path.display()))
    }

    pub fn stats(&self) -> Result<StoreStats, String> {
        self.db
            .stats()
            .map(|stats| StoreStats {
                file_bytes: stats.file_bytes,
                page_size: stats.page_size,
                allocated_pages: stats.allocated_pages,
                free_pages: stats.free_pages,
                pending_pages: stats.pending_pages,
                active_readers: stats.active_readers,
            })
            .map_err(|error| format!("{}: {error}", self.path.display()))
    }

    /// Read from the store.
    ///
    /// The transaction is opened, `work` is run against it and it is dropped. A read transaction is
    /// never committed because it changes nothing.
    pub fn read<T>(&self, work: impl FnOnce(&Reader<'_, '_>) -> Option<T>) -> Option<T> {
        crate::forksafe::guarded(|| {
            let tx = self.db.tx(false).ok()?;
            work(&Reader { tx: &tx })
        })
    }

    pub fn read_checked<T>(
        &self,
        work: impl FnOnce(&Reader<'_, '_>) -> Result<T, String>,
    ) -> Result<T, String> {
        crate::forksafe::guarded(|| {
            let tx = self
                .db
                .tx(false)
                .map_err(|error| format!("{}: {error}", self.path.display()))?;
            work(&Reader { tx: &tx })
        })
    }

    /// Write to the store, committing when `work` answers `Some` and discarding when it answers
    /// `None`.
    ///
    /// This exists so that no caller writes `db.tx(true)` and forgets the commit. The whole of
    /// `work` is one transaction: a crash between two `put`s cannot leave a run attributed to a
    /// directory whose arrival was never recorded, which is the property the SQL version got from
    /// `BEGIN`/`COMMIT`.
    ///
    /// Dropping an uncommitted transaction rolls it back.
    pub fn write<T>(&self, work: impl FnOnce(&Writer<'_, '_>) -> Option<T>) -> Option<T> {
        crate::forksafe::guarded(|| {
            let tx = self.db.tx(true).ok()?;
            let value = work(&Writer(Reader { tx: &tx }))?;
            tx.commit().ok().map(|()| value)
        })
    }

    pub fn write_checked<T>(
        &self,
        work: impl FnOnce(&Writer<'_, '_>) -> Result<T, String>,
    ) -> Result<T, String> {
        crate::forksafe::guarded(|| {
            let tx = self
                .db
                .tx(true)
                .map_err(|error| format!("{}: {error}", self.path.display()))?;
            let value = work(&Writer(Reader { tx: &tx }))?;
            tx.commit()
                .map_err(|error| format!("{}: {error}", self.path.display()))?;
            Ok(value)
        })
    }

    pub fn merge_sync_from(
        &self,
        source: &Store,
        overwrite: bool,
        destination_wins: impl Fn(&[u8], &[u8], &[u8]) -> Result<bool, String>,
    ) -> Result<(), String> {
        let source_tx = source
            .db
            .tx(false)
            .map_err(|error| format!("{}: {error}", source.path.display()))?;
        let source_bucket = match source_tx.get_bucket(Tree::SyncEvent.name()) {
            Ok(bucket) => bucket,
            Err(TagdataError::BucketMissing) => return Ok(()),
            Err(error) => return Err(format!("{}: {error}", source.path.display())),
        };
        let destination_tx = self
            .db
            .tx(true)
            .map_err(|error| format!("{}: {error}", self.path.display()))?;
        let destination = destination_tx
            .get_or_create_bucket(Tree::SyncEvent.name())
            .map_err(|error| format!("{}: {error}", self.path.display()))?;
        let policy = if overwrite {
            MergeConflictPolicy::Overwrite
        } else {
            MergeConflictPolicy::KeepExisting
        };
        let mut preserved = Vec::new();
        if overwrite {
            for data in source_bucket.cursor() {
                // Every read can now report a corrupt page rather than panicking on one, so a row
                // that cannot be read is skipped rather than taking the merge down with it.
                let Ok(Data::KeyValue(source_pair)) = data else {
                    continue;
                };
                let Ok(Some(Data::KeyValue(destination_pair))) = destination.get(source_pair.key())
                else {
                    continue;
                };
                if destination_wins(
                    source_pair.key(),
                    destination_pair.value(),
                    source_pair.value(),
                )? {
                    preserved.push((
                        source_pair.key().to_vec(),
                        destination_pair.value().to_vec(),
                    ));
                }
            }
        }
        destination
            .merge_from(&source_bucket, MergeOptions::new().conflict_policy(policy))
            .map_err(|error| format!("{}: {error}", self.path.display()))?;
        for (key, value) in preserved {
            destination
                .put(key, value)
                .map_err(|error| format!("{}: {error}", self.path.display()))?;
        }
        destination_tx
            .commit()
            .map_err(|error| format!("{}: {error}", self.path.display()))
    }

    pub fn backup_to(&self, destination: &Path) -> Result<(), String> {
        file::backup(&self.db, destination)
    }

    pub fn ensure_private(&self) -> Result<(), String> {
        file::make_private(&self.path)
            .ok_or_else(|| format!("{}: cannot set mode 0600", self.path.display()))
    }

    /// Remove every row in `span`, a small chunk to a transaction, and answer how many went.
    ///
    /// Use this for long sweeps so other processes can acquire the writer between chunks.
    pub fn delete_span_in_chunks(&self, tree: Tree, span: &Span) -> usize {
        let mut gone = 0;
        let mut previous: Option<Vec<u8>> = None;
        loop {
            let Some(doomed) = self.read(|reader| {
                let mut spent = 0;
                let mut doomed = Vec::new();
                reader.scan(tree, span, |key, value| {
                    doomed.push(key.to_vec());
                    spent += LEAF_OVERHEAD + key.len() + value.len();
                    if spent >= DELETE_BUDGET {
                        Walk::Stop
                    } else {
                        Walk::On
                    }
                });
                Some(doomed)
            }) else {
                return gone;
            };
            if doomed.is_empty() {
                return gone;
            }
            // The loop is driven by the store getting smaller, so it has to notice when the store
            // does not. A committed transaction that removed nothing would otherwise be read back,
            // deleted again and committed again, for ever — and a shell that hangs is very much
            // worse than a bound that is not enforced. Seeing the same first key twice is the exact
            // statement of "no progress", and it costs one comparison a chunk.
            if previous.as_deref() == Some(doomed[0].as_slice()) {
                return gone;
            }
            previous = Some(doomed[0].clone());
            // A chunk that fails is a chunk that changed nothing, so stopping leaves the store
            // exactly as consistent as finishing would have — with more rows in it.
            if self
                .write(|writer| {
                    for key in &doomed {
                        writer.delete(tree, key);
                    }
                    Some(())
                })
                .is_none()
            {
                return gone;
            }
            gone += doomed.len();
        }
    }

    /// Remove a list of keys that is not a span, under the same budget and for the same reason.
    ///
    /// [`Store::delete_span_in_chunks`] is the one to reach for; this is its sibling for the case a
    /// range cannot express. A secondary index is the whole of that case: `run_argv` is keyed
    /// `(mode, argv, dir_id)`, so the entries belonging to one directory are scattered through it
    /// and the only way to name them is one at a time. `super::prune` derives each from the primary
    /// key it is already deleting.
    ///
    /// Missing keys still count toward the chunk budget.
    pub fn delete_keys_in_chunks(&self, tree: Tree, keys: &[Vec<u8>]) -> usize {
        let mut gone = 0;
        let mut rest = keys;
        while !rest.is_empty() {
            let mut spent = 0;
            let take = rest
                .iter()
                .take_while(|key| {
                    let first = spent == 0;
                    spent += LEAF_OVERHEAD + key.len();
                    first || spent < DELETE_BUDGET
                })
                .count();
            let (chunk, tail) = rest.split_at(take);
            rest = tail;
            // A chunk that fails changed nothing, so stopping leaves the store exactly as
            // consistent as finishing would have — with more rows in it.
            let Some(taken) = self
                .write(|writer| Some(chunk.iter().filter(|key| writer.delete(tree, key)).count()))
            else {
                return gone;
            };
            gone += taken;
        }
        gone
    }
}

/// A transaction that can be read.
///
/// `'r` is the transaction borrow and `'tx` is the engine transaction lifetime.
pub struct Reader<'r, 'tx> {
    tx: &'r Tx<'tx>,
}

impl Reader<'_, '_> {
    /// The value under `key`, or `None` — which is also the answer for a bucket no write has
    /// created yet, because an empty store and an empty bucket hold the same rows.
    pub fn get(&self, tree: Tree, key: &[u8]) -> Option<Vec<u8>> {
        let bucket = self.tx.get_bucket(tree.name()).ok()?;
        match bucket.get(key).ok()?? {
            Data::KeyValue(pair) => Some(pair.value().to_vec()),
            Data::Bucket(_) => None,
        }
    }

    /// Whether `key` is there at all, without copying its value.
    pub fn has(&self, tree: Tree, key: &[u8]) -> bool {
        self.tx
            .get_bucket(tree.name())
            .ok()
            .and_then(|bucket| bucket.get_kv(key).ok().flatten())
            .is_some()
    }

    /// Walk `span` in key order, stopping when `visit` says so.
    ///
    /// A seek to the lower bound followed by a walk to the upper one — **not** a full scan with a
    /// filter, which is the same mistake `LIKE` was. The keys and values are borrowed from the
    /// mapped file, so a visitor that only looks at them allocates nothing.
    pub fn scan(&self, tree: Tree, span: &Span, mut visit: impl FnMut(&[u8], &[u8]) -> Walk) {
        let Ok(bucket) = self.tx.get_bucket(tree.name()) else {
            return;
        };
        for found in bucket.range(span.bounds()) {
            if let Ok(Data::KeyValue(pair)) = found
                && visit(pair.key(), pair.value()) == Walk::Stop
            {
                return;
            }
        }
    }

    /// The first row of `span` that `pick` answers `Some` for.
    ///
    /// The `LIMIT 1` of the store: it stops at the row it takes rather than reading the range.
    pub fn find<T>(
        &self,
        tree: Tree,
        span: &Span,
        mut pick: impl FnMut(&[u8], &[u8]) -> Option<T>,
    ) -> Option<T> {
        let mut found = None;
        self.scan(tree, span, |key, value| match pick(key, value) {
            Some(taken) => {
                found = Some(taken);
                Walk::Stop
            }
            None => Walk::On,
        });
        found
    }

    /// Everything in `span` that `take` answers `Some` for, in key order.
    pub fn collect<T>(
        &self,
        tree: Tree,
        span: &Span,
        mut take: impl FnMut(&[u8], &[u8]) -> Option<T>,
    ) -> Vec<T> {
        let mut found = Vec::new();
        self.scan(tree, span, |key, value| {
            if let Some(taken) = take(key, value) {
                found.push(taken);
            }
            Walk::On
        });
        found
    }

    /// How many rows are in `span`. Counted by walking it, so it is a range and not the bucket.
    pub fn count(&self, tree: Tree, span: &Span) -> usize {
        let mut rows = 0;
        self.scan(tree, span, |_, _| {
            rows += 1;
            Walk::On
        });
        rows
    }
}

/// A transaction that can also be written.
///
/// Derefs to [`Reader`], because a write path reads: the upsert that makes this store an aggregate
/// is a `get`, an add and a `put`.
pub struct Writer<'r, 'tx>(Reader<'r, 'tx>);

impl<'r, 'tx> std::ops::Deref for Writer<'r, 'tx> {
    type Target = Reader<'r, 'tx>;

    fn deref(&self) -> &Reader<'r, 'tx> {
        &self.0
    }
}

impl Writer<'_, '_> {
    /// Put `value` under `key`, creating the bucket if this is the first row in it.
    ///
    /// Keys and values are owned for the transaction lifetime.
    pub fn put(&self, tree: Tree, key: Vec<u8>, value: Vec<u8>) -> Option<()> {
        let bucket = self.0.tx.get_or_create_bucket(tree.name()).ok()?;
        bucket.put(key, value).ok()?;
        Some(())
    }

    /// Remove `key`. `false` when it was not there, which is not a failure.
    pub fn delete(&self, tree: Tree, key: &[u8]) -> bool {
        self.0
            .tx
            .get_bucket(tree.name())
            .is_ok_and(|bucket| bucket.delete(key).is_ok())
    }

    /// Remove every row in `span`, in this transaction.
    ///
    /// Keys are collected before deletion so the active range iterator is not mutated.
    pub fn delete_span(&self, tree: Tree, span: &Span) -> usize {
        let doomed: Vec<Vec<u8>> = self.0.collect(tree, span, |key, _| Some(key.to_vec()));
        let mut gone = 0;
        for key in &doomed {
            if self.delete(tree, key) {
                gone += 1;
            }
        }
        gone
    }

    /// Empty a bucket completely, by dropping it.
    ///
    /// What `history -c` and `forget_runs` need. A bucket that is not there reads as empty, so the
    /// next `put` simply makes it again.
    pub fn clear(&self, tree: Tree) -> bool {
        self.0.tx.delete_bucket(tree.name()).is_ok()
    }
}

#[cfg(test)]
mod tests;
