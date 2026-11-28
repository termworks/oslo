//! `hash` — the shell's remembered command locations.
//!
//! The cache is a real one rather than a stub, because the observable behaviour scripts depend on
//! is `hash -r` *forgetting*: install a new binary earlier on `PATH` and the shell must find it.
//! A shell that never caches passes that test by accident, but then `hash foo` cannot report
//! "not found" for a name that does not exist, which is the other half of the contract.
//!
//! The table lives in a thread-local rather than in [`Environment`], so it is per-process: a
//! forked subshell inherits a copy and its own `hash -r` cannot reach back into the parent, which
//! is what bash does too. Listings walk it in bash's order: by FNV-1 bucket of the name, newest
//! first within a bucket.

use crate::env::builtins::spawn::resolve_program;
use crate::env::origin_now;
use crate::env::scope::Environment;
use oslo_base::error::Result;
use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

const USAGE: &str = "hash: usage: hash [-lr] [-p pathname] [-dt] [name ...]";
/// The bucket count of bash's command hash table, which decides its listing order.
const BUCKETS: u32 = 256;

struct Entry {
    path: PathBuf,
    hits: u32,
    /// When the entry was made, so a listing can put the newest of a bucket first.
    seq: u64,
}

thread_local! {
    static TABLE: RefCell<HashMap<String, Entry>> = RefCell::new(HashMap::new());
    static SEQ: Cell<u64> = const { Cell::new(0) };

    /// Whether `hashall` is on. Mirrored here rather than read from the `Environment`, because
    /// [`lookup`] is a free function — every caller that asks "what would this word run" reaches
    /// it, and most of them have no shell in hand. `set +o hashall` writes it; see
    /// [`note_hashall`].
    static HASHING: Cell<bool> = const { Cell::new(true) };
}

/// Tell the table whether `hashall` is on, when `set` changes it.
///
/// Turning it off empties the table, as bash does: what is left would otherwise be consulted by
/// `hash name` and reported by `hash` in a shell that says it is not hashing.
pub fn note_hashall(on: bool) {
    HASHING.with(|hashing| hashing.set(on));
    if !on {
        forget_all();
    }
}

/// Whether the shell is remembering command locations.
fn hashing() -> bool {
    HASHING.with(Cell::get)
}

#[derive(Default)]
struct Options {
    reset: bool,
    list: bool,
    delete: bool,
    terse: bool,
    path: Option<String>,
}

/// The option run, and where the names start; `Err` is the status of a usage error.
fn parse(args: &[String]) -> std::result::Result<(Options, usize), i32> {
    let mut opts = Options::default();
    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            i += 1;
            break;
        }
        if arg.len() < 2 || !arg.starts_with('-') {
            break;
        }
        for (at, c) in arg[1..].char_indices() {
            match c {
                'r' => opts.reset = true,
                'l' => opts.list = true,
                'd' => opts.delete = true,
                't' => opts.terse = true,
                'p' => {
                    let rest = &arg[1 + at + 1..];
                    let value = match rest.is_empty() {
                        true => {
                            i += 1;
                            args.get(i).cloned()
                        }
                        false => Some(rest.to_string()),
                    };
                    let Some(value) = value else {
                        let body = "hash: -p: option requires an argument";
                        crate::env::complain_option(args, 'p', body, USAGE);
                        return Err(2);
                    };
                    opts.path = Some(value);
                    break;
                }
                other => {
                    crate::env::complain_option(
                        args,
                        other,
                        &format!("hash: -{other}: invalid option"),
                        USAGE,
                    );
                    return Err(2);
                }
            }
        }
        i += 1;
    }
    Ok((opts, i.min(args.len())))
}

/// `hash [-lr] [-p pathname] [-dt] [name …]`.
pub fn builtin_hash(env: &mut Environment, args: &[String]) -> Result<i32> {
    let (opts, start) = match parse(args) {
        Ok(parsed) => parsed,
        Err(status) => return Ok(status),
    };
    let names = &args[start..];

    if opts.reset {
        // Also drops the line editor's own set of runnable names, built by walking `$PATH`.
        forget_all();
    }
    if (opts.delete || opts.terse) && names.is_empty() {
        let letter = if opts.delete { 'd' } else { 't' };
        let body = format!("hash: -{letter}: option requires an argument");
        crate::env::complain(args, &format!("-{letter}"), &body, "needs a name", None);
        return Ok(1);
    }
    if names.is_empty() {
        if opts.path.is_some() {
            eprintln!("{USAGE}");
            return Ok(2);
        }
        match opts.list {
            true => print_portable(),
            // `hash -r` is a request to forget, not to report.
            false if !opts.reset => print_table(),
            false => {}
        }
        return Ok(0);
    }
    if opts.terse {
        return Ok(print_terse(args, names));
    }
    if !hashing() {
        eprintln!("{}hash: hashing disabled", origin_now());
        return Ok(1);
    }

    let mut status = 0;
    for name in names {
        // A word with a slash is a path, not something a `$PATH` search could answer.
        if name.contains('/') {
            continue;
        }
        if let Some(path) = &opts.path {
            insert(name, PathBuf::from(path), 0);
        } else if opts.delete {
            if TABLE.with(|t| t.borrow_mut().remove(name)).is_none() {
                not_found(args, name);
                status = 1;
            }
        } else if env.get_function(name).is_some() || env.is_builtin(name) {
            // A function or a builtin is found before `$PATH`, so there is nothing to remember.
        } else {
            TABLE.with(|t| t.borrow_mut().remove(name));
            match resolve_program(name) {
                Some(path) => insert(name, path, 0),
                None => {
                    not_found(args, name);
                    status = 1;
                }
            }
        }
    }
    Ok(status)
}

fn not_found(args: &[String], name: &str) {
    crate::env::complain(
        args,
        name,
        &format!("hash: {name}: not found"),
        "not in the table or on the PATH",
        Some("`hash` on its own lists what has been found so far"),
    );
}

/// `hash -t`: the remembered path, prefixed with the name when several were asked for.
fn print_terse(args: &[String], names: &[String]) -> i32 {
    let mut status = 0;
    for name in names {
        match recall(name) {
            Some(path) if names.len() == 1 => println!("{}", path.display()),
            Some(path) => println!("{name}\t{}", path.display()),
            None => {
                not_found(args, name);
                status = 1;
            }
        }
    }
    status
}

/// Resolve a command word through the table, filling it in on a miss.
///
/// This is the whole point of the cache and the one function the command-resolution path needs:
/// a bare word is answered from the table when it is there, searched for and remembered when it
/// is not, and the hit is counted either way so `hash` can report it.
///
/// A word containing a slash is a path, not a `PATH` search — bash does not hash those, and
/// neither does this: caching `./configure` would make the entry meaningless the moment the
/// shell changed directory.
pub fn lookup(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        return resolve_program(name);
    }
    // A name this directory hides is not on `$PATH` as far as anything asking here is concerned.
    if oslo_base::command::hidden(name) {
        return None;
    }
    // With `hashall` off the table is neither read nor written: every lookup is a fresh `$PATH`
    // search, which is exactly what turning it off asks for.
    if !hashing() {
        return resolve_program(name);
    }
    if let Some(path) = recall(name) {
        // A script trusts the table as bash does, unless `checkhash` asks for the check. At a
        // prompt a remembered path that has since been removed is searched for again, which also
        // drops the dead entry.
        let trusted = !super::shopt::on("checkhash") && !crate::exec::pipeline::is_interactive();
        if trusted || path.is_file() {
            return Some(path);
        }
        TABLE.with(|t| t.borrow_mut().remove(name));
    }
    let path = resolve_program(name)?;
    remember(name, path.clone());
    Some(path)
}

/// Drop every remembered location.
///
/// `hash -r`'s other caller is [`Environment::set_var`](crate::env::scope::Environment::set_var):
/// assigning to `PATH` invalidates every entry in the table by definition.
pub fn forget_all() {
    TABLE.with(|t| t.borrow_mut().clear());
    oslo_ui::invalidate_command_cache();
}

/// Record `path` for `name` with `hits`, keeping an existing entry's place in the listing.
fn insert(name: &str, path: PathBuf, hits: u32) {
    let seq = SEQ.with(|seq| {
        seq.set(seq.get() + 1);
        seq.get()
    });
    TABLE.with(|t| {
        let mut table = t.borrow_mut();
        let entry = table.entry(name.to_string()).or_insert(Entry {
            path: path.clone(),
            hits,
            seq,
        });
        entry.path = path;
        entry.hits = hits;
    });
}

/// Record that `name` resolved to `path`, and count the lookup that found it.
///
/// The path is *replaced*, not kept: a stale entry that survived a `PATH` change is the failure
/// mode a command cache exists to be blamed for.
pub fn remember(name: &str, path: PathBuf) {
    let hits = TABLE.with(|t| t.borrow().get(name).map_or(0, |entry| entry.hits));
    insert(name, path, hits + 1);
}

/// Look up a remembered location, counting the hit.
pub fn recall(name: &str) -> Option<PathBuf> {
    TABLE.with(|t| {
        let mut table = t.borrow_mut();
        let entry = table.get_mut(name)?;
        entry.hits += 1;
        Some(entry.path.clone())
    })
}

/// bash's string hash, FNV-1 over the bytes, which orders the listings of its hash tables.
pub(super) fn fnv(name: &str) -> u32 {
    name.bytes().fold(2_166_136_261u32, |hash, byte| {
        hash.wrapping_mul(16_777_619) ^ u32::from(byte)
    })
}

fn bucket(name: &str) -> u32 {
    fnv(name) & (BUCKETS - 1)
}

/// Every entry as `(name, path, hits)`, in the order bash walks its table.
fn in_bash_order() -> Vec<(String, PathBuf, u32)> {
    TABLE.with(|t| {
        let table = t.borrow();
        let mut entries: Vec<(&String, &Entry)> = table.iter().collect();
        entries.sort_by_key(|(name, entry)| (bucket(name), std::cmp::Reverse(entry.seq)));
        entries
            .into_iter()
            .map(|(name, entry)| (name.clone(), entry.path.clone(), entry.hits))
            .collect()
    })
}

fn print_table() {
    // Distinct from "empty", and bash draws the same distinction.
    if !hashing() {
        eprintln!("{}hash: hashing disabled", origin_now());
        return;
    }
    let entries = in_bash_order();
    if entries.is_empty() {
        // Not an error: bash says so on stdout and exits 0.
        println!("hash: hash table empty");
        return;
    }
    println!("hits\tcommand");
    for (_, path, hits) in entries {
        println!("{:>4}\t{}", hits, path.display());
    }
}

/// `hash -l`: the table as commands that would rebuild it.
fn print_portable() {
    for (name, path, _) in in_bash_order() {
        println!("builtin hash -p {} {name}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::{bucket, builtin_hash, recall, remember};
    use crate::env::Environment;
    use std::path::PathBuf;

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_known_command_is_remembered_and_a_missing_one_reported() {
        let mut env = Environment::new();
        assert_eq!(builtin_hash(&mut env, &argv(&["hash", "sh"])).unwrap(), 0);
        assert_eq!(
            builtin_hash(&mut env, &argv(&["hash", "no-such-command-xyz"])).unwrap(),
            1
        );
    }

    /// `hash -r` exists so that a newly installed binary is found; a table that survives it would
    /// keep handing back the old path forever.
    #[test]
    fn resetting_forgets_everything() {
        let mut env = Environment::new();
        remember("oslo-hash-test", PathBuf::from("/nowhere/oslo-hash-test"));
        assert!(recall("oslo-hash-test").is_some());
        assert_eq!(builtin_hash(&mut env, &argv(&["hash", "-r"])).unwrap(), 0);
        assert!(recall("oslo-hash-test").is_none());
    }

    /// A search fills the table, and the next lookup is answered from it.
    #[test]
    fn a_lookup_populates_the_table_and_the_next_one_is_served_from_it() {
        assert_eq!(super::lookup("sh"), super::resolve_program("sh"));
        assert!(recall("sh").is_some(), "the search should have been cached");
        assert_eq!(super::lookup("no-such-command-xyz"), None);
        assert!(recall("no-such-command-xyz").is_none());
    }

    /// A word with a slash is a path, not a `PATH` search, so it must not enter the table.
    #[test]
    fn a_path_is_not_hashed() {
        assert!(super::lookup("/bin/sh").is_some());
        assert!(recall("/bin/sh").is_none());
    }

    /// A binary that moved must not be reported at its old location for the rest of the session.
    #[test]
    fn remembering_again_replaces_the_path() {
        remember("oslo-moved", PathBuf::from("/old/oslo-moved"));
        remember("oslo-moved", PathBuf::from("/new/oslo-moved"));
        assert_eq!(recall("oslo-moved"), Some(PathBuf::from("/new/oslo-moved")));
    }

    /// `hash -r` has to reach the line editor's own set of `$PATH` executables too.
    #[test]
    fn resetting_also_drops_the_completion_cache() {
        use oslo_ui::command_index::CommandIndex;
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        let before = CommandIndex::executables(path);

        let mut env = Environment::new();
        assert_eq!(builtin_hash(&mut env, &argv(&["hash", "-r"])).unwrap(), 0);

        // A fresh allocation means the directories were read again rather than replayed.
        assert!(!Arc::ptr_eq(&before, &CommandIndex::executables(path)));
    }

    #[test]
    fn an_unknown_option_is_a_usage_error() {
        let mut env = Environment::new();
        assert_eq!(builtin_hash(&mut env, &argv(&["hash", "-Z"])).unwrap(), 2);
    }

    /// The buckets bash's table puts these in, which is the order `hash` lists them.
    #[test]
    fn names_fall_in_bashs_buckets() {
        assert_eq!(bucket("grep"), 33);
        assert_eq!(bucket("ls"), 122);
        assert_eq!(bucket("cat"), 187);
        assert_eq!(bucket("sed"), 215);
    }
}
