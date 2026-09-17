# Build recipes

A `.make.lua` in a project, and `make build` runs a recipe out of it. Dependencies, parameters, and
a rule for when work can be skipped — a `justfile` or a `Makefile`, except that it is not a small
language invented to avoid writing a program. oslo already has a program: the config is Lua, the
directory environment is Lua, and the shell can run a command without a `/bin/sh` between it and the
argv. So the third file is the same language, pointed at builds.

```lua
-- .make.lua
local make = oslo.make

make.recipe{
  name    = "build",
  desc    = "the static release binary",
  deps    = { "fmt-check" },
  inputs  = { "src/**/*.rs", "Cargo.toml" },
  outputs = { "target/release/app" },
  stale   = "content",
  params  = { { "--type", desc = "minimal | full", default = "full" } },
  run = function(a)
    local features = a.type == "minimal" and {} or { "--all-features" }
    sh.cargo("build", "--release", table.unpack(features))
  end,
}

make.alias("b", "build")
```

```console
$ make                       # no recipe: list them
build  the static release binary
b      → build

$ make build --type minimal
→ fmt-check
→ build

$ make build                 # nothing changed
· fmt-check
· build  up to date
```

> ## This is in `oslo`, not in `oslo-minimal`
>
> Behind the **`make`** cargo feature, which is off by default. Without it there is no `oslo.make`,
> no `oslo make` tool and no `make` builtin — so the word falls through to `$PATH` and GNU make
> answers, which is what it does on every other shell.
>
> It reads a file in the working directory and runs what it finds. That it only does so **when you
> ask** is what makes it safer than [directory environments](directory-environments.md), not what
> makes it free.

## How it works

```text
 make build              at a prompt: the builtin — env/builtins/make.rs
   ├── not interactive, or no .make.lua here ──► /usr/bin/make, unchanged
   ▼
 oslo make build         a child process — src/cli/make.rs
   ▼
 make::governing(cwd)    walk up, nearest ancestor holding .make.lua
   ▼
 chdir to its directory  make's rule: a recipe resolves `src/` against the project
   ▼
 engine + init.lua + bindings        so oslo.make exists before the file mentions it
   ▼
 .env.lua, if allowed    the directory environment, so a recipe runs in what it declares
   ▼
 load .make.lua          declarations only — every body is a function, nothing runs
   ▼
 oslo.make.__main()      parse argv, plan the graph, run it, set the exit status
```

### Why it is a child process, and not a builtin

This is the decision the whole feature is shaped around, and it was forced by a measurement.

A builtin registered from Lua runs **while the shell holds its own state**, so every call that
reaches the shell fails from inside one:

```text
run RAISED   -> shell state is busy; an oslo.* call that reaches the shell cannot run from here.
sh RAISED    -> shell state is busy; …
lines RAISED -> shell state is busy; …
```

A build runner that cannot run a command is not a build runner — and running commands is precisely
what the lock covers. It is **twenty-eight calls, not the whole API**: `oslo.fs`, `oslo.json`,
`oslo.db`, `oslo.hash`, `oslo.git` and the rest work perfectly well inside a builtin, and
[your own tools](your-own-tools.md) has the full table. But the three above are the three a recipe
is made of, so for *this* feature the distinction changes nothing.

[Directory environments](directory-environments.md) get around the same lock by taking it, snapshotting,
releasing it and running the file — but `direnv::arrive` is called from `startup/repl.rs`
*between commands*, at the one moment the shell holds nothing. A builtin has no such moment: it is
called from inside dispatch, in the middle of the state it would have to release.

So `oslo make` is a separate process, which is what `oslo macros`, `oslo hook` and `oslo secret`
already are: a fresh `oslo` with its own engine, holding no interactive state, where the whole API
simply works.

What that costs is what `make` and `just` already cost — **a recipe cannot `cd` the shell that
called it, or set a variable in it.** That is the semantics of a recipe, not a limitation of this.

**What it does get is the directory's own environment.** `oslo make` evaluates the project's
`.env.lua` — allow list and all — after the config and before `.make.lua` is read, so a recipe
resolves what the directory declares. Without it a recipe ran in whatever the calling shell was
holding: the interactive session's load from whenever it last entered the project, or nothing at
all when the command was typed from somewhere else.

### The `make` builtin gets out of the way

`make` is a real program that a great many projects are built with, and shadowing it would be the
mistake this codebase already refuses for `test`. The precedent is `which`, which answers about
*this* shell at a prompt and hands the word to `/usr/bin/which` everywhere else. Three conditions,
and all must hold:

1. **The shell is interactive.** In a script, `make` is the program the script was written for.
2. **A `.make.lua` governs the working directory.** No file, no claim on the name.
3. `\make` and `command make` reach the program, as they do for every builtin.

A project with a `Makefile` *and* a `.make.lua` gets the Lua one at a prompt: a repository holding
both usually has one of them for everybody else.

`type make` answers *make is a shell builtin*, and `command -v make` exits 0. That is not
decoration. `your-own-tools.md` lists *"tools are invisible to everything that answers questions
about names"* as a limitation of `register_tool`; there was no reason to ship that hole twice.

### Nearest ancestor, and nothing merged

The walk is direnv's, verbatim: up from the working directory, take the first `.make.lua`. Nearest
wins outright — two files on one path means the inner one governs and the outer one does not, so
what `make build` does never depends on how deep in the tree you were standing.

### Strict `sh`, and only inside a recipe

`oslo.run` deliberately never raises: *a command that fails is not an exceptional event in a shell,
it is Tuesday.* That is right at a prompt and wrong in a build, where make's rule — stop at the
first non-zero — is the only safe default.

So the runner swaps `sh` for a strict one while a recipe runs, and swaps it back on the way out.
Inside a recipe `sh.cargo(…)` raises on a non-zero status and the message
names the command; `oslo.run{…}` keeps its ordinary manners for the caller who wants to read
`r.status` themselves.

```lua
sh.cargo("build")                              -- raises: cargo exited 101
local r = oslo.run{ "cargo", "build" }         -- answers: r.ok is false
```

**A command oslo answers in rows has no status to check.** `sh.ls(…)` and `sh.df(…)` give a list of
rows rather than a result — see [structured pipelines](structured-pipelines.md) — so strict `sh`
cannot fail on them, and a recipe that needs the status of one writes `oslo.run{…}`.

## Staleness — the part `just` does not have

A recipe with no `outputs` is **phony**: it always runs, which is the default, and is the inverse of
make's `.PHONY`. One that declares `outputs` is skipped when it is up to date.

| `stale` | the question | cost |
|---|---|---|
| `"mtime"` *(default)* | is any input newer than any output? | one `stat` per file |
| `"content"` | do the inputs hash to what they hashed to last time? | one read per file |

Mtime is compared to the **nanosecond**, and an output must be **strictly newer** than every input.
Both halves are needed and each was a wrong build on its own. At second resolution an input edited
and an output written inside the same second compare equal, so a recipe that had not been rebuilt
reported `up to date` — and the fast inner loop is exactly where a build finishes inside one second.
Nanoseconds alone do not fix it either, because the *filesystem* decides the resolution: tmpfs
stamps to the jiffy, a few milliseconds, so two writes a fraction of a millisecond apart come back
byte-identical. Equal therefore counts as stale. That costs a spurious rebuild when a build
genuinely finishes inside one tick and buys never shipping a stale artifact.

`oslo.fs.stat` gained `mtime_ns` for this — a whole timestamp, not a sub-second remainder, so two
files compare with one `<`. `mtime` stays seconds, which is what a person prints.

`"content"` is the reason to have this at all. A `git checkout` moves every mtime in the tree, so
mtime staleness rebuilds the world after switching branches and back; a content hash does not.
The fingerprint is kept in [`oslo.db`](plugins.md), under a key covering the project, the recipe and
its arguments — so `make build --type minimal` after `make build` is not mistaken for a no-op.

**The stamp is written after the body returns, never during the check.** The first version recorded
the inputs up front, so a build that *failed* reported "up to date" on the next run — which is the
one answer a build tool must never give. `--force` runs a recipe regardless.

A recipe declaring `outputs` and no `inputs` is refused when the file is read: it could never be up
to date, and saying so beats a build that silently reruns for ever.

## Configuration

Everything is `oslo.make`, and a `.make.lua` is the only file that calls it.

```lua
make.settings{ quiet = false, keep_going = false, stale = "mtime" }

make.recipe{
  name    = "build",       -- required; a leading `_` keeps it out of the listing
  desc    = "…",           -- the second column of `make` with no argument
  deps    = { "a", "b" },  -- run first, each once per invocation
  inputs  = { "src/**" },  -- globs; `**` walks
  outputs = { "out.bin" }, -- declaring any of these makes the recipe skippable
  stale   = "content",     -- or "mtime"
  quiet   = true,          -- no `→ name` line of its own
  params  = { { "--who", desc = "…", default = "world" } },
  run     = function(args) … end,
}

make.alias("b", "build")
make.import("tools/.make.lua")     -- another file's recipes, in this graph
make.run("check-static")           -- one recipe from inside another, memoised
make.names()                       -- the declared names, as data
```

`run` is handed one table: declared parameters with their defaults filled in, any `--flag value`,
`--flag=value` or bare `--flag` from the command line, and `args.rest` for everything that was not a
flag. An undeclared flag arrives too — a recipe passing its arguments through should not have to
declare them first.

```
  -l, --list        the recipes and what they say they do
  -n, --dry-run     name every recipe that would run, and run none
  -f, --force       run even a recipe that is up to date
  -k, --keep-going  carry on after a recipe fails
  -q, --quiet       no progress lines, only what the recipes print
      --watch       watch the resolved recipe inputs and rerun the target
      --postpone    wait for a change before the first watched run
      --restart     restart a running watched target after a change
  -h, --help        this text
```

### Watching the resolved plan

With the independent [`watch`](watch.md) feature, `oslo make --watch check` resolves `check` and its
dependencies without running any body. The watcher receives the deduplicated declared input
patterns from that plan, plus `.make.lua` and each successful `make.import`. Patterns are retained
as patterns, so a file created later can satisfy `src/**/*.rs`; only expanding the files that exist
at startup would miss it.

The worker reruns `/proc/self/exe make TARGET ARGS...` without `--watch` and without `--force`.
Arguments remain exact argv and ordinary phony/staleness rules decide what executes. `--postpone`
suppresses the initial run and `--restart` replaces a still-running process group on a change. A
plan without any declared `inputs` is refused instead of silently watching the whole repository.

The trust boundary has not moved: Oslo reads `.make.lua` only after an explicit `oslo make`
invocation. Changing directory does not load it, and watch planning does not execute recipe bodies.

## What makes it different

`just` invented a language, and everything a language has to grow is grafted on: `{{interpolation}}`
because there are no expressions, `set shell := […]` because there is no way to run a command
directly, a `[private]` attribute because there is no scope. Here a variable is a `local`, a
condition is an `if`, and a list of exclusions is a list.

`make` has the one thing `just` gave up — knowing whether work can be skipped — and it decides with
mtimes, which is why a fresh checkout rebuilds the world. `stale = "content"` is the same idea with
the failure mode removed.

Both shell out through `/bin/sh`, so every recipe is one careless value away from word-splitting.
`sh.rm(name)` is argv end to end: there is no quoting step, so there is no quoting bug.

The reference comparison was oslo's own build: a `Makefile` and a `.make.lua` producing the same
binary, the second with no `$$`, no backslash continuations, no `.PHONY` list and no hand-maintained
`help:` target repeating what the recipes already say. **The `Makefile` has since been deleted** —
two build files in one tree meant every change had to be made twice, and the second one rots.

What replaced it is not another build tool. `scripts/build.sh` is a bootstrap and nothing else:
cargo, one binary, stop. It exists because this file is run by the `make` builtin, which lives
inside the shell it builds — a fresh checkout, a CI runner and a distribution packager all arrive
with cargo and no oslo, and none of them can start here.

## What it cannot do

- **No `-j` flag** — but a recipe can now run things in parallel itself. `oslo.spawn` used to be
  silently useless here: it delivers at a safe point, `oslo make` has no read loop, and nothing ever
  drained the queue, so the callback never ran and the recipe reported success anyway. A make run
  now installs a servicer of its own, and two calls do the waiting:

  ```lua
  oslo.make.recipe{ name = "build-all", run = function()
    for _, crate in ipairs(CRATES) do
      oslo.spawn{ "cargo", "build", "-p", crate,
        on_exit = function(out, status) if status ~= 0 then io.write(out) end end }
    end
    local r = oslo.settle{ timeout_ms = 600000 }
    assert(r.settled, r.outstanding .. " builds did not finish in time")
  end }
  ```

  `job:wait([timeout_ms])` answers `out, status` for one spawn, or `nil, why`. `oslo.settle` waits
  for all of them and answers `{ fired, outstanding, settled }`. Both block on the same descriptors
  the line editor polls, so they return the moment a worker finishes rather than on a poll interval.
  What is still missing is oslo *scheduling* the dependency graph across cores for you.
- **No pattern rules.** `%.o: %.c` is expressible as a recipe that declares recipes, and generating
  them at load time works, but there is nothing built in.
- **A recipe cannot change the shell that called it.** It is a child. `cd`, `export` and setting a
  shell variable affect the recipe and nothing else — make's semantics and just's.
- **No completion of recipe names yet.** Reading the names means evaluating the file, and doing that
  on a Tab press is arbitrary execution on a keystroke. The declared names are data
  (`make.names()`), so the cache this needs is possible; it is not built.
- **`make` is a builtin only in an interactive shell.** In a script, in `oslo -c`, and in a
  `#!/bin/oslo` file the word is the program. `oslo make` works everywhere.
- **A row-answering command cannot fail a recipe.** Strict `sh` checks a status, and `sh.ls(…)`
  answers with rows instead. `oslo.run{…}` is the way to check one.
- **No `--fmt`, no `--dump`.** It is Lua.
- **There is no converter from a `Makefile`.** oslo's own was deleted rather than translated, by
  hand, once.
- **It cannot build the shell it runs in.** The `make` builtin is inside oslo, so a machine without
  oslo cannot reach a recipe. That first build is `scripts/build.sh`, and keeping it to exactly one
  job — cargo, one binary — is what stops it growing back into the second build file.

## Where it lives

| path | what |
|---|---|
| `crates/oslo-shell/src/make.rs` | `governing`, `root_of`, `NAME` — which file, and nothing else |
| `crates/oslo-shell/src/env/builtins/make.rs` | the `make` builtin, and the handover to the program |
| `crates/oslo-runtime/src/lua/api/make.rs` | `__argv`, `__file`, `__root`, `__status`, `__emit`, `__relative` |
| `crates/oslo-runtime/src/lua/api/make.lua` | recipes, the graph, staleness, parameters, the listing |
| `src/cli/make.rs` | `oslo make`: find, chdir, boot the engine, run |
| `src/cli/tools.rs` | the row that makes `oslo make` reachable |
| `tests/make_tests.rs` | the end-to-end suite, one temporary project per case |
| `tests/make_cli_tests.rs` | dependency inputs, imports, future files, argv and no-body watch planning |
| `scripts/build.sh` | the bootstrap: cargo, one static binary, for a machine with no oslo |
| `.make.lua` | oslo's own build, and the worked example this page describes |
| `.make.lua` | oslo's own build, as recipes |
