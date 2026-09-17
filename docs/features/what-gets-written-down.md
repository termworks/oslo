# What gets written down

oslo keeps its own record of what you do: a **log**, one row per line in the order you typed them,
and an **aggregate**, folded by directory and command line. The log knows the order executions
happened in, the aggregate how often and how recently, and folding throws the first away — which is
why there are two.

<!-- demo:begin -->
[![what-gets-written-down demo](https://asciinema.org/a/1262754.svg)](https://asciinema.org/a/1262754)
<!-- demo:end -->

## How it works

A command's exit status, its duration and the directory it ran in do not exist when the line is
typed. So one line is written twice, and the two rows are joined by the id the first went in under.

```
Enter
 ├─ remember()                the editor's copy and $HISTFILE — plain text, one line
 │                            skipped by a leading space, and by oslo.history.ignore
 ├─ Track::append(line, mode) Tree::History, row id N — BEFORE the command runs
 │    │                       session and seq are filled in here, not by the caller
 │    └─ sync::append_local   a 32-byte event id, revision 1, no completion yet
 │
 ├─ ─ ─ ─ ─  the command runs, for as long as it likes  ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─
 └─ the command boundary, where the status finally exists
      ├─ pre-record rule       as typed / these lines instead / nothing at all
      ├─ Track::record(Step)   Tree::Dir + Tree::Run: where it ran, dwell, counters
      ├─ record_outcome(N, …)  Tree::Outcome: segment 0 is the line,
      │                                       segments 1.. are the links of a chain
      └─ sync::complete_local  the same event, revision 2, completion attached
```

Writing the line first is the decision everything else follows from: a long command is in another
terminal's history *while it is still running*, and a command that exits the shell or has to be
killed is in the history afterwards. The cost is that a log row with no outcome is an ordinary
state — still running, the shell died, the line never reached execution — not a failure.

### The log row

| field | |
|---|---|
| `line` | as typed, byte for byte |
| `mode` | `sh` or `lua`, so recalling a Lua line at a shell prompt does not run it as shell |
| `session` | which shell typed it: a small ordinal from `Tree::Meta`, **not the pid** — four bytes rather than the eighteen a `pid-starttime` string costs per row, and the only question anything asks of it is "same shell or not" |
| `seq` | position within that shell's run, from one |
| `rewritten` | whether what is stored is what was typed |

The fields are framed rather than separated, so `for f in *.rs; do … done` comes back as the one
entry it was typed as, newlines and all, and a NUL in a line survives too — `$HISTFILE` can hold
neither. The row is **appended to, never reordered**, so a value written by an older oslo reads back
with the new fields at their defaults. The key is `u64::MAX - id`: the newest line is the first row.

### One row per link, including the links that never ran

`a && b || c` is one line and several links. The shell used to keep the last status and nothing else;
the outcome bucket keeps each of them, and the distinction it exists for is the skipped link.

```
  make clean && make build && make test        ← one log row, id N

  Tree::Outcome, keyed (u64::MAX - N, segment)
   seg  join   text            status   ms    dir
    0   ""     ""                 2     417    7    the line: where, what, how long
    1   ""     "make clean"       0       5    –
    2   "&&"   "make build"       2     412    –
    3   "&&"   "make test"        –       0    –    never ran: not 0, not a failure
```

`None` is stored as `-1`, because the encoding carries integers rather than options and nothing
exits `-1` on Linux, where the wait status is eight bits and a signal death becomes `128 + n`.

Links are written only when there is more than one, and only for the outermost chain of the line
you typed: the and-or lists inside `if a && b; then …` are not links of it, and a script or an `-c`
command never arms the buffer at all. The stages *inside* one pipeline are not stored — they run at
the same time, so a per-stage wall clock would print one number three times. **The chain's shape is
not stored either, only its outcome:** the line is in the log and the parser is in the same binary,
so a reader re-derives the structure rather than keeping a second copy that can disagree with it.

### Where you were

The aggregate is the other half, and what the ghost suggestion, `cd` by name and the history finder
read. Two buckets hold rows: `Dir`, by integer id, with path, visits, last visit, dwell and worktree
root; `Run`, by `(dir_id, mode, argv)`, with runs, fails, last status, total and worst duration. A
repeat is an increment rather than a row, which is what makes a year of typing a few megabytes. The
index buckets beside them — `DirByPath`, `DirByBase`, `DirByRoot`, `RunByArgv` — hold an id or
nothing at all, because everything else they would hold is already in the key.

The `argv` stored is not always the line: a risky one keeps only its head, so `AWS_SECRET=… aws s3
cp …` is remembered as `aws s3`, the head having dropped the leading assignment and kept the known
subcommand, while the directory, the count and the timing survive. A directory that is `/tmp`
exactly, or has `.git` or `node_modules` as a path *component*, is recorded like any other — what
ran there is in the finder — but is never offered as a `cd` destination.
Dwell is shell-milliseconds rather than wall-clock — two shells in one directory for an hour record
two hours — and one command contributes fifteen minutes at most, so a laptop that slept for nine
hours does not credit nine hours to `~`.

### A secret line, and the gap it does not leave

A line beginning with whitespace, while `oslo.history.ignore_space` is on, is never appended: not to
the editor's copy, not to `$HISTFILE`, not to the log, and so not to the outcome bucket or the
aggregate. The boundary is thrown away too, so the directory it ran in is not recorded and the
seconds it took are credited to nothing — half-honouring the one privacy mechanism a user operates
deliberately would be worse than not offering it. The predictor never sees it either, which is a
property of the log rather than a rule repeated anywhere. (`oslo.history.ignore` is the weaker
filter: `$HISTIGNORE` glob patterns matched against the whole line, keeping it out of the editor's
copy and out of `$HISTFILE` — **the store's log row is still written**.)

`seq` is not redundant with the row's id: the id is dense — one above the newest row there is — so
nothing in it tells a reader that a line is missing. The predictor files each observation under a
stream (`session`) and a position (`seq`), so a gap in `seq` breaks adjacency instead of forging it.

A number is consumed when a row is appended and stays consumed if the row is taken away later — a
`pre-record` refusal, a `forget`, a trim — and that is where a gap comes from. **A secret line never
reaches the counter at all**, since it is never appended, so the rows either side of it are numbered
consecutively and the model reads them as consecutive commands: it learns a transition that never
happened. That is a known hole, written down in `predict/mod.rs` rather than papered over. Advancing
the counter for a line that is not written would make the omission visible; it is not done today.

Everything in that paragraph is about a build that *has* a model. `seq` is written either way — it
is a property of the log, not of the predictor — but in `oslo-minimal` nothing reads it, because the
`vista` feature is off and there is no model to mislead. See
[prediction-and-repair](prediction-and-repair.md).

### The log is a projection of events

Every write to the log also writes a portable event, and the local rows are derived from those
events rather than the other way round — which is what makes two databases mergeable.

```
Tree::SyncEvent        32-byte id → revision, deleted, tie_breaker, recorded_at,
                       seq, rewritten, line, mode, host, session, and a completion
        │ apply_event                {cwd, root, status, ms, segments} once it ends
        ▼
Tree::EventProjection  event id → what THIS database did with it: local id, the
                       stamp it applied, hidden, the one run-row contribution added
        ▼
Tree::History   Tree::Outcome   Tree::Run      the rows everything else reads
Tree::HistoryEvent     local id → event id, for the other direction
```

An event's stamp is `(revision, deleted, tie_breaker)` and the higher stamp wins, in both directions,
so `oslo history sync a b` needs no notion of which file is authoritative. A deletion is a tombstone
— `deleted` set and the revision advanced — so it propagates rather than being undone by the copy
that still has the line. Applying an event **removes the contribution its previous revision made to
the aggregate before adding the new one**, which keeps `runs` a count of executions rather than of
syncs. A row already trimmed here is marked `hidden`, so a later sync does not bring it back.

## What makes it different

bash and zsh write a newline-separated text file, so a multi-line command needs a convention to
survive it at all and whatever metadata the shell keeps has to be squeezed into the same lines:
zsh's `EXTENDED_HISTORY` prefixes each entry with a start time and an elapsed second count, and
there is nowhere to put the directory, the status or the links. oslo writes framed fields into a
key-value store, so the entry is the line and the columns are columns.

**Recording cannot be turned off.** `oslo.feature.set` refuses `history`, `tracking`, `track`,
`log`, `frecency` and `record` outright, and a test asserts that it does, because something
downstream is entitled to assume the log is complete rather than "complete except where a config
had an opinion". The controls that exist instead leave a record that is honestly shaped: redaction
keeps the command and drops the arguments, a profile separates two chronologies without truncating
either, and a line a rule rewrote carries a flag saying so.

## Configuration

```lua
oslo.history.ignore_space = true              -- a leading space means "do not remember"
oslo.history.ignore       = { "ls", "cd *" }  -- $HISTIGNORE, whole-line glob patterns
oslo.history.size         = 10000
oslo.history.file         = "~/.oslo_history"  -- off unless you set it; an export, never read back
```

`file` is for other programs — bash, zsh, anything that reads a `$HISTFILE`. oslo's own history is
the profile database, so leaving it unset costs nothing: the Up arrow, the finder, the `history`
builtin and Tab's ranking all come from there.

```sh
HISTFILE="" oslo                    # leave no trace at all: no file *and* no store; HISTSIZE=0 too
OSLO_PROFILE=claude oslo            # ~/.local/share/oslo/history/claude/, yours untouched
OSLO_ALLHIST=1 oslo -c 'echo hi'    # log an -c line too; off unless set
```

The environment wins over the config. A `pre-record` rule decides what is written for a line that has
finished; it is told `text`, `cwd`, `mode`, `status`, `duration_ms`, `profile` and `segments`:

```lua
oslo.on.pre_record(function(c)
  for _, s in ipairs(c.segments) do
    if s.text:match("^cc ") then
      return { c.text, s.text }        -- the chain, and that link as its own command
    end
  end
end)
```

Returning nothing records the line as typed. A list records those lines: the first becomes the log
row — rewritten **in place, keeping the id**, since the id is what the outcome rows join on — and all
of them reach the aggregate. `false` drops the log row and its outcomes entirely. An empty list is
not a refusal; a rule that matched nothing meant "no change".

`oslo history` operates on the file: `path`, `status`, `list`, `search`, `show`, `stats`, `verify`,
`sync A B`, `export`, `import`, `backup`, `delete`, `clear --yes`, `prune`. `clear` also deletes the
predictor's snapshot, a distillation of exactly what was just deleted.

## Measurements

One measurement, recorded in `crates/oslo-base/src/track/prune/mod.rs`, forced the bounds: the
engine allocates in one 8 MiB step, 400 rows fit in the 128 KiB a fresh store is born with, and
somewhere between 500 and 1,000 rows the file jumps to 8.5 MiB and stays there. There is no
`VACUUM`, so the caps are the difference between those two numbers for the life of the machine.

| bound | value |
|---|---|
| lines kept in the log | `$HISTSIZE`, default 10,000 |
| appends between trims | 100 — a trim takes the file's write lock, and one per line is one lock every other terminal's next keystroke queues behind |
| lines kept per directory | 500 |
| a `runs = 1` row untouched for | 90 days |
| a directory that has stopped existing | 30 days, so an unmounted disk is a pause rather than a deletion |
| the file and its directory | `0600` and `0700` from the moment they exist, asserted by a test |

## What it cannot do

- **Be read back by the shell itself, yet.** Nothing in a running shell reads the outcome rows:
  `chain` reports from a thread-local buffer, the finder and the ghost read the aggregate, and
  `oslo history` reads the events. `Track::observations` exists for a replay that has no caller.
- **Record output.** The line, the language, where, when, how long, what it exited with, what each
  link did. Nothing a command printed is written down.
- **Attach an outcome to a `-c` command.** Under `$OSLO_ALLHIST` such a line gets a log row and an
  event, but there is no command boundary to record a status, a directory or a duration at.
- **Put a line that did not parse into the aggregate.** It is in the log, because you typed it, and
  it gets an outcome row with no status — but it is not a command, so no `run` row is written. A
  typo is often a password typed into the wrong prompt, and a table built to suggest lines back to
  you is the last place one should come to rest.
- **Sync by itself.** `host` is on every event and every run row so that a shared history is a
  filter that already works, but merging is `oslo history sync` run by hand: no daemon, no server.
- **Remember for ever.** Log and outcomes are trimmed by one threshold, outcomes first: once the
  line is gone nothing is left to say which rows belonged to it.

## Where it lives

| path | what |
|---|---|
| `crates/oslo-base/src/track/log.rs` | `Entry`, `append_entry`, `rewrite_line`, `drop_line`, `trim` |
| `crates/oslo-base/src/track/outcome.rs` | `Outcome`, `Observation`, `record_outcome`, the segment keys |
| `crates/oslo-base/src/track/row.rs` | `DirRow`, `RunRow`, and every key and span that reaches one |
| `crates/oslo-base/src/track/write.rs` | `Track::record` — one transaction per prompt |
| `crates/oslo-base/src/track/redact.rs` | `prepare`, `head_of`, `is_risky`, `is_excluded` |
| `crates/oslo-base/src/track/sync.rs` | `HistoryEvent`, `Projection`, the stamp and its ordering |
| `crates/oslo-base/src/track/sync/projection.rs` | `append_local`, `complete_local`, `apply_event` |
| `crates/oslo-runtime/src/startup/tracking.rs` | `Tracker`, `ask_what_to_record`, `record_outcome` |
| `crates/oslo-shell/src/exec/pipeline/segments.rs` | `Segment`, `Join`, and `ran` |
| `src/cli/history.rs`, `src/cli/history/admin.rs` | `oslo history` |
