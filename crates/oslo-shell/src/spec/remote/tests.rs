use super::*;

/// `ls -p` marks a directory with a trailing slash and nothing else with anything, which is the
/// only type information every `ls` agrees on.
#[test]
fn a_trailing_slash_is_what_marks_a_directory() {
    let found = entries_from("bin/\nnotes.md\nsrc/\n\n  \n");
    assert_eq!(
        found,
        vec![
            Entry {
                name: "bin".into(),
                directory: true
            },
            Entry {
                name: "notes.md".into(),
                directory: false
            },
            Entry {
                name: "src".into(),
                directory: true
            },
        ]
    );
}

/// **The path is one word on the far side, whatever is in it.** It was typed by a person, goes into
/// a string the remote shell parses, and a space or a quote in a directory name must not become a
/// second word — let alone a second command.
#[test]
fn a_path_cannot_become_a_command() {
    assert_eq!(quoted_for_remote("/srv/www"), "'/srv/www'");
    assert_eq!(quoted_for_remote("/a b"), "'/a b'");
    assert_eq!(quoted_for_remote("/it's"), "'/it'\\''s'");
    assert_eq!(quoted_for_remote("/x; rm -rf /"), "'/x; rm -rf /'");
    assert_eq!(quoted_for_remote("/$(whoami)"), "'/$(whoami)'");
    assert_eq!(quoted_for_remote("/`id`"), "'/`id`'");
}

/// A leading `~` is left for the far shell to expand — `'~/'` would be a directory called tilde —
/// and everything after it is still quoted.
#[test]
fn a_tilde_is_expanded_over_there() {
    assert_eq!(quoted_for_remote("~"), "~");
    assert_eq!(quoted_for_remote("~/"), "~/");
    assert_eq!(quoted_for_remote("~/pro jects"), "~/'pro jects'");
}

/// **A destination beginning with `-` is an option to `ssh`**, and this word was typed at a prompt.
/// argv keeps a shell out of it; it is no defence against `ssh`'s own option parsing.
#[test]
fn a_destination_that_is_really_a_flag_is_refused() {
    assert!(!a_plausible_destination("-oProxyCommand=touch /tmp/pwned"));
    assert!(!a_plausible_destination("-l"));
    assert!(!a_plausible_destination(""));
    assert!(!a_plausible_destination("two words"));
    assert!(a_plausible_destination("build-01"));
    assert!(a_plausible_destination("ci@gate.example.com"));
    assert!(a_plausible_destination("tron.netbird"));
}

/// A bare `host:` lists the login directory, which is what `ls` with no operand does.
#[test]
fn no_directory_means_the_login_directory() {
    assert_eq!(remote_command(""), "LC_ALL=C ls -1Ap");
    assert_eq!(remote_command("/srv/"), "LC_ALL=C ls -1Ap -- '/srv/'");
}

/// Whether this machine can ssh to itself without being asked anything.
///
/// The live tests below need a real server and a key that opens it. Neither is true of every
/// build machine, so they say so and stop rather than failing for a reason that is not oslo's.
fn can_reach_localhost() -> bool {
    std::process::Command::new("ssh")
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=2",
            "-T",
            "localhost",
            "true",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// The whole path, against a real server: a directory is marked, a file is not, and a name with a
/// space in it survives the quoting.
#[test]
fn a_real_directory_is_listed() {
    if !can_reach_localhost() {
        eprintln!("skipped: no passwordless ssh to localhost");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("www")).unwrap();
    std::fs::write(dir.path().join("notes.md"), "x").unwrap();
    std::fs::write(dir.path().join("a file with spaces"), "x").unwrap();

    let at = format!("{}/", dir.path().display());
    let found = list("localhost", &at).expect("localhost answered");
    assert!(
        found.contains(&Entry {
            name: "www".into(),
            directory: true
        }),
        "{found:?}"
    );
    assert!(
        found.contains(&Entry {
            name: "notes.md".into(),
            directory: false
        }),
        "{found:?}"
    );
    assert!(
        found.contains(&Entry {
            name: "a file with spaces".into(),
            directory: false
        }),
        "the quoting held: {found:?}"
    );
}

/// **A machine that cannot be reached answers `None`, not an empty listing**, and it does so within
/// the deadline rather than hanging the editor. `192.0.2.1` is TEST-NET-1: routable-looking and
/// guaranteed to go nowhere, so the connection stalls instead of being refused — the case the
/// deadline exists for.
#[test]
fn an_unreachable_machine_gives_up_in_time() {
    let started = std::time::Instant::now();
    assert_eq!(list("192.0.2.1", "/etc/"), None);
    let took = started.elapsed();
    assert!(
        took < std::time::Duration::from_secs(5),
        "the editor was held for {took:?}"
    );
}
