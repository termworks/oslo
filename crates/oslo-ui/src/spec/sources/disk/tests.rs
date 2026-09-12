use super::*;

/// A size column is only useful if it is short, and only correct if the boundaries are.
#[test]
fn a_size_is_readable_at_a_glance() {
    assert_eq!(size(0), "0B");
    assert_eq!(size(999), "999B");
    assert_eq!(size(1024), "1K");
    assert_eq!(size(1 << 20), "1M");
    assert_eq!(size(1 << 30), "1G");
    assert_eq!(size(1 << 40), "1T");
    // A disk, rather than a boundary.
    assert_eq!(size(500_107_862_016), "465G");
}

/// **A path with a space in it comes back with the space.** `/proc/mounts` writes it `\040`, and a
/// row offering the escape would insert a path that does not exist.
#[test]
fn an_escaped_space_is_put_back() {
    assert_eq!(unescape("/mnt/my\\040disk"), "/mnt/my disk");
    assert_eq!(unescape("/mnt/plain"), "/mnt/plain");
}

/// Every mount point this machine has is a place, and `/` is always one of them.
#[test]
fn the_root_filesystem_is_mounted() {
    let found = mounts();
    assert!(
        found.iter().any(|one| one.value == "/"),
        "no root mount among {} rows",
        found.len()
    );
    assert!(found.iter().all(|one| one.value.starts_with('/')));
}

/// `/proc/filesystems` has a `nodev` column, and the note has to tell the two apart — it is the
/// difference between a type you can hand a disk and one you cannot.
#[test]
fn a_virtual_filesystem_says_it_needs_no_device() {
    let found = filesystems();
    let proc = found.iter().find(|one| one.value == "proc");
    assert_eq!(proc.map(|one| one.note.as_str()), Some("no device"));
    assert!(found.iter().all(|one| !one.value.is_empty()));
}

/// Every device offered is one a command could be handed.
#[test]
fn a_device_is_named_the_way_commands_take_it() {
    for one in devices() {
        assert!(one.value.starts_with("/dev/"), "{}", one.value);
        assert!(!one.note.is_empty(), "{} has no size", one.value);
    }
}

/// `fstab` offers places, not the `none` a swap line puts in the target field.
#[test]
fn fstab_offers_only_places() {
    for one in fstab() {
        assert!(one.value.starts_with('/'), "{}", one.value);
    }
}

/// The header row of `/proc/swaps` is a header, not a swap file.
#[test]
fn the_swaps_header_is_not_offered() {
    for one in swaps() {
        assert_ne!(one.value, "Filename");
    }
}
