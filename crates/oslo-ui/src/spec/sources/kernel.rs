//! What the running kernel has loaded and what it will let you set.
//!
//! ```text
//!   rmmod ⇥        btrfs      3 users        module
//!   sysctl net.i⇥  net.ipv4.ip_forward       sysctl
//! ```

use super::{Suggestion, read};
use std::sync::OnceLock;

static SYSCTLS: OnceLock<Vec<String>> = OnceLock::new();

/// The modules loaded right now, for `rmmod`, `modinfo` and `modprobe -r`.
///
/// `/proc/modules` rather than `lsmod`, which is a process that reads `/proc/modules`.
///
/// Not cached: loading a module is a thing that happens in the middle of a session, and often the
/// reason the next command is being typed.
pub fn modules() -> Vec<Suggestion> {
    read("/proc/modules")
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let name = fields.next()?;
            // `name size used-by-count dependencies …` — the count is the useful one, because a
            // module with users is a module `rmmod` will refuse.
            let users = fields.nth(1).unwrap_or("0");
            let note = match users {
                "0" => String::new(),
                many => format!("{many} users"),
            };
            Some(Suggestion::new(name, note, "module"))
        })
        .collect()
}

/// Every kernel parameter, in the dotted form `sysctl` takes.
///
/// `/proc/sys` is the same tree with slashes: `net/ipv4/ip_forward` is `net.ipv4.ip_forward`. The
/// walk finds about 1,800 of them, which is why it is cached — the set does not change without a
/// module being loaded, and nothing here is worth a second walk on the next Tab.
///
/// **No note.** The note would be each parameter's current value, and that is 1,800 more file reads
/// for a column nobody is reading while they type.
pub fn sysctls() -> Vec<Suggestion> {
    SYSCTLS
        .get_or_init(|| {
            let mut found = Vec::new();
            walk(std::path::Path::new("/proc/sys"), &mut found);
            found.sort_unstable();
            found
        })
        .iter()
        .map(|name| Suggestion::new(name, "", "sysctl"))
        .collect()
}

/// Depth-first through `/proc/sys`, collecting the leaves as dotted names.
fn walk(dir: &std::path::Path, found: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        // `file_type` rather than `metadata`, so a symlink is seen as a symlink and not followed —
        // `/proc/sys` has loops in it.
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        match kind.is_dir() {
            true => walk(&path, found),
            false if kind.is_file() => {
                if let Some(name) = path.to_str().and_then(|p| p.strip_prefix("/proc/sys/")) {
                    found.push(name.replace('/', "."));
                }
            }
            false => {}
        }
    }
}

#[cfg(test)]
#[path = "kernel/tests.rs"]
mod tests;
