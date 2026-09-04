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

/// Reading is a `OnceLock`, so the second Tab costs nothing and both answers are the same list.
#[test]
fn the_list_is_read_once() {
    let first = all();
    let second = all();
    assert!(std::ptr::eq(first, second));
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
    for host in all {
        assert!(!host.source.is_empty(), "{} has no source", host.name);
        assert!(usable(&host.name), "{} should not be offered", host.name);
    }
}
