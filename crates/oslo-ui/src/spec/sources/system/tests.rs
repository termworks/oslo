use super::*;

/// `name:x:id:…` is the shape both files share, and the comment lines are not entries.
#[test]
fn a_colon_file_gives_up_its_names_and_ids() {
    let text = "\
# a comment
root:x:0:0:root:/root:/bin/bash
bresilla:x:1000:1000::/home/bresilla:/usr/bin/oslo

nobody:x:65534:65534:
";
    let found = named_from(text);
    assert_eq!(
        found,
        [
            ("root".to_string(), "0".to_string()),
            ("bresilla".to_string(), "1000".to_string()),
            ("nobody".to_string(), "65534".to_string()),
        ]
    );
}

/// **This user must be in the list**, or `/etc/passwd` is being read wrongly.
#[test]
fn the_current_user_is_among_them() {
    let Ok(me) = std::env::var("USER") else {
        return; // no `$USER` in this environment; nothing to assert against
    };
    let all = users();
    if all.is_empty() {
        return; // no /etc/passwd, which a container may genuinely not have
    }
    assert!(
        all.iter().any(|one| one.value == me),
        "{me} is not among {} users",
        all.len()
    );
}

/// **Never cached**, or `export X=1; unset <Tab>` would not see `X`.
#[test]
fn a_variable_set_now_is_offered_now() {
    let name = format!("OSLO_SOURCE_PROBE_{}", std::process::id());
    assert!(!variables().iter().any(|one| one.value == name));
    // SAFETY: a name unique to this process, which no other test reads.
    unsafe { std::env::set_var(&name, "here") };
    let found = variables();
    let probe = found.iter().find(|one| one.value == name);
    assert!(probe.is_some(), "a variable set just now was not offered");
    assert_eq!(probe.map(|one| one.note.as_str()), Some("here"));
    unsafe { std::env::remove_var(&name) };
}

/// A menu row is not the place to print `$LS_COLORS`.
#[test]
fn a_long_value_is_cut_to_a_column() {
    let long = "x".repeat(200);
    let short = shorten(&long);
    assert!(short.chars().count() <= 41, "{}", short.chars().count());
    assert!(short.ends_with('…'));
    assert_eq!(shorten("short"), "short");
}

/// A unit is `name.kind`; a drop-in directory beside it is not a thing to start.
#[test]
fn only_real_units_are_offered() {
    assert!(is_a_unit("nginx.service"));
    assert!(is_a_unit("sshd.socket"));
    assert!(is_a_unit("backup.timer"));
    assert!(is_a_unit("multi-user.target"));
    assert!(!is_a_unit("nginx.service.d"));
    assert!(!is_a_unit("system-update"));
    assert!(!is_a_unit("README"));
}

/// A login shell is a path, because a path is what `chsh -s` takes.
#[test]
fn a_shell_is_offered_as_a_path() {
    for one in shells() {
        assert!(one.value.starts_with('/'), "{}", one.value);
    }
}

/// **`posix/` and `right/` are two more copies of the same tree.** Including them would treble the
/// list to say the same thing three ways, and `posix/Europe/Rome` is not a name anybody sets.
#[test]
fn a_timezone_is_a_region_and_a_city_and_nothing_doubled() {
    let found = timezones();
    if found.is_empty() {
        return; // No tzdata installed.
    }
    assert!(found.iter().any(|one| one.value.contains('/')));
    for one in &found {
        assert!(!one.value.starts_with("posix/"), "{}", one.value);
        assert!(!one.value.starts_with("right/"), "{}", one.value);
        // The loose files at the top of the tree are tables about zones, not zones.
        assert!(!one.value.ends_with(".tab"), "{}", one.value);
    }
}

/// A uid nobody has is an empty note, not a panic and not the wrong person.
#[test]
fn a_uid_finds_its_person_or_nobody() {
    assert!(user_named(u32::MAX).is_empty());
    assert_eq!(user_named(0), "root");
}
