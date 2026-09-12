# What to add next

Forty candidates from three surveys: the mainstream shells (fish, zsh, nushell and friends); ground
the first survey missed (version control, jobs, remote hosts, the editor seam); and the unusual
shells (NGS, Oils/YSH, Hilbish, murex, Elvish, Eshell, marcel, dgsh, ksh93, mksh, tcsh, rc,
expect, IPython). Each one names the existing oslo feature it stands on, and none repeats one.

**Ranked by how much each would change daily use, and how much nobody else has it.**

---

# Tier 1: build these first

## `out`: pipe the output you already have

**Pipe the output of the command you already ran, without running it again.** oslo already emits
`OSC 133` marks, so it knows the byte range of every command's output. Keep those bytes in a bounded
per-session ring (`oslo.out.keep = "4MiB"`, off unless set) and the last output becomes a source.

```console
$ find / -name '*.rs' 2>/dev/null      # forty seconds
$ out | detect-columns | where 'size > 1MB'
$ out -2 --save previous.txt           # out --err, out --list also
```

It is worth it for the slow `find`, the paid API call, or the build log you wanted as rows five
seconds too late. A key that inserts `out | ` replaces pressing Up and retyping the line.

**Builds on** the transcript and the marks oslo already emits. **Prior art** kitty's
`@last_cmd_output`, wezterm and Warp: all on the terminal side, all limited to scrollback.
**Effort** medium to hard, and the riskiest idea here: a careless tee changes `isatty` for the
child, so `ls` stops colouring and `git` stops paging. It has to be a pty.

## `undo`: put back what the last line took away

A journal with one row per destructive builtin operation, tied to the history entry that caused it:
`rm` (the trash entry it already writes), `mv` (the source path), and `>` (a copy of the target taken
just before truncation).

```console
$ undo --list
 id  when     op        path        line
 43  2m ago   truncate  build.log   make > build.log
 42  5m ago   remove    src/old.rs  rm src/old.rs
$ undo 42                # refuses if the target changed underneath it
```

It only runs at the prompt, the same way trash does, so scripts stay plain POSIX and pay nothing.
**Builds on** `rm`'s trash directory and the history's per-line ids. **Prior art** two external
SQLite-journalled `undo` tools (2025), ZFS/btrfs snapshots; no shell ties an undo to a history
entry. **Effort** medium. The hard part is `>`: the copy must happen before the write, so pick a
size above which it refuses, and say so loudly.

## `plan`: what this line would actually do, without running it

```console
$ plan cp $src/*.log $dst
  argv    cp /var/log/app/a.log /var/log/app/b.log /backup
  $src    /var/log/app
  glob    *.log → 2 files
$ plan make > build.log
  > build.log
    ^^^^^^^^^ this file exists and is 4.2 MB; > truncates it
```

It runs **the real expander**, the same code the executor uses, so the answer cannot drift from what
would happen. Putting `echo` in front gets word splitting wrong and says nothing about redirections.
Bound to a key, it previews the line under the cursor. As rows, a pre-command hook can refuse:
`plan "$LINE" | where kind == truncate`.

**Builds on** the expander and the caret renderer. **Prior art** `set -n`/`set -v`, nushell's
`explain` for closures; nothing resolves redirections and `$PATH` ahead of time. **Effort** medium.
`$(…)` can't be evaluated without running it, so it must be marked *unevaluated*, never guessed.

## `stuck`: ask a quiet job what it is waiting for

Samples every process in the job and answers one row per stage: the syscall it is parked in, and
that syscall's fd resolved to a path, a socket peer, or the other end of your own pipe. Two samples
a second apart separate a spin loop from a real block.

```console
$ stuck 3
pid    stage  state  blocked_on  target          last_output  cpu
21455  curl   S      connect(3)  10.0.0.5:443    4m12s        0.0%
21456  jq     S      read(0)     pipe from curl  4m12s        0.0%
```

`stuck --explore` keeps it refreshing; `oslo.jobs.stuck(id)` lets a prompt segment flag a silent
job. **Builds on** `ps` rows and `explore`. **Prior art** strace, lsof, pstack, htop: all reached
for after the fact, none of them the shell that owns the job. **Effort** medium; resolving fds
across pipes, sockets and unlinked files without becoming a `/proc` scraper.

## `norms`: "this usually takes 40s"

Every shell times a command; none compares that time to anything. oslo already records duration,
exit status and directory for every run, so a distribution per command falls out of rows it writes
anyway:

* while running: `cargo build · 40s typical · 4m so far`
* after: `failed 6 of the last 6 times in this directory`
* the one that matters: `first time this has ever succeeded here`

What counts as unusual is Lua (`oslo.norms.slow = function(s) return s.elapsed > 4 * s.median end`),
and `norms | sort-by p95 --desc | first 10` shows what wastes the most time.

**Builds on** the history store behind frecency. **Prior art** none in a shell; Datadog/Honeycomb
anomaly detection, flaky-test flags in CI. **Effort** small. The hard part is deciding when two
lines are "the same command" (`cargo build` vs `cargo build --release`).

## `mute`: mute a pipeline stage without deleting it

With the cursor in a stage, Ctrl-M mutes it: the text stays, dimmed and struck through, and
`a | b | c` with `b` muted runs `a | c`. `solo` is the inverse: run only up to the stage under the
cursor, to see what stage 2 hands stage 3. History stores what actually ran, alongside the muted
words.

```console
curl -s api/rows | jq '.items[]' | grep -v draft | head -20
                                   ^^^^^^^^^^^^^ Ctrl-M → runs curl | jq | head
```

**Builds on** the lossless AST (stage boundaries) and per-token styling. **Prior art** mute/solo on
every DAW channel strip, disabled Jupyter cells; every shell makes you edit the text. **Effort**
medium; carrying "muted" through the AST into execution and history as one representation.

## `trace`: `set -x` as rows

```console
$ oslo trace -- ./deploy.sh | where status != 0 | cols line as_typed expanded
$ oslo trace -- ./deploy.sh | explore
```

One typed row per executed command (`seq depth file line function as_typed expanded status ms pid`),
written to its own descriptor so the script's stderr stays clean. `set -x` keeps printing `+ …`
unchanged. Scrolling a 4,000-line trace in `explore` is the difference between debugging a deploy
script and giving up.

**Builds on** the emitter already at `crates/oslo-shell/src/exec/simple/trace.rs`. **Prior art**
`BASH_XTRACEFD` (plain text), nushell's `debug profile` (closures only). **Effort** medium; keeping
`seq` and `depth` coherent across forks.

## `glob qualifiers`: `*.log(older 7d, larger 1M)`

```console
$ rm *.log(older 7d, larger 1M)      # Tab →
$ rm build/a.log build/b.log
```

The qualifier is matched by oslo and **expanded in place into words**, like an abbreviation. The
script never sees a new construct: what runs and what history records is the literal filenames, so
`sh` semantics are untouched. Qualifiers: `(.)` files, `(/)` dirs, `(@)` links, `older`, `larger`,
`newest 10`, `empty`, `x`, `mine`, and plugins can add more (`tracked`, `staged`).
`oslo.glob("*.log", { older = "7d" })` is the same matcher from Lua.

**Builds on** abbreviation expansion and `where` over `ls` rows. **Prior art** zsh's `*(.om[1,10])`,
the one zsh feature people say they can't replace; fish refuses it. **Effort** large. It must be
inert outside the prompt, since `(` after a word already means something, and it must decide what
to do when one qualifier matches four thousand files.

---

# Tier 2: strong, build next

## `detach`: hand a running job to a scratch session
`detach %1` moves a running job (pty, scrollback and all) into a named scratch session and gives
you your prompt back. `scratch attach build` picks it up from another terminal or machine;
`attach %1 here` pulls it back. This works without ptrace when oslo owns the pty controller
(`oslo.jobs.pty = true`).
**Builds on** scratch sessions and the control socket. **Prior art** reptyr, dtach, tmux (which
can only adopt what it started), `disown`. **Effort** large.

## `stream`: `first 3` should stop the producer
Lua tools return all rows at once today, so `ls -R / | first 3` walks the whole disk. A tool may
instead supply `emit = function(argv, input, yield)`; `yield(row)` returns false once downstream
has had enough. This also allows producers that never end (`tail`, log following, `feed` readers).
**From** PowerShell's one-object-at-a-time pipeline. **Effort** large; delivering cancellation
through Lua to an external process without wedging it on a full pipe.

## `jump`: every `file:line` from the last output, numbered
```console
$ cargo test
$ jump
 1  src/lex.rs:88:9         expected `;`, found `}`
 2  src/spec/man.rs:210:5   unused variable: `takes`
$ jump 2                    # opens $EDITOR there; jump --qf fills nvim's quickfix
```
Recognisers are Lua (`oslo.jump.reader{…}`), and rows carry OSC 8 links. **Builds on** `out`'s
buffer and the table picker. **Prior art** vim quickfix, terminal link detection. **Effort** medium.

## `keep`: snapshot before git eats your work
Before `reset --hard`, `checkout --`, `branch -D`, `rebase`, `push --force` or `stash drop`, oslo
commits the index and worktree to `refs/oslo/keep/<history-id>`, then lets the command run.
`keep` lists them; `keep restore <id> --as branch rescue/x` brings one back. It complements `undo`
(oslo's own builtins) by covering external programs. **Prior art** jj's operation log,
git-branchless; `git reflog` never covers the dirty worktree. **Effort** medium.

## `try`: every exit code the pipeline produced, as rows
```console
$ try { sort a.txt | uniq -d | wc -l; } | where 'status != 0' | cols stage argv status
```
One row per process that ran, including those inside `$(…)` and `<(…)` whose statuses bash
discards, with file and line from the AST. `oslo.last_error()` gives Lua the same rows. **From**
YSH's `try` and `_pipeline_status`. **Effort** medium; collecting statuses the executor never
waits on individually today.

## `why`: the long version of the last error, and a hint
Every oslo diagnostic gets a stable code (`OSLO-E014`); `why` explains the last one. Hints are
also registrable for other programs, using stderr the transcript already kept:
```console
$ git push
fatal: The current branch has no upstream branch
$ why
git exited 128: no upstream for 'feature/x'. Hint: git push -u origin feature/x
```
**From** Oils' searchable error ids. **Effort** medium; once printed, codes can never be
renumbered.

## `explain`: every word on the line, labelled
```console
$ explain 'tar -xzf pkg.tgz -C /opt'
tar        archive tool
  -x       extract
  -f       use archive file  ← consumes pkg.tgz
  -C /opt  change to directory before extracting
```
Offline, from the completion specs, `argc` blocks and man pages oslo already loads, so it works on
your own scripts too. **Prior art** explainshell.com (online, generic). **Effort** medium; specs
don't reliably say whether a flag consumes the next word, so it needs a way to say "not sure".

## `doc`: one paragraph for exactly the word under the cursor
`doc rsync --delete` prints that flag's description and nothing else, from the same sources Tab
walks (spec, `argc` block, man page), naming which. With no arguments it reads the word under the
cursor and draws above the prompt. The text is already parsed; today it only shows eight
characters wide in the dropdown. **Effort** small.

## `park`: set a half-typed line aside
Ctrl-Q pushes the line onto a per-session stack; the next prompt pops it back with the cursor
where it was. `oslo park "git push --force-with-lease"` lets any tool **propose** a command that
arrives typed at your next prompt for review, instead of running it. **Prior art** zsh's
`push-line`. **Effort** small, an afternoon.

## `notify`: Lua rules for when a finished command should interrupt you
```lua
oslo.notify.rule { when = function(j) return j.seconds > 30 and not oslo.term.focused end, how = "desktop" }
oslo.notify.rule { when = function(j) return j.exit ~= 0 end, how = "session:laptop" }
```
Delivery picks OSC 99, OSC 777 or `notify-send` from detected capabilities; `session:` routes to
another oslo. `notify log` shows what fired. **Effort** small.

## `toolchain`: the versions a repo pins, on `$PATH`, no shims
Reads `.tool-versions`, `mise.toml`, `.nvmrc`, `.python-version` and `rust-toolchain.toml` on
directory entry, and prepends each installed toolchain's bin directory. A pin that isn't installed
is a diagnostic naming the file, never a silent fall back to the system version. Resolvers are Lua
(`oslo.toolchain.resolver{…}`). **Builds on** directory environments. **Prior art** mise, asdf,
direnv. **Effort** medium.

## `decode`: structured pipelines from ordinary commands
`oslo.register_parser{ claims = function(argv) … end, parse = function(text) … end }`, so
`ss -tan | where 'state == "ESTAB"'` works because something claimed `ss`. It dispatches on the
command that produced the bytes rather than guessing from their shape as `detect-columns` does.
`jc` becomes one low-priority fallback. **From** NGS's `decode`. **Effort** medium.

## `derive`: your own column, and which columns show
```lua
oslo.rows.derive{ tool = "ps", columns = { mb = function(r) return r.rss / 1e6 end },
                  show = { "pid", "name", "mb" } }
```
The column works in `where` and `sort-by`, and `show` sets the default display, so `ps` stops
drawing `cmdline` while the row still carries it. Columns are lazy. **From** PowerShell's
`Update-TypeData` and `DefaultDisplayPropertySet`. **Effort** medium.

## `spread`: once per row, in parallel, results as columns
```console
$ ls | where 'name ~ "\.png$"' | spread 8 -- optipng {name} | where status != 0
```
Each row comes back with `status ms out err`, in input order, with `--fail-fast`, `--retry 2`,
and Ctrl-C cancelling the pool. **Prior art** `xargs -P`, GNU parallel, nushell's `par-each`.
**Effort** medium.

## `heat`: a script's source with time painted on it
`heat ./deploy.sh` shows the script back with a duration per line, hot lines tinted and calls
foldable. Time attaches to AST spans, so loops report totals and spread, and `waited` separates
blocked from working. `heat --rows` feeds `explore`. **Prior art** flame graphs, `PS4`+`date`
hacks. **Effort** medium.

## `pty`: keep a command's terminal behaviour inside a pipe
`pty -e cargo build 2>&1 | where 'line ~ "warning"'` stays line-buffered and coloured.
`oslo.visual = { "less", { "git", "log" } }` names commands that always get a real terminal.
**From** expect's `unbuffer`. **Effort** medium; nothing in oslo allocates a pty today.

## `sink`: `>` to things that aren't files
`/dev/clip` (OSC 52), `/dev/scratch:NAME`, `/dev/rows:VAR`, `/dev/void`, plus
`oslo.register_sink{…}`. A sink applies only when no such file exists, and never under `--posix`,
so a redirection can only start working where it used to fail. **From** Eshell's virtual targets.
**Effort** medium.

## `distill`: turn the last hour into a script
`distill --since 'last cd' --check > rebuild.sh` keeps the lines that worked, drops typos a later
line corrected, adds the `cd` and the variables they read but never set, and turns values that
varied into an `argc` header. **From** IPython's `%save` and `%macro`. **Effort** medium.

---

# Tier 3: good, smaller or narrower

* **`branch history`**: history records `repo`, `branch` and `head`, so the ghost suggestion after
  `git switch release/2` is what you ran on that branch. *Small.*
* **`unsaved`**: running a script your editor holds unsaved gets a caret: "6 unsaved lines in
  neovim". One RPC, 20 ms deadline, silent when there's no editor. *Small.*
* **`frames`**: a failure three functions deep shows every call site with a caret, innermost
  first; `frames` prints the stack as rows. From Elvish's tracebacks. *Medium.*
* **`origin`**: `origin ll`, `origin --kind key ctrl-g` and `origin --kind hook pre-cmd` give the
  file, line and plugin that defined each. From murex's FileRef. *Medium.*
* **`modifiers`**: `@dir=~/src @priv git push` is a one-off cwd and a line left out of history,
  and plugins can register more. Prompt only. From Hilbish. *Medium.*
* **`trap check`**: `trap` parses its body when you set it, so a broken handler fails at the
  `trap` line with a caret instead of at Ctrl-C. From rc. *Small.*
* **`bang`**: `!$:h` and tcsh's full event, word and modifier algebra, expanded into the visible
  line like an abbreviation, so no script ever sees a `!`. *Medium.*
* **`grab`**: `${ cmd; }` and `${| cmd; }` capture without a subshell, and in an assignment
  `v=${| ls }` keeps rows as rows. From mksh and bash 5.3. *Medium.*
* **`bytes`**: J8 escaping (`b'…\yff'`) and a `!type` header, so tabs, newlines and bad UTF-8
  survive `to tsv` and `from tsv` and types come back. From YSH. *Medium.*
* **`collect`**: `collect { for h in $HOSTS; do …; emit host=$h up=no; done } | where …` builds
  rows from plain shell. From NGS and YSH. *Medium.*
* **`rewrite`**: `sort -u f <>; f` writes through a temp file and renames on success; at the
  prompt, `sort f > f` is rerouted with a warning. From ksh93. *Medium.*
* **`hold`**: `ps | hold fat` keeps a named copy of a row stream to `lookup` against later. From
  marcel. *Small.*
* **`feed`**: `make | … | feed build.progress` publishes the latest row, and any session's prompt
  segment or `notify` rule can read it. From dgsh. *Medium.*
* **`offer`**: other programs ask the running shell `sh.completion.at(line, cursor)` over the
  control socket and get rows back; `oslo complete -- 'git switch '` for one-off calls. From
  readline 8.3. *Small.*

---

# Where to start

* **Afternoons:** `park`, `notify`, `bang`, `trap check`, `grab`.
* **A day or two, mostly presentation:** `doc`, `branch history`, `norms`, `unsaved`, `origin`,
  `why`. They show data oslo already parses or writes.
* **Best value per line of new code:** `plan`, `trace`, `heat`, `try`, `stream`.
* **The ones nobody else has:** `norms`, `mute`/`solo`, `stuck`, `unsaved`, `heat`, `feed`.
* **The bold ones:** `out` and `detach` live or die on the pty; `glob qualifiers` is the biggest
  fight and the biggest win; `feed` and `sink` change what a redirection can mean.
* **Pairs, where the first halves the cost of the second:** `out` → `jump`, `plan` → `check`,
  `try` → `why`, `decode` → `derive` → `bytes`. `undo` and `keep` are the same instinct, one for
  oslo's builtins and one for other programs.
* **One coherent project:** `decode`, `derive`, `bytes`, `sink` and `stream` together fix the weak
  ends of the structured pipeline: how rows get in, how you add to them, how they get out.

---

# Considered and rejected

* **`repo`**: git porcelain as typed rows. It ranked second overall; it was cut for scope, not
  quality, and is the first to promote if a bigger project is wanted.
* **`check`**: shellcheck at the prompt, value-aware. It scored highest, but it overlaps `plan`
  and `explain`; revisit after `plan`, which builds most of its plumbing.
* **`worktrees`**: a worktree per branch with its own scratch session and `.env.lua`. Should
  follow `scratch` and `detach`.
* **`rehearse`**: run for real but hold the writes until you keep or scrap them. The most exciting
  idea and the least buildable: it needs an overlay filesystem or a syscall interposer.
* **`result`** (es): capture what a command *returned*, not what it printed. Needs a return
  channel the executor lacks; `try` gets the nearest part of it.
* **`sample`** (nix repl): complete column names from the rows actually flowing. Needs the
  pipeline to be speculatively runnable at completion time.
* **`knob`** (Mathematica): a slider on a command that redraws the rows. Wants `explore` to become
  a live document.
* **`record`** (autoexpect): do an interactive thing once, get a script. Strong; lost only to the
  forty above.
* **`rewind`** (Ammonite): mark state, mess about, restore. Where "state" ends is a research
  problem once the filesystem is included.
* **`cell`**, **`inside`**, **`lsp`**, **`crew`**, **`drift`**, **`tour`**, **`habits`**,
  **`checklist`**: each is a product rather than a feature.
* **`stage`**/**`chan`**, **`syntax`**, **`spoof`**/**`setter`**, **`under`**: real power, and an
  invitation to build a shell nobody else can debug.
* **`bench`**, **`shape`**, **`ren`**, **`peek`**, **`keys read`**, **`examples`**, **`startup`**,
  **`key modes`**, **`abbr suggest`**, **`wire`**, **`extract`**, **`review`**: covered by
  something above, or too narrow.
