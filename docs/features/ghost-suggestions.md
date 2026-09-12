# Ghost suggestions

The grey text drawn past the cursor: what the rest of the line would be if you kept going. It exists
so that the commands you already run are one keystroke away instead of a search, and it is built out
of five sources you order yourself rather than one hard-wired guess.

<!-- demo:begin -->
[![ghost-suggestions demo](https://asciinema.org/a/1262737.svg)](https://asciinema.org/a/1262737)
<!-- demo:end -->

## How it works

`OsloHelper::suggest` is asked on every keystroke. It answers with a **tail** — the characters that
would follow what has been typed — and never with a whole line, because the editor draws the answer
as text appended after the cursor. That single fact is the constraint the whole feature is shaped
around: a suggestion that *replaced* your line could not be drawn where it is drawn without lying
about what pressing Right will do.

```
  keystroke ─► suggest(line, pos)
                 │
                 ├─ line empty, or cursor not at the end ─────────────────► nothing
                 ├─ feature "suggest" masked off at runtime ─────────────► nothing
                 │
                 ▼  for each source in oslo.suggest.sh_sources, in the written order:
   ┌───────────────────────────────────────────────────────────────────────────┐
   │ history     recall::suggest — the language the prompt is reading NOW      │
   │             skipped when the first word is in oslo.suggest.skip_history   │
   │               1. lines run in THIS directory        (store)               │
   │               2. lines run anywhere in THIS worktree (store, memoised)    │
   │               3. anything remembered, newest first   (flat walk)          │
   ├───────────────────────────────────────────────────────────────────────────┤
   │ completion  command_hint — shell prompts only, command position only      │
   │             builtins, aliases, functions, then $PATH; most-used first     │
   ├───────────────────────────────────────────────────────────────────────────┤
   │ path        path_hint — argument words only, shortest entry wins          │
   ├───────────────────────────────────────────────────────────────────────────┤
   │ predict     the vista model; answers a whole line, kept only if that      │
   │             line starts with what you typed                               │
   │             ONLY in a build with the `vista` feature — `oslo`, not        │
   │             `oslo-minimal`. Elsewhere this row answers nothing.           │
   ├───────────────────────────────────────────────────────────────────────────┤
   │ provider    whatever oslo.suggest.provider registered, in registration    │
   │             order; each answers a whole line, kept only if it continues   │
   │             what you typed                                                │
   └───────────────────────────────────────────────────────────────────────────┘
                 │
                 │  the first source with an answer wins outright — no merging,
                 │  no second ranking pass across sources
                 ▼
        tail = candidate[typed.len()..]  ──►  theme.syntax.autosuggestion
```

**The order is the configuration.** There is no weighting and no scoring between sources; a source
either answers or it does not, and the next one is asked only when it did not. The default order is
history, completion, path — taken from fish, for the reason recorded in `settings::Suggest::default`:
a line you have actually run is a better guess than anything that can be ranked.

### History, and where you are standing

The history source is not the editor's flat history. `recall` keeps every remembered line with the
language it was typed in and answers for the language the prompt is showing *now*, so a Lua line is
never offered at a shell prompt and vice versa. The language can change mid-line from a key handler
that cannot reach the editor at all, which is why this lives in the library rather than in an editor
history hinter.

Above the flat set sits the tracking store, which keys a line by the directory it ran in. It is
asked twice: the exact directory first, then the whole worktree, and only then does the flat walk
run. `cargo run --ex` therefore answers with *this* project's example rather than whichever project
you last typed it in. The store also only offers a line that worked — a row is a candidate when its
last status was zero, or when it has run at least once without failing (`runs > fails`) — so a typo
cannot be suggested back for ever the way the flat walk would suggest it.

The widened worktree query is the expensive half and it is memoised, because a prefix search over
every directory at once costs milliseconds at the first keystroke. The memo is sound on two facts
about prefix searches: if nothing starts with a shorter prefix, nothing starts with a longer one;
and an answer still ahead of the field after more typing is still the answer.

### skip_history

`rm z` used to suggest `rm zzz-old-notes` out of history — a path that does not exist any more,
*because the suggested command deleted it*. Accepting a ghost is one keystroke, and for `rm` that is
one keystroke aiming a destructive command at whatever the name happens to match now. So
`oslo.suggest.skip_history` names commands whose past arguments are worthless to offer back; `rm` is
the only one by default. The judgement is on the first word alone, by the name as run, so `/bin/rm`
and `rm` are the same command to somebody who typed one and meant the other. **Only the history
source is skipped** — the filesystem still completes the argument, which is the answer that was
wanted in the first place.

### The other three

`completion` answers for a command name being typed. It refuses a quoted word, refuses anything that
is not in command position, and refuses a stem that already names a real command — typing `exit`
suggests nothing, because `exit` is not a prefix of the answer, it *is* the answer. It is also
**shell-only**: everything it can offer is a builtin, an alias, a function or a program on `$PATH`,
and none of those can run at a Lua prompt. Candidates are ordered by frecency, then shell-provided
over external, then shortest, then alphabetically.

`path` answers for the argument rather than the command, reading the directory the typed word names.
A dotfile is offered only once you have typed the dot, the same rule globbing follows, and the
shortest matching entry wins as the least presumptuous answer.

`predict` is the vista model, which knows what usually follows what you have been doing. It is the
only source that can offer a line you have never typed here before, which is its value and its risk,
and it is **not in the default order** until it has been measured against the history source on a
real corpus. Because vista matches on containment rather than prefix, `suggest` keeps only the
guesses that genuinely start with the typed line.

**And it is the only source that is not in every build.** `predict` needs the `vista` cargo
feature, which the published `oslo` has and `oslo-minimal` does not; see
[prediction-and-repair](prediction-and-repair.md). The other three read what is already on the
machine — your history, the completion specs, the filesystem — so they are always there.

The name still *parses* in a build without the model, answering nothing rather than refusing the
config. That is deliberate: a config is shared between machines, and a source that cannot answer is
skipped exactly like one that had nothing to say. The practical consequence is that the line below
is a safe thing to write on a machine you have not checked:

```lua
oslo.suggest.sh_sources = { "predict", "history", "path" }
```

Under `oslo` the model answers first; under `oslo-minimal` it is silently skipped and history
answers. Nothing errors either way.

### Drawing and accepting

The tail is painted in `theme.syntax.autosuggestion` — an explicit grey, index 240 dark and 250
light, rather than the bright-black slot, because a ghost has to sit a measured distance behind the
text you are typing. It is drawn on the same row and counted in the block height, and it is
suppressed on the final frame of a line: a ghost left on screen when you press Enter puts a command
in the scrollback that was never run.

Right at the end of the line takes the whole suggestion; elsewhere it moves the cursor, and in vi's
normal mode it stays a motion so `d<Right>` still deletes a character. A correction from the repair
path is drawn in the same place and never at the same time, so one key accepts whichever is showing.
The suggestion is asked for again at the moment of acceptance rather than remembered from the last
frame, since a remembered one can be stale by exactly the keystroke that accepted it.

## What makes it different

The ghost is part of the shell rather than a layer over the line editor: five sources, ordered by
`oslo.suggest.sh_sources`, first-answer-wins, with no plugin to install and nothing to enable.

The per-language split follows from oslo reading two languages at one prompt. It is the reason the
suggestion cannot be delegated to the line editor's own history: the editor holds one history, and
a shell line and a Lua line are not alternatives for the same slot.

Directory-aware history is oslo's answer to a problem a flat history cannot solve: the same prefix
means different things in different projects, and a flat history only knows which one was typed last.

`skip_history` is oslo's own; it exists because the `rm` case was a real bug and not a hypothetical.

## Configuration

**Two lists, one per language.** `sh_sources` is the shell prompt's and `lua_sources` is the Lua
prompt's, because most of the sources answer with shell: a history of shell lines suggested at a Lua
prompt is never right, and a Lua prompt wants completion — the names in scope — rather than what you
typed yesterday. There is no combined `sources`; a single list could only ever be right for one of
them.

```lua
oslo.suggest.sh_sources  = { "history", "completion", "path" }  -- the shell prompt's default order
oslo.suggest.lua_sources = { "completion" }                     -- the Lua prompt's default

oslo.suggest.sh_sources = { "predict", "history", "path" }      -- ask the model first; `oslo` only
oslo.suggest.sh_sources = { "provider", "history" }             -- a plugin first, then your history
oslo.suggest.sh_sources = {}                                    -- no suggestions at all

oslo.suggest.skip_history = { "rm", "shred", "trash" }       -- {} means every command
oslo.suggest.accept       = "ctrl-f"   -- as well as Right, which always accepts
oslo.suggest.accept_word  = "alt-right"

oslo.keys["alt-a"] = "accept-suggestion"       -- the same two actions, under oslo.keys
oslo.keys["alt-f"] = "accept-suggestion-word"  -- "accept-word" is accepted too

oslo.theme = { syntax = { autosuggestion = { fg = "244", italic = true } } }
```

Source names: `history`; `completion` or `completions`; `path`, `paths` or `file`; `predict` or
`prediction` — which needs the `vista` feature to *answer*, though it always parses; and `provider`
or `providers`.
A name nothing answers to is reported when the config is read rather than silently turning a source
off. Duplicates are dropped and the written order is kept.

`oslo.keys` is consulted before `oslo.suggest.accept`, so a key named in both does what `oslo.keys`
says. To turn suggestions off for a while without losing what the config said:

```lua
oslo.feature.set("suggest", false)   -- a mask; turning it back on restores oslo.suggest
```

### Your own source

```lua
oslo.suggest.provider {
  name = "tldr",
  answer = function(ctx)          -- ctx = { line, cursor, cwd, language }
    if ctx.line:match("^git com") then return "git commit --amend" end
  end,
}
oslo.suggest.sh_sources = { "history", "provider", "path" }
```

**It answers the whole line, not the tail.** That is what a provider naturally has — a page, a table
of examples, a model all know *the command*, not the seven characters left to type. oslo computes
the remainder.

**Where it sits is your decision.** Registering puts a provider in front of nothing; it is asked when
`oslo.suggest.sh_sources` says `provider`, in the position that list gives it. VS Code's inline
providers work the other way round — `yieldsToGroupIds` is declared by the *provider*, so the plugin
decides whether it defers to you. That is the part deliberately not copied.

Three rules a provider cannot escape:

- **It must continue the line.** The ghost is drawn as trailing text and Right accepts it, so an
  answer that is not a continuation would make that key insert something never suggested. Such an
  answer is refused and reported in `messages` — never trimmed into something the provider did not
  write.
- **It must be quick.** This is the keystroke path. One that takes longer than 20 ms more than three
  times is switched off for the session and says so, because a shell that stutters looks like oslo
  being slow rather than like a plugin being slow.
- **Raising is declining.** An error is recorded once and the next source is asked; it never costs
  you the keystroke.

`oslo.suggest.providers()` lists what is registered, in the order they are asked.

### A provider that has to go and ask something

An answer over a network cannot be given on the keystroke path, so a provider says `request` instead
of `answer` and calls `reply` whenever the answer turns up:

```lua
oslo.suggest.provider {
  name = "llm",
  debounce_ms = 120,       -- wait for typing to stop; 120 is the default
  timeout_ms = 2000,       -- give up after this; 2000 is the default
  request = function(ctx, reply)
    oslo.spawn { "my-llm", ctx.line, on_exit = function(out) reply(out) end }
  end,
}
```

`request` still runs on the keystroke path — what makes it asynchronous is what it *starts*, not how
long it takes to start it. `oslo.spawn` is the intended shape.

The three things that make this safe are worth knowing, because they are what an inline-completion
feature usually gets wrong:

- **The debounce is real.** A line is asked about only once typing has been still for
  `debounce_ms`, so a word typed at speed is one question rather than eight. The editor is woken at
  that moment rather than waiting for your next keystroke — otherwise the delay would expire only
  when you pressed the very key it exists to avoid asking about.
- **`reply` is bound to the line it was asked about.** An answer for `gi` that arrives after you have
  typed `git ` does not match and is never drawn. This is not a check that could be forgotten: the
  answer is stored under the question, and only an exact match is drawn.
- **A decline is remembered, and so is a timeout.** Either way the line is not asked about again;
  otherwise a provider that had stopped answering would be asked once per frame for ever.

When the answer lands the line repaints on its own — the same mechanism an asynchronous prompt uses.

### What a late answer may do to what is already there

The decision this whole mechanism exists for. `predict` answers in microseconds and a model over a
network answers in hundreds of milliseconds, so by the time the slow one speaks there is usually
already a suggestion drawn. Neither answer to *what now* is right for everybody:

```lua
oslo.suggest.provider {
  name = "llm",
  on_late = "fill",       -- the default: draw only if nothing else did
  on_late = "replace",    -- draw over what another source gave
  settle_ms = 400,        -- but only if it came back within this; 400 is the default
  request = …,
}
```

**`fill` never changes what is on screen.** A provider set this way answers in a second pass that
runs only when every source declined, so it can add a suggestion but never take one over — and its
position in `oslo.suggest.sh_sources` stops mattering, because "answer when nobody else did" is a
different question from "answer before history".

**`replace` answers in its own turn**, which is what puts it ahead of whatever is listed after it.
That is the setting for when the slow provider is the one you actually trust. `settle_ms` is what
makes it liveable: an answer that took longer than that is not allowed to rewrite a line you have
had time to read — though it is still offered in the fill pass, where replacing nothing is not
startling.

There is no `drop`. For a provider that answers later, every answer is late, so a mode that refused
late answers would be a slower way of writing `oslo.suggest.forget()`.

### When to ask at all

```lua
oslo.suggest.provider {
  name = "llm",
  min_chars = 4,          -- a model asked about `g` is being asked nothing
  max_line = 512,         -- a pasted 4 KB line is not a prompt
  enabled = function(ctx) -- anything the two above cannot express
    return not ctx.cwd:match("/private")
  end,
  request = …,
}
```

`enabled` **is** the context rule. A `when = { command = …, cwd = … }` table was the other design,
and a predicate is strictly more expressive than any set of fields would be, in the language the rest
of the config is already written in. For a provider that would send your typing off the machine, it
is the setting that matters most — and a predicate that raises is read as *no*, because a guard
nobody can evaluate has not said yes.

The cheap tests run first: `min_chars` and `max_line` are integer comparisons, and calling into Lua
to discover that the line was two characters long would be the expensive way to answer a cheap
question.

## Measurements

The provider mechanism costs a shell that has none **nothing**. `bench/keystroke.rs`, min of three
runs, against the same tree without it:

| | paint | hint |
|---|---|---|
| without providers | 2.23 µs | 2.20 µs |
| with the mechanism, none registered | 2.20 µs | 2.18 µs |

`provider` is not in the default `sources`, so the arm is never reached; when it is, `any()` is a
flag read. A *registered* provider costs whatever it does, bounded by the 20 ms budget above.

`cargo bench --bench keystroke`, release, on this machine:

| path | cost |
|---|---:|
| `paint` (colouring the line) | 2.12 µs/keystroke |
| `command_hint` against all of `$PATH` | 2.12 µs/keystroke |
| `settings::current()` | 0.02 µs/read |

`cargo bench --bench predict`, release, same machine — one `next` call with a partial line, which is
what the `predict` source costs per keystroke:

| history | predict |
|---|---:|
| 1,000 commands | 1.7 µs |
| 10,000 commands | 4.2 µs |
| 50,000 commands | 4.2 µs |

The history source's two store queries were measured against a 25,000-row, 3,000-directory store and
are recorded in `crates/oslo-ui/src/recall/nearby.rs`: the exact-directory question 33 µs, the
worktree question 1.8 ms for a long prefix and 7.1 ms for a single character. Typing one 26-character
line cost 69 ms of database work before the memo existed. With the memo the same line cost 13 ms the
first time and 0.8 ms once the answer had been remembered — 86 µs on the slowest keystroke.
**The memo is not an optimisation, it is what makes the worktree question shippable.**

## What it cannot do

- It can only ever be a strict continuation. Fuzzy matching is a dropdown feature and stays there;
  a suggestion that reordered or replaced your characters cannot be drawn as text after the cursor.
- Nothing is suggested from the middle of a line, or for an empty one.
- A multi-line history entry is never offered, from either the store or the flat set. Ghost text is
  one row, and printing embedded newlines would strand the tail under the prompt.
- The `predict` source is absent from a build without the `vista` feature, which includes the
  published `oslo-minimal`. It is named in the config either way and simply answers nothing there,
  so a shared config does not break — but nothing announces that it is inert.
- The `predict` source carries no language filter — `Model::next` sends only the partial line, so
  the mode a command was learnt under does not narrow the query. `path` is not language-filtered
  either; only `history` and `completion` are.
- The `path` source does not know what the command expects. It lists a directory; it has no notion
  of an argument that should be a hostname, a branch or a signal name.
- Path matching is case-sensitive and ignores `oslo.completion.case_sensitive`, which governs the
  dropdown.
- The worktree memo does not hear about other terminals. A line learnt elsewhere may take until the
  next command to be offered in a worktree this shell has already asked about.
- `predict` is silent until the snapshot has loaded on its background thread, and silent for good
  in a shell that keeps no history.
- Ranking is per source only. There is no way to say "prefer the model's answer when it is confident
  and history's otherwise" — the first source with an answer wins.

## Where it lives

| path | what |
|---|---|
| `crates/oslo-ui/src/lib.rs` | `OsloHelper::suggest`, `consumes_its_arguments`, `paint_hint` |
| `crates/oslo-ui/src/hinting.rs` | `command_hint`, `path_hint`, `Ranked::beats` |
| `crates/oslo-ui/src/recall/mod.rs` | `recall::suggest`, `remembered`, `seed`, `remember` |
| `crates/oslo-ui/src/recall/nearby.rs` | `from_store`, `place`, `remembered_answer`, `forget_answers_for` |
| `crates/oslo-base/src/track/query.rs` | `Track::suggestion_here`, `Track::suggestion_in_workspace` |
| `crates/oslo-base/src/track/row.rs` | `RunRow::worked`, `RunRow::standing` |
| `crates/oslo-ui/src/suggest.rs` | the provider registry, the continuation invariant, the budget |
| `crates/oslo-runtime/src/lua/api/suggest.rs` | `oslo.suggest.provider` — the Lua reader |
| `crates/oslo-ui/src/pending.rs` | what the editor waits for when an answer is not ready yet |
| `crates/oslo-ui/src/settings/mod.rs` | `Source`, `Suggest` |
| `crates/oslo-ui/src/settings/from_lua.rs` | reading `oslo.suggest` |
| `crates/oslo-ui/src/edit/session/accept.rs` | `Session::take_hint` |
| `crates/oslo-ui/src/edit/session/frame.rs` | `draw`, `first_word` |
| `crates/oslo-runtime/src/startup/native.rs` | `Assist::hint_text`, `Assist::binding` |
| `crates/oslo-base/src/predict/mod.rs` | `suggest_here`, `Model::next` |
| `bench/keystroke.rs`, `bench/predict.rs` | the numbers above |
