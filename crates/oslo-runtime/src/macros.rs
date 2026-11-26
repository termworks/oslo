//! Opening the macro manager, from wherever it was asked for.
//!
//! Two things ask: `oslo macros show`, and the key bound to `oslo.macros.key` at the prompt. They
//! want the same screen over the same database, so the screen lives here rather than in either —
//! `oslo-ui` owns the drawing and the keys, `oslo-base` owns the store, and this is the short piece
//! that hands one to the other.
//!
//! # The screen closes to edit, and comes back
//!
//! Enter opens `$EDITOR`, which wants the alternate screen and the terminal's own modes. So the
//! manager exits, the editor runs, and the manager reopens with the query it had. Holding a
//! full-screen widget open around a child that also drives the terminal is how two programs end up
//! fighting over one termios.

use crate::editor;
use oslo_base::macros::{self, Entry, Kind};
use oslo_ui::manager::{Act, Backing, Item, Outcome};

/// Open the manager and run it until it is dismissed.
///
/// `None` when there is no terminal to draw on, which is what makes `oslo macros show | cat` print
/// a list instead of taking over a screen that is not there.
pub fn screen() -> Option<Result<(), String>> {
    let store = match macros::open() {
        Ok(store) => store,
        Err(problem) => return Some(Err(problem)),
    };
    let mut query = String::new();
    loop {
        let items = items(&store);
        let mut backing = Database { store: &store };
        match oslo_ui::manager::open(items, &query, &mut backing)? {
            Outcome::Cancelled => return Some(Ok(())),
            Outcome::Edit { item, query: asked } => {
                query = asked;
                if let Err(problem) = edit_one(&store, &item) {
                    return Some(Err(problem));
                }
            }
        }
    }
}

/// Everything the screen shows: the database and the scripts, then what the configuration defined.
fn items(store: &macros::Store) -> Vec<Item> {
    let session = oslo_base::track::session::id();
    let all = macros::all(store);
    let clashing = clashes(&all);
    let stored = all.into_iter().map(|entry| (entry, true));
    let elsewhere = macros::live::from_elsewhere()
        .into_iter()
        .map(|entry| (entry, false));
    stored
        .chain(elsewhere)
        .map(|(entry, from_database)| Item {
            kind: entry.kind.word().to_string(),
            first: summary(&entry),
            session_off: macros::live::session::is_off(&session, &entry.name),
            stored: from_database,
            clash: from_database && clashing.contains(&entry.name),
            name: entry.name,
            tags: entry.tags,
            created: entry.created,
            active: entry.active,
        })
        .collect()
}

/// The names more than one kind holds, sorted.
pub fn clashes(entries: &[Entry]) -> Vec<String> {
    let mut names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
    names.sort_unstable();
    let mut found: Vec<String> = names
        .windows(2)
        .filter(|pair| pair[0] == pair[1])
        .map(|pair| pair[0].to_string())
        .collect();
    found.dedup();
    found
}

/// What a row says about a macro, beside its name.
///
/// **An alias is its body; a function or a script is its language.** A row is one line and a script
/// is two hundred, so the first line of one is `#!/usr/bin/env python3` — which says nothing you
/// could not have guessed and takes the width a name and a tag can use. What is worth knowing at a
/// glance is what you are about to open, so the row says `python3` and Enter shows the rest.
pub fn summary(entry: &Entry) -> String {
    match entry.kind {
        Kind::Func | Kind::Script => macros::shebang_interpreter(&entry.body)
            .unwrap_or_else(|| entry.kind.extension(&entry.body).to_string()),
        _ => {
            let first = entry
                .body
                .lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("")
                .trim();
            match first.chars().count() > 60 {
                true => first.chars().take(57).collect::<String>() + "...",
                false => first.to_string(),
            }
        }
    }
}

/// Open one in the editor and store what comes back.
fn edit_one(store: &macros::Store, item: &Item) -> Result<(), String> {
    let Some(kind) = Kind::named(&item.kind) else {
        return Ok(());
    };
    if kind == Kind::Script {
        return match macros::scripts::path_of(&item.name) {
            Some(path) => editor::edit_file(&path).map(|_| ()),
            None => Ok(()),
        };
    }
    // **A configured alias is copied in before it is edited.** It came from a file this does not own
    // and cannot write back to; copying it makes it yours, which is also the only way to get one
    // that can be turned off.
    let entry = macros::get(store, kind, &item.name).unwrap_or_else(|| {
        macros::live::from_elsewhere()
            .into_iter()
            .find(|e| e.kind == kind && e.name == item.name)
            .unwrap_or_else(|| Entry::new(kind, &item.name, ""))
    });

    match editor::edit(&entry.body, kind.extension(&entry.body))? {
        None => Ok(()),
        Some(body) => macros::put_and_publish(store, &Entry { body, ..entry }),
    }
}

/// The screen's hands: it decides, this does it.
struct Database<'a> {
    store: &'a macros::Store,
}

impl Backing for Database<'_> {
    fn act(&mut self, item: &Item, act: Act) {
        let Some(kind) = Kind::named(&item.kind) else {
            return;
        };
        match act {
            Act::Forget => {
                let _ = macros::remove_and_publish(self.store, kind, &item.name);
            }
            Act::Everywhere(active) => {
                macros::set_active(self.store, kind, &item.name, active);
                let _ = macros::publish(self.store);
            }
            // **The session this process is in**, which for `oslo macros` — a child of the shell
            // whose macros it manages — is the shell's, through `$OSLO_SESSION`.
            Act::Session(off) => {
                let _ =
                    macros::live::session::set(&oslo_base::track::session::id(), &item.name, off);
            }
        }
    }
}
