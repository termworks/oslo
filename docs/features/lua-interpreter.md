# The Lua interpreter

oslo speaks Lua 5.4 through [`luna`](https://github.com/onix-os/luna), a stackless bytecode VM with
a tracing garbage collector, written in pure Rust. Pure Rust is the requirement everything else
follows from: oslo ships as a single statically linked musl binary, and speaking Lua had to cost no
C toolchain. `mlua` — a binding to the reference interpreter — would compile some thirty thousand
lines of C into the binary and need a musl cross-compiler to link it.

It is a dependency **pinned to a tag** — `v0.5.0` — rather than vendored or tracked on a branch. A
branch would make a build depend on the day it ran; a tag moves only when somebody moves it, and
`cargo update` cannot move it at all. `Cargo.lock` records the commit behind the tag either way, so
what the tag buys is a name a human can read in a diff. See `crates/oslo-luavm/Cargo.toml`.

**This replaced a tree walker.** Until recently the evaluator was oslo's own, walking a `full_moon`
AST. That crate and its parser are gone: 19,630 lines deleted, in exchange for coroutines, `goto`,
byte-exact strings, collected cycles, unbounded recursion and roughly an order of magnitude in
speed. What the trade cost is recorded under [What it cannot do yet](#what-it-cannot-do-yet).

<!-- demo:begin -->
[![lua-interpreter demo](https://asciinema.org/a/1262741.svg)](https://asciinema.org/a/1262741)
<!-- demo:end -->

## How it works

luna compiles a chunk to bytecode and runs it on its own stack inside a `gc-arena` heap. That heap
is the fact that shapes everything above it: a `luna::Value<'gc>` carries a garbage-collector
lifetime and exists **only** inside `lua.enter(|ctx| …)`. It cannot be returned, stored in a struct,
or built by code that has never heard of a VM.

oslo's own `Value` — in `oslo-base`, an `Rc` anyone can keep — stays the interchange currency, and
the two meet at exactly one place:

```text
  the shell, forty-odd files            the boundary                    the VM
  ────────────────────────────   ─────────────────────────────   ──────────────────────
  oslo_base::value::Value        oslo_luavm::convert             luna::Value<'gc>
  (owned, Rc, no lifetime)   ──►   into_lua / from_lua      ──►  (arena, 'gc lifetime)
                                          │
  structured pipeline,                    │  Engine  ── eval, load, call_function
  settings, theme, hooks,                 │  Host    ── what a native may ask
  the whole oslo.* API                    │  globals ── the shared namespace
```

`oslo-base` has no dependency on any engine at all. That is what lets the structured pipeline,
`settings`, `theme`, `hooks` and the ~200 registered callables be written, compiled and tested with
no VM in scope — and it is why swapping the engine underneath was a change to one crate rather than
to seventy files.

### The crate graph

```text
  luna (pinned) ──► oslo-luavm ──► oslo-shell ──► oslo-runtime
                          ▲              ▲              │
                     oslo-base ──────────┴──────────────┘
```

### Crossing the boundary

Three things cross, and each is a rule worth knowing before touching `convert.rs`:

* **Tables are copied, not shared.** The tree walker's tables were the interpreter's own `Rc`, so
  Rust could fetch a global table and mutate it and Lua would see the change. A VM table cannot
  leave the collector, so what `Host::global` answers is a *snapshot*. Anything meaning to reach the
  live table goes through `Host::set_field`, or is done in Lua. This is how
  `package.preload` and `oslo.completion.for_command` are written.
* **Metatables cross both ways.** They did not at first, and three whole surfaces were silently
  dead — `sh.*`, `oslo.prompt` and `oslo.theme.styles` are each an *empty* table whose entire
  behaviour is its metatable, so copying only the entries delivered `{}`.
* **Identity is preserved.** A memo keyed on the `Rc` pointer means a value that was one thing stays
  one thing: `oslo.from_json == oslo.json.decode` answers true, and a table containing itself stays
  cyclic rather than being truncated — which is what lets `oslo.json.encode` refuse it by name.

Functions cross inside `Function::Held`, an `Rc<dyn Any>` the shell never looks inside. Two things
travel in it and only `convert.rs` knows which: a `Native` (a Rust closure going in, wrapped as a
luna callback) or a `StashedFunction` (a Lua function coming out, rooted so the collector keeps it
while a hook or a completer holds it).

**This is why the port was small.** All ~200 callables register through one helper, `util::native`,
whose signature is oslo's own — `Fn(&dyn Host, Vec<Value>) -> LuaResult<Vec<Value>>` — and 198 of
them ignore the `Host` entirely. Their bodies never changed.

## What makes it different

oslo's config is `init.lua`, and the values a config produces are the same `Value` the shell reads
— no serialisation step, no second config format, and no separate configuration language to learn
beside the one the prompt already speaks.

`io.popen` and `os.execute` are meant to refuse by name rather than work, because in real Lua both
run their argument through `/bin/sh` — someone else's shell, from inside this one, and nothing at
all on a system where oslo is the only shell installed.

### Handles, and things that stream

**Everything oslo hands out that owns something is an object with a metatable**, built by
`api/handle.rs`. The verbs live behind `__index`, so `pairs` over a handle shows nothing to get
wrong and internals stay internal; `__newindex` refuses a typo rather than adding a field; and
`<close>` releases at the end of the block, after which every verb says so.

```lua
local db <close> = oslo.db.open("notes")     -- the file is shut here
local tmp <close> = oslo.fs.mktempdir()      -- the directory is removed here
```

`oslo.db.open`, `oslo.spawn`, `oslo.after`/`oslo.every` and `oslo.fs.mktempdir` are the four.
`h:close()` is the same call as leaving a `<close>` scope, and answers whether it was the one that
did the closing.

**No handle sets `__gc`**, and the reason differs by kind. For a database, a file or a pipe there is
nothing a finalizer would do that collection does not already: the verbs hold Rust values, and
collecting the handle drops them. `<close>` buys the *moment*, not the release. For a spawn, a timer
or a temporary directory a finalizer would be wrong — those handles are normally written for the
effect and thrown away, so `__gc` would cancel the callback, stop the timer, and remove a directory
whose path had been copied out.

**Anything that iterates is lazy, and the iterator is a handle too.** A generic `for` needs
something callable and a scope-bound release needs a metatable, so these carry `__call` — one value
that works in both positions, rather than two returns a caller has to remember to keep together:

```lua
for line in oslo.lines{"cargo", "build"} do oslo.ui.log(line) end
for path in oslo.fs.walk("/etc") do print(path) end
for line in oslo.fs.lines("/proc/mounts") do … end

local out <close> = oslo.lines{"journalctl", "-f"}   -- reaped when the block ends
for line in out do if line:find("error") then break end end
```

A loop that runs out cleans up on its own. A loop that `break`s does not, because luna does not
close a `for`'s closing value — which is what `<close>` and `:close()` are for.

`oslo.lines` carries two options the streaming case needs:

```lua
local build = oslo.lines{ "cargo", "build", stderr = true }
for line in build do oslo.ui.log(line) end
if build:status() ~= 0 then error("the build failed") end
```

**`stderr = true` merges the streams into the one pipe**, in the order the command wrote them.
Without it `oslo.lines{"cargo","build"}` sees nothing at all, because cargo writes its progress and
its errors to stderr — the binding used to call the non-merging form of a function whose merging
form already existed. One pipe rather than two, because two drained separately cannot say which line
came first. `oslo.run` and `oslo.pipe` refuse the key rather than ignoring it: they capture the
streams separately and have `capture_err` for that.

**`handle:status()` is how the command ended**, or `nil` while nobody has waited yet. It does not
block. What a loop leaves behind depends on how it ended:

| how the read ended | `status()` |
|---|---|
| the loop ran out | the command's own status |
| `close()` while it was still writing | 141 — it took a `SIGPIPE`, as `cmd \| head -1` does |
| `close()` while it was quiet | its real status, after waiting for it |

That last row is worth reading twice: `close()` is not a way to cancel a sleeping command. It drops
the read end and waits, and a child that is not writing never notices. Killing one is
`oslo.spawn`'s business.

### A string is bytes

```lua
local png = oslo.fs.read("logo.png")   -- the file, exactly
#png                                    -- its size
oslo.fs.write("copy.png", png)          -- byte for byte
string.unpack("<i4", oslo.db.open("d"):get("row"))
```

`oslo.fs.read` used to go through `String::from_utf8_lossy`, so every byte that was not text came
back as `U+FFFD` — a read that looked like it had worked and had quietly changed the file. The same
happened to anything `string.pack` produced on its way out to the shell, and `oslo.lines` failed
outright on a command whose output had one non-UTF-8 byte in it.

**There is still exactly one string type.** `type()` says `string` either way, `t["a"]` and the
table indexed by the bytes `a` are the same slot, and equality is Lua's. Internally the shell keeps
two representations — `Value::Str` for text, `Value::Bytes` for what is not — and they cannot
overlap, because valid and invalid UTF-8 are disjoint and one constructor decides which you get.

What a *name* is remains text: a path, a variable, a command word. Handing `oslo.fs.read`'s answer
to something expecting one of those is a message rather than a mangling, and `oslo.json.encode`
refuses bytes outright, since no JSON string could hold them and still be the same bytes.

### Facts the shell already knows, without a process

A prompt draws on every keystroke that changes the line, so anything it asks for has to be cheap.
These are the questions people shell out for, answered from files the kernel already keeps:

```lua
oslo.git.head()          -- { branch = "main", commit = "…", detached = false }
oslo.git.operation()     -- "rebase" while one is part-way through, else nil
oslo.git.upstream()      -- "origin/main", or nil when the branch tracks nothing
oslo.git.stash()         -- how many entries; 0 outside a repository
oslo.git.tag()           -- a tag pointing at HEAD

oslo.sys.kernel()        -- "7.0.0-29-generic"
oslo.sys.cpus()          -- what this shell may run on, cgroup quota included
oslo.sys.loadavg()       -- { 1, 5, 15 }
oslo.sys.memory()        -- bytes: total, available, free, swap_total, swap_free
oslo.sys.uptime()        -- seconds
```

`oslo.git` reads `.git` — one to three small file reads and no `git` process. It handles a linked
worktree, where `.git` is a *file* and the refs live in the repository it was linked from.
**`dirty` and `ahead`/`behind` are deliberately absent**: both mean real work through the object
database, and a hand-rolled wrong answer is worse than no answer. Ask for those off the prompt:

```lua
oslo.every(2000, function()
  oslo.spawn{ "git", "status", "--porcelain",
    on_exit = function(out) oslo.state.set("git.dirty", out ~= "") end }
end)
```

`oslo.sys.memory()` answers bytes, not `39G`, for the same reason the structured pipeline hands a
`Size` over as a number: a caller comparing needs one, and a caller showing has `oslo.ui`.

### `$PATH` is a list

```lua
oslo.env.path_add("~/.local/bin")               -- front, once, absolute, tilde expanded
oslo.env.path_add("./node_modules/.bin")        -- relative to where you are now
oslo.env.path_add("/opt/fallback", { last = true })
oslo.env.path_add("/usr/share/man", { var = "MANPATH" })
oslo.env.path_remove("/nix/*")                  -- a pattern; answers how many went
oslo.env.has_path("~/.cargo/bin")
for _, dir in ipairs(oslo.env.path()) do … end
```

The alternative is string surgery on `oslo.env.get("PATH")`, and each of its edge cases is one
somebody hits: the missing separator, appending where prepending was meant so the project's tool
loses to the system one, a reload that grows the variable every time, `./bin` resolving against
wherever the shell later stands, and the empty entry a trailing colon leaves — which means "the
current directory" to the dynamic linker. These were `oslo.direnv.path_add`, behind a feature; they
are in every build now, over the same implementation `oslo.direnv.path_add` uses.

### Bytes, summarised and carried

```lua
oslo.hash.sha256("hello")               -- lower-case hex
oslo.hash.file("/usr/bin/oslo")         -- streamed; the file is never held in memory
oslo.hex.encode(oslo.fs.read("k.bin"))  -- and oslo.hex.decode back
oslo.base64.decode(token)               -- wrapping at 64 or 76 columns is ignored
```

These only became possible when a shell value could hold arbitrary bytes: hashing what
`oslo.fs.read` used to answer for a binary file hashed a lossy rendering, giving a checksum that
matched nothing and no sign of why. `oslo.base64` hides nothing and protects nothing — it is a
change of alphabet. `oslo.secret` is what encrypts.

`oslo.fs` gained two that were awkward to write by hand: `oslo.fs.touch(path)` does both halves —
creating what is missing *and* dating what is not, where `append(path, "")` does only the first —
and `oslo.fs.usage(dir)` adds up a tree into `{ bytes, files, dirs, unreadable }` without following
symlinks, which over `oslo.fs.walk` in Lua would cost a `stat` and a boundary crossing per file.

**A directory neither can read is counted rather than fatal**, and both say how many they skipped —
`usage(dir).unreadable` and `walk(dir):unreadable()`. That is what lets `/etc` be walked or measured
by somebody who is not root: the walk used to *raise* at the first unreadable subdirectory, ending
the loop with a fraction of the tree seen and no way to catch it.

### Processes and filesystems, as fields

```lua
local p = oslo.proc.info(oslo.proc.pid())
-- p.pid, p.ppid, p.name, p.state, p.threads, p.rss, p.size, p.uid,
-- p.command, p.argv, p.exe, p.cwd
for _, child in ipairs(oslo.proc.children(oslo.proc.pid())) do … end

local d = oslo.fs.disk(".")   -- { total, free, available, files, files_free }
```

`ps` is the canonical thing to shell out to and the canonical thing to get wrong: its columns are
padded to a width that depends on their contents, `COMMAND` contains spaces so it cannot be the
*n*th field, and a process whose name has a space in it — which is allowed — breaks every
`awk '{print $2}'` ever written. `p.argv` is the arguments exactly as they were passed, because
`/proc/<pid>/cmdline` is NUL-separated. `p.state` is `"zombie"`, not `"Z"`. `p.rss` is bytes.

`p.exe` and `p.cwd` are nil for a process that is not yours — those are `/proc` symlinks only the
owner may read, and nil says "unavailable" where `""` would look like a process with no executable.

`oslo.fs.disk` answers for the filesystem the path is *on*, so any path on the mount does.
**`free` and `available` differ on purpose**: a filesystem reserves a percentage for root, so
`available` is what you may write and `free` is what exists. `df` shows the first.

### History as rows, not as a file

```lua
for _, c in ipairs(oslo.history.commands{ limit = 200 }) do
  -- c.line, c.mode, c.runs, c.last_at, c.dir, c.places, c.worked, c.session, c.host, c.root
end

oslo.history.forget("curl -H 'Authorization: Bearer …' …")
```

The finder over this is one opinion about what to show. **Anybody else's needs the rows** — "what do
I run in this project", "which of my commands have only ever failed", "what did I run last week and
not since" are each one loop over a table, and none of them is a feature the shell should grow. A
history *file* cannot answer any of them; it is a list of lines. The tracker has been writing the
directory, the count, the exit status and the rest on every command since long before there was
anything to read it with.

`forget` takes the line out of every directory *and* out of the log, not only the aggregate — a
half-forgotten line is one that comes back on the next start. Having it as a function rather than
only as the finder's Delete key means "forget anything matching this" can be a rule applied on every
start, which a key press cannot be.

**Only an interactive shell has a store.** A script, an `oslo -c` and a subshell see an empty
history — they have nowhere to write to either. `commands()` answers an empty list rather than nil
there, so the loop above is safe in a file that might be run either way.

### Being told when a file changes

```lua
local watch <close> = oslo.fs.watch("src", { "write", "create", "delete" })
oslo.every(500, function()
  for change in watch do
    if change.name:match("%.rs$") then oslo.spawn{ "cargo", "check" } end
  end
end)
```

The kinds are `write` (saved — one event per save, not one per `write(2)`), `modify`, `create`,
`delete`, `move`, `attrib`, `open` and `read`; no list means all of them. A change carries `name`,
`path`, `kind` and `directory`.

**Polling is the interface, and that is not laziness.** A Lua handler runs only at a safe point — a
command boundary or an idle prompt — so a callback promising "when the file changes" would be a lie
about *when*. This is a queue instead: the kernel fills it whether or not anyone is looking, the
handle drains it when a timer gets round to it, and nothing is lost in between. That last part is
what a `stat`-and-compare loop cannot do at all, since it only ever sees the state a file ended in.

It does not recurse — inotify watches a directory, not a tree — and `<close>` is what releases the
kernel's watch.

### A failure carries facts, and is still the message

`nil, message` is the convention everywhere in `oslo.*`, and the second value is now an object:

```lua
local text, err = oslo.fs.read("/nope")
print(err)                       -- /nope: No such file or directory (os error 2)
print(err.kind, err.code)        -- not-found  2
if err.kind == "permission" then … end
```

`err.kind` is one of `not-found`, `permission`, `exists`, `invalid`, `truncated`, `timeout`,
`interrupted`, `other`; `err.code` is the errno; `err.path` is what the call was about, and
`err.to` as well for `rename` and `copy`. `oslo.db` adds `name` and its own `kind`.

**Everything that read the message still reads it.** `tostring(err)` and `print(err)` give the same
sentence as before, `"oops: " .. err` concatenates, and `err:find(…)`, `err:match(…)` and
`err:upper()` work because `__index` falls through to the string library. The only observable
change is that `type(err)` is now `"table"`.

The point is that matching English was the only way to ask what went wrong, and a translated C
library or a reworded message broke it. The kind and the errno are the facts; the sentence is a
rendering of them.

## Configuration

**The configuration is a Lua program, and it is one program even when it is several files.**

```lua
-- init.lua
local oslo = require "oslo"      -- the same table the global names, not a copy of it
require "aliases"                -- aliases.lua, beside this file
```

`require "oslo"` answers with the table `oslo` *is*. That is worth stating because the obvious
implementation gets it wrong: values crossing the shell↔VM boundary are converted, so a `preload`
that handed back a shell-side value produced a fresh table per call — and
`require("oslo").completion.max_rows = 42` was written into a copy nothing would ever read. The
registration is done in Lua, against the global, so identity holds and a setting lands.
`tests/config_modules_tests.rs` holds that to it.

The search path is set by `lua::api::policy` from the environment at startup:

```lua
-- ~/.config/oslo/?.lua and ?/init.lua — the config's own directory, so a second file
-- beside init.lua is `require "aliases"` rather than an absolute path spelled out.
-- Then ~/.config/oslo/lua/, for a library kept apart from the config that uses it.
-- Then the system 5.4 directories. Rooted at $XDG_CONFIG_HOME, else $HOME/.config.
print(package.path)

-- So a library of your own lives beside the config that requires it:
--   ~/.config/oslo/lua/mine.lua
local mine = require("mine")

-- Both are ordinary Lua values, so a config can extend the path, or register a
-- module that has no file at all. `preload` wins over the filesystem, so a
-- host-provided module shadows a file of the same name rather than racing it.
package.path = "/opt/team/lua/?.lua;" .. package.path
package.preload["team.colours"] = function() return { accent = "#89b4fa" } end
```

`package.cpath` is the empty string, not unset: a static binary cannot `dlopen` anything, and
advertising a C path would turn an honest "module not found" into a confusing loader error — but a
`cpath` that is absent breaks `package.cpath == ""`, which is how a script asks.

When the optional [`watch`](watch.md) feature is present, Lua can declare a command service with
`oslo.watch.start { paths = { … }, run = { … } }`. The command is an argv list rather than a shell
string, and the returned handle exposes `name()`, `mode()`, and idempotent `stop()`. The blocking
inotify loop runs in a separate Oslo process; it is not a callback executing inside the Lua VM.
Non-persistent services are stopped when the Lua script or interactive shell normally exits.

## Measurements

`target/release/oslo` at fat LTO, best of three, wall clock including process start (3.6 ms, from
100 empty runs in 0.36 s). The tree-walker column is the figure the previous version of this
document recorded, measured the same way.

| What | tree walker | luna |
| --- | --- | --- |
| 1,000,000 iterations of `for i = 1, N do n = n + i end` | 0.281 s | **0.02 s** |
| 1,000,000 table stores, `x[i] = i * 2` | 0.444 s | **0.05 s** |
| 200,000 calls of a one-line Lua function | 0.093 s | **0.02 s** |
| 100 processes, each running an empty chunk | 0.61 s | **0.36 s** |

The benchmarks live in `bench/lua/`. The `_noos` variants time themselves from outside rather than
with `os.clock()`, which is how these figures include process start.

## What it cannot do yet

The standard library is complete enough that oslo's own tests no longer notice its edges: `os`,
`io`, `package`/`require`, `utf8`, `debug`, `coroutine`, `string.pack` and `_G` are all there,
`pairs` iterates in insertion order, recursion is bounded by a catchable error, and floats print as
Lua 5.4 prints them.

Two things remain, both the VM's:

* **A generic `for` does not close its fourth value.** Lua 5.4 closes the loop's closing value when
  the loop ends, `break` and error included; luna does not. That is what would otherwise have made
  `for line in oslo.lines{…} do … break … end` reap its child by itself, and it is why the
  iterators oslo hands out are `__call`able handles you can also `<close>`.

* **A runtime error is `userdata`, where Lua 5.4 raises a string.** `tostring(err)` reaches the
  message, so nothing is lost — but `err:find("…")`, the idiom for inspecting one, cannot index a
  userdata. `error("…")` and `require`'s "module not found" are already strings; it is the errors
  the VM itself raises that are not.
Neither is reachable from ordinary shell use.

**Three that used to be here are gone**, and what they cost is worth recording. `require` now marks
a module as in progress and reports `loop or previous error in loading module 'x'` rather than
recursing to a stack overflow; `os.setlocale` exists and answers for `C`, which on a static
musl-linked binary is not merely the current locale but the only one — both in
`crates/oslo-runtime/src/lua/api/policy.rs`. And a name assigned a table and then a string *does*
move back to the shell now: a script's non-string globals are kept in a table beside `_G` rather
than in it, so the name never lands there and `__newindex` keeps firing — see
[`globals`](#what-makes-it-different).

That last one has a price, paid deliberately: reading a script's own table global is a metamethod
rather than a raw hit, and `rawget(_G, "x")` and `pairs(_G)` no longer see script globals, there
being no `__pairs` in Lua 5.4 to intercept. The standard library is untouched by both, since those
names really are in `_G`. The alternative — an always-empty `_ENV` with everything in a backing
table, which `Closure::load_with_env` makes possible — has no raw-access divergence at all and
costs a metamethod on *every* global read, `print` included.

**Three names oslo refuses on purpose**, and those are not gaps — see
`crates/oslo-runtime/src/lua/api/policy.rs`. `os.execute` and `io.popen` would run their argument
through `/bin/sh`, another shell started from inside this one; `os.tmpname` names a file without
creating it, leaving a window for somebody else to take the name. Each refusal names its
replacement, and `package.path` is set there too, without stock Lua's `./?.lua` — in a shell,
searching the working directory is a script hijack.

## Where it lives

| Path | Key items |
| --- | --- |
| `crates/oslo-luavm/src/lib.rs` | `Engine`, `eval`, `load`, `call_function`, `is_complete`, `current::publish` |
| `crates/oslo-luavm/src/convert.rs` | `into_lua`, `from_lua`, `raise` — values, metatables, functions, identity |
| `crates/oslo-luavm/src/host.rs` | `Host`, `Native`, `CallbackHost`, `run_nested`, the re-entrancy slot |
| `crates/oslo-luavm/src/globals.rs` | the shell's variables as Lua's global namespace |
| `crates/oslo-base/src/value.rs` | `Value`, `Number`, `Key`, `Table`, `Function` — the shell's own, engine-free |
| `crates/oslo-base/src/value/error.rs` | `LuaError`, `LuaResult` |
| `crates/oslo-runtime/src/lua/engine.rs` | `LuaEngine`, `ShellGlobals`, `call_lua_builtin`, `status_from_lua` |
| `crates/oslo-runtime/src/lua/api/` | the `oslo.*` namespace; `api/util.rs` registers every callable; `api/run.rs` builds `sh` |
| `crates/oslo-shell/src/data/tools/where_.rs` | `where` and `each`, compiling one expression and running it per row |
| `crates/oslo-runtime/src/lua/api/policy.rs` | the names oslo replaces, and where `require` looks |
| `tests/lua_eval_tests.rs`, `tests/lua_corpus/` | the language tests and the hand-written corpus |
