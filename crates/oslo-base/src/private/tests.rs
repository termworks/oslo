use super::*;

fn mode_of(path: &Path) -> u32 {
    std::fs::symlink_metadata(path)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o777
}

fn leftovers(directory: &Path) -> Vec<String> {
    std::fs::read_dir(directory)
        .expect("read_dir")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".new"))
        .collect()
}

#[test]
fn a_replaced_file_is_private_and_leaves_no_scratch() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("kept");
    replace(&path, b"one").expect("first");
    replace(&path, b"two").expect("second");
    assert_eq!(std::fs::read(&path).expect("read"), b"two");
    assert_eq!(mode_of(&path), 0o600);
    assert!(leftovers(dir.path()).is_empty());
}

/// The audit's h9: a link planted where a write lands must not carry the write elsewhere.
#[test]
fn a_symlink_target_stays_untouched() {
    let dir = tempfile::tempdir().expect("dir");
    let target = dir.path().join("authorized_keys");
    std::fs::write(&target, b"ssh-ed25519 AAAA mine\n").expect("write");
    let path = dir.path().join("deploy.sealed");
    std::os::unix::fs::symlink(&target, &path).expect("symlink");
    std::os::unix::fs::symlink(&target, dir.path().join("deploy.new")).expect("symlink");

    replace(&path, b"sealed bytes").expect("replace");
    assert_eq!(
        std::fs::read(&target).expect("read"),
        b"ssh-ed25519 AAAA mine\n"
    );
    assert!(
        !std::fs::symlink_metadata(&path)
            .expect("lstat")
            .is_symlink()
    );
    assert_eq!(std::fs::read(&path).expect("read"), b"sealed bytes");
}

#[test]
fn create_never_replaces() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("key");
    assert!(create(&path, b"first").expect("create"));
    assert!(!create(&path, b"second").expect("create again"));
    assert_eq!(std::fs::read(&path).expect("read"), b"first");
    assert_eq!(mode_of(&path), 0o600);

    // A dangling link is something there, and is not followed to make a file elsewhere.
    let dangling = dir.path().join("dangling");
    let elsewhere = dir.path().join("elsewhere");
    std::os::unix::fs::symlink(&elsewhere, &dangling).expect("symlink");
    assert!(!create(&dangling, b"x").expect("create"));
    assert!(!elsewhere.exists());
    assert!(leftovers(dir.path()).is_empty());
}

#[test]
fn a_read_takes_only_a_regular_file() {
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("file");
    std::fs::write(&file, b"bytes").expect("write");
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&file, &link).expect("symlink");

    assert_eq!(
        read(&file, 64, Links::Refuse).expect("read"),
        Some(b"bytes".to_vec())
    );
    assert_eq!(
        read(&dir.path().join("absent"), 64, Links::Refuse),
        Ok(None)
    );
    let refused = read(&link, 64, Links::Refuse).expect_err("a link was followed");
    assert!(refused.contains("symbolic link"), "{refused}");
    assert_eq!(
        read(&link, 64, Links::Follow).expect("follow"),
        Some(b"bytes".to_vec())
    );

    let too_big = read(&file, 4, Links::Refuse).expect_err("over the cap");
    assert!(too_big.contains("larger than"), "{too_big}");
    assert!(read(dir.path(), 64, Links::Follow).is_err());
    assert!(read(Path::new("/dev/zero"), 64, Links::Follow).is_err());
}

/// A FIFO with no writer would block an ordinary open for ever.
#[test]
fn a_fifo_is_refused_rather_than_waited_on() {
    let dir = tempfile::tempdir().expect("dir");
    let fifo = dir.path().join("ff.sealed");
    nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::S_IRWXU).expect("mkfifo");
    let refused = read(&fifo, 64, Links::Follow).expect_err("a FIFO was read");
    assert!(refused.contains("FIFO"), "{refused}");
}

#[test]
fn a_directory_is_made_private_and_an_owned_one_tightened() {
    let dir = tempfile::tempdir().expect("dir");
    let deep = dir.path().join("a/b");
    directory(&deep).expect("make");
    assert_eq!(mode_of(&deep), 0o700);
    assert_eq!(mode_of(&dir.path().join("a")), 0o700);

    let open = dir.path().join("open");
    std::fs::create_dir(&open).expect("mkdir");
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    directory(&open).expect("exists");
    assert_eq!(
        mode_of(&open),
        0o755,
        "a directory nobody said oslo owns was changed"
    );
    assert!(loose(&open));
    own_directory(&open).expect("own");
    assert_eq!(mode_of(&open), 0o700);
    assert!(!loose(&open));
}

#[test]
fn a_lock_excludes_another_holder() {
    let dir = tempfile::tempdir().expect("dir");
    let held = lock(dir.path(), false).expect("lock");
    let other = File::open(dir.path().join(".lock")).expect("open");
    assert!(other.try_lock().is_err(), "two holders at once");
    drop(held);
    assert!(other.try_lock().is_ok(), "the lock outlived its holder");
    assert_eq!(mode_of(&dir.path().join(".lock")), 0o600);
}
