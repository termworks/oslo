//! `oslo.make` — the four things the recipe runner cannot ask Lua for, and the runner itself.
//!
//! Everything here is plumbing between the `oslo make` tool and [`make.lua`](./make.lua), which is
//! where the graph, the staleness check and the listing actually live. The split is
//! `nix.rs`'s, for the same reason it gives: what is written in Lua can be read, replaced and
//! argued with by the person whose build it is.
//!
//! # Why a child process is the whole design
//!
//! A registered builtin runs while the shell holds its own state, so `oslo.run` inside one fails
//! with *shell state is busy* — measured, and the reason `docs/features/your-own-tools.md` lists it
//! as a limit. A build runner that cannot run a command is not a build runner.
//!
//! `direnv` gets around it by running between commands, where the shell holds nothing. A builtin
//! has no such moment: it is called from inside dispatch, in the middle of the state it would have
//! to release.
//!
//! So `oslo make` is a **separate process** — a fresh `oslo` with its own engine, holding no
//! interactive state, where the whole API is simply available. That is what `oslo macros` and
//! `oslo secret` already are. What it costs is what `make` and `just` already cost: a recipe cannot
//! `cd` the shell that called it. That is the semantics, not a regression.

use super::util::{int, ok, put, text};
#[cfg(feature = "watch")]
use oslo_base::value::LuaError;
use oslo_base::value::{Table, Value};
use std::cell::RefCell;
use std::path::{Path, PathBuf};

/// The runner, in the language it is meant to be read in.
const RUNNER: &str = include_str!("make.lua");

thread_local! {
    /// The last string the runner handed back. See `__emit`.
    static EMITTED: RefCell<Option<String>> = const { RefCell::new(None) };
}

thread_local! {
    /// What this process was asked to do. Empty in an interactive shell, which never calls `__main`.
    static INVOCATION: RefCell<Invocation> = RefCell::new(Invocation::default());
}

/// The `oslo make` call, as the Lua side needs to see it.
#[derive(Default)]
struct Invocation {
    file: PathBuf,
    root: PathBuf,
    args: Vec<String>,
    /// What the process should exit with. `None` until the runner says.
    status: Option<i32>,
    #[cfg(feature = "watch")]
    watch: Option<WatchRequest>,
}

#[cfg(feature = "watch")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchRequest {
    pub target: String,
    pub args: Vec<String>,
    pub patterns: Vec<String>,
    pub initial: bool,
    pub policy: oslo_shell::watch::Policy,
}

/// Record what the tool was asked, before the engine is handed the recipe file.
pub fn begin(file: &Path, root: &Path, args: &[String]) {
    INVOCATION.with(|slot| {
        *slot.borrow_mut() = Invocation {
            file: file.to_path_buf(),
            root: root.to_path_buf(),
            args: args.to_vec(),
            status: None,
            #[cfg(feature = "watch")]
            watch: None,
        };
    });
}

/// Take whatever the runner last handed back through `__emit`.
pub fn emitted() -> Option<String> {
    EMITTED.with(|slot| slot.borrow_mut().take())
}

/// The status the runner asked to exit with, if it got far enough to ask.
pub fn status() -> Option<i32> {
    INVOCATION.with(|slot| slot.borrow().status)
}

#[cfg(feature = "watch")]
pub fn take_watch() -> Option<WatchRequest> {
    INVOCATION.with(|slot| slot.borrow_mut().watch.take())
}

/// Build the `oslo.make` table: the parts that have to be Rust, and nothing else.
pub fn build() -> Value {
    let mut make = Table::new();

    // oslo.make.__argv() -> { "build", "--type", "minimal" }
    put(&mut make, "__argv", |_, _| {
        let mut list = Table::new();
        INVOCATION.with(|slot| {
            for (i, arg) in slot.borrow().args.iter().enumerate() {
                list.set(Value::int(i as i64 + 1), Value::str(arg.clone()));
            }
        });
        ok(Value::table(list))
    });

    // oslo.make.__file() -> "/home/you/project/.make.lua"
    put(&mut make, "__file", |_, _| {
        ok(INVOCATION.with(|slot| Value::str(slot.borrow().file.display().to_string())))
    });

    // oslo.make.__root() -> "/home/you/project"
    put(&mut make, "__root", |_, _| {
        ok(INVOCATION.with(|slot| Value::str(slot.borrow().root.display().to_string())))
    });

    // oslo.make.__status(n) — what the process exits with.
    put(&mut make, "__status", |_, args| {
        let code = int(&args, 1, "oslo.make.__status")?;
        INVOCATION.with(|slot| slot.borrow_mut().status = Some(code as i32));
        ok(Value::Nil)
    });

    // oslo.make.__emit(text) — hand a string back to the Rust side.
    //
    // `eval_as` answers with a status and not with a value, so a caller that wants text out of the
    // runner has to be given somewhere to put it. Only the help page uses this, and only so that
    // the page stays a pure function the tests can read.
    put(&mut make, "__emit", |_, args| {
        let text = text(&args, 1, "oslo.make.__emit")?;
        EMITTED.with(|slot| *slot.borrow_mut() = Some(text));
        ok(Value::Nil)
    });

    #[cfg(feature = "watch")]
    put(&mut make, "__watch", |_, args| {
        let value = args
            .first()
            .ok_or_else(|| LuaError::new("oslo.make.__watch: expected a table".to_string()))?;
        let Value::Table(request) = value else {
            return Err(LuaError::new(
                "oslo.make.__watch: expected a table".to_string(),
            ));
        };
        let request = request.borrow();
        let target = watch_text(&request.get_str("target"), "target")?;
        let args = watch_strings(&request.get_str("args"), "args")?;
        let patterns = watch_strings(&request.get_str("paths"), "paths")?;
        let initial = match request.get_str("initial") {
            Value::Bool(value) => value,
            other => {
                return Err(LuaError::new(format!(
                    "oslo.make.__watch: initial must be a boolean, got {}",
                    other.type_name()
                )));
            }
        };
        let policy = match watch_text(&request.get_str("policy"), "policy")?.as_str() {
            "coalesce" => oslo_shell::watch::Policy::Coalesce,
            "restart" => oslo_shell::watch::Policy::Restart,
            other => {
                return Err(LuaError::new(format!(
                    "oslo.make.__watch: unknown policy {other:?}"
                )));
            }
        };
        INVOCATION.with(|slot| {
            slot.borrow_mut().watch = Some(WatchRequest {
                target,
                args,
                patterns,
                initial,
                policy,
            });
        });
        ok(Value::Nil)
    });

    // oslo.make.__relative(path) -> the path as it reads from the project root.
    //
    // Only for messages. A recipe that printed an absolute path for every file in the tree would
    // be unreadable, and the root is a fact this side already holds.
    put(&mut make, "__relative", |_, args| {
        let path = text(&args, 1, "oslo.make.__relative")?;
        let shown = INVOCATION.with(|slot| {
            let root = slot.borrow().root.clone();
            Path::new(&path)
                .strip_prefix(&root)
                .map(|rest| rest.display().to_string())
                .unwrap_or(path.clone())
        });
        ok(Value::str(shown))
    });

    Value::table(make)
}

#[cfg(feature = "watch")]
fn watch_text(value: &Value, field: &str) -> Result<String, LuaError> {
    match value {
        Value::Str(text) => Ok(text.to_string()),
        other => Err(LuaError::new(format!(
            "oslo.make.__watch: {field} must be a string, got {}",
            other.type_name()
        ))),
    }
}

#[cfg(feature = "watch")]
fn watch_strings(value: &Value, field: &str) -> Result<Vec<String>, LuaError> {
    let Value::Table(items) = value else {
        return Err(LuaError::new(format!(
            "oslo.make.__watch: {field} must be a list of strings"
        )));
    };
    items
        .borrow()
        .sequence()
        .iter()
        .enumerate()
        .map(|(index, value)| watch_text(value, &format!("{field}[{}]", index + 1)))
        .collect()
}

/// Run `make.lua`, which fills `oslo.make` in with everything a recipe file talks to.
///
/// The chunk *adds* to the table it is handed, and it reaches that table through the global rather
/// than through its argument — a table across the boundary is a copy, so additions to the argument
/// would land on a value thrown away when the call returned. `nix::add_helpers` carries
/// the long version of this.
///
/// **A failure is reported and startup continues.** An interactive shell that would not start
/// because the recipe runner has a typo in it would be a far worse outcome than one that cannot
/// run recipes.
pub(super) fn add_helpers(host: &dyn oslo_luavm::Host) {
    let chunk = "oslo.make";
    let source = format!("local install = (function() {RUNNER} end)()\ninstall(oslo.make)");
    if let Err(e) = host.eval(&source, chunk) {
        eprintln!("oslo: {chunk}: {e}");
    }
}
