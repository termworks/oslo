//! What the machine knows, offered as completions.
//!
//! ```text
//!   kill 12⇥          $pids        1247  cargo            pid
//!   chown ⇥           $users       bresilla                user
//!   umount /m⇥        $mounts      /mnt/backup             mount
//!   ssh ⇥             $hosts       build-01  known host    host
//! ```
//!
//! # Why these are sources and not specs
//!
//! A spec describes a *command*: its options, how many arguments it takes, what each one means.
//! None of that produces the answer to `kill <Tab>`, because the answer is not a fact about `kill`
//! — it is a fact about **this machine at this moment**, and it is the same answer `pkill`,
//! `renice`, `strace` and `tail --pid` all want.
//!
//! So a source is written once and pointed at from as many specs as want it. That is the whole
//! economy of the thing: nineteen sources here answer arguments for dozens of commands, where a spec
//! per command would be dozens of specs that each go stale on their own.
//!
//! # Cheap enough for the Tab key
//!
//! **Nothing here starts a process.** Every source is a read of `/proc`, `/sys`, `/etc` or the
//! environment — the files the kernel and libc already keep for exactly these questions. zsh asks
//! `getent`, `ps` and `systemctl` for some of the same answers, and pays a fork per Tab for it.
//!
//! What is cached and what is not follows from what changes:
//!
//! | | |
//! |---|---|
//! | [`hosts`], [`system`] users, groups, shells, services, timezones | read once |
//! | [`procs`] | read every time — a pid list a minute old is a list of the wrong pids |
//! | [`disk`], [`net`], [`system`] variables | read every time — the shell itself changes them |
//! | [`kernel`] sysctls | read once — an 1,800-file walk, and the set does not move |
//! | [`kernel`] modules | read every time — loading one is often why the next command is typed |
//!
//! # Adding one
//!
//! A function answering `Vec<Suggestion>`, and a line in [`offers`]. Then any spec — shipped, or
//! one you wrote — can name it as `$whatever` in a positional or a flag's value.

pub mod disk;
pub mod hosts;
pub mod kernel;
pub mod net;
pub mod procs;
pub mod system;

/// One thing a source offers.
pub struct Suggestion {
    /// What Tab inserts.
    pub value: String,
    /// The second column: what this is, when the value alone does not say. A pid's command, a
    /// host's file, a service's state.
    pub note: String,
    /// The kind column. Every spec offer is otherwise labelled `value`, which says nothing when
    /// the rows are processes.
    pub kind: &'static str,
}

impl Suggestion {
    pub fn new(
        value: impl Into<String>,
        note: impl Into<String>,
        kind: &'static str,
    ) -> Suggestion {
        Suggestion {
            value: value.into(),
            note: note.into(),
            kind,
        }
    }
}

/// What `$name` offers, or `None` if it is not a source at all.
///
/// **`None` rather than an empty list**, because the two mean different things to the caller: a
/// source that found nothing has answered, and a name that is not a source has not — the latter
/// falls through to the shell, which is how `$(git branch)` and `$bash(…)` still work.
pub fn offers(name: &str, word: &str) -> Option<Vec<Suggestion>> {
    Some(match name {
        "hosts" => hosts::offers(word),
        "pids" => procs::pids(),
        "signals" => procs::signals(),
        "users" => system::users(),
        "groups" => system::groups(),
        "variables" => system::variables(),
        "interfaces" => net::interfaces(),
        "ports" => net::ports(),
        "mounts" => disk::mounts(),
        "fstab" => disk::fstab(),
        "devices" => disk::devices(),
        "filesystems" => disk::filesystems(),
        "swaps" => disk::swaps(),
        "services" => system::services(),
        "shells" => system::shells(),
        "timezones" => system::timezones(),
        "terminals" => procs::terminals(),
        "modules" => kernel::modules(),
        "sysctls" => kernel::sysctls(),
        _ => return None,
    })
}

/// Read a file, or nothing at all if it is not there.
///
/// Every source here reads files that may be absent — no `/proc` in a container built without it,
/// no `/etc/group` on a minimal image — and an absent file is an empty list rather than an error.
/// A completion is not the place to report that the system is unusual.
pub(super) fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// The fields of a colon-separated line — `/etc/passwd` and `/etc/group` are both this shape.
pub(super) fn colons(line: &str) -> Vec<&str> {
    line.split(':').collect()
}

#[cfg(test)]
mod tests;
