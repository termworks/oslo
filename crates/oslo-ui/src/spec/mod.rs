//! Completion specs: what to suggest after a command name.
//!
//! [`SpecRegistry`] holds one [`CommandSpec`] per known command; the per-command data lives
//! in [`definitions`], and ranking uses [`FrecencyTracker`].
//!
//! # The shape is carapace's
//!
//! A command is a name, some flags, some subcommands, and â the part oslo lacked until now â a
//! declared answer for **each argument position**. That is the model
//! [carapace-spec](https://github.com/carapace-sh/carapace-spec) settled on, and it is what makes
//! the difference between `git checkout <Tab>` offering files and offering branches: the shape of
//! the command is data, and only the values need computing.
//!
//! What a position completes to is an [`Action`], which is deliberately *not* resolved here â see
//! [`action`] for why a value list stays text until the Tab key is pressed.
//!
//! # Owned strings, since a config can declare one
//!
//! Every field here used to be `&'static str`, which is the natural shape for four hand-written
//! definitions compiled into the binary and an impossible one for a spec built at runtime from Lua.
//! That single word was the whole reason a plugin's only route to completion was `for_command` â a
//! function that has to re-implement subcommand matching, flag parsing and descriptions by hand.
//!
//! The cost is an allocation per string at *build* time and a pointer chase at *read* time. Both
//! were measured on a Tab against `git comm`, which is the deepest walk of this data the shell does:
//! see `docs/features/completion-and-matching.md`.

pub mod action;
pub mod custom;
pub mod definitions;
pub mod flag;
pub mod frecency;
pub mod resolve;
/// What the machine knows — hosts, pids, users, mounts — offered as completions.
pub mod sources;
pub mod vars;

pub use action::{Action, Query};
pub use frecency::FrecencyTracker;

use std::collections::HashMap;
use std::rc::Rc;

/// Whether a flag takes an argument, and whether it insists.
///
/// carapace spells these on the flag itself â `-v=` takes one, `-o, --optarg?` takes one or none â
/// and the distinction is not decoration: it decides whether the word *after* the flag is that
/// flag's value or the command's next positional argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Arg {
    /// A plain switch. `--verbose`.
    #[default]
    None,
    /// `--file=` â the next word belongs to this flag.
    Required,
    /// `--optarg?` â a value only when it is written `--optarg=x`, never as a separate word.
    Optional,
}

/// How many words a flag's argument is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Nargs {
    #[default]
    One,
    Exactly(usize),
    /// `nargs: -1` â everything up to the next flag.
    Any,
}

/// What the parser does with flags that appear after the first positional argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Parsing {
    /// Flags and arguments mix freely.
    #[default]
    Interspersed,
    /// The first positional argument ends flag parsing â `ssh host -l` passes `-l` to the host.
    NonInterspersed,
    /// Nothing is a flag. `env`, `sudo`, `xargs`: the words belong to whatever runs next.
    Disabled,
}

#[derive(Debug, Clone, Default)]
pub struct OptionSpec {
    /// Every spelling of one flag â `["-m", "--message"]`. All of them are offered.
    pub names: Vec<String>,
    pub description: String,
    pub takes: Arg,
    pub nargs: Nargs,
    /// `--verbose*` â may be given more than once, so it stays on offer after it has been used.
    pub repeatable: bool,
    /// `--internal&` â real, and never offered.
    pub hidden: bool,
    /// `--out!` â the command refuses to run without it.
    pub required: bool,
    pub default: Option<String>,
    /// What this flag's **argument** completes to.
    pub values: Action,
}

impl OptionSpec {
    /// The plain flag, as declared, with no `=value` on it.
    pub fn new(names: Vec<String>, description: String) -> Self {
        Self {
            names,
            description,
            ..Self::default()
        }
    }

    /// Whether `word` is one of this flag's spellings, and what was written after an `=`.
    ///
    /// `--file=x` is one word carrying both the flag and its value, and a walk that only compared
    /// whole words would read it as an argument nobody declared.
    pub fn matches<'w>(&self, word: &'w str) -> Option<Option<&'w str>> {
        let (head, inline) = match word.split_once('=') {
            Some((head, value)) => (head, Some(value)),
            None => (word, None),
        };
        self.names.iter().any(|name| name == head).then_some(inline)
    }
}

/// One command: its name, what it takes, and what each of its positions completes to.
///
/// A subcommand is the same thing one level down, so [`SubcommandSpec`] is this type under the name
/// that reads better at the nesting site. They were two identical structs, which meant every field
/// added here had to be added twice.
#[derive(Debug, Clone, Default)]
pub struct CommandSpec {
    pub name: String,
    /// Other names this command answers to. `git co` for `checkout`, when a spec says so.
    pub aliases: Vec<String>,
    pub description: String,
    /// Real, and never offered.
    pub hidden: bool,
    pub parsing: Parsing,
    pub subcommands: Vec<SubcommandSpec>,
    pub options: Vec<OptionSpec>,
    /// Flags every subcommand inherits. Declared once, answered for at every depth.
    pub persistent: Vec<OptionSpec>,
    /// What each argument position completes to, first position first.
    pub positional: Vec<Action>,
    /// What every position past the end of `positional` completes to.
    pub positional_any: Action,
    /// The same two, for the words after a bare `--`.
    pub dash: Vec<Action>,
    pub dash_any: Action,
}

pub type SubcommandSpec = CommandSpec;

impl CommandSpec {
    /// Whether this command answers to `word`, by its name or one of its aliases.
    pub fn answers_to(&self, word: &str) -> bool {
        self.name == word || self.aliases.iter().any(|alias| alias == word)
    }

    /// The flag `word` names, looked up in this command's own flags and the inherited ones.
    pub fn flag<'a>(&'a self, inherited: &'a [OptionSpec], word: &str) -> Option<&'a OptionSpec> {
        self.options
            .iter()
            .chain(self.persistent.iter())
            .chain(inherited.iter())
            .find(|opt| opt.matches(word).is_some())
    }
}

pub struct SpecRegistry {
    specs: HashMap<String, Rc<CommandSpec>>,
    pub frecency: FrecencyTracker,
}

impl Default for SpecRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SpecRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            specs: HashMap::new(),
            frecency: FrecencyTracker::new(),
        };
        for spec in definitions::all() {
            registry.register(spec);
        }
        registry
    }

    pub fn register(&mut self, spec: CommandSpec) {
        self.specs.insert(spec.name.clone(), Rc::new(spec));
    }

    /// The spec for `cmd`, if anything has one.
    ///
    /// **A config's spec wins over a built-in one.** The four compiled in are a starting point, not
    /// a claim to be right forever â `git` grows subcommands faster than this tree does â and
    /// somebody who has written a better one should get theirs. That is the same rule the settings
    /// take, and the opposite of `register_tool`, where a name that already means something keeps
    /// its meaning because a *command* changing under a script is a different kind of surprise.
    ///
    /// `Rc` rather than a reference because the config-declared table cannot lend one out: it lives
    /// in a `RefCell` that has to be given back before this returns. Cloning the `Rc` is a counter
    /// bump, where cloning the spec would copy git's whole subcommand tree on every keystroke.
    pub fn find_spec(&self, cmd: &str) -> Option<Rc<CommandSpec>> {
        custom::find(cmd).or_else(|| self.specs.get(cmd).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flag_is_recognised_with_and_without_an_inline_value() {
        let opt = OptionSpec {
            names: vec!["-f".into(), "--file".into()],
            takes: Arg::Required,
            ..OptionSpec::default()
        };
        assert_eq!(opt.matches("--file"), Some(None));
        assert_eq!(opt.matches("--file=x"), Some(Some("x")));
        assert_eq!(opt.matches("-f"), Some(None));
        assert_eq!(opt.matches("--other"), None);
    }

    #[test]
    fn a_command_answers_to_its_aliases() {
        let spec = CommandSpec {
            name: "checkout".into(),
            aliases: vec!["co".into()],
            ..CommandSpec::default()
        };
        assert!(spec.answers_to("checkout"));
        assert!(spec.answers_to("co"));
        assert!(!spec.answers_to("commit"));
    }
}
