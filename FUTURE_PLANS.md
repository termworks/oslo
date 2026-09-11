# What to add next

Forty candidates, from three surveys. A hundred and sixty-four ideas were proposed across them and
a hundred and fifty-seven were distinct; these are what survived being scored on three separate
questions: does it fit oslo, would anyone actually want it, and can it be built on what is already
here.

Nothing here repeats something oslo ships. Every one names the existing feature it stands on,
because a feature that does not stand on one is a feature for a different shell. Every one is a
thing you could invoke or see — no platform wishes, nothing that resolves to "but better".

* The **first nine** came from fish, zsh, nushell, elvish, murex, xonsh, PowerShell, the standalone
  tooling ecosystem, terminal integration protocols, and the shells that have tried to put an LLM
  behind the prompt.
* The [**second eleven**](#a-second-round) came from ground the first did not touch: version
  control, remote hosts and containers, job control, discoverability, the editor seam, bulk work at
  scale, and a blue-sky lane aimed at music software, games, aviation, spreadsheets and databases.
* The [**third twenty**](#a-third-round-from-the-unusual-shells) were mined from shells almost
  nobody runs — NGS, Oils/YSH, Hilbish, murex, Elvish, Eshell, marcel, dgsh, ksh93, mksh, tcsh, rc,
  expect, IPython — where the good ideas that never spread are still sitting.

Within each round the order is roughly by what the thing would be worth. None of them is the order
to build in — see [Where to start](#where-to-start).

---

## `norms`

**The shell knows how long this usually takes and how it usually ends, and says so when this run is
not usual.**

Every shell times a command. None of them compares that time to anything.

oslo already records duration, exit status and working directory for every execution — that is what
frecency ranks over. A distribution per `(line, directory)` falls out of rows that are already
being written, and with it:

* a right-prompt segment while something long is running — `cargo build · 40s typical · 4m so far`
* a line after it finishes — `failed 6 of the last 6 times in this directory`
* the one that actually matters — `first time this has ever succeeded here`

What counts as unusual is Lua, because it is a matter of taste and not of fact:

```lua
oslo.norms.slow = function(s) return s.elapsed > 4 * s.median end
```

`norms` on its own emits rows — `line`, `runs`, `median`, `p95`, `fail_rate` — so the summary of
what wastes the most time is a pipeline rather than a report someone had to design:

```console
$ norms | sort-by p95 --desc | first 10
```

**Fits because** the history store already holds duration, exit and cwd, and already answers
frecency queries over those same rows. This is a second question asked of data oslo has paid for.

**Prior art** none, in a shell. Latency anomaly detection in Datadog and Honeycomb; flaky-test
flags on CI dashboards.

**Effort** small. The hard part is the dedup key: deciding when two lines are "the same command" so
the statistics mean anything. `cargo build` and `cargo build --release` are probably not the same
question, and normalising on the command word plus the *shape* of the flags is probably the answer.

---

## `plan`

**What this line would actually do, every expansion and every truncation, without running it.**

```console
$ plan cp $src/*.log $dst
cp
  argv    cp /var/log/app/a.log /var/log/app/b.log /backup
  $src    /var/log/app
  glob    *.log → 2 files
  cp      /usr/bin/cp
```

And the part that earns it a place:

```console
$ plan make > build.log
  > build.log
    ^^^^^^^^^ this file exists and is 4.2 MB; > truncates it
```

It runs **the real expander** — the same code the executor runs — so the answer cannot drift from
what would happen. That is the whole design. A `plan` that reimplements expansion is a second
implementation to keep in step, and the first time it disagrees it is worthless.

What people do today is put `echo` in front of the line, which gets word splitting wrong and says
nothing at all about the redirection.

Bound to a key it previews the line under the cursor without leaving the prompt. Emitting rows lets
a pre-command hook refuse:

```console
$ plan "$LINE" | where kind == truncate
```

**Fits because** the expander is already there and the caret renderer already exists to point at
one word in a line.

**Prior art** `set -n` and `set -v` in bash; nushell's `explain` for closures. Nothing resolves
redirections and `$PATH` ahead of time.

**Effort** medium. The hard part is `$(…)`, which cannot be evaluated without running it. `plan`
must mark it *unevaluated* in the output rather than quietly guessing — one lie and nobody trusts
it again.

---

## `out`

**Pipe the output of the command you already ran, without running it again.**

oslo emits `OSC 133` A/C/D, so it already knows the exact byte range of every command's output.
Keeping those bytes in a bounded per-session ring makes the last output addressable:

```lua
oslo.out.keep = "4MiB"   -- off unless set
```

```console
$ find / -name '*.rs' 2>/dev/null      # forty seconds
$ out | detect-columns | where 'size > 1MB'
$ out -2 --save previous.txt
$ out --list
```

`out` writes the last command's stdout, `out -2` the one before, `out --err` its stderr,
`out --list` shows rows — `n`, `cmd`, `bytes`, `status`, `when`.

Because it is a *source* it joins the structured path, and that is the case worth building for: the
slow `find`, the paid API call, the build log you realise five seconds too late you wanted as rows.
A line-editor binding that inserts `out | ` at the cursor replaces the motion people perform now,
which is pressing Up and retyping the whole line.

**Fits because** the transcript of finished lines already establishes that oslo keeps a record of
what happened, and the byte offsets come free from marks it already emits.

**Prior art** kitty's `@last_cmd_output` and wezterm's output selection — both terminal-side, both
limited to what survived the scrollback. Warp's blocks. No shell keeps its own output addressable.

**Effort** medium to hard, and the riskiest thing on this list. A child writes to the terminal
directly, so capturing means a tee or a pty; a careless tee changes `isatty` for the child, and
then `ls` stops colouring and `git` stops paging. It has to be a pty, or a tee on the shell's side
only.

---

## `explain`

**Every word on the line, labelled: which flag that is, what it consumes, what the redirect does.**

```console
$ explain 'tar -xzf pkg.tgz -C /opt'
tar        archive tool
  -x       extract
  -z       filter through gzip
  -f       use archive file  ← consumes pkg.tgz
pkg.tgz    value of -f
  -C /opt  change to directory before extracting
```

The descriptions come from what oslo already loads in order to draw the completion pager: carapace
specs, `argc` blocks in local scripts, registered Lua tools, with man's summary line as the
fallback. `|` and `2>&1` are labelled off the AST rather than from any spec, and the lossless tree
gives exact spans so the carets land on the right bytes.

`oslo.explain(line)` answers rows — `span`, `word`, `kind`, `means` — so it pipes like everything
else.

Three things separate it from the website: it is offline, it works on **your own scripts** because
an `argc` block is a spec, and it explains the line you are holding rather than a generic one.

**Fits because** the spec store already knows every flag's description — it prints them — and the
tree already carries the spans. This is a second reading of data oslo has paid for once.

**Prior art** explainshell.com, a web service over Ubuntu's man pages. Warp's AI explainer. tldr.
Nothing offline, nothing spec-driven, nothing inside a shell.

**Effort** medium. The hard part is arity: carapace specs do not reliably say whether a flag
consumes the next word, so `-f pkg.tgz` and `-x pkg.tgz` need a heuristic — and a way of saying
"not sure" rather than guessing.

---

## `undo`

**Put back what the last line took away.**

A journal with one row per destructive builtin operation, each bound to the history row id of the
line that caused it:

* `rm` — the trash entry it already writes
* `mv` — the source path
* `>` — a copy of the target, taken immediately before the truncation

```console
$ undo --list
 id  when     op        path        line
 43  2m ago   truncate  build.log   make > build.log
 42  5m ago   remove    src/old.rs  rm src/old.rs
$ undo 42
```

`undo` reverses the newest entry, `undo 41` reverses one and refuses if the target changed
underneath it.

The design point that matters: it is **prompt-only in exactly the way trash already is**. In a
script every one of those operations stays plain POSIX and journals nothing, so the
correctness-critical path pays nothing at all.

**Fits because** half of it ships already as `rm`'s trash directory with its size cap, and the
history log supplies a stable per-line id to hang each row on — which is what lets `undo --list`
name the line that did it rather than only the thing that happened.

**Prior art** two external SQLite-journalled `undo` tools appeared in 2025; ZFS and btrfs
snapshots. No shell has it as a builtin, and none ties an entry to a history entry.

**Effort** medium. The hard part is `>`: the copy has to happen before the write, which means
choosing the size at which you refuse — and being loud about refusing.

---

## `glob qualifiers`

**Type `*.log(older 7d, larger 1M)`, press Tab, and it becomes literal filenames before the line
runs.**

```console
$ rm *.log(older 7d, larger 1M)
```

Tab, and the line reads:

```console
$ rm build/a.log build/b.log
```

That is the whole trick, and it is what makes the feature defensible in a POSIX-first shell. The
qualifier is matched, sorted and sliced by oslo and then expanded **in place, into words**, the way
an abbreviation expands. The script never sees a new construct. What runs, and what history
records, is `rm build/a.log build/b.log`. `oslo -c` and `/bin/sh` semantics are untouched, and
there is no flag to turn on.

Qualifiers: `(.)` files, `(/)` directories, `(@)` symlinks, `older 7d`, `larger 1M`, `newest 10`,
`empty`, `x` executable, `mine`. The table is open, so a plugin adds `tracked` or `staged` beside
the built-in ones. `oslo.glob("*.log", { older = "7d" })` is the same matcher from Lua.

**Fits because** expanding at the prompt is already how oslo settles convenience against meaning —
that is what abbreviations are — and the matching itself is `where` over the file rows `ls`
already produces.

**Prior art** zsh, `*(.om[1,10])`. It is the one zsh feature people say they cannot replace, and
fish has refused it outright. Nobody expands them to literal words first.

**Effort** large. The hard part is the boundary: guaranteeing the syntax is inert in
non-interactive parsing, since `(` after a word is already meaningful shell. And deciding what
happens when a qualifier matches four thousand files.

---

## `trace`

**`set -x`, except the trace comes back as rows you can filter instead of a wall of stderr.**

```console
$ oslo trace -- ./deploy.sh | where status != 0 | cols line as_typed expanded
$ oslo trace -- ./deploy.sh | explore
```

One typed row per executed command — `seq`, `depth`, `file`, `line`, `function`, `as_typed`,
`expanded`, `status`, `ms`, `pid` — written to a separate descriptor, so the script's own stderr
stays clean.

`set -x` at a prompt keeps printing `+ …` byte for byte and every existing script is untouched. The
rows are a second *face* on the same event, which is the split diagnostics already have between the
drawn face and the plain one.

Scrolling a four-thousand-line trace in `explore` rather than grepping it is the difference between
debugging a deploy script and giving up on it.

**Fits because** xtrace and typed rows both exist, and this is the most obvious place the two
should have met and have not. The emitter is already at
`crates/oslo-shell/src/exec/simple/trace.rs`.

**Prior art** bash's `set -x` with `BASH_XTRACEFD`, plain text. zsh's xtrace. nushell's
`debug profile`, closures only. No shell emits its trace as records.

**Effort** medium. The hard part is fork boundaries — keeping `seq` and `depth` coherent when
pipeline stages and subshells are all writing the same descriptor.

---

## `park`

**Half-typed a line and need to check something first? Park it, run the other thing, get it back at
the next prompt.**

A `park` action bound through `oslo.keys` — `ctrl-q` by default — pushes the current buffer onto a
per-session stack and clears the line. The next prompt pops the top back in with the cursor where
it was. The prompt sees a `parked` count, so a Lua segment can draw it. `oslo park --list` and
`--drop` show and discard. A parked line never reaches history until it is actually run.

The part that makes it more than zsh's `push-line`:

```console
$ oslo park "git push --force-with-lease origin main"
```

Any tool — `secret`, `make`, a script registered with `register_tool` — can **propose** a command
that arrives typed at your next prompt for review, instead of running it. That is a different
interaction model for anything dangerous, and it costs one stack.

**Fits because** `Session::apply` is already `(state, key) → (state, Step)`, so this is one field
and one `Step`, and it is testable without a pty.

**Prior art** zsh's `push-line` and `print -z`. fish has neither; people paste a gist to fake it.

**Effort** small — an afternoon. The only fiddly part is restoring cursor and multi-line state
exactly as they were.

---

## `toolchain`

**Walk into a repository and the versions it pins are on your `$PATH`, with no shims anywhere.**

Directory environments already own the entry and exit hooks and the `$PATH` mutation. This extends
them to read the pin files that are **already in other people's repositories** —
`.tool-versions`, `mise.toml`, `.nvmrc`, `.python-version`, `rust-toolchain.toml` — resolve each to
an installed toolchain, prepend its bin directory on entry and drop it on exit.

```console
$ toolchain
 tool    want     using    pinned_by            path
 node    20.11.0  20.11.0  .nvmrc               ~/.local/share/node/20.11.0/bin
 python  3.11     3.11.8   .python-version      /usr/bin
 rust    1.75     —        rust-toolchain.toml
```

A pin that is not installed is a diagnostic naming the file and the line. It is never a silent
fallback to the system one, which is the failure mode that makes the existing tools infuriating:
the wrong `python` runs and nothing says why.

```lua
oslo.toolchain.resolver{ name = "node", find = function(want) … end }
```

nix-as-data becomes one resolver among others.

**Fits because** directory environments already do the hard half, and the rows are the same
"explain yourself" shape the rest of oslo's tools have.

**Prior art** mise, asdf, direnv's `use_node`, nix-direnv. All of them either shim or make you
write the glue.

**Effort** medium. The hard part is resolution: "installed" means a different directory layout for
every upstream installer.


# A second round

Eleven more, from a second survey over ground the first one did not touch: version control, remote
hosts and containers, process and job control, discoverability, the seam with editors, bulk work at
scale, and a blue-sky lane pointed at music software, games, aviation, spreadsheets, databases and
observability. Fifty-six proposed, fifty-four distinct, scored the same three ways.

These are numbered from ten onwards only in the sense that they came second. Several of them are
better than several of the first nine.

---

## `doc`

**Press a key with the cursor on any word and get the one paragraph that describes exactly that word — offline, no browser.**

`doc` answers for a single token instead of a page: `doc rsync --delete` prints the description of that flag and nothing else, `doc git commit` prints the subcommand's summary and the arguments it requires. It resolves the token through the same ladder Tab already walks — a carapace spec under `share/completion`, an `@describe`/`@option` block if the command is an argc script, then the man-page parse in `crates/oslo-shell/src/spec/man.rs` — and every answer names its source, so you can tell a maintained description from a scraped one. With no arguments it reads the word under the cursor and draws the paragraph above the prompt without disturbing the line you are typing; `doc --rows rsync` emits `{word, kind, text, source, takes_value}` into the pipeline instead. oslo already parses all of this text and currently only ever shows it eight characters wide in a dropdown column.

```console
$ doc rsync --delete
--delete    delete extraneous files from destination dirs
            takes no value · source: completion spec (rsync)
```

**Fits because** the spec reader in `crates/oslo-shell/src/spec/`, the man-derived specs behind `compgen` and the userin pager all ship; only the presentation is missing.

**Prior art** fish's man-page completion descriptions, `curl --help --data`, kmdr, cheat.sh, IDE quick-docs on hover — none of them answer for the word under the cursor from a local spec corpus.

**Effort** small; deciding the precedence order when three sources describe the same flag differently.

## `stuck`

**Ask a job that has gone quiet what it is actually waiting for.**

`stuck` (or `stuck 3`) samples every process in a pipeline and answers with one row per stage — `pid stage state blocked_on target last_output cpu rss` — where `blocked_on` is the syscall the process is parked in and `target` is that syscall's fd resolved to a path, a socket peer, or the other end of your own pipe. Two samples a second apart give real cpu and io deltas, so a spin loop and a genuine block do not both read as "running". `stuck --explore` opens the rows in the existing viewer and keeps them refreshing; `oslo.jobs.stuck(id)` returns the same rows to Lua, which is enough for a prompt segment that flags a job with no output for a minute.

```console
$ stuck 3
pid    stage  state  blocked_on  target             last_output  cpu    rss
21455  curl   S      connect(3)  10.0.0.5:443       4m12s        0.0%   6M
21456  jq     S      read(0)     pipe from curl     4m12s        0.0%   3M
21457  less   S      read(0)     pipe from jq       4m12s        0.0%   4M
```

**Fits because** `ps` already emits rows and `explore` already views them; this is the same machinery pointed at one job, in the house style of naming the exact thing that was wrong.

**Prior art** strace, lsof, pstack, htop, procs — all reached for after the fact, none of them the shell that owns the job.

**Effort** medium; resolving an fd to a meaningful `target` across pipes, sockets and unlinked files without turning into a `/proc` scraper.

## `mute`

**Mute a stage of the pipeline you are typing — it stays in the line, greyed out, and does not run.**

With the cursor inside a pipeline stage, ctrl-m marks that stage muted: the text stays exactly where it is, dimmed and struck through, and the lossless AST records it as skipped, so `a | b | c` with `b` muted really executes `a | c`. No commenting out, no deleting the grep you will want back in ten seconds. `solo` is the inverse — run the line only up to and including the stage under the cursor, which is how you find out what stage 2 was handing stage 3. History stores the effective line together with the muted words, so `out`, `explain` and a later recall all agree about what ran, and `oslo.line.stages()` returns rows of index/text/muted for a keymap or a plugin to drive.

```console
curl -s api/rows | jq '.items[]' | grep -v draft | head -20
                                   ^^^^^^^^^^^^^ cursor here, ctrl-m → runs curl | jq | head
```

**Fits because** the lossless AST already knows where every stage begins and ends, and the line editor already renders per-token styling for ghost suggestions and caret diagnostics.

**Prior art** mute and solo on every DAW channel strip, disabled cells in Jupyter, commented-out clauses in SQL editors — bash, zsh, fish and nushell all make you edit the text.

**Effort** medium; carrying "muted" from the editor through the AST into execution and history without it becoming a second, divergent representation of the line.

## `jump`

**Every file:line the last command printed, numbered — `jump 3` opens your editor there.**

When a command finishes, oslo reads the output it already captured and pulls out position references — `path:line:col`, rustc and gcc carets, pytest `::` addresses, grep `--vimgrep` — as rows `{n, path, line, col, kind, text}`. Bare `jump` shows them in the `oslo.ui.table` picker, `jump 3` opens `$EDITOR` at that spot, and `jump --qf` pushes the whole list into the parent neovim's quickfix list over `$NVIM` so `:cnext` walks it. Each row is written with an OSC 8 hyperlink, so the same list is clickable in a terminal with no editor attached. Recognisers are Lua rather than a fixed table: `oslo.jump.reader{ name = "phpunit", match = …, row = … }`.

```console
$ cargo test
$ jump
 1  src/lex.rs:88:9         expected `;`, found `}`
 2  src/spec/man.rs:210:5   unused variable: `takes`
$ jump 2
```

**Fits because** the captured output is the same buffer round one's `out` keeps addressable, and OSC 8 hyperlinks plus the table picker already ship.

**Prior art** vim's quickfix and `nvim -q`, terminal link detection in VSCode and WezTerm, `rg --vimgrep | vim -q -` — each needs the editor to own the terminal or the command to be run twice.

**Effort** medium; recognisers that find real positions in noisy output without inventing them.

## `detach`

**Hand a job that is already running to a scratch session, and take your terminal back.**

`detach %1` moves a running job — pty, scrollback and all — into a named scratch, prints the name, and returns your prompt; the process never learns anything happened. `scratch attach build` picks it up from another terminal, another tmux pane, or another machine over the control socket, and `attach %1 here` is the reverse, pulling a parked job back into this terminal as a foreground job. This needs no ptrace or reptyr tricks because oslo can own the pty controller for interactive jobs (`oslo.jobs.pty = true`), and `jobs` gains `where` and `session` columns so the rows say plainly which of your jobs live here and which live elsewhere.

```console
$ detach %1
%1 → scratch "build" (pty kept, 4h12m elapsed)
$ scratch attach build      # from the laptop, over the control socket
```

**Fits because** scratch already runs detached sessions with ptys and replay logs, and the control socket already answers questions from other processes; this is the verb between them.

**Prior art** reptyr, dtach and abduco, screen and tmux (which can only adopt what they started), `disown` (which keeps the job and loses the terminal). No shell moves a live job out of its own terminal.

**Effort** large; handing off the controlling terminal, the process group and the signal path without the child noticing.

## `branch history`

**History remembers which branch you were on, so switching branches brings back what you ran there.**

The history record already carries cwd, exit and duration; add `repo`, `branch` and `head`, which cost nothing because the prompt reads them from `.git` on every line anyway. Then `history | where branch == 'release/2'` is a real query, the history finder gains a branch filter beside the ones it has, and ghost suggestions rank commands run on *this* branch above global frecency — so the first suggestion after `git switch release/2` is last week's deploy incantation rather than something from main. `history stats | group-by branch` shows where the time actually went.

```console
$ history | where branch == 'release/2' and exit != 0 | sort-by when desc | first 5 | cols when,cmd,exit
```

**Fits because** the history record, the finder and frecency-ranked ghost suggestions all exist — this is one more column on a row that is already written on every line.

**Prior art** atuin records cwd, host and session but not branch; fish and zsh record neither; none of them rank suggestions by repo context.

**Effort** small; ranking that prefers the current branch without starving the global frecency that makes suggestions useful outside a repo.

## `spread`

**Run a command once per row, several at a time, and get the results back as columns on those same rows.**

`spread` is a pipeline verb: rows in, one child per row, N at a time, and each row comes out with `status ms out err` appended — so the failures are a `where status != 0` away instead of a wall of interleaved text. Output order follows input order regardless of who finishes first, `--fail-fast` and `--retry 2` do what they say, and Ctrl-C cancels the pool rather than one child. While it runs, a single status line names how many are running, done and failed, plus the slowest row still open. Lua gets the same thing as `oslo.rows.spread(rows, 8, function(r) … end)`.

```console
$ ls | where 'name ~ "\.png$"' | spread 8 -- optipng {name} | where status != 0 | cols name,status,err
```

**Fits because** structured pipelines already carry typed rows through `each` and `map` and already have the streaming and backpressure machinery.

**Prior art** `xargs -P` (no ordering, no per-item status), GNU parallel (`--joblog` is a separate file you join by hand), nushell's `par-each` (in-process, no exit columns), rust-parallel.

**Effort** medium; ordered output from unordered completion while streaming, without buffering the whole result set.

## `notify`

**Rules, in Lua, for when a finished command is worth interrupting you — and where the interruption lands.**

A rule is a predicate over the finished job, so the threshold is yours and it can depend on anything: `oslo.notify.rule{ when = function(j) return j.seconds > 30 and not oslo.term.focused end, how = "desktop" }`, fired from `on-job-finish`. Delivery picks OSC 99 where the terminal has it and OSC 777 where it does not, falling back to `notify-send`, all decided by the capability detection oslo already does rather than by a setting you maintain. `notify -- make deploy` is the one-off form, and `how = "session:laptop"` routes over the control socket to another oslo, so a build on the desktop taps you on the machine you moved to. `notify log` emits rows — `when command status seconds route delivered` — so a rule that never fires is visible instead of mysterious.

```lua
oslo.notify.rule {
  when = function(j) return j.seconds > 30 and not oslo.term.focused end,
  how  = "desktop",
}
oslo.notify.rule { when = function(j) return j.exit ~= 0 end, how = "session:laptop" }
```

**Fits because** `on-focus-change`, `on-job-finish` and terminal capability detection all ship; the missing piece is a place to say when it matters.

**Prior art** ntfy and noti wrap the command, Warp has a global seconds threshold, kitty defines OSC 99, and everyone else has a preexec snippet.

**Effort** small; deciding delivery from detected capabilities rather than from configuration.

## `keep`

**Before any git command that can lose work, oslo commits your worktree to a hidden ref you can list and restore.**

A pre-exec guard on the external commands named in `oslo.keep.commands` — by default `reset --hard`, `checkout --`, `branch -D`, `rebase`, `push --force`, `stash drop` — writes the index and worktree as a commit under `refs/oslo/keep/<history-id>`, then lets the command run untouched. `keep` lists rows (when, history id, command, branch, files, bytes) and `keep restore <id>` brings one back as a worktree, a branch, or a diff. This is a different angle from the `undo` already chosen: `undo` journals oslo's own destructive builtins, `keep` snapshots before an *external* program runs, and what it leaves behind is a real commit git can diff and cherry-pick.

```console
$ git reset --hard origin/main
$ keep | first 1 | cols id,when,files,command
id     when   files  command
a41f2  8s     12     git reset --hard origin/main
$ keep restore a41f2 --as branch rescue/a41f2
```

**Fits because** rm safety already intercepts a destructive command before it runs and asks; this is the same interception pointed at the other program that eats work.

**Prior art** jj's operation log, git-branchless working-copy snapshots, sapling `undo --interactive`; `git reflog` covers commits and never the dirty worktree.

**Effort** medium; snapshotting index and worktree fast enough to be invisible on a large repo, and pruning the refs before they become the repo's largest object set.

## `heat`

**Run a script and see its own source with the time painted onto the lines that spent it.**

`heat ./deploy.sh` runs the script, then shows it back to you with a duration column per line, hot lines tinted, and nested calls foldable so a helper's forty seconds can be opened where it was called from. Because the AST is lossless, time attaches to byte spans rather than to line numbers guessed from `PS4`, so a loop body reports its total, its call count and the spread across iterations, and a here-doc or a `$(…)` gets its own row. `heat --rows ./deploy.sh` emits `span line col text calls self total waited` — `waited` being time blocked on a pipe or a read rather than burning in a child — which sorts, groups and opens in `explore` like anything else.

```console
$ heat --rows ./deploy.sh | sort-by self desc | first 5 | cols line,text,calls,self,waited
```

**Fits because** it exists only because the lossless AST can hold a span per word; timers and the explore viewer supply the rest.

**Prior art** flame graphs, otel-cli wrapping commands in spans, `PS4`-plus-`date` profiling hacks, per-step timings in CI. None of them attribute the time back onto the script's own text.

**Effort** medium; attributing wall clock to spans across subshells and pipelines, and separating waiting from working.

## `unsaved`

**Running a script your editor is still holding unsaved gets a caret under the word, before it runs.**

Before executing a file — `./deploy.sh`, `bash x.sh`, a `.` on a script, a `make` recipe whose file is open — oslo asks the editor that owns the terminal which buffers are modified, and if the file about to run is one of them it prints the ordinary caret diagnostic naming it: the buffer, the editor, and how many lines differ from disk. It never refuses and never saves for you. `oslo.editor.unsaved` is `"warn"` (the default when an editor is found), `"ask"` (an `oslo.ui.confirm` offering to write it first), or `"off"`. The query is one RPC call with a 20 ms deadline, and no answer means no diagnostic, so a shell in a bare terminal pays nothing.

```console
$ ./deploy.sh
  ^^^^^^^^^^^ 6 unsaved lines in neovim (buffer 4)
```

**Fits because** caret diagnostics and the pre-exec hook both ship, and "every diagnostic names the thing that was wrong" is the house rule this applies directly.

**Prior art** none in any shell; IDEs sidestep the question by saving for you before a run.

**Effort** small; talking to each editor that can own a terminal, and staying silent within the deadline when none does.


# A third round: from the unusual shells

Twenty more, mined from shells almost nobody runs. The brief was to find what is distinctive to
them rather than what every shell has, and to judge each idea on whether it went uncopied *because
it is bad* or *because the shell it lived in was obscure* — rewarding only the second.

Sixty were mined, fifty-nine distinct, from: NGS, Oils/YSH, Hilbish, PowerShell, murex, Elvish,
Eshell, marcel, dgsh, ksh93, mksh, tcsh, rc, Tcl/expect, IPython, and bash 5.3's readline.

Each names the shell it came from and what that shell actually does, because the adaptation matters
more than the transcription and the original is the evidence.

---

## `decode`

**Structured pipelines that start with an ordinary command, because something registered a parser for that command.**

`detect-columns` guesses from the shape of the bytes, which is why it merges `RSS` and `TTY` on a busy `ps aux`. `decode` dispatches on the argv that produced the bytes instead: `oslo.register_parser{ claims = function(argv) ... end, parse = function(text) return rows end }` registers a parser that claims particular commands, and the planner already knows the argv of the byte-producing stage at the handover into the first tool, so the lookup costs nothing at runtime. `jc` is registered as one low-priority fallback rather than being wired in as a special case. When no parser claims a command the bytes flow exactly as they do today, so nothing that works now changes.

```lua
oslo.register_parser{
  claims = function(argv) return argv[1] == "ss" end,
  parse = function(text)
    return oslo.rows.parse(text, "{state} {recvq} {sendq} {local} {peer}")
  end,
}
-- then:  ss -tan | where 'state == "ESTAB"' | group-by peer
```

**From** NGS, whose double-backtick capture calls `decode(s, hints)` — a multimethod dispatched by `guard` clauses on hints like `hints['process'].command.argv[0] == 'find'`, so whichever registered parser claims the producing command turns its output into data.

**Fits because** it is the missing half of the byte-prefix handover in `structured-pipelines.md`, and it turns `detect-columns`'s admitted guesswork into a registry, the way `oslo.register_tool` already does for whole verbs.

**Effort** medium; the hard part is priority and ambiguity when two parsers claim the same argv, and deciding what a parser gets to see of a command that is still running.

---

## `modifiers`

**One-off overrides typed in front of the command: run it somewhere else, in the other language, without writing it down.**

The line editor peels `@name` and `@name=value` words off the front of an interactive line before the parser sees them, and each desugars onto machinery oslo already owns. `@dir=~/src` is a push/pop around the command, `@priv` is the hook that declines to record the line, `@lua=`/`@sh=` is the per-line language pick two-languages-one-prompt already makes, `@alias=false` and `@abbr=false` suppress expansion, and `@profile=work` picks a history profile for that line only. `oslo.modifiers.register{ name = "time", wrap = function(value, run) ... end }` lets a plugin add its own, so `@keep` or `@plan` become prefixes instead of new commands; the highlighter paints them as their own token and the completer offers the registered names at the start of a line. They are parsed only at the interactive prompt — in a script, in `sh -c`, or in a sourced file, a program named `@priv` on `$PATH` still wins.

```console
$ @dir=~/src/oslo @priv git push --force-with-lease
```

**From** Hilbish, which lets one command line carry stackable `@` flags — `@priv`, `@dir=~/Projects`, `@runner=lua`, `@alias=false` — that change that command and nothing else.

**Fits because** every modifier is an existing knob (language per line, history profile, expansion points, the record-this-line hook) given one uniform prefix, and confining it to interactive input is oslo's rule that conveniences never reach a script.

**Effort** medium; the hard part is the peel happening in the editor without the highlighter, the completer and the transcript disagreeing about where the real command starts.

---

## `derive`

**Add your own computed column to `ls` or `ps` once in Lua, and choose which columns the table shows.**

`oslo.rows.derive{ tool = "ls", columns = { age = function(row) return oslo.now() - row.mtime end } }` adds `age` to every row `ls` produces, everywhere, so `ls | where 'age > 30d'` and `ls | sort-by age` both work, and because the planner reads declared columns before anything runs, `cols aeg` is still refused by name. The second half is the one nobody copies: `show = { "name", "size", "age" }` sets the default display set, so the drawn table is three columns wide while the row still carries all of them and `cols` or `explore` reach the rest. That is what oslo's producers need — `ps` emits `pid`, `name`, `cmdline`, `is_kernel`, and `cmdline` ruins the table every time — and it beats trimming the producer, because trimming loses the data and a display set does not. A derived column is lazy, evaluated when a stage first asks for it, so a `derive` nobody reads costs nothing.

```lua
oslo.rows.derive{
  tool = "ps",
  columns = { mb = function(r) return r.rss / 1e6 end },
  show = { "pid", "name", "mb" },
}
```

**From** PowerShell, whose extended type system bolts members onto every instance of a type session-wide with `Update-TypeData`, and whose `DefaultDisplayPropertySet` decides which of an object's properties a table actually prints.

**Fits because** oslo already declares each producer's columns beside the code that fills them and tests the declaration for drift; `derive` opens that registry to Lua, as `register_tool` opened whole tools.

**Effort** medium; the hard part is laziness — a derived column must not be computed for rows that get filtered out, and a Lua error inside one has to name the column and the row rather than killing the pipeline.

---

## `try`

**`try { ... }` gives you every exit code the pipeline actually produced, as rows.**

`try` takes a brace group, runs it with errexit and errtrace suspended, leaves `$?` exactly as the command before it left it, and emits one row per process or builtin that ran: `stage`, `argv`, `status`, `signal`, `file`, `line`, and the tail of its stderr — including the processes inside `$(...)` and `<(...)` whose statuses bash drops on the floor. Error handling becomes a query instead of an idiom, `failed` is the one-word test for the common case, and Lua gets `oslo.last_error()` returning the same rows. The file and line come from the lossless AST, so the report can carry a caret into the block you wrote.

```console
$ try { sort a.txt | uniq -d | wc -l; } | where 'status != 0' | cols stage argv status
```

**From** YSH, whose `try { ... }` runs a block with errexit suspended, always exits 0 so the handler runs, and fills `_error`, `_pipeline_status` and `_process_sub_status` — because `$?` is tied to the pipeline rule of the grammar and so loses the status in `local x=$(false)`.

**Fits because** it is a rows producer, dropping into the same vocabulary as `history` and `ps`, and it is the per-process status table that `out` and `trace` both assume exists.

**Effort** medium; the hard part is collecting statuses from subshells and process substitutions that the current executor never waits on individually.

---

## `origin`

**Ask what defined this alias, this key binding, this hook — and get the file, the line, and the plugin.**

Every registration already flows through one Lua call, so capture the calling chunk and line at that moment and keep it beside the entry. `origin ll` answers with rows `{ name, kind, source, line, plugin, when }`; `origin --kind key ctrl-g` answers for a binding; `origin --kind hook pre-cmd` lists every handler in attachment order with the file each came from, which is the question the hooks doc's "handlers accumulate, they never replace" design makes unanswerable today. `oslo.origin("ll")` returns the same row to Lua. The diagnostics improve for free: when two plugins both register the builtin `note`, the message names both files instead of silently picking one.

```console
$ origin --kind hook pre-cmd | cols name plugin source line
```

**From** murex, which tags every function, variable, alias, event and autocompletion with a FileRef recording package, file and load time, and dumps the registry with `runtime --fileref`.

**Fits because** plugins and the runtimepath already load other people's code that registers builtins, completions, hooks and keys, and oslo's rule is that every diagnostic names the thing that was wrong.

**Effort** medium; the hard part is attributing a registration made inside a helper function to the plugin that caused it rather than to the helper's own file.

---

## `frames`

**When a script dies three functions deep, oslo shows all three call sites with carets, not just the last line.**

The executor keeps a small frame stack — file, function name, and the span of the call word — costing one push and pop per function call and per sourced file, and the lossless AST already carries the spans, so nothing is re-parsed to print them. On a fatal error the existing caret diagnostic grows a `called from` section, innermost first, each frame printing its source line with the call word underlined. A `frames` builtin prints the current stack as rows from inside a trap or a function, and `oslo.on.error` handlers receive the same rows, so a config can log a structured failure instead of scraping stderr. Only stderr text changes: exit status, stdout and every trap fire stay as they are.

```console
$ frames | cols function file line | to json
```

**From** Elvish, whose uncaught exceptions print `Traceback:` with one entry per frame — `[interactive], line 1:` or a filename and line, each showing the source text of the offending line.

**Fits because** caret diagnostics and the lossless AST already pin every word to a span, and today a failure names only the last of the four places you would want to look.

**Effort** medium; the hard part is keeping the push and pop free enough that deep recursion in a hot script pays nothing measurable.

---

## `stream`

**`first 3` should stop the thing producing rows, not wait politely for all of them.**

oslo's Lua tool contract is `rows = function(argv, input) return { ... } end` — every row materialised before the next stage sees one — so `ls -R / | first 3` walks the whole disk, and an endless producer cannot be written at all. A tool may instead supply `emit = function(argv, input, yield)` and call `yield(row)` per row, where `yield` returns false once a downstream stage has stopped wanting rows and the tool is expected to return. `first`, `each` and `explore` become the stages that say enough, and the drawn table's rule already anticipates it: a stream cannot be a table, so streamed output is a header plus one unpadded line per row. Beyond laziness this buys producers that never end — `tail`, a `feed` reader, log following over a `tree` — each a plain stage with `where` in front of it.

```lua
oslo.register_tool{
  name = "walk",
  emit = function(argv, _, yield)
    for p in sh.walk(argv[1]) do
      if not yield{ path = p } then return end
    end
  end,
}
```

**From** PowerShell, where cmdlets process one object at a time through Begin/Process/End and `Select-Object -First n` stops the upstream command as soon as it has enough.

**Fits because** the pipeline planner already fixes every edge's shape before any stage runs, which is exactly what is needed to know where cancellation can be delivered, and `explore` and `each` are already stages that answer nothing downstream.

**Effort** large; the hard part is delivering cancellation through the Lua interpreter and back out to an external process at the head of the pipeline without leaving it wedged on a full pipe.

---

## `distill`

**Take the lines that worked out of the last hour and hand back a script that runs them.**

`distill 112-130 > deploy.sh` writes those history entries as a real POSIX script, and does three things a history dump cannot: it drops the lines that exited non-zero and the ones a later line corrected — prediction-and-repair already knows which was a typo for which — it emits a `cd` for the directory the lines actually ran in plus a leading block for every variable they read but never set, and it turns the values that varied between otherwise-identical lines into an `argc` header so the script takes them as flags. `distill --since 'last cd'`, `distill --touching ./src` and `distill --last 20` pick ranges without counting. `distill --check` runs `oslo fmt --check` over the result and reports any line that only parses because it was typed interactively — an abbreviation, a bare `math` expression, a two-language line — rather than writing a script that will not run. The session was already a document; this is the export.

```console
$ distill --since 'last cd' --check > scripts/rebuild.sh
```

**From** IPython, which turns a session into source with `%save script.py 1-10 15`, binds history ranges to a re-runnable name with `%macro`, and exports the whole input history as a notebook with `%notebook`.

**Fits because** history already records exit status, cwd, branch and timing per entry, `argc` already declares arguments from comments, and `oslo fmt` round-trips through the lossless AST.

**Effort** medium; the hard part is the read-but-never-set analysis — deciding which names the selected lines depend on from outside themselves, and which were set by a line you dropped.

---

## `why`

**`why` after something failed: the long version of the message, and a hint for the program that actually failed.**

Every oslo diagnostic gets a stable code printed in the drawn report's footer, leaving the frozen one-line transport form byte-for-byte as `tests/diagnostics_stay_plain.rs` holds it. Bare `why` explains the last diagnostic of this session; `why OSLO-E014` explains any of them. The larger half is that hints are registrable for other people's programs, which is where an interactive shell earns its keep: `oslo.hint{ command = "git", status = 128, match = "detached HEAD", text = ... }` means `why` says something useful after a failing external command too, using the stderr the transcript already kept. It is deliberately not `explain` or `doc` — those say what a command is, this says what just went wrong.

```console
$ git push
fatal: The current branch has no upstream branch
$ why
git exited 128 — no upstream for 'feature/x'. Hint: git push -u origin feature/x
```

**From** Oils, where every diagnostic carries a searchable id — `setvar couldn't find matching 'var x' (OILS-ERR-10)` — and an error catalog holds a section per id with what the user probably meant.

**Fits because** the transcript already keeps the last command's stderr, so `why` needs no new capture path, and diagnostics are the feature with the strongest stated rule in the repo.

**Effort** medium; the hard part is that the codes are a promise — once printed they can never be renumbered, so the catalog has to be stable before the first one ships.

---

## `pty`

**`pty cmd | grep x` makes the command behave as if it were still talking to your terminal — line-buffered, coloured, progress bars and all.**

The `pty` builtin allocates a pseudo-terminal, runs the command with it as stdout, and copies through to the real descriptor: `pty -e` for stderr too, `pty -s 200x50` to fix the reported window size, `pty --no-color` to set `NO_COLOR=1` and `TERM=dumb` inside. The same machinery carries a policy layer: `oslo.visual = { "less", "top", { "git", "log" }, { "git", "diff" } }` names command-and-subcommand shapes that always get a real terminal, so `git log | head` stops paging into a pipe. It is a new builtin name, so no existing script changes meaning. It exists because oslo's whole structured-pipeline plan turns on whether a stage's stdout is a terminal, and this is the one place the answer should be a deliberate yes.

```console
$ pty -e cargo build 2>&1 | where 'line ~ "warning"' | first 20
```

**From** Tcl/expect's `unbuffer`, which runs a program on a pseudo-terminal so libc keeps stdout line-buffered when it is redirected — its own example being that `od -c /tmp/fifo | more` shows nothing until a page accumulates.

**Fits because** terminal-integration already does capability detection and owns the `TERM` and window-size questions, and the pipeline planner already branches on whether a stage's stdout is a terminal.

**Effort** medium; the hard part is the copy loop — job control, window-size changes, signals and EOF on the master all have to behave, and nothing in oslo allocates a pty today.

## `hold`

**Park a row stream under a name so a later pipeline can join against it instead of recomputing it.**

`hold procs` is a pass-through stage: rows go out unchanged and a copy is kept under `procs` for the rest of the session, columns and types intact. `hold --list` emits rows of `name`, `rows`, `columns` and `held_at`, and `hold --drop procs` forgets one. Recall goes through the second-stream hatch that already exists — `lookup`, `append` and `merge` take their other side as a Lua expression, so `oslo.rows.held("procs")` is the whole of the new surface; when the planner learns to recurse into an operand, `lookup (hold procs) pid` reads better and means the same thing. marcel spells this `>$`, and that spelling is exactly what oslo cannot take: `>@procs` in a POSIX script is a redirection to a file named `@procs`, and a shell that quietly reinterprets it has broken the one rule. Making the reservoir a verb costs one word and changes nothing.

```console
$ ps | where 'rss > 1e8' | hold fat | length
14
$ ls | lookup 'oslo.rows.held("fat")' pid | cols name pid rss
```

**From** marcel, which stores a stream into a named reservoir with `>$` — `gen 100 1 | red -i * >$ factorials` — and keeps it for the workspace so a later pipeline can read it back.

**Fits because** `out` is already the one-slot unnamed version of this, and `lookup`/`append`/`merge` already accept a Lua expression as their second stream.

**Effort** small; deciding what a held stream costs in memory and when a large one spills rather than growing without a ceiling.

## `offer`

**Another program asks your running shell "what completes here?" and gets rows back.**

`oslo.live` already serves a Lua API over the control socket; `offer` adds `sh.completion.at(line, cursor)` to it, returning rows of `value`, `display`, `kind`, `description` and `source` — carapace spec, Lua provider, filesystem, frecency — for any line, not only the one being typed. An editor's terminal pane, a `nav` preview or a sibling tool then gets the specs, the Lua providers and the frecency ranking without reimplementing a word of it. `oslo complete -- 'git switch '` is the same answer in one exec, for callers with no socket. This is not the rejected completion preview: that was a UI inside the shell, this is an interface out of it.

```lua
local rows = sh.completion.at("git switch ", 12)
for _, r in ipairs(rows) do
  print(r.value, r.kind, r.source)   -- develop  branch  spec:git
end
```

**From** bash 5.3 by way of readline 8.3, which added `export-completions`, a bindable command that writes the current line's completions to stdout in a defined format instead of displaying them.

**Fits because** the control socket is documented as another program asking a running shell a question in Lua, and it already answers `env.get`; completion specs, Lua providers and fuzzy ranking all ship.

**Effort** small; pinning a stable row shape for `kind` and `source` so callers can rank and group without parsing descriptions.

## `trap check`

**`trap` parses its body when you set it, so a broken handler fails at the `trap` line with a caret, not at Ctrl-C.**

When `trap 'code' INT` runs, oslo parses `code` immediately and reports any syntax error at the `trap` call site, naming the trap and the signal, with the caret renderer that every other diagnostic uses. The string is stored unchanged and re-parsed at delivery exactly as POSIX requires, so nothing about the handler's meaning moves — only the moment you find out. `trap` with no arguments emits rows of `signal`, `body`, `parses` and `source`, and `parses` is `false` for a handler that will blow up when it fires. rc gets this for free by making handlers ordinary named functions, parsed at definition time, and Duff's stated reason is precisely this; oslo cannot adopt the form, only the moment.

```console
$ trap 'rm -f $tmp; exit 1' INT
trap: handler for INT does not parse
  rm -f $tmp; exit 1
              ^ unexpected end of input in `$tmp`
  installed anyway; POSIX re-parses at delivery
```

**From** rc, which defines signal handlers as ordinary functions — `fn sigint { rm /tmp/junk; exit }` — and so catches a syntax error in a handler when you write it rather than when the signal arrives.

**Fits because** every oslo diagnostic already names the thing that was wrong with a caret under it, and the trap and signal-delivery paths are already reasoned about carefully in `interrupt-escape.md`.

**Effort** small; keeping the eager parse strictly diagnostic, so a handler that fails to parse is still installed and still re-parsed late.

## `grab`

**`$(...)` forks and flattens your rows to text; `${| ... }` forks nothing and keeps the rows.**

oslo takes both mksh and bash spellings with their published semantics for text — `${ cmd; }` captures output, `${| cmd; }` takes whatever the command left in `REPLY`, both in the current execution context with no subshell — and adds one behaviour only oslo can have: in an assignment, `v=${| ls }` binds the row table rather than the rendered lines, so `$v | where 'size > 1M'` re-enters the structured pipeline with types intact and no `to json | from json` round trip. Because the Lua VM is already in-process, `${| lua stats() }` is the cheapest call in the shell; a fork is exactly what it does not need. Variables set inside survive, which is the whole point of the no-subshell form and the reason `read` inside a `while` loop finally works. `${ ` is a syntax error in every pre-oslo shell, so no existing script can reach the extension.

```console
$ big=${| ls }
$ echo "$big" | where 'size > 1M' | cols name mtime
$ n=0; ${ while read -r l; do n=$((n+1)); done < hosts; }; echo $n
42
```

**From** mksh and bash 5.3, whose `${|command;}` and `${ command; }` run the command in the current shell execution context with no subshell and no fork.

**Fits because** structured pipelines already carry typed rows end to end, and command substitution is the single place that stream is forced back to flat text.

**Effort** medium; running a command body inside the current execution context without leaking its redirections, traps or `set` changes back into the caller.

## `bang`

**The `!$:h` algebra, expanded into the visible line the way an abbreviation is, so you read the text before you run it and no script ever sees a `!`.**

Typing `!$:h` and pressing space rewrites the line in place, exactly as abbreviations already do; nothing expands at parse time, and `--posix` has no `!` handling at all. Events (`!!`, `!-2`, `!vi`, `!?err?`), word designators (`^`, `$`, `2-4`, `*`, `%`) and the modifier chain (`h`, `t`, `r`, `e`, `s/l/r/`, `&`, `g`, `a`, `q`, `x`, `p`) all come across from tcsh, but the modifiers run over oslo's lossless AST rather than over a string: `:t` on `~/src/x/y.rs` knows it is one word, `:q` is real quoting instead of backslash guessing, and `:g`/`:a` iterate over words the parser identified. `!?err?` resolves through the history finder that already exists, and `:p` drops the result onto the line unrun, which is what people actually reach for `:p` to do. Doing it csh's way — at parse time — would change what a script means, so prompt-only is not a compromise here, it is the only correct form.

```console
$ vim ~/src/oslo/config.toml
$ cp !$:h/Cargo.toml .
$ cp ~/src/oslo/Cargo.toml .        # what the line reads before you press return
```

**From** tcsh, whose history substitution combines an event, a word designator and a chain of modifiers into one token that expands at parse time.

**Fits because** abbreviations already rewrite the visible line before it runs, the history finder already does event lookup, and the lossless AST is what lets modifiers operate on words instead of characters.

**Effort** medium; implementing the full modifier chain over AST words while keeping expansion instant enough to sit on the space key.

## `sink`

**`>` can name a thing that is not a file — the clipboard, a scratch, a Lua function — and you can add your own in Lua.**

oslo ships four virtual targets: `/dev/clip` writes through OSC 52, `/dev/scratch:NAME` appends to a detached session's pane, `/dev/rows:VAR` parses the stream by the producer's declared form and stores it as a universal variable, and `/dev/void` discards. `oslo.register_sink{ path = "/dev/notes", open = function(mode) return function(chunk) ... end end }` adds more, mirroring Eshell's open-returns-a-writer shape so a sink can hold state and close cleanly, with `>>` and `>` passed through as the mode so `/dev/clip` can concatenate across two commands. A sink is resolved at redirection time only when the path does not exist on disk and the name is registered; under `--posix` the table is empty and `> /dev/clip` is the `ENOENT` it is today, so no script changes meaning — a redirection can only start working where it previously failed. `sink` with no arguments lists them as rows: path, kind, where it was registered from, bytes written this session.

```console
$ cargo test 2>&1 | tail -40 > /dev/clip
$ ls -l | to json > /dev/rows:listing
$ sink
path            kind      from                  bytes
/dev/clip       builtin   osc52                 3184
/dev/notes      lua       ~/.config/oslo/init.lua   0
```

**From** Eshell, which redirects to buffers, to Lisp symbols and to virtual devices like `/dev/clip` and `/dev/kill`, with new ones added by pushing onto `eshell-virtual-targets`.

**Fits because** redirection already has one resolution point in `exec`, oslo already owns the clipboard through OSC 52 and named sessions through `scratch`, and the rule that a redirected last stage renders to Text is what lets a sink take bytes without special-casing rows.

**Effort** medium; ordering resolution so the filesystem always wins and a registered sink can never shadow a real path.

## `bytes`

**A filename with a tab, a newline or a broken UTF-8 byte survives `to tsv` and comes back the same value.**

The transport face already escapes `\t` and `\n` in a cell, but `lines` and `parse` deliberately never unescape and a `Val::Bytes` cell renders as a description, so the round trip is one-way today. `bytes` adopts J8 at that boundary only: a cell goes out bare when it is plain text and as `b'…\yff'` when it is not, so a reader can tell a quoted cell from a backslash some foreign program wrote, and `from tsv --j8` restores the exact bytes. The same header idea gives `to tsv --typed` a `!type` row, so `from tsv` hands back Size, Time and Number instead of three strings and `where 'size > 1GB'` works on a file you wrote yesterday. The drawn face does not change at all.

```console
$ ls | to tsv --j8 --typed > inventory.tsv8
$ head -2 inventory.tsv8
!type Str Size Time
b'weird\tname' 4096 1757548800
$ from tsv --j8 < inventory.tsv8 | where 'size > 1GB' | sort-by size
```

**From** YSH's J8 Notation, which adds `u'…'` and `b'hi \yff'` string styles alongside JSON strings, plus TSV8's `!tsv8` and `!type` header rows that carry column types out of the process.

**Fits because** it lands on the transport/display split in `structured-pipelines.md`, and `Val::Bytes` exists as a distinct variant precisely so a read that is not text is never quietly lossy.

**Effort** medium; deciding per cell whether it needs quoting, cheaply, on streams with many columns.

## `collect`

**Put a loop of ordinary shell commands in a block and get rows out the other end.**

`collect { … }` runs a brace group with an `emit` builtin in scope; each `emit host=$h up=yes` appends a row, and `collect` itself is a nothing→rows producer, so it plugs into a pipeline the way `ls` does. Nested `collect` blocks stack, which is what makes a two-level structure — a host and its checks — expressible without dropping into Lua. The Lua counterpart is `oslo.rows.collect(function(emit) … end)`, and `register_tool` can be written in terms of it. Columns are Unknown at plan time, which the planner already handles, since nothing may be refused on Unknown.

```console
$ collect {
>   for h in $HOSTS; do
>     ping -c1 -W1 "$h" >/dev/null 2>&1 && emit host=$h up=yes || emit host=$h up=no
>   done
> } | where 'up == "no"'
host      up
db3       no
```

**From** NGS's `collector { … collect(x) … }`, which wraps a block around a `collect` callback whose behaviour is chosen by the seed value, and YSH's `ctx push (dict) { ctx emit … }`, which keeps a stack of contexts a block writes into.

**Fits because** rows can be made today only by `oslo.register_tool` in Lua or one of the four built-in producers; this is the shell-side constructor for the same thing, and it makes `spread` usable on rows you built yourself.

**Effort** medium; scoping `emit` to the block, and stacking nested collectors so an inner `emit` cannot land in the outer stream.

## `rewrite`

**Filter a file back into itself without the `sort f > f` moment that empties it.**

The `<>;` operator is unclaimed, so oslo spells it for itself: the write goes through a temp inode on the same filesystem and is renamed over the original on exit 0, leaving the file untouched on any nonzero exit or signal. The same machinery becomes a net at the prompt — when oslo sees a redirection target that is also an input of the same command, it reroutes and says so once, instead of watching the file go to zero bytes. `undo` gets a journal entry either way, and the failure path is a caret under the redirection naming the file and the fd. The silent rerouting is confined to interactive use, because changing what `> f` does in a script would change what that script means; the operator is the scriptable half.

```console
$ sort -u hosts.txt <>; hosts.txt
$ sort -u hosts.txt > hosts.txt
oslo: hosts.txt is both an input and the target of >
      writing through a temp file and renaming on success
```

**From** ksh93, whose `<>;word` opens a file for reading and writing and truncates it to the offset reached only if the command succeeded.

**Fits because** `rm` safety ships and `undo` already treats a destructive builtin as something to journal; truncation on `>` is the one destructive act the shell itself performs and the only one with no net.

**Effort** medium; deciding when a target is "the same file" as an input across symlinks, bind mounts and relative paths, without guessing.

## `feed`

**A long-running pipeline publishes a named value that every other oslo session, and your prompt, can read.**

`... | feed build.progress` is a pass-through stage that publishes its most recent row under a name on the control socket that already exists; `feed build.progress` with no input reads the latest value back as a row, and `oslo.feed("build.progress")` reads it from Lua. The second reader is the point: an async prompt segment can paint the last row of a build running in another terminal, and a `notify` rule can fire when a feed changes, without either knowing which process produces it. Values are last-write-wins with a timestamp and a producer pid, never a queue, because a queue makes the reader responsible for draining and a prompt segment cannot be. `feed --list` emits `name`, `updated`, `pid` and `value`, so a stale feed left by a dead job is visible rather than mysterious.

```console
$ make build 2>&1 | lines | parse '{pct}% {step}' | feed build.progress | to text
```
```lua
oslo.prompt.segment("build", function()
  local r = oslo.feed("build.progress")
  return r and (r.pct .. "% " .. r.step) or nil
end)
```

**From** dgsh, which names stored values with `dgsh-writeval` and `dgsh-readval` over Unix-domain sockets so a value produced inside a running process graph can be read later by another process.

**Fits because** the control socket and the async and animated prompt segments all ship; what is missing is a place for a running pipeline to put a value those segments can read, and `scratch` already makes the cross-session case ordinary.

**Effort** medium; keeping publication off the hot path of a fast stream, so a feed never becomes backpressure on the pipeline carrying it.

---


# Where to start

Forty is a menu, not a plan. These are the cuts that matter when choosing from it.

**Afternoons.** `park`, `notify`, `bang`, `trap check` and `grab`. Each is one contained change on
machinery that already exists, and each is the kind of thing that is missed daily once it is there.

**A day or two, and almost entirely presentation.** `doc`, `branch history`, `norms`, `unsaved`,
`origin`, `why`. These read data oslo already parses or already writes and simply put it somewhere a
person can see. `doc` is the clearest case: every description it shows is already loaded to draw one
eight-character column in the completion dropdown.

**Best value per line of new code.** `plan`, `trace`, `heat`, `try`, `stream`. All five take
internals that exist and turn them into something you can look at or query. `trace`'s emitter is
already written.

**The ones nobody else has.** `norms` compares a run to its own history. `mute` and `solo` come off
a mixing desk. `stuck` answers the question every hung pipeline raises and no shell has ever
answered. `unsaved` is one RPC and a caret. `heat` exists only because the AST is lossless. `feed`
is a shape of pipeline — a graph rather than a line — that died with dgsh and nothing replaced.

**The bold ones.** `out` and `detach` both live or die on the pty and are the two most likely to eat
a week. `glob qualifiers` is the biggest fight and the biggest prize. `feed` and `sink` change what
a redirection can mean, which is the most load-bearing syntax in the language.

**Natural pairs, where one halves the cost of the next.**

* `out` → `jump`, because `jump` reads the buffer `out` keeps.
* `plan` → `check`, because `plan` builds most of its plumbing.
* `try` → `why`, because both stand on per-process status the executor does not collect yet.
* `decode` → `derive` → `bytes`, which together finish the structured pipeline: how rows get in,
  how you add a column, and how they survive being written to a file.
* `undo` and `keep` are the same instinct — one pointed at oslo's own builtins, one at other
  people's programs. Either is useful alone.

**The theme, if you want one.** Round three keeps arriving at the same place from different
shells: oslo's structured pipeline has a strong middle and weak ends. `decode` fixes the entrance,
`bytes` and `sink` fix the exit, `derive` makes the rows extensible, `stream` makes them arrive
sooner. Building those five as one project would be more coherent than picking five favourites off
the list.

---

# Considered and rejected

## From the first round

* **check** — shellcheck's job against oslo's own tree, value-aware at the prompt. Scored highest
  of the twenty and was still cut: it overlaps `plan` on the danger warnings and `explain` on the
  per-word rendering, and at large effort it should not be the third thing in that neighbourhood.
  Worth revisiting after `plan`, which builds most of its plumbing.
* **bench** — a benchmark runner. `norms` gets comparable numbers out of history for nothing.
* **shape** — static column checking for pipelines. `plan` with a smaller blast radius.
* **ren** — safe bulk rename. Sits inside `undo`'s territory, and `undo` covers more of what people
  actually lose.
* **peek**, **keys read**, **examples**, **startup** — all real, all small. `park` was the better
  afternoon because it is the only one of the four that oslo's other tools can call.
* **key modes**, **abbr suggest** — each a second layer on a surface already well served.

## From the second round

* **repo** — git's porcelain as typed rows (`repo status`, `repo log`, `repo branches`) feeding
  `where`/`group-by`/`stats`. It ranked second overall and is the most obviously *correct* idea
  here. It was cut for scope, not for quality: it is a whole subsystem shaped like a second tool
  registry, and `branch history` gets a large part of the daily value for a fraction of the work.
  This is the first thing to promote if a bigger project is wanted.
* **worktrees** — a worktree per branch, each with its own scratch and `.env.lua`, one key to hop
  between them. Excellent and squarely in oslo's world; overlaps `scratch` and `detach` enough that
  it should follow them rather than precede them.
* **cell** — `# %%` cells in a plain shell script, one keypress from the editor to run one. The
  best of the notebook ideas, but it needs an editor plugin to be worth anything, and a feature
  that only works with a companion plugin is a different kind of commitment.
* **inside** — walk into a repo and the shell is already in its container. Genuinely good, and
  genuinely a container runtime problem wearing a shell's clothes.
* **rehearse** — run for real but hold the writes until you say keep or scrap. The most exciting
  idea in either round and the least buildable: it needs an overlay filesystem or a syscall
  interposer, and anything less is a promise the shell cannot keep.
* **checklist**, **crew**, **drift**, **lsp**, **tour**, **habits** — each solves a real problem
  and each is a product rather than a feature. Revisit individually if one of them turns out to be
  the thing you actually want oslo to be.

## From the third round

Mined from the unusual shells and cut for scope, not for quality. Several of these are better than
things that made the list; they lost on buildability or on overlapping a choice already made.

* **result** (es) — `<={ ... }` captures what a command *returned* rather than what it printed, so
  rows stay rows and numbers stay numbers with no fork. The purest idea in the round. It needs a
  return channel the executor does not have, and `try` gets the nearest useful part of it.
* **sample** (nix repl) — after a pipe, complete column names from the rows that actually flow
  there rather than from a spec somebody wrote. Obviously right, and it needs the pipeline to be
  speculatively runnable at completion time, which is a different feature wearing this one's face.
* **knob** (Mathematica) — put a slider on a command; move it and the rows redraw underneath. The
  wildest thing proposed in any round. Cut because it wants `explore` to become a live document.
* **record** (expect's `autoexpect`) — do the fiddly interactive thing once by hand and oslo writes
  the script that repeats it. Strong and self-contained; it lost only to the twenty above it.
* **rewind** (Ammonite) — mark the shell's state, mess about, put it back in one word. Lovely, and
  the boundary of "state" is a research project: variables and cwd are easy, the filesystem is not.
* **stage** (scsh) and **chan** (Oh) — a Lua block as a real forked pipeline stage with its own
  fds, and a pipe you can hold in a variable and hand to two commands. Both are the same bet `feed`
  makes, and `feed` makes it more visibly.
* **syntax** (Oh) — a Lua builtin that receives the lossless AST of its arguments instead of an
  expanded argv. The most powerful hook in the round and the easiest to shoot yourself with.
* **spoof** (es) and **setter** (ksh93's discipline functions, second angle) — interpose Lua on
  `open`, on `$PATH` lookup, or on assignment, with the real one still callable underneath. Real
  power, and an invitation to make a shell nobody else can debug.
* **under** (Rash) — inside this block or directory, every bare line runs through a wrapper you
  chose. One line from being `docker exec` for everything you type, and one line from being a
  footgun that changes what a script means.
* **wire** (rc) and **extract** (es) — pipe an arbitrary fd into the next stage and see the
  descriptor table first; turn a glob's `*`s into columns on the rows. Both good, both narrower
  than what got in.
* **review** (Eshell) — when a command floods the screen, park at the top of its output rather than
  the bottom, and leave the moment you type. Small, pleasant, and a pager question more than a
  shell one.
