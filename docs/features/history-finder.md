# The history finder

Up opens a full-screen list of every command the shell has run, newest first, with a search bar
along the bottom and whatever was already on the line typed into it. It exists because the two
usual ways of getting an old command back — walking the list one entry at a time, or an incremental
search that shows you one match at a time — both require you to already know what you are looking
for.

<!-- demo:begin -->
[![history-finder demo](https://asciinema.org/a/1262738.svg)](https://asciinema.org/a/1262738)
<!-- demo:end -->

## How it works

Up is handled by `history_prev`, which opens the finder only on the *first* press of a fresh line:
after that it is an ordinary walk. Several things can decline before anything is drawn, and all of
them mean "carry on as though the key had not been pressed" rather than "the user cancelled" — a
cancelled finder must leave the line exactly as it was, whereas one that could not open has to fall
through to the walk Up meant before it existed.

```
Up, on a fresh line, with "car" in the buffer
  │
  ├─ oslo.finder.enabled == false ──────────────┐
  ├─ oslo.feature.set("finder", false) ─────────┤
  ├─ no tracking store (HISTFILE="") ───────────┼──→ walk history one line at a time
  ├─ nothing recorded in this language ─────────┘
  ▼
Track::commands(oslo.finder.limit)
  │   one scan of the Run tree, folded by (line, mode):
  │   line, mode, runs, last_at, dir, places, worked, session, host, root
  ├── keep only mode == the language the prompt is in
  ▼
finder::open(commands, cwd, now, fuzzy, seed = "car")
  │   alternate screen + raw mode, both undone on every path out
  │
  └─ loop ─→ rank(commands, query, cwd, fuzzy)   fold the pattern once for the batch
               │
               └─→ retain(in_scope)  ─→  frame()  ─→  write only if it changed
  ▼
Outcome::Chosen { line, mode }  →  the line goes back on the prompt, unrun
Outcome::Cancelled              →  the prompt comes back untouched
```

**The store is read once, when the finder opens, and never again while you type.** Everything after
that is a filter over a `Vec<Command>` held in memory. Going back to the database per keystroke
would put a transaction — and, on a cold page cache, a disk read — between a character and its
frame.

The list is drawn bottom-up: index zero sits nearest the search bar, so the best match is closest
to where your eyes and the cursor already are. Up therefore advances *through* the vector and Down
comes back toward zero, which is the reverse of the numeric direction and is what
`arrow_navigation_follows_the_screen` in `finder/run/tests.rs` pins down.

### The scopes

The arrows do not move a cursor, because the search box only ever appends — there is nothing to
move over. They move the scope instead, along a line from widest to narrowest, wrapping at both
ends. Every scope filters on a fact the store wrote down at the time the command ran; none of it is
re-derived, because which shell ran a line and which machine it ran on cannot be recovered from the
line afterwards.

```
        ←── Left widens                              Right narrows ──→
  ┌──────────┬────────┬───────────┬─────────────┬─────────────┐
  │  global  │  host  │  session  │  directory  │  workspace  │
  └──────────┴────────┴───────────┴─────────────┴─────────────┘
       ▲                                                 │
       └───────────── wraps, in both directions ─────────┘

  global      everything the store knows
  host        row.host is empty or == this machine's short name
  session     row.session == this shell's "pid-starttime"
  directory   row.dir == $PWD, exactly
  workspace   row.root == the git worktree the shell is standing in
```

**Inside a git worktree it opens in `workspace`**, so Up shows what was run in this project rather
than everything ever run. Outside one, or in a worktree nothing has been run in yet, it opens in
`global` — an empty list would read as lost history. `oslo.finder.scope` picks a fixed scope
instead; any scope with nothing in it still falls back to `global`.

`host` is **identical to `global` today** and is deliberately still its own scope: the store is
local, so every row in it was run here. It becomes a real filter the moment history is shared
between machines, and having the name already means that change is a filter rather than a new
concept to learn.

The bar's right end says what you are looking at: `default @ [global] || 12/840` — the profile, the
scope badge, then how many rows matched out of how many the *scope* could show. The denominator is
counted over the scope rather than over the whole store, because `12/840` is only meaningful if the
larger number is the pool the query narrowed.

Tab is a different axis: it moves to the next profile, which is a different store and so a
different history entirely. The query and the scope survive the switch, because you are asking the
same question of a different history.

### Marking, with Ctrl-Space

**Ctrl-Space marks the row under the cursor and steps to the next one.** Marking a scattered handful
and pressing Delete once is the case it exists for, and stopping after each mark would have meant two
keystrokes a row. "Next" is upward, because the list grows up from the search bar. Stepping back onto
a marked row unmarks it.

A mark is kept as the command it is — the line and the mode, the same pair `forget` takes — and never
as a position. Every keystroke re-ranks and re-filters the list, so a mark held by index would move to
whatever row happened to take that slot: you would mark `rm -rf build`, type three letters, and delete
something else. Marks therefore survive typing, a scope change and a profile change, and you can mark,
narrow the query, mark more, and delete the lot.

The checkbox column appears only once something is marked. Two cells against every row of a list
nobody is marking is noise on every line, and the single shift when the first mark lands is cheaper
than carrying it always.

### Delete

Delete forgets **everything marked**, or the highlighted command when nothing is — the same rule
`ui choose` uses for Enter, so neither way of working has to be announced. Forgetting is total: every
run of that line, in every directory, out of the event log as well as the aggregate.

Unless `oslo.finder.confirm_delete` is off it asks first, with *no* selected, so a stray Enter answers
the safe way. The question counts what is about to go — `delete 4 from history?` — because one row is
what the eye is already on and four is a claim worth checking.

While the question is up it owns the keyboard: Left, Right and Tab flip the answer, Enter commits it,
and Esc answers *no* rather than closing the finder, because changing your mind about a deletion is
not the same as wanting to leave. **Delete again means yes** — the key that asked is the one already
under the finger, and pressing it twice is how somebody who knows what they are doing gets past the
guard without reaching for Enter. It answers regardless of which button is highlighted, because the
second press *is* the answer.

The question takes exactly the three rows the search bar already owns, so the list does not shift
while you decide and the row you are about to delete stays under your eye.

`Track::forget` deletes from the `Run` tree and its `RunByArgv` twin, and then from the `History`
tree, writing a deletion event for each removed line. **The second half was a bug fix**: forget
used to touch the aggregate alone, so the line stayed in the log and came back through `recent()`
on the next start — a password typed on a command line included. "Forget this" is a statement about
the line, not about one index of it.

The selection stays where it was afterwards, so a run of unwanted lines can be cleared without
moving the cursor back each time. Typing, by contrast, homes the selection to the top: a query
change makes the old index meaningless, whereas a deletion leaves the rows around it the same.

### The keys

| Key | In the finder |
| --- | --- |
| Up / Down, Ctrl-P / Ctrl-N | move the selection |
| PageUp / PageDown | move by one window |
| Left / Right, Ctrl-B / Ctrl-F | widen / narrow the scope |
| Tab | the next profile |
| Ctrl-Space | mark this row and step to the next |
| Delete, Ctrl-D | forget everything marked, or the highlighted row |
| Delete again | while the question is up, answer *yes* |
| Backspace | one character off the query |
| Ctrl-U | clear the query |
| Enter | put the line on the prompt, unrun |
| Esc, Ctrl-C | leave, prompt untouched |

## What makes it different

The data is the difference. bash and zsh keep history as a flat file of lines in the order they
were typed, and their search reads that list; oslo keeps a key-value store whose `Run` tree already
records, per `(directory, mode, argv)`, how many times a line has run, when it last ran, whether it
worked, which shell and machine ran it and which git worktree it was in. The finder is a *reader* of
that — nothing is recorded for its benefit — which is why "commands I ran in this directory" and
"commands this shell ran" are one keypress rather than a feature that would need a new file format.

bash's Ctrl-R is an incremental reverse search: one match at a time, in reverse order, with no view
of the alternatives. This is a list first and a search second, which is why the key is Up — Ctrl-R
is muscle memory pointing at a search. Both work; Ctrl-R reaches the same finder through the emacs
keymap.

For deleting, bash offers `history -d offset`, which removes the entry at a position in the current
in-memory list. Delete here is a statement about a *command line*: every copy of it, under every
directory, in both trees, recorded as a deletion so a later sync does not resurrect it.

The bottom-up layout is not oslo's invention and the source says so: fzf and atuin both settled on
results growing upward from the input, because the input is where the cursor is.

## Configuration

```lua
oslo.finder.enabled        = true      -- off means Up walks history a line at a time
oslo.finder.key            = "up"
oslo.finder.limit          = 10000     -- distinct commands loaded when it opens
oslo.finder.confirm_delete = true      -- Delete asks before forgetting a command
oslo.finder.scope          = "auto"    -- auto / global / host / session / directory / workspace
oslo.completion.fuzzy      = "smart"   -- off / tight / smart / loose; shared with Tab
```

Matching is shared with the completion dropdown on purpose: "how loosely should matching work" is
one preference, and two settings would only ever be a way to set them inconsistently.

Any key can open it, by name, through the ordinary binding table:

```lua
oslo.keys["ctrl-g"] = "history-search"
```

It can be switched off for a while without touching the configuration, which is what a directory
hook wants:

```lua
oslo.feature.set("finder", false)
```

Three hooks bracket a search. They are fired by the caller rather than from inside the finder,
because everything above the `open` call can decline, and a hook that fired for a search which
never appeared would be lying. Every field arrives as a string.

```lua
oslo.on.history_open(function(h)   end)  -- { seed }
oslo.on.history_select(function(h) end)  -- { line }
oslo.on.history_close(function(h)  end)  -- { chosen = "true" | "false" }
```

A profile is chosen with the environment and nothing else:

```sh
OSLO_PROFILE=claude oslo        # ~/.local/share/oslo/history/claude/
```

## Measurements

`cargo bench --bench fuzzy`, run on this branch: a three-character pattern at `smart` scored
against 3300 candidates takes **229.9 µs** when the pattern is folded once for the batch, against
328.5 µs when it is folded per candidate — 99 µs, or 30%, saved.

That is the shape `rank` uses on every keystroke, and the matching pass only: the finder then sorts
the survivors and clones them. `bench/fuzzy.rs` calls `Fuzzed::score`, where the finder calls
`Fuzzed::rank`, which also returns the match quality.

## What it cannot do

- **It cannot see a command run after it opened.** The rows are read once, so a line another shell
  ran while you are searching appears on the next opening, not this one.
- Only `oslo.finder.limit` distinct lines are kept, and the truncation happens *after* the
  newest-first sort — so beyond the limit the oldest commands are unreachable from the finder even
  when they match.
- Only the current language's commands. A Lua line at a shell prompt would produce something that
  cannot run, so the list is filtered to `sh` or `lua` before the finder ever sees it.
- The query matches the command text and nothing else. Directory, host, session and worktree are
  scopes, not query syntax; there is no way to type a filter for them.
- `workspace` shows nothing at all when the shell is not inside a git worktree — both the row and
  the shell must have a root for a row to be in this one.
- `host` filters nothing today. See above.
- The search box has no cursor: characters, Backspace and Ctrl-U, which is the price of the arrows
  meaning scope.
- **Enter is never plural.** Marks are for Delete; Enter puts one line on the prompt, because a
  prompt holds one line. Marking and pressing Enter takes the row under the cursor.
- Marks do not survive leaving the finder. Esc drops them, as it drops everything else.
- Nothing is remembered between openings: it always opens on `global`, in the current profile.
- `oslo.finder.key` is validated as a key name but is only ever compared against `"up"`. Setting it
  to anything else does not bind that key — it turns off the Up behaviour, and the binding table
  above is how you bind another one.
- Delete has no undo. The rows are gone from the store and the only way back is to run the command
  again — which is why the question counts them, and why *no* is what Enter answers.
- The finder never runs anything. Enter puts the line on the prompt for you to edit or accept,
  which is the contract every other recall in the shell has.

## Where it lives

| Path | What is in it |
| --- | --- |
| `crates/oslo-ui/src/finder/mod.rs` | `Scope`, `Scope::next`, `Scope::previous`, `Scope::label` |
| `crates/oslo-ui/src/finder/run.rs` | `State::toggle_mark`, `State::doomed`, `State::forget_doomed` |
| `crates/oslo-ui/src/finder/run.rs` | `open`, `Outcome`, `State`, `State::in_scope`, `State::forget_selected`, `State::next_profile` |
| `crates/oslo-ui/src/finder/rank.rs` | `rank`, `Ranked`, `ago`, `is_here` |
| `crates/oslo-ui/src/finder/render.rs` | `frame`, `visible_rows`, `confirm_row` |
| `crates/oslo-ui/src/ask/look.rs` | `Preset::History` — the rows, the striping and the bar |
| `crates/oslo-base/src/track/history.rs` | `Command`, `Track::commands`, `Track::forget` |
| `crates/oslo-base/src/track/session.rs` | `id`, `host` — what the session and host scopes compare against |
| `crates/oslo-base/src/track/profile.rs` | `current`, `after`, `available`, `store_path` |
| `crates/oslo-runtime/src/startup/native.rs` | `open_finder`, and Up in `history_prev` |
| `crates/oslo-ui/src/settings/mod.rs` | `Finder` and its defaults |
