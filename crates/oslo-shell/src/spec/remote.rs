//! Listing a directory on another machine, so `scp host:/pa<Tab>` can answer.
//!
//! ```text
//!   ssh -o BatchMode=yes -o ConnectTimeout=5 build  LC_ALL=C ls -1Ap -- '/srv/'
//! ```
//!
//! This is the one completion source in oslo that runs a program and talks to a network, and it is
//! here rather than in `oslo-ui` for that reason — see [`oslo_ui::spec::remote`], which decides when
//! to ask and remembers the answer.
//!
//! # Why it is allowed to fork at all
//!
//! Every other source is a file read, because there is always a local file that knows: `/proc` for
//! processes, `/etc/passwd` for users. **There is no local file that says what is on another
//! machine.** Either oslo asks the machine or it offers nothing, and offering nothing is what it did
//! before this — after `host:` the menu answered the *local* filesystem, which is worse than either.
//!
//! # What keeps it from being a hang
//!
//! * **`run::bounded_within`**, the runner every macro uses, with a longer deadline — ten seconds,
//!   see `WAIT` — its own process group, killed as a group when it expires.
//! * **`BatchMode=yes`**, which is the load-bearing one. Without it `ssh` prompts — for a password,
//!   for a passphrase, to accept a host key — and a prompt from a child while the editor holds the
//!   terminal in raw mode is a shell nobody can type into. With it, a machine that would have asked
//!   simply fails, and the menu stays shut. This is why it works only for machines a key already
//!   opens, which is the case worth having.
//! * **`ConnectTimeout`** below the deadline, so an unreachable address fails on its own rather than
//!   being killed.
//! * **One listing per directory per command**, which the caller enforces.
//!
//! # What is not done
//!
//! No `ControlMaster` is started. Opening a shared connection behind somebody's back leaves a socket
//! and a process they did not ask for; one that *exists* is used automatically by `ssh` itself, and
//! a person who wants the speed can say so in their `~/.ssh/config` — where it belongs.

use super::run::Ran;
use oslo_ui::spec::remote::Entry;

/// How long `ssh` may spend on the connection itself.
///
/// Under the deadline, so a machine that is simply not there ends as a failure with ssh's own
/// reason rather than as a killed process.
const CONNECT_SECONDS: &str = "5";

/// How long a whole listing may take: connect, authenticate, `ls`.
///
/// **Not the macro deadline.** Two seconds is plenty for a local program and too little for a
/// handshake to a slow or distant machine — which then offered nothing, while bash and zsh, which
/// wait as long as it takes, listed it. This is only ever reached on Tab, so it is a wait somebody
/// asked for.
const WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// The names in `dir` on `host`, or why they could not be read.
pub fn list(host: &str, dir: &str) -> Result<Vec<Entry>, String> {
    if !a_plausible_destination(host) {
        return Err(format!("'{host}' is not a machine ssh can be asked"));
    }
    let mut ssh = std::process::Command::new("ssh");
    ssh.arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg(format!("ConnectTimeout={CONNECT_SECONDS}"))
        // Warnings about a changed key or an added host are not completions or reasons, so only
        // errors reach stderr — whose last line is what a failure is explained with.
        .arg("-o")
        .arg("LogLevel=ERROR")
        // No terminal: this is a child of a shell whose terminal is in raw mode, and a pty here
        // would let the far end write onto the drawn menu.
        .arg("-T")
        .arg(host)
        .arg(remote_command(dir));

    let ran = super::run::bounded_within(ssh, WAIT);
    match ran.ok {
        true => Ok(entries_from(&ran.out)),
        false => Err(why_not(host, &ran)),
    }
}

/// One line saying why a listing failed: ssh's or `ls`'s own last word, or the deadline.
fn why_not(host: &str, ran: &Ran) -> String {
    if ran.expired {
        return format!("{host}: no answer in {}s", WAIT.as_secs());
    }
    match ran
        .err
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
    {
        Some(line) => line.to_string(),
        None => format!("{host}: could not be listed"),
    }
}

/// The command run on the far side.
///
/// `ls -1Ap`: one name per line, dotfiles included but not `.` and `..`, and a `/` after each
/// directory — which is the only type information needed and the only one every `ls` agrees on.
/// `LC_ALL=C` so a locale cannot reorder or translate anything.
fn remote_command(dir: &str) -> String {
    match dir.is_empty() {
        // No operand lists the login directory, which is what a bare `host:` means.
        true => "LC_ALL=C ls -1Ap".to_string(),
        false => format!("LC_ALL=C ls -1Ap -- {}", quoted_for_remote(dir)),
    }
}

/// A path as a single word for the *remote* shell.
///
/// **A leading `~` is left bare** so the far shell expands it — `host:~/pro<Tab>` is an ordinary
/// thing to type, and `'~/'` would be a directory called tilde. Everything after it is quoted, so a
/// space or a quote in the name is still one word and nothing in it can become a command.
fn quoted_for_remote(dir: &str) -> String {
    if dir == "~" {
        return "~".to_string();
    }
    if let Some(rest) = dir.strip_prefix("~/") {
        return match rest.is_empty() {
            true => "~/".to_string(),
            false => format!("~/{}", single_quoted(rest)),
        };
    }
    single_quoted(dir)
}

/// `it's` becomes `'it'\''s'` — the only escape a single-quoted shell word has.
fn single_quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// Whether a word can be handed to `ssh` as a destination.
///
/// **A destination beginning with `-` is an option**, and this one was typed by whoever is at the
/// keyboard: `scp -oProxyCommand=… ` is a word `ssh` would read as a flag rather than a machine.
/// Nothing here is passed through a shell, so there is no quoting to get wrong — but argv is not a
/// defence against a program's own option parsing.
fn a_plausible_destination(host: &str) -> bool {
    !host.is_empty() && !host.starts_with('-') && !host.contains(|c: char| c.is_whitespace())
}

/// One `ls -1Ap` line per entry, a trailing `/` marking a directory.
fn entries_from(out: &str) -> Vec<Entry> {
    out.lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .map(|line| match line.strip_suffix('/') {
            Some(name) => Entry {
                name: name.to_string(),
                directory: true,
            },
            None => Entry {
                name: line.to_string(),
                directory: false,
            },
        })
        .collect()
}

#[cfg(test)]
#[path = "remote/tests.rs"]
mod tests;
