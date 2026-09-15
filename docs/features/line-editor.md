# The line editor

oslo owns the row it edits — buffer, layout, redraw, emacs and vi keymaps — rather than renting it
from a library. There is no `readline` and no `rustyline`, which is what makes the parts that are
hard to get right (grapheme boundaries, wrapping, the pending wrap, a right prompt that does not
fight the multiplexer) things that can be asserted in a unit test with no terminal anywhere.

<!-- demo:begin -->
[![line-editor demo](https://asciinema.org/a/1262740.svg)](https://asciinema.org/a/1262740)
<!-- demo:end -->

## How it works

The editor is three pure layers with one impure loop on top. `Buffer` holds the text and the cursor
and every edit is a method on it. `layout::place` turns a prompt, the text, a cursor, a hint and a
terminal width into cell coordinates. `screen::redraw` turns those coordinates into escapes. Only
`session::read_line` reads a key or writes a byte.

`Session::apply(key, assist)` is the state machine, and it performs no terminal I/O of its own: it
is a function from (state, key) to (state, `Step`). Anything that needs the screen is handed back to
the loop as a `Step` — `OpenCompletion`, `ToggleLanguage`, `ClearScreen` — because the loop is the
only thing that knows what a language is or where the block starts. That split is why 105 unit tests
can drive the editor a key at a time without a pty.

```
key
 ├─ Key::Resized ───────► prompt::invalidate() + redraw. Never reaches a binding:
 │                        nobody pressed it, and a prompt that measured the old
 │                        width is wrong at the new one.
 ├─ watches_keys() ─────► Assist::key_hook(key, text, cursor)
 │   (one atomic load)      Swallow │ Line{text,cursor,submit,erase} │ None = carry on
 ├─ Assist::binding(key)─► oslo.keys, then suggest.accept*, then oslo's defaults
 │                         ► Bound::{ToggleLanguage, ClearScreen, SearchHistory,
 │                                   AcceptHint, AcceptHintWord, Interrupt,
 │                                   Complete, Lua(name)}
 ├─ Right, not vi normal ► take_hint() else take_repair(); if neither had
 │                         anything, fall through to an ordinary cursor move
 ├─ Vi::apply ──────────► Handled{redraw}, or Passthrough for insert mode,
 │                        Enter and the interrupt keys
 └─ keymap::action(key) ► Action ─► one Buffer method ─► Step
```

**A configured binding is consulted before vi, and the order is load-bearing.** Vi reads `Alt(x)` as
Esc-then-`x`, because a terminal spells `M-x` as `ESC x` and vi users press Esc-then-key fast enough
that both bytes arrive together. Checking vi first would make every `oslo.keys["alt-…"]` entry
unreachable for anyone using vi mode, and an explicit binding has to beat a heuristic about what
somebody probably meant.

`Step::Continue { redraw: false }` is the other half of the loop's economy. A key that moved nothing
— an unbound chord, End pressed at the end, a refused vi key — does not repaint, and building a
frame is where the hint lookup and the layout pass live.

### The Assist seam

`Assist` is the trait through which everything oslo-specific arrives: highlighting, the ghost hint,
the correction, the completion dropdown, history, the finder, abbreviations and the Lua key hooks.
Every method has a default that does nothing, so `NoAssist` is a complete implementation and is what
the tests use.

Two methods are shaped by cost rather than by meaning. `watches_keys` exists only so a session with
no `key` handler attached does not build a `String` of the line on every keystroke — "is anyone
listening" is answered first, with one atomic load. And `hint_text` answers the suggestion
**unstyled**, `paint_hint` styling it separately, because the same text is both drawn and inserted
and the drawn one carries escapes that must never reach the buffer.

### Graphemes, stored as characters

The cursor is an index into a `Vec<char>`, and every index that leaves the buffer is clamped to an
extended-grapheme boundary by `edit::display`. A shell line is short, so copying a `Vec<char>` is
irrelevant next to being able to say that `move_left`, `backspace`, `delete`, `transpose` and vi's
`r` all act on whole clusters. Combining marks, emoji skin-tone modifiers, regional-indicator flags,
keycap sequences and ZWJ families are each one thing to move over and one thing to delete.

Width is a separate question, answered in `layout`. A `DisplayMap` is built from the raw line before
anything is drawn: it renders control characters as inert notation (`^[`, `^I`, `^M`, `^?`, and
`U+0085`-style for the rest) while keeping the raw text exact in the buffer, and it maps raw cursor
offsets to rendered ones in both directions. Raw escape bytes therefore never reach a frame.

### Laying the row out

```
    0                                                              cols
    ├───────────┬─────────────────────────┬──────────┬─────┬─────────┤
    │  prompt   │  text (drawn painted,   │  hint    │ gap │  right  │
    │           │   measured plain)       │ (ghost)  │ ≥1  │ prompt  │
    └───────────┴─────────────────────────┴──────────┴─────┴─────────┘
     prompt_cells        │                      used
                         └─ cursor_cells        rows = physical_rows(used, cols)
                            cursor_row = cursor_cells / cols
                            cursor_col = cursor_cells % cols
```

Three rules fall out of that picture:

* **The painted text is drawn and the plain text is measured.** They must print the same characters;
  the highlighter is the only thing that knows where its own escapes are, so re-deriving that here
  would be a second, disagreeing parser.
* **The hint is part of the block but never part of the cursor.** It occupies cells, so it can push
  the block onto another row, and `cursor_cells` is computed from the text alone.
* **The right prompt is drawn inline, in the same string, only on the first row, and only when it
  fits** — `right_cells < cols - used`, strictly, so at least one blank column always separates it
  from the line. When it does not fit it is simply not drawn: wrapping it or letting it overlap
  what is being typed are both worse than leaving it out.

A row filled exactly to the last column keeps a *pending wrap*: the terminal has not moved to the
next row yet. `physical_rows` preserves that state, and getting it wrong is what walks a prompt up
the screen a row per keystroke.

### Putting it on the terminal

```
ESC[<from_row>A    \r    ESC[J    <frame>    \r    ESC[<n>A    ESC[<col>C
└ up to the block's │      │         │        │       │           └ across
  first row         │      │         │        │       └ up to the cursor row
                    │      │         │        └ commit the pending wrap
                    │      │         └ prompt + text + hint + right prompt
                    │      └ erase everything below: this is all ours, and it is
                    │        the only thing that removes a shorter frame
                    └ column one, only after the row is right
```

**Every move is relative and there is no `ESC 7` / `ESC 8`.** There is one cursor-save slot per
terminal and the multiplexer hosting the session shares it, so a restore lands wherever somebody
else's save left the cursor. Relative moves also survive scrolling: writing a frame at the bottom of
the screen moves the block up and moves the cursor with it, so the arithmetic stays true. The erase
happens at the top-left *before* anything is drawn — after the content it runs into the pending
wrap, where the cursor is still in the last column and `ESC [ J` would take back the character just
written.

The prompt is handed to `read_line` as a **function**, not a string, so a vi mode indicator can
repaint on Esc. It is not called per keystroke: a generation counter says when an input to it moved,
so a prompt that shells out still runs once per line. Frames are wrapped in DEC 2026
synchronized-output brackets when the terminal was verified to understand them.

## What makes it different

readline is a library a shell rents, and its state — the kill ring, the keymap, the undo list —
lives outside the shell that is using it. oslo's editor is part of the shell, so `Ctrl-D` can be
end-of-input on an empty line and forward-delete on a full one without anything having to be told
what "empty" means, and a `key` hook written in Lua sees a keystroke *before* any binding does.

The bindings themselves are readline's, deliberately: `C-w`, `C-k`, `C-u`, `C-y`, `C-t`, `M-b`,
`M-f`, `M-d`, `M-DEL`, `M-u`, `M-l`, `M-c`, each with its readline name in the comment beside it.
Both kinds of word are kept, because both habits are decades old — `M-DEL` takes one alphanumeric
run, `C-w` takes a whole whitespace-delimited word, so `C-w` after `/usr/local/bin` takes all of it.
What is *not* kept is readline's kill ring: oslo keeps one killed entry, on the grounds that `M-y`
is reached by a vanishing fraction of users and an unused ring is state that can still be wrong.

Right at the end of the line accepting the ghost suggestion is fish's `forward-char`, and oslo does
the same for the same reason — Tab opens the dropdown when there is a choice to make, Right says
"yes, that one". The vi cursor shapes follow fish too: `oslo.vi.cursor_*` where fish has
`fish_cursor_*`, in fish's vocabulary, so a config need not be translated word by word. A Lua key
handler answering `submit = true` is zsh's `bindkey -s '…\n'` — the key runs the line rather than
only typing it — and both `$RPS1` and `$RPROMPT` are read, because both are in people's fingers.

Adding `erase = true` to that runs it without ever showing it. The line is drawn as though nothing
had been typed, the cursor is parked at the top of the prompt block instead of stepped past it, and
the *next* prompt is drawn over those same rows. An accepted line normally stays where it was typed
because it is the record of what produced the output beneath it — but a key that *is* a command has
no such record to keep, and pressing it repeatedly would otherwise stack one `$ nav` and one prompt
per keypress.

**Nothing is cleared, deliberately.** The prompt stays on screen for as long as the command runs,
which matters when the command opens a floating pane beside it: clearing at the keypress would take
the shell away and leave a hole until the browser exited. The consequence is that whatever the
command prints lands on the prompt, and that is why this is opt-in — a key bound to something that
prints wants the default, which keeps its line as the record of the output below.

Only meaningful alongside `submit`; on its own it would take away the line you are still editing,
so it does nothing.

### A bracket that closes itself

```text
echo |          "  →  echo "|"        opened, and closed for you
echo "hi|"      "  →  echo "hi"|      stepped over, not doubled
echo "|"  backspace  →  echo |        both halves, because you only made one gesture
it|             '  →  it'|            an apostrophe: a word is to the left of it
echo |x         (  →  echo (|x        something is already there to close over
```

On by default, unlike vi mode, and for the opposite reason: this is not a different way of editing,
it is the same way with one keystroke saved. `oslo.autopair.enabled = false` turns it off.

**The whole feature is two questions about the neighbours.** Pairing is only right when the closer
would land where nothing else wants to be, so a pair opens only when the character to the *right* is
not something the closer would be pushed against — and, for a quote, when the character to the
*left* is not a word character. That second rule is the one that matters in a shell: `it's` and
`don't` are apostrophes, and a stray quote swallows the rest of the line.

Stepping over is decided *before* opening, and has to be: the character that closes a quote is the
character that opens one, so the other order would answer every closing quote by opening a new pair.

**It does not know whether the cursor is inside a string.** That would mean parsing the line on
every keystroke, and the answer would still be wrong halfway through typing one. One character on
each side is why it is predictable — you can see the reason for what it did without knowing what the
shell made of the line. It also never removes a character you typed: the worst it does is add one,
and backspace takes a closer only while the two are still adjacent and still a pair.

The rules are [zsh-autopair](https://github.com/hlissner/zsh-autopair)'s — the ones people have
actually lived with — restated as a table.

### Vi text objects

```text
echo he|llo there    ciw  →  echo | there     the word the cursor is in
echo he|llo there    daw  →  echo |there      the word and the space that goes with it
echo "he|llo"        ci"  →  echo "|"         inside the quotes
echo "he|llo"        da"  →  echo |           the quotes as well
f(a, b|, c)          ci(  →  f(|)             inside the innermost pair
src/li|b.rs          ciW  →  |                the whole path, not one segment of it
```

`i` and `a` after an operator, then what: `w` `W` for a word, `"` `'` `` ` `` for a quoted run, and
`(` `[` `{` `<` — or their closers, or vim's `b` and `B` — for a bracketed one. Every operator takes
them, so `yi(` copies an argument list and `da"` removes a quoted word with its quotes.

**A text object is not a motion**, which is why it is its own module
(`crates/oslo-ui/src/edit/object.rs`) rather than four more arms in the motion table. A motion
answers *where the cursor goes* and the operator turns that into a range — which is why `cw` has to
be special-cased into `ce`, because the range and the movement disagree. An object answers the range
outright and never moves anything by itself, and keeping the two apart is what stops the second
inheriting the first's exceptions.

Three character kinds decide a word: whitespace, word characters, and punctuation. That split *is*
`iw` — `src/lib.rs` is five objects to `w` and one to `W`, which is the whole difference between
them and why a path wants the capital.

Quotes are paired **from the start of the line** rather than searched outward from the cursor,
because the same character opens and closes: whether the one to your left is an opener depends only
on how many came before it. A `\"` does not end a string. As in vim, a cursor between two pairs takes
the next pair along.

`i` and `a` keep their own meaning when no operator is waiting — a bare `i` is still insert.

### `Ctrl-X` — the line, in your editor

A long line is hard to edit on one row whatever keymap you use. `Ctrl-X` writes the line to a
temporary `.sh` file, opens `$VISUAL` or `$EDITOR` on it, and takes back whatever was saved.

**A key, not a vi command.** zsh's is `v` in normal mode, which is unreachable for everyone using
the emacs keymap — most people. This is an ordinary binding and works in both. It is not readline's
`C-x C-e` either: that is a two-key chord and oslo has no chord mechanism at all, so one keystroke
on the same letter is the nearest thing that costs nothing to build.

Quitting without saving, or saving with no change, leaves the line exactly as it was — the same rule
the macro manager's editing follows, and the same `crate::editor::edit` behind it, so which editor
gets picked is decided in one place. The trailing newline every editor adds is dropped; newlines
*inside* stay, because a shell line may genuinely have them.

Rebindable like anything else:

```lua
oslo.keys["ctrl-x"] = "none"        -- give the key back
oslo.keys["alt-e"]  = "edit-line"   -- fish's binding for the same thing
```

## Configuration

```lua
oslo.autopair.enabled   = true        -- a bracket or a quote closes itself

oslo.vi.enabled         = true        -- vi mode; false for the emacs keymap only
oslo.vi.cursor_insert   = "line"      -- block / line / underscore, each + " blink"
oslo.vi.cursor_normal   = "block"
oslo.vi.cursor_replace  = "underscore"

oslo.misc.escape_delay  = 25          -- ms to wait for the rest of an escape sequence
oslo.misc.idle_timeout  = 0           -- seconds before on-idle-timeout fires; 0 is off

oslo.suggest.accept      = "ctrl-f"   -- as well as Right, which always accepts
oslo.suggest.accept_word = "alt-f"    -- one word of the suggestion
```

Keys are bound by name — `ctrl-r`, `alt-u`, `shift-tab`, `f1` to `f12`, `up`, `space` — never as
escape sequences, since the sequence a key produces is usually the thing being worked around. A
binding is either an action name or a function:

```lua
oslo.keys["f2"]     = "toggle-language"
oslo.keys["ctrl-s"] = function(line) return "sudo " .. line.text end
oslo.keys["alt-u"]  = function(line)
  return { text = line.text .. " [" .. line.word .. "]", cursor = 0 }
end
oslo.keys["shift-tab"] = "none"       -- unbind a key oslo bound before the config ran
```

The action names are a fixed list, so a typo is reported rather than silently doing nothing:
`toggle-language` (or `toggle-mode`), `clear-screen`, `history-search` (or
`history-search-backward`), `accept-suggestion`, `accept-suggestion-word` (or `accept-word`),
`interrupt`, `complete`, `edit-line` (or `edit-command-line`), `expand-glob` (or `glob-expand-word`,
on `alt-*` by default), `list-glob` (or `glob-list-expansions`, no default key), `yank-last-arg`
(or `insert-last-argument`, on `alt-.` and `alt-_`), `beginning-of-history` (on `alt-<`),
`end-of-history` (on `alt->`), `insert-comment` (on `alt-#`), and `none` (or
`nothing`). The two glob actions are described in [globbing.md](globbing.md#seeing-it-before-it-runs). `escape_delay` is the one worth raising over a
slow link: Esc alone is recognised only when no further byte arrives within it, so too low a value
makes an arrow key read as Esc. It is clamped to 1–2000 ms rather than refused.

## Measurements

`cargo bench --bench keystroke`, release, on the machine this was written on. The editor repaints
the whole row on every key, and this is what a repaint consults:

| | µs |
|---|---:|
| paint — colouring a 57-character line | 2.17 |
| ghost suggestion, command word against all of `$PATH` | 2.14 |
| repair, ordinary typing | 0.22 |
| repair, an actual near-miss | 29.05 |
| one settings read (the editor does 2–4 per key) | 0.02 |

`cargo test -p oslo-ui --lib edit::` runs 105 tests in under 10 ms. None of them opens a terminal.

## What it cannot do

* **Move vertically.** A pasted or typed newline is held in the buffer and laid out as a real row,
  but Up and Down are history, in both keymaps, and vi has no `j` or `k`. Home and End go to the
  start and end of the *whole* buffer, not of the visual line.
* **Undo in emacs mode.** There is no `C-_`; `Buffer::snapshot` is only ever called by the vi
  keymap, whose `u` walks back up to 128 states.
* **Give you readline's kill ring.** One entry, no `M-y` rotate. Vi's `p` and `P` use that same
  entry, so a vi yank and a `C-y` agree.
* **Search incrementally in place.** `C-r` hands the whole line over to the finder and takes a whole
  line back; there is no in-line `(reverse-i-search)` prompt.
* **Everything vi.** Normal, insert and replace, and nothing else: no visual mode, no `.` repeat,
  no named registers, no marks. Text objects *are* there — see below.
* **Guarantee purity across the seam.** `Session::apply` writes no bytes, but an `Assist` it calls
  may: `search_history` and `history_prev` can open the full-screen finder, and `complete` draws the
  dropdown, both from inside `apply`. The state machine is pure; the seam is a convention.
* **Out-measure the terminal.** Widths come from the `unicode-width` tables; a terminal that
  disagrees about how wide an emoji is puts the cursor somewhere oslo did not intend, and nothing
  here can detect that.
* **Edit at all without a terminal.** `read_line` falls back to a plain `stdin` read with no
  editing, and writes the prompt only under `TERM=dumb` — down a pipe the shell is driven by a
  script, and a prompt would be noise in the data.
* **Edit on a terminal that says it cannot draw.** `TERM=dumb` takes the same road, and readline
  answers that variable the same way. A dumb terminal cannot address a cursor, and every keystroke
  the editor answers is answered by redrawing a row *in place* — so the mechanism has nothing to
  write to, and what it produces instead is noise. Measured against a program driving oslo over a
  pty: **~7 KB of cursor movement and repainting per command**, into a stream being read as output;
  336 bytes with the editor off.

  Nothing is lost that the terminal could have shown — history recall, completion, the vi keymap and
  the ghost suggestion are all *drawn*, and a terminal that says it cannot draw has said it cannot
  have them. What is left is what a dumb terminal has always had: a prompt, a line, and Enter.

## Where it lives

| path | what |
|---|---|
| `crates/oslo-ui/src/edit/session.rs` | `Session::apply`, `Step`, `Bound`, `KeyHook`, `read_line` |
| `crates/oslo-ui/src/edit/session/assist.rs` | the `Assist` trait and `NoAssist` |
| `crates/oslo-ui/src/edit/session/frame.rs` | `draw`, `next_input`, `read_plain`, `first_word` |
| `crates/oslo-ui/src/edit/session/accept.rs` | `take_hint`, `take_repair` |
| `crates/oslo-ui/src/edit/buffer.rs` | `Buffer` — every edit, the kill entry, the undo stack |
| `crates/oslo-ui/src/edit/display.rs` | `DisplayMap`, `advance_cells`, the boundary helpers |
| `crates/oslo-ui/src/edit/layout.rs` | `Row`, `Placed`, `place`, `cursor_for_cell` |
| `crates/oslo-ui/src/edit/screen.rs` | `redraw`, `finish`, `At` |
| `crates/oslo-ui/src/edit/keymap.rs` | `Action` and `action` — the emacs table |
| `crates/oslo-ui/src/edit/vi.rs` | `Vi::apply`, motions, operators, counts |
| `crates/oslo-ui/src/vi.rs` | `Mode`, `Cursor`, `Cursors`, `observe` |
| `crates/oslo-ui/src/keys.rs` | `oslo.keys` key names and action names |
| `crates/oslo-runtime/src/startup/native.rs` | `ShellAssist` — oslo's side of the seam |
| `bench/keystroke.rs` | the numbers above |
