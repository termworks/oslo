//! The names that run only after the `$PATH` search has failed, gathered for the prompt.
//!
//! A stored function and an autoloaded one. Dispatch finds each of them by asking
//! at the moment of the call, which is right for dispatch and useless to the interface: the
//! highlighter, the completer, the ghost and the correction all need the *set*, and they need it
//! without a database open per keystroke.
//!
//! So it is gathered here and published to [`oslo_base::vocab`], which is what those four read.
//! Gathered once per prompt rather than per keystroke — the set cannot change while a line is being
//! typed in a way that matters — and again the moment this shell writes to the store, so
//! `oslo macros add` is never followed by a prompt that denies the thing it just made.

use crate::env::Environment;
use oslo_base::macros::{self, Kind};

/// Re-read the names and hand them to the prompt.
///
/// Cheap when there is nothing to find, which is the overwhelming case: a `stat(2)` says whether a
/// macro database exists at all, and one `read_dir` covers the autoload directory.
pub fn refresh(env: &Environment) {
    let mut names: Vec<(String, &'static str)> = Vec::new();
    stored_into(&mut names);
    autoloaded_into(env, &mut names);
    oslo_base::vocab::set_dynamic(names);
}

/// The stored functions that are switched on.
///
/// The same filter dispatch applies — an inactive entry, or one this session turned off, is not a
/// name that runs, and the prompt must not claim otherwise.
fn stored_into(out: &mut Vec<(String, &'static str)>) {
    if !macros::database().is_some_and(|path| path.exists()) {
        return;
    }
    let Ok(store) = macros::open() else {
        return;
    };
    let session = oslo_base::track::session::id();
    for entry in macros::database_entries(&store) {
        if entry.kind != Kind::Func
            || !entry.active
            || macros::live::session::is_off(&session, &entry.name)
        {
            continue;
        }
        out.push((entry.name, "macro"));
    }
}

/// The files in `~/.config/oslo/functions`, each of which defines the function it is named for.
fn autoloaded_into(env: &Environment, out: &mut Vec<(String, &'static str)>) {
    let Some(dir) = crate::exec::simple::autoload::directory(env) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        // A directory named `x.sh` is not a function, and neither is anything `path_for` would
        // refuse. Asked of the same predicate the loader uses, so the two cannot drift again.
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(stem) = name.strip_suffix(".sh")
            && crate::exec::simple::autoload::loadable(stem)
        {
            out.push((stem.to_string(), "function"));
        }
    }
}
