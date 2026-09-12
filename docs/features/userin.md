# Asking for something

Thirteen widgets — a filterable list, a text field, a yes/no, a file browser, a spinner, a pager and
the rest — reachable three ways: as a builtin at an oslo prompt, as a program from every other
shell, and from Lua. They are one implementation with three doors, so what a question looks like
does not depend on who asked it.

```sh
ui choose alpha beta gamma                 # at an oslo prompt: the builtin
oslo userin choose alpha beta gamma        # from bash, make, a .desktop file: the program
```

```lua
oslo.ui.choose{ items = { "alpha", "beta" } }          -- from a config or a plugin
```

The reason to have them at all is that a shell already owns a terminal and already knows how to draw
on it. A script that wants to ask something should not need a second program installed beside the
shell to do it — and on the machine this was measured on, that second program costs **21 ms and 13.7
MB** where this costs **0.4 ms** and is already there.

<!-- demo:begin -->
[![userin demo](https://asciinema.org/a/1263433.svg)](https://asciinema.org/a/1263433)
<!-- demo:end -->

## How it works

One body, three entry points. Every door parses argv (or a Lua table) into a *spec* — `Choice`,
`Input`, `Confirm`, `Browse`, `Table` — hands it to the widget in `oslo_ui::ask`, and reports the
`Answer` it gets back.

```
ui choose …          ┐
oslo userin choose … ├─→ ui::run(called, args) ─→ Choice{…} ─→ ask::choose ─→ Answer<Vec<String>>
oslo.ui.prompt.…     ┘                                                            │
                                                                    ┌─────────────┴─────────────┐
                                                              Given(v) → stdout          Cancelled → 1
                                                                    status 0             NoTerminal → 2
```

`Answer<T>` is the whole interface between a widget and its caller, and it has three cases rather
than two because *"they said no"* and *"there was nobody there"* are different facts and a script
that cannot tell them apart will eventually treat one as the other.

### The three rules a script depends on

* **The answer is stdout; everything else is stderr.** The panel, the legend, the border and the
  cursor all go to stderr, which is why `$(oslo userin input)` captures the answer and the widget is
  still visible while it is being asked. It is the same reason `read -p` puts its prompt there.
* **Cancelling is status 1 with no output.** `x=$(oslo userin input) || exit` is therefore correct.
  A widget that returned `""` on Esc would make cancelled and empty the same thing.
* **No terminal is status 2.** Distinct from cancelled, so `if …; then` can branch on "nobody was
  there to ask" — which is what a widget in a cron job or a CI runner hits.

### Items come from wherever you have them

Operands win; stdin is the fallback. `ls | oslo userin filter` and `oslo userin filter a b c` are
both the obvious thing, and neither needs a flag to say which it is.

### Two doors, and why the program is not redundant

The builtin cannot be reached from anything that is not oslo. A bash script, a `sh -c`, a Makefile
recipe, a status bar shelling out — all of them reach a *program*, and inside them the word `ui`
finds whatever is on `$PATH`. `oslo userin` is that program, running `ui::run` with a different name
in its usage line and nothing else changed.

It is the same shape `oslo scratch` uses, and for the same reason: one body means the two answers
cannot disagree.

## The widgets

| widget | answers with | its own options |
|---|---|---|
| `input` | one line | `--placeholder` `--prompt` `--value` `--password` `--required` |
| `write` | one block | `--header` `--placeholder` `--value` |
| `confirm` | **the status** — 0 yes, 1 no | `--yes` `--no` `--default` and the question |
| `choose` | the chosen lines | `--header` `--multi` `--height` |
| `filter` | the chosen lines, after typing | `--header` `--multi` `--height` `--exact` |
| `table` | the chosen row | `--separator` `--header-row` `--height` |
| `file` | a path | `--all` `--directory` `--height` |
| `style` | the text, framed | `--border` `--fg` `--bg` `--bold` `--padding` |
| `format` | the text, rendered | `--type markdown\|template\|code\|text` `--field K=V` |
| `join` | blocks side by side or stacked | `--horizontal` `--vertical` `--align` |
| `pager` | nothing; it shows | `--title` `--wrap` |
| `log` | nothing; it writes a line | `--level` `--time` `--field K=V` |
| `spin` | what the command printed | `--title` `--quiet` `--` then the command |

`confirm` is the one that answers with its status rather than its output, because `oslo userin
confirm "go on?" && do_it` is the shape every script wants; printing `yes` for the caller to compare
against would be a worse interface wearing the same clothes.

## What makes it different

**gum** is the closest thing, and the comparison is the reason this exists: it is a 13.7 MB Go
binary you install separately, and each call pays a process start it cannot avoid. Here the widgets
are inside a shell you are already running, so the same call is a builtin at a prompt and a 5.2 MB
static binary you already have from anywhere else. Numbers below.

**dialog / whiptail** take the whole screen and hand back their answer on stderr, which is the
opposite of the rule above and the reason every `whiptail` example ends in `3>&1 1>&2 2>&3`. These
draw in place, erase themselves, and leave the transcript above them untouched.

**bash's `select`** is a numbered menu with no filtering, no arrow keys and no cancel, and it is the
only thing POSIX-ish shells offer. `read -p` is the other half, and has no list.

**The Lua door is not a second implementation.** `oslo.ui.choose` and its siblings build the same
specs and call the same widgets, so a question asked by a config looks exactly like one asked by a
script. A Lua API that re-implemented what the shell does would drift from it, and everyone would
stop trusting whichever one they had not tested last.

It has one behaviour the command line does not: **without a terminal it asks on a line** instead of
answering nothing — `super::ask::on_a_line`, which works down a pipe and over a serial console and
leaves the question in the transcript. A config asking a question at startup must not simply fail
because stdin is a file.

## Configuration

Presentation options are shared by every widget, parsed once in `ui/chrome.rs` and `ui/look.rs`
rather than per widget — which is how `ui file --border` and `ui choose --border` would otherwise
come to mean subtly different things.

```sh
--border rounded|square|double|thick|none     --border-fg COLOUR   --fit content|full
--align start|center|end                      --padding-x N        --padding-y N
--no-legend                                   --legend-gap N       --fullscreen
```

The list widgets take the look as well: `--look`, `--filter-at top|bottom`, `--reverse`,
`--marker`, `--prompt`, `--placeholder`, `--slot-left`, `--slot-right`, `--list-width`,
`--surface-rows`, `--list-gap`, `--list-pad`, `--surface`, `--stripe`.

Colours come from the theme (`oslo.theme`), so a widget matches the shell it was asked from without
being told anything. `filter` takes its fuzziness from `oslo.completion.fuzzy` for the same reason —
matching should behave the way matching behaves everywhere else here — and `--exact` overrides it
for one call.

## Measurements

Fifty calls of the simplest widget that needs no terminal, best of three, on this machine:

| | per call | binary |
|---|---|---|
| `oslo userin style done` | **0.4 ms** | 5.2 MB — the whole shell, static musl |
| `gum style done` | **21 ms** | 13.7 MB — the widgets alone |

That is ~50× per call, and it is all process start: both do the same trivial amount of work. In a
loop that asks something once per item — a release script, a `for` over branches — it is the
difference between the prompt feeling instant and feeling like a program is being launched, because
one of them is.

At an oslo prompt the builtin pays no process start at all.

## What it cannot do

- **Ask without a terminal.** Every widget that reads a key answers `NoTerminal` and exits 2 down a
  pipe or in CI. There is no `--default` fallback except on `confirm`, where the default is the
  answer given when there is nobody to ask.
- **Give a script its own keymap.** The keys are the shell's — arrows, Tab, Esc, Ctrl-C, `y`/`n` on
  `confirm` — and nothing here rebinds them per call.
- **Say *which* of a multi-selection was cancelled.** `--multi` answers with the chosen lines or
  with nothing at all; there is no partial answer.
- **Carry structure.** Answers are lines. `table` picks a row and prints it as the text of that row;
  nothing here emits JSON, and a value containing a newline cannot survive the round trip.
- **Be a screen.** These are inline widgets that erase themselves; `--fullscreen` borrows the
  alternate screen for one question and gives it back. There is no persistent panel, no mouse and no
  layout beyond `join`.
- **Change the shell.** `oslo userin` is a process: it can tell you what somebody chose and cannot
  `cd`, set a variable or define a function. That is what the builtin and the Lua door are for.
- **Be recorded.** There is no demo for this page yet; every other feature document opens with one.

## Where it lives

| path | what |
|---|---|
| `crates/oslo-shell/src/env/builtins/ui.rs` | `run`, `builtin_ui`, `tool`, `report`, `usage`, and `input`/`write`/`confirm`/`style`/… |
| `crates/oslo-shell/src/env/builtins/ui/lists.rs` | `choose`, `filter`, `file`, `table` — the ones that take a `Look` |
| `crates/oslo-shell/src/env/builtins/ui/chrome.rs` | `chrome_flag`: the border, the placement, the legend |
| `crates/oslo-shell/src/env/builtins/ui/look.rs` | `look_flag`: the rows, the marker, the surface |
| `crates/oslo-ui/src/ask/mod.rs` | `Answer<T>`, `status`, and the stdout/stderr rule |
| `crates/oslo-ui/src/ask/choose.rs` | `Choice`, `choose`, `filter`, `pick_or_create` |
| `crates/oslo-ui/src/ask/confirm.rs` | `Confirm`, the two buttons, the default |
| `crates/oslo-ui/src/ask/{input,write,file,table,pager,spin,log,style,join,format}.rs` | one widget each |
| `crates/oslo-runtime/src/lua/api/ui/prompt.rs` | the Lua door onto the same widgets |
| `src/cli/tools.rs` | `userin` in `TOOLS`, and the bridge to `userin_tool` |
| `tests/userin_tests.rs` | the door: the widget list, and the three statuses through the program |
