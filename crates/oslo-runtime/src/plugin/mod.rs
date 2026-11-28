//! Somebody else's Lua, found on the [runtimepath] and run at startup.
//!
//! # Loaded at startup, not by a stub
//!
//! The obvious design is a stub builtin per declared name, which loads the plugin when called. It
//! does not work, and the shell says so plainly:
//!
//! ```text
//! oslo: note: shell state is busy; an oslo.* call that reaches the shell cannot run from here.
//! ```
//!
//! A builtin runs *while the shell holds its state*, and `oslo.register_builtin` needs that state to
//! register anything — so a plugin loaded from inside a builtin can never register the builtin it
//! was loaded to provide. The stub is not a fixable design; it is the wrong place.
//!
//! So loading happens once, at startup, where the state is free.
//!
//! # What this replaced
//!
//! It used to be lazier: an index of every declared name, and a plugin loaded when a typed line
//! mentioned one of its words. That bought a startup measured in hundreds of microseconds, and cost
//! a manifest, a generated index, a trust ledger, and one wart nobody could explain — `type note`
//! before `note` had ever been typed reported it as not found, because the plugin providing it had
//! not run.
//!
//! Loading everything costs a few milliseconds on a shell that starts in under one, and the wart is
//! gone: every plugin's commands exist at the first prompt.
//!
//! # What loads plugins
//!
//! Only the interactive shell. `oslo -c` and a script never read `init.lua` and never walk the path
//! either: a plugin extends the shell you type at, and a script that depended on one would break for
//! anybody who had not installed it.
//!
//! [runtimepath]: crate::runtimepath

pub mod doctor;
pub mod health;
pub mod loading;
/// The assertions a plugin writes about itself, and the harness that runs them.
pub mod test;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

thread_local! {
    /// Which of the user's secrets each plugin may read, as the user's own config granted them.
    ///
    /// **Granted by the person, not declared by the plugin.** A plugin listing its own permissions
    /// decides nothing — it can list everything. This is filled from `init.lua`, which runs first,
    /// so every grant is in place before any plugin loads.
    static GRANTS: RefCell<HashMap<String, Vec<String>>> = RefCell::new(HashMap::new());
    /// Set once [`load_all`] starts: from then on no grant is taken.
    static GRANTS_CLOSED: Cell<bool> = const { Cell::new(false) };
}

/// Let `name` read these of the user's secrets. Keyed, so granting twice replaces.
pub fn grant_secrets(name: &str, secrets: Vec<String>) {
    GRANTS.with(|slot| slot.borrow_mut().insert(name.to_string(), secrets));
}

/// Why a grant may not be made now, if it may not: while a plugin loads, or once plugins have
/// started loading. What is left is the user's `init.lua`.
pub fn grant_refused() -> Option<&'static str> {
    if loading::current().is_some() {
        return Some("oslo.plugin.secrets: a plugin may not grant; only your init.lua does");
    }
    if GRANTS_CLOSED.with(Cell::get) {
        return Some(
            "oslo.plugin.secrets: plugins have loaded; a grant goes in init.lua, which runs first",
        );
    }
    None
}

fn granted(name: &str) -> Vec<String> {
    GRANTS.with(|slot| slot.borrow().get(name).cloned().unwrap_or_default())
}

/// Where packages install to: `site/pack/<any>/start/<plugin>`.
pub fn directory() -> Option<PathBuf> {
    crate::runtimepath::site_dir().map(|site| site.join("pack"))
}

/// The name a plugin is attributed under: the directory its root is.
///
/// Not a name it chose for itself — a plugin naming itself would be a plugin naming somebody else,
/// and this decides which of the user's secrets it may read.
fn plugin_name(root: &Path) -> String {
    root.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Run every plugin on the runtimepath, in path order.
///
/// **A raise is reported and the rest still load.** One plugin failing is one of several, and taking
/// the shell down with it is worse than doing without it.
///
/// Nothing is asked and nothing is approved: what is on the path runs, because somebody put it
/// there. A prompt would only ask them to confirm a decision they already made by copying the
/// directory in.
pub fn load_all() {
    GRANTS_CLOSED.with(|closed| closed.set(true));
    if !crate::runtimepath::enabled() {
        return;
    }
    let roots = crate::runtimepath::roots();
    for file in crate::runtimepath::plugin_files(&roots) {
        let name = plugin_name(&file.root);
        // Attributed while it runs, so a handle `oslo.secret` hands out during the load knows which
        // plugin asked for it.
        let outcome = loading::while_loading(&name, &granted(&name), || {
            crate::lua::engine::load_plugin_file(&file.path, &file.root)
        });
        if let Err(problem) = outcome {
            oslo_base::messages::error(format!("plugin {}", file.label()), problem);
        }
    }
}

/// Run one root's plugins straight out of a directory, without it being on the path.
///
/// For `oslo plugin test`, so an author can run what they are writing where it sits.
pub fn load_from(directory: &Path) -> Result<String, String> {
    let root = crate::runtimepath::Root {
        path: directory.to_path_buf(),
        after: false,
    };
    let files = crate::runtimepath::plugin_files(std::slice::from_ref(&root));
    if files.is_empty() {
        return Err(format!(
            "{}/plugin/ has no .lua files in it",
            directory.display()
        ));
    }
    let name = plugin_name(directory);
    for file in files {
        loading::while_loading(&name, &granted(&name), || {
            crate::lua::engine::load_plugin_file(&file.path, &file.root)
        })?;
    }
    Ok(name)
}

#[cfg(test)]
#[path = "mod/tests.rs"]
mod tests;
