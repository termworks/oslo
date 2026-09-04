//! The `oslo.*` table: what a Lua program can ask the shell to do.
//!
//! Split from `engine.rs` so the engine owns the Lua state and this owns the surface a script
//! actually sees. Grouped by what each call acts on — commands, variables, the filesystem, the
//! shell itself — because that is how someone writing a script looks for them.
//!
//! The shape of every fallible call is Lua's own: `nil, message` on failure rather than an error,
//! so `local ok, err = oslo.sys.cd(p)` reads the way `io.open` does. A raised error is reserved for a
//! caller mistake (a missing argument, a name that cannot exist), which is a bug in the script
//! rather than a condition it should handle.
//!
//! Nothing here reimplements shell behaviour. `oslo.sys.cd` runs the `cd` builtin and `oslo.glob`
//! calls the shell's own globber, so the two interfaces cannot drift apart.

use crate::lua::engine::{BUILTIN_KEY_PREFIX, PROMPT_KEY, Registry, borrow_env};
use oslo_base::value::LuaError;
use oslo_base::value::{Table, Value};
use oslo_luavm::Host;
use oslo_shell::env::Environment;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

#[cfg(feature = "argc")]
mod args;
mod builtin;
/// `oslo.command` — which programs on `$PATH` this directory can see.
pub mod command;
mod complete;
mod convert;
mod db;
mod digest;
#[cfg(feature = "direnv")]
pub(crate) mod direnv;
mod env;
mod envlist;
pub(crate) mod external;
/// `oslo.feature` — turning parts of the shell off and on while it runs.
pub mod feature;
mod fs;
mod git;
mod handle;
mod history;
mod json;
/// `oslo.live` — the small surface another program may ask a running shell about.
pub mod live;
mod machine;
#[cfg(feature = "make")]
pub mod make;
pub(crate) mod mark;
#[cfg(feature = "math")]
mod math;
mod messages;
#[cfg(feature = "nix")]
mod nix;
mod optional;
mod path;
mod policy;
#[cfg(feature = "vista")]
mod predict;
mod problem;
mod proc;
mod procinfo;
pub(crate) mod prompt;
mod publish;
mod re;
mod rows;
mod run;
#[cfg(feature = "secrets")]
mod secret;
pub(crate) mod segment;
mod shell;
pub(crate) mod spawn;
mod spec;
mod state;
/// `oslo.stream` — a unix socket, as a Lua handle.
pub mod stream;
mod suggest;
mod term;
mod theme;
pub(crate) mod timer;
pub(crate) mod tool;
mod ui;
#[cfg(feature = "watch")]
mod watch;
#[cfg(feature = "watch")]
pub(crate) mod watch_service;
mod word;

pub mod hooks;
pub(crate) use shell::handlers as hook_handlers;
pub(crate) mod util;

use util::{put, text};

/// Build the `oslo` table and install it as a global.
pub fn install(host: &dyn Host, registry: &Registry, env: Arc<Mutex<Environment>>) {
    let mut oslo = Table::new();
    // **Four namespaces, declared before anything fills them.** `oslo` had grown a flat surface of
    // twenty-two names where `nix_develop` sat between `login` and `path_add`, which tells a reader
    // nothing about what belongs with what. `fs`, `json`, `path`, `proc` and `re` were already
    // tables; these are the rest, grouped the same way. They are created here rather than at the
    // point of use because five different builders contribute to them.
    let mut variables_t = Table::new();
    let mut process = Table::new();
    let mut system = Table::new();
    let mut ui = Table::new();
    run::install(host, &mut oslo, &env);
    oslo.set_str("fs", fs::build());
    let mut paths = path::build();
    if let Value::Table(table) = &mut paths {
        prompt::shorten(&mut table.borrow_mut());
    }
    oslo.set_str("path", paths);
    prompt::install(&mut oslo, &mut ui, registry);
    tool::install(&mut oslo);
    // `@` from Lua: the same file the builtin writes, so `@name` has one place to look.
    mark::install(&mut oslo);
    // `oslo.predict.*`, `oslo.repair` and `oslo.last_failed` exist only in a build that has the
    // model. A config guards on them the way it guards on any other optional surface:
    //   if oslo.repair then … end
    #[cfg(feature = "vista")]
    {
        oslo.set_str("predict", predict::build());
        predict::install(&mut oslo, &env);
    }
    // The settings tables exist before the config runs, empty, so that
    // `oslo.completion.max_rows = 5` is an assignment rather than an attempt to index nil. Every
    // one of these is read back after the config by walking the table, so an empty one that the
    // user never touches is indistinguishable from an absent one — which is what makes leaving
    // them here free.
    //
    // **Every settings table, not most of them.** `vi`, `notify` and `dirs` were missing, so
    // `oslo.vi.enabled = false` — the spelling the README documents — died with "attempt to index
    // a nil value" while `oslo.vi = { enabled = false }` worked. A config language where two
    // spellings of the same thing differ by whether somebody remembered to add a line here is one
    // nobody can hold in their head.
    //
    // **A fallback, never a replacement.** This used to assign unconditionally, and `theme` is in
    // the list, so it overwrote the real `oslo.theme` that `prompt::install` had just built one
    // line above — taking `oslo.theme.styles` and its `__newindex` with it. The README's
    // `oslo.theme.styles["my.dir"] = …` therefore died on an empty table, which is the very
    // failure this loop exists to prevent, caused by the fix for it. A namespace that something
    // else already built with behaviour keeps what it has.
    for name in [
        "completion",
        "suggest",
        "lua",
        "history",
        "keys",
        "finder",
        "misc",
        "transcript",
        "vi",
        "notify",
        "dirs",
        "theme",
        "abbr",
        "scratch",
        "macros",
        "table",
    ] {
        if matches!(oslo.get(&Value::str(name)), Value::Nil) {
            oslo.set(
                Value::str(name),
                Value::Table(Rc::new(RefCell::new(Table::new()))),
            );
        }
    }

    // Reading what has been run, merged into the settings namespace of the same name rather than
    // replacing it: `oslo.history` is one subject and should not be two tables. See [`history`].
    extend(&mut oslo, "history", history::build());

    // Painting, merged into the settings table of the same name: a config fills `oslo.theme` with
    // data, and these are the two entries in it that are functions. See [`theme`].
    extend(&mut oslo, "theme", theme::build());

    // `oslo.builtin` is the same thing one level deeper: it is a namespace *per builtin*, so
    // `oslo.builtin.rm.to_tmp = true` indexes twice and both tables have to be here. A new
    // builtin with settings adds a line beside `rm`.
    let mut builtin = Table::new();
    builtin.set(
        Value::str("rm"),
        Value::Table(Rc::new(RefCell::new(Table::new()))),
    );
    builtin.set(
        Value::str("nav"),
        Value::Table(Rc::new(RefCell::new(Table::new()))),
    );
    oslo.set(
        Value::str("builtin"),
        Value::Table(Rc::new(RefCell::new(builtin))),
    );
    // `oslo.completion.for_command` is one level deeper for the same reason: the documented
    // spelling `oslo.completion.for_command.nix = …` indexes twice. Without this it died on
    // "attempt to index a nil value" while `oslo.completion.for_command = { … }` worked — the exact
    // split the settings loop above exists to prevent, one level down where nobody looked.
    if let Value::Table(completion) = oslo.get_str("completion") {
        let missing = matches!(completion.borrow().get_str("for_command"), Value::Nil);
        if missing {
            completion.borrow_mut().set(
                Value::str("for_command"),
                Value::Table(Rc::new(RefCell::new(Table::new()))),
            );
        }
        // `oslo.completion.spec` — the declarative half. A function in the same table as the
        // settings, because that is where somebody looks for anything about completion.
        spec::install(&mut completion.borrow_mut());
        // `oslo.completion.provider` — candidates computed at Tab time, merged with oslo's own.
        complete::install(&mut completion.borrow_mut());
        // `oslo.completion.forget(name)` — the other half, which registration never had.
        complete::forget::install(&mut completion.borrow_mut());
    }
    // `oslo.suggest.provider` — a ghost written in Lua. In the settings table for the same reason:
    // `oslo.suggest.sh_sources` is next to it, and the two are read together.
    if let Value::Table(table) = oslo.get_str("suggest") {
        suggest::install(&mut table.borrow_mut());
    }
    // `oslo.feature` — a namespace of functions rather than a settings table, because a feature is
    // not configuration. It is a runtime mask over configuration, and the two must not look alike.
    oslo.set_str("feature", feature::build(registry));
    // Its sibling: `oslo.feature` gates a builtin from a fixed table, `oslo.command` gates a
    // program on `$PATH` from an open set. Same predicate shape, same moment, same mask.
    oslo.set_str("command", command::build(registry));
    problem::install(host);
    oslo.set_str("db", db::build(host));
    oslo.set_str("state", state::build());
    // The one native the client library needs; the framing and the verbs are plain Lua on top.
    oslo.set_str("stream", stream::build());
    // The other half: what a peer may ask *this* shell. Nothing binds until `serve()` runs.
    oslo.set_str("live", live::build(&env));
    oslo.set_str("messages", messages::build());
    #[cfg(feature = "math")]
    oslo.set_str("math", Value::table(math::build()));
    timer::install(&mut oslo);
    spawn::install(&mut oslo);
    builtin::install(&mut oslo);
    oslo.set_str("json", json::build());
    oslo.set_str("hash", digest::hash());
    oslo.set_str("hex", digest::hex());
    oslo.set_str("base64", digest::base64());
    oslo.set_str("re", re::build());
    oslo.set_str("word", word::build());
    oslo.set_str("proc", proc::build_proc());
    oslo.set_str("job", proc::build_job());

    // The converters, plus `from_json` as an alias for `oslo.json.decode` — the same function
    // under the name the rest of the `from_*` family has.
    let converters = convert::build();
    if let (Value::Table(into), Value::Table(json)) = (&converters, &oslo.get_str("json")) {
        let decode = json.borrow().get_str("decode");
        into.borrow_mut().set_str("from_json", decode);
    }
    if let Value::Table(into) = &converters {
        for (name, f) in into.borrow().pairs() {
            oslo.set(name, f);
        }
    }
    // **Four namespaces, not twenty-two loose names.** `oslo` had grown a flat surface where
    // `nix_develop` sat between `login` and `path_add`, which tells a reader nothing about what
    // belongs with what. `fs`, `json`, `path`, `proc` and `re` were already tables; these are the
    // rest of the surface grouped the same way. What stays on `oslo` itself is only what belongs
    // to no group: `glob`, and the two `register_*` hooks that extend the shell.
    ui::install(&mut ui);
    commands(&mut process, &env);
    env::install(&mut variables_t, &env);
    filesystem(&mut oslo, &mut system, &env);
    shell(
        &mut oslo,
        &mut process,
        &mut ui,
        &mut variables_t,
        registry,
        &env,
    );

    shell::install(&mut oslo, &mut system, &mut process, registry, &env);
    // What the machine can say about itself, for a prompt that would otherwise shell out.
    machine::install(&mut system);

    oslo.set_str("env", Value::table(variables_t));
    extend(&mut oslo, "proc", process);
    oslo.set_str("sys", Value::table(system));
    // What this terminal can do, asked of the terminal itself. See [`term`].
    oslo.set_str("term", Value::table(term::build()));
    // The structured verbs as plain functions. See [`rows`].
    oslo.set_str("rows", rows::build());
    oslo.set_str("ui", Value::table(ui));
    optional::install(&mut oslo, host, &env);
    let oslo = Value::table(oslo);
    host.set_global("oslo", oslo);
    // After the global exists, because these are written in Lua and reach the table through it.
    publish::publish(host);
    #[cfg(feature = "make")]
    make::add_helpers(host);
    #[cfg(feature = "nix")]
    nix::add_helpers(host);
    // **`_VERSION` is the language's, not the VM's.** luna answers `"luna"`, and the tree walker it
    // replaced answered `"Lua 5.4"` — the migration dropped the line, and a script asking the
    // standard question got a name no Lua script has ever been written against. oslo speaks Lua
    // 5.4, `docs/features/lua-interpreter.md` opens by saying so, and the release smoke test has
    // been asking for it ever since.
    host.set_global("_VERSION", Value::str("Lua 5.4"));
    // Last, so it replaces the standard names rather than being replaced by them.
    policy::apply(host);
    // And then the shorthand: `oslo.fs` is also `fs`, `oslo.math.eval` is also `math.eval`.
    //
    // After `policy::apply`, so a name policy replaced is the one that gets lifted. Nothing in
    // `_G` is overwritten — see `flatten_namespace` — so this can only add.
    host.flatten_namespace("oslo");
}
/// Fold everything in `additions` into the table already on `oslo` under `name`.
///
/// `proc` is built by its own module and then extended here, because running a command and
/// signalling one are the same subject arriving from two places. Merged rather than replaced: the
/// module's own entries must survive.
fn extend(oslo: &mut Table, name: &str, additions: Table) {
    match oslo.get(&Value::str(name)) {
        Value::Table(existing) => {
            for (key, value) in additions.pairs() {
                existing.borrow_mut().set(key, value);
            }
        }
        _ => oslo.set(Value::str(name), Value::table(additions)),
    }
}

/// Running shell commands: `exec` for the side effect, `capture` for the output.
fn commands(oslo: &mut Table, env: &Arc<Mutex<Environment>>) {
    // oslo.proc.exec(cmd) -> status. Output goes wherever the shell's output goes.
    let env_exec = Arc::clone(env);
    put(oslo, "exec", move |_, args| {
        let cmd = text(&args, 1, "oslo.proc.exec")?;
        // **A command boundary, so anything spawned that has finished is handed back here.**
        // The read loop delivers between commands, which a *script* never reaches — without this,
        // `oslo.spawn{ on_exit = f }` in a script would silently never call `f`, which is exactly
        // the kind of quiet nothing a callback API must not have. Before the borrow below, because
        // a callback that reaches `oslo.*` would otherwise meet the shell's state already held.
        spawn::deliver_if_any();
        let mut guard = borrow_env(&env_exec)?;
        let ast = oslo_shell::syntax::parse_bash_script(&cmd)
            .map_err(|e| LuaError::new(format!("oslo.proc.exec: {e}")))?;
        let status = oslo_shell::exec::eval_command_list(&mut guard, &ast)
            .map_err(|e| LuaError::new(format!("oslo.proc.exec: {e}")))?;
        Ok(vec![Value::int(status as i64)])
    });

    // oslo.proc.capture(cmd) -> { out = string, status = number }
    //
    // The gap that made Lua unusable for real work: `exec` reports only whether a command
    // succeeded, so anything that needed a command's *answer* — a version string, a device list,
    // the output of `uname` — had to be written in shell instead.
    //
    // A table rather than two return values: `r.out` says what it is at the call site, where
    // `local a, b = oslo.proc.capture(...)` does not, and a table can grow a field later without
    // breaking every caller's positional unpacking.
    //
    // There is deliberately **no `err` field**. This runs the same capture `$(cmd)` does, which
    // takes stdout and leaves stderr attached to the shell's own — so an `err` here could only
    // ever be the empty string, and a field that is always empty reads as "the command printed
    // no diagnostics" rather than "nobody looked". A script that wants them folded together asks
    // for it the way a shell does: `oslo.proc.capture("cmd 2>&1")`.
    //
    // Trailing newlines are stripped, matching `$(cmd)`: the shell's own capture does it, and a
    // Lua script comparing against "x" should not have to remember the command printed "x\n".
    let env_capture = Arc::clone(env);
    put(oslo, "capture", move |_, args| {
        let cmd = text(&args, 1, "oslo.proc.capture")?;
        let mut guard = borrow_env(&env_capture)?;
        let captured = oslo_shell::exec::eval_command_substitution(&mut guard, &cmd);
        // The substitution records its own child's status separately — `last_status` is the
        // shell's, which a capture does not touch, so reading that gave every command 0.
        let status = guard
            .take_substitution_status()
            .unwrap_or(guard.last_status);
        drop(guard);

        let mut result = Table::new();
        match captured {
            Ok(out) => {
                result.set_str("out", Value::str(out.trim_end_matches('\n')));
                result.set_str("status", Value::int(status as i64));
            }
            Err(e) => {
                result.set_str("out", Value::str(""));
                result.set_str("status", Value::int(e.failure_status() as i64));
            }
        }
        Ok(vec![Value::table(result)])
    });
}

/// The working directory and pathname expansion.
fn filesystem(oslo: &mut Table, system: &mut Table, env: &Arc<Mutex<Environment>>) {
    put(system, "pwd", |_, _| {
        Ok(vec![Value::str(
            std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy(),
        )])
    });

    // oslo.sys.cd(path) -> true, or nil + message.
    //
    // Goes through the `cd` builtin rather than `std::env::set_current_dir` so that `$PWD`,
    // `$OLDPWD` and the directory stack agree with it afterwards — a Lua script that moved the
    // process without telling the shell would leave `pwd` reporting the old directory.
    let env_cd = Arc::clone(env);
    put(system, "cd", move |_, args| {
        let path = text(&args, 1, "oslo.sys.cd")?;
        let mut guard = borrow_env(&env_cd)?;
        let argv = vec!["cd".to_string(), path.clone()];
        Ok(
            match oslo_shell::env::builtins::builtin_cd(&mut guard, &argv) {
                Ok(0) => vec![Value::Bool(true)],
                Ok(_) => vec![
                    Value::Nil,
                    Value::str(format!("cd: {path}: cannot change directory")),
                ],
                Err(e) => vec![Value::Nil, Value::str(e.to_string())],
            },
        )
    });

    // oslo.glob(pattern) -> { "a", "b", ... }, in the shell's own sorted order.
    //
    // Lua's standard library has no pathname expansion at all, so listing "every .conf in here"
    // meant shelling out. An empty table for no matches, never the pattern itself: returning the
    // unmatched pattern is a shell convention that has surprised people for forty years, and Lua
    // code checking `#matches == 0` is what a caller will naturally write.
    put(oslo, "glob", |_, args| {
        let pattern = text(&args, 1, "oslo.glob")?;
        // One unquoted run, which is what makes every metacharacter in the string live — the
        // caller passed a pattern, not a word that might have been quoted.
        let field = [oslo_shell::expand::Run::new(
            pattern.clone(),
            oslo_shell::expand::Origin::Literal,
        )];
        let matches = oslo_shell::expand::glob::expand_glob(&field);
        // `expand_glob` yields the pattern back when nothing matched, the way an unquoted word
        // does in a command line. Here that would be a lie, so it becomes an empty table.
        let matches = if matches == vec![pattern] {
            Vec::new()
        } else {
            matches
        };
        let mut table = Table::new();
        for (i, path) in matches.into_iter().enumerate() {
            table.set(Value::int(i as i64 + 1), Value::str(path));
        }
        Ok(vec![Value::table(table)])
    });
}

/// Aliases, builtins, the prompt, and the shell's exit status.
fn shell(
    oslo: &mut Table,
    process: &mut Table,
    ui: &mut Table,
    names: &mut Table,
    registry: &Registry,
    env: &Arc<Mutex<Environment>>,
) {
    // `oslo.source(path)` — run a **shell** file in this shell.
    //
    // The config is Lua, but a person's `~/.profile` and their pile of `aliases.sh` are shell and
    // are shared with every other shell they use. Rewriting them in Lua is not an answer; being
    // able to read them is.
    //
    // Sourced, not run: `oslo.run{"sh", path}` would execute the file in a *child*, where every
    // alias, export and function it defines dies with the process. This is the same code path as
    // the `source` builtin, so it lands in the environment the prompt actually uses.
    //
    // Answers `true`, or `false` and a message — a missing `~/.profile` is the ordinary case on a
    // fresh machine and must not stop the rest of the config from loading.
    // `oslo.quote(word)` — one word, safe to paste back into a command line.
    //
    // **The other half of `c.commands`.** Reading a line as data is only useful if it can be
    // written back, and a handler that rebuilds one by concatenating words is wrong the moment a
    // path has a space in it: `cat 'a dir'` parses to the single word `a dir`, and returning
    // `"ls " .. word` hands the shell two arguments and an error. That is not a hypothetical —
    // it is what the first configuration written against `c.commands` did.
    //
    // The same quoting `printf %q` produces, because there should be one answer to "how does this
    // shell write a word" and it should not depend on which half of the shell is asking.
    put(oslo, "quote", |_, args| {
        let word = text(&args, 1, "oslo.quote")?;
        Ok(vec![Value::str(oslo_shell::env::builtins::shell_quote(
            &word,
        ))])
    });

    let env_source = Arc::clone(env);
    put(oslo, "source", move |_, args| {
        let path = text(&args, 1, "oslo.source")?;
        let mut guard = borrow_env(&env_source)?;
        match oslo_shell::env::builtins::builtin_source(
            &mut guard,
            &["source".to_string(), path.clone()],
        ) {
            Ok(0) => Ok(vec![Value::Bool(true)]),
            Ok(status) => Ok(vec![
                Value::Bool(false),
                Value::str(format!("{path}: exited {status}")),
            ]),
            Err(e) => Ok(vec![Value::Bool(false), Value::str(e.to_string())]),
        }
    });

    let env_alias = Arc::clone(env);
    put(names, "set_alias", move |_, args| {
        let name = text(&args, 1, "oslo.env.set_alias")?;
        let target = text(&args, 2, "oslo.env.set_alias")?;
        borrow_env(&env_alias)?.set_alias(&name, &target);
        Ok(Vec::new())
    });

    let env_get_alias = Arc::clone(env);
    put(names, "alias", move |_, args| {
        let name = text(&args, 1, "oslo.env.alias")?;
        Ok(vec![match borrow_env(&env_get_alias)?.get_alias(&name) {
            Some(target) => Value::str(target),
            None => Value::Nil,
        }])
    });

    // oslo.proc.exit(code) -> never returns.
    //
    // A Lua program's own way to decide what the shell exits with. Without it the only way to
    // choose an exit status was `oslo.proc.exec("exit 3")`, which is a shell command pretending to be
    // a language feature. `os.exit` would leave the shell's own cleanup — the EXIT trap, the
    // flush — unrun, so this goes through the shell's exit path instead.
    put(process, "exit", |_, args| {
        let code = match args.first() {
            Some(v) => v.as_number().map(|n| n.as_float() as i32).unwrap_or(0),
            None => 0,
        };
        Err(LuaError::exit_request(code))
    });

    // oslo.register_builtin(name, callback)
    // oslo.register_builtin{ name = …, run = …, desc = …, complete = … }
    //
    // The callback is stored and run (PLAN R9.8). Until that round it was *dropped* and a stub
    // registered under the name instead, which is worse than doing nothing:
    // `oslo.register_builtin('ls', …)` made `ls /` print nothing and exit 0.
    let env_builtin = Arc::clone(env);
    let registry_builtin = Rc::clone(registry);
    put(oslo, "register_builtin", move |host, args| {
        let declared = builtin::declaration(&args)?;
        // Stored first: if the shell registration fails there is no name in the registry
        // pointing at a callback that is not there.
        registry_builtin.borrow_mut().insert(
            format!("{BUILTIN_KEY_PREFIX}{}", declared.name),
            declared.run.clone(),
        );
        builtin::remember(host, &declared);

        let mut guard = borrow_env(&env_builtin)?;
        let key = declared.name.clone();
        // **`env` is used, not discarded.** It was `|_env, args|` — a live, exclusive
        // `&mut Environment` bound and thrown away, while the Lua body reached back for the same
        // state through a lock the frame above was already holding. `register_dynamic_builtin`
        // clones the callback out of the registry precisely so this closure can hold one; its own
        // comment says so. The capability was designed in and never used.
        let wants = declared.wants;
        guard.register_dynamic_builtin(&declared.name, move |env, args| {
            Ok(builtin::call::call(env, &key, wants, args))
        });
        Ok(Vec::new())
    });

    let registry_prompt = Rc::clone(registry);
    put(ui, "prompt", move |_, args| {
        let Some(f @ Value::Function(_)) = args.first() else {
            return Err(LuaError::new(
                "oslo.ui.prompt: the argument must be a function",
            ));
        };
        registry_prompt
            .borrow_mut()
            .insert(PROMPT_KEY.to_string(), f.clone());
        Ok(Vec::new())
    });

    // Right prompts are configured through `oslo.prompt.right`.
}
