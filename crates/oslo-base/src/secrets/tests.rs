//! What can be tested without a store of its own. Keeping and reading back go through the real
//! binary in `tests/secrets_tests.rs`: they answer from `$XDG_DATA_HOME`, and a test that pointed
//! the *process* at a temporary one would be changing the environment under every other test's
//! thread — the hazard `track::universal` documents.

use super::*;

/// **A name is a filename**, and one that reaches out of the directory is refused rather than
/// quietly rewritten: a secret stored somewhere other than where it was asked for is worse than an
/// error, because nothing looks wrong until the day it is read by whoever owns that other place.
#[test]
fn a_name_cannot_reach_out_of_the_store() {
    for bad in ["..", "../elsewhere", "a/b", "", ".hidden", "with\0nul"] {
        assert!(path(bad).is_err(), "{bad:?} was accepted");
    }
}

/// An ordinary name lands in the store, under its own name, with the format's extension.
#[test]
fn a_name_is_a_file_in_the_store() {
    let Ok(path) = path("deploy-token") else {
        return; // No $HOME in this environment; nothing to assert about where it would go.
    };
    assert!(path.ends_with("deploy-token.sealed"), "{}", path.display());
    assert!(path.to_string_lossy().contains("oslo/secrets"));
}

/// **A directory called `.git` is not a repository**, and the difference is the whole value of the
/// warning: the machine this was written on has an empty `~/.git` that `git` itself does not
/// recognise, and the first version of the check told its owner their key was about to be
/// committed to it.
#[test]
fn an_empty_dot_git_is_not_a_repository() {
    let dir = tempfile::tempdir().expect("tempdir");
    let empty = dir.path().join("looks-like-one");
    std::fs::create_dir_all(&empty).expect("an empty .git");
    assert!(!super::place::is_repository(&empty));

    // A real one has `HEAD` in it…
    std::fs::write(empty.join("HEAD"), "ref: refs/heads/main\n").expect("write");
    assert!(super::place::is_repository(&empty));

    // …and a worktree or submodule has a *file* saying where its directory is.
    let pointer = dir.path().join("worktree-dot-git");
    std::fs::write(&pointer, "gitdir: /elsewhere\n").expect("write");
    assert!(super::place::is_repository(&pointer));
}

/// **A malformed line is an error, not a skipped line.** The failure the loudness prevents is a
/// store quietly encrypting to one fewer recipient than its owner believes.
///
/// It parses `key` lines, which a build without `crypt` refuses.
#[test]
#[cfg(feature = "crypt")]
fn a_conf_says_what_it_says_and_nothing_it_does_not() {
    let parsed = conf::parse(
        "# a comment\n\
         default work\n\
         \n\
         [user]\n\
         key file /k/one\n\
         \n\
         [work]\n\
         directory /w\n\
         key command pass show oslo/id\n",
    )
    .expect("it parses");

    assert_eq!(parsed.default.as_deref(), Some("work"));
    assert_eq!(parsed.sections.len(), 2);
    let work = parsed.section("work").expect("the work section");
    assert_eq!(work.directory.as_deref(), Some(std::path::Path::new("/w")));
    assert_eq!(
        work.keys,
        [KeySource::Command(vec![
            "pass".to_string(),
            "show".to_string(),
            "oslo/id".to_string()
        ])]
    );

    for wrong in [
        "recipient age1abc\n",        // before any section
        "[work]\nkey wallet /w\n",    // not a kind of key
        "[work]\nrecipients age1a\n", // not a word this file knows
        "[work]\nkey file\n",         // nothing after it
        "[..]\nkey file /k\n",        // not a store name
        "[work]\ndefault other\n",    // `default` is not a store's business
    ] {
        assert!(conf::parse(wrong).is_err(), "{wrong:?} was accepted");
    }
}

/// Editing splices lines, so what a person wrote around the change survives being edited by a
/// command — the reason this is not simply re-rendered from the parsed model.
#[test]
#[cfg(feature = "crypt")]
fn editing_keeps_the_comments_and_the_order() {
    let mut parsed = conf::parse("# mine\n\n[user]\nkey file /k/one\n").expect("it parses");
    parsed.add("user", "key file /k/two");
    parsed.add("work", "directory /w");
    parsed.set_default("work");

    assert_eq!(
        parsed.text(),
        "default work\n# mine\n\n[user]\nkey file /k/one\nkey file /k/two\n\n[work]\ndirectory /w\n"
    );

    assert_eq!(parsed.remove("user", |line| line == "key file /k/one"), 1);
    assert_eq!(parsed.remove("user", |line| line == "key file /k/one"), 0);
    assert!(parsed.text().contains("key file /k/two"));
    assert!(parsed.text().contains("# mine"));
}

/// A store name is a directory name and a section header, so it may not be either out of place.
#[test]
fn a_store_name_cannot_reach_out_of_its_directory() {
    for bad in ["", "..", "../elsewhere", "a/b", ".hidden", "a..b", "a b"] {
        assert!(conf::valid_store_name(bad).is_err(), "{bad:?} was accepted");
    }
    for good in ["user", "work", "plugin.notes", "a-b_c.2"] {
        assert!(conf::valid_store_name(good).is_ok(), "{good:?} was refused");
    }
}
