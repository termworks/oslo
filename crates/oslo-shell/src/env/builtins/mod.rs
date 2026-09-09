//! Shell builtins.
//!
//! Grouped by what they act on rather than listed alphabetically, so a change to (say)
//! directory handling touches one file. [`register_default_builtins`] is the single place
//! every builtin is wired up, and [`Environment::builtin_names`] reads back from it — adding
//! one here is enough to make it visible to completion and to `type`.
//!
//! The builtins with enough behaviour of their own to argue about — `exec`, `command`, `builtin`,
//! `getopts`, `let` — each get a file named for them, because the alternative is one file that
//! grows without limit and that nobody can review a change to.
//!
//! [`Environment::builtin_names`]: crate::env::Environment::builtin_names

mod abbr;
pub(crate) mod arrays;
mod builtin;
mod caller;
mod chain;
mod colon;
mod command;
mod conditionals;
mod control;
mod copy;
mod declare;
mod directories;
#[cfg(feature = "direnv")]
mod direnv;
mod edit;
mod emit;
mod exec;
mod getopts;
mod hash;
mod io;
mod jobs;
mod keep;
mod r#let;
mod locate;
#[cfg(feature = "make")]
mod make;
mod mapfile;
mod mark;
#[cfg(feature = "math")]
mod math;
mod messages;
mod nav;
mod process;
mod remove;
#[cfg(feature = "scratch")]
mod scratch;
mod shopt;
pub(crate) mod spawn;
mod status;
mod suspend;
#[cfg(feature = "scratch")]
pub use scratch::tool as scratch_tool;
mod times;
mod ui;
/// `oslo userin …` — the same widgets as the `ui` builtin, for a shell that is not oslo.
pub use ui::tool as userin_tool;
mod ulimit;
mod variables;

pub use builtin::builtin_builtin;
pub use caller::builtin_caller;
pub use chain::builtin_chain;
pub use colon::builtin_colon;
pub use command::builtin_command;
pub use conditionals::{builtin_extended_test, builtin_test};
pub use control::{
    builtin_break, builtin_continue, builtin_eval, builtin_exit, builtin_return, builtin_source,
    builtin_type,
};
pub use declare::builtin_declare;
pub use directories::{builtin_cd, builtin_dirs, builtin_popd, builtin_pushd, builtin_pwd};
pub use exec::builtin_exec;
pub use getopts::builtin_getopts;
pub use hash::builtin_hash;
pub use io::{builtin_echo, builtin_printf, builtin_read, shell_quote};
pub use jobs::{builtin_bg, builtin_disown, builtin_fg, builtin_jobs, builtin_wait};
pub use r#let::builtin_let;
pub use locate::{builtin_whereis, builtin_which};
/// `which` and `whereis`, which only a shell can answer: an alias, a builtin and a stored macro are
/// all invisible to the programs by those names.
#[cfg(feature = "make")]
pub use make::builtin_make;
pub use mapfile::builtin_mapfile;
pub use messages::builtin_messages;
pub use process::{
    builtin_kill, builtin_trap, builtin_umask, pending_signal, run_debug_trap, run_err_trap,
    run_exit_trap, run_pending_traps,
};
pub use shopt::builtin_shopt;
pub use status::builtin_status;
pub use suspend::builtin_suspend;
pub use times::builtin_times;
pub use ulimit::builtin_ulimit;
pub use variables::{
    builtin_alias, builtin_export, builtin_local, builtin_readonly, builtin_set, builtin_shift,
    builtin_unalias, builtin_unset,
};

/// Whether a command's redirections must outlive it — the `exec > "$log" 2>&1` form.
///
/// A builtin is never handed its own redirections, so this is the one thing about `exec` the
/// dispatcher has to know: [`crate::exec::simple`] must build a *non-restoring*
/// [`crate::exec::redirect::RedirectGuard`] (`RedirectGuard::for_exec`) when this returns true,
/// and the ordinary restoring one otherwise.
pub use exec::makes_redirections_permanent as exec_makes_redirections_permanent;

/// The shell's remembered command locations, for whoever resolves `PATH` lookups.
///
/// [`hash_lookup`] is the one a command-resolution path wants: it answers from the table, falls
/// back to a `PATH` search, and remembers the result. [`hash_remember`] and [`hash_recall`] are
/// its two halves, for a caller that has already searched. `hash` on its own reports the table
/// and `hash -r` clears it.
///
/// `crate::exec::simple::external::look_up_command` resolves every bare command word through
/// [`hash_lookup`], which is what makes the table describe the session rather than only what an
/// explicit `hash name` put there. [`hash_forget_all`] is the invalidation side: `hash -r` and an
/// assignment to `PATH`.
pub use hash::forget_all as hash_forget_all;
pub use hash::lookup as hash_lookup;
pub use hash::note_hashall;
pub use hash::recall as hash_recall;
pub use hash::remember as hash_remember;

/// Put the directory a session began in into the ring `cd -N` counts back through.
///
/// The interactive loop calls this once, before the first prompt, and nothing else calls it at all.
/// `cd -` reads `$OLDPWD` and so works from the very first move; `cd -N` reads the ring, which is
/// only appended to by a *successful* change of directory. Without the starting directory in it the
/// two disagree for exactly one command — `cd -1` answers "no such entry" in a shell where `cd -`
/// works — and the shell's own documentation says they are the same thing.
///
/// Seeding it from [`crate::env::builtins::builtin_cd`]'s helper instead would seed every script's
/// ring as well, and a script has no wandering to count back through.
pub use directories::ring::record as remember_directory;

use crate::env::scope::Environment;

pub fn register_default_builtins(env: &mut Environment) {
    env.register_custom_builtin("cd", builtin_cd);
    env.register_custom_builtin("pwd", builtin_pwd);
    env.register_custom_builtin("echo", builtin_echo);
    env.register_custom_builtin("printf", builtin_printf);
    env.register_custom_builtin("export", builtin_export);
    env.register_custom_builtin("unset", builtin_unset);
    env.register_custom_builtin("set", builtin_set);
    env.register_custom_builtin("shift", builtin_shift);
    env.register_custom_builtin("exit", builtin_exit);
    env.register_custom_builtin("break", builtin_break);
    env.register_custom_builtin("continue", builtin_continue);
    env.register_custom_builtin("return", builtin_return);
    env.register_custom_builtin("alias", builtin_alias);
    env.register_custom_builtin("unalias", builtin_unalias);
    env.register_custom_builtin("true", |_, _| Ok(0));
    env.register_custom_builtin("false", |_, _| Ok(1));
    env.register_custom_builtin("type", builtin_type);
    // The programs by these names read `$PATH` and nothing else, so they are wrong about every
    // alias, builtin and stored macro in this shell. `/usr/bin/which` still answers the old way.
    env.register_custom_builtin("which", builtin_which);
    env.register_custom_builtin("whereis", builtin_whereis);
    // `make` claims the word only at a prompt in a directory a `.make.lua` governs; everywhere
    // else it hands over to the program. See `make.rs`.
    #[cfg(feature = "make")]
    env.register_custom_builtin("make", builtin_make);
    env.register_custom_builtin("eval", builtin_eval);
    env.register_custom_builtin(".", builtin_source);
    env.register_custom_builtin("source", builtin_source);
    env.register_custom_builtin("read", builtin_read);
    env.register_custom_builtin("local", builtin_local);
    env.register_custom_builtin("pushd", builtin_pushd);
    env.register_custom_builtin("popd", builtin_popd);
    env.register_custom_builtin("dirs", builtin_dirs);
    env.register_custom_builtin("readonly", builtin_readonly);
    env.register_custom_builtin("test", builtin_test);

    env.register_custom_builtin("[", builtin_test);
    env.register_custom_builtin("[[", builtin_extended_test);
    env.register_custom_builtin("emit", emit::builtin_emit);
    env.register_custom_builtin("funced", edit::builtin_funced);
    env.register_custom_builtin("vared", edit::builtin_vared);
    env.register_custom_builtin("trap", builtin_trap);
    env.register_custom_builtin("umask", builtin_umask);
    env.register_custom_builtin("wait", builtin_wait);
    env.register_custom_builtin("kill", builtin_kill);

    // `OSC 52` to the terminal, so it works over SSH where a clipboard helper cannot.
    env.register_custom_builtin("copy", copy::builtin_copy);
    // The prefix that gives `copy --last` something to copy. Opt-in, one command at a time: a
    // shell that kept every command's output would pay for all of them to answer about one.
    env.register_custom_builtin("keep", keep::builtin_keep);
    env.register_custom_builtin("abbr", abbr::builtin_abbr);
    env.register_custom_builtin("ui", ui::builtin_ui);
    env.register_custom_builtin("nav", nav::builtin_nav);
    // `mark` — remember this directory as `@name`, and forget it by typing `mark` again. The other
    // half of the `@name` expansion: without it every name had to be declared in a config first.
    //
    // **A word, not the `@` sigil.** A leading symbol is reserved: `@` at the start of a line is
    // being kept for something else, so nothing here may claim that position.
    env.register_custom_builtin("mark", mark::builtin_mark);
    // The finder the key opens, for a prompt counting tabs and for a name already known.
    #[cfg(feature = "scratch")]
    env.register_custom_builtin("scratch", scratch::builtin_scratch);
    // The directory ring: where you have been. Walking it is `cd -` and `cd -N`, so the only
    // builtin left is the one that shows you the numbers those take. Separate from `pushd`/`popd`,
    // which are explicit and which scripts rely on.
    env.register_custom_builtin("dirh", directories::ring::builtin_dirh);
    #[cfg(feature = "direnv")]
    env.register_custom_builtin("direnv", direnv::builtin_direnv);
    // A script's own arguments, parsed from the comments that declare them. A builtin rather than
    // the `eval "$(argc …)"` line bash needs, because the parser is linked in — see `crate::argc`.
    #[cfg(feature = "math")]
    env.register_custom_builtin("math", math::builtin_math);

    #[cfg(feature = "argc")]
    env.register_custom_builtin("argc", crate::argc::builtin_argc);

    // Job control. `wait` is registered above but belongs with these: all five read one table.
    env.register_custom_builtin("jobs", builtin_jobs);
    env.register_custom_builtin("fg", builtin_fg);
    env.register_custom_builtin("bg", builtin_bg);
    env.register_custom_builtin("disown", builtin_disown);

    // POSIX's null command. Registered like any other builtin rather than special-cased in the
    // parser, because `:` really is one — `while :; do` and `: ${x:=default}` are ordinary
    // command invocations that happen to do nothing with their arguments.
    env.register_custom_builtin(":", builtin_colon);

    env.register_custom_builtin("exec", builtin_exec);
    env.register_custom_builtin("command", builtin_command);
    env.register_custom_builtin("builtin", builtin_builtin);
    env.register_custom_builtin("getopts", builtin_getopts);
    env.register_custom_builtin("let", builtin_let);
    env.register_custom_builtin("hash", builtin_hash);
    env.register_custom_builtin("times", builtin_times);
    env.register_custom_builtin("ulimit", builtin_ulimit);

    // `shopt` is a namespace of its own, not an alias for `set -o`: see the module docs. It is
    // also the only way to reach `exec::simple::set_autocd`, which had no caller before it.
    // `rm`. A builtin that shadows `/bin/rm` for every script on the machine, so its extensions
    // are confined to an interactive shell and an option it does not know is handed to the real
    // `rm` — see the module docs, which are the argument for it being safe to register at all.
    env.register_custom_builtin("rm", remove::builtin_rm);

    env.register_custom_builtin("shopt", builtin_shopt);
    env.register_custom_builtin("caller", builtin_caller);
    // `chain` — what each link of the last `a && b` did. See its module docs: the shell already
    // computed this and dropped it, and `$PIPESTATUS` only answers one level down.
    env.register_custom_builtin("chain", builtin_chain);
    env.register_custom_builtin("status", builtin_status);
    // `messages` — what this session said after it has scrolled off. A builtin rather than a tool
    // because the buffer is this process's memory; see its module docs.
    env.register_custom_builtin("messages", builtin_messages);
    env.register_custom_builtin("suspend", builtin_suspend);

    // One builtin, two names, exactly as in bash — `readarray` is the spelling that says what it
    // does, `mapfile` is the one scripts were written against.
    env.register_custom_builtin("mapfile", builtin_mapfile);
    env.register_custom_builtin("readarray", builtin_mapfile);

    // Two names for one builtin. Registering them is also what makes the `is_declaration` branch
    // in `exec::simple` mean something: it exists so that `declare FOO=bar` reaches the builtin
    // instead of being applied behind its back.
    env.register_custom_builtin("declare", builtin_declare);
    env.register_custom_builtin("typeset", builtin_declare);
}

#[cfg(test)]
mod tests {
    use crate::env::Environment;

    /// Every builtin this round added has to be reachable by name, or it may as well not exist:
    /// dispatch goes through the registry, and so do `type` and completion.
    #[test]
    fn every_new_builtin_is_registered() {
        let env = Environment::new();
        for name in [
            ":",
            "exec",
            "command",
            "builtin",
            "getopts",
            "let",
            "hash",
            "times",
            "ulimit",
            "declare",
            "typeset",
            "shopt",
            "mapfile",
            "readarray",
            "caller",
            "status",
            "suspend",
            "rm",
            "chain",
            "messages",
        ] {
            assert!(env.is_builtin(name), "{name} is not registered");
            assert!(
                env.get_builtin(name).is_some(),
                "{name} has no implementation in the registry"
            );
        }
    }
}
