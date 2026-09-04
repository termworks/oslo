# Features

One document per feature: what it is, how it works, what it cannot do, and where it lives in the
tree. These go deeper than the README — the README says a feature exists, these say how it is built
and why it is built that way.

Every claim in here was checked against the source. Where a design was forced by a measurement or by
a bug, the document says so, because those are the sentences worth reading twice.

## Two binaries

A release publishes two per architecture, and one page below describes something only the first has:

| | |
|---|---|
| `oslo` | every optional feature. `make build` |
| `oslo-minimal` | none of them — the floor a distribution would ship as `/bin/sh`. `scripts/build.sh --minimal` |

**[Prediction and repair](prediction-and-repair.md) is `oslo` only.** It is behind the `vista`
cargo feature, so `oslo-minimal` has no model: it learns nothing, writes no `.model` file, offers no
`predict` suggestions, draws no correction after a mistyped line, and has neither `oslo.repair` nor
`oslo.predict`. A config written for one runs under the other — `oslo.suggest.sh_sources` still accepts
`"predict"` and simply gets no answer from it — as long as it asks before calling a name:
`if oslo.repair then … end`.

**[Scratches](scratch.md) are `oslo` only**, behind the `scratch` cargo feature. In `oslo-minimal` the key that
opens the finder is unbound and does whatever it otherwise would; the `oslo.scratch` settings are still
read and simply mean nothing, so one config works under both with nothing to ask.

**[Directory environments](directory-environments.md) are `oslo` only**, behind the `direnv` cargo
feature — the largest of the three at 140 KB, and off because it is the one part of the shell that
reads a file on arrival in a directory and can run what it finds there. In `oslo-minimal` `cd` is
just `cd`, and the word `direnv` falls through to `$PATH` so the real one still works.

**[Build recipes](build-recipes.md) are `oslo` only**, behind the `make` cargo feature. In
`oslo-minimal` there is no `oslo.make`, no `oslo make` tool and no `make` builtin, so the word falls
through to `$PATH` and GNU make answers — which is what it does on every other shell.

**[Watch services](watch.md) are `oslo` only**, behind the independent `watch` cargo feature. A
build without it has no `oslo watch`, `oslo.watch`, or `oslo.fs.watch`; `scratch`, `direnv`, and
`make` gain their watch integrations only when both corresponding features are present.

**[The `text` verbs](structured-pipelines.md#text--strings-where-several-of-them-are-rows) are
`oslo` only**, behind the `text` cargo feature — `split`, `replace`, `trim` and the rest, answering
a pipeline in rows rather than lines, with
[`path`](structured-pipelines.md#path--the-same-machinery-pointed-at-filenames) beside them under
the same flag. In `oslo-minimal` neither name is registered, so both fall through to `$PATH` like
any other word.

**[Universal variables](universal-variables.md) are `oslo` only**, behind the `universal` cargo
feature — one file, replaced atomically, re-read when it has changed. In `oslo-minimal` there is no
`set -U` and nothing ever looks for the file, so `set` is exactly the `set` POSIX describes.

**[nix, as data](nix.md) is `oslo` only**, behind the `nix` cargo feature — every `nix --json`
answer as a Lua table. Independent of `direnv`: the one thing the two share,
`oslo.direnv.nix_develop()`, needs both. In `oslo-minimal` there is no `oslo.nix`, and a config asks
for it the way it asks about anything optional — `if oslo.nix then … end`.

**[Plugins](plugins.md) are half `oslo`-only.** `oslo.db` — a database a config owns — and the
`pre-cmd` veto that lets a hook decline to have a line recorded are in **both** binaries: they are
capabilities, and a config should not have to ask whether they exist. *Installing* is behind the
`plugin` cargo feature at 80 KB, because fetching somebody's code and deciding whether to trust it
is not something a `/bin/sh` does. In `oslo-minimal` the word `plugin` falls through to `$PATH`.

**[Secrets](secrets.md) are `oslo` only**, and are two cargo features rather than one. `secrets` is
the filing — stores, names, `oslo secret run`, the lazy variable, `oslo.secret`, the hooks — at
108 KB and one package, with no crypto of its own; `crypt` is the built-in mechanism — a sealed box,
a key you keep and recipients you publish — at 72 KB and seventeen more. A distribution can ship the first alone and name the machine's own tool. In `oslo-minimal`
there is neither.

**[Arguments in comments](argc.md) is `oslo` only**, behind the `argc` cargo feature and the largest
of them at 300 KB — it vendors a parser and brings five crates oslo does not otherwise link. In
`oslo-minimal` there is no `argc` builtin and no `--argc-eval`, so the word `argc` falls through to
`$PATH` and the real one still works.

**[Completion spec files](completion-and-matching.md#spec-files) are `oslo` only**, behind the
`spec` cargo feature — a `.yaml` per command, in carapace-spec's format, found by the name of the
command being completed. The *model* it fills in — positions, flag values, persistent flags — is in
both binaries and reachable from a config either way; the feature is the reader for the file. In
`oslo-minimal` a spec directory is a directory.

**[The calculator](math.md) is `oslo` only**, behind the `math` cargo feature at 96 KB — `math '3 km
in miles'` and `oslo.math`, with dimensions rather than a table of conversion pairs. In
`oslo-minimal` the word `math` falls through to `$PATH`.

**[Syncing](syncing.md) is in both, and carries one part fewer in `oslo-minimal`.** History and
macros travel from either binary; secrets are behind the feature above, so a build without them has
no `secrets` part to name rather than one that is named and refused.

Everything else on this page is in both binaries.

Each document opens with a recording of the feature actually running. They are not screencasts
somebody performed: every one is a script in [`scripts/demo`](../../scripts/demo/), driven into a
real shell by [`record.sh`](../../scripts/demo/record.sh), so any of them can be made again after
the code changes and a recording that stops matching the shell is a bug in one or the other.

```sh
scripts/demo/fixture.sh                          # the directory the demos run in
scripts/demo/record.sh scripts/demo/nav.demo     # re-record one
scripts/demo/publish.sh nav                      # upload it, remember the id
scripts/demo/embed.sh                            # put the players back in the documents
```

## The two languages

| | |
|---|---|
| [Two languages, one prompt](two-languages-one-prompt.md) | Shell and Lua at the same prompt, switched in place |
| [The Lua interpreter](lua-interpreter.md) | Lua in pure Rust — what lets a static musl binary speak it with no C toolchain |
| [Your own tools](your-own-tools.md) | `register_tool`, builtins and autoloaded functions from Lua |
| [Hooks](hooks.md) | The points a config can attach to, and what a return value means |
| [Timers](timers.md) | `oslo.after` and `oslo.every` — the only things that mean "later" |
| [Drawing](drawing.md) | The shell's own output widgets, and taking them over |

## The pipeline

| | |
|---|---|
| [Structured pipelines](structured-pipelines.md) | Rows for the next stage, text for you — decided before anything runs |
| [Stream coordinates](stream-coordinates.md) | `{0:1}` — a stage addressing what the one before it printed, and what it was |
| [POSIX, where it counts](posix-fidelity.md) | What a script is guaranteed, and the corpus that proves it |
| [The job that will not take a Ctrl-C](interrupt-escape.md) | Why the shell never sees the keystroke, and the watcher that does |
| [Diagnostics](diagnostics.md) | A caret under the word that was wrong — and one line for anything reading stderr |
| [`oslo fmt`](formatting.md) | A formatter that only moves whitespace, because the tree keeps every byte |

## Typing

| | |
|---|---|
| [The line editor](line-editor.md) | oslo owns the row it edits: buffer, layout, redraw, keymaps |
| [Ghost suggestions](ghost-suggestions.md) | The grey continuation, and the five sources you order yourself |
| [Prediction and repair](prediction-and-repair.md) | A model of what you run: what comes next, and what you meant |
| [Completion and matching](completion-and-matching.md) | The dropdown, and matching as a transform rather than a prefix test |
| [Abbreviations](abbreviations.md) | `gco ` becomes `git checkout ` in the buffer, where you can see it |
| [Macros](macros.md) | `oslo macros` — aliases, abbreviations, functions, scripts and variables, in a database with a manager |
| [Arguments in comments](argc.md) | A script declares its options in comments and the shell parses them |
| [A calculator that knows units](math.md) | `math '3 km in miles'` — dimensions, so `3 km + 2 s` is a refusal |

## Memory

| | |
|---|---|
| [What gets written down](what-gets-written-down.md) | The log, the outcomes, the chains, and what is deliberately not recorded |
| [The history finder](history-finder.md) | Full-screen search with scopes that narrow and widen |
| [Where you have been](where-you-have-been.md) | Directory tracking, `cd -N`, `cd root`, and ranking that puts match quality first |
| [One shell, several histories](profiles-and-histories.md) | `$OSLO_PROFILE`, and keeping an agent's commands out of yours |
| [Syncing between machines](syncing.md) | `oslo profile sync` — history, macros and secrets over ssh, deletions included |

## The environment

| | |
|---|---|
| [Directory environments](directory-environments.md) | `.env.lua` per project, with an allow gate and an undo record |
| [nix, as data](nix.md) | Every `nix --json` answer as a Lua table, extended in Lua |
| [Build recipes](build-recipes.md) | `.make.lua` — a justfile in the language the config is already in |
| [Watch services](watch.md) | inotify-backed command reruns from the CLI, directory environments, and recipes |
| [The filesystem navigator](nav.md) | `nav`: type to filter, arrows to move, Esc to take the shell there |
| [rm, and the things that can bite](rm-and-safety.md) | Recoverable at the prompt, POSIX in a script |
| [Scratches](scratch.md) | Named sessions that outlive the terminal they were opened in |
| [Plugins](plugins.md) | Somebody else's Lua, installed once — with a database and a trust gate |
| [Secrets](secrets.md) | Encrypted at rest, decrypted when something asks — with the crypto itself replaceable |
| [The control socket](control-socket.md) | Another program asking this shell a question — or moving it — in Lua, bound only when asked |
| [Universal variables](universal-variables.md) | `set -U` — one value every session sees, in one file, with no daemon |

## Appearance and control

| | |
|---|---|
| [Asking for something](userin.md) | Thirteen widgets — at an oslo prompt, from every other shell, and from Lua |
| [The prompt](the-prompt.md) | Named segments with priorities, gathered once — and a segment may animate |
| [What a line leaves behind](transcript.md) | The prompt replaced by what was run, marked so a terminal can fold it |
| [Colours](theme.md) | Every role settable, with inheritance and background detection |
| [The terminal knows what is happening](terminal-integration.md) | What oslo tells the terminal and the multiplexer |
| [Features you can turn off](runtime-features.md) | A runtime mask over your configuration, never an assignment to it |

## Reading these

Each document has the same shape:

```
How it works          the mechanism, with a diagram
What makes it         the contrast with bash, zsh or fish — stated only where it
  different             could be checked
Configuration         spellings verified against the code that reads them
Measurements          real numbers only; the section is absent when there are none
What it cannot do     required, and never empty
Where it lives        paths and the types or functions that matter
```

The **What it cannot do** section is the one to read first if you are deciding whether to rely on
something. It is required in every document precisely because a feature list that only lists wins is
not documentation.
