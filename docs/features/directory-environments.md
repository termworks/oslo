# Directory environments

A `.env.lua` in a project, loaded when you walk in and taken back out exactly when you walk out.
oslo reads it itself — no `direnv` binary, no bash subprocess, no prompt hook and no `eval`
protocol — because most of direnv's machinery exists to let an external program talk to a shell it
did not write, and **oslo is the shell**.

> ## This is in `oslo`, not in `oslo-minimal`
>
> Everything on this page is behind the **`direnv`** cargo feature, which is off by default.
>
> | | `cd` into a project |
> |---|---|
> | `oslo` | reads `.env.lua`, loads and unloads |
> | `oslo-minimal` | is just `cd` |
>
> ```sh
> scripts/build.sh              # the full binary, every feature
> scripts/build.sh --minimal    # no directory environments at all
> ```
>
> It costs **140 KB** — 5,733,104 bytes without it against 5,877,472 with — and the reason it is off
> is not the size. This is the one part of the shell
> that **reads a file on arrival in a directory and can run what it finds there**. That is a
> different kind of trust from anything else oslo does unprompted, and a `/bin/sh` on a
> distribution should not do it because a shell somewhere else wanted it to.
>
> Without the feature there is no `direnv` builtin — the word falls through to `$PATH`, so the real
> direnv still works if it is installed — no `oslo.direnv` API, no `.env.lua` reading,
> and no dev shell import.

<!-- demo:begin -->
[![directory-environments demo](https://asciinema.org/a/1262735.svg)](https://asciinema.org/a/1262735)
<!-- demo:end -->

## How it works

The state lives in memory and the work happens on the `cd` path. All of it is one function,
`Direnv::arrive`, called from the read loop before each prompt whenever the directory differs from
the one last settled — so the route does not matter: a key binding that jumps, a Lua hook or a `cd`
inside a sourced file all land here, because the prompt is when the environment has to be true.

```
 a directory change, by any route
   ▼
 find::applicable(dir)      walk up, nearest ancestor holding .env.lua
   │                        feature "direnv" off ⇒ answer None, which is the no-file path
   ├── same owner, nothing edited ─────────────────────────► nothing happens
   │                                    (one stat per watched file, and return)
   ▼
 unload()   ALWAYS FIRST, so two projects cannot merge
   ├─ restore()          the Lua prompt — before the variables, not after
   ├─ undo.to_apply()    each variable, with the export flag it had
   ├─ aliases.to_apply()
   └─ unset OSLO_DIRENV
   ▼
 allow.status(path) ── Denied ─────► Event::Denied
   │               └─ NotAllowed ──► Event::Blocked   (printed once per path)
   ▼ Allowed
 before = (vars, aliases)     under the environment lock
 run(rc)                      lock RELEASED — the file runs in the Lua engine
 after  = (vars, aliases)     under the lock again
   ▼
 forward = Diff::between(before, after) ─► - removed  ~ changed  + added   (the report)
 undo    = forward.reverse()            ─► what leaving will apply
 export OSLO_DIRENV = encode(owner, undo)
   ▼
 Event::Loaded { owner, changed, aliases, functions }  + Event::Failed beside it, not instead
```

The lock is taken twice rather than held across `run`, and that was forced by a bug: the file's job
is to call `oslo.env.set` and friends, which take the same lock with `try_lock`, so holding it made
every one of them fail with "shell state is busy" while the load reported success. A file that
failed half way still had its first half take effect, so the diff is taken regardless.

### The file runs in its own directory

direnv has always `cd`-ed to the file's directory before running it, and half the files in the
world say `PATH_add ./bin`. **`.env.lua` did not**, and that was a bug rather than a difference:
`~/proj/.env.lua` saying `oslo.direnv.path_add("./bin")`, entered as `cd ~/proj/app/src`, resolved
`./bin` against `app/src` and put a directory that does not exist onto `$PATH` — silently, and for as
long as you stood in the project. Which answer you got depended on how deep you walked in, so it
worked from the project root and only ever failed for somebody in a subdirectory.

It chdirs now, and restores the shell's own directory afterwards whatever happens, panic
included: a shell left standing somewhere the user did not walk to is a far worse outcome than an rc
file that failed. `tests/directory_environment_tests.rs` is what holds it.

`oslo.direnv.dir()` answers with that directory, for a path being *stored* rather than used — a cache
location, a marker written into `oslo.state`, a value handed to something that runs later — where
`"./thing"` stops meaning anything the moment the shell moves on. It is `nil` outside a directory
environment, which is the honest answer in `init.lua` and in `oslo make`.

Nearest ancestor wins outright; loading every ancestor would make an outer file's effect depend on
how deep you happened to be standing. A directory holding an `.envrc` as well is no concern of
oslo's — that file is direnv's, and the two coexist without either shadowing the other.

### A question that may not take forever

This file runs on the `cd` path, synchronously, before the prompt is drawn — and the things it
naturally asks are the slow ones. `git ls-remote` against a host that is down, a cold `nix eval`, a
`stat` on an NFS mount that has gone away: any of them hangs **every `cd` into that subtree**, with
no way out but SIGINT.

```lua
local r = oslo.run{ "git", "ls-remote", "--heads", "origin",
                    capture = true, timeout_ms = 800, on_timeout = "KILL" }
if r.timed_out then oslo.env.set("BRANCH_STATE", "unknown") end
```

`timed_out` is on every result, always a boolean, so a caller can test it without having set a
deadline. What the command wrote before the deadline is kept. `on_timeout` is `"TERM"` (the default)
or `"KILL"`, and it is *which signal is sent*, not an escalation ladder — a caller who needs to
outlast a `trap '' TERM` says `"KILL"`.

**A deadline forces the command into a child.** Without one, a call with nothing to capture runs in
this shell — that is what makes `sh.cd("/tmp")` move it — and there is nothing there to interrupt,
because that path dispatches builtins and shell functions in-process. So
`oslo.run{"cd", "/tmp", timeout_ms = 100}` runs in a child and no longer moves the shell. That is
the honest reading of a timeout: you cannot interrupt the thing that *is* the shell.

The signal goes to a process group created for the command, so it reaches what the command itself
started rather than only the shell wrapping it.

### Leaving, and what else counts as a change

Variables and aliases are put back by the undo record. Everything *else* a file did had no moment at
which to be undone, and a file could not say that anything other than itself should trigger a reload.

```lua
oslo.direnv.watch_file(".tool-versions", "flake.lock")  -- reload when these change too
oslo.direnv.watch_dir("config")                          -- entries added or removed
oslo.direnv.on_unload(function()                         -- a moment on the way out
  oslo.completion.forget("projtool")
end)
```

`on_unload` may be called more than once and runs **last-registered-first**, so a file tears down in
the reverse of the order it set up. It runs **before the variables are put back** — the unload path
releases the environment lock first, deliberately, so a prompt function reading a variable sees the
directory's value — which means a callback has the *whole* API here, `oslo.env.get` and `oslo.run`
included. One that raises is reported and the others still run: by that point the undo record is
already being spent, and stopping half way would leave the directory's variables set with nothing
left to remove them.

`oslo.completion.forget(name)` is the other half of registering one, and answers **how many** of the
three completion surfaces held that name — `0` when none did. Until now both registries could only
be cleared entirely.

**`watch_dir` watches the directory, not the files in it.** A directory's timestamp moves when an
entry is added, removed or renamed and does not move when a file inside it is edited in place. So it
notices `config/new.toml` appearing and does not notice `config/app.toml` changing. That is what
direnv's `watch_dir` has always done; the two are one call so they cannot drift.

Both watch calls are **refused outside a load**. The list is drained by the next arrival, so a call
from a timer or a background callback would quietly attach a path to whichever unrelated project
loaded next.

### Watching a command instead of the environment

With the independent [`watch`](watch.md) feature, a file can own a long-running command watcher:

```lua
oslo.direnv.watch_command {
  name = "server",
  paths = { "src/**/*.rs", "Cargo.toml" },
  run = { "cargo", "run" },
  policy = "restart",
}
```

This is deliberately different from the two calls above. `watch_file` and `watch_dir` add reasons
to reload `.env.lua`; `watch_command` leaves the environment loaded and reruns the declared argv.
It defaults to `initial = false`, starts only while the allowed environment is loading, and is
stopped through the same reverse-order unload list when the shell leaves. `persist = true` omits
that ownership. With Scratch support, the automatic service has a replayable log and a generated
name containing the shell session, so two shells in one project do not stop each other's watcher.

### A variable the project keeps to itself

`oslo.env.set` exports by default, and for a long time that was the only thing it could do — so a
`.env.lua` could not create a variable without handing it to every child the project ever spawned.
A project name, a chosen profile, a computed cache key: all of it reached the editor you launched
from that directory, and then its crash report.

```lua
oslo.env.set("PROJECT", "oslo")                      -- exported, as before
oslo.env.set("CACHE_KEY", key, { export = false })   -- this shell only
oslo.env.all{ exported = false }                     -- everything, local included
oslo.env.snapshot()                                  -- with the flag each one carries
```

Demoting a name that is *already* exported takes two steps inside the binding, and skipping the
first is a silent no-op: `set_var` ORs the flag it is handed with the one the variable carries, so
passing `false` alone changes nothing. Clearing the entry and writing it again is the only way down
— the same dance the unload path does — guarded by a readonly check, because `unset_var` does not
check and `set_var` does.

There is deliberately **no `restore`**. Putting a snapshot back means removing what is set now and
absent from it, and doing that across a `cd` would fight this file's own undo record — two
mechanisms describing the same variables with no agreed order, and `PWD` alone would move the shell.
Undoing a directory's work is what the undo record is for.

### The allow gate

A file getting to run code when you walk into a directory is arbitrary code execution by anyone who
can get you to clone a repository. So an rc file is inert until allowed — **not read** — and the
keys are direnv's, copied deliberately rather than reinvented.

| decision | key | consequence |
|---|---|---|
| allow | `sha256(absolute path + "\n" + contents)` | editing an allowed file revokes it |
| deny | `sha256(absolute path + "\n")` | a denial survives every edit |

Deny is checked first, so a refused place cannot talk its way back in by changing: allowing is a
statement about a piece of text, denying is a statement about a place. Tokens are empty files named
by the hash under `$XDG_DATA_HOME/oslo/direnv/{allow,deny}`, the directory at `0700`. Any failure to
answer reads as not allowed; a store that cannot be read must not mean "run it".

### What "added / changed / removed" is computed from

Two snapshots of the same two things: every variable as `(value, exported)` — not only the exported
ones, so a shell-local variable a directory exports comes back *local* rather than deleted — and
every alias. `Diff::between` keeps only the keys that moved, so anything you changed by hand while
standing there survives; `Diff::reverse` is the undo; and `None` and `Some("")` stay distinct all
the way through, because `[ -n "$FOO" ]` reads them differently.

Twelve names are excluded as the shell's own: `_`, `BASHPID`, `EPOCHREALTIME`, `EPOCHSECONDS`,
`LINENO`, `OLDPWD`, `PIPESTATUS`, `PPID`, `PWD`, `RANDOM`, `SECONDS`, `SHLVL`. Reading a file moves
`LINENO`, so a file that assigned nothing came out of the diff having "changed" it; `PWD` is
the same mistake with teeth, since the file runs with the working directory set to its own, and
putting it "back" would be a directory environment quietly moving the shell.

Removals are reported first, then changes, then additions, each row cut to the terminal with a count
of what was dropped. A Nix dev shell adds thirty-odd boilerplate variables and takes away one that
mattered; sorted the other way, the `-GITHUB_TOKEN` is in the part that got truncated.

### The undo record travels with the variables

Variables have to go into the real `environ` for a child to see them, so everything spawned from a
shell standing in a project inherits that project's environment. Without a record travelling
alongside, a new pane or a nested `oslo` starts believing nothing is loaded while holding all of it,
and no `cd` can shift it. That is what direnv's `DIRENV_DIFF` is for, skipping it here was a
mistake, and `OSLO_DIRENV` is the same idea — length-prefixed, so a value may contain anything:

```text
1 4:/tmp 3:FOO 1:1 3:bar 5:EMPTY 1:u 0:
  └owner  └name └flag └value      └ was unset before, so unset it again
```

Anything malformed decodes to nothing rather than to half a record. An inherited environment is
adopted **stale**, so the first arrival re-runs the file even when the directory matches: `execve`
carries variables and nothing else, so the child holds the project's `$PATH` while having none of
its aliases or its prompt, and treating that as loaded is what made a project alias report `command
not found` in a shell whose `$PATH` was already the project's.

### The dev shell, without entering one

`oslo.direnv.nix_develop()` reads
`nix print-dev-env --json`. The JSON is a faithful dump of the builder, so a name that would wreck
the shell you are standing in has to be dropped here rather than relied on to be absent: `IGNORED`
in `nix_shell.rs` is that list — `HOME`, `PWD`, `OLDPWD`, `SHELL`, `SHLVL`, `TERM`, `TZ`, the
`TMP*`/`TEMP*` family, `NIX_*` build variables and a few more — plus exported bash functions, whose
encoding oslo cannot run. The `$PATH` the dev shell reports is also merged with your own rather
than replacing it; without that, a `cd` into a flake silently loses half the commands you had.

**`shellHook` is not run unless a project asks.** It is exported like any other variable, so it
lands in the environment — but it is a bash program rather than data, and running it means executing
somebody else's script on every entry to the directory. `nix develop` runs it and so does
nix-direnv; plain direnv does not, and neither does oslo. A project that wants it says so:

```lua
oslo.direnv.nix_develop{ hook = true }              -- this directory's flake
oslo.direnv.nix_develop{ flake = "..#other", hook = true }
```

It runs **after** the variables are set, because a hook is written expecting the shell it is
entering — and through oslo rather than through bash, so the `$PATH` it sees is the one the caller
will have.

### The functions, which are the other half of a dev shell

`print-dev-env --json` has **two** top-level keys, and `variables` is the smaller one. For an
ordinary flake:

| | count |
|---|---|
| `variables`, `exported` — imported | 93 |
| `variables`, `var` / `array` — dropped | 32 / 22 |
| **`bashFunctions`** | **110** |

Those 110 are stdenv's build system: `genericBuild`, `runHook`, every `*Phase`,
`substituteInPlace`, `patchShebangs`, `moveToOutput`, the `nix*Log` family. Without them a dev shell
is a set of paths; with them it is somewhere you can build.

```lua
oslo.direnv.nix_develop{ functions = true }
```

All 110 parse, and **98 of them run with no shell-level error**. `runHook`, `runPhase`,
`genericBuild`, `substituteInPlace` and `printPhases` were each checked against bash and produce
byte-identical output — so a phase sequence runs here the way it does inside `nix develop`:

```
> phases="one two"; genericBuild
Running phase: one
ONE
Running phase: two
TWO
```

Getting there took four things the shell was missing, each found by running the functions rather
than reading them:

| | what stdenv needs it for |
|---|---|
| `${!v<op>}` | `runHook` is `${!hooksSlice+"${!hooksSlice}"}` |
| an indirection naming an **array** | that `hooksSlice` holds `preConfigureHooks[@]` |
| `${a[@]+alt}` and `${a[@]:-d}` | the same line — the array is the thing being tested |
| `local -a` | `substituteInPlace` declares one on its first line |

Arrays themselves were never the problem: oslo has had them, and `a=(x y)`, `a+=(z)`, `${a[@]}`,
`${#a[@]}` all matched bash before any of this. What was missing were those four forms around them.

Defined **before** `shellHook`, since a hook calling `runHook` or `addToSearchPath` is ordinary.
One `eval` for all of them rather than 110 — 40 ms for 66 KB in a debug build, which is the whole
cost of the option.

### While it runs

Output is captured to a temporary file so it can be printed under the line naming the rc file — a
pipe has a fixed capacity and nothing draining it, so a chatty file would block on its own
output. Reading it once at the end is right for a file that takes a moment and wrong for one that
takes a minute: `oslo.direnv.nix_develop()` against a cold store builds for as long as it takes, into a file nobody
is reading, and a long build is then indistinguishable from a hang. So the file is also tailed:

```
 t = 0       run(rc) starts; stdout and stderr are the scratch file
 t < 500ms   nothing is drawn at all — which is nearly every arrival
 t = 500ms   still running: whole lines that have arrived go to the real terminal in
             the block's own rail, checked every 80ms; a partial line is held back
 end         the tail stops and says what it printed; the summary prints only the rest
```

Progress goes to the caller's *saved* stdout, taken before the redirect, and only when that was a
terminal: nobody is watching a pipe arrive, and the lines would land in the middle of their data.

## What makes it different

bash, zsh and fish have no directory environments of their own; the comparison is with direnv, and
it is mostly about being inside rather than outside. direnv is a separate process, so it has to
serialise its state into your environment, hand a generated script to a shell it did not write, and
`eval` the result. Here the diff lives in memory, unloading is `Diff::reverse`, and the only thing
written into the environment is the undo record — the one piece that is not incidental to being an
external binary, since children inherit variables whoever set them. One place the outside position
wins: `direnv prune` can be selective because each token file holds the path it stands for. oslo's
tokens are empty by design, so its prune is all or nothing, and it says so rather than pretending.

## Configuration

```lua
-- .env.lua
oslo.env.set("DATABASE_URL", "postgres://localhost/app_dev")
oslo.env.set_alias("t", "cargo test")
oslo.direnv.path_add("./bin")         -- prepended, idempotent, gone when you leave
oslo.direnv.nix_develop()             -- or oslo.direnv.nix_develop("..#other")
oslo.ui.prompt(function() return "PRODUCTION> " end)
```

```sh
direnv allow      # trust this file as it stands now         (permit, grant)
direnv deny       # refuse this path, whatever it becomes    (block, revoke)
direnv status     # what is loaded, what was found, whether it is trusted
direnv reload     # forget the loaded state and the cached dev-shell evaluation
direnv prune      # drop every decision
direnv edit       # $VISUAL, else $EDITOR, else vi; then says the edit revoked it
```

`allow` and `deny` take effect where you stand rather than on the next `cd`: the builtin cannot run
Lua itself, so it leaves a reload request that the read loop carries out before the next prompt.

```lua
oslo.feature.set("direnv", false)          -- unloads what is loaded, not just stops it
oslo.feature.when("direnv", function(dir)  -- re-asked on every directory change
  return oslo.fs.find_up(".env.lua", dir) ~= nil
end)

oslo.on.report(function(r)                -- draw the report yourself
  -- r.kind == "direnv"; r.state is loaded, unloaded, blocked, denied or failed;
  -- r.changed and r.aliases are {name=, change="added"|"changed"|"removed"}
  if r.kind == "direnv" and r.state == "loaded" then
    oslo.ui.block(("%d changes"):format(#r.changed)):done()
    return true                              -- handled; oslo prints nothing
  end
end)
```

`$XDG_DATA_HOME` locates the allow store.

## An `.envrc` project

oslo does not read `.envrc`. It used to, against a reimplementation of direnv's stdlib — some 1100
lines of Rust tracking 1.4k lines of somebody else's bash so that `use flake`, `layout python` and
`source_up` meant here what they mean there. Every one of those is a place for the two to disagree
in a way only a user finds, and none of it is work oslo has to do. direnv exists and is good at it.

So an `.envrc` project runs direnv, and the whole of it is three lines in `init.lua`:

```lua
-- Only where `direnv` resolves to a file: a path has a slash in it, a builtin does not.
oslo.env.set("PROMPT_COMMAND", 'type direnv 2>/dev/null | grep -q / && eval "$(direnv export bash)"')

oslo.feature.when("direnv", function(d)          -- oslo's own, where oslo's file is
  return oslo.fs.find_up(".env.lua", d) ~= nil
end)
oslo.command.when("direnv", function(d)          -- the real binary, where direnv's file is
  return oslo.fs.find_up(".envrc", d) ~= nil
      or oslo.env.get("DIRENV_DIFF") ~= nil
end)
```

`$PROMPT_COMMAND` is evaluated against the live environment before every prompt, which is exactly
what direnv's own bash hook is built on — so this loads on the way in and unloads on the way out,
with no oslo code in the middle. `.envrc` projects are direnv's, `.env.lua` projects are oslo's, and
a directory with neither has no `direnv` command at all.

Two details, both of which fail quietly if you leave them out:

- **The `type` guard.** The two tools answer to the same word. In a `.env.lua` directory oslo's
  builtin holds the name, so an unguarded hook runs `direnv export bash` against the builtin and
  gets `export: not a direnv command` on every prompt. `type` writes a path for a file and
  `is a shell builtin` for a builtin, and it respects `oslo.command.when`'s mask, so it is the one
  question that distinguishes them.
- **The `DIRENV_DIFF` clause.** direnv has to be **run from outside** a directory to unload it. Hide
  the binary the moment you leave and the unload never happens, so the project's variables stay set
  for the rest of the session.

**If `bash` on your `$PATH` is a link to oslo**, tell direnv where a real one is — it runs `.envrc`
through bash, and that file is bash, extglob and all:

```toml
# ~/.config/direnv/direnv.toml
bash_path = "/usr/bin/bash"
```

`direnv status` prints the `bash_path` it is using, which is the quick way to check.

## Measurements

`OSLO_TIME_PROMPT=1` against the release binary, driven through a pty, with a two-line `.env.lua` in a
temporary project. The `direnv` phase of the prompt is `arrive` and nothing else:

| what happened | direnv phase |
|---|---:|
| no rc file anywhere above | 0.0 ms |
| a file was found but is not allowed | 0.1–0.3 ms |
| leaving: unload and restore | 0.1–0.4 ms |
| loading a two-line `.env.lua` | 81.0, 81.4 ms |
| loading a file that also runs `sleep 0.1` | 162.2, 162.1 ms |

The last two rows differ by exactly the sleep, which is the shape of a quantisation rather than of
work: **a load in a terminal is rounded up to a multiple of the tail's 80 ms poll interval**, since
stopping the tail joins a thread asleep in one of those slices. That interval is `EVERY` in
`environments/live.rs`. Everything that is not a load costs tenths of a millisecond.

Recorded in `nix_shell.rs` rather than measured here: `nix print-dev-env` costs about half a second
on a warm store and several on a cold one. Hence the cache at `.direnv/dev-env.json`, keyed on the
arguments and on `flake.nix`, `flake.lock`, `shell.nix` and `default.nix` as they stand, and written
`0600` — it is a verbatim dump of a dev shell.

## What it cannot do

- **Notice an edit while you are standing in the directory.** `arrive` runs when the directory
  differs from the last settled one, so the stamp check that catches an edited file only fires on
  the next arrival. `direnv reload` is the answer, and `direnv edit` leaves the request itself.
- **Restore anything but variables, aliases and the Lua prompt.** A shell function a file
  defines stays defined after you leave. Keybindings are excluded deliberately: a key meaning
  different things in different directories, with nothing on screen to say so, is worse than not
  having the feature at all.
- **Catch an edit that keeps the file's length and lands inside the mtime granularity.** The stamp
  is `(mtime, length)` from one `stat`; the content hash catches it on the next genuine reload.
- **Read an `.envrc`.** That file is direnv's and is left to direnv — see above for the one line
  that wires the two together.
- **Follow a Nix dev shell exactly.** `--json` is a dump of values, so anything the bash form *does*
  besides assigning is invisible to it; two such differences have been found by hand, and a third
  is likely.
- **Work in a script or `sh -c`.** A non-interactive shell has no directory environment at all: its
  environment comes from whoever ran it, and a file in the working directory quietly changing that
  would make scripts depend on where they were invoked from.

## Where it lives

| path | what |
|---|---|
| `crates/oslo-shell/src/direnv/mod.rs` | `Direnv::arrive`, `load`, `unload`, `Event`, `maintained` |
| `crates/oslo-shell/src/direnv/allow.rs` | `Allow::status`/`allow`/`deny`/`prune`, both hashes |
| `crates/oslo-shell/src/direnv/diff.rs` | `Diff::between`, `reverse`, `changes`, `Change` |
| `crates/oslo-shell/src/direnv/carry.rs` | `OSLO_DIRENV`: `encode`, `decode` |
| `crates/oslo-shell/src/direnv/find.rs` | `applicable`, `here`, `owner` |
| `crates/oslo-shell/src/direnv/paths.rs` | `watch`, `take_watches`, `absolute`, `prepend_into` |
| `crates/oslo-shell/src/nix_shell.rs` | `print-dev-env --json`, `IGNORED`, the cache |
| `crates/oslo-shell/src/env/builtins/direnv.rs` | the `direnv` builtin |
| `crates/oslo-runtime/src/startup/environments/` | running the file, `capturing`, `live`, the block |
| `crates/oslo-runtime/src/lua/api/direnv.rs` | `oslo.direnv.path_add`, `oslo.direnv.nix_develop` |
| `crates/oslo-runtime/src/lua/api/watch_service.rs` | `oslo.direnv.watch_command`, service handles and teardown |
