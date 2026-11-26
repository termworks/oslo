//! The stored scripts: executable files in one directory.
//!
//! ```text
//! ~/.local/sbin/NAME        a script that is on
//! ~/.local/sbin/.off/NAME   a script that is off
//! ```
//!
//! Every file in either place whose first line is a `#!` shebang is a script. The file is the whole
//! record: its name is the script's name, its contents are the body, the `# tags: a, b` line in its
//! leading comment block holds the tags, and the directory it sits in says whether it is on. Nothing
//! about a script is kept in the macro database.
//!
//! oslo finds a script the way any program is found, through `$PATH`.

pub mod ledger;

use super::{Entry, Kind, UNSTAMPED};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The subdirectory holding the scripts that are off.
const OFF: &str = ".off";

/// `$OSLO_MACROS_BIN`, or `~/.local/sbin`.
pub fn directory() -> Option<PathBuf> {
    if let Some(named) = std::env::var_os("OSLO_MACROS_BIN")
        && !named.is_empty()
    {
        return Some(PathBuf::from(named));
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".local/sbin"))
}

/// `<directory>/.off`, where a script goes when it is turned off everywhere.
pub fn off_directory() -> Option<PathBuf> {
    Some(directory()?.join(OFF))
}

/// The two places a script can be, with whether a script there is on.
fn places() -> Vec<(PathBuf, bool)> {
    match (directory(), off_directory()) {
        (Some(on), Some(off)) => vec![(on, true), (off, false)],
        _ => Vec::new(),
    }
}

/// Every script, sorted by name. A name in both places is the one that is on.
pub fn all() -> Vec<Entry> {
    let mut found: BTreeMap<String, Entry> = BTreeMap::new();
    for (dir, active) in places() {
        let Ok(listing) = std::fs::read_dir(&dir) else {
            continue;
        };
        for item in listing.flatten() {
            let name = item.file_name().to_string_lossy().into_owned();
            if found.contains_key(&name) {
                continue;
            }
            if let Some(entry) = read(&dir.join(&name), &name, active) {
                found.insert(name, entry);
            }
        }
    }
    found.into_values().collect()
}

/// The script called `name`, from whichever place holds it.
pub fn get(name: &str) -> Option<Entry> {
    places()
        .into_iter()
        .find_map(|(dir, active)| read(&dir.join(name), name, active))
}

/// The file the script called `name` is, on or off.
pub fn path_of(name: &str) -> Option<PathBuf> {
    places()
        .into_iter()
        .map(|(dir, _)| dir.join(name))
        .find(|path| read(path, name, true).is_some())
}

/// One file as a script, or `None` when it is not one.
fn read(path: &Path, name: &str, active: bool) -> Option<Entry> {
    if name.starts_with('.') || !super::valid_name(name) {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let body = String::from_utf8(std::fs::read(path).ok()?).ok()?;
    if !body.starts_with("#!") {
        return None;
    }
    Some(Entry {
        kind: Kind::Script,
        name: name.to_string(),
        tags: tags_of(&body),
        body,
        created: seconds(meta.created().or_else(|_| meta.modified())),
        active,
        stamp: UNSTAMPED,
    })
}

/// When a file was last written, in Unix seconds.
pub(crate) fn modified(path: &Path) -> u64 {
    seconds(std::fs::metadata(path).and_then(|meta| meta.modified())) as u64
}

fn seconds(when: std::io::Result<std::time::SystemTime>) -> i64 {
    when.ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

/// Write a script into the place its state names, with its tags in its header.
pub fn write(entry: &Entry) -> Result<(), String> {
    if !super::valid_name(&entry.name) || entry.name.starts_with('.') {
        return Err(format!("{:?} is not a name a script can have", entry.name));
    }
    let (Some(on), Some(off)) = (directory(), off_directory()) else {
        return Err("no $HOME, so there is nowhere to keep scripts".to_string());
    };
    let (target, other) = match entry.active {
        true => (on.join(&entry.name), off.join(&entry.name)),
        false => (off.join(&entry.name), on.join(&entry.name)),
    };
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    write_one(&target, &with_tags(&entry.body, &entry.tags))?;
    if other.exists() {
        std::fs::remove_file(&other).map_err(|e| format!("{}: {e}", other.display()))?;
    }
    Ok(())
}

/// One file, replaced through a rename, keeping the mode an existing file had.
fn write_one(path: &Path, body: &str) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path)
        .map(|meta| meta.permissions().mode())
        .unwrap_or(0o700);
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let scratch = path.with_file_name(format!(".{name}.oslo-new"));
    let problem = |e: std::io::Error| format!("{}: {}", path.display(), crate::error::reason(&e));
    let mut file = std::fs::File::create(&scratch).map_err(problem)?;
    file.write_all(body.as_bytes()).map_err(problem)?;
    file.set_permissions(std::fs::Permissions::from_mode(mode))
        .map_err(problem)?;
    drop(file);
    std::fs::rename(&scratch, path).map_err(problem)
}

/// Delete the script called `name`. Answers whether there was one.
pub fn remove(name: &str) -> bool {
    let mut gone = false;
    while let Some(path) = path_of(name) {
        if std::fs::remove_file(&path).is_err() {
            break;
        }
        gone = true;
    }
    gone
}

/// Move a script on or off. `None` when there is no script called `name`.
pub fn set_active(name: &str, active: bool) -> Option<bool> {
    let from = path_of(name)?;
    let to = match active {
        true => directory()?.join(name),
        false => off_directory()?.join(name),
    };
    if from != to {
        std::fs::create_dir_all(to.parent()?).ok()?;
        std::fs::rename(&from, &to).ok()?;
    }
    Some(active)
}

/// Whether `path` is a script this session has turned off.
pub fn is_session_off(path: &Path) -> bool {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return false;
    };
    directory().is_some_and(|dir| dir == parent)
        && super::live::session::is_off(&crate::track::session::id(), &name.to_string_lossy())
}

/// The lines of the leading comment block that follows the shebang, with their positions.
fn header<'a>(lines: &'a [&'a str]) -> impl Iterator<Item = (usize, &'a str)> + 'a {
    let skip = usize::from(lines.first().is_some_and(|line| line.starts_with("#!")));
    lines
        .iter()
        .copied()
        .enumerate()
        .skip(skip)
        .take_while(|(_, line)| {
            let line = line.trim();
            line.is_empty() || line.starts_with('#')
        })
}

/// What follows `tags:` on a `# tags: a, b` line.
fn tag_line(line: &str) -> Option<&str> {
    line.trim_start()
        .strip_prefix('#')?
        .trim_start()
        .strip_prefix("tags:")
}

/// The tags a script's header names, in order, each once.
pub fn tags_of(body: &str) -> Vec<String> {
    let lines: Vec<&str> = body.lines().collect();
    let Some(list) = header(&lines).find_map(|(_, line)| tag_line(line)) else {
        return Vec::new();
    };
    let mut tags: Vec<String> = Vec::new();
    for tag in list.split(|c: char| c == ',' || c.is_whitespace()) {
        if super::valid_tag(tag) && !tags.iter().any(|had| had == tag) {
            tags.push(tag.to_string());
        }
    }
    tags
}

/// `body` with its tag line saying `tags`: replaced, added after the shebang, or removed.
pub fn with_tags(body: &str, tags: &[String]) -> String {
    if tags_of(body) == tags {
        return body.to_string();
    }
    let new = (!tags.is_empty()).then(|| format!("# tags: {}", tags.join(", ")));
    let mut lines: Vec<String> = body.lines().map(str::to_string).collect();
    let borrowed: Vec<&str> = body.lines().collect();
    let at = header(&borrowed).find_map(|(at, line)| tag_line(line).map(|_| at));
    match (at, new) {
        (Some(at), Some(new)) => lines[at] = new,
        (Some(at), None) => {
            lines.remove(at);
        }
        (None, Some(new)) => {
            let after = usize::from(body.starts_with("#!")).min(lines.len());
            lines.insert(after, new);
        }
        (None, None) => {}
    }
    let mut text = lines.join("\n");
    if body.ends_with('\n') || body.is_empty() {
        text.push('\n');
    }
    text
}

/// A scripts directory of its own for a test, behind one lock for the whole crate.
///
/// `$OSLO_MACROS_BIN` is process-wide, so every test that reaches a script holds this.
#[cfg(test)]
pub(crate) mod testdir {
    use std::sync::{Mutex, MutexGuard};

    pub(crate) struct Own {
        pub dir: tempfile::TempDir,
        _lock: MutexGuard<'static, ()>,
    }

    pub(crate) fn own() -> Own {
        static LOCK: Mutex<()> = Mutex::new(());
        let lock = LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        // SAFETY: serialised by `LOCK`; nothing outside these tests reads the variable.
        unsafe { std::env::set_var("OSLO_MACROS_BIN", dir.path()) };
        Own { dir, _lock: lock }
    }

    impl Drop for Own {
        fn drop(&mut self) {
            // SAFETY: still under `LOCK`, which is released after this.
            unsafe { std::env::remove_var("OSLO_MACROS_BIN") };
        }
    }
}

#[cfg(test)]
#[path = "scripts/tests.rs"]
mod tests;
