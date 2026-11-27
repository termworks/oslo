#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WordPart {
    Literal(String),
    /// Characters the lexer stripped a backslash from.
    ///
    /// Distinct from [`WordPart::Literal`] because escaping is a form of quoting: `\*` must not
    /// glob and `a\ b` must not field-split, neither of which a literal can express. Kept as its
    /// own variant rather than a flag on `Literal` so that every match on a word part has to
    /// decide what escaping means for it.
    Escaped(String),
    SingleQuoted(String),
    DoubleQuoted(Vec<WordPart>),
    Variable {
        name: String,
        expansion_type: ParamExpansion,
    },
    /// `${name[subscript]}` — a reference into an indexed array.
    ///
    /// A separate variant rather than an `Option<Subscript>` on [`WordPart::Variable`] so that
    /// every consumer has to decide what a subscript means for it. The two are otherwise the same
    /// reference, and the operators in [`ParamExpansion`] apply to both.
    ArrayRef {
        name: String,
        subscript: Subscript,
        expansion_type: ParamExpansion,
    },
    CommandSubstitution(String),
    /// bash 5.3's `${ cmd; }` and `${| cmd; }`: the body runs in the current shell, and the word
    /// is what it printed or, with `reply`, the value it left in `REPLY`.
    FunctionSubstitution {
        body: String,
        reply: bool,
    },
    /// `<(cmd)` or `>(cmd)` — the command runs, and the word becomes a *filename* naming a pipe
    /// connected to it.
    ///
    /// A word part rather than a redirection because that is what it is: `diff <(a) <(b)` passes
    /// two ordinary arguments to `diff`, which opens them like files. The `bool` is whether the
    /// shell reads from the command (`<`) or writes to it (`>`).
    ProcessSubstitution {
        /// True for `<(cmd)`: the substituted process writes, the caller reads.
        reads_from_command: bool,
        command: String,
    },
    Arithmetic(String),
    Tilde(String),
}

/// What sits between the brackets of an array reference.
///
/// `[@]` and `[*]` differ exactly as `"$@"` and `"$*"` do: the first is one field per element,
/// the second is one field with the elements joined by IFS. Anything else is an arithmetic
/// expression naming a single element, which is why it is a [`Word`] — `${a[i+1]}` has to expand
/// `i` before it can be evaluated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subscript {
    /// `[@]` — every element, kept apart under quoting.
    All,
    /// `[*]` — every element, joined by the first character of IFS.
    Joined,
    /// `[expr]` — one element, selected by an arithmetic expression.
    Index(Word),
}

/// The operator inside a `${...}`, with its operand still unexpanded.
///
/// Every operand is a [`Word`], not a `String`: `${x:-$HOME}` has to expand its default, and
/// `${x:=$y}` has to *assign* the expanded text. Storing the raw source here and expanding it in
/// `crate::expand::param` is what keeps `$HOME` from being persisted literally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamExpansion {
    Normal,
    /// `${#name}` — character length, or the positional count for `#`, `@` and `*`.
    Length,
    /// `${name:-word}`, `${name-word}`, and the assigning `:=` / `=` forms.
    DefaultValue {
        default: Word,
        assign_if_unset: bool,
        /// The `:` variants also treat a set-but-empty parameter as absent; the colon-less ones
        /// test only for *unset*, which is the whole difference between `${x-d}` and `${x:-d}`.
        test_null: bool,
    },
    UseAlternative {
        alternative: Word,
        test_null: bool,
    },
    ErrorIfUnset {
        message: Word,
        test_null: bool,
    },
    /// `${name%pat}` / `${name%%pat}`.
    RemoveSuffix {
        pattern: Word,
        longest: bool,
    },
    /// `${name#pat}` / `${name##pat}`.
    RemovePrefix {
        pattern: Word,
        longest: bool,
    },
    /// `${name:offset}` / `${name:offset:length}`; both operands are arithmetic.
    Substring {
        offset: Word,
        length: Option<Word>,
    },
    /// `${name/pat/rep}` and its `//`, `/#`, `/%` variants.
    Replace {
        pattern: Word,
        replacement: Word,
        scope: ReplaceScope,
    },
    /// `${name^}`, `${name^^}`, `${name,}`, `${name,,}`.
    CaseConvert {
        /// Which characters are eligible; `None` means every character.
        pattern: Option<Word>,
        upper: bool,
        /// Doubled operator: convert every eligible character rather than just the first.
        all: bool,
    },
    /// `${name~}` and `${name~~}`: swap the case of the first, or of every, eligible character.
    CaseToggle {
        pattern: Option<Word>,
        all: bool,
    },
    /// `${name@op}`: one of bash's transformations, `Q E P A K a k U u L`.
    Transform(char),
    /// `${!name}` — `name` holds the name of the parameter to expand.
    ///
    /// **The payload is what to do once the second name is known**, because the two compose:
    /// `${!v:-d}` is "the thing `v` names, defaulted", and `${!v+alt}` is "alt, if the thing `v`
    /// names is set". stdenv's `runHook` is written with the second form, so the composition is
    /// not a curiosity — a shell without it cannot run a dev shell's hooks.
    ///
    /// `Indirect(Normal)` is the plain `${!name}`.
    Indirect(Box<ParamExpansion>),
}

/// Which occurrences `${name/pat/rep}` replaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplaceScope {
    /// `/` — the leftmost match.
    First,
    /// `//` — every match.
    All,
    /// `/#` — only a match anchored at the start.
    Prefix,
    /// `/%` — only a match anchored at the end.
    Suffix,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Word {
    pub parts: Vec<WordPart>,
}

impl Word {
    pub fn from_literal(s: &str) -> Self {
        Self {
            parts: vec![WordPart::Literal(s.to_string())],
        }
    }
}

/// What an assignment writes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssignmentTarget {
    /// `name=…`.
    Name(String),
    /// `name[expr]=…` — one element of an indexed array. The subscript is arithmetic and is
    /// evaluated when the assignment runs, so `a[i+1]=x` writes where `i+1` points *then*.
    Element { name: String, index: Word },
}

impl AssignmentTarget {
    /// The variable being written to, without its subscript.
    pub fn name(&self) -> &str {
        match self {
            Self::Name(name) => name,
            Self::Element { name, .. } => name,
        }
    }
}

/// One element of an array literal: `(a b)` or `([2]=a [5]=b)`, which may be mixed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArrayElement {
    /// An explicit `[index]=`, evaluated as arithmetic. `None` means "the next free index".
    pub index: Option<Word>,
    pub value: Word,
}

/// The right-hand side of an assignment.
///
/// An array literal is *not* a word: `a=(1 2 3)` is three elements, and flattening it to the
/// source text `(1 2 3)` is what made `echo "$a"` print the parentheses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssignmentValue {
    Scalar(Word),
    Array(Vec<ArrayElement>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignment {
    pub target: AssignmentTarget,
    pub value: AssignmentValue,
    /// `+=` — append to the existing value rather than replacing it. For an array literal that
    /// means appending elements after the highest index in use.
    pub append: bool,
}

impl Assignment {
    /// A plain `name=value` assignment, which is what almost every caller builds.
    pub fn scalar(name: &str, value: Word) -> Self {
        Self {
            target: AssignmentTarget::Name(name.to_string()),
            value: AssignmentValue::Scalar(value),
            append: false,
        }
    }

    /// The variable this assignment writes to, without any subscript.
    pub fn name(&self) -> &str {
        self.target.name()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedirectKind {
    Input,        // <
    Output,       // >
    Append,       // >>
    ReadWrite,    // <>
    DupInput,     // <&
    DupOutput,    // >&
    Heredoc,      // <<
    HeredocStrip, // <<-
    Clobber,      // >|
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redirection {
    pub fd: Option<i32>,
    pub kind: RedirectKind,
    pub target: Word,
    /// Body of a here-document or here-string, as parts rather than text.
    ///
    /// It has to be a [`Word`]: an unquoted heredoc body is expanded before it reaches the
    /// command, so the parser cannot flatten it to a string without deciding, at parse time,
    /// what `$v` means. A quoted delimiter (`<<'EOF'`) suppresses expansion, and the parser
    /// records that by storing a single literal part.
    pub heredoc_content: Option<Word>,
    /// True when the body came from `<<< word` rather than `<<DELIM`.
    ///
    /// A here-string's document is the expanded word *plus* a newline, and a here-document's is
    /// the body text exactly as written — which already carries its own line terminators. The
    /// newline cannot be baked into `heredoc_content`, because `<<< "$(cmd)"` must first lose
    /// the trailing newlines command substitution strips, and a literal part appended before
    /// expansion is not covered by that rule.
    pub here_string: bool,
    /// `{name}>file`: the descriptor is chosen by the shell (10 or above) and stored in `name`,
    /// or, for `{name}>&-`, read from it. `name` may carry a subscript, as in `{fd[1]}`.
    pub fd_var: Option<String>,
    /// True when brace expansion turned the target into several words, as in `> o{a,b}`, which
    /// is an ambiguous redirect when the command runs; `target` is then the text as written.
    pub ambiguous: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimpleCommand {
    pub assignments: Vec<Assignment>,
    pub words: Vec<Word>,
    pub redirections: Vec<Redirection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Simple(SimpleCommand),
    Compound {
        kind: CompoundCommand,
        redirections: Vec<Redirection>,
    },
    FunctionDef {
        name: String,
        body: Box<Command>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompoundCommand {
    If {
        condition: CommandList,
        then_branch: CommandList,
        elif_branches: Vec<(CommandList, CommandList)>,
        else_branch: Option<CommandList>,
    },
    While {
        condition: CommandList,
        body: CommandList,
    },
    Until {
        condition: CommandList,
        body: CommandList,
    },
    For {
        var_name: String,
        items: Option<Vec<Word>>,
        body: CommandList,
    },
    /// `select name [in words]; do … done`: a numbered menu on stderr, one line of stdin per turn.
    ///
    /// `items` is `None` for `select name; do`, which offers the positional parameters.
    Select {
        var_name: String,
        items: Option<Vec<Word>>,
        body: CommandList,
    },
    /// `coproc [NAME] command`: `body` runs asynchronously, joined to the shell by two pipes.
    ///
    /// `NAME` (`COPROC` when omitted) becomes an array of the shell's two ends, `NAME_PID` the pid.
    Coproc {
        name: String,
        body: Box<Command>,
    },
    Case {
        word: Word,
        items: Vec<CaseItem>,
    },
    /// `(( expr ))` — an arithmetic expression evaluated as a *command*.
    ///
    /// The expression is kept as source text, not as a parsed tree: POSIX expands parameters and
    /// command substitutions over it before any of it is arithmetic, so it cannot be parsed until
    /// it runs. `crate::expand::arithmetic` owns that whole pipeline, and reusing it here is what
    /// makes `(( x += 1 ))` and `$(( x += 1 ))` agree.
    ///
    /// This is *not* a [`CompoundCommand::Subshell`]: `(( x++ ))` must leave `x` incremented in
    /// the shell that ran it, which is the entire reason the construct exists.
    Arithmetic(String),
    /// `for ((init; cond; step)) do … done`.
    ///
    /// Each section is optional and independently so — `for ((;;))` is the idiomatic infinite
    /// loop — hence three `Option`s rather than one struct with empty strings, which would make
    /// "absent" and "the empty expression" indistinguishable.
    ArithmeticFor {
        init: Option<String>,
        cond: Option<String>,
        step: Option<String>,
        body: CommandList,
    },
    Subshell(CommandList),
    Group(CommandList),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseItem {
    pub patterns: Vec<Word>,
    pub body: CommandList,
    /// What to do once this branch's body has run.
    pub post_action: CaseAction,
}

/// What `case` does after running the body of a branch that matched.
///
/// Recorded per branch rather than inferred, because the three terminators are three different
/// programs: dropping the distinction made `;&` and `;;&` behave as `;;`, which silently skipped
/// the branches the script was relying on running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CaseAction {
    /// `;;` — leave the `case`.
    #[default]
    ExitCase,
    /// `;&` — run the *next* branch's body without testing its patterns at all.
    FallThrough,
    /// `;;&` — carry on testing the remaining branches' patterns, as if this one had not matched.
    ContinueMatching,
}

impl CaseAction {
    /// The source terminator this action was written as.
    ///
    /// Shared by the two function printers so that what they emit re-parses to the same tree.
    pub fn terminator(self) -> &'static str {
        match self {
            Self::ExitCase => ";;",
            Self::FallThrough => ";&",
            Self::ContinueMatching => ";;&",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pipeline {
    pub negated: bool,
    /// The `time` keyword prefixed this pipeline: report `real`/`user`/`sys` once it finishes.
    ///
    /// A property of the pipeline rather than of a command, because that is what `time` measures:
    /// `time a | b` times the whole pipe, not `a`. It changes nothing about what the pipeline
    /// runs, what it writes to stdout, or the status it reports — see
    /// `exec::pipeline::eval_pipeline`.
    pub timed: bool,
    pub commands: Vec<Command>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AndOrOp {
    And, // &&
    Or,  // ||
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AndOrList {
    pub first: Pipeline,
    pub rest: Vec<(AndOrOp, Pipeline)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListOp {
    Sequential, // ;
    Background, // &
    Newline,
}

#[derive(Debug, Clone, Eq)]
pub struct ListItem {
    pub and_or: AndOrList,
    pub op: ListOp,
    /// Line in the source this item starts on, 1-based, or 0 when it did not come from a file.
    ///
    /// Carried so `$LINENO` can answer. It is per *list item* — per statement — because that is
    /// the granularity POSIX describes and the one a `die() { echo "$0:$LINENO"; }` helper wants:
    /// the line of the command being run, not of some sub-expression inside it.
    pub line: u32,
}

/// Equality is *structural*: where a command was written is not part of what it is.
///
/// Hand-written rather than derived so that `line` stays out of it. Two items that run the same
/// commands are the same command whether they came from line 3 or line 30 — which is what the
/// unparse round-trip test asserts, since re-emitting a script legitimately moves everything onto
/// different lines.
impl PartialEq for ListItem {
    fn eq(&self, other: &Self) -> bool {
        self.and_or == other.and_or && self.op == other.op
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommandList {
    pub items: Vec<ListItem>,
}
