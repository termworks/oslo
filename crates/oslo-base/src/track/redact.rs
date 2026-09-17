//! What may be written down, and how much of it.
//!
//! This store records the command line *and* the directory it ran in, and the directory is often
//! the more identifying half — a client's name, an employer's, an unreleased project. The
//! mechanisms that answer that are layered so that no single one has to be perfect.
//!
//! The two the user controls are elsewhere and come first: a leading space still means "do not
//! record this", and a non-interactive shell has no store at all. What is left here is the
//! structural mitigation and the filters.
//!
//! # The structural mitigation
//!
//! A secret is almost never the command *name*; it is in the arguments. So when a line trips any
//! filter, the row is still written — with `argv` reduced to [`head_of`] alone. The directory, the
//! run count, the timing and the exit status all survive; only the risky text is dropped. A
//! denylist that has to be perfect is a bad design. A denylist whose worst failure is reduced
//! resolution is a fine one, and that is what makes it acceptable that the list below is
//! incomplete — it always will be.

use regex::Regex;
use std::sync::OnceLock;

/// The longest line worth remembering.
///
/// Past this it is a paste rather than a habit: it will never be suggested, it is the shape a
/// leaked key arrives in, and storing it would put a kilobyte of one-off text in a table whose
/// whole argument is that it is bounded by distinct behaviour.
const MAX_ARGV: usize = 4096;

/// Words that wrap another command rather than being one.
///
/// Grouping everything a user does under `sudo` makes the per-tool timing table a joke.
const WRAPPERS: &[&str] = &[
    "builtin", "command", "doas", "env", "ionice", "nice", "nohup", "sudo", "time",
];

/// Wrapper options that take a separate value, so the value is not mistaken for the command.
///
/// Without this, `sudo -u root cargo build` groups under `root`. Long options are skipped whole
/// because the glued `--user=root` form is the common one.
const VALUE_FLAGS: &[&str] = &["-C", "-U", "-c", "-g", "-n", "-p", "-u"];

/// Tools whose first argument names what they are actually doing.
///
/// `cargo build` and `cargo test` are not the same activity and must not share a timing row, which
/// is the whole reason `head` exists as a column. The alternative — a list of *verbs* — was
/// rejected: it groups `ls build` as a build.
const SUBCOMMANDS: &[&str] = &[
    "apt",
    "apt-get",
    "aws",
    "brew",
    "bun",
    "bundle",
    "cargo",
    "composer",
    "conda",
    "deno",
    "dnf",
    "docker",
    "gcloud",
    "gem",
    "gh",
    "git",
    "go",
    "helm",
    "ip",
    "kubectl",
    "nix",
    "nmcli",
    "npm",
    "pacman",
    "pip",
    "pip3",
    "pnpm",
    "podman",
    "poetry",
    "port",
    "rustup",
    "systemctl",
    "terraform",
    "tmux",
    "uv",
    "yarn",
    "zfs",
    "zpool",
];

/// Commands whose arguments are, by their nature, credentials.
const SECRET_COMMANDS: &[&str] = &[
    "gpg",
    "htpasswd",
    "keyctl",
    "mysql",
    "op",
    "openssl",
    "pass",
    "psql",
    "secret-tool",
    "security",
    "ssh-add",
    "vault",
];

/// Subcommands that hand a tool a credential.
const SECRET_SUBCOMMANDS: &[(&str, &str)] = &[
    ("aws", "configure"),
    ("aws", "sso"),
    ("docker", "login"),
    ("gh", "auth"),
    ("npm", "adduser"),
    ("npm", "login"),
    ("npm", "token"),
];

/// Variable names that say what they hold.
const SECRET_NAMES: &[&str] = &["KEY", "PASS", "SECRET", "TOKEN"];

/// Option names whose value is a credential, in every spelling anyone writes them.
fn secret_option() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(
            r"(?i)^--?(password|passwd|token|secret|api[-_]?key|auth|bearer|credential)([=:].*)?$",
        )
        .expect("a literal pattern")
    })
}

/// The interesting part of a command line: what you ran, not how you launched it.
///
/// Leading `VAR=value` assignments and wrapper words come off, and a tool whose first argument is a
/// subcommand keeps two words. `sudo cargo build --release` is `cargo build`.
///
/// Only the first physical line is read: a command continued over several lines is one entry with
/// newlines in it, and its head is on the first of them.
pub fn head_of(line: &str) -> String {
    let words: Vec<&str> = first_command(line.lines().next().unwrap_or_default())
        .split_whitespace()
        .collect();

    let mut at = 0;
    while at < words.len() {
        if is_assignment(words[at]) {
            at += 1;
            continue;
        }
        if WRAPPERS.contains(&words[at]) {
            match after_wrapper(&words, at + 1) {
                // A wrapper with nothing after it is the command. `time` on its own is `time`.
                Some(next) => at = next,
                None => break,
            }
            continue;
        }
        break;
    }

    let Some(&command) = words.get(at) else {
        return String::new();
    };
    match words.get(at + 1) {
        Some(&second) if SUBCOMMANDS.contains(&command) && is_subcommand(second) => {
            format!("{command} {second}")
        }
        _ => command.to_string(),
    }
}

/// The characters that end one command and begin another, or send its output somewhere.
const SEPARATORS: [char; 8] = [';', '&', '|', '(', ')', '<', '>', '`'];

/// As much of a line as belongs to the command it starts with.
///
/// Splitting on whitespace alone leaves a separator glued to the word beside it, and the head is
/// then a string no other line will ever produce: `cd; pwd` gave a head of `cd;`, filed apart from
/// every other `cd` the user has ever run. Since the whole reason `head` is a column is that "what
/// does this tool cost me" should be one grouped read, a punctuation mark that splits a tool into
/// two groups defeats it.
///
/// Leading separators come off first — `(cd build && make)` opens with one, and cutting at it would
/// leave nothing and drop the row entirely rather than merely mis-file it.
///
/// Quoting is not tracked, deliberately. Cutting `git commit -m "wip; fix"` at the quoted `;` shortens
/// a line whose first two words this has already read, and those are all it returns. A quote-aware
/// scan here would be a second parser living beside the real one and drifting from it, to decide
/// something no caller can observe.
fn first_command(line: &str) -> &str {
    let line = line.trim_start().trim_start_matches(SEPARATORS);
    match line.find(SEPARATORS) {
        Some(end) => &line[..end],
        None => line,
    }
}

/// The index of the first word after a wrapper's own options and assignments, or `None` when the
/// wrapper was the last word.
fn after_wrapper(words: &[&str], mut at: usize) -> Option<usize> {
    while at < words.len() {
        let word = words[at];
        if is_assignment(word) {
            at += 1;
            continue;
        }
        if word.starts_with('-') && word.len() > 1 {
            at += if VALUE_FLAGS.contains(&word) { 2 } else { 1 };
            continue;
        }
        return Some(at);
    }
    None
}

/// Whether a word is a `NAME=value` assignment.
fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Whether a word is shaped like a subcommand rather than an option, a path or a file name.
fn is_subcommand(word: &str) -> bool {
    word.starts_with(|c: char| c.is_ascii_alphabetic())
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// What may be stored for a line: the `argv` to keep, and its `head`.
///
/// Both are empty for a line with no command in it, which is the signal not to write a row at all.
/// When the line trips a filter the two come back equal — see the module note on why that is the
/// right failure.
pub fn prepare(line: &str) -> (String, String) {
    let line = line.trim();
    let head = head_of(line);
    if head.is_empty() {
        return (String::new(), String::new());
    }
    if is_risky(line) {
        return (head.clone(), head);
    }
    (line.to_string(), head)
}

/// Whether a line's arguments must not be written down.
///
/// None of these is load-bearing on its own, and none of them needs to be: a false positive costs
/// one line's worth of resolution in a statistics table.
pub fn is_risky(line: &str) -> bool {
    if line.len() > MAX_ARGV {
        return true;
    }
    // A heredoc body, or any continued line. Never recorded: the interesting text of a heredoc is
    // by definition not the command, and a multi-line entry can never be offered as a suggestion
    // anyway.
    if line.contains('\n') {
        return true;
    }

    let words: Vec<&str> = line.split_whitespace().collect();
    let head = head_of(line);
    let mut parts = head.split(' ');
    let command = parts.next().unwrap_or_default();
    let subcommand = parts.next().unwrap_or_default();
    if SECRET_COMMANDS.contains(&command) || SECRET_SUBCOMMANDS.contains(&(command, subcommand)) {
        return true;
    }

    for (at, word) in words.iter().enumerate() {
        if is_risky_word(word) {
            return true;
        }
        // `-u user:pass`, which curl and friends take and which no shape test would catch.
        if *word == "-u" && words.get(at + 1).is_some_and(|next| is_user_password(next)) {
            return true;
        }
    }
    false
}

/// Whether one word is, or plainly holds, a credential.
fn is_risky_word(word: &str) -> bool {
    if secret_option().is_match(word) {
        return true;
    }
    // A glued short option with a value — `-phunter2`. Long options are excluded because
    // `--prefix=/usr` is not a password.
    if word.len() > 2 && word.starts_with("-p") && !word.starts_with("--") {
        return true;
    }
    if word.len() >= "Authorization:".len() && word.to_ascii_lowercase().contains("authorization:")
    {
        return true;
    }
    if is_assignment(word) {
        let (name, value) = word.split_once('=').unwrap_or((word, ""));
        let upper = name.to_ascii_uppercase();
        if SECRET_NAMES.iter().any(|secret| upper.contains(secret)) && could_be_a_secret(value) {
            return true;
        }
        // **The value is judged, never `NAME=value`.** The name is a label the user chose, and
        // gluing it to the value makes an ordinary assignment look like a key: `WAYLAND_DISPLAY=
        // wayland-0` is 25 characters with upper case, lower case and a digit in it, which is
        // every test [`looks_like_a_key`] applies — so it was reduced to `export` and vanished
        // from the history it was typed into.
        return is_known_key(value) || looks_like_a_key(value);
    }
    is_known_key(word) || looks_like_a_key(word)
}

/// Whether an assignment's *value* could be the secret, rather than pointing at where one is.
///
/// **The name alone is not enough**, and a real line proved it:
///
/// ```text
/// sudo make hardware-smoke SIGN_KEY="$MOK/MOK.priv" SIGN_CERT="$MOK/MOK.der"
/// ```
///
/// `SIGN_KEY` contains `KEY`, so the whole line was reduced to `make` and never appeared in the
/// history again — while the value it was hiding was `$MOK/MOK.priv`, a path to a file. Three
/// shapes cannot be the secret and are the overwhelming majority of what people write:
///
/// ```text
/// KEY=            nothing to leak
/// KEY="$MOK/x"    a variable reference; whatever is secret lives in the variable, not here
/// KEY=~/x.priv    a path, which names a key rather than being one
/// ```
///
/// A literal — `KEY=hunter2`, `TOKEN=abc123` — still trips it, which is the case the rule is for.
fn could_be_a_secret(value: &str) -> bool {
    let value = value.trim_matches(['"', '\'']);
    if value.is_empty() || value.contains('$') {
        return false;
    }
    // A path, in any of the ways one is written. A base64 secret can hold a `/` too, which is why
    // this is not the last word on the matter: `is_known_key` and `looks_like_a_key` still read the
    // value afterwards, so a blob that happens to contain a slash is caught by its shape.
    let a_path = value.starts_with(['/', '~', '.']) || value.contains('/');
    !a_path
}

/// The credential formats that announce themselves.
fn is_known_key(word: &str) -> bool {
    let value = word.rsplit(['=', ':']).next().unwrap_or(word);
    for candidate in [word, value] {
        if candidate.starts_with("sk-")
            || candidate.starts_with("AKIA")
            || candidate.starts_with("eyJ")
            || candidate.starts_with("-----BEGIN")
        {
            return true;
        }
        if let Some(rest) = candidate.strip_prefix("gh")
            && rest.starts_with(['p', 'o', 'u', 's', 'r'])
            && rest[1..].starts_with('_')
        {
            return true;
        }
    }
    false
}

/// Whether a word has the shape of a key rather than of anything a person typed.
///
/// Long, mixed case, with digits, and with none of the punctuation that makes a path a path.
fn looks_like_a_key(word: &str) -> bool {
    word.len() >= 24
        && !word.contains('/')
        && !word.contains('.')
        && word.chars().any(|c| c.is_ascii_uppercase())
        && word.chars().any(|c| c.is_ascii_lowercase())
        && word.chars().any(|c| c.is_ascii_digit())
}

/// Whether a word is a `user:password` pair.
fn is_user_password(word: &str) -> bool {
    word.split_once(':')
        .is_some_and(|(user, password)| !user.is_empty() && !password.is_empty())
}

/// Directories that are excluded as themselves, not as the top of a subtree.
///
/// `/tmp` is a lobby: nobody wants `cd` to jump to it. Plenty of people work in `/tmp/build-xyz`,
/// and refusing those with it would silently delete the feature for them.
const EXCLUDED_DIRS: &[&str] = &["/tmp"];

/// Path components that exclude everything beneath them.
///
/// A store that ranks `~/.cargo/registry/src/index.crates.io-abcd/serde-1.0.219` because a build
/// touched it is worse than useless, and the same goes for the thousand directories a
/// `node_modules` is.
const EXCLUDED_COMPONENTS: &[&str] = &[".git", "node_modules"];

/// Whether a directory is never offered as a `cd` destination.
///
/// **Only the jump.** What runs in one is recorded like anywhere else: a command that vanished from
/// the history finder because of where it ran is a history that cannot be trusted, and `/tmp` is
/// where a lot of throwaway work happens. This is the design's directory exclusion list
/// (`docs/research/smart-cd.md`, Privacy and size, item 6), applied the way `$HOME` always was — see
/// `super::query`. Note which of these are subtrees and which are not: the difference is the
/// difference between refusing `/tmp` and refusing everybody who builds in it.
pub fn is_excluded(path: &str) -> bool {
    let path = path.trim_end_matches('/');
    if EXCLUDED_DIRS.contains(&path) {
        return true;
    }
    path.split('/')
        .any(|component| EXCLUDED_COMPONENTS.contains(&component))
}

#[cfg(test)]
#[path = "redact/tests.rs"]
mod tests;
