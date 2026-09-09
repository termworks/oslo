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
//!   your own history           machines you have run `ssh`, `scp` or `rsync` against
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
//! * **Bare addresses out of a file.** `127.0.0.1` and `::1` are in `/etc/hosts` on every machine
//!   and are never what somebody is half way through typing. An address you *typed* is kept: see
//!   below.
//! * **`getent hosts` and NIS**, which zsh asks. Both are a process, or a network round trip, on
//!   the Tab key. The files above are a `read` each and cover what a person actually types.
//!
//! # Why the history is not an afterthought
//!
//! **`HashKnownHosts yes` is the default on Debian and Ubuntu**, so `known_hosts` is a column of
//! `|1|…` HMACs with no name in any of them. A person with no `~/.ssh/config` then has *nothing*
//! in the four files above — which is exactly what this feature met on the machine it was written
//! for: 29 hashed entries, no config, and `localhost` plus a row of `ip6-…` as the entire answer.
//!
//! What somebody has typed is the only remaining record of where they go, and it is better
//! evidence than a file: a name in `known_hosts` is a machine that answered once, while a name
//! after `ssh` is a machine they meant.
//!
//! # Read once
//!
//! A session's hosts do not change while you are typing, and this is on the Tab path. The files are
//! read on the first completion that asks and remembered; nothing re-reads them. The history is
//! already in memory — the editor seeds it at startup — so it costs no read either.

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

    // `typed` marks a name that came from a command somebody actually ran, which is held to a
    // looser test — see [`usable`].
    let mut take = |name: &str, source: &'static str, typed: bool| {
        let wanted = match typed {
            true => a_name_at_all(name),
            false => usable(name),
        };
        if wanted && seen.insert(name.to_string()) {
            found.push(Host {
                name: name.to_string(),
                source,
            });
        }
    };

    if !home.is_empty() {
        for name in from_ssh_config(&read(&format!("{home}/.ssh/config"))) {
            take(&name, "ssh config", false);
        }
        for name in from_known_hosts(&read(&format!("{home}/.ssh/known_hosts"))) {
            take(&name, "known host", false);
        }
    }
    for name in from_known_hosts(&read("/etc/ssh/ssh_known_hosts")) {
        take(&name, "known host", false);
    }
    for name in from_etc_hosts(&read("/etc/hosts")) {
        take(&name, "/etc/hosts", false);
    }
    for name in from_history() {
        take(&name, "connected before", true);
    }
    found
}

/// The machines this shell has actually been told to connect to.
///
/// **Without this the feature has no data on an ordinary machine.** `HashKnownHosts yes` is the
/// default on Debian and Ubuntu, so `known_hosts` holds `|1|…` HMACs with no name in them, and a
/// user with no `~/.ssh/config` then has *nothing* to complete — which is exactly what happened on
/// the machine this was written for: 29 hashed entries, no config, and the only readable names on
/// the whole system were `localhost` and the `ip6-…` rows of `/etc/hosts`.
///
/// A name that was typed after `ssh` is a machine by construction — better evidence than a file,
/// because it is somewhere this person actually goes. The history is already in memory (the editor
/// seeds it at startup), so this costs no read and no fork on the Tab path.
fn from_history() -> Vec<String> {
    /// Commands whose first bare operand is a machine.
    const HOST_FIRST: &[&str] = &["ssh", "mosh", "telnet", "ping"];
    /// Commands that take paths, where the machine is the operand wearing an `@` or a `:`.
    const COPIES: &[&str] = &["scp", "sftp", "rsync"];

    let mut names = Vec::new();
    for line in crate::recall::for_language("shell") {
        let mut words = line.split_whitespace();
        let Some(command) = words
            .next()
            .map(|first| first.rsplit('/').next().unwrap_or(first))
        else {
            continue;
        };
        let host_first = HOST_FIRST.contains(&command);
        if !host_first && !COPIES.contains(&command) {
            continue;
        }
        let mut bare_operands = 0;
        for word in words.filter(|word| !word.starts_with('-')) {
            // **`user@host` and `host:path` name the machine wherever they sit.** `rsync -a build/
            // ci@box:/srv` has its remote *second*, so taking the first operand made `build/` a
            // host — a local directory offered as a machine.
            if let Some((_, after)) = word.split_once('@') {
                push_host(&mut names, after.split(':').next().unwrap_or(after));
                continue;
            }
            if let Some((before, _)) = word.split_once(':')
                && !before.contains('/')
            {
                push_host(&mut names, before);
                continue;
            }
            // A bare word is a machine only for `ssh` and its like, and only the first one — after
            // that come the command and its arguments.
            bare_operands += 1;
            if host_first && bare_operands == 1 {
                push_host(&mut names, word);
            }
        }
    }
    names
}

/// Keep a word that could be a machine's name. A path is not one.
fn push_host(names: &mut Vec<String>, word: &str) {
    if !word.is_empty() && !word.contains('/') {
        names.push(word.to_string());
    }
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
    a_name_at_all(name) && !is_an_address(name)
}

/// The part of [`usable`] that holds however the name was found.
fn a_name_at_all(name: &str) -> bool {
    !name.is_empty()
        // A pattern, not a machine.
        && !name.contains(['*', '?'])
        // `HashKnownHosts yes`. There is no name in it to show.
        && !name.starts_with('|')
        // `Host !bad` — a negation inside a pattern list.
        && !name.starts_with('!')
}

/// A bare address, which is noise when it came out of a *file* — every machine has `127.0.0.1` and
/// `::1` in `/etc/hosts` and nobody is ever half way through typing one.
///
/// **An address somebody typed is different**, and is kept: `ssh 172.30.0.248` is evidence of a
/// machine that person goes to, and on a host whose `known_hosts` is hashed it may be the only
/// evidence there is. See the `typed` argument in [`gather`].
fn is_an_address(name: &str) -> bool {
    name.parse::<std::net::IpAddr>().is_ok()
}

#[cfg(test)]
#[path = "hosts/tests.rs"]
mod tests;
