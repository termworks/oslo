//! Shell options: the set behind `set -e`, `set -o pipefail` and `$-`.
//!
//! One table ([`ALL`]) is the single source of truth for every way an option can be named — the
//! single letter `set -e` uses, the long name `set -o errexit` uses, and the character `$-`
//! reports. A second list anywhere else is how a shell ends up accepting `set -o errexit` while
//! `$-` still says the option is off.
//!
//! # Querying an option
//!
//! Storage is a bitset, so a query is a bit test and callers can take a copy. The intended entry
//! points are the accessors on [`Environment`], not this type:
//!
//! ```
//! # use oslo_shell::env::Environment;
//! # use oslo_shell::env::options::ShellOption;
//! # let mut env = Environment::new();
//! env.set_option(ShellOption::ErrExit, true);
//! assert!(env.errexit()); // the named accessor, for the options with behaviour
//! assert!(env.option(ShellOption::ErrExit)); // the general form, for the rest
//! ```
//!
//! [`Environment`]: crate::env::scope::Environment

mod apply;

pub use apply::{SetArgs, SetError, SetListing, parse_set_args};

/// Every option this shell knows, whether or not it does anything yet.
///
/// Listing an option here is a promise about *naming*, not about behaviour: `set -o pipefail`
/// must be accepted and reported by `$-`/`set -o` even in the release where the pipeline code
/// has not been taught to read it, because a script that turns an option on and gets "invalid
/// option" back is broken in a way that a script whose option is a no-op is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellOption {
    AllExport,
    Notify,
    ErrExit,
    NoGlob,
    HashAll,
    Keyword,
    Monitor,
    NoExec,
    OneCmd,
    NoUnset,
    Verbose,
    XTrace,
    NoClobber,
    IgnoreEof,
    NoLog,
    PipeFail,
    Posix,
    Vi,
    Emacs,
    /// The shell is talking to a person. Set from the invocation, never by `set`.
    Interactive,
    /// The program came from `-c`. Set from the invocation, never by `set`.
    CommandString,
    /// The program is being read from standard input. Set from the invocation, never by `set`.
    StdinInput,
    /// oslo's own line editor is reading this shell's commands: a person at a prompt. Set by the
    /// REPL alone — no letter and no name, so neither `set` nor `$-` can reach it.
    Prompt,
}

/// How an option can be written.
pub struct OptionSpec {
    pub option: ShellOption,
    /// The `set -x` letter, and the character `$-` reports. Some options have only a long name.
    pub letter: Option<char>,
    /// The `set -o` name. `None` for the three invocation flags, which `set` cannot change.
    pub name: Option<&'static str>,
    /// What turning it on does, in one line, for `--help --details`.
    ///
    /// Held here rather than in the help text so that the two cannot drift: an option added to
    /// this table appears in the reference without anybody remembering to write it down twice.
    pub about: &'static str,
    /// Why turning this on does nothing, when it does nothing.
    ///
    /// **An option that is accepted and ignored is worse than one that is refused.** A script
    /// writing `set -o notify` and getting status 0 is entitled to believe the shell will report
    /// finished jobs immediately; it will not. This is the same argument `shopt`'s `Fixed` states
    /// were built on, applied to the other half of the option namespace.
    pub unsupported: Option<&'static str>,
}

impl OptionSpec {
    /// Whether `set` may change this option. False only for the invocation flags.
    pub fn settable(&self) -> bool {
        self.name.is_some()
    }

    /// The spec for an option, by the value.
    pub fn of(option: ShellOption) -> Option<&'static OptionSpec> {
        ALL.iter().find(|s| s.option == option)
    }
}

const fn spec(
    option: ShellOption,
    letter: Option<char>,
    name: Option<&'static str>,
    about: &'static str,
) -> OptionSpec {
    OptionSpec {
        option,
        letter,
        name,
        about,
        unsupported: None,
    }
}

/// An option the shell knows the name of and does not implement.
const fn refused(
    option: ShellOption,
    letter: Option<char>,
    name: Option<&'static str>,
    about: &'static str,
    why: &'static str,
) -> OptionSpec {
    OptionSpec {
        option,
        letter,
        name,
        about,
        unsupported: Some(why),
    }
}

/// Every option, in the order `$-` and `set -o` report them.
///
/// Sorted by letter with the letterless options after the ones that have letters, which is what
/// bash's own ordering amounts to; the three invocation flags come last because `$-` puts them
/// there (`bash -c 'echo $-'` prints `hBc`, not `chB`).
pub const ALL: &[OptionSpec] = &[
    spec(
        ShellOption::AllExport,
        Some('a'),
        Some("allexport"),
        "export every variable an assignment creates",
    ),
    refused(
        ShellOption::Notify,
        Some('b'),
        Some("notify"),
        "report a finished background job as soon as it finishes",
        "a finished background job is reported at the next prompt, not immediately",
    ),
    spec(
        ShellOption::ErrExit,
        Some('e'),
        Some("errexit"),
        "exit as soon as a command fails",
    ),
    spec(
        ShellOption::NoGlob,
        Some('f'),
        Some("noglob"),
        "leave glob patterns alone instead of expanding them",
    ),
    spec(
        ShellOption::HashAll,
        Some('h'),
        Some("hashall"),
        "remember where a command was found, to skip the next $PATH search",
    ),
    refused(
        ShellOption::Keyword,
        Some('k'),
        Some("keyword"),
        "treat an assignment after the command name as an assignment",
        "an assignment after the command name is an argument, not an assignment",
    ),
    spec(
        ShellOption::Monitor,
        Some('m'),
        Some("monitor"),
        "job control: each job in its own process group, with fg and bg",
    ),
    spec(
        ShellOption::NoExec,
        Some('n'),
        Some("noexec"),
        "read and parse commands without running them, to check syntax",
    ),
    refused(
        ShellOption::OneCmd,
        Some('t'),
        Some("onecmd"),
        "exit after reading and running one command",
        "the shell does not exit after one command",
    ),
    spec(
        ShellOption::NoUnset,
        Some('u'),
        Some("nounset"),
        "fail rather than expand an unset variable to nothing",
    ),
    refused(
        ShellOption::Verbose,
        Some('v'),
        Some("verbose"),
        "echo each line of input as it is read",
        "input is not echoed as it is read; `set -x` traces what runs",
    ),
    spec(
        ShellOption::XTrace,
        Some('x'),
        Some("xtrace"),
        "print each command, expanded, before it runs",
    ),
    spec(
        ShellOption::NoClobber,
        Some('C'),
        Some("noclobber"),
        "refuse to overwrite an existing file with `>`",
    ),
    spec(
        ShellOption::IgnoreEof,
        None,
        Some("ignoreeof"),
        "do not let Ctrl-D end an interactive shell",
    ),
    refused(
        ShellOption::NoLog,
        None,
        Some("nolog"),
        "keep function definitions out of the history",
        "history records function definitions like anything else",
    ),
    spec(
        ShellOption::PipeFail,
        None,
        Some("pipefail"),
        "a pipeline fails if any command in it failed, not just the last",
    ),
    spec(
        ShellOption::Posix,
        None,
        Some("posix"),
        "follow POSIX in the four places bash's default differs",
    ),
    refused(
        ShellOption::Vi,
        None,
        Some("vi"),
        "vi key bindings while editing a line",
        "the editing mode is `oslo.vi.enabled` or `--no-vi`, fixed when the editor is built",
    ),
    refused(
        ShellOption::Emacs,
        None,
        Some("emacs"),
        "emacs key bindings while editing a line",
        "the editing mode is `oslo.vi.enabled` or `--no-vi`, fixed when the editor is built",
    ),
    spec(
        ShellOption::Interactive,
        Some('i'),
        None,
        "the shell is interactive (from the invocation; `set` cannot change it)",
    ),
    spec(
        ShellOption::CommandString,
        Some('c'),
        None,
        "the commands came from `-c` (from the invocation)",
    ),
    spec(
        ShellOption::StdinInput,
        Some('s'),
        None,
        "commands are read from standard input (from the invocation)",
    ),
    spec(
        ShellOption::Prompt,
        None,
        None,
        "oslo's line editor is reading the commands (set by the prompt itself)",
    ),
];

impl ShellOption {
    /// The option `set -LETTER` names, if any. Rejects the invocation flags: `set -i` is an
    /// error in bash, and accepting it would let a script claim the shell is interactive.
    pub fn from_letter(letter: char) -> Option<Self> {
        ALL.iter()
            .find(|s| s.settable() && s.letter == Some(letter))
            .map(|s| s.option)
    }

    /// The option `set -o NAME` names, if any.
    pub fn from_name(name: &str) -> Option<Self> {
        ALL.iter().find(|s| s.name == Some(name)).map(|s| s.option)
    }

    /// This option's `set -o` name, or `None` for the invocation flags.
    pub fn name(self) -> Option<&'static str> {
        self.spec().name
    }

    /// This option's letter, or `None` for the options only `set -o` can reach.
    pub fn letter(self) -> Option<char> {
        self.spec().letter
    }

    fn spec(self) -> &'static OptionSpec {
        ALL.iter()
            .find(|s| s.option == self)
            .expect("every ShellOption variant is in ALL")
    }

    /// Position in [`ALL`], which is also this option's bit in [`ShellOptions`].
    fn bit(self) -> u32 {
        ALL.iter()
            .position(|s| s.option == self)
            .expect("every ShellOption variant is in ALL") as u32
    }
}

/// The options currently in force, one bit each.
///
/// `Copy`, deliberately: a caller deep in expansion or redirection wants to ask a question about
/// the options without holding a borrow on the whole [`Environment`].
///
/// [`Environment`]: crate::env::scope::Environment
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellOptions {
    bits: u32,
}

/// **Not all-zero.** `hashall` is on in every shell that has it — bash reports `h` in `$-` from the
/// first command — so the default state has to say so, or `set +o hashall` would be turning off
/// something the listing already claimed was off while the shell went on hashing.
impl Default for ShellOptions {
    fn default() -> Self {
        let mut options = ShellOptions { bits: 0 };
        options.set(ShellOption::HashAll, true);
        options
    }
}

impl ShellOptions {
    pub fn is_set(self, option: ShellOption) -> bool {
        self.bits & (1 << option.bit()) != 0
    }

    pub fn set(&mut self, option: ShellOption, on: bool) {
        if on {
            self.bits |= 1 << option.bit();
        } else {
            self.bits &= !(1 << option.bit());
        }
    }

    /// The value of `$-`: the letter of every set option that has one, in [`ALL`] order.
    ///
    /// Options with no letter (`pipefail`, `posix`, …) are invisible here — that is what `$-`
    /// means, and it is why `set -o` exists.
    pub fn flag_string(self) -> String {
        ALL.iter()
            .filter(|s| self.is_set(s.option))
            .filter_map(|s| s.letter)
            .collect()
    }

    /// `set -o` with no name: one line per settable option, `name` then a tab then `on`/`off`.
    ///
    /// Padded to bash's column width so the two shells' output can be compared directly.
    pub fn long_listing(self) -> String {
        self.listing(|name, on| format!("{:<15}\t{}\n", name, if on { "on" } else { "off" }))
    }

    /// `set +o`: the same states written as the commands that would restore them.
    ///
    /// This is the form POSIX requires to be re-inputtable, so `save=$(set +o)` … `eval "$save"`
    /// round-trips.
    pub fn reinputtable_listing(self) -> String {
        self.listing(|name, on| format!("set {}o {}\n", if on { '-' } else { '+' }, name))
    }

    fn listing(self, line: impl Fn(&str, bool) -> String) -> String {
        ALL.iter()
            .filter_map(|s| s.name.map(|name| (name, self.is_set(s.option))))
            .map(|(name, on)| line(name, on))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{ALL, ShellOption, ShellOptions};

    /// The bitset indexes [`ALL`], so a duplicate entry would silently alias two options.
    #[test]
    fn every_option_appears_exactly_once() {
        for (i, spec) in ALL.iter().enumerate() {
            assert_eq!(
                ALL.iter().position(|s| s.option == spec.option),
                Some(i),
                "{:?} is listed twice",
                spec.option
            );
        }
        assert!(ALL.len() <= 32, "the bitset is a u32");
    }

    #[test]
    fn letters_and_names_are_unique() {
        for (i, spec) in ALL.iter().enumerate() {
            for other in &ALL[i + 1..] {
                assert!(
                    spec.letter.is_none() || spec.letter != other.letter,
                    "{:?} shares a letter with {:?}",
                    spec.option,
                    other.option
                );
                assert!(
                    spec.name.is_none() || spec.name != other.name,
                    "{:?} shares a name with {:?}",
                    spec.option,
                    other.option
                );
            }
        }
    }

    #[test]
    fn setting_one_option_leaves_the_others_alone() {
        let mut opts = ShellOptions::default();
        opts.set(ShellOption::ErrExit, true);
        opts.set(ShellOption::PipeFail, true);
        assert!(opts.is_set(ShellOption::ErrExit) && opts.is_set(ShellOption::PipeFail));
        assert!(!opts.is_set(ShellOption::NoUnset));
        opts.set(ShellOption::ErrExit, false);
        assert!(!opts.is_set(ShellOption::ErrExit));
        assert!(opts.is_set(ShellOption::PipeFail));
    }

    /// `set -i` must not be a way for a script to claim the shell is interactive.
    #[test]
    fn invocation_flags_have_no_set_spelling() {
        assert_eq!(ShellOption::from_letter('i'), None);
        assert_eq!(ShellOption::from_letter('c'), None);
        assert_eq!(ShellOption::from_name("interactive"), None);
        assert_eq!(ShellOption::from_letter('e'), Some(ShellOption::ErrExit));
        assert_eq!(
            ShellOption::from_name("pipefail"),
            Some(ShellOption::PipeFail)
        );
    }

    #[test]
    fn flag_string_is_letters_only_in_table_order() {
        let mut opts = ShellOptions::default();
        opts.set(ShellOption::XTrace, true);
        opts.set(ShellOption::ErrExit, true);
        opts.set(ShellOption::NoClobber, true);
        // pipefail has no letter, so it cannot show up in `$-`.
        opts.set(ShellOption::PipeFail, true);
        opts.set(ShellOption::CommandString, true);
        // `h` is in the default set, so it is here without being asked for — see
        // `scope::options::tests::options_start_at_their_defaults_and_survive_a_round_trip`.
        assert_eq!(opts.flag_string(), "ehxCc");
    }

    #[test]
    fn reinputtable_listing_round_trips_through_the_parser() {
        let mut opts = ShellOptions::default();
        opts.set(ShellOption::NoUnset, true);
        let listing = opts.reinputtable_listing();
        assert!(listing.contains("set -o nounset\n"));
        assert!(listing.contains("set +o errexit\n"));
        assert!(!listing.contains("interactive"));
    }

    #[test]
    fn long_listing_names_every_settable_option() {
        let listing = ShellOptions::default().long_listing();
        assert_eq!(
            listing.lines().count(),
            ALL.iter().filter(|s| s.settable()).count()
        );
        assert!(listing.contains("errexit        \toff"));
    }
}
