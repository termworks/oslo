# Watch services

`oslo watch` runs an exact command whenever a file set changes. It uses Linux inotify directly,
keeps one command process group at a time, and can put the worker in a [Scratch](scratch.md) so its
output remains attachable after the launching terminal leaves.

```sh
oslo watch 'src/**/*.rs' Cargo.toml -- cargo check
oslo watch --postpone --restart --name api 'src/**/*.rs' -- cargo run
oslo watch --foreground config.toml -- sh -c 'echo changed: "$PWD"'
```

The `--` is mandatory. Everything after it is argv; shell syntax is interpreted only when the
command explicitly names a shell such as `sh -c`.

## How it works

One blocking worker owns the inotify descriptors and the watched child:

```text
CLI / .env.lua / .make.lua
             │ WatchSpec
             ▼
     inotify + poll loop ──► child process group
             │
             └────────────► optional Scratch pty and replay log
```

There is no periodic directory scan while idle. A queue overflow, removed descriptor, or unmounted
watch root rebuilds the complete descriptor set and schedules a run rather than silently losing a
change. New directories below a recursive root are installed as they appear; the initial scan does
not follow symlinked directories.

Literal files are watched through their parent, so an editor replacing a file by rename is seen. A
literal directory matches its direct entries. `*`, `?`, and bracket classes stay within one path
component; `**` crosses zero or more components. Wildcards do not include hidden components unless
that component begins with `.` in the pattern. Relative paths are fixed against the launch
directory once, and there are no implicit exclusions.

## Process policy

The default `--coalesce` policy permits one child and one dirty bit. Changes during a run cause one
more run after that child exits, regardless of how many changes arrived. A failing child is reported
and watching continues.

`--restart` sends `SIGTERM` to the complete child process group after a debounced change, waits
`--grace` milliseconds, sends `SIGKILL` if required, reaps it, and then starts the replacement.
`SIGINT`, `SIGTERM`, and `SIGHUP` received by the watcher perform the same group cleanup before the
watcher exits. Child stdin is `/dev/null`; stdout and stderr inherit the worker.

## Lua

The generic API is available when the `watch` feature is compiled:

```lua
local service = oslo.watch.start {
  name = "check",
  paths = { "src/**/*.rs", "Cargo.toml" },
  run = { "cargo", "check" },
  initial = true,
  debounce_ms = 100,
  policy = "coalesce",       -- or "restart"
  grace_ms = 1000,
  scratch = "auto",         -- false, true, or an exact name also work
}

print(service:name(), service:mode())
service:stop()               -- true once, false on later calls
```

A non-persistent Lua service is stopped when a script or normal interactive shell exits. Setting
`persist = true` transfers that lifetime to the user; the returned handle or `oslo scratch -k NAME`
must stop it explicitly.

An allowed `.env.lua` can bind that service to its directory environment:

```lua
oslo.direnv.watch_command {
  name = "server",
  paths = { "src/**/*.rs", "Cargo.toml" },
  run = { "cargo", "run" },
  policy = "restart",
}
```

Directory services default to `initial = false`. Startup is registered for teardown only after it
succeeds, and unload stops it unless `persist = true`. `watch_command` runs a command after content
events. In contrast, `watch_file` and `watch_dir` only tell the directory-environment loader which
paths should make the environment itself reload.

## Build recipes

```sh
oslo make --watch check
oslo make --watch --postpone check
oslo make --watch --restart serve
```

Watch planning resolves the target and dependencies but does not execute their bodies. It watches
the deduplicated declared input patterns from the resolved plan, the governing `.make.lua`, and
every successful `make.import`. Keeping the patterns rather than only their current matches is what
lets a future matching source file trigger the worker.

The watched command is `/proc/self/exe make TARGET ARGS...` without `--watch` or `--force`. Recipe
staleness and phony behavior therefore remain unchanged. A graph with no declared inputs is refused;
Oslo does not guess that the entire repository should be watched. Merely changing directory never
loads `.make.lua`; the file is trusted only after the explicit `oslo make --watch` invocation.

## Scratch execution

With `watch,scratch`, an interactive `oslo watch` or Lua service using `scratch = "auto"` starts a
detached Scratch. Non-terminal CLI use stays foreground so a script cannot detach accidentally.
`--foreground`, `--scratch[=NAME]`, and `--attach` override that choice.

Generated names contain the logical service name and an eight-hex digest of the normalized root,
patterns, policy, and argv. Directory-environment services also contain a short session component,
so two shells do not own the same scoped Scratch. Exact names are validated and live collisions are
refused rather than replaced.

Program-backed Scratches use the existing keeper, socket, pty, and bounded replay log. The worker is
execed through a private one-use marker before normal Oslo threads start; inherited non-standard
descriptors are closed so a persistent service cannot keep its caller's output pipes open.

## Measurements

Measured on the static `x86_64-unknown-linux-musl` release at fat LTO and `opt-level = "s"`, with
every other Cargo feature held on and the same post-link unwind-section removal used by `.make.lua`:

| build | exact bytes |
|---|---:|
| all features except `watch` | 5,547,488 |
| all features including `watch` | 5,608,928 |
| `watch` delta | **61,440 bytes (60 KiB)** |

No new crate dependency was added. The delta includes the CLI, Lua adapters, pattern engine,
process-group runner, Scratch hosting path, and inotify support; it is not counted as free merely
because Oslo previously had a smaller filesystem-watch API.

## What it cannot do

- Linux is the supported platform; the event source is inotify.
- It is not a shell-language parser. Use an explicit `sh -c` when a pipeline or redirection is
  intended.
- Scratch output is replayable but a Scratch is not a multiplexer: one client may attach at a time.
- A persistent service is intentionally not stopped on unload or shell exit. Losing its name means
  finding it with `oslo scratch -l` before it can be killed.
- Recipe watch mode requires declared `inputs`; there is no repository-wide inference fallback.

## Where it lives

| path | what |
|---|---|
| `crates/oslo-shell/src/watch/` | inotify events, patterns, recursive descriptors, child groups, runner |
| `crates/oslo-shell/src/scratch/program.rs` | program-backed Scratch launch and generated names |
| `crates/oslo-runtime/src/lua/api/watch_service.rs` | `oslo.watch.start`, handles, scoped registry |
| `crates/oslo-runtime/src/lua/api/direnv.rs` | `oslo.direnv.watch_command` ownership |
| `crates/oslo-runtime/src/lua/api/make.lua` | recipe-plan input and import collection |
| `src/cli/watch.rs` | public options, exact argv parsing, launch selection |
| `src/cli/make.rs` | recipe watch handoff |
| `tests/watch_tool_tests.rs`, `tests/make_cli_tests.rs` | process, Scratch, Lua, and recipe integration |
