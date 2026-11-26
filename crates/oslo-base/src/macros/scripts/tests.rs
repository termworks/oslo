use super::*;
use std::os::unix::fs::PermissionsExt;

fn place(dir: &Path, name: &str, body: &str) -> PathBuf {
    std::fs::create_dir_all(dir).expect("mkdir");
    let path = dir.join(name);
    std::fs::write(&path, body).expect("write");
    path
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).expect("stat").permissions().mode() & 0o777
}

/// Any file with a shebang is a script; anything else in the directory is not.
#[test]
fn a_file_with_a_shebang_is_a_script() {
    let own = testdir::own();
    place(own.dir.path(), "deploy", "#!/usr/bin/env bash\necho hi\n");
    place(own.dir.path(), "notes.txt", "just text\n");
    place(own.dir.path(), ".hidden", "#!/bin/sh\n");
    std::fs::create_dir_all(own.dir.path().join("subdir")).expect("mkdir");

    let names: Vec<String> = all().into_iter().map(|entry| entry.name).collect();
    assert_eq!(names, ["deploy"]);
    let deploy = get("deploy").expect("a script");
    assert_eq!(deploy.kind, Kind::Script);
    assert_eq!(deploy.body, "#!/usr/bin/env bash\necho hi\n");
    assert!(deploy.active);
    assert!(get("notes.txt").is_none());
}

/// `.off/` holds the scripts that are off, and a name in both places is the live one.
#[test]
fn a_script_in_the_off_directory_is_off() {
    let own = testdir::own();
    let off = own.dir.path().join(".off");
    place(&off, "quiet", "#!/bin/sh\n");
    place(&off, "both", "#!/bin/sh\necho off\n");
    place(own.dir.path(), "both", "#!/bin/sh\necho on\n");

    assert!(!get("quiet").expect("found").active);
    let both: Vec<Entry> = all().into_iter().filter(|e| e.name == "both").collect();
    assert_eq!(both.len(), 1, "one entry per name");
    assert!(both[0].active);
    assert!(both[0].body.contains("echo on"));
}

/// A new script is private and executable; an existing one keeps the mode it had.
#[test]
fn writing_makes_an_executable_and_keeps_a_mode() {
    let own = testdir::own();
    write(&Entry::new(Kind::Script, "fresh", "#!/bin/sh\n")).expect("write");
    assert_eq!(mode(&own.dir.path().join("fresh")), 0o700);

    let shared = place(own.dir.path(), "shared", "#!/bin/sh\n");
    std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    write(&Entry::new(Kind::Script, "shared", "#!/bin/sh\necho new\n")).expect("write");
    assert_eq!(mode(&shared), 0o755);
    assert_eq!(get("shared").expect("there").body, "#!/bin/sh\necho new\n");
    let leftovers = std::fs::read_dir(own.dir.path())
        .expect("ls")
        .flatten()
        .filter(|item| item.file_name().to_string_lossy().ends_with(".oslo-new"))
        .count();
    assert_eq!(leftovers, 0, "the scratch file was renamed into place");
}

/// Off moves the file into `.off/` and on moves it back, contents untouched.
#[test]
fn off_and_on_move_the_file() {
    let own = testdir::own();
    place(own.dir.path(), "tool", "#!/bin/sh\necho tool\n");

    assert_eq!(set_active("tool", false), Some(false));
    assert!(!own.dir.path().join("tool").exists());
    assert!(own.dir.path().join(".off/tool").exists());
    assert!(!get("tool").expect("still a script").active);

    assert_eq!(set_active("tool", true), Some(true));
    assert!(own.dir.path().join("tool").exists());
    assert_eq!(get("tool").expect("back").body, "#!/bin/sh\necho tool\n");
    assert_eq!(set_active("missing", false), None);
}

/// Removing deletes the file, on or off.
#[test]
fn removing_deletes_the_file() {
    let own = testdir::own();
    place(own.dir.path(), "gone", "#!/bin/sh\n");
    place(&own.dir.path().join(".off"), "also", "#!/bin/sh\n");
    assert!(remove("gone"));
    assert!(remove("also"));
    assert!(!remove("gone"), "nothing left to remove");
    assert!(all().is_empty());
}

/// Writing an entry that is off puts it in `.off/` and takes away a live copy.
#[test]
fn writing_an_off_entry_lands_in_the_off_directory() {
    let own = testdir::own();
    place(own.dir.path(), "flip", "#!/bin/sh\n");
    let entry = Entry {
        active: false,
        ..Entry::new(Kind::Script, "flip", "#!/bin/sh\necho off\n")
    };
    write(&entry).expect("write");
    assert!(!own.dir.path().join("flip").exists());
    assert!(own.dir.path().join(".off/flip").exists());
}

/// A path outside the scripts directory is never one a session turned off.
#[test]
fn only_a_path_in_the_directory_can_be_off_for_a_session() {
    let _own = testdir::own();
    assert!(!is_session_off(Path::new("/usr/bin/true")));
}

/// The tag line is read from the comment block at the top, and nowhere else.
#[test]
fn tags_come_from_the_header() {
    let body = "#!/usr/bin/env bash\n# tags: desktop, net\n\nset -e\n";
    assert_eq!(tags_of(body), ["desktop", "net"]);
    assert_eq!(tags_of("#!/bin/sh\n#tags:a b,a\n"), ["a", "b"]);
    assert_eq!(
        tags_of("#!/bin/sh\n\n# about this\n# tags: late\n"),
        ["late"]
    );
    assert!(tags_of("#!/bin/sh\necho hi\n# tags: body\n").is_empty());
    assert!(tags_of("#!/bin/sh\n# tags:\n").is_empty());
    assert!(tags_of("#!/bin/sh\n# no tags here\n").is_empty());
}

/// Setting tags replaces the line, adds one after the shebang, or takes it away.
#[test]
fn tags_are_written_into_the_header() {
    let tags = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
    assert_eq!(
        with_tags("#!/bin/sh\necho hi\n", &tags(&["git"])),
        "#!/bin/sh\n# tags: git\necho hi\n"
    );
    assert_eq!(
        with_tags("#!/bin/sh\n# tags: old\necho hi\n", &tags(&["a", "b"])),
        "#!/bin/sh\n# tags: a, b\necho hi\n"
    );
    assert_eq!(
        with_tags("#!/bin/sh\n# tags: old\necho hi\n", &[]),
        "#!/bin/sh\necho hi\n"
    );
    let same = "#!/bin/sh\n# tags:   a,b\necho hi";
    assert_eq!(with_tags(same, &tags(&["a", "b"])), same, "untouched");
    assert_eq!(
        with_tags("#!/bin/sh", &tags(&["x"])),
        "#!/bin/sh\n# tags: x",
        "no newline is added where there was none"
    );
}
