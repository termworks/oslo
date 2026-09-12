//! The bottom of oslo: what everything above it is built out of.
//!
//! Five modules with one thing in common — none of them knows there is a shell above them. That is
//! the whole selection rule, and it was measured rather than guessed: every one of these had zero
//! references to `ui`, `exec`, `expand` or the syntax layer before it was moved, and the compiler
//! now keeps it that way.
//!
//! * [`ast`] — the syntax tree the parser produces and the executor walks.
//! * [`error`] — the one error type the evaluator unwinds with.
//! * [`feature`] — the parts of the shell a config can turn off and on again while it runs.
//! * [`hooks`] — where the shell reaches a hook, without knowing that Lua exists.
//! * [`track`] — the store behind history, frecency and the recorded outcome of a command.
//!
//! # What is not here yet
//!
//! **`Environment`.** It reads as though it belongs — a variable store depending on nothing — and
//! the crate plan says so. It does not: the store *holds* the shell's builtin table as a field and
//! `Environment::new()` fills it in, so it sits above the builtins rather than below them. Moving
//! it means deciding who constructs a shell environment, and that is a question about the shell,
//! not about this crate. It is left where it is until the answer is worth the churn.

pub mod ast;
/// The output of a command that was asked to keep it, for `copy --last`.
pub mod background;
pub mod base64;
/// Brace expansion — `{a,b}`, `{1..9}` — shared by the parser and the highlighter.
///
/// Here rather than in the shell because the prompt has to agree with the expander about what a
/// word means: a highlighter that did not know `{a,b}` painted a valid path as a dead one.
pub mod brace;
pub mod capture;
/// Stream coordinates — `{line:word}` addressing of what a command printed.
pub mod clock;
pub mod command;
pub mod coords;
/// A caret under the word that was wrong — the drawn face of an error.
///
/// **One `#[cfg]` for the whole feature.** The stub mirrors every signature, so no caller anywhere
/// in the workspace writes a conditional of its own; the choice is made here and nowhere else.
#[cfg(feature = "diagnostics")]
pub mod diag;
#[cfg(not(feature = "diagnostics"))]
#[path = "diag_stub.rs"]
pub mod diag;
/// The `@name` directory table, shared by expansion and completion.
pub mod dirs;
pub mod error;
pub mod exe;
/// Parts of the shell a config can turn off and on again while it runs.
pub mod feature;
/// Shell pattern matching — `*`, `?`, `[…]` — shared by expansion, `case`, and the prompt.
pub mod glob;
pub mod hooks;
pub mod lossless;
pub mod macros;
/// What this session said, kept after it has scrolled off.
pub mod messages;
/// The depth guard every parse passes through, and the heredoc scan that feeds alias expansion.
pub mod nesting;
#[cfg(feature = "vista")]
pub mod predict;
pub mod prompts;
/// Whether the command now running must leave no trace of itself.
pub mod quiet;
#[cfg(feature = "secrets")]
pub mod secrets;
/// Untrusted text on its way into a diagnostic, with what the terminal would obey spelled out.
pub mod shown;
/// How much stack is left, for the constructs that re-enter the interpreter.
pub mod stack;
/// A database a config or a plugin owns, kept apart from oslo's own.
pub mod store;
/// The shell's version, as one number rather than one per crate.
/// Tilde expansion — `~`, `~user`, `~+`, `~-` — shared by the shell and the prompt.
pub mod tilde;
pub mod track;
/// The dynamic value the shell and its Lua share — tables, numbers, strings.
pub mod value;
pub mod version;
/// Names the shell can run that `$PATH` has never heard of, for the prompt to draw.
pub mod vocab;
pub mod wire;

pub use error::{Result, ShellError};
