//! oslo's own tools: `oslo config`, `oslo history`, and the rest.
//!
//! # Sharing the operand slot with scripts
//!
//! POSIX defines the shell's synopsis as `sh [options] [command_file [argument...]]` — the first
//! operand **is a script path**, and neither bash nor dash reserves a single word in that slot. So
//! a tool name has to be read there without ever taking an invocation a script could have wanted.
//!
//! [`as_operand`] is that rule, and it holds because of one measured fact: **every shebang
//! produces a slashed argv[1]**. A `#!/bin/oslo` script is executed by the kernel as
//! `execve("/bin/oslo", ["/bin/oslo", "./config"])` when run from the current directory, and with
//! the full path when found on `$PATH`. A bare `config` in that slot can therefore only have been
//! typed by a person — which is what leaves the word free to mean something.
//!
//! The second condition does the rest: a file of that name always wins. oslo does not search
//! `$PATH` for a script operand, so when no such file exists the alternative was never "run
//! something else", it was `No such file or directory`. An error becomes useful; nothing that
//! worked changes meaning.

/// One tool.
pub struct Tool {
    /// The part after `oslo-`.
    pub name: &'static str,
    /// One line, for the help.
    pub about: &'static str,
}

/// Every tool oslo knows how to be.
///
/// The single list: the dispatcher and the help both read it, so a tool cannot be reachable and
/// undocumented, or listed and unreachable.
pub const TOOLS: &[Tool] = &[
    Tool {
        name: "macros",
        about: "the aliases, abbreviations, functions and scripts you keep",
    },
    Tool {
        name: "config",
        about: "inspect and edit the Lua configuration",
    },
    Tool {
        name: "profile",
        about: "profiles, the key that pairs two machines, and the sync",
    },
    Tool {
        name: "history",
        about: "search, export and prune the command history",
    },
    #[cfg(feature = "direnv")]
    Tool {
        name: "direnv",
        about: "manage per-directory environments",
    },
    #[cfg(feature = "make")]
    Tool {
        name: "make",
        about: "run a recipe from the project's .make.lua",
    },
    #[cfg(feature = "watch")]
    Tool {
        name: "watch",
        about: "run a command when watched paths change",
    },
    Tool {
        name: "hook",
        about: "list and test the shell hooks",
    },
    // Core rather than a feature: the parser it walks is linked whatever else is turned on, so
    // there is no dependency to leave out and a `#[cfg]` seam would buy almost no bytes.
    Tool {
        name: "fmt",
        about: "lay out a shell script the way the parser reads it",
    },
    // The client library and where to reach this shell. A *tool* rather than a builtin because
    // every caller is another program — a sibling's Lua, a script — reaching oslo through
    // `io.popen`, where a builtin does not exist.
    Tool {
        name: "lua-api",
        about: "the Lua client library another program loads",
    },
    #[cfg(feature = "plugin")]
    Tool {
        name: "plugin",
        about: "install, list and remove shell plugins",
    },
    // **A tool as well as a builtin, and the two are not redundant.** The builtin is what a person
    // types at an oslo prompt; this is what anything *else* asks — a prompt segment, a status bar,
    // a script — and those reach oslo through `io.popen` or `sh -c`, where a builtin does not
    // exist and the name resolves to whatever is on `$PATH` instead.
    #[cfg(feature = "scratch")]
    Tool {
        name: "scratch",
        about: "list the named sessions, or go into one",
    },
    // The widgets, for everything that is not an oslo prompt. Same body as the `ui` builtin: a
    // bash script, a Makefile recipe or a `sh -c` reaches a program, and cannot reach a builtin.
    Tool {
        name: "userin",
        about: "ask for something: choose, filter, input, confirm and the rest",
    },
    #[cfg(feature = "secrets")]
    Tool {
        name: "secret",
        about: "values kept encrypted, handed out when something asks",
    },
];

/// The tool a first *operand* names, if it safely names one.
///
/// This is what makes `oslo history` work without taking the operand slot away from scripts. Three
/// conditions, and all of them must hold:
///
/// 1. **No `/` in it.** Every shebang produces a slashed path — `./config` when run from the
///    current directory, the full path when found on `$PATH` — so a bare word can only have been
///    typed by a person. Verified against the kernel rather than assumed.
/// 2. **No file of that name exists.** A script always wins. `oslo config` next to a `./config`
///    runs the script, exactly as it does today.
/// 3. It is one of [`TOOLS`].
///
/// Condition 2 is what makes this safe rather than merely unlikely to bite. oslo does not search
/// `$PATH` for a script operand, so if no such file exists the alternative was not "run something
/// else" — it was `oslo: config: No such file or directory`. Nothing that works today can change
/// meaning; only an error becomes useful.
///
/// The escape hatches, for the day somebody has a script named `hook`: `oslo ./hook` and
/// `oslo -- hook` both say "this is a path" and are honoured.
pub fn as_operand(word: &str) -> Option<&'static Tool> {
    // **A regular file, not merely something of that name.** The operand slot means *a script to
    // run*, and a directory is never one — so `oslo config` in a project that happens to hold a
    // `config/` directory was answering `config: No such file or directory` rather than opening the
    // configuration. `make/`, `config/` and `history/` are ordinary directory names, and each of
    // them silently took a tool away from anybody standing in such a project. A real `./config`
    // script still wins, which is the rule this was always for.
    as_operand_when(word, |word| std::path::Path::new(word).is_file())
}

/// [`as_operand`], with the filesystem passed in.
///
/// Split out **so the tests never touch the process's working directory.** Proving "a real file
/// wins" by `chdir`-ing into a temporary directory would work exactly once: `cwd` is process-wide,
/// libtest runs tests on threads, and a sibling resolving a relative path mid-`chdir` sees the
/// wrong one. The same in-process global-state trap as `environ`, which has caused three flaky
/// tests in this codebase already.
fn as_operand_when(word: &str, exists: impl Fn(&str) -> bool) -> Option<&'static Tool> {
    if word.contains('/') {
        return None;
    }
    if exists(word) {
        return None;
    }
    from_name(word)
}

/// The tool with exactly this name.
pub fn from_name(name: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|tool| tool.name == name)
}

/// Run a tool. The status is the process's.
///
/// Every tool answers `--help` and nothing else yet — the sub-subcommands are still to be written.
/// A stub that *says* it is a stub beats one that accepts arguments and ignores them: this way a
/// script built against a tool that does not do its job yet fails now rather than silently.
pub fn run(tool: &'static Tool, args: &[String]) -> i32 {
    if tool.name == "history" {
        return crate::cli::history::run(args);
    }
    if tool.name == "macros" {
        return crate::cli::macros::run(args);
    }
    if tool.name == "config" {
        return crate::cli::config::run(args);
    }
    if tool.name == "hook" {
        return crate::cli::hook::run(args);
    }
    if tool.name == "fmt" {
        return crate::cli::fmt::run(args);
    }
    if tool.name == "lua-api" {
        return crate::cli::live::run(args);
    }
    #[cfg(feature = "direnv")]
    if tool.name == "direnv" {
        return crate::cli::direnv::run(args);
    }
    #[cfg(feature = "make")]
    if tool.name == "make" {
        return crate::cli::make::run(args);
    }
    #[cfg(feature = "watch")]
    if tool.name == "watch" {
        return crate::cli::watch::run(args);
    }
    if tool.name == "profile" {
        return crate::cli::profile::run(args);
    }
    #[cfg(feature = "plugin")]
    if tool.name == "plugin" {
        return crate::cli::plugin::run(args);
    }
    // The same code the builtin runs, so the two answers cannot disagree about what is running.
    // `args` here has no command name in front of it, which the builtin's does.
    #[cfg(feature = "scratch")]
    if tool.name == "scratch" {
        // The overview page is this crate's, like `history`'s, so the two read alike. Everything
        // else is the shell's, and is the same code the builtin runs.
        if args.iter().any(|a| a == "--help" || a == "-h") {
            print!(
                "{}",
                crate::cli::scratch::text(crate::cli::help::Paint::detect())
            );
            return 0;
        }
        return oslo::env::builtins::scratch_tool(args);
    }
    // Its own `--help` too: the widget list is the help, and this tool has no page of its own to
    // print instead.
    if tool.name == "userin" {
        return oslo::env::builtins::userin_tool(args);
    }
    #[cfg(feature = "secrets")]
    if tool.name == "secret" {
        return crate::cli::secret::run(args);
    }
    let paint = crate::cli::help::Paint::detect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{}", help(tool, paint));
        return 0;
    }
    if let Some(unknown) = args.first() {
        eprint!("{}", help(tool, paint));
        eprintln!("\noslo {}: {unknown:?}: no such subcommand", tool.name);
        return 2;
    }
    print!("{}", help(tool, paint));
    0
}

/// A tool's own help.
fn help(tool: &'static Tool, paint: crate::cli::help::Paint) -> String {
    format!(
        "{}\n  {} {} {}\n\n{}\n  {}\n",
        paint.head("USAGE"),
        paint.key("oslo"),
        paint.key(tool.name),
        paint.slot("<subcommand> [...]"),
        paint.head("SUBCOMMANDS"),
        paint.dim("none yet — this tool is not implemented"),
    )
}

#[cfg(test)]
#[path = "tools/tests.rs"]
mod tests;
