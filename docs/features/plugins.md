# Plugins

Something you install once and then have, written in Lua, using only public `oslo.*` API. A plugin
adds commands to the shell you type at — and nothing else in oslo had to change for it to, because
`oslo.register_builtin` and the hooks were already there.

> ## The database and the veto are in every build; installing is not
>
> Two halves, and the line between them is where policy starts.
>
> | | `oslo` | `oslo-minimal` |
> |---|---|---|
> | `oslo.db` — a database a config owns | yes | **yes** |
> | a hook that can decline to record a line | yes | **yes** |
> | `oslo plugin`, the runtimepath, loading | yes | no |
>
> A database and a hook that can decline to write something down are *capabilities*: small,
> self-contained, and useful to a config that will never install anything. Behind a switch they
> would give oslo two dialects, where a config has to ask whether `oslo.db` exists before using it.
>
> Loading is different in kind — it runs somebody else's code and lets it
> reserve command names. A `/bin/sh` on a distribution has no use for any of that.
> It is behind the **`plugin`** cargo feature and costs **78 KB**: 5,797,328 bytes without it
> against 5,877,472 with. In `oslo-minimal` the word `plugin` falls through to `$PATH`.

<!-- demo:begin -->
[![plugins demo](https://asciinema.org/a/1263436.svg)](https://asciinema.org/a/1263436)
<!-- demo:end -->

## What a plugin is

A directory with `plugin/` in it — the same shape a config root has, which is why one can be
developed beside your `init.lua` and moved into a package later without being edited:

```text
notes/
  plugin/notes.lua   run at startup, alphabetically
  lua/helper.lua     modules for `require`, never run on their own
  after/plugin/      run after everything else
```

```lua
-- notes/plugin/notes.lua
oslo.register_builtin{ name = "note", run = function(argv)
  print("note: " .. (argv[2] or "nothing"))
  return 0
end }
```

**`plugin/` runs, `lua/` is required.** A file under `plugin/` is a statement oslo executes for you;
a file under `lua/` does nothing until something requires it, which is where a plugin's helpers go.
A plugin gets its root as `...`, so it can read a file it ships:

```lua
local root = ...
local f = io.open(root .. "/data.txt", "r")
```

## The runtimepath

oslo follows **neovim's model**, which is also [hexe's](../../../hexe) and [trek's](../../../trek):
an ordered list of roots, each laid out the same way inside.

```text
~/.config/oslo                 yours
/etc/xdg/oslo                  the system's
~/.local/share/oslo/site       where packages install
  + site/pack/*/start/*        each one, as its own root
~/.local/share/oslo/runtime    oslo's own
…/after                        the same list, reversed
```

The `after` half is the first half reversed, so your own config directory is both first and last.

**Installing is putting a directory on the path.** There is no install command, no manifest and no
approval:

```console
$ cp -r notes ~/.local/share/oslo/site/pack/mine/start/
$ oslo plugin list
```

`plugin list` prints the path and every file that would run, in the order it would run them. A `-`
marks a root that does not exist yet.

## Order

`init.lua` first, then `plugin/**/*.lua` from every root in path order, then the `after` roots. Files
are sorted within a directory, because directory order is filesystem order and differs between
machines.

It used to be the other way round — `conf.d` first, so a hand-written file always beat anything a
package dropped in. That reads well until two *plugins* disagree, where it decides nothing at all.
**`after/plugin/` is the seam that does both**: a line that must win goes in
`~/.config/oslo/after/plugin/`, and that works between two plugins as well as against one.

## Trust

There is none, and that is deliberate. What is on the path runs, because you put it there — a prompt
would only ask you to confirm a decision you already made by copying the directory in.

oslo used to gate plugins on a content hash recorded at install. Any edit revoked it, including the
author's own, so a plugin under development went silently off after every save. It also decided
nothing about what a plugin may *do*: a plugin is Lua with the whole `oslo.*` API, which includes
running commands.

What survives is the one boundary a hash never gave you: **which of your secrets a plugin may read**,
now granted by you rather than declared by it.

```lua
-- ~/.config/oslo/init.lua, which runs before any plugin
oslo.plugin.secrets("notes", { "gh-token" })
```

A plugin nobody granted anything reads nothing. Denying by default is the only safe way round.
Only `init.lua` grants: `oslo.plugin.secrets` raises while a plugin's file is loading and once
plugins have started loading, so a plugin cannot grant itself anything. What the grant does and
does not hold back — a hook a plugin registers runs with your full reach — is in
[secrets](secrets.md#what-a-plugin-may-reach).

## When something misbehaves

```console
$ oslo --noplugin
```

Starts with none of them, which is how you answer "is it me or a plugin?" `OSLO_NOPLUGIN=1` does the
same for a whole shell. Then `oslo plugin doctor` for the path and what would run from it, and
`oslo plugin doctor <name>` to load that one and ask its own health checks.

A plugin that raises while loading is reported and the rest still load — deliberately unlike
`init.lua`, where a raise is fatal.

## What it cannot do

- **No `oslo <plugin>` subcommand.** `oslo -c` and scripts never read `init.lua` and never walk the
  path; a plugin extends the shell you type at, and a script depending on one would break for
  anybody who had not installed it.
- **Sandbox anything.** A plugin is Lua with the whole `oslo.*` API, which includes running commands.
  The memory ceiling on a load stops a runaway table, not a hostile author.
- **Survive its own bugs.** A plugin that raises while loading is reported and skipped; one that
  hangs hangs the shell.
- **Resolve dependencies, or find plugins by name.** There is no registry. A plugin that needs
  another says so in prose.
- **Pick up a plugin installed mid-session.** The path is walked once at startup, the same rule the
  config follows.

## Where it lives

| path | what |
|---|---|
| `crates/oslo-base/src/store.rs` | the database behind `oslo.db`: names, limits, where files go |
| `crates/oslo-runtime/src/lua/api/db.rs` | `oslo.db` itself — `open`, and the handle's verbs |
| `crates/oslo-runtime/src/startup/repl/precmd.rs` | what a `pre-cmd` answer means, and when the line is written down |
| `crates/oslo-base/src/quiet.rs` | the flag `set -x` reads, held while a hidden command runs |
| `crates/oslo-runtime/src/runtimepath.rs` | the path of roots, and what runs from each |
| `crates/oslo-runtime/src/plugin/mod.rs` | loading at startup, and the secrets grant |
| `crates/oslo-runtime/src/plugin/doctor.rs` | the path, and a plugin's own health checks |
| `src/cli/plugin.rs` | `oslo plugin list / doctor / test` |
| `tests/plugin_tests.rs` | the path and loading, through the real binary |
