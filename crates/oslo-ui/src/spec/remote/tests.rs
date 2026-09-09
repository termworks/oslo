use super::*;

/// What has been typed after `host:` splits into the directory to list and the fragment to match.
#[test]
fn a_half_typed_path_names_a_directory_and_a_fragment() {
    assert_eq!(split(""), ("", ""));
    assert_eq!(split("/var/lo"), ("/var/", "lo"));
    assert_eq!(split("/var/log/"), ("/var/log/", ""));
    assert_eq!(split("rel"), ("", "rel"));
    assert_eq!(split("/"), ("/", ""));
    assert_eq!(split("a/b/c"), ("a/b/", "c"));
}

/// **A directory is asked for once.** Walking `host:/usr/<Tab>lib/<Tab>` would otherwise open a
/// connection per keystroke rather than per directory.
#[test]
fn a_directory_is_listed_once_per_command() {
    let asked = Rc::new(std::cell::Cell::new(0));
    let count = Rc::clone(&asked);
    set_lister(Some(Rc::new(move |_host: &str, _dir: &str| {
        count.set(count.get() + 1);
        Some(vec![Entry {
            name: "log".into(),
            directory: true,
        }])
    })));

    assert_eq!(entries("box", "/var/").map(|e| e.len()), Some(1));
    assert_eq!(entries("box", "/var/").map(|e| e.len()), Some(1));
    assert_eq!(asked.get(), 1, "the second look reused the first answer");
    // A different directory is a different question.
    let _ = entries("box", "/srv/");
    assert_eq!(asked.get(), 2);
    set_lister(None);
}

/// **A machine that cannot be reached is remembered too**, or every keystroke pays the deadline.
/// `None` and an empty list are different answers: one was never read, the other is empty.
#[test]
fn an_unreachable_machine_is_not_asked_twice() {
    let asked = Rc::new(std::cell::Cell::new(0));
    let count = Rc::clone(&asked);
    set_lister(Some(Rc::new(move |_h: &str, _d: &str| {
        count.set(count.get() + 1);
        None
    })));

    assert_eq!(entries("gone", "/"), None);
    assert_eq!(entries("gone", "/"), None);
    assert_eq!(asked.get(), 1);

    // …until a command runs, because connecting a VPN is a thing done between two prompts.
    forget();
    assert_eq!(entries("gone", "/"), None);
    assert_eq!(asked.get(), 2);
    set_lister(None);
}

/// With nothing installed there is no remote completion, and asking is not an error.
#[test]
fn nothing_is_offered_without_a_lister() {
    set_lister(None);
    assert!(!available());
    assert_eq!(entries("box", "/"), None);
}
