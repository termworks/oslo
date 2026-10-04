# oslo

A POSIX shell in Rust that also speaks Lua, with a structured pipeline that scripts written before
it existed provably cannot reach. Linux only.

```sh
oslo                       # a prompt
oslo script.sh arg1        # run a shell script
oslo build.lua             # a Lua script — same command, no flag
oslo -c 'echo hello'       # run a command
```

<!-- demo:begin -->
[![oslo](https://asciinema.org/a/1262743.svg)](https://asciinema.org/a/1262743)
<!-- demo:end -->

Every feature has its own page and its own recording in [docs/features](docs/features/), each made
by a script in [scripts/demo](scripts/demo/) so it can be made again after the code changes. This
page is the tour; those pages are the reference.

---

## Two languages, one prompt

Shift+Tab switches between shell and Lua **in place** — your line, your cursor and your history stay
where they are. Each language keeps its own history, completion and colouring.

```
bresilla@tron | I | sh  > ls -la | grep rust
bresilla@tron | I | lua > for _, f in ipairs(sh.ls(".")) do print(f.name) end
```

A `!` prefix runs one Lua line from a shell prompt without changing mode. History keeps what it
always had — `!!`, `!$`, `!5`, `!-2`, `!1:2..4` — and a space is how you say you meant Lua: `!5` is
event five, `! 5 + 5` is ten.
→ [two-languages-one-prompt.md](docs/features/two-languages-one-prompt.md)

## Structured pipelines

A command produces two things: text for you, rows for the next command. The pipe decides which
**before anything runs**, by reading what each stage declares — never by looking at the bytes.

```sh
df | where 'free < 1e9' | sort-by free
ps | where 'not is_kernel' | first 5 | cols name
ls | where 'size > 1000' | sort-by size | cols name size_human
ps | group-by is_kernel | count                      # and distinct, stats
cat /etc/passwd | parse '{user}:{x}:{uid}:{rest}' | where 'uid > 1000' | get user
```

Filters are **Lua**, not a dialect invented for the occasion, so the escape hatch is the same
language as the filter. An ordinary command may sit at either end — where the tools stop, what they
made is handed on as bytes:

```sh
kubectl get pods -o json | from json | where 'status.phase == "Running"' | cols name
ps | first 5 | to json | jq .
```

→ [structured-pipelines.md](docs/features/structured-pipelines.md)

## Stream coordinates

A stage can address what the stage before it printed, by position — the job `xargs` exists for,
without `xargs`:

```sh
cat hosts.txt | ssh {0:0} uptime               # line 0, word 0 of what `cat` printed
cat hosts.txt | ping {*:0}                     # word 0 of every line — one process, many arguments
cat one.txt   | echo "ran {%0:0} on {%0:1}"    # {%n} is the stage; {n} is what it printed
```

Every value arrives as one argument, so a filename with a space stays one filename.
→ [stream-coordinates.md](docs/features/stream-coordinates.md)

## POSIX, where it counts

432 corpus scripts run under oslo and under bash and compared byte for byte, plus a rule that every
extension oslo added is unreachable from shell written before oslo existed. That second half is a
build failure rather than a promise: `tests/posix_stays_on_the_byte_path.rs` runs the whole corpus
and requires zero structured edges. → [posix-fidelity.md](docs/features/posix-fidelity.md)

## The rest

| | |
|---|---|
| [The prompt](docs/features/the-prompt.md) | named segments with priorities, gathered once — and a segment may animate |
| [What a line leaves behind](docs/features/transcript.md) | the prompt replaced by what was run, drawable by another program |
| [The line editor](docs/features/line-editor.md) | oslo owns the row it edits — buffer, layout, redraw, keymaps |
| [Ghost suggestions](docs/features/ghost-suggestions.md) | the grey continuation, five sources you order yourself |
| [Prediction and repair](docs/features/prediction-and-repair.md) | a model of what you run: what comes next, and what you meant |
| [Completion](docs/features/completion-and-matching.md) | the dropdown, matching as a transform rather than a prefix test, and carapace specs |
| [Globbing](docs/features/globbing.md) | bash's `**` and options exactly, non-UTF-8 names intact, `*.log(older 7d)` at the prompt |
| [The Lua interpreter](docs/features/lua-interpreter.md) | Lua in pure Rust — what lets a static musl binary speak it with no C toolchain |
| [Your own tools](docs/features/your-own-tools.md) | `register_tool`, builtins and autoloaded functions from Lua |
| [Hooks](docs/features/hooks.md) | thirty-two moments a config can attach to |
| [Timers](docs/features/timers.md) | `oslo.after` and `oslo.every` — the only things that mean "later" |
| [Asking for something](docs/features/userin.md) | thirteen widgets, at a prompt, from any shell, or from Lua |
| [The terminal](docs/features/terminal-integration.md) | what oslo tells the terminal, and what it asks it |
| [What gets written down](docs/features/what-gets-written-down.md) | the log, the outcomes, and what is deliberately not recorded |
| [The history finder](docs/features/history-finder.md) | full-screen search with scopes that narrow and widen |
| [Profiles](docs/features/profiles-and-histories.md) · [syncing](docs/features/syncing.md) | keeping an agent's commands out of yours; two machines agreeing |
| [Where you have been](docs/features/where-you-have-been.md) | directory tracking, `cd -N`, `cd root` |
| [Directory environments](docs/features/directory-environments.md) | `.env.lua` per project, with an allow gate and an undo record |
| [Watch services](docs/features/watch.md) | inotify-backed command reruns from the CLI, `.env.lua`, or recipe inputs |
| [nix, as data](docs/features/nix.md) · [the calculator](docs/features/math.md) | every `nix --json` answer as a Lua table; `math '3 km in miles'` |
| [rm, and the things that can bite](docs/features/rm-and-safety.md) | recoverable at the prompt, POSIX in a script |
| [Scratches](docs/features/scratch.md) · [plugins](docs/features/plugins.md) · [secrets](docs/features/secrets.md) | sessions that outlive a terminal; somebody else's Lua; values kept encrypted |
| [Colours](docs/features/theme.md) · [drawing](docs/features/drawing.md) · [nav](docs/features/nav.md) | every role settable; the output widgets; the filesystem navigator |
| [Abbreviations](docs/features/abbreviations.md) · [macros](docs/features/macros.md) · [argc](docs/features/argc.md) | `gco ` becomes `git checkout `; one store for all of it; options declared in comments |
| [Interrupt escape](docs/features/interrupt-escape.md) · [runtime features](docs/features/runtime-features.md) | the job that will not take a Ctrl-C; turning things off at runtime |
| [The control socket](docs/features/control-socket.md) | another program asking this shell a question — or moving it — in Lua, bound only when asked |

## Configuration

One file, `~/.config/oslo/init.lua`, and it is Lua rather than a settings dialect:

```lua
oslo.suggest.sh_sources = { "history", "completion", "path" }
oslo.completion.fuzzy   = "smart"
oslo.keys["ctrl-g"]     = function(line) return line.text .. " --help" end
oslo.on.pre_cmd(function(c) if c.argv[1] == "rm" then print("careful") end end)
```

`oslo config` inspects and edits it. Every setting, hook and key is on the page for the feature it
belongs to; the table above is the map.

This repository carries its own copy in `config/`, and `make configs` installs it — `config/*`
becomes `~/.config/oslo/*`. That is the one command between editing the config in a checkout and
the shell reading it.

### The hooks

Thirty-two moments a config can attach to, named `pre-`, `post-` or `on-`. `oslo hook list` prints
the current set; [hooks.md](docs/features/hooks.md) says what each one is handed and what a return
value means.

## Tools

Fourteen of them — `macros`, `config`, `profile`, `history`, `direnv`, `make`, `watch`, `hook`,
`fmt`, `lua-api`, `plugin`, `scratch`, `userin`, `secret` — each with its own help. A script of the
same name always wins.

## Building

You do not have oslo yet, so the build is a script:

```sh
scripts/build.sh            # static musl release, every feature — the binary to use
scripts/build.sh --minimal  # static release, none of the optional features
scripts/build.sh --native   # this machine's target, for a quick local binary
nix build                   # the same static musl binary, through the flake
```

Once you have it, the build is [`.make.lua`](docs/features/build-recipes.md) and the shell runs it:

```sh
oslo make            # every recipe, with what each of them says it does
oslo make build      # the same static release
oslo make dev        # a plain debug build, for iterating
oslo make verify     # fmt, line limits, README paths, tests, clippy, rustdoc — all of it
oslo make install    # to $PREFIX/bin and /usr/bin
```

At an oslo prompt in this directory, `make` alone is enough — the builtin hands the word over to the
program everywhere else. There is no `Makefile`: `scripts/build.sh` exists precisely because
`.make.lua` cannot build the shell that reads it.

### Nix binary cache

Only the latest release per package and architecture is protected from cache
cleanup. Pins use `*-latest-*` with `--keep-revisions 1`; older releases may
need rebuilding after garbage collection.

The shared Cachix cache is `termworks`. CI publishes only on pushed tags matching
`v*`. Use a published release tag to download cached packages. The flake advertises
the cache URL and public key; accept the cache configuration when Nix prompts.
Replace `vX.Y.Z` with a tag whose cache workflow has succeeded:

```sh
nix build github:termworks/oslo/vX.Y.Z
nix build github:termworks/oslo/vX.Y.Z#oslo-minimal
```

When consuming Oslo as an input of another flake, configure the cache on the
consumer machine as well. With the Cachix CLI installed:

```sh
cachix use termworks
```

Alternatively, add these settings to `/etc/nix/nix.conf` and restart the Nix
daemon if your installation uses one:

```ini
extra-substituters = https://termworks.cachix.org
extra-trusted-public-keys = termworks.cachix.org-1:Ty7sSVALfD5ajbcWBIdaNHcaEx3fEmVrOo+rSzy0mvE=
```

The consumer must request the same build inputs as CI. Overriding the package's
Nixpkgs input or changing features can require a new build. Branch revisions are
not published to the cache unless they are also tagged with a matching release tag.

#### Publishing setup

1. In the `termworks` cache settings at [Cachix](https://app.cachix.org), generate
   a per-cache write token using Cachix-managed signing.
2. In the GitHub repository's **Settings → Secrets and variables → Actions**,
   add the token as `CACHIX_AUTH_TOKEN`. An organization Actions secret restricted
   to selected repositories can be shared by other Termworks projects.
3. Commit the workflow, then push a release tag matching `v*` that includes it.
   Branch pushes, release events, and manual dispatch do not trigger publishing.

`.github/workflows/nix-cache.yml` builds both variants on native x86_64 and ARM64
Linux runners. It uploads their runtime closures after the flake's install checks
pass, then verifies downloads and shell startup on fresh runners with builders
disabled. Both variants are pinned by release tag against cache garbage
collection. Pull requests do not run this publishing workflow.

The write token is a secret: do not commit it or place it in `flake.nix`.
Other repositories can publish to the same cache with their own Nix build jobs.

### Optional features

All twelve are off *by default*, and off for the same reason: a shell that is going to be `/bin/sh`
should carry what every session needs and nothing else. `scripts/build.sh` turns them on; the published
release artifact is the default build.

Each cost is what turning that one feature *off* takes back out of the full build, measured on the
static musl binary — **4,448,192 bytes with none of them, 5,608,928 with all twelve**:

| feature | costs | brings |
|---|---:|---|
| `argc` | +300 KB | a script declares its options in comments and the shell parses them; the only one that vendors a parser |
| `vista` | +297 KB | the model: `predict` as a suggestion source, `oslo.repair`, `oslo.predict.*`, and the correction after a mistyped line |
| `direnv` | +140 KB | `.env.lua` read on arrival in a directory, the `direnv` builtin, `oslo.direnv` |
| `secrets` | +108 KB | the filing: `oslo secret`, several stores, `secret run`, the lazy variable, the hooks. No crypto of its own |
| `math` | +96 KB | `math '3 km in miles'` and `oslo.math` — dimensions, so `3 km + 2 s` is a refusal |
| `plugin` | +80 KB | `oslo plugin` — installing somebody else's Lua. `oslo.db` and the `pre-cmd` veto are in **every** build |
| `crypt` | +72 KB | the built-in mechanism, so a fresh install encrypts without being told anything |
| `nix` | +48 KB | `oslo.nix` — every `nix --json` answer as a Lua table, and flake-output completion |
| `scratch` | +44 KB | named sessions that outlive their terminal, and the key that finds them |
| `make` | +28 KB | `.make.lua` — recipes with dependencies and staleness, the `oslo make` tool and the `make` builtin |
| `watch` | +60 KB | event-driven command reruns, declarative services, recipe input watching, and Scratch hosting |
| `spec` | +20 KB | a `.yaml` per command in [carapace-spec](https://github.com/carapace-sh/carapace-spec) format, found by name; the completion *model* it fills is in every build |

`crypt` implies `secrets`, so the two can only be removed together: 180 KB for the pair.

```sh
scripts/build.sh --minimal     # static release, none of them
```

**There are no others**, and in particular none that serve the test suite — `--all-features` turns
on exactly the twelve above. A config is written to work either way, because a build without the
feature simply does not have the name:

```lua
oslo.keys["f4"] = function(line)
  return oslo.repair and oslo.repair(line.text) or line.text
end
```

Every `.rs` file is under 600 lines, enforced by `scripts/check-loc.sh`.

To make it the system `/bin/sh` — the symlink, the dpkg diversion that survives a dash upgrade, what
to check afterwards and how to undo all of it — see [docs/default-shell.md](docs/default-shell.md).

## Known gaps

`coproc`, `select`, associative arrays, and process substitution on a system with no `/dev/fd`. Each
is listed with its cause and what to write instead in [docs/known-gaps.md](docs/known-gaps.md), and
each one *says so* rather than quietly doing something else — a syntax error naming the construct,
or a builtin refusing the option.

## Licence

MIT.
