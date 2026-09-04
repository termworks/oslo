//! Block devices, filesystem types, and where things are or could be mounted.
//!
//! ```text
//!   mkfs.ext4 /dev/sd⇥    /dev/sda1   932G          device
//!   mount -t ⇥            ext4                      filesystem
//!   mount /m⇥             /mnt/backup   ext4        fstab
//!   umount /m⇥            /mnt/backup   ext4        mount
//!   swapoff ⇥             /swapfile   file          swap
//! ```
//!
//! # `$fstab` and `$mounts` are opposites, and both are needed
//!
//! `umount` wants what **is** mounted; `mount` wants what **could be**. They come from different
//! files — `/proc/mounts` and `/etc/fstab` — and a spec that used one for the other would offer
//! exactly the wrong half of the answer every time.

use super::{Suggestion, read};

/// Every block device, as `/dev/…` — the name the commands take.
///
/// `/sys/class/block` is a directory of them, which is why this needs neither `lsblk` nor a
/// `/dev` walk that would also turn up every character device on the machine.
///
/// **Zero-length devices are left out.** A machine has eight unused `loop` devices on it and none
/// of them is a thing to `mkfs`; they are noise in a menu that is otherwise disks.
pub fn devices() -> Vec<Suggestion> {
    let Ok(entries) = std::fs::read_dir("/sys/class/block") else {
        return Vec::new();
    };
    let mut found: Vec<Suggestion> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            let sectors: u64 = read(&format!("/sys/class/block/{name}/size"))
                .trim()
                .parse()
                .ok()?;
            (sectors > 0)
                .then(|| Suggestion::new(format!("/dev/{name}"), size(sectors * 512), "device"))
        })
        .collect();
    found.sort_unstable_by(|a, b| a.value.cmp(&b.value));
    found
}

/// A byte count a person can read at a glance.
fn size(bytes: u64) -> String {
    const UNITS: &[(u64, &str)] = &[
        (1 << 40, "T"),
        (1 << 30, "G"),
        (1 << 20, "M"),
        (1 << 10, "K"),
    ];
    for (scale, unit) in UNITS {
        if bytes >= *scale {
            return format!("{}{unit}", bytes / scale);
        }
    }
    format!("{bytes}B")
}

/// The filesystem types this kernel can mount.
///
/// `/proc/filesystems` marks the ones needing no device — `tmpfs`, `proc`, `overlay` — with a
/// leading `nodev`, which is worth keeping as the note: it is the difference between a type you can
/// hand a disk and one you cannot.
pub fn filesystems() -> Vec<Suggestion> {
    read("/proc/filesystems")
        .lines()
        .filter_map(|line| {
            let virtual_only = line.starts_with("nodev");
            let name = line.split_whitespace().last()?;
            let note = match virtual_only {
                true => "no device",
                false => "",
            };
            Some(Suggestion::new(name, note, "filesystem"))
        })
        .collect()
}

/// Where things are mounted, with the filesystem beside each.
///
/// **The mount point, not the device**, because that is what `umount`, `df` and `findmnt` take —
/// and the device is the field a person is least likely to be able to type from memory.
///
/// Not cached: a `mount` you just ran has to appear in the next `umount <Tab>`.
pub fn mounts() -> Vec<Suggestion> {
    read("/proc/mounts")
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let _device = fields.next()?;
            let point = fields.next()?;
            let kind = fields.next().unwrap_or_default();
            Some(Suggestion::new(unescape(point), kind, "mount"))
        })
        .collect()
}

/// Where things *could* be mounted: the targets `/etc/fstab` declares.
///
/// This is what `mount /m<Tab>` wants, and it is the complement of [`mounts`] rather than a
/// duplicate of it — the interesting entries are precisely the ones not mounted yet.
///
/// Swap lines are left out: their target field is `none` or `swap`, which is not a place.
pub fn fstab() -> Vec<Suggestion> {
    read("/etc/fstab")
        .lines()
        .map(|line| line.split('#').next().unwrap_or_default())
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let _device = fields.next()?;
            let point = fields.next()?;
            let kind = fields.next().unwrap_or_default();
            let is_a_place = point.starts_with('/');
            is_a_place.then(|| Suggestion::new(unescape(point), kind, "fstab"))
        })
        .collect()
}

/// The swap files and partitions in use, for `swapoff`.
pub fn swaps() -> Vec<Suggestion> {
    read("/proc/swaps")
        .lines()
        .skip(1) // A header row: `Filename Type Size Used Priority`.
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let file = fields.next()?;
            let kind = fields.next().unwrap_or_default();
            Some(Suggestion::new(unescape(file), kind, "swap"))
        })
        .collect()
}

/// `/proc/mounts` and `/etc/fstab` both escape a space in a path as `\040`, and a row showing the
/// escape would insert a path that does not exist.
fn unescape(path: &str) -> String {
    path.replace("\\040", " ")
}

#[cfg(test)]
#[path = "disk/tests.rs"]
mod tests;
