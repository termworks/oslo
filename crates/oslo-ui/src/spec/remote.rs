//! Listing a directory on another machine, for `scp host:/pa<Tab>`.
//!
//! ```text
//!   scp report.pdf build:/srv/<Tab>
//!     /srv/www/       directory   remote
//!     /srv/backup/    directory   remote
//!     /srv/notes.md               remote
//! ```
//!
//! # Why this is a hook and not a function
//!
//! Listing the far side means running `ssh`, and oslo-ui does not run anything — the same
//! inversion as [`crate::completion::set_command_completer`], and for the same layering reason.
//! The shell installs the lister; this decides *when* to ask it and remembers what it said.
//!
//! # It is asked on a keystroke, so it is asked rarely
//!
//! Every other completion source in oslo is a file read precisely because this path holds the
//! terminal in raw mode. This one cannot be: there is no local file that says what is on another
//! machine. So the cost is paid where it cannot be avoided and bounded everywhere it can be:
//!
//! * **A directory is asked for once.** The answer is remembered for the rest of the command, so
//!   walking `host:/usr/<Tab>lib/<Tab>` costs one connection per directory and none for a second
//!   look at the same one.
//! * **A failure is remembered too.** A machine that is not reachable would otherwise cost the
//!   lister's full deadline on *every* keystroke, which is a shell that freezes each time Tab is
//!   pressed rather than once.
//! * **Both are forgotten when a command runs** — see [`forget`]. Connecting a VPN, adding a key
//!   or creating the directory are all things somebody does between two prompts, and a session-long
//!   memory of "unreachable" would outlive the reason for it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// One name on the far machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    /// Shown as a trailing `/`, and what lets the menu keep the word open for the next segment.
    pub directory: bool,
}

/// Given an ssh destination and a directory, the names in it — or `None` when it could not be read.
///
/// `None` and `Some(vec![])` are different answers: an empty directory has been listed, and a
/// machine that could not be reached has not. Only the first is worth showing as "nothing here".
pub type Lister = Rc<dyn Fn(&str, &str) -> Option<Vec<Entry>>>;

/// A machine and a directory on it — what one listing is remembered under.
type Asked = (String, String);

/// What came back, `None` being a machine that could not be reached.
type Answer = Option<Vec<Entry>>;

thread_local! {
    /// Thread-local for the reason the command completer is: only the editor's thread completes.
    static LISTER: RefCell<Option<Lister>> = const { RefCell::new(None) };
    static SEEN: RefCell<HashMap<Asked, Answer>> = RefCell::new(HashMap::new());
}

/// Install the lister. `None` removes it, and remote completion goes quiet.
pub fn set_lister(hook: Option<Lister>) {
    LISTER.with(|slot| *slot.borrow_mut() = hook);
    forget();
}

/// Whether anything can answer for another machine at all.
pub fn available() -> bool {
    LISTER.with(|slot| slot.borrow().is_some())
}

/// The names in `dir` on `host`, asking at most once per directory per command.
pub fn entries(host: &str, dir: &str) -> Option<Vec<Entry>> {
    let key = (host.to_string(), dir.to_string());
    if let Some(known) = SEEN.with(|seen| seen.borrow().get(&key).cloned()) {
        return known;
    }
    // Cloned out before the call: the lister runs a command, which can complete another word, and
    // that would come back through here onto the outstanding borrow.
    let lister = LISTER.with(|slot| slot.borrow().clone())?;
    let found = lister(host, dir);
    SEEN.with(|seen| seen.borrow_mut().insert(key, found.clone()));
    found
}

/// Forget every listing. Called once per command — see the module docs.
pub fn forget() {
    SEEN.with(|seen| seen.borrow_mut().clear());
}

/// The directory to list and the fragment to match, from what has been typed after `host:`.
///
/// ```text
///   ""            →  ("",          "")        the login directory
///   "/var/lo"     →  ("/var/",     "lo")
///   "/var/log/"   →  ("/var/log/", "")
///   "rel"         →  ("",          "rel")     relative to the login directory
/// ```
pub fn split(stem: &str) -> (&str, &str) {
    match stem.rfind('/') {
        Some(at) => stem.split_at(at + 1),
        None => ("", stem),
    }
}

#[cfg(test)]
#[path = "remote/tests.rs"]
mod tests;
