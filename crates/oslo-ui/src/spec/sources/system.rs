//! The names this machine keeps: users, groups, shells, variables, services, timezones.
//!
//! ```text
//!   chown ⇥                    bresilla   1000            user
//!   chsh -s ⇥                  /usr/bin/fish              shell
//!   unset PA⇥                  PATH   /usr/bin:…          variable
//!   systemctl start ng⇥        nginx.service   service    service
//!   timedatectl set-timezone ⇥ Europe/Amsterdam           timezone
//! ```
//!
//! Six sources in one file because they are one idea — a name the system already wrote down
//! somewhere — and because each is a dozen lines. Splitting them would be six files of imports.
//!
//! # What is read once, and what is not
//!
//! Users, groups, shells, service units and timezones are read once: adding a user mid-line is not
//! a thing that happens. Variables are read every time, because **the shell itself changes them** —
//! `export X=1` then `unset <Tab>` has to see `X`, and a cache filled at startup would be wrong
//! before the first command finished.

use super::{Suggestion, colons, read};
use std::sync::OnceLock;

static USERS: OnceLock<Vec<(String, String)>> = OnceLock::new();
static GROUPS: OnceLock<Vec<(String, String)>> = OnceLock::new();
static SERVICES: OnceLock<Vec<String>> = OnceLock::new();
static ZONES: OnceLock<Vec<String>> = OnceLock::new();

/// Everyone with an account, their uid beside them.
///
/// `/etc/passwd` only. LDAP and the rest live behind `getent`, which is a process — and a machine
/// whose users come from a directory server has thousands of them, which is not a menu.
pub fn users() -> Vec<Suggestion> {
    USERS
        .get_or_init(|| named("/etc/passwd"))
        .iter()
        .map(|(name, id)| Suggestion::new(name, id, "user"))
        .collect()
}

/// Every group, its gid beside it.
pub fn groups() -> Vec<Suggestion> {
    GROUPS
        .get_or_init(|| named("/etc/group"))
        .iter()
        .map(|(name, id)| Suggestion::new(name, id, "group"))
        .collect()
}

/// `name:x:id:…` — the shape `/etc/passwd` and `/etc/group` share.
fn named(path: &str) -> Vec<(String, String)> {
    named_from(&read(path))
}

/// The parse itself, so a test can state every case without a file to put them in.
fn named_from(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| {
            let fields = colons(line);
            let name = fields.first()?;
            (!name.is_empty()).then(|| (name.to_string(), fields.get(2).unwrap_or(&"").to_string()))
        })
        .collect()
}

/// Every variable in this process's environment, its value beside it.
///
/// **Never cached**: `export X=1` then `unset <Tab>` has to see `X`, and a cache filled at startup
/// would be wrong before the first command finished.
///
/// The value is truncated, because a completion menu is not the place to print `$LS_COLORS`.
pub fn variables() -> Vec<Suggestion> {
    let mut found: Vec<Suggestion> = std::env::vars()
        .map(|(name, value)| Suggestion::new(name, shorten(&value), "variable"))
        .collect();
    found.sort_unstable_by(|a, b| a.value.cmp(&b.value));
    found
}

/// A value short enough to sit in a column.
fn shorten(value: &str) -> String {
    const MOST: usize = 40;
    match value.char_indices().nth(MOST) {
        Some((at, _)) => format!("{}…", &value[..at]),
        None => value.to_string(),
    }
}

/// The systemd units installed on this machine.
///
/// Read from the unit directories rather than from `systemctl list-units`, which is a process and
/// on a cold cache a slow one. What that costs is the *state* column — a directory listing cannot
/// say whether a unit is running — so the note is the unit's type instead, which is the part that
/// tells `nginx.service` from `nginx.socket`.
///
/// Read once: units arrive with a package installation, not while you are typing.
pub fn services() -> Vec<Suggestion> {
    SERVICES
        .get_or_init(|| {
            const WHERE: &[&str] = &[
                "/etc/systemd/system",
                "/run/systemd/system",
                "/usr/lib/systemd/system",
                "/lib/systemd/system",
            ];
            let mut found: Vec<String> = WHERE
                .iter()
                .filter_map(|dir| std::fs::read_dir(dir).ok())
                .flatten()
                .flatten()
                .filter_map(|entry| {
                    let name = entry.file_name().to_str()?.to_string();
                    // A unit is `name.kind`; anything else in these directories is a drop-in
                    // directory or a symlink farm, and neither is a thing to start.
                    is_a_unit(&name).then_some(name)
                })
                .collect();
            found.sort_unstable();
            found.dedup();
            found
        })
        .iter()
        .map(|name| {
            let kind = name.rsplit('.').next().unwrap_or_default();
            Suggestion::new(name, kind, "service")
        })
        .collect()
}

/// Whether a name in a unit directory is a unit rather than a drop-in.
fn is_a_unit(name: &str) -> bool {
    const KINDS: &[&str] = &[
        ".service",
        ".socket",
        ".timer",
        ".target",
        ".mount",
        ".path",
        ".slice",
        ".scope",
        ".automount",
        ".swap",
    ];
    KINDS.iter().any(|kind| name.ends_with(kind))
}

/// The login shells this machine offers, for `chsh -s` and `usermod -s`.
///
/// `/etc/shells` is the list, and it is the list `chsh` itself checks against — a shell missing
/// from it is one `chsh` will refuse, so offering anything else would be offering a refusal.
pub fn shells() -> Vec<Suggestion> {
    read("/etc/shells")
        .lines()
        .map(|line| line.split('#').next().unwrap_or_default().trim())
        .filter(|line| line.starts_with('/'))
        .map(|path| Suggestion::new(path, "", "shell"))
        .collect()
}

/// Every timezone name, in the `Region/City` form everything takes.
///
/// A walk of `/usr/share/zoneinfo`, cached: about 450 names that change when the tzdata package
/// does, which is not during a session.
///
/// **`posix/` and `right/` are skipped.** They are two more complete copies of the same tree, and
/// including them would treble the list to say the same thing three ways. The loose files at the
/// top — `zone.tab`, `leapseconds`, `tzdata.zi` — are data about the zones rather than zones.
pub fn timezones() -> Vec<Suggestion> {
    ZONES
        .get_or_init(|| {
            let mut found = Vec::new();
            zones(std::path::Path::new("/usr/share/zoneinfo"), &mut found);
            found.sort_unstable();
            found
        })
        .iter()
        .map(|name| Suggestion::new(name, "", "timezone"))
        .collect()
}

fn zones(dir: &std::path::Path, found: &mut Vec<String>) {
    const SKIP: &[&str] = &["posix", "right"];
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if path.is_dir() {
            if !SKIP.contains(&name.as_str()) {
                zones(&path, found);
            }
            continue;
        }
        // A zone name always has a region in it. The bare files at the top of the tree are tables
        // and leap-second data, and `Factory` is a placeholder nobody sets.
        if let Some(zone) = path
            .to_str()
            .and_then(|p| p.strip_prefix("/usr/share/zoneinfo/"))
            && zone.contains('/')
        {
            found.push(zone.to_string());
        }
    }
}

/// The name behind a uid, for a source that has a number and wants a person.
///
/// Reuses whatever [`users`] already read, so this is a lookup and not a second parse.
pub(super) fn user_named(uid: u32) -> String {
    let wanted = uid.to_string();
    USERS
        .get_or_init(|| named("/etc/passwd"))
        .iter()
        .find(|(_, id)| *id == wanted)
        .map(|(name, _)| name.clone())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "system/tests.rs"]
mod tests;
