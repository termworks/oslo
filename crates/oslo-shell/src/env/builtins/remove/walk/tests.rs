//! What the walk does that `remove_dir_all` did not.
//!
//! These go through [`super::remove_tree`] directly rather than through the builtin, so a case can
//! set up a tree the argument parser has no way to describe — an unreadable subdirectory, a
//! symlink pointing at a directory that must survive.

use super::{Walk, describe, remove_tree};
use std::fs;
use std::path::Path;

/// A walk that asks nothing and says nothing: the shape almost every case wants.
fn quiet(recursive: bool) -> Walk {
    Walk {
        origin: "oslo: ".to_string(),
        recursive,
        ..Walk::default()
    }
}

/// **A read-only directory is a permission failure, and the walk says so** — which is what the
/// prompt's `sudo` offer is made from.
#[test]
fn a_permission_failure_is_noticed() {
    use std::os::unix::fs::PermissionsExt;
    if nix::unistd::geteuid().is_root() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("tree");
    write(&root.join("locked/f"), "x");
    let locked = root.join("locked");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).expect("chmod");

    let walk = quiet(true);
    let outcome = remove_tree(&root, "tree", &walk);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).expect("chmod back");

    assert!(outcome.failed);
    assert!(walk.denied.get(), "the failure was permission");
    assert!(!quiet(true).denied.get(), "and a walk starts without one");
}

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, text).expect("write");
}

#[test]
fn a_plain_file_goes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("f");
    write(&file, "x");

    let outcome = remove_tree(&file, "f", &quiet(false));

    assert!(!outcome.failed);
    assert!(!file.exists());
}

#[test]
fn a_tree_goes_from_the_bottom_up() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("tree");
    write(&root.join("a/b/c/deep"), "x");
    write(&root.join("top"), "x");

    let outcome = remove_tree(&root, "tree", &quiet(true));

    assert!(!outcome.failed);
    assert!(!root.exists());
}

/// **The case this module was written for.** One unremovable file used to abort the whole walk and
/// blame the operand; now it names the file and everything else still goes.
#[test]
fn one_unremovable_file_does_not_stop_the_rest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("tree");
    write(&root.join("locked/f"), "x");
    write(&root.join("free/g"), "x");
    write(&root.join("loose"), "x");
    // No write permission on the directory means its entry cannot be unlinked.
    fs::set_permissions(root.join("locked"), perm(0o500)).expect("chmod locked");

    let outcome = remove_tree(&root, "tree", &quiet(true));

    assert!(outcome.failed, "the walk should report a failure");
    assert!(!root.join("free").exists(), "the readable subtree stayed");
    assert!(!root.join("loose").exists(), "the loose file stayed");
    assert!(root.join("locked/f").exists(), "the locked file vanished");

    fs::set_permissions(root.join("locked"), perm(0o700)).expect("restore");
}

/// A directory that could not be emptied is not reported *again* — the child already was.
#[test]
fn a_parent_of_a_failure_is_removed_when_the_failure_is_elsewhere() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("tree");
    write(&root.join("keep/f"), "x");
    write(&root.join("other/g"), "x");
    fs::set_permissions(root.join("keep"), perm(0o500)).expect("chmod");

    let outcome = remove_tree(&root, "tree", &quiet(true));

    assert!(outcome.failed);
    assert!(!root.join("other").exists(), "the sibling should be gone");
    assert!(root.exists(), "the root cannot go while a child remains");

    fs::set_permissions(root.join("keep"), perm(0o700)).expect("restore");
}

/// **The one that would delete a home directory.** `rm -r link` removes the link; whatever it
/// points at is untouched, because `symlink_metadata` says it is not a directory.
#[test]
fn a_symlink_to_a_directory_is_one_entry() {
    let dir = tempfile::tempdir().expect("tempdir");
    let precious = dir.path().join("precious");
    write(&precious.join("do-not-delete"), "x");
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&precious, &link).expect("symlink");

    let outcome = remove_tree(&link, "link", &quiet(true));

    assert!(!outcome.failed);
    assert!(!link.exists(), "the link should be gone");
    assert!(
        precious.join("do-not-delete").exists(),
        "the walk followed the symlink"
    );
}

/// A symlink *inside* a tree is removed as a link too, and its target survives the walk.
#[test]
fn a_symlink_inside_a_tree_is_not_followed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let precious = dir.path().join("precious");
    write(&precious.join("keep"), "x");
    let root = dir.path().join("tree");
    fs::create_dir_all(&root).expect("mkdir");
    std::os::unix::fs::symlink(&precious, root.join("link")).expect("symlink");

    let outcome = remove_tree(&root, "tree", &quiet(true));

    assert!(!outcome.failed);
    assert!(!root.exists());
    assert!(precious.join("keep").exists(), "the target was walked");
}

/// Without `-r`, a directory is removed only when it is empty — the `-d` behaviour.
#[test]
fn without_recursion_only_an_empty_directory_goes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let empty = dir.path().join("empty");
    fs::create_dir(&empty).expect("mkdir");
    let full = dir.path().join("full");
    write(&full.join("f"), "x");

    assert!(!remove_tree(&empty, "empty", &quiet(false)).failed);
    assert!(!empty.exists());

    assert!(remove_tree(&full, "full", &quiet(false)).failed);
    assert!(full.join("f").exists(), "a non-empty directory was emptied");
}

/// A missing entry is a failure, unless `-f` said otherwise.
#[test]
fn force_forgives_a_missing_entry() {
    let dir = tempfile::tempdir().expect("tempdir");
    let gone = dir.path().join("gone");

    assert!(remove_tree(&gone, "gone", &quiet(false)).failed);

    let mut forcing = quiet(false);
    forcing.force = true;
    assert!(!remove_tree(&gone, "gone", &forcing).failed);
}

/// A tree deeper than a recursive implementation would survive.
#[test]
fn a_very_deep_tree_does_not_overflow_the_stack() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("deep");
    let mut path = root.clone();
    for _ in 0..2000 {
        path = path.join("d");
    }
    fs::create_dir_all(&path).expect("create the deep tree");

    let outcome = remove_tree(&root, "deep", &quiet(true));

    assert!(!outcome.failed);
    assert!(!root.exists());
}

/// **A Ctrl-C stops the walk between entries.** A builtin runs in the shell process, so before
/// this the keystroke set a flag that nothing looked at until the whole `rm` had already finished
/// — `rm -rf` over a large tree could not be called off once it had started.
///
/// Driven through `note_interrupt`, which is the thread-local half of the same flag the signal
/// handler sets, so the case does not have to signal a multi-threaded test binary.
#[test]
fn an_interrupt_stops_the_walk_and_leaves_the_rest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("tree");
    for n in 0..200 {
        write(&root.join(format!("f{n}")), "x");
    }

    crate::exec::job::note_interrupt();
    let outcome = remove_tree(&root, "tree", &quiet(true));
    // Put the flag back where the test found it, so the next case on this thread is not stopped.
    let _ = crate::exec::job::interrupt_pending();

    assert!(outcome.interrupted, "the walk ran to the end anyway");
    assert!(root.exists(), "the tree was removed despite the interrupt");
    assert_eq!(
        std::fs::read_dir(&root).expect("read tree").count(),
        200,
        "the walk removed entries after the interrupt"
    );
}

/// And the interrupt is left for the evaluator: a builtin that cleared it would stop the `rm` and
/// let the `&&` after it run as though nothing had happened.
#[test]
fn the_interrupt_is_peeked_rather_than_taken() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("tree");
    write(&root.join("f"), "x");

    crate::exec::job::note_interrupt();
    let _ = remove_tree(&root, "tree", &quiet(true));
    let still_pending = crate::exec::job::interrupt_pending();

    assert!(
        still_pending,
        "the walk swallowed the interrupt the command boundary needed"
    );
}

#[test]
fn the_kinds_are_named_the_way_rm_names_them() {
    let dir = tempfile::tempdir().expect("tempdir");
    let empty = dir.path().join("empty");
    write(&empty, "");
    let full = dir.path().join("full");
    write(&full, "x");
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&full, &link).expect("symlink");

    let of = |p: &Path| describe(&fs::symlink_metadata(p).expect("stat"));
    assert_eq!(of(&empty), "regular empty file");
    assert_eq!(of(&full), "regular file");
    assert_eq!(of(&link), "symbolic link");
    assert_eq!(of(dir.path()), "directory");
}

fn perm(mode: u32) -> fs::Permissions {
    use std::os::unix::fs::PermissionsExt;
    fs::Permissions::from_mode(mode)
}
