//! A store's own crypto: the key it writes with, the writers it accepts, and which refusal it
//! reports when nothing opens a file.
//!
//! The format is [`native`]'s. What is decided here is the store's part of it:
//!
//! * **The writer** is the key the store writes with, so a file says which key sealed it.
//! * **The accepted writers** are the store's recipients, as this machine lists them, and the key
//!   that is opening the file. A file sealed by anybody else is refused, however well it opens.
//! * **The context** is the store's name and the secret's, so a file renamed, swapped for another
//!   or moved to another store does not open. A seal with no name is bound to a fixed label.
//! * **`OSLO2`** is read with one warning per process until the store has been rotated, and
//!   refused after: `rotate` writes `refuse OSLO2` into the store's section of `secrets.conf`.

use super::native::{self, Context, Plain, Refused, SecretKey};
use super::{KeySource, Recipient, Store, conf, dumpable, key};
use std::sync::atomic::{AtomicBool, Ordering};

/// What `rotate` writes into a store's section once nothing in it is `OSLO2`.
pub(super) const REFUSE_OSLO2: &str = "refuse OSLO2";

/// Whether this process has already said that a store holds `OSLO2` files.
static WARNED: AtomicBool = AtomicBool::new(false);

impl Store {
    /// Seal `value` as this store's key, to this store's recipients.
    pub(super) fn seal_natively(
        &self,
        name: Option<&str>,
        value: &[u8],
    ) -> Result<Vec<u8>, String> {
        let _hidden = dumpable::Hidden::new();
        let writer = self.key_to_write_with()?;
        let recipients = self.encrypt_to(&writer)?;
        native::seal(&writer, &recipients, &self.context(name), value)
    }

    /// Open `ciphertext` with whichever of this store's keys opens it, file keys before program
    /// keys, and report the most specific refusal when none does.
    pub(super) fn unseal_natively(
        &self,
        name: Option<&str>,
        ciphertext: &[u8],
    ) -> Result<Plain, String> {
        let what = self.what(name);
        let parsed = native::parse(ciphertext).map_err(|e| format!("{what}: {e}"))?;
        if parsed.legacy() && !self.reads_oslo2 {
            return Err(format!(
                "{what}: an OSLO2 file, which anybody holding a recipient's public half can write; \
                 this store has refused them since it was rotated (`{REFUSE_OSLO2}` in secrets.conf)"
            ));
        }
        let _hidden = dumpable::Hidden::new();
        let writers = self.writers();
        let context = self.context(name);
        let (mut tried, mut best, mut failed) = (0, Refused::NotOurs, Vec::new());
        for external in [false, true] {
            for answer in self.keys_of(external) {
                let Ok(key) = answer.map_err(|e| failed.push(e)) else {
                    continue;
                };
                tried += 1;
                match native::open(&key, &writers, &context, &parsed) {
                    Ok(value) => {
                        if parsed.legacy() {
                            self.warn_legacy(&what);
                        }
                        return Ok(value);
                    }
                    Err(refused) if rank(&refused) > rank(&best) => best = refused,
                    Err(_) => {}
                }
            }
        }
        Err(self.why_it_did_not_open(&what, tried, best, &failed))
    }

    /// Who this store writes for: its recipients, or the writer alone when it lists none.
    ///
    /// **A store with no `recipient` line is written for its own key**, and adding the first
    /// recipient is the moment that stops being implied, which is why the command writes the
    /// implied one down first.
    fn encrypt_to(&self, writer: &SecretKey) -> Result<Vec<[u8; 32]>, String> {
        if self.recipients.is_empty() {
            return Ok(vec![native::public_of(writer)]);
        }
        self.recipients.iter().map(Recipient::public).collect()
    }

    /// The writers this store accepts besides the key that opens a file: its usable recipients.
    fn writers(&self) -> Vec<[u8; 32]> {
        self.recipients
            .iter()
            .filter_map(|recipient| recipient.public().ok())
            .collect()
    }

    fn context<'a>(&'a self, name: Option<&'a str>) -> Context<'a> {
        match name {
            Some(name) => Context::Named {
                store: &self.name,
                name,
            },
            None => Context::Anonymous,
        }
    }

    /// `store/name`, or the store alone for a seal with no name.
    fn what(&self, name: Option<&str>) -> String {
        match name {
            Some(name) => format!("{}/{name}", self.name),
            None => self.name.clone(),
        }
    }

    fn warn_legacy(&self, what: &str) {
        if WARNED.swap(true, Ordering::Relaxed) {
            return;
        }
        crate::messages::warn(
            "secret",
            format!(
                "{what} is an OSLO2 file, which anybody holding a recipient's public half could \
                 have written; `oslo secret --store {} rotate` rewrites the store and refuses \
                 OSLO2 from then on",
                self.name
            ),
        );
    }

    /// Mark this store as holding no `OSLO2` file, so one that appears later is refused.
    pub(super) fn refuse_oslo2(&self) -> Result<(), String> {
        let mut declared = conf::read()?;
        if declared
            .section(&self.name)
            .is_some_and(|s| s.refuses_oslo2)
        {
            return Ok(());
        }
        declared.add(&self.name, REFUSE_OSLO2);
        declared.write()
    }

    /// Why nothing opened it: the most specific refusal any key gave, then every key source that
    /// failed rather than answered.
    fn why_it_did_not_open(
        &self,
        what: &str,
        tried: usize,
        best: Refused,
        failed: &[String],
    ) -> String {
        let said = self.refusal(what, tried, best);
        match failed.is_empty() {
            true => said,
            false => format!("{said}; {}", failed.join("; ")),
        }
    }

    fn refusal(&self, what: &str, tried: usize, best: Refused) -> String {
        let skipped = key::no_exec() && self.keys.iter().any(KeySource::is_external);
        if skipped && best == Refused::NotOurs {
            return format!(
                "{what}: no key opened it. $OSLO_SECRET_NO_EXEC is set, so {} of {} key sources \
                 were not run",
                self.keys.iter().filter(|s| s.is_external()).count(),
                self.keys.len()
            );
        }
        match best {
            _ if tried == 0 => format!("{}: no key to open it with", self.name),
            Refused::NotOurs => format!(
                "{what}: no key opened it, of the {tried} this store has: it was sealed for other \
                 keys, or under another name or store, or its header was changed"
            ),
            Refused::Writer(_) => format!(
                "{what}: {best}. Either somebody wrote it with a public half alone, or the key \
                 that wrote it is not listed here: `oslo secret recipient add` it if it is yours"
            ),
            refused => format!("{what}: {refused}"),
        }
    }
}

/// How much a refusal says about the file: a tampered body more than an unknown writer, and both
/// more than a key that is not a recipient.
fn rank(refused: &Refused) -> u8 {
    match refused {
        Refused::NotOurs => 0,
        Refused::Format(_) => 1,
        Refused::Writer(_) => 2,
        Refused::Tampered => 3,
    }
}
