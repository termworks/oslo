use super::*;

/// A store of its own, and a scripts directory of its own, for one test.
fn store() -> ((scripts::testdir::Own, tempfile::TempDir), Store) {
    let scripts = scripts::testdir::own();
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(&dir.path().join("macros.db")).expect("open");
    ((scripts, dir), store)
}

fn entry(kind: Kind, name: &str, body: &str) -> Entry {
    Entry::new(kind, name, body)
}

#[test]
fn what_goes_in_comes_back_out() {
    let (_dir, store) = store();
    put(&store, &entry(Kind::Alias, "gco", "git checkout")).expect("put");
    assert_eq!(
        get(&store, Kind::Alias, "gco").map(|e| e.body),
        Some("git checkout".to_string())
    );
    assert!(
        get(&store, Kind::Abbrev, "gco").is_none(),
        "a different kind"
    );
    assert!(remove(&store, Kind::Alias, "gco"));
    assert!(get(&store, Kind::Alias, "gco").is_none());
}

/// **The kinds share a store but not a namespace.** An alias and a script may both be called
/// `deploy`; that is a mistake worth being able to see rather than one to make impossible.
#[test]
fn one_name_can_be_more_than_one_kind() {
    let (_dir, store) = store();
    put(&store, &entry(Kind::Alias, "deploy", "echo one")).expect("put");
    put(
        &store,
        &entry(Kind::Script, "deploy", "#!/bin/sh\necho two\n"),
    )
    .expect("put");
    assert_eq!(kinds_of(&store, "deploy"), vec![Kind::Alias, Kind::Script]);
    assert_eq!(kinds_of(&store, "nothing"), vec![]);
}

#[test]
fn everything_comes_back_in_a_stable_order() {
    let (_dir, store) = store();
    put(&store, &entry(Kind::Script, "zz", "#!/bin/sh\n")).expect("put");
    put(&store, &entry(Kind::Alias, "b", "echo b")).expect("put");
    put(&store, &entry(Kind::Alias, "a", "echo a")).expect("put");
    put(&store, &entry(Kind::Abbrev, "m", "echo m")).expect("put");

    let names: Vec<String> = all(&store)
        .into_iter()
        .map(|e| format!("{}/{}", e.kind.word(), e.name))
        .collect();
    assert_eq!(names, ["alias/a", "alias/b", "abbrev/m", "script/zz"]);
}

/// A name becomes a command word, so anything that could be an operator is not a name.
#[test]
fn a_name_that_could_be_an_operator_is_refused() {
    for bad in ["", "a b", "a;b", "a|b", "-x", "a>b", "a(b", "a$b", "a\nb"] {
        assert!(!valid_name(bad), "{bad:?} should not be a name");
    }
    for good in [
        "gco",
        "g.co",
        "g-co",
        "g_co",
        "k8s",
        "c++",
        "user@host",
        "50%",
    ] {
        assert!(valid_name(good), "{good:?} should be a name");
    }
}

#[test]
fn a_bad_name_is_refused_rather_than_stored() {
    let (_dir, store) = store();
    assert!(put(&store, &entry(Kind::Alias, "two words", "x")).is_err());
    assert!(all(&store).is_empty());
}

// ---------------------------------------------------------------- created, tags, active

/// **A body is stored verbatim**, header or no header, because it is code and not a field.
#[test]
fn the_header_carries_the_fields_and_the_body_carries_itself() {
    let (_dir, store) = store();
    let awkward = "#!/bin/sh\n# a body with\ttabs, a comma, and\nseveral lines\n";
    let mine = Entry::new(Kind::Func, "deploy", awkward).tagged(&["work".into(), "ci".into()]);
    put(&store, &mine).expect("put");

    let back = get(&store, Kind::Func, "deploy").expect("stored");
    assert_eq!(back.body, awkward, "byte for byte");
    assert_eq!(back.tags, ["work", "ci"]);
    assert!(back.active);
    assert_eq!(back.created, mine.created);
}

/// A record from before there were fields is a bare body. Read as one rather than migrated.
#[test]
fn a_record_with_no_header_is_a_body_and_nothing_else() {
    let plain = decode(Kind::Alias, "gs", "git status --short");
    assert_eq!(plain.body, "git status --short");
    assert!(plain.active, "on, because off has to be said");
    assert_eq!(plain.created, 0);
    assert!(plain.tags.is_empty());
}

/// **Editing a macro does not make it new.** The manager's first column would otherwise say a
/// three-year-old alias was created the last time you fixed a typo in it.
#[test]
fn rewriting_one_keeps_the_day_it_was_made() {
    let (_dir, store) = store();
    let mut first = Entry::new(Kind::Alias, "gs", "git status");
    first.created = 1_000_000;
    put(&store, &first).expect("put");

    put(&store, &Entry::new(Kind::Alias, "gs", "git status --short")).expect("put again");
    let back = get(&store, Kind::Alias, "gs").expect("stored");
    assert_eq!(back.created, 1_000_000, "the day it was made, not today");
    assert_eq!(back.body, "git status --short");
}

/// Off is not gone: the body is still there, and the snapshot a shell reads is what changes.
#[test]
fn turning_one_off_keeps_it_and_takes_it_out_of_the_snapshot() {
    let (_dir, store) = store();
    put(&store, &entry(Kind::Alias, "gs", "git status")).expect("put");
    assert_eq!(set_active(&store, Kind::Alias, "gs", false), Some(false));

    let back = get(&store, Kind::Alias, "gs").expect("still stored");
    assert!(!back.active);
    assert_eq!(back.body, "git status", "off, not forgotten");
    assert_eq!(set_active(&store, Kind::Alias, "nothing", false), None);
}

#[test]
fn the_tags_in_use_are_listed_once_each_in_order() {
    let entries = [
        Entry::new(Kind::Alias, "a", "x").tagged(&["git".into(), "system".into()]),
        Entry::new(Kind::Alias, "b", "x").tagged(&["git".into()]),
        Entry::new(Kind::Alias, "c", "x"),
    ];
    assert_eq!(tags(&entries), ["git", "system"]);
}

// ---------------------------------------------------------------- what a shebang says

#[test]
fn a_shebang_names_the_language_not_the_finder() {
    assert_eq!(shebang_interpreter("#!/bin/sh\n").as_deref(), Some("sh"));
    assert_eq!(
        shebang_interpreter("#!/usr/bin/python3.13\n").as_deref(),
        Some("python3.13")
    );
    // `env` is *how* a shebang finds an interpreter, not what the interpreter is.
    assert_eq!(
        shebang_interpreter("#!/usr/bin/env python3\n").as_deref(),
        Some("python3")
    );
    assert_eq!(
        shebang_interpreter("#!/usr/bin/env -S FOO=1 node\n").as_deref(),
        Some("node")
    );
    assert_eq!(shebang_interpreter("echo no shebang\n"), None);
    assert_eq!(shebang_interpreter(""), None);
}

/// The extension is what makes an editor colour the buffer, which is most of why you wanted one.
#[test]
fn the_extension_follows_the_language() {
    assert_eq!(Kind::Alias.extension(""), "sh");
    assert_eq!(Kind::Script.extension("#!/usr/bin/env python3\n"), "py");
    assert_eq!(Kind::Script.extension("#!/bin/sh\n"), "sh");
    assert_eq!(Kind::Script.extension("no shebang"), "sh");
    assert_eq!(Kind::Func.extension("-- a lua comment\n"), "lua");
    assert_eq!(Kind::Func.extension("echo hello\n"), "sh");
}

// ---------------------------------------------------------------- the snapshot

/// The snapshot is what a starting shell reads, so it carries only what a starting shell needs.
#[test]
fn only_the_startup_kinds_reach_the_snapshot() {
    assert!(Kind::Alias.wanted_at_startup());
    assert!(Kind::Abbrev.wanted_at_startup());
    assert!(!Kind::Func.wanted_at_startup(), "found after $PATH fails");
    assert!(!Kind::Script.wanted_at_startup());
}

/// A newline in a body would be a record boundary to a line-oriented reader.
#[test]
fn a_body_with_newlines_and_tabs_survives_the_round_trip() {
    let awkward = "line one\nline\ttwo\\three\n";
    let there = snapshot::round_trip_for_test(awkward);
    assert_eq!(there, awkward);
}

/// **A space does not make a value a command.**
///
/// `is_a_value` treated any whitespace as "this must be run", so `EDITOR="code --wait"` was stored
/// as a recipe and *executed*. It stayed invisible while a stored variable deferred to one already
/// set — and became a live bug the moment stored variables started winning, because then every
/// inherited exported variable containing a space went through this: `SSH_CONNECTION` came back as
/// the output of trying to execute an IP address, `LS_COLORS` likewise.
#[test]
fn a_value_with_spaces_is_still_a_value() {
    assert!(super::is_a_value("code --wait"));
    assert!(super::is_a_value("1.2.3.4 22 5.6.7.8 22"));
    assert!(super::is_a_value("di=01;34:ln=01;36"));
}

/// Substitution is what marks a recipe, and substitution has a syntax.
#[test]
fn only_substitution_makes_a_recipe() {
    assert!(!super::is_a_value("$(oslo secret get gh-token)"));
    assert!(!super::is_a_value("`hostname`"));
    assert!(!super::is_a_value("  "));
    assert!(!super::is_a_value(""));
}
