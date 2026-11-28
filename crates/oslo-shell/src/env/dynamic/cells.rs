//! What a listing shows of a variable bash computes when it is read.
//!
//! bash keeps each such variable's last value in the variable itself and computes a new one only
//! when the script reads the name. `declare -p`, `set` and a bare `declare` read nothing, so they
//! show the value the last read or assignment left: none at all before the first.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// The computed names that live outside the variable table, listed with what they last answered.
pub const COMPUTED: [&str; 8] = [
    "BASH_ARGV0",
    "BASH_COMMAND",
    "BASH_MONOSECONDS",
    "EPOCHREALTIME",
    "EPOCHSECONDS",
    "RANDOM",
    "SECONDS",
    "SRANDOM",
];

/// The computed names kept in the table, listed with no value until the script reads them.
const STORED: [&str; 11] = [
    "BASHPID",
    "HISTCMD",
    "BASH_SUBSHELL",
    "LINENO",
    "COMP_WORDBREAKS",
    "GROUPS",
    "DIRSTACK",
    "BASH_ARGC",
    "BASH_ARGV",
    "BASH_ALIASES",
    "BASH_CMDS",
];

/// Which of [`STORED`] the script has read, one bit each.
static READ: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// The last value each of [`COMPUTED`] answered or was given.
    static CELLS: RefCell<HashMap<&'static str, String>> = RefCell::new(HashMap::new());
}

/// The script read `name`: a listing shows its value in the table from now on.
pub fn mark_read(name: &str) {
    if let Some(at) = STORED.iter().position(|stored| *stored == name) {
        READ.fetch_or(1 << at, Ordering::Relaxed);
    }
}

/// Whether `name` is one of the table's computed names that no read has shown yet.
pub fn unread(name: &str) -> bool {
    STORED
        .iter()
        .position(|stored| *stored == name)
        .is_some_and(|at| READ.load(Ordering::Relaxed) & (1 << at) == 0 && !super::retired(name))
}

/// The table's names a listing shows as last read or assigned, rather than as they are now.
const RECORDED: [&str; 2] = ["LINENO", "HISTCMD"];

/// `name` answered or was given `value`.
pub fn remember(name: &str, value: &str) {
    let mut names = COMPUTED.iter().chain(RECORDED.iter());
    if let Some(&key) = names.find(|computed| **computed == name) {
        CELLS.with(|cells| cells.borrow_mut().insert(key, value.to_string()));
    }
}

/// What `name` last answered or was given, if anything.
pub fn remembered(name: &str) -> Option<String> {
    CELLS.with(|cells| cells.borrow().get(name).cloned())
}
