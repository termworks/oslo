//! How the rest of the shell asks about `set -e`, `set -u`, `set -o pipefail` and friends.
//!
//! An `impl` block of its own file: the accessors are the API three separate rounds of work hang
//! off (errexit in the evaluator, nounset in expansion, noclobber in redirection, pipefail in the
//! pipeline), and they are cheap enough — a bit test on a `Copy` bitset — that no caller needs to
//! cache the answer.
//!
//! # Which accessor to use
//!
//! * the named ones ([`Environment::errexit`], [`Environment::nounset`], …) for the options that
//!   have behaviour attached, because a reader of the call site should not have to know what
//!   `ShellOption::NoClobber` is;
//! * [`Environment::option`] for anything else, including options oslo only stores.
//!
//! Nothing here *acts* on an option. Storage and behaviour are deliberately separate: `set -x`
//! must be accepted, reported by `$-` and listed by `set -o` whether or not the tracing code
//! exists yet.

use super::Environment;
use crate::env::options::{ShellOption, ShellOptions};

impl Environment {
    /// Every option currently in force. `Copy`, so this does not borrow the environment.
    pub fn options(&self) -> ShellOptions {
        self.options
    }

    /// Replace the whole option set, e.g. when restoring it after a subshell-like construct.
    pub fn set_options(&mut self, options: ShellOptions) {
        self.options = options;
    }

    /// Whether `option` is on.
    pub fn option(&self, option: ShellOption) -> bool {
        self.options.is_set(option)
    }

    /// Turn `option` on or off. The only writer besides [`Environment::set_options`].
    pub fn set_option(&mut self, option: ShellOption, on: bool) {
        self.options.set(option, on);
    }

    /// `set -e`: a command that fails ends the shell.
    pub fn errexit(&self) -> bool {
        self.option(ShellOption::ErrExit)
    }

    /// `set -u`: expanding an unset parameter is an error.
    pub fn nounset(&self) -> bool {
        self.option(ShellOption::NoUnset)
    }

    /// `set -x`: print each expanded command to stderr, prefixed with `$PS4`.
    pub fn xtrace(&self) -> bool {
        self.option(ShellOption::XTrace)
    }

    /// `set -v`: echo each line of input as it is read.
    pub fn verbose(&self) -> bool {
        self.option(ShellOption::Verbose)
    }

    /// `set -n`: read and parse commands but do not run them — what `sh -n script` is for.
    ///
    /// This accessor existed with no callers, so the option appeared in `set -o` and did nothing:
    /// `oslo -n -c 'echo x'` printed `x`, where bash and dash print nothing. That is worse than an
    /// unimplemented option, because `sh -n` is how packaging validates maintainer scripts, and
    /// running one that was only meant to be parsed is a security problem rather than a missing
    /// feature.
    ///
    /// Always false for an interactive shell. POSIX says the option "shall be ignored" there, and
    /// the reason is practical: a `set -n` typed at a prompt would otherwise leave a session that
    /// reads every later line and runs none of them, with no way to type its way out.
    pub fn noexec(&self) -> bool {
        self.option(ShellOption::NoExec) && !self.interactive()
    }

    /// `set -f`: do not expand pathnames.
    pub fn noglob(&self) -> bool {
        self.option(ShellOption::NoGlob)
    }

    /// `set -C`: `>` refuses to truncate an existing file; only `>|` may.
    pub fn noclobber(&self) -> bool {
        self.option(ShellOption::NoClobber)
    }

    /// `set -o pipefail`: a pipeline reports the rightmost non-zero stage, not the last stage.
    pub fn pipefail(&self) -> bool {
        self.option(ShellOption::PipeFail)
    }

    /// `set -a`: every assignment is exported.
    pub fn allexport(&self) -> bool {
        self.option(ShellOption::AllExport)
    }

    /// `set -m`: run jobs in their own process groups with job-control notification.
    pub fn monitor(&self) -> bool {
        self.option(ShellOption::Monitor)
    }

    /// `--posix` / `set -o posix`: follow POSIX where bash's own default differs.
    ///
    /// **This is the single source of truth for POSIX mode.** It used to be two: a process-global
    /// `AtomicBool` in `exec::simple` with only `#[cfg(test)]` writers, *and* this option, which
    /// the `set -o` table accepted and nothing ever read. The command line and `set -o posix`
    /// both land here now, so the two can no longer disagree — and a forked subshell inherits the
    /// mode with the rest of its environment rather than out of a static.
    ///
    /// Four things read it:
    ///
    /// - command search puts a special builtin ahead of a function (POSIX 2.9.1.1)
    /// - an error in a special builtin ends a non-interactive shell (POSIX 2.8.1)
    ///   — both in `crate::exec::simple::posix`
    /// - `$?` beside a command substitution in the same word keeps the *previous command's*
    ///   status rather than the substitution's, which is what bash 5.3 changed to
    ///   (`crate::expand::word`)
    /// - `trap` lists conditions as `INT` rather than `SIGINT`
    ///   (`crate::env::builtins::process::trap`)
    pub fn posix(&self) -> bool {
        self.option(ShellOption::Posix)
    }

    /// Whether this shell is interactive. Set from the invocation; `set` cannot change it.
    pub fn interactive(&self) -> bool {
        self.option(ShellOption::Interactive)
    }

    /// Whether oslo's own line editor is reading this shell's commands. Set by the REPL alone.
    pub fn at_prompt(&self) -> bool {
        self.option(ShellOption::Prompt)
    }

    /// The value of `$-`.
    pub fn option_flags(&self) -> String {
        self.options.flag_string()
    }
}

#[cfg(test)]
mod tests {
    use crate::env::Environment;
    use crate::env::options::ShellOption;

    /// `h` is not an exception to "start off": bash has `hashall` on from the first prompt, and
    /// `$-` says so — `bash -c 'echo $-'` answers `hBc`. Everything else starts clear.
    #[test]
    fn options_start_at_their_defaults_and_survive_a_round_trip() {
        let mut env = Environment::new();
        assert!(!env.errexit() && !env.nounset() && !env.pipefail());
        assert_eq!(env.option_flags(), "h");

        env.set_option(ShellOption::ErrExit, true);
        assert!(env.errexit());
        assert_eq!(env.option_flags(), "eh");

        let saved = env.options();
        env.set_option(ShellOption::ErrExit, false);
        assert!(!env.errexit());
        env.set_options(saved);
        assert!(env.errexit());
    }

    /// A subshell inherits the options: `set -e; (false)` has to see errexit in the child.
    #[test]
    fn entering_a_subshell_keeps_the_options() {
        let mut env = Environment::new();
        env.set_option(ShellOption::NoUnset, true);
        env.enter_subshell();
        assert!(env.nounset());
    }

    /// POSIX mode has exactly one home, and `set -o posix` reaches it. The option had no reader
    /// at all before, which is what made `--posix` unimplementable and the special-builtin rule
    /// in `exec::simple` dead code.
    #[test]
    fn posix_mode_is_an_ordinary_option() {
        let mut env = Environment::new();
        assert!(!env.posix());
        env.set_option(ShellOption::Posix, true);
        assert!(env.posix());
        assert!(env.option(ShellOption::Posix));
        // …and it survives into a subshell, where a static could not have been undone on the way
        // back out.
        env.enter_subshell();
        assert!(env.posix());
    }
}
