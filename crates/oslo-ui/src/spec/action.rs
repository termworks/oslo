//! What one argument position completes to, and why it stays text until Tab.
//!
//! A carapace position is a list of strings, and each string is one of three things:
//!
//! ```yaml
//! positional:
//!   - ["dev", "staging\tthe shared one", "$files([.yaml])", "$prefix(env-)"]
//! ```
//!
//! a **literal** (value, description and a style oslo has no use for, tab-separated), a **macro**
//! that produces values, or a **modifier** that changes the values the entries before it produced.
//! A `` ||| `` inside one entry attaches modifiers to that entry alone.
//!
//! # Why it is not parsed once at load time
//!
//! Because a value may name a variable — `["$files([${C_FLAG_SUFFIX//,/, }])"]` — and the variables
//! are the flags and arguments of the line being typed. Substituting first and parsing after is the
//! order carapace uses, and it is the only order that lets `--suffix=.go` change what the next
//! position offers. Parsing is a handful of `split`s over a handful of short strings; the walk that
//! got here was the expensive part.
//!
//! # And why an [`Action`] can also be a function
//!
//! oslo has Lua. A string DSL exists in carapace because YAML has no functions, and there is no
//! reason for a config written in a real language to go through it — so `positional = { fn }` is a
//! first-class form rather than an escape hatch bolted on. The list form is what a *file* uses.

use std::collections::HashMap;
use std::rc::Rc;

/// One offer a position produced, before it is quoted against the word being completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub value: String,
    pub description: Option<String>,
    /// `$tag(branches)` — the dropdown's kind badge, which is the column that tells a branch from
    /// a file. carapace paints a colour here instead; oslo already has a better column for it.
    pub tag: Option<String>,
}

impl Offer {
    pub fn plain(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            description: None,
            tag: None,
        }
    }
}

/// What a position's values are computed from: the line, as far as it has been typed.
#[derive(Debug, Clone, Default)]
pub struct Query {
    /// The positional arguments of the command being completed, first positional first.
    /// `${C_ARG0}` is `args[0]`.
    pub args: Vec<String>,
    /// Every word of the command, its name first — what a `$(…)` macro is handed as `"$@"`.
    pub words: Vec<String>,
    /// The word being completed. `${C_VALUE}`.
    pub value: String,
    /// What a word break cut off the front of that word, break included: `VAR=` of `VAR=gh`, empty
    /// for a whole word.
    pub before: String,
    /// Flags that were given a value, keyed by longhand in upper case: `${C_FLAG_SUFFIX}`.
    pub flags: HashMap<String, String>,
    /// Where to look. `$chdir` is the only thing that changes it.
    pub dir: String,
}

impl Query {
    /// The value of one `${…}` name, or `None` when nothing has set it.
    pub fn variable(&self, name: &str) -> Option<String> {
        if let Some(index) = name.strip_prefix("C_ARG") {
            return index
                .parse::<usize>()
                .ok()
                .and_then(|at| self.args.get(at))
                .cloned();
        }
        if let Some(flag) = name.strip_prefix("C_FLAG_") {
            return self.flags.get(flag).cloned();
        }
        if name == "C_VALUE" {
            return Some(self.value.clone());
        }
        std::env::var(name).ok()
    }
}

/// A macro `oslo-ui` cannot answer on its own: `$(cmd)`, `$bash(cmd)`, `$spec(file)`.
///
/// The same inversion as [`crate::completion::set_command_completer`] and for the same reason —
/// running a command needs the shell, and the shell sits above this crate. Handed the macro name
/// (empty for `$(…)`), its argument, and the line so far.
pub type Runner = Rc<dyn Fn(&str, &str, &Query) -> Vec<Offer>>;

/// A position answered by a function rather than a list.
pub type Compute = Rc<dyn Fn(&Query) -> Vec<Offer>>;

thread_local! {
    static RUNNER: std::cell::RefCell<Option<Runner>> = const { std::cell::RefCell::new(None) };
}

/// Install the runner for macros that need a shell. `None` removes it.
pub fn set_runner(runner: Option<Runner>) {
    RUNNER.with(|slot| *slot.borrow_mut() = runner);
}

/// The installed runner, cloned out — an answer runs Lua or a subshell, which can complete another
/// word, which would come back through here and panic on the outstanding borrow.
pub fn runner() -> Option<Runner> {
    RUNNER.with(|slot| slot.borrow().clone())
}

/// What a position offers.
#[derive(Clone, Default)]
pub enum Action {
    /// Nothing was declared. The caller falls through to whatever it would have done anyway.
    #[default]
    None,
    /// A carapace value list.
    List(Vec<String>),
    /// A function, for a config written in a language that has them.
    Call(Compute),
}

impl std::fmt::Debug for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Action::None => f.write_str("Action::None"),
            Action::List(list) => f.debug_tuple("Action::List").field(list).finish(),
            Action::Call(_) => f.write_str("Action::Call(..)"),
        }
    }
}

impl Action {
    pub fn is_none(&self) -> bool {
        matches!(self, Action::None)
    }

    /// A list from anything that can be turned into strings.
    pub fn list<I, S>(values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Action::List(values.into_iter().map(Into::into).collect())
    }
}

/// One value-producing entry of a list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    Literal {
        value: String,
        description: Option<String>,
    },
    /// `$files([.go])` — name and raw argument, both after variable substitution.
    Macro { name: String, arg: String },
}

/// Something that changes the values already produced rather than producing any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Modifier {
    Chdir(String),
    Filter(Vec<String>),
    Retain(Vec<String>),
    List(String),
    UniqueList(String),
    MultiParts(Vec<String>),
    Prefix(String),
    Suffix(String),
    Tag(String),
    /// Drop what the line already has. `$filterargs`.
    FilterArgs,
    /// Parsed so that a spec using it is not read as malformed, and then honoured by nothing: the
    /// dropdown has no elvish styles, no usage line, and appends no space there is to suppress.
    Ignored,
}

/// One entry of a list, split on `` ||| `` and classified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// `None` when the entry is a modifier applying to everything before it.
    pub piece: Option<Piece>,
    pub modifiers: Vec<Modifier>,
}

/// Read one entry of a value list. `text` has already had its variables substituted.
pub fn entry(text: &str) -> Entry {
    let mut parts = text.split(" ||| ");
    let head = parts.next().unwrap_or_default();
    let modifiers: Vec<Modifier> = parts.filter_map(modifier).collect();

    if let Some(name) = head.strip_prefix('$') {
        // A modifier written as the head applies to the batch rather than to a value of its own.
        if let Some(m) = modifier(head) {
            let mut all = vec![m];
            all.extend(modifiers);
            return Entry {
                piece: None,
                modifiers: all,
            };
        }
        let (name, arg) = call(name);
        return Entry {
            piece: Some(Piece::Macro { name, arg }),
            modifiers,
        };
    }

    // `value\tdescription\tstyle`. The style is carapace's and oslo has no use for it.
    let mut fields = head.splitn(3, '\t');
    let value = fields.next().unwrap_or_default().to_string();
    let description = fields.next().filter(|d| !d.is_empty()).map(str::to_string);
    Entry {
        piece: Some(Piece::Literal { value, description }),
        modifiers,
    }
}

/// Split `name(arg)` into its halves. `$(cmd)` has no name, which is how the shell macro is spelled.
fn call(text: &str) -> (String, String) {
    match text.find('(') {
        Some(at) if text.ends_with(')') => (
            text[..at].to_string(),
            text[at + 1..text.len() - 1].to_string(),
        ),
        _ => (text.to_string(), String::new()),
    }
}

/// The modifier `text` names, or `None` when it names something else.
fn modifier(text: &str) -> Option<Modifier> {
    let (name, arg) = call(text.strip_prefix('$')?);
    Some(match name.as_str() {
        "chdir" => Modifier::Chdir(arg),
        "filter" => Modifier::Filter(bracketed(&arg)),
        "retain" => Modifier::Retain(bracketed(&arg)),
        "list" => Modifier::List(arg),
        "uniquelist" => Modifier::UniqueList(arg),
        "multiparts" => Modifier::MultiParts(bracketed(&arg)),
        "prefix" => Modifier::Prefix(arg),
        "suffix" => Modifier::Suffix(arg),
        "tag" => Modifier::Tag(arg),
        "filterargs" => Modifier::FilterArgs,
        "style" | "usage" | "suppress" | "nospace" | "noprefix" | "shift" | "split" | "splitp" => {
            Modifier::Ignored
        }
        _ => return None,
    })
}

/// The members of a `[a, b, c]` argument. A bare word is a list of one, which is how
/// `$files(.go)` and `$files([.go])` come to mean the same thing.
pub fn bracketed(arg: &str) -> Vec<String> {
    let inner = arg
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(arg);
    inner
        .split(',')
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect()
}

#[cfg(test)]
#[path = "action/tests.rs"]
mod tests;
