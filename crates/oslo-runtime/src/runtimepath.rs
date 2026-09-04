//! Where oslo looks for Lua: a path of roots, not a directory.
//!
//! This is neovim's model, and it is the one `hexe` and `trek` use — three tools in one family
//! that each invented a layout would stop being a family, and a person who learned any of them
//! would learn nothing about the others.
//!
//! An ordered list of roots, each with the same layout inside:
//!
//! ```text
//! <root>/plugin/**/*.lua    run at startup, alphabetically
//! <root>/lua/               modules for `require`, never run on their own
//! <root>/after/plugin/      run after everything else
//! ```
//!
//! The list, in the order it is read:
//!
//! ```text
//! ~/.config/oslo                 yours
//! /etc/xdg/oslo                  the system's
//! ~/.local/share/oslo/site       where packages install
//!   + site/pack/*/start/*        each one, as its own root
//! ~/.local/share/oslo/runtime    oslo's own
//! …/after                        the same list, reversed
//! ```
//!
//! # What this replaced
//!
//! `conf.d/` was fish's answer to the same problem and it worked, but it was fish's name for
//! neovim's `plugin/`, with the opposite order and no `lua/` or `after/` beside it. One family, one
//! layout: what a person learns here carries to the other two.
//!
//! **`plugin/` runs and `lua/` is required.** A tool that runs everything it finds leaves a plugin
//! author nowhere to keep a helper, and every helper then has to defend itself against running
//! twice.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub const RUN_DIR: &str = "plugin";
pub const LUA_DIR: &str = "lua";
pub const AFTER_DIR: &str = "after";

/// How deep `plugin/**` is walked, and how many files may run. Bounded because the path reaches
/// directories oslo does not own.
const MAX_DEPTH: usize = 8;
const MAX_FILES: usize = 512;

/// One root on the path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    pub path: PathBuf,
    /// An `after` root: same layout, read last.
    pub after: bool,
}

/// One file to run, and the root it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginFile {
    pub path: PathBuf,
    /// The root, not the `plugin/` directory: a plugin's data sits beside its `plugin/` and `lua/`,
    /// so the root is the only useful thing to hand it.
    pub root: PathBuf,
}

impl PluginFile {
    /// What to call it in a message: the part below its root, which is short and still unambiguous.
    pub fn label(&self) -> String {
        self.path
            .strip_prefix(&self.root)
            .unwrap_or(&self.path)
            .display()
            .to_string()
    }
}

fn config_home() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("oslo"))
}

fn data_home() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("oslo"))
}

/// Where a package installs to.
pub fn site_dir() -> Option<PathBuf> {
    Some(data_home()?.join("site"))
}

/// Where oslo's own Lua lives.
pub fn runtime_dir() -> Option<PathBuf> {
    Some(data_home()?.join("runtime"))
}

/// Every root, in the order they are read.
///
/// The `after` half is the first half reversed, so the root that comes first — yours — also gets the
/// last word. That is what makes `after/` an override seam rather than just another place to put
/// files.
pub fn roots() -> Vec<Root> {
    let mut head: Vec<PathBuf> = Vec::new();
    head.extend(config_home());
    head.push(PathBuf::from("/etc/xdg/oslo"));
    if let Some(site) = site_dir() {
        head.push(site.clone());
        head.extend(packages(&site));
    }
    head.extend(runtime_dir());

    let mut out: Vec<Root> = head
        .iter()
        .map(|path| Root {
            path: path.clone(),
            after: false,
        })
        .collect();
    out.extend(head.iter().rev().map(|path| Root {
        path: path.join(AFTER_DIR),
        after: true,
    }));
    out
}

/// `pack/<any>/start/<plugin>` under the site directory, each a root of its own.
///
/// The `<any>` level lets a person group what they installed — by where it came from, by what it is
/// for — without oslo having an opinion about the grouping. A plugin is laid out exactly like a
/// config root, so one can be developed beside your `init.lua` and moved into a package later
/// without being edited.
fn packages(site: &Path) -> Vec<PathBuf> {
    let pack = site.join("pack");
    let mut out = Vec::new();
    for group in sorted_dirs(&pack) {
        let start = pack.join(group).join("start");
        for name in sorted_dirs(&start) {
            out.push(start.join(name));
        }
    }
    out
}

/// Directory names in `dir`, sorted, symlinks resolved.
///
/// Sorted because directory order is filesystem order: it differs between machines and changes after
/// a reinstall, so an unsorted walk makes load order something nobody can reproduce.
///
/// Symlinks resolved because linking a plugin into a root is how people develop one, and a walk that
/// skipped links would work everywhere except on the machine the plugin is being written on.
fn sorted_dirs(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| !name.starts_with('.'))
        .collect();
    names.sort();
    names
}

/// Every file that would run, in the order it would run.
///
/// `<root>/plugin/**/*.lua` for each root in path order, sorted within a directory, subdirectories
/// after the files beside them.
pub fn plugin_files(roots: &[Root]) -> Vec<PluginFile> {
    let mut out = Vec::new();
    for root in roots {
        walk(&root.path.join(RUN_DIR), &root.path, &mut out, 0);
    }
    out
}

fn walk(dir: &Path, root: &Path, out: &mut Vec<PluginFile>, depth: usize) {
    if depth >= MAX_DEPTH || out.len() >= MAX_FILES {
        return;
    }

    let mut files: Vec<PathBuf> = Vec::new();
    let mut subs: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        // `is_dir`/`is_file` follow symlinks, which is what makes a linked-in plugin work.
        if path.is_dir() {
            subs.push(path);
        } else if path.is_file() && name.ends_with(".lua") {
            files.push(path);
        }
    }
    files.sort();
    subs.sort();

    for path in files {
        if out.len() >= MAX_FILES {
            return;
        }
        out.push(PluginFile {
            path,
            root: root.to_path_buf(),
        });
    }
    for sub in subs {
        walk(&sub, root, out, depth + 1);
    }
}

/// `package.path` for `require`, built from the same roots.
///
/// `<root>/lua/?.lua` for every root, so a plugin's helper is `require("thing.util")` wherever the
/// plugin lives. The config directory itself stays on it — not only its `lua/` — because a fragment
/// beside `init.lua` has always been `require("aliases")` and moving that would break every config
/// in existence.
pub fn require_path() -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(cfg) = config_home() {
        let cfg = cfg.display();
        parts.push(format!("{cfg}/?.lua"));
        parts.push(format!("{cfg}/?/init.lua"));
    }
    for root in roots() {
        let root = root.path.display();
        parts.push(format!("{root}/{LUA_DIR}/?.lua"));
        parts.push(format!("{root}/{LUA_DIR}/?/init.lua"));
    }
    parts.join(";")
}

/// Whether plugins run at all. `--noplugin`, or `OSLO_NOPLUGIN`.
///
/// The first question when a shell misbehaves is "is it me or a plugin?", and a shell with no way to
/// start without them makes that unanswerable.
pub fn enabled() -> bool {
    !DISABLED.load(Ordering::Relaxed) && std::env::var_os("OSLO_NOPLUGIN").is_none()
}

/// Process-wide, not per-thread: `--noplugin` is read while argv is parsed and asked about again
/// from the shell loop, which is not necessarily the same thread.
pub fn disable() {
    DISABLED.store(true, Ordering::Relaxed);
}

static DISABLED: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
mod tests;
