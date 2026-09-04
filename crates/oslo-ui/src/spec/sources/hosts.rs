//! The machines this one already knows about, for `ssh`, `scp`, `sftp` and `rsync`.
//!
//! ```text
//!   scp report.pdf ba<Tab>
//!   ba-build-01    ~/.ssh/config
//!   backup.lan     ~/.ssh/known_hosts
//! ```
//!
//! # Why a shell has to do this
//!
//! `scp file user@<Tab>` is the completion people miss most when they move shells, and it cannot
//! come from a spec: the answer is not a property of `scp`, it is a property of *this machine*.
//! zsh has had `_hosts` since forever and it is the reason nobody notices it is there.
//!
//! # Where the names come from, and in what order
//!
//! ```text
//!   ~/.ssh/config              Host lines — the names a person chose
//!   ~/.ssh/known_hosts         machines actually connected to
//!   /etc/ssh/ssh_known_hosts   the same, system-wide
//!   /etc/hosts                 names this machine resolves without asking anyone
//! ```
//!
//! **The order decides which source a name is credited to, not where it lands in the menu** — the
//! dropdown does its own ranking, by what you have actually run. A host in both your config and
//! your `known_hosts` appears once, and says `ssh config`, because that is the name you chose to
//! give it rather than the one it answered with.
//!
//! Each name carries where it was found, shown beside it: a machine you do not recognise says
//! whether you invented it, connected to it once, or merely have it in `/etc/hosts`.
//!
//! # What is deliberately left out
//!
//! * **Wildcards.** `Host *` and `*.example.com` are patterns, not machines; inserting one produces
//!   a name that resolves to nothing.
//! * **Hashed `known_hosts` entries.** `HashKnownHosts yes` stores `|1|…` — a name nobody can read
//!   and nothing can connect to. Offering it would be offering a hash.
//! * **Bare addresses.** `127.0.0.1` and `::1` are in `/etc/hosts` on every machine and are never
//!   what somebody is half way through typing. zsh makes this a style; here it is simply the
//!   answer, because the shell that wants an address has one already.
//! * **`getent hosts` and NIS**, which zsh asks. Both are a process, or a network round trip, on
//!   the Tab key. The four files above are a `read` each and cover what a person actually types.
//!
//! # Read once
//!
//! A session's hosts do not change while you are typing, and this is on the Tab path. The files are
//! read on the first completion that asks and remembered; nothing re-reads them.

use std::collections::BTreeSet;
use std::sync::OnceLock;

static HOSTS: OnceLock<Vec<Host>> = OnceLock::new();

/// One machine, and where its name was found.
pub struct Host {
    pub name: String,
    /// Shown beside the name in the menu, so a name you do not recognise says where it came from.
    pub source: &'static str,
}

/// Every host, as the menu wants them — with whatever `user@` is already on the line kept.
///
/// **`user@` is carried through.** A candidate has to match the whole word or nothing does, and
/// `ci@ga` *is* the word — so an offer of the bare `gate.example.com` matches nothing and the menu
/// stays shut, which is what `scp f.txt ci@ga<Tab>` did before this. Whatever was typed up to the
/// last `@` goes back on the front of every host.
pub fn offers(word: &str) -> Vec<super::Suggestion> {
    let user = word.rfind('@').map(|at| &word[..=at]).unwrap_or_default();
    all()
        .iter()
        .map(|host| super::Suggestion::new(format!("{user}{}", host.name), host.source, "host"))
        .collect()
}

/// Every host name this machine knows.
pub fn all() -> &'static [Host] {
    HOSTS.get_or_init(gather)
}

fn gather() -> Vec<Host> {
    let mut found = Vec::new();
    let mut seen = BTreeSet::new();
    let home = std::env::var("HOME").unwrap_or_default();

    let mut take = |name: &str, source: &'static str| {
        if usable(name) && seen.insert(name.to_string()) {
            found.push(Host {
                name: name.to_string(),
                source,
            });
        }
    };

    if !home.is_empty() {
        for name in from_ssh_config(&read(&format!("{home}/.ssh/config"))) {
            take(&name, "ssh config");
        }
        for name in from_known_hosts(&read(&format!("{home}/.ssh/known_hosts"))) {
            take(&name, "known host");
        }
    }
    for name in from_known_hosts(&read("/etc/ssh/ssh_known_hosts")) {
        take(&name, "known host");
    }
    for name in from_etc_hosts(&read("/etc/hosts")) {
        take(&name, "/etc/hosts");
    }
    found
}

fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// `Host alias other` — one line may name several.
fn from_ssh_config(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = keyword(line, "host") else {
            continue;
        };
        names.extend(rest.split_whitespace().map(str::to_string));
    }
    names
}

/// The word a `Host`/`HostName` line begins with, case-insensitively as ssh reads it.
fn keyword<'a>(line: &'a str, want: &str) -> Option<&'a str> {
    let (first, rest) = line.split_once(|c: char| c.is_whitespace() || c == '=')?;
    first.eq_ignore_ascii_case(want).then(|| rest.trim())
}

/// `host,host2 ssh-rsa AAAA…`, or `[host]:2222 …`.
fn from_known_hosts(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // A marker line — `@cert-authority`, `@revoked` — puts the names one field later.
        let fields = match line.starts_with('@') {
            true => line.split_whitespace().nth(1),
            false => line.split_whitespace().next(),
        };
        let Some(field) = fields else {
            continue;
        };
        for name in field.split(',') {
            names.push(unbracket(name).to_string());
        }
    }
    names
}

/// `[example.com]:2222` is one host on a non-default port. The brackets are syntax.
fn unbracket(name: &str) -> &str {
    let Some(inner) = name.strip_prefix('[') else {
        return name;
    };
    match inner.split_once("]:") {
        Some((host, _)) => host,
        None => inner.trim_end_matches(']'),
    }
}

/// `127.0.0.1 localhost alias` — the address, then every name it answers to.
fn from_etc_hosts(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        names.extend(line.split_whitespace().skip(1).map(str::to_string));
    }
    names
}

/// Whether a name is worth offering: something a person could connect to and read.
fn usable(name: &str) -> bool {
    !name.is_empty()
        // A pattern, not a machine.
        && !name.contains(['*', '?'])
        // `HashKnownHosts yes`. There is no name in it to show.
        && !name.starts_with('|')
        // `Host !bad` — a negation inside a pattern list.
        && !name.starts_with('!')
        && !is_an_address(name)
}

/// A bare IPv4 or IPv6 address, which is never what somebody is half way through typing.
fn is_an_address(name: &str) -> bool {
    name.parse::<std::net::IpAddr>().is_ok()
}

#[cfg(test)]
#[path = "hosts/tests.rs"]
mod tests;
