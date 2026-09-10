//! The set of runnable command names, cached so that typing does not walk `$PATH`.
//!
//! The hinter used to rebuild this on every keystroke: lock the environment, `read_dir` all of
//! `$PATH` — measured here at 113 directories and 3373 executables — and allocate a `String` per
//! entry, 3.3 ms and 3373 allocations for one character. The set only changes when `$PATH`
//! changes or a directory on it does, so it is cached on exactly those two facts.

use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

/// How long a cached set is trusted before its directories are re-stat'ed.
///
/// Stat'ing 113 directories is far cheaper than reading them, but it is not free either, and a
/// new executable appearing mid-keystroke is not worth paying for on every character.
const REVALIDATE_AFTER: Duration = Duration::from_millis(500);

/// Bumped by [`invalidate`]; part of the cache key, so a bump discards whatever is cached.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Drop the cached command set.
///
/// `hash -r` means "forget where commands live", and a shell that kept completing a command it
/// had just been told to forget would be lying. Called from the `hash` builtin.
pub fn invalidate() {
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// Build the cache now, on a thread of its own, so the first Tab does not pay for it.
///
/// Reading 113 directories and 3373 executables costs about 10ms warm and a great deal more from a
/// cold page cache — and it was being paid on the first keystroke that wanted a completion, which
/// is the one moment the shell is being watched. The work is the same; it just happens while you
/// are still reading the prompt.
///
/// Detached on purpose: nothing waits for it. If the first Tab arrives before it finishes, the two
/// meet at the cache's own lock and the second one to arrive uses what the first built — the
/// existing behaviour, at its existing cost, which is the worst this can do.
pub fn warm(path: String) {
    std::thread::spawn(move || {
        let _ = CommandIndex::executables(&path);
    });
}

#[derive(PartialEq, Eq)]
struct Key {
    generation: u64,
    path: String,
    /// One entry per `$PATH` directory: its modification time, or `None` if it does not exist.
    stamps: Vec<Option<SystemTime>>,
}

struct Entry {
    key: Key,
    names: Arc<HashSet<String>>,
    /// The same names in order, so a prefix is a range rather than a scan. See [`CommandIndex::sorted`].
    sorted: Arc<Vec<String>>,
    checked_at: Instant,
}

/// A process-wide cache of the executables on `$PATH`.
///
/// Global rather than a field on the helper so that the `hash` builtin — which has no way to
/// reach the line editor — can still invalidate it.
pub struct CommandIndex;

fn cache() -> &'static Mutex<Option<Entry>> {
    static CACHE: OnceLock<Mutex<Option<Entry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

impl CommandIndex {
    /// Every executable reachable by bare name through `path`.
    ///
    /// The result is shared, not copied: callers filter it by prefix without allocating.
    pub fn executables(path: &str) -> Arc<HashSet<String>> {
        Self::both(path).0
    }

    /// Every executable on `path`, in order.
    ///
    /// **What a prefix search wants.** The set answers membership in one step but has no order, so
    /// finding the names starting with `car` meant a `starts_with` against all of them — a few
    /// thousand per keystroke, for the ghost suggestion. Sorted, the answer is a contiguous range
    /// found by binary search. Built with the set, from the same scan, and shared the same way.
    pub fn sorted(path: &str) -> Arc<Vec<String>> {
        Self::both(path).1
    }

    /// The two views of one scan, taken under a single lock.
    ///
    /// **Both, together, or neither.** These used to be two calls: fill the cache, drop the lock,
    /// take it again to read the sorted half. The cache holds *one* entry for whichever `$PATH` was
    /// asked for last, and something else asks between those two locks — `warm` is a background
    /// thread doing exactly this at startup. The second lock then answered from a different
    /// directory's scan, or from an entry that had been replaced with one for another path
    /// entirely, and the caller got names that were not on its `$PATH` or none at all.
    fn both(path: &str) -> (Arc<HashSet<String>>, Arc<Vec<String>>) {
        let generation = GENERATION.load(Ordering::Relaxed);
        let mut guard = cache().lock().unwrap_or_else(|held| held.into_inner());

        if let Some(entry) = guard.as_ref() {
            let fresh = entry.key.generation == generation
                && entry.key.path == path
                && entry.checked_at.elapsed() < REVALIDATE_AFTER;
            if fresh {
                return (Arc::clone(&entry.names), Arc::clone(&entry.sorted));
            }
        }

        let key = Key {
            generation,
            path: path.to_string(),
            stamps: stamps(path),
        };

        if let Some(entry) = guard.as_mut()
            && entry.key == key
        {
            // Nothing moved; renew the lease rather than re-reading the directories.
            entry.checked_at = Instant::now();
            return (Arc::clone(&entry.names), Arc::clone(&entry.sorted));
        }

        let names = Arc::new(scan(path));
        let mut ordered: Vec<String> = names.iter().cloned().collect();
        ordered.sort_unstable();
        let sorted = Arc::new(ordered);
        *guard = Some(Entry {
            key,
            names: Arc::clone(&names),
            sorted: Arc::clone(&sorted),
            checked_at: Instant::now(),
        });
        (names, sorted)
    }

    /// The names on `path` that start with `stem`, as a slice of [`Self::sorted`].
    pub fn starting_with(sorted: &[String], stem: &str) -> std::ops::Range<usize> {
        let first = sorted.partition_point(|name| name.as_str() < stem);
        let past = sorted[first..]
            .iter()
            .position(|name| !name.starts_with(stem))
            .map_or(sorted.len(), |n| first + n);
        first..past
    }

    /// Whether a bare command name resolves to something runnable.
    ///
    /// Names containing `/` are not looked up here: they are paths, and the caller has to stat
    /// them itself.
    pub fn contains(path: &str, name: &str) -> bool {
        !name.contains('/') && Self::executables(path).contains(name)
    }

    /// Whether anything runnable *starts with* `stem` — a half-typed command rather than a wrong
    /// one.
    ///
    /// A binary search over the sorted index, so it costs what a lookup costs. That is the point:
    /// [`nearest`] walks the whole index measuring edit distances, and this answers "still typing"
    /// before that work is reached. Typing `cargo` asks it five times and scans nothing.
    pub fn has_prefix(path: &str, stem: &str) -> bool {
        if stem.is_empty() || stem.contains('/') {
            return false;
        }
        !Self::starting_with(&Self::sorted(path), stem).is_empty()
    }
}

/// The runnable name most like `name`, if one is close enough to be worth suggesting.
///
/// Costs nothing to offer: the index of every executable on `$PATH` is already built and cached
/// for the completion dropdown, so this is a walk over data the shell is holding anyway.
///
/// "Close enough" is deliberately strict — one edit for a short name, two for a long one. A shell
/// that guesses wildly is worse than one that says nothing, because a wrong suggestion is read as
/// a fact and typed out.
pub fn nearest(path: &str, name: &str) -> Option<String> {
    if name.len() < 2 {
        return None;
    }
    let names = CommandIndex::executables(path);
    // **Two letters the wrong way round, asked first.** It is the commonest typo there is —
    // `gerp`, `gti`, `tset`, `buidl` — and Levenshtein charges *two* for it, which the budget
    // below only affords to a name of five characters or more. Every short command anybody
    // mistypes was therefore beyond repair: `gerp foo` got no correction at all.
    //
    // Asked first rather than folded into the distance because it also settles the ties. `gti` is
    // one substitution from `gtf` and one swap from `git`, and by any distance those are equal —
    // so the shell answered with whichever the scan reached first, which was the wrong one about
    // as often as not. A swap is what a person actually did; it wins.
    if let Some(swapped) = transpositions(name).find(|word| names.contains(word)) {
        return Some(swapped);
    }
    // **`sorted`, not the set.** The set has no order, so two candidates at the same distance were
    // separated by the hash — the same shell, asked the same question twice, could answer
    // differently. See the tie note above; this is the half of it that has to be decided even
    // when neither candidate is a swap.
    nearest_of(CommandIndex::sorted(path).iter().map(String::as_str), name)
}

/// The nearest of `candidates` to `name`, by the same rule [`nearest`] applies to `$PATH`.
///
/// **Separate from `nearest` because `$PATH` is not the only list of names a shorthand can miss.**
/// `@work` mistyped as `@wrok` is the same mistake as `gerp` for `grep`, made against the marks a
/// config registered instead of against the executables — and answering it with a second, subtly
/// different rule is how two features that look alike start behaving differently.
///
/// `candidates` should be in a stable order, for the tie reason above.
pub fn nearest_of<'a>(candidates: impl Iterator<Item = &'a str>, name: &str) -> Option<String> {
    if name.len() < 2 {
        return None;
    }
    let ordered: Vec<&str> = candidates.collect();
    if let Some(swapped) = transpositions(name).find(|word| ordered.contains(&word.as_str())) {
        return Some(swapped);
    }
    let budget = if name.len() <= 4 { 1 } else { 2 };
    let mut best: Option<(usize, &str)> = None;
    for candidate in ordered {
        // A candidate wildly different in length cannot be within budget, and skipping it here
        // avoids the quadratic work for most of the three thousand entries.
        if candidate.len().abs_diff(name.len()) > budget {
            continue;
        }
        let distance = edit_distance(name, candidate, budget)?;
        if distance <= budget && best.is_none_or(|(d, _)| distance < d) {
            best = Some((distance, candidate));
        }
    }
    best.map(|(_, name)| name.to_string())
}

/// Every way of swapping one adjacent pair of characters in `name`.
///
/// Characters rather than bytes: a swap inside a multi-byte character is not a typo anybody made,
/// and slicing one would panic.
pub(crate) fn transpositions(name: &str) -> impl Iterator<Item = String> + '_ {
    let chars: Vec<char> = name.chars().collect();
    let chars2 = chars.clone();
    (0..chars.len().saturating_sub(1))
        .filter(move |at| chars[*at] != chars[at + 1])
        .map(move |at| {
            let mut swapped = chars2.clone();
            swapped.swap(at, at + 1);
            swapped.into_iter().collect()
        })
}

/// Levenshtein distance, abandoning once it cannot come in under `budget`.
///
/// Returns `Some(distance)` always; the `Option` is for the caller's `?` convenience on an empty
/// index. Rows are kept as two vectors rather than a matrix because only the previous one matters.
pub(crate) fn edit_distance(a: &str, b: &str, budget: usize) -> Option<usize> {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        current[0] = i + 1;
        let mut row_best = current[0];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            current[j + 1] = (previous[j] + cost)
                .min(previous[j + 1] + 1)
                .min(current[j] + 1);
            row_best = row_best.min(current[j + 1]);
        }
        // Every later row is at least this good, so a row that is already over budget settles it.
        if row_best > budget {
            return Some(budget + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    Some(previous[b.len()])
}

fn stamps(path: &str) -> Vec<Option<SystemTime>> {
    path.split(':')
        .map(|dir| fs::metadata(dir).and_then(|m| m.modified()).ok())
        .collect()
}

fn scan(path: &str) -> HashSet<String> {
    let mut names = HashSet::new();
    for dir in path.split(':') {
        // An empty `$PATH` element means the current directory, per POSIX.
        let dir = if dir.is_empty() { "." } else { dir };
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if is_executable(&entry) {
                names.insert(entry.file_name().to_string_lossy().into_owned());
            }
        }
    }
    // **Hidden names are left out of the index rather than filtered by its readers.** Six things
    // read this — completion, hinting, highlighting, `repair`, the `=` shorthand and the
    // did-you-mean after a failed command — and a check in each is six chances for one to disagree
    // and offer a name that will not run. `oslo_base::command::apply` invalidates the cache when
    // the set moves, which is what makes building it in here correct.
    names.retain(|name| !oslo_base::command::hidden(name));
    names
}

/// Whether a directory entry is something the shell could actually run.
///
/// The permission bit is checked, not just "is it a file": `$PATH` directories are full of
/// data files, and offering `pkgconfig` as a command is noise.
fn is_executable(entry: &fs::DirEntry) -> bool {
    // **`DirEntry::metadata` does not follow symlinks** — it is `lstat`, whatever this comment
    // used to claim. So every symlinked command was reported as not a file and left out of the
    // index: `ls` and `cat` are links to `coreutils`, which is most of what a person types. They
    // ran fine, but the highlighter drew them red as unknown and completion offered nothing.
    //
    // `fs::metadata` on the path is the one that follows, which is what is wanted here: a link to
    // a real executable is runnable, and a dangling one is not.
    let Ok(meta) = fs::metadata(entry.path()) else {
        return false;
    };
    meta.is_file() && meta.permissions().mode() & 0o111 != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A near-miss is offered; a wild guess is not.
    ///
    /// A wrong suggestion is worse than none, because it is read as a fact and typed out.
    #[test]
    fn only_a_close_name_is_suggested() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        for name in ["cargo", "grep", "systemctl"] {
            make_exe(dir.path(), name);
        }
        invalidate();
        let path = dir.path().to_str().unwrap();

        // One letter out, and one letter missing.
        assert_eq!(nearest(path, "cargi").as_deref(), Some("cargo"));
        assert_eq!(nearest(path, "systemctf").as_deref(), Some("systemctl"));
        // Nothing like anything here.
        assert_eq!(nearest(path, "zzzzzz"), None);
        // A short name gets one edit of leeway, not two: `ls` must not suggest `cd`.
        assert_eq!(nearest(path, "xy"), None);
    }

    /// **Two letters the wrong way round is one mistake, not two.**
    ///
    /// `gerp` is four characters, so it gets one edit of leeway, and Levenshtein charges two for a
    /// swap — which left the commonest typo of all with no correction at all. The swap is asked
    /// about before the distance is measured.
    #[test]
    fn a_transposition_is_repaired() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        for name in ["grep", "git", "gtf"] {
            make_exe(dir.path(), name);
        }
        invalidate();
        let path = dir.path().to_str().unwrap();

        assert_eq!(nearest(path, "gerp").as_deref(), Some("grep"));
        // **And it beats a substitution at the same distance.** `gti` is one letter from `gtf` and
        // one swap from `git`; the swap is what the typist did. Before this the answer was
        // whichever the hash reached first.
        assert_eq!(nearest(path, "gti").as_deref(), Some("git"));
    }

    /// The same question, asked twice, answers the same way.
    ///
    /// The candidates used to be walked in the set's order, which is the hash's, so two names at
    /// equal distance were separated by nothing at all.
    #[test]
    fn an_equal_distance_tie_is_settled_the_same_way_every_time() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        // All one substitution from `bxxxx`, and no swap of it is any of them.
        for name in ["baxxx", "bxaxx", "bxxax", "bxxxa"] {
            make_exe(dir.path(), name);
        }
        invalidate();
        let path = dir.path().to_str().unwrap();

        let first = nearest(path, "bxxxx");
        assert!(first.is_some());
        for _ in 0..20 {
            assert_eq!(nearest(path, "bxxxx"), first);
        }
    }

    /// A symlinked command counts. Most of coreutils is symlinks — `ls` and `cat` both point at
    /// `coreutils` — and leaving them out marked the commonest commands unknown.
    #[test]
    fn a_symlink_to_an_executable_is_runnable() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        make_exe(dir.path(), "coreutils");
        std::os::unix::fs::symlink("coreutils", dir.path().join("ls")).unwrap();
        // A link going nowhere is not runnable, and must not be offered.
        std::os::unix::fs::symlink("gone", dir.path().join("dangling")).unwrap();

        invalidate();
        let names = CommandIndex::executables(dir.path().to_str().unwrap());
        assert!(names.contains("ls"), "a symlinked command is runnable");
        assert!(names.contains("coreutils"));
        assert!(
            !names.contains("dangling"),
            "a dangling link is not runnable"
        );
    }

    /// The cache is one global slot; two tests using different `$PATH`s would evict each other.
    pub(super) static SERIAL: Mutex<()> = Mutex::new(());

    pub(super) fn make_exe(dir: &std::path::Path, name: &str) {
        let p = dir.join(name);
        let mut f = fs::File::create(&p).unwrap();
        f.write_all(b"#!/bin/sh\n").unwrap();
        drop(f);
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn finds_executables_and_skips_data_files() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        make_exe(dir.path(), "oslo-test-exe");
        fs::write(dir.path().join("oslo-test-data"), b"x").unwrap();

        let path = dir.path().to_str().unwrap();
        let names = CommandIndex::executables(path);
        assert!(names.contains("oslo-test-exe"));
        assert!(!names.contains("oslo-test-data"));
    }

    #[test]
    fn a_second_lookup_is_served_from_the_cache() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        make_exe(dir.path(), "oslo-cached-exe");
        let path = dir.path().to_str().unwrap();

        let first = CommandIndex::executables(path);
        let second = CommandIndex::executables(path);
        // Same allocation, so no directory was read the second time.
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn invalidate_forces_a_rescan() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        let before = CommandIndex::executables(path);
        make_exe(dir.path(), "oslo-late-exe");
        invalidate();
        let after = CommandIndex::executables(path);
        assert!(!before.contains("oslo-late-exe"));
        assert!(after.contains("oslo-late-exe"));
    }

    #[test]
    fn a_different_path_is_a_different_answer() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        make_exe(a.path(), "oslo-in-a");
        make_exe(b.path(), "oslo-in-b");

        assert!(CommandIndex::executables(a.path().to_str().unwrap()).contains("oslo-in-a"));
        assert!(CommandIndex::executables(b.path().to_str().unwrap()).contains("oslo-in-b"));
        assert!(!CommandIndex::executables(b.path().to_str().unwrap()).contains("oslo-in-a"));
    }
}

#[cfg(test)]
mod paired_tests {
    use super::*;

    /// **The set and the sorted list must describe the same `$PATH`, under concurrency.**
    ///
    /// They were fetched under two separate locks: fill the cache, drop the lock, take it again to
    /// read the ordered half. The cache holds *one* entry — for whichever path was asked for last —
    /// so anything asking in between left the second lock answering from a different directory's
    /// scan. That is not hypothetical: [`warm`] is a background thread doing exactly this while the
    /// first prompt is being typed at.
    ///
    /// A loop against a thread rather than a single call, because the window is only as wide as one
    /// lock release; the buggy version fails this within a few dozen rounds.
    #[test]
    fn the_ordered_view_belongs_to_the_path_it_was_asked_for() {
        let _guard = super::tests::SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mine = tempfile::tempdir().unwrap();
        let theirs = tempfile::tempdir().unwrap();
        super::tests::make_exe(mine.path(), "only-in-mine");
        super::tests::make_exe(theirs.path(), "only-in-theirs");
        let mine = mine.path().to_str().unwrap().to_string();
        let elsewhere = theirs.path().to_str().unwrap().to_string();

        invalidate();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let signal = Arc::clone(&stop);
        let hammer = std::thread::spawn(move || {
            while !signal.load(Ordering::Relaxed) {
                let _ = CommandIndex::executables(&elsewhere);
            }
        });

        for _ in 0..2000 {
            let ordered = CommandIndex::sorted(&mine);
            assert!(
                !ordered.iter().any(|name| name == "only-in-theirs"),
                "the ordered view came from another path's scan: {ordered:?}"
            );
        }

        stop.store(true, Ordering::Relaxed);
        hammer.join().expect("the other caller finishes");
    }
}
