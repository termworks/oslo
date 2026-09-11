use super::*;

/// The names a person chose, one line at a time.
#[test]
fn a_config_gives_up_its_host_names() {
    let text = "\
Host prod
  HostName 10.0.0.7
Host build-01 build-02
  User ci
host lower-case-keyword
HOST=equals-is-a-separator-too
";
    assert_eq!(
        from_ssh_config(text),
        [
            "prod",
            "build-01",
            "build-02",
            "lower-case-keyword",
            "equals-is-a-separator-too"
        ]
    );
}

/// **`HostName` is not `Host`.** A prefix test would take it and offer the address behind an alias
/// the user deliberately made to avoid typing.
#[test]
fn hostname_is_not_host() {
    assert_eq!(from_ssh_config("HostName 10.0.0.7\n"), Vec::<String>::new());
    assert_eq!(from_ssh_config("Hostname box.lan\n"), Vec::<String>::new());
}

#[test]
fn known_hosts_gives_up_every_name_on_a_line() {
    let text = "\
one.example.com ssh-rsa AAAA
two.lan,two ssh-ed25519 AAAA
[gate.example.com]:2222 ssh-rsa AAAA
@cert-authority ca.example.com ssh-rsa AAAA
# a comment
";
    assert_eq!(
        from_known_hosts(text),
        [
            "one.example.com",
            "two.lan",
            "two",
            "gate.example.com",
            "ca.example.com"
        ]
    );
}

/// `[host]:port` is one host on a non-default port; the brackets are syntax, not the name.
#[test]
fn a_bracketed_port_is_unwrapped() {
    assert_eq!(unbracket("[gate.example.com]:2222"), "gate.example.com");
    assert_eq!(unbracket("[gate.example.com]"), "gate.example.com");
    assert_eq!(unbracket("plain.example.com"), "plain.example.com");
}

/// The address is field one and never a name; everything after it is.
#[test]
fn etc_hosts_gives_the_names_and_not_the_address() {
    let text = "\
127.0.0.1\tlocalhost localhost.localdomain
::1     ip6-localhost
192.168.1.10 nas nas.lan   # the box in the cupboard
";
    assert_eq!(
        from_etc_hosts(text),
        [
            "localhost",
            "localhost.localdomain",
            "ip6-localhost",
            "nas",
            "nas.lan"
        ]
    );
}

/// **Nothing that cannot be connected to, and nothing that cannot be read.**
#[test]
fn patterns_hashes_and_addresses_are_not_offered() {
    assert!(!usable("*"), "a pattern is not a machine");
    assert!(!usable("*.example.com"));
    assert!(!usable("build-?"));
    assert!(!usable("|1|abc=|def="), "a hashed entry has no name in it");
    assert!(!usable("!excluded"));
    assert!(!usable("127.0.0.1"), "an address is never half typed");
    assert!(!usable("::1"));
    assert!(!usable("192.168.1.10"));
    assert!(!usable(""));

    assert!(usable("prod"));
    assert!(usable("nas.lan"));
    assert!(usable("build-01"));
    // A name that merely *contains* digits and dots is still a name.
    assert!(usable("host1.example.com"));
}

/// While the files are unchanged the second Tab reads nothing and gets the same list.
#[test]
fn the_list_is_read_once_per_change() {
    let first = all();
    let second = all();
    assert!(Arc::ptr_eq(&first, &second));
}

/// Every name carries where it was found, and the first source to claim a name keeps it.
#[test]
fn a_name_is_credited_to_the_first_source_that_had_it() {
    // `gather` reads the real files, so this asserts the shape rather than the contents: whatever
    // is found, no name appears twice and every one names a source the menu can show.
    let all = all();
    let mut names: Vec<&str> = all.iter().map(|host| host.name.as_str()).collect();
    let before = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), before, "a host was offered twice");
    for host in all.iter() {
        assert!(!host.source.is_empty(), "{} has no source", host.name);
        assert!(usable(&host.name), "{} should not be offered", host.name);
    }
}

/// **The history is where the hosts are on an ordinary machine.** `HashKnownHosts yes` is the
/// Debian and Ubuntu default, so `known_hosts` holds `|1|…` HMACs with no name in them; with no
/// `~/.ssh/config` the file sources answer nothing at all, and what somebody typed is the only
/// record of where they go.
#[test]
fn a_machine_that_was_connected_to_is_remembered() {
    crate::recall::clear();
    crate::recall::seed(vec![
        ("ssh tron.netbird".to_string(), "shell".to_string()),
        ("ssh 172.30.0.248".to_string(), "shell".to_string()),
        ("ssh tron.netbird uptime".to_string(), "shell".to_string()),
        ("echo not a host at all".to_string(), "shell".to_string()),
    ]);
    let found = super::from_history();
    assert!(found.contains(&"tron.netbird".to_string()), "{found:?}");
    // An address somebody typed is a machine they go to, even though one out of `/etc/hosts`
    // would be noise.
    assert!(found.contains(&"172.30.0.248".to_string()), "{found:?}");
    // The argument to a remote command is not a second host.
    assert!(!found.contains(&"uptime".to_string()), "{found:?}");
    crate::recall::clear();
}

/// **A copy names its machine wherever the machine sits.** `rsync -a build/ ci@box:/srv` has its
/// remote *second*, so taking the first operand made the local directory `build/` a host.
#[test]
fn the_remote_half_of_a_copy_is_the_host() {
    crate::recall::clear();
    crate::recall::seed(vec![
        (
            "rsync -a build/ ci@buildbox:/srv/".to_string(),
            "shell".to_string(),
        ),
        (
            "scp report.pdf gate.lan:/tmp".to_string(),
            "shell".to_string(),
        ),
    ]);
    let found = super::from_history();
    assert!(found.contains(&"buildbox".to_string()), "{found:?}");
    assert!(found.contains(&"gate.lan".to_string()), "{found:?}");
    assert!(
        !found.contains(&"build/".to_string()),
        "a local path: {found:?}"
    );
    assert!(!found.contains(&"report.pdf".to_string()), "{found:?}");
    crate::recall::clear();
}
