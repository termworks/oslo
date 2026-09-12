//! The `oslo.*` surface: what is there, and which of it the shell's own lock keeps out.
//!
//! # What this exists to catch
//!
//! `tests/lua_surface_tests.rs` walks the Lua 5.4 standard library and asserts nothing on it is
//! `nil`. Nothing did the same for `oslo.*` — the namespace that is the entire reason this shell
//! has a config language — so two declared-but-broken shapes sat there unnoticed:
//!
//! * `oslo.register_tool{ accepts = "bytes" }` was accepted by the validator, made the planner
//!   read standard input, and then had its bytes dropped one call short of the handler, because
//!   `data::custom::Handler` had no parameter to carry them.
//! * `oslo.lines{"cargo","build"}` saw nothing, because cargo writes to stderr and the binding
//!   called the non-merging sibling of a function whose merging form already existed.
//!
//! Both are the same failure: a promise in the surface that nothing walked.
//!
//! # The other half, which is the more valuable one
//!
//! **The busy-state lock is 28 functions, not a wall.** `docs/features/your-own-tools.md` said
//! *"Nothing inside a tool or a registered builtin may reach the shell"*, and that sentence is why
//! several capabilities were believed unreachable that were reachable all along: `borrow_env` — the
//! only thing that takes the lock — appears in seven of forty-six api files.
//!
//! So the second half of this file runs a registered builtin and asserts, for real, which calls
//! work from inside one and which raise. A namespace that grows a `borrow_env` it did not have
//! fails here rather than in somebody's config.

mod common;

use common::oslo_bin;
use std::process::{Command, Stdio};

/// Run a Lua chunk through the real binary and answer with everything it printed.
fn lua(source: &str) -> String {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("probe.lua");
    std::fs::write(&file, source).expect("write");
    let out = Command::new(oslo_bin())
        .arg(&file)
        .env("HOME", dir.path())
        .env("XDG_DATA_HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join("config"))
        .stdin(Stdio::null())
        .output()
        .expect("spawn oslo");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// Every namespace on `oslo`, and what it must be.
///
/// Feature-gated ones are absent from a default build and are checked separately — a config asks
/// `if oslo.nix then`, which is the documented contract in `docs/features/runtime-features.md`.
const NAMESPACES: &[(&str, &str)] = &[
    ("oslo.db", "table"),
    ("oslo.env", "table"),
    ("oslo.feature", "table"),
    ("oslo.fs", "table"),
    ("oslo.git", "table"),
    ("oslo.hash", "table"),
    ("oslo.hex", "table"),
    ("oslo.history", "table"),
    ("oslo.job", "table"),
    ("oslo.json", "table"),
    ("oslo.messages", "table"),
    ("oslo.opts", "table"),
    ("oslo.path", "table"),
    ("oslo.proc", "table"),
    ("oslo.re", "table"),
    ("oslo.rows", "table"),
    ("oslo.state", "table"),
    ("oslo.sys", "table"),
    ("oslo.term", "table"),
    ("oslo.theme", "table"),
    ("oslo.ui", "table"),
    ("oslo.word", "table"),
    #[cfg(feature = "watch")]
    ("oslo.watch", "table"),
    ("oslo.run", "function"),
    ("oslo.pipe", "function"),
    ("oslo.lines", "function"),
    ("oslo.spawn", "function"),
    ("oslo.settle", "function"),
    ("oslo.after", "function"),
    ("oslo.every", "function"),
    ("oslo.register_builtin", "function"),
    ("oslo.register_tool", "function"),
    ("oslo.version", "string"),
];

#[test]
fn every_namespace_is_there_and_is_what_it_claims() {
    let checks: String = NAMESPACES
        .iter()
        .map(|(path, _)| format!("print('{path}=' .. type({path}))\n"))
        .collect();
    let said = lua(&checks);
    for (path, want) in NAMESPACES {
        assert!(
            said.contains(&format!("{path}={want}")),
            "{path} is not a {want}\n{said}"
        );
    }
}

/// A representative function from each namespace, so a table that survives while its contents are
/// renamed still fails. Chosen to be the ones other tests and the docs actually name.
const FUNCTIONS: &[&str] = &[
    "oslo.fs.stat",
    "oslo.fs.glob",
    "oslo.fs.read",
    "oslo.fs.write",
    #[cfg(feature = "watch")]
    "oslo.fs.watch",
    #[cfg(feature = "watch")]
    "oslo.watch.start",
    "oslo.path.join",
    "oslo.path.parent",
    "oslo.json.decode",
    "oslo.json.encode",
    "oslo.re.match",
    "oslo.re.replace",
    "oslo.hash.sha256",
    "oslo.hash.file",
    "oslo.db.open",
    "oslo.state.get",
    "oslo.state.set",
    "oslo.env.get",
    "oslo.env.set",
    "oslo.env.all",
    "oslo.env.snapshot",
    "oslo.completion.forget",
    "oslo.env.path_add",
    "oslo.proc.status",
    "oslo.proc.exec",
    "oslo.proc.parse",
    "oslo.rows.where",
    "oslo.rows.sort_by",
    "oslo.rows.render",
    "oslo.rows.from_json",
    "oslo.word.braces",
    "oslo.word.matches",
    "oslo.git.root",
    "oslo.git.branch",
    "oslo.ui.width",
    "oslo.ui.ask",
    "oslo.ui.match",
    "oslo.ui.match_at",
    "oslo.ui.rank",
    "oslo.ui.wrap",
    "oslo.ui.title",
    "oslo.ui.subtitle",
    "oslo.theme.style",
    "oslo.theme.define",
    "oslo.theme.depth",
    "oslo.sys.cd",
    "oslo.opts.get",
    "oslo.term.size",
    "oslo.messages.error",
];

#[test]
fn no_advertised_function_is_missing() {
    let checks: String = FUNCTIONS
        .iter()
        .map(|path| format!("print('{path}=' .. type({path}))\n"))
        .collect();
    let said = lua(&checks);
    let missing: Vec<&&str> = FUNCTIONS
        .iter()
        .filter(|path| !said.contains(&format!("{path}=function")))
        .collect();
    assert!(missing.is_empty(), "not functions: {missing:?}\n{said}");
}

/// The shorthand `oslo.flatten_namespace` installs — `oslo.fs` is also `fs`.
///
/// It can only ever *add*: a name already in `_G` is left alone. So this asserts the lift happened,
/// not that the two are the same object.
#[test]
fn the_namespaces_are_also_reachable_unqualified() {
    let said =
        lua("print('fs=' .. type(fs)) print('json=' .. type(json)) print('re=' .. type(re))");
    for want in ["fs=table", "json=table", "re=table"] {
        assert!(said.contains(want), "{want} missing\n{said}");
    }
}

// ───────────────────────────────────────────────────────────── the lock boundary

/// Run `body` inside a registered builtin and answer what it printed.
///
/// A registered builtin is one of the two places the shell holds its own state while Lua runs — the
/// other being an answering hook — so it is where `borrow_env` refuses. `oslo.proc.exec` is how the
/// builtin gets invoked from the same chunk that declared it.
fn inside_a_builtin(body: &str) -> String {
    lua(&format!(
        r#"
oslo.register_builtin{{ name = "probe", run = function(argv, shell)
{body}
  return 0
end }}
oslo.proc.exec("probe")
"#
    ))
}

/// What a builtin *can* reach. Every one of these is a namespace with no `borrow_env` in it.
///
/// This is the list the documentation denied existed. If one of these starts raising, a namespace
/// grew a lock and the docs need the same correction again.
#[test]
fn most_of_the_api_works_from_inside_a_builtin() {
    let said = inside_a_builtin(
        r#"
  local function try(label, f)
    local ok, err = pcall(f)
    print(label .. "=" .. (ok and "works" or "FAILS"))
  end
  try("fs",    function() assert(oslo.fs.stat("/etc/hostname").size) end)
  try("glob",  function() assert(#oslo.fs.glob("/etc/host*") >= 1) end)
  try("json",  function() assert(oslo.json.decode('{"a":1}').a == 1) end)
  try("hash",  function() assert(oslo.hash.sha256("x")) end)
  try("state", function() oslo.state.set("k", "v"); assert(oslo.state.get("k") == "v") end)
  try("db",    function() local d = oslo.db.open("surface"); d:set("k", "v"); assert(d:get("k") == "v") end)
  try("path",  function() assert(oslo.path.join("a", "b") == "a/b") end)
  try("ui",    function() assert(oslo.ui.width()) end)
  try("git",   function() local _ = oslo.git.root() end)
  try("word",  function() assert(#oslo.word.braces("{a,b}") == 2) end)
  try("parse", function() assert(oslo.proc.parse("a | b")[2].link == "|") end)
  try("match", function() assert(oslo.ui.match("git checkout", "gco")) end)
  try("rank",  function() assert(oslo.ui.rank({"git checkout","zzz"}, "gco")[1].text) end)
  try("wrap",  function() assert(#oslo.ui.wrap("a b c d", 3) > 1) end)
  try("style", function() assert(oslo.theme.style("x", "fg:red")) end)
  -- Pure computation over records, which is why the pipeline verbs reach in here at all. `where` is
  -- the interesting one: it evaluates Lua per row through the engine already running, so this is a
  -- re-entry. See `tests/rows_verb_tests.rs`.
  try("rows",  function() assert(oslo.rows.length({{a=1}}) == 1) end)
  try("where", function() assert(#oslo.rows.where({{a=1},{a=9}}, "a > 5") == 1) end)
"#,
    );
    for name in [
        "fs", "glob", "json", "hash", "state", "db", "path", "ui", "git", "match", "rank", "wrap",
        "style", "rows", "where",
    ] {
        assert!(
            said.contains(&format!("{name}=works")),
            "{name} no longer works from a builtin — did it grow a borrow_env?\n{said}"
        );
    }
}

/// And what it cannot: the calls that take the lock.
///
/// **`oslo.proc.status` is still on this list, and no longer matters.** It is `$?`, and a builtin
/// could not reach it by any route — the `oslo.*` call raised and the Lua global answered nil. The
/// call still refuses, because it still takes the lock; what changed is that a builtin is now
/// *handed* the answer as `shell.status` and has no reason to ask. See
/// `tests/lua_builtin_effects_tests.rs`.
#[test]
fn the_locked_calls_refuse_from_inside_a_builtin() {
    let said = inside_a_builtin(
        r#"
  local function try(label, f)
    local ok, err = pcall(f)
    print(label .. "=" .. (ok and "works" or "refused"))
  end
  try("run",     function() oslo.run{"true"} end)
  try("env_get", function() oslo.env.get("HOME") end)
  try("env_set", function() oslo.env.set("SURFACE", "1") end)
  try("status",  function() oslo.proc.status() end)
  try("path_add",function() oslo.env.path_add("/tmp") end)
"#,
    );
    for name in ["run", "env_get", "env_set", "status", "path_add"] {
        assert!(
            said.contains(&format!("{name}=refused")),
            "{name} no longer refuses — if that is deliberate, this test records the old rule\n{said}"
        );
    }
}

/// The refusal says which call and what to do, not just that something went wrong.
#[test]
fn the_refusal_names_itself() {
    let said = inside_a_builtin(
        r#"
  local ok, err = pcall(function() oslo.run{"true"} end)
  print("message=" .. tostring(err))
"#,
    );
    assert!(said.contains("shell state is busy"), "{said}");
    assert!(
        said.contains("register_builtin"),
        "the message should name the thing the caller wrote\n{said}"
    );
}
