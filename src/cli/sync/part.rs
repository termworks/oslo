//! The three things that travel, and the two halves each of them has.
//!
//! # One shape, three storages
//!
//! Every part does the same four steps: ask the far end for a snapshot, merge it against ours,
//! report what moved, hand the merged copy back. What differs is only what a snapshot *is* — a
//! database file for history and macros (the scripts carried in it as records), a tar-less bundle
//! of sealed files for secrets — and that difference is confined to [`snapshot_of`] and
//! [`absorb`].

use super::ssh::{agree, over_ssh};
use oslo::track::{Track, sync_files};
use std::path::PathBuf;

// Only a build with secrets has a directory of sealed files to carry.
#[cfg(feature = "secrets")]
mod bundle;
mod report;

pub use report::Moved;

/// One of the things a machine keeps.
///
/// **Secrets are here only in a build that has them.** A part that could be named and then refused
/// would be worse than one that is simply absent: `--only secrets` says *no such part* on a build
/// without the feature, and the help lists what that build can actually do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    History,
    Macros,
    #[cfg(feature = "secrets")]
    Secrets,
}

impl Part {
    pub fn word(self) -> &'static str {
        match self {
            Part::History => "history",
            Part::Macros => "macros",
            #[cfg(feature = "secrets")]
            Part::Secrets => "secrets",
        }
    }
}

/// Everything, in the order it is reported.
pub fn every() -> Vec<Part> {
    vec![
        Part::History,
        Part::Macros,
        #[cfg(feature = "secrets")]
        Part::Secrets,
    ]
}

pub fn named(word: &str) -> Option<Part> {
    match word {
        "history" => Some(Part::History),
        "macros" | "macro" => Some(Part::Macros),
        #[cfg(feature = "secrets")]
        "secrets" | "secret" => Some(Part::Secrets),
        _ => None,
    }
}

/// Sync every named part with `remote`, under the profile named.
///
/// **The fingerprints are checked once, for all of them.** The profile key is what says these two
/// machines are the same person's, and a per-part check would ask the same question three times.
pub fn named_profile(
    remote: &str,
    wanted: &[Part],
    profile: &str,
    dry_run: bool,
) -> Result<(), String> {
    agree(remote, profile)?;

    for part in wanted {
        one(remote, *part, profile, dry_run)?;
    }
    if dry_run {
        println!("dry run — nothing was written on either side");
    }
    Ok(())
}

/// Ask, merge, report, hand back.
fn one(remote: &str, part: Part, profile: &str, dry_run: bool) -> Result<(), String> {
    let held = tempfile::Builder::new()
        .prefix("oslo-sync-")
        .tempdir()
        .map_err(|e| format!("no temporary directory: {e}"))?;
    let theirs = held.path().join("theirs");

    let answered = over_ssh(remote, &["profile", "send", part.word(), profile], None)?;
    let snapshot = off_the_wire(remote, part, &answered)?;
    absorb(part, &theirs, &snapshot)?;

    let moved = merge_into(part, &theirs, profile, dry_run)?;
    // Said before the far end is written to, so the line a person reads is in the order the work
    // happened rather than after a round trip that may take a while.
    report::say(part, &moved);

    if dry_run {
        return Ok(());
    }
    // Merged again over there rather than dropped on top, so anything recorded on that machine
    // while this was running survives.
    let merged = on_the_wire(part, &snapshot_of(part, &theirs)?);
    // What it prints is deliberately nothing — see [`report::say_far`].
    over_ssh(
        remote,
        &["profile", "receive", part.word(), profile],
        Some(&merged),
    )?;
    Ok(())
}

/// What every answer from the far end begins with, so that a stale oslo cannot be mistaken for an
/// empty one.
///
/// **Measured against the failure it exists for.** An oslo without `sync` does not fail loudly: the
/// word is not one of its tools, so it looks for a *program* called `sync` — and `sync(1)` is a real
/// one on most systems. It ran, printed nothing, and exited 0, which the near end read as *that
/// machine has no history* and then died somewhere else entirely. A version that says its own name
/// turns that into one sentence.
const WIRE: &str = "OSLOSYNC1 ";

/// A part's bytes, with the header that says what they are.
fn on_the_wire(part: Part, payload: &[u8]) -> Vec<u8> {
    let mut bytes = format!("{WIRE}{}\n", part.word()).into_bytes();
    bytes.extend_from_slice(payload);
    bytes
}

/// The other way, refusing anything that did not come from an oslo that speaks this.
fn off_the_wire(remote: &str, part: Part, bytes: &[u8]) -> Result<Vec<u8>, String> {
    let stale = || {
        format!(
            "{remote}: did not answer as an oslo that can sync.\n  \
             Its oslo is most likely too old — `oslo sync` needs one on both machines."
        )
    };
    let rest = bytes.strip_prefix(WIRE.as_bytes()).ok_or_else(stale)?;
    let end = rest
        .iter()
        .position(|byte| *byte == b'\n')
        .ok_or_else(stale)?;
    // The part is echoed back, so a mixed-up answer is caught rather than merged into the wrong
    // store — which for secrets would mean writing sealed files where history goes.
    if &rest[..end] != part.word().as_bytes() {
        return Err(format!(
            "{remote}: was asked for {} and answered with something else",
            part.word()
        ));
    }
    Ok(rest[end + 1..].to_vec())
}

/// Merge the far end's copy into ours, and theirs into it, so both hold the union.
fn merge_into(
    part: Part,
    theirs: &std::path::Path,
    profile: &str,
    dry: bool,
) -> Result<Moved, String> {
    match part {
        Part::History => {
            let mine = history_path(profile)?;
            if !mine.exists() {
                return Err(format!(
                    "{profile}: no history at {} — nothing to sync yet",
                    mine.display()
                ));
            }
            let report = sync_files(&mine, &theirs.join("hist.db"), dry)?;
            Ok(Moved::from_history(&report))
        }
        Part::Macros => {
            let mine = oslo::macros::open()?;
            let other = oslo::macros::Store::open(&theirs.join("macros.db"))
                .ok_or_else(|| "the macros that arrived cannot be opened".to_string())?;
            let report = oslo::macros::sync::merge(&mine, &other, dry)?;
            // Everything derived: the flat file a starting shell reads, and the aliases another
            // shell sources.
            if !dry && !report.quiet() {
                oslo::macros::publish(&mine)?;
            }
            // The scripts, through a scratch store: they are files here and records on the wire.
            let held = tempfile::Builder::new()
                .prefix("oslo-scripts-")
                .tempdir()
                .map_err(|e| format!("no temporary directory: {e}"))?;
            let scratch = oslo::macros::Store::open(&held.path().join("scripts.db"))
                .ok_or_else(|| "cannot make a store to merge the scripts in".to_string())?;
            let scripts = oslo::macros::sync::merge_scripts(&scratch, &other, dry)?;
            Ok(Moved::from_macros(&report.plus(scripts)))
        }
        #[cfg(feature = "secrets")]
        Part::Secrets => {
            let mine = oslo::secrets::Store::selected(None)?;
            let other = oslo::secrets::Store {
                directory: theirs.join("secrets"),
                ..mine.clone()
            };
            let report = oslo::secrets::sync::merge(&mine, &other, dry)?;
            Ok(Moved::from_secrets(&report))
        }
    }
}

/// Where this profile's history lives.
pub fn history_path(name: &str) -> Result<PathBuf, String> {
    if !oslo::track::profile::valid(name) {
        return Err(format!("{name:?} is not a usable profile name"));
    }
    let data = std::env::var("XDG_DATA_HOME").ok();
    let home = std::env::var("HOME").ok();
    oslo::track::profile::profile_dir(data.as_deref(), home.as_deref(), name)
        .map(|dir| dir.join("hist.db"))
        .ok_or_else(|| "no $XDG_DATA_HOME and no $HOME, so there is no store".to_string())
}

/// One part of this machine, as bytes to send.
fn snapshot_of(part: Part, from: &std::path::Path) -> Result<Vec<u8>, String> {
    match part {
        Part::History => std::fs::read(from.join("hist.db"))
            .map_err(|e| format!("{}: {e}", from.join("hist.db").display())),
        Part::Macros => std::fs::read(from.join("macros.db"))
            .map_err(|e| format!("{}: {e}", from.join("macros.db").display())),
        #[cfg(feature = "secrets")]
        Part::Secrets => bundle::pack(&from.join("secrets")),
    }
}

/// The other way: bytes that arrived, laid out where a merge can read them.
///
/// **Nothing arriving is not the same as an empty file arriving.** A machine that has never made a
/// macro sends no bytes at all — see [`mine`] — and writing those as a zero-length `macros.db`
/// produces a file that is not a database and cannot be opened. Left absent, the merge opens a
/// fresh empty store instead and everything here travels to them, which is the right answer.
fn absorb(part: Part, into: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::create_dir_all(into).map_err(|e| format!("{}: {e}", into.display()))?;
    match part {
        // An empty body means that machine has none of these. A fresh empty database is what it
        // means, and lets the ordinary merge run and send everything here over to them; the
        // alternative, a zero-length file, is not a database and cannot be opened at all.
        Part::History if bytes.is_empty() => oslo::track::Track::open(&into.join("hist.db"))
            .map(|_| ())
            .ok_or_else(|| "cannot make an empty history to merge against".to_string()),
        Part::Macros if bytes.is_empty() => oslo::macros::Store::open(&into.join("macros.db"))
            .map(|_| ())
            .ok_or_else(|| "cannot make an empty macro store to merge against".to_string()),
        Part::History => write_at(&into.join("hist.db"), bytes),
        Part::Macros => write_at(&into.join("macros.db"), bytes),
        #[cfg(feature = "secrets")]
        Part::Secrets => bundle::unpack(&into.join("secrets"), bytes),
    }
}

fn write_at(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// `oslo sync send WHAT PROFILE` — this machine's copy, on standard output.
pub fn send(args: &[String]) -> i32 {
    let Some(part) = args.first().and_then(|word| named(word)) else {
        return crate::cli::sync::fail("send needs one of: history, macros, secrets");
    };
    let profile = args
        .get(1)
        .cloned()
        .unwrap_or_else(oslo::track::profile::current);
    // Headed, so that the near end can tell "this machine has none of these" from "the thing that
    // answered was not an oslo that speaks this".
    match mine(part, &profile).and_then(|bytes| write_out(&on_the_wire(part, &bytes))) {
        Ok(()) => 0,
        Err(e) => crate::cli::sync::fail(&e),
    }
}

/// A consistent copy of one part of this machine.
///
/// **A snapshot rather than the file.** Both databases are live and a shell may be writing to one
/// this instant; copying the bytes would give the far end a file that half-opens.
fn mine(part: Part, profile: &str) -> Result<Vec<u8>, String> {
    match part {
        Part::History => {
            let path = history_path(profile)?;
            let Some(held) = scratch(&path)? else {
                return Ok(Vec::new());
            };
            Track::open_existing(&path, true)?.backup_to(&held.path())?;
            read_back(held.path())
        }
        Part::Macros => {
            let path = oslo::macros::database().ok_or("nowhere to read macros from")?;
            let held = tempfile::Builder::new()
                .prefix("oslo-send-")
                .tempdir()
                .map_err(|e| format!("no temporary directory: {e}"))?;
            let copy = held.path().join("copy.db");
            if path.exists() {
                oslo::macros::Store::open(&path)
                    .ok_or_else(|| format!("{}: cannot be opened", path.display()))?
                    .backup_to(&copy)?;
            }
            // The scripts travel as records beside the database's own.
            let store = oslo::macros::Store::open(&copy)
                .ok_or_else(|| format!("{}: cannot be opened", copy.display()))?;
            oslo::macros::scripts::ledger::stamp_into(&store)?;
            drop(store);
            read_back(copy)
        }
        #[cfg(feature = "secrets")]
        Part::Secrets => bundle::pack(&oslo::secrets::Store::selected(None)?.directory),
    }
}

/// Somewhere for a database to write its own consistent copy.
///
/// `None` when there is nothing to copy, which is not an error: a machine with no macros yet has
/// none to send, and the far end merging an empty snapshot is exactly right.
///
/// A directory rather than a temporary *file*, because `backup_to` writes the database itself and
/// refuses a destination that already exists — which is precisely what `tempfile()` hands back.
fn scratch(source: &std::path::Path) -> Result<Option<Copy>, String> {
    if !source.exists() {
        return Ok(None);
    }
    let held = tempfile::Builder::new()
        .prefix("oslo-send-")
        .tempdir()
        .map_err(|e| format!("no temporary directory: {e}"))?;
    Ok(Some(Copy { held }))
}

/// A temporary directory and the file inside it a copy goes to.
struct Copy {
    held: tempfile::TempDir,
}

impl Copy {
    fn path(&self) -> PathBuf {
        self.held.path().join("copy.db")
    }
}

fn read_back(path: PathBuf) -> Result<Vec<u8>, String> {
    std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))
}

fn write_out(bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let mut out = std::io::stdout();
    if out.write_all(bytes).is_err() || out.flush().is_err() {
        return Err("cannot write the snapshot".to_string());
    }
    Ok(())
}

/// `oslo sync receive WHAT PROFILE` — merge what arrives on standard input.
///
/// **Merged, never dropped on top.** Between the moment the other end asked for a copy and the
/// moment it hands one back, something here may have changed.
pub fn receive(args: &[String]) -> i32 {
    use std::io::Read;
    let Some(part) = args.first().and_then(|word| named(word)) else {
        return crate::cli::sync::fail("receive needs one of: history, macros, secrets");
    };
    let profile = args
        .get(1)
        .cloned()
        .unwrap_or_else(oslo::track::profile::current);

    let mut incoming = Vec::new();
    if let Err(e) = std::io::stdin().read_to_end(&mut incoming) {
        return crate::cli::sync::fail(&format!("cannot read standard input: {e}"));
    }
    if incoming.is_empty() {
        // Nothing at all on standard input is the near end deciding there was nothing to send,
        // which is not a failure. A *headed* answer with an empty body is the same thing said out
        // loud, and goes through the merge below.
        return 0;
    }
    let incoming = match off_the_wire("the machine that called", part, &incoming) {
        Ok(bytes) => bytes,
        Err(e) => return crate::cli::sync::fail(&e),
    };

    let held = match tempfile::Builder::new().prefix("oslo-receive-").tempdir() {
        Ok(dir) => dir,
        Err(e) => return crate::cli::sync::fail(&format!("no temporary directory: {e}")),
    };
    let theirs = held.path().join("theirs");
    if let Err(e) = absorb(part, &theirs, &incoming) {
        return crate::cli::sync::fail(&e);
    }
    match merge_into(part, &theirs, &profile, false) {
        Ok(moved) => {
            report::say_far(part, &moved);
            0
        }
        Err(e) => crate::cli::sync::fail(&e),
    }
}
