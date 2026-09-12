//! oslo, as one name.
//!
//! The shell is six crates now, each depending only on those below it:
//!
//! | crate | what it is |
//! |---|---|
//! | [`oslo_luavm`] | a Lua 5.4 VM in pure Rust — stackless, with a tracing collector |
//! | [`oslo_base`] | the syntax tree, the error type, feature bits, the hook registry, the store |
//! | [`oslo_ui`] | the line editor, completion, the dropdown, widgets, theming, the finder |
//! | [`oslo_shell`] | syntax adaptation, expansion, execution, the builtins, directory environments |
//! | [`oslo_runtime`] | the Lua API a config is written against, and starting up |
//!
//! This crate is the facade over all five. It exists so that `oslo::env`, `oslo::ui` and the rest
//! keep meaning what they always meant — to a test, to an example, to anything outside the tree —
//! while the code behind them lives where the compiler can check that it only reaches downward.
//! Nothing is implemented here.

#[cfg(feature = "vista")]
pub use oslo_base::predict;
#[cfg(feature = "secrets")]
pub use oslo_base::secrets;
/// The bottom of the stack: the syntax tree, the error type, the feature bits, the hook registry
/// and the tracking store.
pub use oslo_base::{
    ast, command, error, feature, hooks, macros, messages, nesting, track, version,
};

#[cfg(feature = "direnv")]
pub use oslo_shell::direnv;
/// Which directory's `.make.lua` governs this one. Running one is `oslo_runtime`'s.
#[cfg(feature = "make")]
pub use oslo_shell::make;
#[cfg(feature = "scratch")]
pub use oslo_shell::scratch;
/// Completion spec files, and the macros a spec names. See `oslo_shell::spec`.
pub use oslo_shell::spec;
#[cfg(feature = "watch")]
pub use oslo_shell::watch;
/// The shell: syntax adaptation, expansion, execution, the builtins, the structured pipeline and
/// directory environments.
pub use oslo_shell::{data, env, exec, expand, lexer, names, syntax};

/// The old name for [`syntax`], because a great deal of the tree still says it. There is one shell
/// parser and it is rune's; `syntax` is the lowering into oslo's own tree.
pub use oslo_shell::syntax as parser;

/// The interface layer: the line editor, completion, the dropdown, the widgets, theming and the
/// finder.
pub use oslo_ui as ui;

/// The Lua API a config is written against, and the interpreter that owns it.
pub use oslo_runtime::lua;

/// Helper-level tests for the editor, driven against a real `Environment`.
///
/// Here rather than in `oslo-ui` because that is the point of them: the crate under test knows the
/// shell only through a trait, and these are what check the wiring to the real one.
#[cfg(test)]
mod ui_tests;

pub use env::Environment;
pub use error::{Result, ShellError};
pub use exec::{JobManager, eval_command_list};
pub use lexer::Lexer;
pub use oslo_runtime::{INTERPRETER_STACK, LuaEngine};
pub use parser::parse_bash_script;
pub use ui::OsloHelper;
