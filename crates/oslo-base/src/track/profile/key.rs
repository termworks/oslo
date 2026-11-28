//! The key that says two machines mean the same profile.
//!
//! # What it is for
//!
//! A profile is a history, and two shells on two machines only have *the same* history if somebody
//! said so. The name is not enough: `default` on your laptop and `default` on a box you have an
//! account on are different histories that happen to share a word, and syncing them because the
//! words match would merge a stranger's commands into yours.
//!
//! So a profile carries a key, and `oslo profile sync` refuses unless both ends hold the same one.
//! Carrying it to the second machine — `oslo profile export` there, `import` here — is the step
//! that says *these two are one profile*, and it is deliberately a thing a person does once.
//!
//! # What it is not
//!
//! **Not the security of the sync.** That is ssh's job, and ssh is already doing it: the transport
//! is authenticated and encrypted before any of this is consulted. What the key answers is
//! *identity* — which history is this — and the only thing that crosses the wire is a fingerprint,
//! which is a hash and gives nothing away.
//!
//! # Where it lives
//!
//! `$XDG_STATE_HOME/oslo/profiles/<name>.key`, mode `0600` — **not** beside the store under
//! `$XDG_DATA_HOME`. The store is the thing that gets synced, backed up and copied between
//! machines; a key inside it would travel with every copy, which is exactly what a key that means
//! "this machine is authorised" must not do.
//!
//! `oslo` and `profiles/` are `0700`, and every write goes through [`crate::private`]: fsynced,
//! renamed or linked into place, never half there after a crash.

use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

/// A profile key held in memory, wiped when it is dropped.
pub type ProfileKey = Zeroizing<[u8; KEY]>;

/// What a profile key file holds, so one is recognisable on sight.
const PREFIX: &str = "OSLO-PROFILE-1:";

/// A key is 32 bytes, like every other key here.
const KEY: usize = 32;

/// How many hex characters of the hash a fingerprint shows.
///
/// Sixteen — 64 bits. Enough that two profiles never collide by accident, short enough to read out
/// over a phone and compare by eye, which is the thing it is for.
const FINGERPRINT: usize = 16;

/// Where `name`'s key is.
pub fn path(name: &str) -> Option<PathBuf> {
    if !super::valid(name) {
        return None;
    }
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
        })?;
    Some(base.join("oslo/profiles").join(format!("{name}.key")))
}

/// The key `name` holds, or `None` when it has none yet.
///
/// Only a regular file is read, at most [`crate::private::SMALL`] bytes; a link to one is followed.
pub fn read(name: &str) -> Result<Option<ProfileKey>, String> {
    use crate::private::{Links, SMALL};
    let path = located(name)?;
    let Some(bytes) = crate::private::read(&path, SMALL, Links::Follow)?.map(Zeroizing::new) else {
        return Ok(None);
    };
    std::str::from_utf8(&bytes)
        .map_err(|_| "not text".to_string())
        .and_then(decode)
        .map(Some)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Make one for `name`, refusing to replace one that is already there.
///
/// **Refusing rather than overwriting**, because the key is what ties this machine to the others: a
/// second `init` that quietly replaced it would leave a profile that syncs with nothing and says
/// nothing about why. The refusal is the filesystem's — the key is linked into place without
/// replacing — so two processes making the first key at once cannot both win.
pub fn generate(name: &str) -> Result<ProfileKey, String> {
    let path = located(name)?;
    let key = fresh()?;
    prepare(&path)?;
    match crate::private::create(&path, encode(&key).as_bytes())? {
        true => Ok(key),
        false => Err(format!(
            "{name}: already has a key at {} — `oslo profile export` copies it to another machine",
            path.display()
        )),
    }
}

/// The key `name` holds, made now if it has none: whichever process links one into place first,
/// every other one reads.
pub fn obtain(name: &str) -> Result<ProfileKey, String> {
    if let Some(key) = read(name)? {
        return Ok(key);
    }
    let path = located(name)?;
    let key = fresh()?;
    prepare(&path)?;
    match crate::private::create(&path, encode(&key).as_bytes())? {
        true => Ok(key),
        false => {
            read(name)?.ok_or_else(|| format!("{}: is there and holds no key", path.display()))
        }
    }
}

/// Write `key` as `name`'s, whatever was there before.
///
/// This is `import`'s half: replacing is the whole point when the key is arriving from the machine
/// that already has the profile.
pub fn install(name: &str, key: &[u8; KEY]) -> Result<PathBuf, String> {
    let path = located(name)?;
    prepare(&path)?;
    crate::private::replace(&path, encode(key).as_bytes())?;
    Ok(path)
}

fn located(name: &str) -> Result<PathBuf, String> {
    path(name).ok_or_else(|| format!("{name:?} is not a usable profile name"))
}

fn fresh() -> Result<ProfileKey, String> {
    let mut key = Zeroizing::new([0u8; KEY]);
    getrandom::fill(&mut key[..]).map_err(|e| format!("no randomness from the system: {e}"))?;
    Ok(key)
}

/// `$XDG_STATE_HOME/oslo` and its `profiles/`, made or tightened to `0700`: both are oslo's.
fn prepare(path: &Path) -> Result<(), String> {
    let profiles = path
        .parent()
        .ok_or_else(|| format!("{}: has no directory to be in", path.display()))?;
    if let Some(oslo) = profiles.parent() {
        crate::private::own_directory(oslo)?;
    }
    crate::private::own_directory(profiles)
}

/// What two machines compare to decide they mean the same profile.
///
/// A hash, so it can be printed, logged and sent over a wire that the key itself must never cross.
pub fn fingerprint(key: &[u8; KEY]) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(b"oslo profile key v1");
    digest.update(key);
    digest
        .finalize()
        .iter()
        .take(FINGERPRINT / 2)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A key, as it is written in a file or handed to another machine.
///
/// **Hex, not base64.** `base64` is a dependency of the secrets feature and this has to work in a
/// build that has no secrets at all — and sixty-four hex characters is a thing somebody can read
/// down a phone line, which is exactly how a profile key gets to the second machine.
pub fn encode(key: &[u8; KEY]) -> Zeroizing<String> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut text = Zeroizing::new(String::with_capacity(PREFIX.len() + KEY * 2 + 1));
    text.push_str(PREFIX);
    for byte in key {
        text.push(HEX[(byte >> 4) as usize] as char);
        text.push(HEX[(byte & 0xf) as usize] as char);
    }
    text.push('\n');
    text
}

/// The first key in what a file or a person gave us.
pub fn decode(text: &str) -> Result<ProfileKey, String> {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .ok_or("no profile key in it")?;
    let body = line
        .strip_prefix(PREFIX)
        .ok_or_else(|| format!("a profile key begins with {PREFIX}, and this does not"))?
        .trim();
    if body.len() != KEY * 2 {
        return Err(format!(
            "a profile key is {} characters, and this is {}",
            KEY * 2,
            body.len()
        ));
    }
    let mut key = Zeroizing::new([0u8; KEY]);
    for (at, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&body[at * 2..at * 2 + 2], 16)
            .map_err(|_| "the key is not hexadecimal".to_string())?;
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_survives_being_written_down() {
        let key = [7u8; KEY];
        let text = encode(&key);
        assert!(text.starts_with(PREFIX));
        assert_eq!(*decode(&text).expect("read"), key);
        assert_eq!(
            *decode(&format!("# carried by hand\n{}", text.as_str())).expect("read"),
            key
        );

        for bad in [
            "",
            "# only a comment",
            "hello",
            "OSLO-PROFILE-1:zz",
            "OSLO-PROFILE-1:0102",
        ] {
            assert!(decode(bad).is_err(), "{bad:?} was accepted");
        }
    }

    /// **The fingerprint is of the key and nothing else**, so two machines that hold the same key
    /// agree without either of them sending it.
    #[test]
    fn the_fingerprint_follows_the_key() {
        let one = fingerprint(&[1u8; KEY]);
        let same = fingerprint(&[1u8; KEY]);
        let other = fingerprint(&[2u8; KEY]);

        assert_eq!(one, same);
        assert_ne!(one, other);
        assert_eq!(one.len(), FINGERPRINT);
        assert!(one.chars().all(|c| c.is_ascii_hexdigit()));
        // And it is not the key: an attacker holding this learns nothing to send.
        assert!(!encode(&[1u8; KEY]).contains(&one));
    }

    /// A name that is not a profile name has nowhere to keep a key, rather than a path built out of
    /// whatever was passed in.
    #[test]
    fn a_bad_name_has_no_path() {
        for bad in ["", "..", "a/b", "with space"] {
            assert!(path(bad).is_none(), "{bad:?} was given a path");
        }
    }
}
