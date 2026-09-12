//! `oslo make` — run a recipe from the project's `.make.lua`.
//!
//! # A process, not a builtin
//!
//! This is the whole reason the feature is shaped the way it is. A builtin registered from Lua runs
//! while the shell holds its own state, so `oslo.run` inside one fails with *shell state is busy* —
//! and a build runner that cannot run a command is not one. `direnv` escapes that by running
//! between commands, at the one moment the shell holds nothing; a builtin has no such moment,
//! because it is called from the middle of the state it would have to release.
//!
//! So a recipe runs here, in a **fresh `oslo`** with its own engine and no interactive state, where
//! the whole Lua API simply works. `oslo macros`, `oslo hook` and `oslo secret` are the same shape.
//!
//! What that costs is what `make` and `just` already cost: a recipe cannot `cd` the shell that
//! called it or set a variable in it. That is the semantics of a recipe, not a limitation of this.
//!
//! # The order of the three steps
//!
//! ```text
//! find .make.lua      walk up from the working directory — oslo_shell::make
//!   ▼
//! chdir to its directory        make's rule: a recipe resolves `src/` against the project
//!   ▼
//! engine + config + bindings    so `oslo.make` exists before the file mentions it
//!   ▼
//! load .make.lua                declarations only — every body is a function, nothing runs
//!   ▼
//! oslo.make.__main()            parse the argv, plan the graph, run it, set the status
//! ```
//!
//! Loading the user's `init.lua` first is deliberate: a person's own helpers should be reachable
//! from a recipe, and it is the same "reproduce the load in this process" rule `oslo hook` follows.

use crate::cli::help::Paint;

/// Run the tool. The status is the process's.
pub(crate) fn run(args: &[String]) -> i32 {
    // **Before the file is looked for**, because `oslo make --help` has to answer somewhere that has
    // no project in it — which is exactly where somebody asks what the command is.
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{}", help(Paint::detect()));
        return 0;
    }

    let here = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("oslo make: cannot read the working directory: {e}");
            return 1;
        }
    };

    let Some(file) = oslo::make::governing(&here) else {
        eprintln!(
            "oslo make: no {} in {} or any directory above it",
            oslo::make::NAME,
            here.display()
        );
        return 1;
    };
    let Some(root) = oslo::make::root_of(&file).map(std::path::Path::to_path_buf) else {
        eprintln!("oslo make: {} has no directory", file.display());
        return 1;
    };

    // **Before the engine, not after.** A recipe resolves `src/**/*.rs` against the project, and the
    // glob runs the moment the file is read — so the working directory has to be true by then.
    if let Err(e) = std::env::set_current_dir(&root) {
        eprintln!("oslo make: cannot enter {}: {e}", root.display());
        return 1;
    }

    oslo_runtime::lua::api::make::begin(&file, &root, args);

    let env = oslo::env::Environment::new();
    let files = oslo_runtime::startup::lua_init::config_files(&env);
    let Ok(engine) = oslo_runtime::LuaEngine::new() else {
        eprintln!("oslo make: no Lua interpreter");
        return 1;
    };
    let shared = std::sync::Arc::new(std::sync::Mutex::new(env));
    if !oslo_runtime::startup::lua_init::install_bindings(&engine, shared) {
        eprintln!("oslo make: the Lua bindings could not be installed");
        return 1;
    }
    // **Before any recipe can spawn.** A `.make.lua` never enters a REPL, so nothing was watching
    // the pipe a finished worker nudges — `oslo.spawn` here queued its result and no callback ever
    // ran. `job:wait()` and `oslo.settle()` are what do the looking now, and this is what gives
    // them something to look at.
    oslo_runtime::startup::lua_init::install_batch_delivery();
    for path in &files {
        oslo_runtime::startup::lua_init::load_config(&engine, path);
    }

    // The recipe file is loaded by path so an error points into it rather than into a string.
    let Some(text) = file.to_str() else {
        eprintln!("oslo make: {}: path is not valid UTF-8", file.display());
        return 1;
    };
    if let Err(e) = engine.load_file(text) {
        eprintln!(
            "oslo: {}: {e}",
            oslo_ui::marks::path(&file.display().to_string())
        );
        return 1;
    }

    if let Err(e) = engine.eval_as("oslo.make.__main()", "oslo.make") {
        eprintln!("oslo make: {e}");
        return 1;
    }

    #[cfg(feature = "watch")]
    if let Some(request) = oslo_runtime::lua::api::make::take_watch() {
        return watch(root, request);
    }

    // `__main` always ends by setting one. A missing status means it raised past its own handler,
    // which the `eval_as` above would already have reported — so this is the belt to that braces.
    oslo_runtime::lua::api::make::status().unwrap_or(1)
}

#[cfg(feature = "watch")]
fn watch(root: std::path::PathBuf, request: oslo_runtime::lua::api::make::WatchRequest) -> i32 {
    let mut argv = vec![
        "/proc/self/exe".to_string(),
        "make".to_string(),
        request.target.clone(),
    ];
    argv.extend(request.args);
    crate::cli::watch::launch(crate::cli::watch::Request {
        spec: oslo::watch::WatchSpec {
            name: format!("make-{}", request.target),
            root,
            patterns: request.patterns,
            argv,
            initial: request.initial,
            debounce: std::time::Duration::from_millis(100),
            policy: request.policy,
            grace: std::time::Duration::from_millis(1000),
        },
        scratch: crate::cli::watch::ScratchChoice::Auto,
        attach: false,
    })
}

/// The page, which has to answer in a directory holding no project at all.
///
/// **The options come from the runner, not from here.** `make.lua` parses them, so it is the only
/// thing that knows which exist — a second list in this file would be one release away from
/// describing a flag that had been renamed. [`OPTIONS`] is the fallback for a build with no working
/// interpreter, and `the_help_lists_every_flag_the_runner_parses` keeps it honest.
fn help(paint: Paint) -> String {
    let file = paint.key(oslo::make::NAME);
    format!(
        "{usage}\n  oslo make [OPTIONS] [RECIPE [ARGS...]]      no recipe: list them\n\n\
         {reads}\n  \
         {file}, from the nearest directory at or above this one\n  \
         every recipe runs with that directory as its working directory\n  \
         your init.lua is loaded first, so your own helpers are in scope\n\n\
         {options}",
        usage = paint.head("USAGE"),
        reads = paint.head("WHAT IT READS"),
        options = options(),
    )
}

/// The runner's own option list, asked of the runner.
///
/// An engine with no recipe file in it, because the flags are the runner's and not the project's.
fn options() -> String {
    let engine = match oslo_runtime::LuaEngine::new() {
        Ok(engine) => engine,
        Err(_) => return OPTIONS.to_string(),
    };
    let env = std::sync::Arc::new(std::sync::Mutex::new(oslo::env::Environment::new()));
    if !oslo_runtime::startup::lua_init::install_bindings(&engine, env) {
        return OPTIONS.to_string();
    }
    match engine.eval_as("oslo.make.__emit(oslo.make.__options())", "oslo.make") {
        Ok(()) => oslo_runtime::lua::api::make::emitted().unwrap_or_else(|| OPTIONS.to_string()),
        Err(_) => OPTIONS.to_string(),
    }
}

/// What to print when there is no interpreter to ask. Kept in step by a test.
const OPTIONS: &str = "OPTIONS\n  -l, --list        the recipes and what they say they do\n  \
     -n, --dry-run     name every recipe that would run, and run none\n  \
     -f, --force       run even a recipe that is up to date\n  \
     -k, --keep-going  carry on after a recipe fails\n  \
     -q, --quiet       no progress lines, only what the recipes print\n      \
     --watch       watch the resolved recipe inputs and rerun the target\n      \
     --postpone    wait for a change before the first watched run\n      \
     --restart     restart a running watched target after a change\n  \
     -h, --help        this text\n";

#[cfg(test)]
#[path = "make/tests.rs"]
mod tests;
