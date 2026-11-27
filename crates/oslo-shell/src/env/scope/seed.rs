//! What a fresh shell starts with: the variables it describes *itself* with, and the three aliases
//! a person at a prompt is given.
//!
//! Every one is set only if the name is not already taken, so a caller who exported their own
//! `$UID` or `$OSTYPE` keeps it. They are here rather than in `scope.rs` because they answer one
//! question — "what process is this?" — and because that file is at the 600-line limit.

use super::Environment;
use super::environ::environ_set;

impl Environment {
    /// The three conveniences oslo ships — `ll`, `la`, `l` — for an interactive session only.
    ///
    /// **They used to be seeded in [`Environment::new`], which is every shell there is.** A script
    /// that defined `l() { … }` got `ls -CF` instead of its own function, because an alias is
    /// resolved before a function and the shell had quietly put one there. bash and dash both run
    /// the function; oslo now does too. It is also what the README promises about every extension
    /// oslo adds: unreachable from shell written before oslo existed.
    ///
    /// Called by the REPL when it builds the session's environment. A subshell forked from that
    /// session inherits the table by copy, so `(ll)` still works where you typed it.
    pub fn seed_interactive_aliases(&mut self) {
        for (name, expansion) in [("ll", "ls -la"), ("la", "ls -A"), ("l", "ls -CF")] {
            self.aliases
                .entry(name.to_string())
                .or_insert_with(|| expansion.to_string());
        }
    }

    /// Set the variables that describe the process the shell is running as.
    ///
    /// Unset, these fail *quietly* and wrongly: `[ "$UID" = 0 ]` is the root check in most install
    /// scripts, and an unset `$UID` makes it answer "not root" on a machine where the script is
    /// running as root. `$PPID` names the parent for lock files and process trees.
    ///
    /// Not exported, matching bash. Which of them an inherited value overrides follows bash name
    /// by name: `PPID`, `OPTERR` and `COMP_WORDBREAKS` are always the shell's own.
    pub(super) fn seed_process_vars(&mut self) {
        let uid = nix::unistd::getuid().as_raw();
        let euid = nix::unistd::geteuid().as_raw();
        let ppid = nix::unistd::getppid().as_raw();
        // `$SHLVL` counts how deep a shell is nested. Incremented from whatever we inherited, so
        // a shell inside a shell reads 2 — which is what a prompt showing nesting depth needs, and
        // what starship asks for by `--shlvl`.
        let shlvl = std::env::var("SHLVL")
            .ok()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .unwrap_or(0)
            .saturating_add(1);
        // Written through to the real environment as well as the table: a child gets its
        // environment from `execve`, so a value that lives only in oslo's map would leave every
        // nested shell reading the *grandparent's* depth.
        let shlvl = shlvl.to_string();
        self.vars.insert("SHLVL".to_string(), (shlvl.clone(), true));
        environ_set("SHLVL", &shlvl);

        // **Which session this is, exported so a child can say so too.** `oslo macros` runs as a
        // child of the shell whose macros it manages, and "off for this session" is a statement
        // about the *parent*: without a name both processes agree on, the manager would write down
        // a session nobody is running.
        //
        // The id itself is decided in `main`, by `track::session::begin`, because *being a shell* is
        // what starts a session and building an `Environment` is not — a tool builds one too, and a
        // tool that stamped a new session over the inherited one would be talking about itself.
        let session = oslo_base::track::session::id();
        self.vars
            .insert("OSLO_SESSION".to_string(), (session.clone(), true));
        environ_set("OSLO_SESSION", &session);
        // `PPID` is always this shell's; `UID` and `EUID` keep an inherited value, as bash does,
        // and are read-only integers only when the shell set them.
        self.vars
            .insert("PPID".to_string(), (ppid.to_string(), false));
        self.set_integer("PPID");
        self.set_readonly("PPID");
        for (name, value) in [("UID", uid), ("EUID", euid)] {
            if !self.vars.contains_key(name) {
                self.vars
                    .insert(name.to_string(), (value.to_string(), false));
                self.set_integer(name);
                self.set_readonly(name);
            }
        }
        for (name, value) in [
            ("OSTYPE", "linux-gnu".to_string()),
            ("HOSTTYPE", std::env::consts::ARCH.to_string()),
            ("MACHTYPE", machine_type()),
            ("PS4", "+ ".to_string()),
            ("BASH_LOADABLES_PATH", LOADABLES_PATH.to_string()),
        ] {
            if !self.vars.contains_key(name) {
                self.vars.insert(name.to_string(), (value, false));
            }
        }
        self.force_var("OPTERR", "1");
        self.force_var("COMP_WORDBREAKS", WORD_BREAKS);
    }

    /// Set `name` to `value` whatever was inherited, keeping an inherited export flag.
    fn force_var(&mut self, name: &str, value: &str) {
        let exported = self.vars.get(name).is_some_and(|(_, exported)| *exported);
        self.vars
            .insert(name.to_string(), (value.to_string(), exported));
        if exported {
            environ_set(name, value);
        }
    }

    /// The bash release oslo declares compatibility with: major, minor, patch level.
    const BASH_COMPAT: (u32, u32, u32) = (5, 3, 9);

    /// `$IFS`, which POSIX says the shell sets at startup to space, tab and newline.
    ///
    /// **It was a default without a variable.** Field splitting read `get_var("IFS")` and fell
    /// back to `" \t\n"` when nothing was there, so splitting behaved correctly while the
    /// *variable* did not exist: `${#IFS}` was 0, `${IFS+SET}` was empty, and under `set -u` any
    /// mention of `$IFS` was a fatal "unbound variable". `/usr/bin/xdg-terminal-exec` opens with
    /// `XTE__OIFS=$IFS` under `set -u` and died there.
    ///
    /// Not exported, matching bash and dash — `export -p` lists it in neither — because a child
    /// process has no use for its parent's field separators.
    ///
    /// Set only when unset, like everything else here: a caller who put `IFS` in the environment
    /// meant it, and POSIX says an inherited value is used.
    pub(super) fn seed_field_separator(&mut self) {
        if !self.vars.contains_key("IFS") {
            self.vars
                .insert("IFS".to_string(), (" \t\n".to_string(), false));
        }
    }

    /// `$PWD`, **exported**, from where the process actually is.
    ///
    /// POSIX: the shell sets `PWD` at startup to the absolute pathname of the current working
    /// directory. oslo only ever wrote it from `cd`, so a shell that had not changed directory yet
    /// had none at all — and nothing that inherits the environment could find out where it was.
    ///
    /// That is not a theoretical gap. `sshd` does not set `PWD`, so a shell logged into over SSH
    /// started with it missing, and every prompt renderer run as a child — hexe, starship,
    /// anything reading `$PWD` because it is handed a pipe rather than a terminal — had no
    /// directory to show until the first `cd` happened to write one.
    ///
    /// An inherited value is kept **only when it names this directory**. POSIX allows using the
    /// inherited one, and a value that survives a `chdir` the shell did not make is a lie: it is
    /// how `$PWD` ends up pointing at where the *parent* was.
    pub(super) fn seed_working_directory(&mut self) {
        let Ok(here) = std::env::current_dir() else {
            return;
        };
        let here = here.to_string_lossy().to_string();
        // Compared by what they resolve to, not as text: an inherited `$PWD` may reach this same
        // directory through a symlink, and POSIX prefers that spelling because it is the one the
        // user typed to get here.
        let inherited_is_here = self
            .vars
            .get("PWD")
            .map(|(value, _)| value.as_str())
            .filter(|value| !value.is_empty())
            .and_then(|value| std::fs::canonicalize(value).ok())
            .is_some_and(|resolved| std::fs::canonicalize(&here).is_ok_and(|now| resolved == now));
        // Written through to the real environment as well as the table, for the reason `SHLVL`
        // above is: a child's environment comes from `execve` reading `environ`, so a value that
        // lives only in oslo's map is invisible to exactly the programs this exists to serve.
        if !inherited_is_here {
            self.vars.insert("PWD".to_string(), (here.clone(), true));
            environ_set("PWD", &here);
            return;
        }
        // Kept, but exported: an inherited-but-unexported `PWD` is invisible to a child, which is
        // the whole reason this exists.
        if let Some(slot) = self.vars.get_mut("PWD") {
            slot.1 = true;
            environ_set("PWD", &slot.0.clone());
        }
    }

    /// `OPTIND`, which POSIX says a shell starts at 1.
    ///
    /// **`shift $((OPTIND-1))` is the line every option-parsing script ends with**, and it runs
    /// whether or not `getopts` matched anything. Unset, `$OPTIND` expands to nothing, the
    /// arithmetic reads it as 0 and the line becomes `shift -1` — "numeric argument required",
    /// status 1, and the positional parameters left where they were. bash and dash both start it
    /// at 1.
    ///
    /// Not exported, as in bash: it describes this shell's scan, and a child inheriting a
    /// half-finished cursor would be told something untrue.
    pub(super) fn seed_option_index(&mut self) {
        self.force_var("OPTIND", "1");
        self.set_integer("OPTIND");
    }

    /// `$BASH`, `$BASH_VERSION` and the read-only `BASH_VERSINFO` array.
    pub(super) fn seed_compatibility_vars(&mut self) {
        let (major, minor, patch) = Self::BASH_COMPAT;
        // **`$BASH` is how a script starts another copy of the shell**, and it was the one member
        // of this family that was missing. `exec "$BASH" -c …` and `"$BASH" script.sh` are ordinary
        // idioms — with nothing here they expanded to the empty word and the shell reported
        // `exec: : not found` with status 127, on a script that runs everywhere else.
        //
        // The resolved executable rather than `argv[0]`: bash reports the path it was invoked by,
        // but oslo is commonly reached through a symlink named `bash`, and the resolved path is the
        // one that still works when the child looks it up. Left unset when it cannot be read, since
        // an empty `$BASH` is the very thing this fixes.
        if !self.vars.contains_key("BASH")
            && let Ok(exe) = std::env::current_exe()
        {
            self.vars
                .insert("BASH".to_string(), (exe.display().to_string(), false));
        }
        self.force_var(
            "BASH_VERSION",
            &format!("{major}.{minor}.{patch}(1)-release"),
        );
        // Six elements, in bash's order: major, minor, patch, build, release status, machine.
        let fields = [
            major.to_string(),
            minor.to_string(),
            patch.to_string(),
            "1".to_string(),
            "release".to_string(),
            machine_type(),
        ];
        self.vars.remove("BASH_VERSINFO");
        self.arrays.insert(
            "BASH_VERSINFO".to_string(),
            crate::env::scope::array::ShellArray::from_values(fields),
        );
        self.set_readonly("BASH_VERSINFO");
    }
}

/// bash's `$BASH_LOADABLES_PATH` default.
const LOADABLES_PATH: &str =
    "/usr/local/lib/bash:/usr/lib/bash:/opt/local/lib/bash:/usr/pkg/lib/bash:/opt/pkg/lib/bash:.";

/// readline's word break characters, which bash publishes as `$COMP_WORDBREAKS`.
const WORD_BREAKS: &str = " \t\n\"'@><=;|&(:";

/// The GNU triplet bash reports as `$MACHTYPE`.
fn machine_type() -> String {
    let arch = std::env::consts::ARCH;
    let vendor = match arch {
        "x86_64" | "x86" => "pc",
        _ => "unknown",
    };
    format!("{arch}-{vendor}-linux-gnu")
}

#[cfg(test)]
mod pwd_tests {
    use crate::env::Environment;

    /// **A shell knows where it is before anything asks.** POSIX requires `PWD` at startup, and
    /// oslo only ever wrote it from `cd` — so a shell logged into over SSH, where `sshd` sets no
    /// `PWD`, had none until the user happened to change directory. Every prompt renderer run as a
    /// child reads `$PWD`, because it is handed a pipe and cannot ask the terminal.
    #[test]
    fn a_new_environment_knows_its_directory() {
        let env = Environment::new();
        let pwd = env.get_var("PWD").expect("PWD is set at startup");
        assert!(!pwd.is_empty());
        assert_eq!(
            std::fs::canonicalize(pwd).ok(),
            std::env::current_dir()
                .ok()
                .and_then(|p| std::fs::canonicalize(p).ok()),
            "PWD must name the directory the process is actually in"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::Environment;

    /// **`OPTIND` starts at 1**, which is what `shift $((OPTIND-1))` depends on.
    ///
    /// That line ends every option-parsing script and runs whether or not `getopts` matched
    /// anything. Unset, the arithmetic read the empty expansion as 0 and the line became
    /// `shift -1`: "numeric argument required", status 1, positional parameters untouched.
    #[test]
    fn the_option_index_starts_at_one() {
        let env = Environment::new();
        assert_eq!(env.get_var("OPTIND"), Some("1"));
    }

    /// And it describes *this* shell, so it is not handed to children.
    #[test]
    fn the_option_index_is_not_exported() {
        let env = Environment::new();
        assert!(
            !env.get_exported_vars().contains_key("OPTIND"),
            "a child would be told a cursor that is not its own"
        );
    }

    /// **`$BASH` names a shell a script can actually start.** `exec "$BASH" -c …` is an ordinary
    /// idiom; with the variable unset it expanded to the empty word and the shell answered
    /// `exec: : not found` with status 127, on a script that runs under every other bash.
    #[test]
    fn the_shell_can_name_itself() {
        let env = Environment::new();
        let named = env.get_var("BASH").expect("a shell knows its own path");
        assert!(!named.is_empty(), "an empty $BASH is what this fixes");
        assert!(
            std::path::Path::new(named).exists(),
            "$BASH must name something that can be run: {named}"
        );
    }

    /// It describes this shell, so it is not exported — bash does not export it either.
    #[test]
    fn the_shells_own_path_is_not_exported() {
        let env = Environment::new();
        assert!(!env.get_exported_vars().contains_key("BASH"));
    }
}
