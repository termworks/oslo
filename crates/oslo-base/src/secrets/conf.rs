//! `secrets.conf` — which stores exist, where each keeps its files, what it encrypts to, and where
//! its keys come from.
//!
//! # Why a file, and not Lua
//!
//! `oslo secret get` is dispatched as a tool: it never builds an `Environment`, never reads
//! `init.lua`, never starts a Lua interpreter. That is not an oversight — `$(oslo secret get
//! gh-token)` has to work from `dash`, from `cron`, from a `Makefile` and from a container, none of
//! which have run an oslo config. Configuration that only existed after `init.lua` had run would
//! apply in an interactive shell and silently not apply everywhere else, which for a *recipient
//! list* means encrypting to the wrong key and finding out at restore time.
//!
//! So it is a flat file that the process doing the decrypting reads for itself. One
//! `read_to_string` of a few hundred bytes, and nothing to keep in step.
//!
//! # Beside the key, not inside the store
//!
//! `$XDG_STATE_HOME/oslo/secrets.conf`, with the identity — not `$XDG_DATA_HOME` with the
//! ciphertext. The store is meant to be committable; this file names this machine's key paths, and
//! a recipient list published with a store is a disclosure of exactly which devices can read it.
//! A `secrets.conf` restored from somebody else's backup silently changing who this machine
//! encrypts to is recipient injection with nobody having to attack anything.
//!
//! # Edited in place
//!
//! `oslo secret key add` and its neighbours splice lines rather than re-render the file, so
//! comments, blank lines and the order things were written in survive being edited by a command.

use std::path::PathBuf;
use std::time::Duration;

use super::cipher::{self, Cipher};
use super::key::KeySource;

/// Where the file is: `$OSLO_SECRET_CONF`, else `$XDG_STATE_HOME/oslo/secrets.conf`.
pub fn path() -> Option<PathBuf> {
    named().or_else(|| Some(super::place::state_directory()?.join("secrets.conf")))
}

/// The file `$OSLO_SECRET_CONF` names, when it names one.
fn named() -> Option<PathBuf> {
    std::env::var_os("OSLO_SECRET_CONF")
        .filter(|named| !named.is_empty())
        .map(PathBuf::from)
}

/// A path as this file may write one: absolute, or `~/…` against `$HOME`. `what` names the line.
pub fn absolute(what: &str, written: &str) -> Result<PathBuf, String> {
    if let Some(rest) = written.strip_prefix("~/") {
        let home = std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .ok_or_else(|| format!("`{what} {written}` starts with ~/, and $HOME is not set"))?;
        return Ok(PathBuf::from(home).join(rest));
    }
    let path = PathBuf::from(written);
    match path.is_absolute() {
        true => Ok(path),
        false => Err(format!(
            "`{what} {written}` is relative, so it would mean a different place in every \
             directory; write an absolute path, or one that starts with ~/"
        )),
    }
}

/// One store's declaration.
#[derive(Debug, Default, Clone)]
pub struct Section {
    pub name: String,
    pub directory: Option<PathBuf>,
    pub keys: Vec<KeySource>,
    /// Who this store's files are written for, as written.
    pub recipients: Vec<String>,
    /// Set when another program does this store's crypto instead of oslo's own.
    pub cipher: Cipher,
    /// `crypto hook`: Lua does this store's crypto, so only a process running Lua can open it.
    pub hooked: bool,
    /// `key timeout`: how long a key or cipher command may take.
    pub timeout: Option<Duration>,
    /// `refuse OSLO2`: a file in the format before writers were authenticated is refused.
    pub refuses_oslo2: bool,
}

/// The whole file: the lines as written, and what they mean.
#[derive(Debug, Default, Clone)]
pub struct Conf {
    lines: Vec<String>,
    pub default: Option<String>,
    pub sections: Vec<Section>,
}

/// Read it, or an empty one when there is no file — which is every install that has never
/// configured anything, and must cost one failed `open`.
pub fn read() -> Result<Conf, String> {
    use crate::private::{Links, SMALL};
    let Some(path) = path() else {
        return Ok(Conf::default());
    };
    let Some(bytes) = crate::private::read(&path, SMALL, Links::Follow)? else {
        return Ok(Conf::default());
    };
    let text = String::from_utf8(bytes).map_err(|_| format!("{}: not text", path.display()))?;
    parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// **A malformed line is an error, never a skipped line.** The failure this prevents is a typo in a
/// `recipient` that leaves a store encrypted to one fewer key than its owner believes, which is
/// discovered on the day the other key is the only one left.
pub fn parse(text: &str) -> Result<Conf, String> {
    let mut conf = Conf {
        lines: text.lines().map(str::to_string).collect(),
        ..Conf::default()
    };
    let mut here: Option<Section> = None;

    for (number, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let at = number + 1;

        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            let name = name.trim();
            valid_store_name(name).map_err(|e| format!("line {at}: {e}"))?;
            if let Some(done) = here.replace(Section {
                name: name.to_string(),
                ..Section::default()
            }) {
                conf.sections.push(done);
            }
            continue;
        }

        let (word, rest) = split_word(line);
        let shown = super::recipient::shown(word);
        let verb = shown.as_str();
        match (word, here.as_mut()) {
            ("default", None) => conf.default = Some(rest.to_string()),
            ("default", Some(_)) => {
                return Err(format!(
                    "line {at}: `default` belongs above the first [store]"
                ));
            }
            (_, None) => {
                return Err(format!("line {at}: {verb:?} before any [store]"));
            }
            ("directory", Some(section)) => {
                section.directory =
                    Some(absolute(verb, rest).map_err(|e| format!("line {at}: {e}"))?);
            }
            ("key", Some(section)) if split_word(rest).0 == "timeout" => {
                section.timeout = Some(
                    super::child::parse_timeout(split_word(rest).1)
                        .map_err(|e| format!("line {at}: {e}"))?,
                );
            }
            // **Refused rather than parsed and ignored.** Without the built-in mechanism these say
            // nothing — the key and the recipients belong to whatever program the store names — and
            // a line that quietly did nothing is how somebody comes to believe a colleague can read
            // a store they cannot.
            ("key" | "recipient" | "refuse", Some(_)) if !cfg!(feature = "crypt") => {
                return Err(format!(
                    "line {at}: this oslo has no built-in crypto, so `{verb}` means nothing here — \
                     the key and the recipients are the business of the `encrypt`/`decrypt command` \
                     this store names"
                ));
            }
            ("recipient", Some(section)) => section.recipients.push(rest.to_string()),
            ("refuse", Some(section)) => match rest {
                "OSLO2" => section.refuses_oslo2 = true,
                _ => return Err(format!("line {at}: `refuse` takes OSLO2 and nothing else")),
            },
            ("key", Some(section)) => section
                .keys
                .push(KeySource::parse(rest).map_err(|e| format!("line {at}: {e}"))?),
            ("crypto", Some(section)) => match rest {
                "hook" => section.hooked = true,
                "native" => section.hooked = false,
                other => {
                    return Err(format!(
                        "line {at}: `crypto` is `hook` or `native`, not {other:?}"
                    ));
                }
            },
            ("encrypt", Some(section)) => {
                section.cipher.encrypt =
                    Some(cipher::parse(verb, rest).map_err(|e| format!("line {at}: {e}"))?);
            }
            ("decrypt", Some(section)) => {
                section.cipher.decrypt =
                    Some(cipher::parse(verb, rest).map_err(|e| format!("line {at}: {e}"))?);
            }
            (_, _) => return Err(format!("line {at}: {verb:?} is not a thing to say here")),
        }
    }
    if let Some(done) = here {
        conf.sections.push(done);
    }
    Ok(conf)
}

impl Conf {
    /// What one store declares, if it declares anything.
    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.name == name)
    }

    /// The file as it now stands.
    pub fn text(&self) -> String {
        let mut text = self.lines.join("\n");
        if !text.is_empty() {
            text.push('\n');
        }
        text
    }

    /// Put `line` at the end of `section`, adding the section if it is not there yet.
    pub fn add(&mut self, section: &str, line: &str) {
        match self.span(section) {
            Some((_, end)) => self.lines.insert(end, line.to_string()),
            None => {
                if !self.lines.is_empty() && !self.lines[self.lines.len() - 1].trim().is_empty() {
                    self.lines.push(String::new());
                }
                self.lines.push(format!("[{section}]"));
                self.lines.push(line.to_string());
            }
        }
    }

    /// Take out every line in `section` that `wanted` says to, and answer how many went.
    pub fn remove(&mut self, section: &str, wanted: impl Fn(&str) -> bool) -> usize {
        let Some((start, end)) = self.span(section) else {
            return 0;
        };
        let mut gone = 0;
        for at in (start..end).rev() {
            if wanted(self.lines[at].trim()) {
                self.lines.remove(at);
                gone += 1;
            }
        }
        gone
    }

    /// The store a bare `oslo secret` means.
    pub fn set_default(&mut self, name: &str) {
        let line = format!("default {name}");
        let first_section = self
            .lines
            .iter()
            .position(|l| l.trim_start().starts_with('['))
            .unwrap_or(self.lines.len());
        match self.lines[..first_section]
            .iter()
            .position(|l| split_word(l.trim()).0 == "default")
        {
            Some(at) => self.lines[at] = line,
            None => self.lines.insert(0, line),
        }
    }

    /// Where `section`'s lines are: its header index, and one past its last line.
    fn span(&self, section: &str) -> Option<(usize, usize)> {
        let header = format!("[{section}]");
        let start = self.lines.iter().position(|l| l.trim() == header)?;
        let end = self.lines[start + 1..]
            .iter()
            .position(|l| l.trim_start().starts_with('['))
            .map(|at| start + 1 + at)
            .unwrap_or(self.lines.len());
        Some((start + 1, end))
    }

    /// Write it back where [`path`] says, mode `0600` and atomically — it names key paths, and a
    /// half-written one would be a store with half a recipient list.
    ///
    /// `$XDG_STATE_HOME/oslo` is oslo's and is made or tightened to `0700`; the directory of a
    /// `$OSLO_SECRET_CONF` is somebody's choice and is only made when missing. A symbolic link at
    /// the file's own path is replaced by the file, and its target is left as it was.
    pub fn write(&self) -> Result<(), String> {
        let path = path().ok_or("no $XDG_STATE_HOME and no $HOME, so there is nowhere for it")?;
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            match named() {
                Some(_) => crate::private::directory(parent)?,
                None => crate::private::own_directory(parent)?,
            }
        }
        crate::private::replace(&path, self.text().as_bytes())
    }
}

/// A store name is a directory name and a section header, so it may not be either of those things
/// out of place. `plugin.` is a namespace rather than a rule: the command line reaches it, Lua
/// does not.
pub fn valid_store_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("a store needs a name".to_string());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        || name.starts_with('.')
        || name.contains("..")
    {
        return Err(format!(
            "{:?} is not a usable name for a store",
            super::recipient::shown(name)
        ));
    }
    Ok(())
}

/// The first word, and everything after it.
pub fn split_word(line: &str) -> (&str, &str) {
    match line.split_once(char::is_whitespace) {
        Some((word, rest)) => (word, rest.trim()),
        None => (line, ""),
    }
}
