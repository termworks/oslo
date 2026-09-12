use super::*;

/// A repository laid out the way git lays one out, without running git.
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let git = dir.path().join(".git");
    std::fs::create_dir_all(git.join("refs/heads/feat")).unwrap();
    std::fs::create_dir_all(git.join("refs/tags")).unwrap();
    std::fs::create_dir_all(git.join("refs/remotes/origin")).unwrap();
    std::fs::write(git.join("HEAD"), "ref: refs/heads/develop\n").unwrap();
    std::fs::write(git.join("refs/heads/develop"), "a\n").unwrap();
    std::fs::write(git.join("refs/heads/feat/after-fish"), "b\n").unwrap();
    std::fs::write(git.join("refs/remotes/origin/main"), "c\n").unwrap();
    std::fs::write(git.join("refs/remotes/origin/HEAD"), "ref: x\n").unwrap();
    std::fs::write(
        git.join("packed-refs"),
        "# pack-refs with: peeled fully-peeled sorted\n\
         aaaa refs/heads/main\n\
         bbbb refs/tags/v0.1.0\n\
         ^cccc\n",
    )
    .unwrap();
    std::fs::write(
        git.join("config"),
        "[core]\n\tbare = false\n[remote \"origin\"]\n\turl = git@example.com:x.git\n[remote \"upstream\"]\n\turl = x\n",
    )
    .unwrap();
    dir
}

fn values(found: Vec<Suggestion>) -> Vec<String> {
    found.into_iter().map(|one| one.value).collect()
}

/// **Loose and packed refs are one list.** `git gc` folds branches into `packed-refs` at any time,
/// so a source reading only `refs/heads/` would silently lose branches as a repository ages.
#[test]
fn a_branch_is_found_loose_or_packed() {
    let dir = repo();
    let found = values(branches(&dir.path().to_string_lossy()));
    assert!(found.contains(&"develop".to_string()), "{found:?}");
    assert!(found.contains(&"main".to_string()), "packed: {found:?}");
    // A branch with a slash is a directory and a file, and its name is both halves.
    assert!(found.contains(&"feat/after-fish".to_string()), "{found:?}");
}

/// The checked-out branch says so, which is the one thing the name does not tell you.
#[test]
fn the_current_branch_is_marked() {
    let dir = repo();
    let found = branches(&dir.path().to_string_lossy());
    let current = found.iter().find(|one| one.value == "develop").unwrap();
    assert_eq!(current.note, "current");
    let other = found.iter().find(|one| one.value == "main").unwrap();
    assert_eq!(other.note, "");
}

/// A completion typed three directories down is still inside the repository.
#[test]
fn the_repository_is_found_from_below() {
    let dir = repo();
    let deep = dir.path().join("a/b/c");
    std::fs::create_dir_all(&deep).unwrap();
    let found = values(branches(&deep.to_string_lossy()));
    assert!(found.contains(&"develop".to_string()), "{found:?}");
}

/// Somewhere that is not a repository is an empty answer, not a panic and not the refs of a
/// repository further up than the filesystem root.
#[test]
fn somewhere_else_offers_nothing() {
    assert!(branches("/proc").is_empty());
    assert!(tags("/proc").is_empty());
    assert!(remotes("/proc").is_empty());
}

/// A remote that has never been fetched has a config stanza and no refs, and is still a remote.
#[test]
fn a_remote_comes_from_the_config() {
    let dir = repo();
    let found = values(remotes(&dir.path().to_string_lossy()));
    assert_eq!(found, vec!["origin".to_string(), "upstream".to_string()]);
}

/// Tags come from `packed-refs` on any repository with history behind it.
#[test]
fn a_packed_tag_is_a_tag() {
    let dir = repo();
    assert_eq!(
        values(tags(&dir.path().to_string_lossy())),
        vec!["v0.1.0".to_string()]
    );
}

/// **`origin/HEAD` is a symbolic ref, not a branch.** Checking it out detaches the head, which is
/// never what somebody picking from a menu meant.
#[test]
fn revisions_are_branches_then_remotes_then_tags_without_head() {
    let dir = repo();
    let found = values(revisions(&dir.path().to_string_lossy(), "v"));
    assert!(found.contains(&"origin/main".to_string()), "{found:?}");
    assert!(!found.contains(&"origin/HEAD".to_string()), "{found:?}");
    let branch = found.iter().position(|one| one == "develop").unwrap();
    let remote = found.iter().position(|one| one == "origin/main").unwrap();
    let tag = found.iter().position(|one| one == "v0.1.0").unwrap();
    assert!(branch < remote && remote < tag, "{found:?}");
}

/// A worktree's `.git` is a file, and its refs belong to the repository it points at.
#[test]
fn a_worktree_reaches_the_shared_refs() {
    let main = repo();
    let git = main.path().join(".git");
    let inner = git.join("worktrees/wt");
    std::fs::create_dir_all(&inner).unwrap();
    std::fs::write(inner.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::write(inner.join("commondir"), "../..\n").unwrap();

    let tree = tempfile::tempdir().unwrap();
    std::fs::write(
        tree.path().join(".git"),
        format!("gitdir: {}\n", inner.display()),
    )
    .unwrap();

    let found = values(branches(&tree.path().to_string_lossy()));
    assert!(found.contains(&"develop".to_string()), "{found:?}");
    assert!(found.contains(&"feat/after-fish".to_string()), "{found:?}");
}

/// **A bare Tab is a menu; a typed prefix is a search.** With 68 tags and three branches, putting
/// tags in the empty menu buries every branch — the menu breaks ties alphabetically, so `0.1.1`
/// wins. Nothing is lost, because a prefix brings them straight back.
#[test]
fn an_empty_tab_offers_branches_and_a_prefix_finds_tags() {
    let dir = repo();
    let at = dir.path().to_string_lossy().to_string();

    let bare = values(revisions(&at, ""));
    assert!(bare.contains(&"develop".to_string()), "{bare:?}");
    assert!(bare.contains(&"origin/main".to_string()), "{bare:?}");
    assert!(
        !bare.contains(&"v0.1.0".to_string()),
        "a tag in the bare menu"
    );

    let typed = values(revisions(&at, "v"));
    assert!(typed.contains(&"v0.1.0".to_string()), "{typed:?}");
    assert!(typed.contains(&"develop".to_string()), "{typed:?}");
}
