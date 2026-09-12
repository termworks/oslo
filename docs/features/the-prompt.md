# The prompt

Five render keys, each a function of the same facts, and a prompt that can be a **list of named
pieces** rather than one string. A string is opaque: once the prompt is one, nothing can tell the
branch from the user's name, restyle one part, or drop the least important piece when the terminal
is narrow — it can only be truncated, and truncation always eats the end.

<!-- demo:begin -->
[![the-prompt demo](https://asciinema.org/a/1262752.svg)](https://asciinema.org/a/1262752)
<!-- demo:end -->

## How it works

The facts are gathered once and handed to whatever is going to draw with them: `segment_context`
builds one `Context`, `Context::to_lua` turns it into one Lua table, and `render_segments` clones
that same table for every segment. **Five segments therefore cost one branch lookup, not five.**
The branch is not a fork either — `git_branch` walks up for a `.git` entry and reads `.git/HEAD`,
answering a short hash when the head is detached.

```
 ┌ once per rendered prompt ────────────────────────────────────────────────┐
 │ segment_context()  status ok duration_ms cwd branch user host language   │
 │                    vimode cols jobs continuation command                 │
 └────────────────────────────┬─────────────────────────────────────────────┘
                              │  the same table, cloned per segment
                              ▼
  oslo.prompt.left ─┬─ "string"            used exactly as written, not called
                    ├─ function(ctx)       its return value; a number is allowed
                    ├─ { oslo.segment{} }  rendered → measured → fitted
                    └─ { command = "…" }   forked, with a deadline
                    │
            (unset) └─► oslo.ui.prompt(f) ─► lua line: built-in prompt
                                             sh  line: $PS1 → built-in prompt
```

The five keys are `left`, `right`, `continuation`, `transient` and `title`, and each resolves
through those same four shapes. `transient` is the prompt redrawn in place once the line has been
accepted, written only to a terminal; `title` names the tab, and is the key whose context has
`command` set while something runs. `oslo.prompt` itself is an empty table with `__index` and
`__newindex`, so an assignment reaches the registry rather than landing in the table — Lua does not
consult `__newindex` for a key a table already has, so the first shape of this, which pre-filled the
fields with setters, silently did nothing at all.

### Priorities, and what a narrow terminal drops

A segment's `render(ctx)` returns **spans** — `{ text = …, style = … }` — rather than a string, so
the styling stays data and a style is a *name* looked up in the theme. Each piece is measured in
printed cells, escapes not counted, and `segment::fit` drops the lowest priority until the rest fit.
Among equals the later piece goes first, so the prompt shortens from the right and the order written
is the order kept. The last piece is never dropped: a prompt of nothing is worse than one that
overruns.

```
 cols = 60, budget = cols/2 = 30      ← half the terminal, so there is room to type

 user(p10, 8) + cwd(p90, 20) + git(p50, 12) = 40 cells
   drop the lowest priority ──────────► user
                        cwd + git      = 32 cells   still over
   drop the lowest of what is left ───► git
                        cwd            = 20 cells   fits
```

### The right prompt

`prompt.right`, then `$RPS1` or `$RPROMPT`, then oslo's own: `❮`, the status *number* when the last
command failed — the left arrow can only go red — the duration when it is worth saying, the branch
and the directory. Nothing is said below 500 ms, so a quick success leaves only the branch and path.

It is laid out, not positioned with cursor motion: `edit/layout.rs` pads the first row out to the
right edge and appends it there, and only when it is *strictly* narrower than the columns left over
— equal would let it touch the text and read as one run. When it does not fit it is not drawn.
**There is no save/restore**, because a terminal has one DECSC slot and it is shared with the
dropdown and with any multiplexer hosting the session.

### Handing the prompt to another program

```
render(spec, ctx)   key = command + args AS WRITTEN, before $status is filled in
  │
  ├─ async = false ──► run it, deadline = timeout_ms (default 200 ms)
  │                     answered → cache it and use it
  │                     overran  → kill it, use the last answer
  │
  └─ async = true ───► spawn a thread, then:
       no cached answer yet → wait max(timeout_ms, 2 s)   ← the one cold prompt
       marked slow already  → the cached answer now, refresh behind
       otherwise            → wait timeout_ms; on a miss, mark slow, use the cache
                              an answer that DIFFERS bumps the generation, so it
                              lands on screen instead of one command late
```

The cache key is the argv **as written**. The first version keyed it on the substituted arguments,
and every prompt worth writing passes `$status`, `$jobs` or `$duration_ms` — all of which move
between prompts — so every lookup missed, `async` answered nothing, and the shell drew its own
prompt instead.

The grace on the first answer exists because answering nothing makes the caller fall back to oslo's
own prompt, which is a different *width*: the editor lays the row out against the width it was given
once, so the next redraw writes in the wrong place and the screen doubles up. The "marked slow"
branch is the mirror case — a tool that reliably overruns loses that bet every time, and with a left
and a right prompt that was `2 × timeout_ms` per command spent to arrive at the answer already in
hand. One answer inside the deadline clears the mark.

### `$PS1`, and when it is not used

`$PS1` wins over the built-in prompt and loses to a Lua one. It is **not** used for a Lua line: it
describes a shell prompt, and drawing `oslo$` in front of something that is not a shell command is
the confusion the language segment exists to prevent. `$PS2` is the continuation prompt (`"> "` when
unset), `$RPS1`/`$RPROMPT` the right one, and `$PROMPT_COMMAND` runs before every prompt — including
the first, which is where a bash prompt integration draws itself.

Escapes are decoded first, then the result goes through parameter expansion and command
substitution. The other order is not a refinement but a bug: the word parser reads a backslash as
quoting, so `PS1='\w'` would expand to the letter `w`.

| escape | is |
|---|---|
| `\u` `\h` `\H` `\w` `\W` `\s` `\$` | user, short host, full host, `~`-path, its basename, `oslo`, `#` for root |
| `\t` `\T` `\@` `\A` `\d` `\D{…}` | 24h, 12h, 12h am/pm, `HH:MM`, `Wed Aug 10`, any `strftime` |
| `\!` `\#` `\j` `\l` `\v` `\V` | next history number, the same, jobs, tty basename, version, version |
| `\a` `\e` `\n` `\r` `\\` `\nnn` | bell, escape, newline, return, backslash, an octal byte |
| `\[` `\]` | accepted and dropped |

An escape not in the table keeps its backslash, so a prompt written for bash degrades to something
readable rather than to silence.

### Caching and invalidation

The prompt is rendered when the line starts and then only when an input to it has moved. A
generation counter is what the editor's redraw loop reads — one relaxed atomic load per frame — and
three things bump it: a vi mode change, a terminal resize (a prompt told `$cols` is wrong at the new
width), and a background external answer that differs from the one cached. That last one needs a
second mechanism, because a blocking wait for a key cannot notice a counter: while any background
run is outstanding the key read becomes a 15 ms timed one, and a timeout with a moved generation
hands the loop a `PromptRefreshed` event that nothing binds. Without it the fresh answer sat in the
cache until the next keystroke — which, for the last prompt of a session, is never.

The built-in prompt is padded to `measured_width`: every language in `LANGUAGES` is rendered and the
widest wins, so switching between `sh` and `lua` cannot move the line. For a prompt that is *free*
to render — anything but `{ command = … }` — the three vi variants are rendered once per line and
kept, but only those of matching width, so the mode letter can be repainted without asking the
prompt's owner for it again.

## What makes it different

bash's `PS1` needs `\[` and `\]` around non-printing runs so readline can compute the prompt's
width. oslo measures what a prompt *prints* — escapes are skipped when counting cells — so the
markers say nothing it does not already know and are dropped rather than emitted. A prompt copied
out of a `.bashrc` works with them still in it.

bash has no right prompt at all. zsh has one under two names, and oslo accepts both: `RPS1` wins
over `RPROMPT` when both are set, as in zsh. A variable set to the empty string is an explicit
request for no right prompt and suppresses oslo's default rather than falling back to it.

`oslo.prompt.transient` has no equivalent to configure elsewhere: zsh has no way to say "redraw the
accepted line differently", and the implementations people use get there by wrapping ZLE widgets.
oslo owns its own line editor, so it is one render key and a rewind to the top of the block.
`oslo.prompt.title` is fish's `fish_title`, and a function rather than a setting for fish's reason —
the answer differs at a prompt and while something is running.

The segment shape is deliberately hexe's, so a config written for one reads the same in the other,
and a raw table is rejected for hexe's reason: a typo in a field name would otherwise produce a
segment that renders nothing, silently.


## What a finished line leaves behind

Running a line can replace its prompt with a record of what was run — a rule, the command in
brackets, and the exit code of the command before it:

```

---[ cargo test --lib ]------------------------------[ 14:54:48 ]---

running 801 tests
```

Off unless a config asks for it, drawable by another program, and marked with an escape sequence a
terminal can fold on. → **[transcript.md](transcript.md)**

## Configuration

```lua
oslo.prompt.left         = function(p) return p.cwd .. " > " end
oslo.prompt.right        = function(p)
  return p.duration_ms and (p.duration_ms .. "ms") or ""
end
oslo.prompt.continuation = function() return "… " end
oslo.prompt.transient    = function() return "> " end
oslo.prompt.title        = function(p) return p.command or p.cwd end
oslo.prompt.left         = nil   -- back to $PS1 / the built-in prompt
```

`ctx` carries `status`, `ok`, `duration_ms`, `cwd`, `branch`, `user`, `host`, `language`, `vimode`,
`cols`, `jobs`, `continuation` and `command`. What is not known is `nil` rather than an empty
string, so `if ctx.branch then` is the right test outside a work tree. A key that is not one of the
five raises on assignment rather than being quietly ignored.

```lua
oslo.theme.styles["my.dir"] = "fg:#8be9fd bold"

oslo.prompt.left = {
  oslo.segment{ name = "dir", priority = 90, render = function(ctx)
    return { { text = oslo.path.shorten(ctx.cwd, 3), style = "my.dir" } }
  end },
  oslo.segment{ name = "git",                             -- no priority means 50
    render = function(ctx) return ctx.branch or "" end },  -- a bare string is allowed
}
```


### A prompt that moves

A segment may ask to be drawn again on a clock, which is what a spinner is:

```lua
local frames, n = { "⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧" }, 0
oslo.prompt.left = {
  oslo.segment{ name = "spin", every = 100, render = function()
    n = n + 1
    return { { text = frames[(n % #frames) + 1], style = "prompt.aside" } }
  end },
  oslo.segment{ name = "git", render = function(ctx) return ctx.branch or "" end },
}
```

`every` is milliseconds, and a floor rather than a promise: the editor redraws when it next comes up
for air, which it does at least this often and sooner if a key is pressed. `0` is off. Below 60 ms is
clamped — the cap is on the segment so a config cannot ask for a rate that makes the shell unusable
over a link it was not written on.

**The other segments do not re-run.** This is the whole reason `every` is affordable. The prompt
keeps two counters: one saying the string on screen is stale, which a frame moves, and one saying
the *segments behind it* are, which only a real change moves — a new directory, a variable, a
branch. Each segment's output is kept under its name, and a frame re-runs only the ones whose
interval has come. A spinner at ten frames a second next to a segment that shells out to `git`
costs ten spinner calls and no `git` at all.

**Nor does an external prompt.** A prompt that is a command is the same rule taken seriously: it
never asks to be re-run, so a frame does not run it. Measured on a prompt of one external command
beside one segment at `every = 120`, left alone for three seconds — 23 frames and **24 spawns**
before that guard existed, 24 frames and **2** after. The two are the first render and the `async`
answer landing, which invalidates on its own so a late arrival still gets through.

Left unguarded it is not merely wasteful: with `async` the overlapping runs interleave, and what
that looks like on the terminal is a prompt whose colours come apart.

Measured, on a prompt of exactly those two segments left alone for three seconds: 29 redraws, four
spinner frames cycling evenly, and the other segment rendered **once**.

A segment with no `name` is never cached — two of them would share one entry and take turns
overwriting it — so an animated segment needs a name to be animated cheaply.
A style is a name. A dotted one is a theme slot — `prompt.user`, `prompt.host`, `prompt.cwd`,
`prompt.git`, `prompt.ok`, `prompt.failed`, `prompt.aside` — and follows the colour scheme that is
loaded; anything else is parsed as a colour, so `"cyan"` and `"#8be9fd"` work with no theme defined.
A definition in `oslo.theme.styles` outranks oslo's idea of a slot; the slots themselves are set
through `oslo.theme.prompt = { cwd = "blue", git = "green" }`. The helpers a prompt reaches for are
`oslo.git.branch()`, `oslo.git.root()`, `oslo.path.shorten(path, keep)` and `oslo.path.home(path)`.

```lua
oslo.prompt.left = {
  command    = "starship",
  args       = { "prompt", "--status=$status", "--cmd-duration=$duration_ms" },
  timeout_ms = 200,      -- the default; anything below 1 is clamped to 1
  async      = true,
}
```

Substitutable in `args`: `$status`, `$duration_ms`, `$cwd`, `$cols`, `$jobs`, `$language`,
`$branch`, `$vimode`, `$user`, `$host` — every field a native segment can render, which a test pins
— and `$frame`, which is about the drawing rather than the shell. An absent optional becomes the
empty string; anything else is left as written. The tool's stderr is inherited on purpose, since its
complaints belong on the terminal rather than folded into the prompt.

### An external prompt that moves

`every = <ms>` re-runs the command on a clock, so a prompt you have already built — with its own
colours, its own zones, its own layout — can grow a moving part without being rebuilt as a segment
list somewhere else:

```lua
oslo.prompt.right = {
  command = "pixy",
  args    = { "render", "prompt.right", "--target=ansi", "--frames-ms", "$frames_ms" },
  async   = true,
  every   = 150,      -- floored at 100; 0 or absent is off
  frames  = 1200,     -- ask for every frame of the next 1200ms at once
}
```

**`frames` is what keeps `every` from costing a process.** Without it, `every` starts a program per
frame for as long as the shell is open. But the animation needs nothing from outside — the pictures
are a function of time — so `$frames_ms` tells the tool how far ahead to draw, it answers with the
whole cycle, and oslo plays it back. `every` then only says how often to *redraw*.

```json
{"frames":[{"text":"⣾ …","next_frame_ms":150},{"text":"⣽ …","next_frame_ms":150}]}
```

One spawn per 1200 ms rather than one per 150 ms, for the same glyph turning at the same speed.

**And no spawn at all where nothing can be drawn.** `TERM=dumb` says the terminal cannot draw — it
is why oslo already sends no colour, no OSC 133 marks and no terminal queries — so a frame there has
no product, and asking for one would be a process spawned six times a second for a picture that
never appears.

This is not a hypothetical. It was found by a program driving oslo over a pty as a command channel:
its test suite finished in 0.4 s under `bash` and did not finish in 180 s under oslo. It presented
as a per-keystroke render — `strace` showed two `git status` per character typed — and it was
nothing of the kind. oslo renders a prompt once per prompt, then and now; what was ticking was
`every`, six times a second, into a terminal with nothing on it, and each tick ran a prompt tool
that itself shelled out to `git`, `sudo` and `oslo scratch`.

Measured on a 0×0 pty with `TERM=dumb`, one external prompt at `every = 150`, eight seconds:
**53 spawns before, 2 after** — the first render and the `async` answer landing. On a real terminal
the same test still spawns 53, because there the frames are the point.

The clock is simply never armed, so nothing downstream has to know: the wait goes back to blocking
on a key, and the prompt is rendered once per prompt — which is what a terminal that cannot draw
wanted from the start.

**Opt-in, because it is a promise about the tool**: that it understands the horizon and answers with
a list rather than a picture. Anything else it prints is drawn as the prompt, exactly as before, so
`frames` cannot break a tool that turns out not to speak it.

**Past the horizon the strip is spent.** The pictures are a function of time, but the *data* under
them — a branch, an exit status — is not, so it is asked for again rather than looped forever.

**`$frame` still works and is no longer needed.** It was a counter oslo kept because the tool was a
fresh process each time; a frame *number*, though, cannot say when the next picture falls due, which
is exactly what made the cycle impossible to enumerate. A tool that reads the clock and reports each
frame's hold needs no counter.

**It is a process per frame, and that is the price.** A segment's `every` calls a Lua function; this
one starts a program. The floor is 100 ms rather than a segment's 60 for that reason. Nothing else in
the prompt re-runs on a frame — see above — so this is the one cost, and it is opted into.

**And it is a rate limit, not only a clock.** With `async`, the answer landing invalidates the
prompt, and an invalidation otherwise reads as "run again" — so the prompt spawns itself as fast as
the tool can finish. Measured at 110 spawns in three seconds where twenty were asked for. So an
interval holds against a real change too: a new directory is drawn on the next frame rather than
immediately, which at 100 ms nobody sees.

Shell-side: `PS1`, `PS2`, `RPS1` or `RPROMPT`, `PROMPT_COMMAND`, and `$OSLO_MODE`, exported before
the prompt is drawn so a `PS1` can say which language it is prompting for. `OSLO_TIME_PROMPT=1`
reports where each prompt's time went, on stderr, one line per prompt.

## Measurements

`OSLO_TIME_PROMPT=1`, release build, in a pty, in this repository, built-in prompt on both sides:

```
oslo: prompt 0.1ms — prompt-right 0.1 · prompt-left 0.1 · size 0.0 · prompt-command 0.0
                   · macros 0.0 · direnv 0.0 · pre-prompt 0.0
```

An external prompt is run **twice per prompt**: once before the editor is entered, to record the
row's width, and once by the editor's own render closure. Counted with a `prompt.left` command that
appended a line to a file on each run — four runs across two prompts, with `async` both true and
false.

Two numbers recorded in the source, each from the change it forced: 91 ms per spawn of an external
prompt, so 273 ms added to every command while the three vi variants were rendered eagerly, which is
why they are now prepared only for a prompt that costs nothing (`startup/read.rs`); and `hexe shp
prompt` at ~33 ms against a 10 ms deadline, which is why a missed deadline is remembered
(`lua/api/external.rs`).

## What it cannot do

- **An external prompt runs twice per prompt.** The width has to be known before the editor starts,
  and the editor renders again for itself. `async` makes the second run cheap; a synchronous tool
  pays in full, twice.
- A segment is never told it was dropped; `Rendered` carries the name, but nothing reports it.
- The fitting budget is fixed at `max(cols, 20) / 2`. Only whole segments are dropped — one that
  would rather shorten itself has to do it in its own `render`, from `ctx.cols`.
- A prompt is one row. There is no multi-line prompt key, and the right prompt is drawn on the first
  row only.
- The vi mode letter is redrawn mid-line only for the built-in prompt, or for a prompt whose three
  mode variants come out the same width. Anything else catches up at the next prompt.
- `$PS1` cannot draw a Lua line's prompt, and there is no `PS1`-equivalent for `transient` or
  `title`. `$PROMPT_COMMAND` is read as one string of shell code and nothing else.
- A prompt function that raises is reported on stderr and the key falls back; it is not disabled, so
  one that always raises complains once per prompt.
- None of this runs in a script or `sh -c`. There is no prompt and no config.

## Where it lives

| path | what |
|---|---|
| `crates/oslo-runtime/src/startup/prompt.rs` | `segment_context`, `title_context`, `primary_prompt` |
| `crates/oslo-runtime/src/lua/context.rs` | `Context`, `Context::to_lua` |
| `crates/oslo-runtime/src/lua/api/prompt.rs` | the `oslo.prompt` table, `style_named`, `oslo.git` |
| `crates/oslo-runtime/src/lua/api/segment.rs` | `oslo.segment`, `describe`, `spans_to_text`, `fit` |
| `crates/oslo-runtime/src/lua/api/external.rs` | `Spec`, `spec_of`, `fill`, `render`, `spawn`, `run` |
| `crates/oslo-runtime/src/lua/engine.rs` | `render_with`, `render_segments`, `prompt_is_free` |
| `crates/oslo-runtime/src/startup/rc.rs` | `ps1`, `ps2`, `rps1`, `expand_prompt`, `decode_escapes` |
| `crates/oslo-runtime/src/startup/read.rs` | the render closure, the transient prompt, the vi variants |
| `crates/oslo-runtime/src/startup/timing.rs` | `OSLO_TIME_PROMPT` |
| `crates/oslo-ui/src/prompt.rs` | `git_branch`, `measured_width`, the generation and refresh counters |
| `crates/oslo-ui/src/row.rs` | `note_row`, `repaint`, `rewind_after_readline` |
| `crates/oslo-ui/src/edit/layout.rs` | `place` — where the right prompt is actually drawn |
| `crates/oslo-ui/src/edit/session/frame.rs` | `next_input`, the 15 ms slice while a refresh is outstanding |
