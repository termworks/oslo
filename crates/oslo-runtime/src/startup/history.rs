//! Command-history settings, snapshots and the `history` builtin.

/// How many jobs `\j` in a `$PS1` reports.
///
/// Zero, and honestly so: [`oslo_shell::exec::job::JobManager`] installs the signal handlers but keeps
/// no table of running jobs, so there is nothing to count. Reporting zero is what a shell with no
/// background jobs would say anyway; when job tracking arrives this is the one place to change.
pub fn job_count() -> usize {
    0
}

use oslo_base::error::Result;
use oslo_shell::Environment;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// The default number of entries kept when `$HISTSIZE` says nothing.
///
/// bash's own default is 500; 10000 is the number nearly every distribution's default profile
/// sets instead, and history that ends before yesterday's command is history nobody searches.
const DEFAULT_HISTSIZE: usize = 10_000;

/// What the editor's history is configured from.
#[derive(Debug, PartialEq, Eq)]
pub struct Settings {
    /// The file history is loaded from and appended to, or `None` when there is no such file.
    ///
    /// `None` is the default, and it does **not** mean this session keeps no record — see
    /// [`Self::no_trace`].
    pub file: Option<PathBuf>,
    /// Whether the user asked for this session to leave nothing behind at all.
    ///
    /// **Separate from `file` being `None`, and it has to be.** `HISTFILE=""` is the documented way
    /// to run a shell that records nothing, and while the file had a default the two were the same
    /// question: the only way to have no file was to ask for none. Now that there is no default
    /// file, `file: None` is what an unconfigured shell looks like, and reading it as "leave no
    /// trace" would have silently taken the tracking store — the finder, `cd` ranking, the
    /// prediction model — away from everybody.
    pub no_trace: bool,
    /// Maximum entries held in memory and written to the file.
    pub max_size: usize,
    /// `oslo.history.ignore_space`: a line starting with whitespace is not remembered.
    pub ignore_space: bool,
    /// `oslo.history.ignore_dups`: a line identical to the one before it is not remembered.
    pub ignore_dups: bool,
    /// `oslo.history.immediate`: count a line in the tracking store on Enter, before it runs.
    pub immediate: bool,
}

/// Read `$HISTFILE` and `$HISTSIZE`, and `oslo.history` where the config set it.
///
/// An explicitly *empty* `HISTFILE` disables the file, and so does saying nothing at all.
///
/// **There is no default history file.** It used to fall back to `~/.oslo_history`, which meant a
/// shell that had never been configured wrote a plain-text copy of everything typed into it, into
/// `$HOME`, forever — a second store nothing reads. The history you search comes from the profile
/// database; the file exists only for other programs, so it is worth having only when you say so:
///
/// ```lua
/// oslo.history.file = "~/.oslo_history"
/// ```
///
/// `HISTFILE=~/.oslo_history oslo` does the same for one session.
///
/// **The environment wins.** `HISTSIZE=50 oslo` has to mean fifty for the same reason every other
/// shell variable outranks a config file: it is the setting you can make for one invocation without
/// editing anything. `oslo.history` fills in what the environment did not say — which until now it
/// did not do at all, having been parsed, validated, tested and then read by nothing.
pub fn settings(env: &Environment) -> Settings {
    let configured = oslo_ui::settings::current().history.clone();

    // An *empty* setting is the request to leave no trace; an absent one is simply nobody having
    // asked for a file. The two used to be the same because the file had a default.
    let (file, no_trace) = match env.get_var("HISTFILE") {
        Some("") => (None, true),
        Some(path) => (Some(PathBuf::from(path)), false),
        None => match configured.file.as_deref() {
            Some("") => (None, true),
            Some(path) => (Some(PathBuf::from(expand_tilde(env, path))), false),
            None => (None, false),
        },
    };
    let max_size = match env.get_var("HISTSIZE") {
        Some(value) => histsize(Some(value)),
        None => configured.size.unwrap_or(DEFAULT_HISTSIZE),
    };
    Settings {
        file,
        no_trace,
        max_size,
        ignore_space: configured.ignore_space,
        ignore_dups: configured.ignore_dups,
        immediate: configured.immediate,
    }
}

/// `~/h` as a path, since a config writes a path the way a person says one.
fn expand_tilde(env: &Environment, path: &str) -> String {
    let Some(rest) = path.strip_prefix('~') else {
        return path.to_string();
    };
    match home(env) {
        Some(home) => format!("{}{rest}", home.display()),
        None => path.to_string(),
    }
}

/// `$HISTSIZE` as a size, falling back to the default for anything that is not a number.
///
/// A negative or unparseable `HISTSIZE` means "the user did not say", not "keep nothing":
/// silently disabling history because of a typo in a profile is the worse failure. Zero is
/// honoured, because writing `HISTSIZE=0` is unambiguous.
fn histsize(raw: Option<&str>) -> usize {
    match raw {
        Some(text) => text.trim().parse::<usize>().unwrap_or(DEFAULT_HISTSIZE),
        None => DEFAULT_HISTSIZE,
    }
}

fn home(env: &Environment) -> Option<PathBuf> {
    let home = env
        .get_var("HOME")
        .map(str::to_string)
        .or_else(|| std::env::var("HOME").ok())?;
    if home.is_empty() {
        None
    } else {
        Some(PathBuf::from(home))
    }
}

/// Return whether the original line is excluded by `history.ignore_space`.
pub fn is_secret(raw_line: &str) -> bool {
    // `oslo.history.ignore_space = false` turns the leading-space convention off, for somebody who
    // pastes indented lines and would rather keep them. It used to be hardcoded on, so the setting
    // existed, was read, was stored — and did nothing.
    oslo_ui::settings::current().history.ignore_space
        && raw_line.starts_with(|c: char| c.is_whitespace())
}

/// What the `history` builtin prints, published by the REPL after every line.
static SNAPSHOT: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Set by `history -c`, cleared by the REPL once it has emptied the editor's own history.
static CLEAR_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Publish the editor's current history for the builtin to read.
pub fn publish(entries: Vec<String>) {
    if let Ok(mut guard) = SNAPSHOT.lock() {
        *guard = entries;
    }
}

/// Whether `history -c` ran since the last check, clearing the flag.
pub fn take_clear_request() -> bool {
    CLEAR_REQUESTED.swap(false, Ordering::SeqCst)
}

/// Make `history` a builtin of this shell.
///
/// Registered for non-interactive shells too, so that `type history` and completion agree with
/// what the shell can actually run; there it reports an empty history, exactly as bash does.
pub fn register(env: &mut Environment) {
    env.register_custom_builtin("history", builtin_history);
}

/// The variable that puts `-c` commands into the history too.
pub const ALLHIST: &str = "OSLO_ALLHIST";

/// Whether `$OSLO_ALLHIST` asks for `sh -c` commands to be recorded.
///
/// **Off unless asked**, and an environment variable rather than a config setting for a reason
/// that only matters once oslo is `/bin/sh`: `-c` does not read `init.lua`, so a Lua setting
/// would mean starting an interpreter and running the user's config on *every* `system()` call on
/// the machine. This is one `getenv`.
///
/// Exporting it is the way to turn it on for a session — and that is also the thing to know about
/// it: a child process inherits the variable, so a program that shells out while it is set has its
/// shell-outs recorded too. `make` driving a build through `sh -c` will fill the log. Set it where
/// you want the recording, not globally, unless that is what you meant.
/// **`0`, `false`, `no` and `off` mean off**, rather than merely being non-empty.
///
/// The `$NO_COLOR` convention is "any value at all", and it works there because the name is
/// already a negative and nobody writes `NO_COLOR=0`. This name reads as a switch, so somebody
/// will write `OSLO_ALLHIST=0` meaning off — and having that turn recording *on* is the kind of
/// surprise you only find by noticing your history filling up.
pub fn record_commands() -> bool {
    match std::env::var(ALLHIST) {
        Ok(value) => !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "false" | "no" | "off"
        ),
        Err(_) => false,
    }
}

/// Record a `-c` command, as though it had been typed at a prompt.
///
/// Both stores, because they answer different questions and a command you ran is a command you
/// ran: the tracking store is what the finder searches, and `$HISTFILE` is what the Up arrow
/// walks. Recording in one and not the other would mean a command you could find but not recall,
/// or the reverse.
pub fn record_command(env: &Environment, text: &str) {
    if text.trim().is_empty() {
        return;
    }
    let settings = settings(env);
    // The same courtesy the prompt extends: a line beginning with a space is not recorded, which
    // is how a password on a command line is kept out of the log.
    if settings.ignore_space && text.starts_with(' ') {
        return;
    }
    // The store is otherwise opened only by the interactive loop, because no other shell has
    // anything to put in it. This one does, so it opens its own — and pays for it, which is part
    // of why `$OSLO_ALLHIST` is off unless asked.
    oslo_base::track::install(
        oslo_base::track::default_path(
            std::env::var("XDG_DATA_HOME").ok().as_deref(),
            std::env::var("HOME").ok().as_deref(),
        )
        .and_then(|path| oslo_base::track::Track::open(&path)),
    );
    if let Some(db) = oslo_base::track::store() {
        db.append(text, oslo_base::track::log::MODE_SHELL);
        db.trim_soon(settings.max_size.max(1));
    }
    store::History::open(settings.file.clone(), settings.max_size).add(text);
}

fn builtin_history(_env: &mut Environment, args: &[String]) -> Result<i32> {
    let entries = SNAPSHOT.lock().map(|g| g.clone()).unwrap_or_default();

    match args.get(1).map(String::as_str) {
        None => print_entries(&entries, entries.len()),
        Some("-c") => {
            if let Ok(mut guard) = SNAPSHOT.lock() {
                guard.clear();
            }
            CLEAR_REQUESTED.store(true, Ordering::SeqCst);
        }
        Some(other) if other.starts_with('-') && other.len() > 1 => {
            eprintln!("oslo: history: {}: invalid option", other);
            eprintln!("history: usage: history [n] | history -c");
            return Ok(2);
        }
        Some(count) => match count.parse::<usize>() {
            Ok(n) => print_entries(&entries, n),
            Err(_) => {
                eprintln!("oslo: history: {}: numeric argument required", count);
                return Ok(1);
            }
        },
    }
    Ok(0)
}

/// Print the last `count` entries, numbered from 1 as bash numbers them.
///
/// The numbers are what `!42` refers to, so they count from the start of the session's history
/// and not from the start of the printed slice.
fn print_entries(entries: &[String], count: usize) {
    let start = entries.len().saturating_sub(count);
    for (i, entry) in entries.iter().enumerate().skip(start) {
        println!("{:5}  {}", i + 1, entry);
    }
}

#[cfg(test)]
mod tests {
    /// `oslo.history` is read when the environment says nothing, and yields to it when it does.
    ///
    /// The whole table was parsed, validated and covered by tests while nothing read it — a
    /// setting that is only *accepted* is worse than one that is refused, because the config looks
    /// right and does nothing.
    #[test]
    fn the_config_fills_in_what_the_environment_leaves_unsaid() {
        use oslo_ui::settings::{History, Settings as Interactive, install};

        install(Interactive {
            history: History {
                size: Some(42),
                file: None,
                ..History::default()
            },
            ..Interactive::default()
        });

        let mut env = Environment::new();
        env.unset_var("HISTSIZE");
        assert_eq!(settings(&env).max_size, 42, "the config is read");

        // And the environment still wins, so `HISTSIZE=7 oslo` means seven.
        env.set_var("HISTSIZE", "7", false);
        assert_eq!(settings(&env).max_size, 7);

        install(Interactive::default());
    }

    use super::*;

    #[test]
    fn histsize_defaults_and_survives_nonsense() {
        assert_eq!(histsize(None), DEFAULT_HISTSIZE);
        assert_eq!(histsize(Some("nope")), DEFAULT_HISTSIZE);
        assert_eq!(histsize(Some("-5")), DEFAULT_HISTSIZE);
        assert_eq!(histsize(Some(" 250 ")), 250);
        assert_eq!(histsize(Some("0")), 0);
    }

    #[test]
    fn an_empty_histfile_disables_the_file() {
        let mut env = Environment::new();
        env.set_var("HISTFILE", "", false);
        assert_eq!(settings(&env).file, None);
    }

    #[test]
    fn histfile_is_taken_from_the_variable() {
        let mut env = Environment::new();
        env.set_var("HISTFILE", "/tmp/xyz_history", false);
        env.set_var("HISTSIZE", "42", false);
        let s = settings(&env);
        assert_eq!(s.file, Some(PathBuf::from("/tmp/xyz_history")));
        assert_eq!(s.max_size, 42);
    }

    #[test]
    fn a_leading_space_marks_a_line_secret() {
        assert!(is_secret(" secret --token x"));
        assert!(is_secret("\techo hi"));
        assert!(!is_secret("echo hi"));
    }

    #[test]
    fn the_clear_flag_is_consumed_once() {
        let mut env = Environment::new();
        publish(vec!["echo one".to_string()]);
        assert_eq!(
            builtin_history(&mut env, &["history".into(), "-c".into()]).unwrap(),
            0
        );
        assert!(SNAPSHOT.lock().unwrap().is_empty());
        assert!(take_clear_request());
        assert!(!take_clear_request());
    }

    #[test]
    fn an_invalid_option_is_a_usage_error() {
        let mut env = Environment::new();
        assert_eq!(
            builtin_history(&mut env, &["history".into(), "-z".into()]).unwrap(),
            2
        );
        assert_eq!(
            builtin_history(&mut env, &["history".into(), "two".into()]).unwrap(),
            1
        );
    }
}

/// The in-memory history and the file behind it.
pub mod store;
