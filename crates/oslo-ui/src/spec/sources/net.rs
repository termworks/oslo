//! The network as this machine has it: its interfaces, and the port names it knows.
//!
//! ```text
//!   tcpdump -i ⇥      wlan0   up                 interface
//!   nc localhost ⇥    https   443/tcp            port
//! ```

use super::{Suggestion, read};

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

/// The service names in `/etc/services`, each with its port and protocol.
///
/// **The name is the value and the number is the note**, which is the way round that helps: a
/// person types `nc host ht<Tab>` because they cannot remember 443, and every tool that takes a
/// port name here takes the number too.
///
/// A name appearing for both TCP and UDP is offered once, on whichever came first — the second row
/// would be the same word with a different note, which is a duplicate as far as the menu is
/// concerned.
pub fn ports() -> Vec<Suggestion> {
    let mut found = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for line in read("/etc/services").lines() {
        let line = line.split('#').next().unwrap_or_default();
        let mut fields = line.split_whitespace();
        let (Some(name), Some(port)) = (fields.next(), fields.next()) else {
            continue;
        };
        if seen.insert(name.to_string()) {
            found.push(Suggestion::new(name, port, "port"));
        }
    }
    found
}

#[cfg(test)]
#[path = "net/tests.rs"]
mod tests;
