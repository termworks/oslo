//! The small named things you accumulate: aliases, abbreviations, functions, variables and scripts.
//!
//! ```text
//! ~/.local/share/oslo/macros/macros.db         aliases, abbreviations, functions, variables
//! ~/.local/share/oslo/macros/macros.snapshot   what a starting shell actually reads
//! ~/.local/sbin/                               the scripts, one file each — see [`scripts`]
//! ```
//!
//! A script is the one kind that is not a database row. Every function here that takes a
//! [`Kind`] answers [`Kind::Script`] from the directory instead, so a caller sees five kinds
//! behind one interface.
//!
//! # One store, not one per profile
//!
//! History is per profile because an agent's commands must not pollute the ranking of yours.
//! Aliases are the opposite: they are *your tooling*, and adding one in this shell and not finding
//! it in the next is the whole of the surprise. A profile changes what the shell **remembers**, not
//! what it **knows how to do**.
//!
//! # Why there is a snapshot as well as a database
//!
//! Measured, because the answer decided the design:
//!
//! ```text
//! oslo -c true, static musl            842 µs      (dash: 622 µs)
//! opening this database                2.61 ms     three times a whole shell start
//! reading 60 rows out of it            1.13 ms
//! reading 60 aliases from a flat file  3.6 µs
//! ```
//!
//! An interactive shell already pays 2.6 ms for the tracking store; a second one would double that
//! to answer a question a `read(2)` answers in microseconds. So [`snapshot::write`] is called by
//! every mutation and [`snapshot::read`] is what a starting shell reads.
//!
//! **The database stays the single source of truth.** The snapshot is a cache: delete it and the
//! next `oslo macros` command writes it again, and a shell that finds none simply has no aliases
//! until then rather than reading a database on the startup path.
//!
//! # Only two kinds are in the snapshot
//!
//! An alias must be in hand before the first line is parsed, and an abbreviation before the first
//! keystroke. A function is looked up **after** the `$PATH` search has already failed — the rule
//! `exec::simple::autoload` states — so it costs a database open only on a line that was going to
//! fail anyway, and nothing at startup. A script is a file on `$PATH` and costs nothing here.

pub mod live;
pub mod scripts;
pub mod snapshot;
pub mod sourced;
pub mod sync;
pub mod watch;

pub use crate::track::kv::Store;
use crate::track::stamp::Stamp;
use std::path::PathBuf;

/// What a record gets when the system has no randomness to give.
///
/// **Revision zero, which loses every comparison.** A stamp that cannot be rolled must not become
/// one that wins ties by accident, so it takes the value the shared rule treats as "not a record" —
/// see [`Stamp::is_real`]. Writing a macro still works; only its place in a sync is forfeit, and a
/// machine with no `getrandom` has larger problems.
const UNSTAMPED: Stamp = Stamp {
    revision: 0,
    deleted: false,
    tie_breaker: [0; 16],
};

/// What a stored name is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// A word replaced before the line is parsed.
    Alias,
    /// A word expanded into the buffer as you type it.
    Abbrev,
    /// A shell or Lua function, found after `$PATH`.
    Func,
    /// A whole program with a shebang: an executable file in [`scripts::directory`].
    Script,
    /// An environment variable whose body is a *recipe* rather than a value.
    ///
    /// `GITHUB_TOKEN` with the body `$(oslo secret get gh-token)` is the case this exists for: the
    /// shell holds the line, not the token, and runs it the first time something reads `$GITHUB_TOKEN`
    /// — see `oslo_shell::expand::param`. A body with nothing to run in it is simply a value, and
    /// costs the same nothing.
    Var,
}

impl Kind {
    /// Every kind there is, in the order a listing shows them.
    ///
    /// **One list, because two would drift.** Adding `Var` found two hand-written arrays of the
    /// four kinds that existed before it — `all` and `kinds_of` — and a variable stored perfectly
    /// well through one of them was invisible to both. Nothing that walks the kinds writes them out
    /// again: it asks here, and the compiler is what notices the next one.
    pub fn every() -> &'static [Kind] {
        &[
            Kind::Alias,
            Kind::Abbrev,
            Kind::Func,
            Kind::Script,
            Kind::Var,
        ]
    }

    pub fn word(self) -> &'static str {
        match self {
            Kind::Alias => "alias",
            Kind::Abbrev => "abbrev",
            Kind::Func => "func",
            Kind::Script => "script",
            Kind::Var => "var",
        }
    }

    pub fn named(word: &str) -> Option<Kind> {
        match word {
            "alias" => Some(Kind::Alias),
            "abbrev" | "abbr" => Some(Kind::Abbrev),
            "func" | "function" => Some(Kind::Func),
            "script" => Some(Kind::Script),
            "var" | "variable" | "env" => Some(Kind::Var),
            _ => None,
        }
    }

    /// Whether a starting shell needs this before it can do anything.
    ///
    /// A variable is here for what it *is*, not for what it costs: the shell is told the recipe at
    /// startup and runs it only when something reads the name, so a `$(…)` body is not a command
    /// run on every prompt. See `oslo_shell::expand::param`.
    pub fn wanted_at_startup(self) -> bool {
        matches!(self, Kind::Alias | Kind::Abbrev | Kind::Var)
    }

    /// What a temporary file holding one of these should be called, so an editor colours it.
    ///
    /// A script says what it is in its own first line, so its extension comes from the shebang
    /// rather than from here.
    pub fn extension(self, body: &str) -> &'static str {
        match self {
            Kind::Alias | Kind::Abbrev | Kind::Var => "sh",
            Kind::Func => {
                if body.trim_start().starts_with("--") || body.contains("local ") {
                    "lua"
                } else {
                    "sh"
                }
            }
            Kind::Script => match shebang_interpreter(body) {
                Some(interp) if interp.contains("python") => "py",
                Some(interp) if interp.contains("lua") => "lua",
                Some(interp) if interp.contains("perl") => "pl",
                Some(interp) if interp.contains("node") => "js",
                _ => "sh",
            },
        }
    }
}

/// Whether a variable's body is a value rather than something that has to be *run*.
///
/// One word with no command in it — `nvim`, `/srv/data`, `$HOME/bin`. The distinction is what
/// decides when a stored variable is applied, and it is the same question in three places, so it is
/// asked in one:
///
/// * a **value** costs nothing, so a shell exports it at startup like any other environment
///   variable, and a program that reads the environment itself finds it there;
/// * a **recipe** — `$(oslo secret get gh-token)` — is run the first time something reads the name,
///   because running every one of them at every shell start is exactly what a shell holding secrets
///   must not do;
/// * and `macros.sh`, which bash sources, can carry the first and not the second.
pub fn is_a_value(body: &str) -> bool {
    let body = body.trim();
    // **A space does not make something a command.** `EDITOR="code --wait"`, `LS_COLORS`, and
    // `SSH_CONNECTION="1.2.3.4 22 5.6.7.8 22"` are values that happen to contain one, and treating
    // them as recipes meant the shell *ran* them and kept whatever came back — so `$SSH_CONNECTION`
    // became the output of trying to execute an IP address. Substitution is what marks a recipe,
    // and substitution has a syntax: `$(…)` or backticks.
    !body.is_empty() && !body.contains("$(") && !body.contains('`')
}

/// One stored thing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub kind: Kind,
    pub name: String,
    pub body: String,
    /// Unix seconds, set the first time it is stored and never moved afterwards.
    ///
    /// The manager's first column. Editing a macro does not make it new, so this survives a rewrite
    /// — see [`put`], which carries the old one forward rather than stamping a fresh one.
    pub created: i64,
    /// Any number of labels: `system`, `git`, `work`. The manager's Left and Right move through
    /// them, which is where a history profile would be if a macro were a record of anything.
    pub tags: Vec<String>,
    /// `false` is off everywhere, until it is turned back on. Not a removal: what it was is still
    /// here, which is the whole difference between turning something off and deleting it.
    pub active: bool,
    /// Which revision of this macro it is, and whether it has been deleted.
    ///
    /// **Not the same switch as `active`**, and the difference matters when two machines sync.
    /// `active` is a setting *of* a macro that still exists; a deleted one is a
    /// [tombstone][crate::track::stamp] — the record stays so that the removal can travel, and
    /// [`get`] answers `None` for it as if it were gone, which for every purpose but sync it is.
    pub stamp: Stamp,
}

impl Entry {
    /// A new one, on and untagged, created now.
    pub fn new(kind: Kind, name: &str, body: &str) -> Entry {
        Entry {
            kind,
            name: name.to_string(),
            body: body.to_string(),
            created: now(),
            tags: Vec::new(),
            active: true,
            stamp: Stamp::first().unwrap_or(UNSTAMPED),
        }
    }

    pub fn tagged(self, tags: &[String]) -> Entry {
        Entry {
            tags: tags.to_vec(),
            ..self
        }
    }
}

/// Unix seconds, or 0 on a clock this cannot read.
///
/// Zero rather than a panic: a timestamp is a column in a list, and a manager that refused to store
/// a macro because the clock was odd would be worse than one whose first column said `just now`.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

/// The interpreter a script's shebang names, if it has one.
///
/// `#!/usr/bin/env python3` answers `python3` rather than `env`, because what the caller wants to
/// know is the *language*, and `env` is how a shebang finds it rather than what it is.
pub fn shebang_interpreter(body: &str) -> Option<String> {
    let first = body.lines().next()?.trim_end();
    let rest = first.strip_prefix("#!")?.trim();
    let mut words = rest.split_whitespace();
    let program = words.next()?;
    let base = program.rsplit('/').next().unwrap_or(program);
    if base == "env" {
        // `env -S`, `env VAR=x prog` — take the first word that is not a switch or an assignment.
        return words
            .find(|word| !word.starts_with('-') && !word.contains('='))
            .map(|word| word.rsplit('/').next().unwrap_or(word).to_string());
    }
    Some(base.to_string())
}

/// Whether `name` is one this may store.
///
/// The same rule the shell has for a function or alias name, and for the same reason: it becomes a
/// command word, so anything that could be an operator or a separator is not a name.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('-')
        && name.chars().all(|c| {
            c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '+' | ':' | '@' | ',' | '%')
        })
}

/// Whether `tag` is one this may store.
///
/// Looser than a name — a tag is never a command word — but it is a field in a comma-separated
/// header and a column in a list, so a comma, a space or a newline is not a tag.
pub fn valid_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag.len() <= 64
        && !tag.starts_with('-')
        && tag
            .chars()
            .all(|c| !c.is_whitespace() && !matches!(c, ',' | '#'))
}

/// `$XDG_DATA_HOME/oslo/macros`, or `~/.local/share/oslo/macros`.
pub fn directory() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
        })?;
    Some(base.join("oslo/macros"))
}

pub fn database() -> Option<PathBuf> {
    Some(directory()?.join("macros.db"))
}

pub fn snapshot() -> Option<PathBuf> {
    Some(directory()?.join("macros.snapshot"))
}

/// Open the database, creating it if it is not there.
pub fn open() -> Result<Store, String> {
    let path = database().ok_or_else(|| {
        "no $XDG_DATA_HOME and no $HOME, so there is nowhere to keep macros".to_string()
    })?;
    Store::open(&path).ok_or_else(|| format!("{}: cannot be opened", path.display()))
}

fn key(kind: Kind, name: &str) -> String {
    format!("{}/{name}", kind.word())
}

/// Store one, replacing whatever had that name and kind.
///
/// **`created` is taken from what is already there**, if anything is: editing a macro changes the
/// macro, not when you first made it, and a caller building an `Entry` to write should not have to
/// remember to go and read the old timestamp first.
/// **The stamp is advanced from whatever is already there**, tombstone included: writing a name
/// somebody deleted brings it back, and the revision has to clear the tombstone's or the next sync
/// would hand the deletion back and undo the write.
pub fn put(store: &Store, entry: &Entry) -> Result<(), String> {
    if !valid_name(&entry.name) {
        return Err(format!("{:?} is not a name this can store", entry.name));
    }
    if entry.kind == Kind::Script {
        return scripts::write(entry);
    }
    let before = stored(store, entry.kind, &entry.name);
    let created = before
        .as_ref()
        .map(|old| old.created)
        .unwrap_or(entry.created);
    let mut stamp = before.map(|old| old.stamp).unwrap_or(entry.stamp);
    stamp.deleted = false;
    stamp.advance();
    let record = Entry {
        created,
        stamp,
        ..entry.clone()
    };
    crate::store::set(
        store,
        &key(entry.kind, &entry.name),
        encode(&record).as_bytes(),
    )
}

/// Read one back, or `None` when there is none — including when what is there is a tombstone.
pub fn get(store: &Store, kind: Kind, name: &str) -> Option<Entry> {
    if kind == Kind::Script {
        return scripts::get(name);
    }
    stored(store, kind, name).filter(|entry| !entry.stamp.deleted)
}

/// The record as it really is, tombstones and all. For sync, which is the one thing that must see
/// a deletion rather than be told there is nothing there.
///
/// Database records only: a script is read with [`scripts::get`].
pub fn stored(store: &Store, kind: Kind, name: &str) -> Option<Entry> {
    let stored = crate::store::get(store, &key(kind, name))?;
    Some(decode(kind, name, &String::from_utf8_lossy(&stored)))
}

/// The header line a record starts with, and then the body exactly as it was written.
///
/// A body is arbitrary text — a script, several hundred lines of it — so it is stored **verbatim**
/// and everything else goes in front of it. The alternative, a field-separated format the body also
/// lives in, means escaping a script on the way in and unescaping it exactly right on the way out,
/// forever, to hold three small values.
///
/// ```text
/// 2 1754870400 on system,git 3 live 9f1c…  (32 hex characters)
/// git status --short
/// ```
///
/// The leading number is the format. Format `1` is the same line without the last three fields and
/// is still read, taking a fresh stamp; a record that starts with neither is read as a bare body
/// from before there were fields at all. Both cost one comparison, and mean a store written by an
/// older oslo opens rather than explodes.
///
/// **The three sync fields go last** so that the ones a person might read are at the front.
fn encode(entry: &Entry) -> String {
    format!(
        "2 {} {} {} {} {} {}\n{}",
        entry.created,
        if entry.active { "on" } else { "off" },
        entry.tags.join(","),
        entry.stamp.revision,
        if entry.stamp.deleted { "dead" } else { "live" },
        hex(&entry.stamp.tie_breaker),
        entry.body
    )
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

fn decode(kind: Kind, name: &str, stored: &str) -> Entry {
    let (header, body) = match stored.split_once('\n') {
        Some((header, body)) if header.starts_with("2 ") || header.starts_with("1 ") => {
            (header, body)
        }
        // No header: a body and nothing else. On, untagged, and undated.
        _ => {
            return Entry {
                kind,
                name: name.to_string(),
                body: stored.to_string(),
                created: 0,
                tags: Vec::new(),
                active: true,
                stamp: Stamp::first().unwrap_or(UNSTAMPED),
            };
        }
    };
    let mut fields = header.split(' ').skip(1);
    let created = fields.next().and_then(|n| n.parse().ok()).unwrap_or(0);
    let active = fields.next() != Some("off");
    let tags = fields
        .next()
        .map(|tags| {
            tags.split(',')
                .filter(|tag| !tag.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    // A record written before there were sync fields gets a fresh stamp, which is the right answer:
    // it has never been synced, so revision one is the truth about it.
    let stamp = match (fields.next(), fields.next(), fields.next()) {
        (Some(revision), Some(state), Some(tie)) => Stamp {
            revision: revision.parse().unwrap_or(1),
            deleted: state == "dead",
            tie_breaker: unhex(tie).unwrap_or([0; 16]),
        },
        _ => Stamp::first().unwrap_or(UNSTAMPED),
    };
    Entry {
        kind,
        name: name.to_string(),
        body: body.to_string(),
        created,
        tags,
        active,
        stamp,
    }
}

/// Turn one on or off, and answer what it now is. `None` if there is no such macro.
pub fn set_active(store: &Store, kind: Kind, name: &str, active: bool) -> Option<bool> {
    if kind == Kind::Script {
        return scripts::set_active(name, active);
    }
    let entry = get(store, kind, name)?;
    put(store, &Entry { active, ..entry }).ok()?;
    Some(active)
}

/// Every tag in use, sorted, each once.
pub fn tags(entries: &[Entry]) -> Vec<String> {
    let mut found: Vec<String> = entries.iter().flat_map(|e| e.tags.clone()).collect();
    found.sort();
    found.dedup();
    found
}

/// Every kind that has something under `name`.
///
/// A name can be an alias *and* a script, which is a mistake rather than a feature — and the way to
/// report it is to be able to see it.
pub fn kinds_of(store: &Store, name: &str) -> Vec<Kind> {
    Kind::every()
        .iter()
        .copied()
        .filter(|kind| get(store, *kind, name).is_some())
        .collect()
}

/// Remove one. Answers whether it was there.
///
/// **A tombstone, not an erasure.** The record stays with its stamp buried, because a row that
/// simply vanished would be indistinguishable from one this machine never had — and the other end
/// of a sync, seeing a macro we lack, would hand it straight back. See [`crate::track::stamp`].
///
/// A script is a file, and is deleted; [`scripts::ledger`] turns the missing file into a tombstone
/// when the next sync runs.
pub fn remove(store: &Store, kind: Kind, name: &str) -> bool {
    if kind == Kind::Script {
        return scripts::remove(name);
    }
    let Some(entry) = get(store, kind, name) else {
        return false;
    };
    let mut stamp = entry.stamp;
    stamp.bury();
    // The body is kept: it costs a few bytes, and it is what lets a sync tell the far end *which*
    // macro is gone rather than only that something was.
    let buried = Entry { stamp, ..entry };
    crate::store::set(store, &key(kind, name), encode(&buried).as_bytes()).is_ok()
}

/// Everything stored, the scripts included, sorted by kind then name.
pub fn all(store: &Store) -> Vec<Entry> {
    let mut found = database_entries(store);
    found.extend(scripts::all());
    found.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.name.cmp(&b.name)));
    found
}

/// Everything in the database: every kind but the scripts.
pub fn database_entries(store: &Store) -> Vec<Entry> {
    let mut found = Vec::new();
    for kind in Kind::every().iter().copied() {
        if kind == Kind::Script {
            continue;
        }
        let prefix = format!("{}/", kind.word());
        for full in crate::store::keys(store, &prefix) {
            let Some(name) = full.strip_prefix(&prefix) else {
                continue;
            };
            if let Some(entry) = get(store, kind, name) {
                found.push(entry);
            }
        }
    }
    found.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.name.cmp(&b.name)));
    found
}

/// Store one and keep the snapshot in step, which is the only way callers should write.
///
/// Separate from [`put`] so that a caller writing a batch pays for one snapshot rather than one per
/// entry — and so that forgetting to rewrite it is a thing you have to do deliberately.
pub fn put_and_publish(store: &Store, entry: &Entry) -> Result<(), String> {
    put(store, entry)?;
    publish(store)
}

/// The same, for a removal.
pub fn remove_and_publish(store: &Store, kind: Kind, name: &str) -> Result<bool, String> {
    let gone = remove(store, kind, name);
    publish(store)?;
    Ok(gone)
}

/// Rewrite everything derived from what the database now says.
///
/// [`mod@snapshot`] is what a starting shell reads, and [`sourced`] is what another shell
/// *sources* to have the aliases. Scripts are files already and have nothing derived from them.
pub fn publish(store: &Store) -> Result<(), String> {
    let entries = database_entries(store);
    snapshot::write(&entries)?;
    // **A failure here is reported, not fatal.** What is written out is a convenience for other
    // programs; a `$HOME` that cannot be written to is a reason to say so and still have stored the
    // macro, which is what the database already did.
    if let Err(problem) = sourced::publish(&entries) {
        // **Said out loud as well as recorded.** `messages::say` writes into the buffer `oslo
        // messages` reads, and a one-shot `oslo macros add` prints that buffer to nobody — so a
        // publish that refused to overwrite somebody's file, or a `$HOME` that could not be
        // written, reported success and left no trace anywhere the person would look.
        eprintln!("oslo macros: {problem}");
        crate::messages::say(
            crate::messages::Level::Warn,
            "macros",
            format!("the copies for other shells could not be written: {problem}"),
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "macros/tests.rs"]
mod tests;
