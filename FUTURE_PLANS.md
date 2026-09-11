# What to add next

Ranked by how much each one would change daily use and how much nobody else has it. Every idea
builds on something oslo already ships. None of them repeats a shipped feature.

---

## Tier 1: the ones worth building first

### `out`: pipe the output you already have
```console
$ cargo build            # 400 lines of errors
$ out | grep E0308       # filters those lines; the build does not run again
```
Uses the transcript oslo already keeps of finished lines. Only kitty and wezterm do anything like
this, and both do it in the terminal, not the shell. **Effort:** medium to hard, and the riskiest
item here: capturing what a child writes to the terminal without breaking it.

### `undo`: put back what the last line took away
`rm`, `mv` and `>` over an existing file are recorded before they happen, so one word restores the
state. Half of it already ships as `rm`'s trash directory. **Effort:** medium. The hard part is `>`,
because the copy has to happen before the write does.

### `plan`: see exactly what a line would do, without running it
Every expansion, glob, word split and redirect target is resolved and shown with carets, and
dangerous results such as `rm -rf $EMPTY/` are flagged. Uses the existing expander and caret
renderer. **Effort:** medium. `$(…)` cannot be evaluated without running it.

### `stuck`: ask a quiet job what it is waiting on
Shows what a job is blocked on (a pipe, a socket, a lock, a read on a terminal) and which process
is on the other end, as rows. No shell has ever answered this. **Effort:** medium.

### `norms`: "this usually takes 4s; it took 40"
When a run is far outside that command's own history (slower, or failed where it usually passes),
oslo says so. The history store already has duration, exit status and directory. **Effort:** small.
The hard part is deciding when two lines count as the same command.

### `mute`: mute a stage of the pipeline you are typing
The stage stays in the line, greyed out, and does not run. `solo` does the opposite. The idea comes
from a mixing desk, and no shell has it. **Effort:** medium.

### `trace`: `set -x` as rows
```console
$ oslo trace -- ./deploy.sh | where status != 0 | cols line expanded
```
The xtrace emitter and typed rows both exist already. **Effort:** medium; forks are the hard part.

### `glob qualifiers`: `*.log(older 7d, larger 1M)`
Press Tab and the qualified glob becomes literal filenames on the line, so nothing magic runs. This
is the zsh feature people cannot give up. **Effort:** large; the biggest fight and the biggest win.

---

## Tier 2: strong, build next

| idea | what it does | effort |
|---|---|---|
| `detach` | hand a running job to a scratch session and get the terminal back | large |
| `stream` | `first 3` stops the producer instead of draining all of it | large |
| `jump` | number every `file:line` the last command printed; `jump 3` opens it | medium |
| `keep` | snapshot the git worktree to a hidden ref before a git command that can lose work | medium |
| `try` + `why` | every exit code in a pipeline as rows, and the long form of the last error | medium |
| `explain` | label every word on the line: which flag, what it consumes, where redirects go | medium |
| `doc` | a key on any word shows the one paragraph describing that flag, offline | small |
| `park` | set aside a half-typed line, run something else, get it back | small |
| `notify` | Lua rules for when a finished command should interrupt you, and where | small |
| `toolchain` | walk into a repo and the versions it pins are on `$PATH`, with no shims | medium |
| `decode` + `derive` | parsers for ordinary commands' output, and your own computed columns | medium |
| `spread` | run once per row in parallel and get the results back as columns | medium |
| `heat` | a script's own source with elapsed time painted onto each line | medium |
| `pty` | `pty cmd \| grep x` keeps colours, line buffering and progress bars | medium |
| `sink` | `>` to the clipboard, a scratch session or a Lua function | medium |
| `distill` | turn the lines that worked in the last hour into a script | medium |

---

## Tier 3: good, smaller or narrower

* **`branch history`**: history remembers the git branch and ranks by it.
* **`unsaved`**: a caret before running a script your editor still holds unsaved.
* **`frames`**: a script that fails three functions deep shows all three call sites.
* **`origin`**: which file, line and plugin defined this alias, key or hook.
* **`modifiers`**: one-off `@dir=…` or `@runner=lua` prefixes in front of a command.
* **`trap check`**: a `trap` body is parsed when it is set, not when Ctrl-C arrives.
* **`bang`**: `!$:h` expanded into the visible line the way abbreviations are.
* **`grab`**: `${| … }` captures a command in the current shell and keeps its rows.
* **`bytes`**: tabs, newlines and bad UTF-8 in filenames survive `to tsv` and back.
* **`collect`**: a loop of plain commands in a block, with rows coming out the end.
* **`rewrite`**: `sort f` back into `f` without the moment where `>` empties it first.
* **`hold`**: park a row stream under a name and join against it later.
* **`feed`**: a long pipeline publishes a value other sessions and the prompt can read.
* **`offer`**: other programs ask the running shell what completes at a cursor position.

---

## How to build them

* **Afternoons:** `park`, `notify`, `trap check`, `bang`, `grab`.
* **Pairs where the first halves the cost of the second:** `out` → `jump`, `try` → `why`,
  `plan` → `check`, `decode` → `derive` → `bytes`.
* **One coherent project:** `decode`, `derive`, `bytes`, `sink` and `stream` together fix the weak
  ends of the structured pipeline: how rows get in, how you add to them, and how they get out.

---

## Not doing, for now

* **`repo`**: git porcelain as typed rows. Cut for scope, not quality; first to promote.
* **`check`**: shellcheck at the prompt. Revisit after `plan`, which builds its plumbing.
* **`worktrees`**: one worktree per branch with its own scratch. Should follow `detach`.
* **`rehearse`**: run for real but hold the writes. Needs an overlay filesystem.
* **`record`**, **`rewind`**, **`result`**, **`sample`**, **`knob`**: strong, but each is a research
  project or wants a subsystem that doesn't exist yet.
* **`cell`**, **`inside`**, **`lsp`**, **`crew`**, **`drift`**, **`tour`**, **`habits`**,
  **`checklist`**: each one is a product rather than a feature.
* **`bench`**, **`shape`**, **`ren`**, **`peek`**, **`examples`**, **`key modes`**,
  **`abbr suggest`**, **`stage`**, **`chan`**, **`syntax`**, **`spoof`**, **`under`**,
  **`wire`**, **`extract`**, **`review`**: covered by something above, or too sharp a tool.
