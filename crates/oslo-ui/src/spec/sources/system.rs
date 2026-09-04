//! The names this machine keeps: users, groups, variables, interfaces, mounts, services.
//!
//! ```text
//!   chown ⇥              bresilla   1000              user
//!   umount /m⇥           /mnt/backup   ext4           mount
//!   unset PA⇥            PATH       /usr/bin:…        variable
//!   ip link set ⇥        wlan0                        interface
//!   systemctl start ng⇥  nginx.service   enabled      service
//! ```
//!
//! Six sources in one file because they are one idea — a name the system already wrote down
//! somewhere — and because each is a dozen lines. Splitting them would be six files of imports.
//!
//! # What is read once, and what is not
//!
//! Users, groups and service units are read once: adding a user mid-line is not a thing that
//! happens. Variables, mounts and interfaces are read every time, because **the shell itself
//! changes them** — `export X=1` then `unset <Tab>` has to see `X`, and a `mount` you just ran has
//! to appear in `umount <Tab>`. A cache there would be wrong within one command.

use super::{Suggestion, colons, read};
use std::sync::OnceLock;

static USERS: OnceLock<Vec<(String, String)>> = OnceLock::new();
static GROUPS: OnceLock<Vec<(String, String)>> = OnceLock::new();
static SERVICES: OnceLock<Vec<String>> = OnceLock::new();

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

/// The network interfaces this machine has.
///
/// `/sys/class/net` is a directory of them, which is why this needs neither `ip` nor a netlink
/// socket. Not cached: interfaces come and go with a VPN, a container, a cable.
pub fn interfaces() -> Vec<Suggestion> {
    let Ok(entries) = std::fs::read_dir("/sys/class/net") else {
        return Vec::new();
    };
    let mut found: Vec<Suggestion> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            let state = read(&format!("/sys/class/net/{name}/operstate"));
            Some(Suggestion::new(name, state.trim(), "interface"))
        })
        .collect();
    found.sort_unstable_by(|a, b| a.value.cmp(&b.value));
    found
}

/// Where things are mounted, with the filesystem beside each.
///
/// **The mount point, not the device**, because that is what `umount`, `df` and `findmnt` take —
/// and the device is the field a person is least likely to be able to type from memory.
///
/// Not cached: a `mount` you just ran has to appear in the next `umount <Tab>`.
pub fn mounts() -> Vec<Suggestion> {
    read("/proc/mounts")
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let _device = fields.next()?;
            let point = fields.next()?;
            let kind = fields.next().unwrap_or_default();
            // `/proc/mounts` escapes a space in a path as `\040`, and a menu row showing the escape
            // would insert something that does not exist.
            Some(Suggestion::new(point.replace("\\040", " "), kind, "mount"))
        })
        .collect()
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

#[cfg(test)]
#[path = "system/tests.rs"]
mod tests;
