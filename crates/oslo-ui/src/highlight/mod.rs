//! Colouring the line being typed, to the depth fish does it.
//!
//! Two stages, and the split is the point. [`lex`] is purely lexical — no `$PATH`, no filesystem,
//! no terminal — so the hard part is testable without any of them. [`classify`] then answers the
//! questions only the shell can: is this word a builtin, a function, a real command or nothing at
//! all, and does that parameter name a file which exists.
//!
//! **The two most useful colours are the ones that need the disk.** fish's `error` marks a command
//! that resolves to nothing *as it is typed*, and `valid_path` marks a parameter that names a real
//! file. Both are a lookup per word per keystroke, which is exactly the cost the command index was
//! built to avoid — so `error` goes through that index, and `valid_path` is capped (see
//! [`Context::check_paths`]).

pub(crate) mod lex;
pub(crate) mod lua;
#[path = "paths.rs"]
mod paths;
use paths::{names_an_existing_file, touches_a_variable};

/// Shared with the correction so the two agree about which word is the command. The correction is
/// its only other caller and is behind `vista`, so a default build — the one that ships — has none.
#[cfg(feature = "vista")]
pub(crate) use lex::is_assignment;
pub use lex::{Role, Span, lex};

use super::command_index::CommandIndex;
use super::theme::{self, Style};

/// What a span is finally coloured as. One variant per role in `theme::Syntax`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenType {
    Command,
    Builtin,
    Function,
    Keyword,
    /// A command name that resolves to nothing.
    Error,
    /// A command that runs what follows it as another user.
    Danger,
    Param,
    /// A parameter that names a file which exists.
    ValidPath,
    Option,
    SingleQuote,
    DoubleQuote,
    Glob,
    /// The wildcards of a glob that matches nothing.
    GlobNoMatch,
    /// A stream coordinate — `{0:1}`, `{%0:0}`. See `lex::Role::Coordinate`.
    Coordinate,
    Number,
    Assignment,
    Escape,
    Operator,
    Redirection,
    End,
    Comment,
    Variable,
    Plain,
}

impl TokenType {
    /// The style this token takes from a theme.
    pub fn style(self, syntax: &theme::Syntax) -> Style {
        match self {
            TokenType::Command => syntax.command,
            TokenType::Builtin => syntax.builtin,
            TokenType::Function => syntax.function,
            TokenType::Keyword => syntax.keyword,
            TokenType::Error => syntax.error,
            TokenType::Param => syntax.param,
            TokenType::ValidPath => syntax.valid_path,
            TokenType::Option => syntax.option,
            TokenType::Danger => syntax.danger,
            TokenType::Glob => syntax.glob,
            TokenType::GlobNoMatch => syntax.glob_nomatch,
            TokenType::Coordinate => syntax.coordinate,
            TokenType::Number => syntax.number,
            TokenType::Assignment => syntax.assignment,
            TokenType::SingleQuote => syntax.single_quote,
            TokenType::DoubleQuote => syntax.double_quote,
            TokenType::Escape => syntax.escape,
            TokenType::Operator => syntax.operator,
            TokenType::Redirection => syntax.redirection,
            TokenType::End => syntax.end,
            TokenType::Comment => syntax.comment,
            TokenType::Variable => syntax.variable,
            TokenType::Plain => Style::default(),
        }
    }
}

/// What the shell knows, for the questions the lexer cannot answer.
pub struct Context<'a> {
    /// The shell's `$PATH`.
    pub path: &'a str,
    /// Answers for builtins — which the command index does not track, because they exist without
    /// any file existing.
    pub is_builtin: &'a dyn Fn(&str) -> bool,
    /// Answers for shell functions and aliases, for the same reason.
    pub is_function: &'a dyn Fn(&str) -> bool,
    /// Whether to `stat` parameters to find the ones that name real files.
    ///
    /// Off for a long line: this is a syscall per word per keystroke, and the point at which it
    /// stops being free is a line long enough that nobody is reading the colours anyway.
    pub check_paths: bool,
}

/// The most words `valid_path` will `stat` on one line.
///
/// A cap rather than a cache: the answer changes when the filesystem does, so a cache would need
/// invalidating on something nothing watches. Eight covers every line anybody types at a prompt,
/// and a line with forty words is one where the colour is not what you are looking at.
const MAX_PATH_CHECKS: usize = 8;

/// Whether each span belongs to a word that globs, and whether that word matches anything.
///
/// **A globbed word arrives here in pieces.** `tw*` is a `Word("tw")` and a `Glob("*")`, and
/// path-checking the first piece asks whether a file called `tw` exists — a question nobody asked,
/// whose answer was then painted onto the line. The word is put back together and resolved once, so
/// `rm *.txt` can say whether it is about to hit anything.
///
/// `None` for every span that is not part of one, which is almost all of them.
fn glob_word_answers(spans: &[Span], ctx: &Context<'_>, checked: &mut usize) -> Vec<Option<bool>> {
    let mut answers = vec![None; spans.len()];
    if !ctx.check_paths || !spans.iter().any(|s| s.role == Role::Glob) {
        return answers;
    }
    let piece = |role: Role| {
        matches!(
            role,
            Role::Word | Role::Glob | Role::Number | Role::SingleQuote | Role::DoubleQuote
        )
    };
    let mut at = 0;
    while at < spans.len() {
        if !piece(spans[at].role) {
            at += 1;
            continue;
        }
        let start = at;
        while at < spans.len() && piece(spans[at].role) {
            at += 1;
        }
        // A quoted piece anywhere means the shell will not expand this word, so it is not a glob.
        let group = &spans[start..at];
        let quoted = group
            .iter()
            .any(|s| matches!(s.role, Role::SingleQuote | Role::DoubleQuote));
        if quoted || !group.iter().any(|s| s.role == Role::Glob) || *checked >= MAX_PATH_CHECKS {
            continue;
        }
        *checked += 1;
        let word: String = group.iter().map(|s| s.text.as_str()).collect();
        let hit = crate::completion::glob_matches_anything(&word);
        for answer in answers.iter_mut().take(at).skip(start) {
            *answer = Some(hit);
        }
    }
    answers
}

/// Resolve every span into its final token type.
pub fn classify(spans: &[Span], ctx: &Context<'_>) -> Vec<(String, TokenType)> {
    let mut out = Vec::with_capacity(spans.len());
    let mut checked = 0usize;
    let globbed = glob_word_answers(spans, ctx, &mut checked);
    let expanded = touches_a_variable(spans);

    for (at, span) in spans.iter().enumerate() {
        let token = match span.role {
            Role::CommandWord => command_token(&span.text, ctx),
            Role::Keyword => TokenType::Keyword,
            Role::Word => {
                if span.text.starts_with('-') && span.text.len() > 1 {
                    TokenType::Option
                } else if let Some(matched) = globbed[at] {
                    // Part of a word that globs; the whole word was resolved once, above.
                    match matched {
                        true => TokenType::ValidPath,
                        false => TokenType::Param,
                    }
                } else if ctx.check_paths && checked < MAX_PATH_CHECKS {
                    // Counted whether or not the file turned out to exist: the cap is on the
                    // syscalls, not on the hits.
                    checked += 1;
                    if !expanded[at] && names_an_existing_file(&span.text) {
                        TokenType::ValidPath
                    } else {
                        TokenType::Param
                    }
                } else {
                    TokenType::Param
                }
            }
            // A glob that matches nothing says so in its wildcards, unless `nullglob` makes an
            // empty expansion the ordinary answer.
            Role::Glob => match globbed[at] {
                Some(false) if !oslo_base::glob::walk::nullglob() => TokenType::GlobNoMatch,
                _ => TokenType::Glob,
            },
            Role::Coordinate => TokenType::Coordinate,
            Role::Number => TokenType::Number,
            Role::Assignment => TokenType::Assignment,
            Role::SingleQuote => TokenType::SingleQuote,
            Role::DoubleQuote => TokenType::DoubleQuote,
            Role::Escape => TokenType::Escape,
            Role::Variable => TokenType::Variable,
            Role::Redirection => TokenType::Redirection,
            Role::Operator => TokenType::Operator,
            Role::End => TokenType::End,
            Role::Comment => TokenType::Comment,
            Role::Plain => TokenType::Plain,
        };
        out.push((span.text.clone(), token));
    }
    out
}

/// Which of the four "this is the thing being run" colours a command word takes.
/// Commands that run what follows them as somebody else.
///
/// Deliberately short. Every entry here changes what every later word on the line is able to do,
/// which is what earns the loudest colour in the theme; a list that also held `rm` and `dd` would
/// spend that colour on things that are merely destructive, and a warning shown constantly is a
/// warning nobody reads.
const RUNS_AS_ANOTHER_USER: &[&str] = &["sudo", "doas", "su", "pkexec", "run0"];

fn command_token(name: &str, ctx: &Context<'_>) -> TokenType {
    if name.is_empty() {
        return TokenType::Plain;
    }
    // A word still being typed is not yet wrong. Marking it red on the first keystroke and green
    // on the last makes the whole line flicker, which is what fish avoids by only colouring a
    // command once it is complete — here, once something resolves.
    // Checked before anything else, and by name rather than by what it resolves to: the point is
    // that the *rest of the line* runs as another user, which is true whether or not this machine
    // happens to have the program installed. A `sudo` that does not resolve is still a warning.
    if RUNS_AS_ANOTHER_USER.contains(&name) {
        return TokenType::Danger;
    }
    // `\cmd` and `\\cmd` ask for the thing the shell's own name is standing in front of, so they
    // must be resolved by what they will actually *reach* — not looked up with the backslash
    // still on, which resolves to nothing and paints a working line as an error. The same bug
    // `=cmd` has below, and the rules are `exec::simple::escape`'s table.
    if let Some(rest) = name.strip_prefix(r"\\") {
        // Only the builtin is skipped; a function by that name still answers.
        return escaped_token(rest, ctx, true);
    }
    if let Some(rest) = name.strip_prefix('\\') {
        // Alias, function and builtin all skipped — only a program on `$PATH` can answer.
        return escaped_token(rest, ctx, false);
    }
    if (ctx.is_builtin)(name) {
        return TokenType::Builtin;
    }
    if (ctx.is_function)(name) {
        return TokenType::Function;
    }
    // A structured verb, a registered tool, an autoloadable function, a stored macro. `$PATH` has
    // never heard of `where`, so `ls | where 'size > 1024'` read as a line with two mistakes in it
    // and ran perfectly.
    //
    // **The kind decides the colour, not merely the fact of being known.** Everything here used to
    // come back `Builtin`, so an autoloadable function was painted as a builtin until the first
    // time it ran and then changed colour underneath the user — the shell disagreeing with itself
    // about what a name is, in the one place that shows.
    if let Some(kind) = oslo_base::vocab::kind_of(name) {
        return match kind {
            "function" | "macro" | "script" => TokenType::Function,
            _ => TokenType::Builtin,
        };
    }
    // `=grep` is the shorthand for where grep lives, so it runs whenever grep does. Looked up as
    // written it resolves to nothing and the line reads as an error while running perfectly.
    if let Some(rest) = name.strip_prefix('=') {
        // The same mask `expand::sugar::equals` reads, so a hidden name draws as an error here
        // rather than as a command the line would then fail to run. See `oslo_base::command`.
        let found = !rest.is_empty()
            && !rest.contains('/')
            && !oslo_base::command::hidden(rest)
            && which::which(rest).is_ok();
        return if found {
            TokenType::Command
        } else {
            TokenType::Error
        };
    }
    if name.contains('/') {
        // A path, not a lookup: `./configure`, `/usr/bin/env`.
        return if which::which(name).is_ok() {
            TokenType::Command
        } else {
            TokenType::Error
        };
    }
    if CommandIndex::contains(ctx.path, name) {
        TokenType::Command
    } else {
        TokenType::Error
    }
}

/// What a backslash-escaped command word resolves to.
///
/// `functions_answer` is the difference between the two forms: `\\cmd` leaves a shell function in
/// the running and `\cmd` does not. Neither can reach a builtin, so a name that is *only* a
/// builtin — `\cd` on a machine with no `/usr/bin/cd` — is an error, and saying so is the point:
/// that line will not run.
fn escaped_token(name: &str, ctx: &Context<'_>, functions_answer: bool) -> TokenType {
    if name.is_empty() {
        return TokenType::Plain;
    }
    if RUNS_AS_ANOTHER_USER.contains(&name) {
        return TokenType::Danger;
    }
    if functions_answer && (ctx.is_function)(name) {
        return TokenType::Function;
    }
    if name.contains('/') {
        return match which::which(name).is_ok() {
            true => TokenType::Command,
            false => TokenType::Error,
        };
    }
    if CommandIndex::contains(ctx.path, name) {
        TokenType::Command
    } else {
        TokenType::Error
    }
}

/// Paint a line with the current theme.
pub fn paint(line: &str, ctx: &Context<'_>) -> String {
    let theme = theme::current();
    let depth = theme::depth();
    let mut tokens = classify(&lex(line), ctx);
    pad_danger(&mut tokens);
    let mut out = String::with_capacity(line.len() * 2);
    for (text, token) in tokens {
        out.push_str(&token.style(&theme.syntax).paint(&text, depth));
    }
    out
}

/// Widen the `sudo` field over the spaces already beside it.
///
/// Existing neighboring spaces are moved into the danger token without changing line width.
fn pad_danger(tokens: &mut [(String, TokenType)]) {
    for at in 0..tokens.len() {
        if tokens[at].1 != TokenType::Danger {
            continue;
        }
        if at > 0 && tokens[at - 1].0.ends_with(' ') && tokens[at - 1].1 != TokenType::Danger {
            tokens[at - 1].0.pop();
            tokens[at].0.insert(0, ' ');
        }
        if let Some(next) = tokens.get_mut(at + 1)
            && next.0.starts_with(' ')
            && next.1 != TokenType::Danger
        {
            next.0.remove(0);
            tokens[at].0.push(' ');
        }
    }
}

#[cfg(test)]
#[path = "paint/tests.rs"]
mod tests;
