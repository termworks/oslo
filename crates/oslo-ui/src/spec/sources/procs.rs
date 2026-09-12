//! Running processes, the signals you can send them, and the terminals they sit on.
//!
//! ```text
//!   kill 12⇥            1247   cargo build      pid
//!   kill -s ⇥           TERM   15               signal
//!   pkill -t ⇥          pts/3  bresilla         terminal
//! ```
//!
//! # Never cached
//!
//! A pid list a minute old is a list of the wrong pids: the point of completing one is that it is
//! running *now*, and offering a process that has exited is worse than offering nothing — you would
//! send the signal to whatever inherited the number. So `/proc` is walked on every Tab.
//!
//! That is affordable because it is a directory read and one small file per entry, with no fork
//! anywhere. zsh asks `ps` for the same list.

use super::Suggestion;

/// Every process this user can see, newest first.
///
/// **Newest first, because that is what you are killing.** The thing you want to stop is nearly
/// always the thing you just started, and `/proc` enumerates in whatever order the directory
/// happens to be in. The dropdown does its own ranking on top, but the order it is given decides
/// ties — and among pids, later is more interesting.
pub fn pids() -> Vec<Suggestion> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut found: Vec<(u32, Suggestion)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let pid: u32 = name.to_str()?.parse().ok()?;
            let command = command_of(pid);
            Some((pid, Suggestion::new(pid.to_string(), command, "pid")))
        })
        .collect();
    found.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().map(|(_, one)| one).collect()
}

/// What a process is running, for the column beside the number.
///
/// `/proc/N/comm` rather than `cmdline`: the short name is what a person recognises, and a full
/// command line is a hundred characters that would push every other column off the row. A kernel
/// thread has an empty `cmdline` and a perfectly good `comm`, which is the other reason.
fn command_of(pid: u32) -> String {
    super::read(&format!("/proc/{pid}/comm")).trim().to_string()
}

/// The signals `kill -s` and `trap` accept.
///
/// A fixed list rather than a parse of anything: these are the POSIX signals plus the common Linux
/// ones, they have not changed in thirty years, and the alternative is `kill -l` — a process, on
/// the Tab key, to be told what the C library already knows.
///
/// **Named without the `SIG`**, because that is what `kill -s` takes and what `trap` takes. The
/// number is the note beside it, so `kill -s 9` remains findable by typing `9`.
pub fn signals() -> Vec<Suggestion> {
    const SIGNALS: &[(&str, u8)] = &[
        ("HUP", 1),
        ("INT", 2),
        ("QUIT", 3),
        ("ILL", 4),
        ("TRAP", 5),
        ("ABRT", 6),
        ("BUS", 7),
        ("FPE", 8),
        ("KILL", 9),
        ("USR1", 10),
        ("SEGV", 11),
        ("USR2", 12),
        ("PIPE", 13),
        ("ALRM", 14),
        ("TERM", 15),
        ("STKFLT", 16),
        ("CHLD", 17),
        ("CONT", 18),
        ("STOP", 19),
        ("TSTP", 20),
        ("TTIN", 21),
        ("TTOU", 22),
        ("URG", 23),
        ("XCPU", 24),
        ("XFSZ", 25),
        ("VTALRM", 26),
        ("PROF", 27),
        ("WINCH", 28),
        ("IO", 29),
        ("PWR", 30),
        ("SYS", 31),
    ];
    SIGNALS
        .iter()
        .map(|(name, number)| Suggestion::new(*name, number.to_string(), "signal"))
        .collect()
}

/// The terminals open on this machine, as `pts/N`.
///
/// **`pts/N` and not `/dev/pts/N`**, because that is the form `pkill -t`, `ps -t` and `write` take;
/// the `/dev/` prefix is what `ls` shows, not what the commands want.
///
/// The note is whoever owns the terminal, which is the whole point of the column on a machine with
/// more than one person logged in — the number alone says nothing about whose session it is.
pub fn terminals() -> Vec<Suggestion> {
    use std::os::unix::fs::MetadataExt;
    let Ok(entries) = std::fs::read_dir("/dev/pts") else {
        return Vec::new();
    };
    let mut found: Vec<(u32, Suggestion)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            // `/dev/pts` also holds `ptmx`, which is the multiplexer rather than a terminal.
            let number: u32 = name.to_str()?.parse().ok()?;
            let owner = entry.metadata().map(|it| it.uid()).unwrap_or_default();
            let who = super::system::user_named(owner);
            Some((
                number,
                Suggestion::new(format!("pts/{number}"), who, "terminal"),
            ))
        })
        .collect();
    found.sort_unstable_by_key(|(number, _)| *number);
    found.into_iter().map(|(_, one)| one).collect()
}

#[cfg(test)]
#[path = "procs/tests.rs"]
mod tests;
