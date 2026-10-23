# The terminal knows what is happening

Everything oslo tells the emulator or the multiplexer it is running inside: where the shell is,
what it is running, where a prompt ends and output begins, and which extra input protocols the
editor may use. It exists because a terminal cannot infer any of that from the bytes it is shown —
scrollback is just text unless the shell says otherwise.

<!-- demo:begin -->
[![terminal-integration demo](https://asciinema.org/a/1262750.svg)](https://asciinema.org/a/1262750)
<!-- demo:end -->

## How it works

Three things are decided before the first prompt is drawn, in this order: what the host *is*
(`$TERM_PROGRAM`), what the user asked for (environment opt-ins), and what the terminal actually
answered. The result is one immutable snapshot in a `OnceLock`, and nothing re-queries afterwards.

```
oslo starts, interactive, $TERM ≠ dumb, /dev/tty opens
   │
   │  ONE write, in this order (negotiate::request):
   │    OSC 11 ; ?              what colour is the background — skipped if $COLORFGBG did
   │    CSI ? u                 is the Kitty keyboard protocol here
   │    CSI ? 2026 $ p          is synchronised output here
   │    OSC 99 ; i=<id>:p=? ;   can you show a title-and-body notification
   │    CSI c                   primary device attributes ─── the barrier
   ▼
   read one byte at a time · 100 ms deadline · ≤4096 B per reply · ≤16384 B in total
   ┌──────────────────────────────────────────────────────────────────────────────┐
   │ Broker: classify(candidate) → Prefix (wait) | Complete (a reply) | Reject     │
   │ Reject moves the candidate's first byte into `pending` and reclassifies       │
   └──────────────────────────────────────────────────────────────────────────────┘
   replies BEFORE the CSI c barrier are believed; anything after it is ignored
   bytes that were never a reply → `pending` → handed to the first line editor, in order
   │
   ▼
   Capabilities::from_environment($TERM_PROGRAM)          host contract
       .with_explicit_opt_ins($OSLO_SYNC_OUTPUT, $OSLO_CLICK_EVENTS,
                              $OSLO_TERMINAL_EXTENSIONS) exact opt-ins
       .with_verified(what actually replied)             negotiation
```

Two constants in there are load-bearing. `TIMEOUT_MS` is 100 and is followed by
`const _: () = assert!(TIMEOUT_MS <= 100);`, so the whole exchange cannot be lengthened by
accident. The second is `SETTLE_MS`, 20, and **it exists because a device-attributes reply does not
prove the terminal answered — only that somebody did.** A multiplexer, an ssh session, `script`, or
an editor's embedded terminal will answer `CSI c` from its own emulation while the queries it
forwarded are still in flight; ending the read there left the real replies to arrive during the
first prompt, where a report about the keyboard protocol is indistinguishable from typing. So the
barrier starts a 20 ms settle rather than ending the read. A terminal on its own goes quiet at once
and pays the settle exactly once, at startup.

### One balanced lifecycle

Marks are emitted through a state machine, not by printing at convenient moments. Any transition
the machine does not accept produces no bytes at all.

```
 phase          event                     what goes to stdout (portable OSC 133)
 ─────────────────────────────────────────────────────────────────────────────────────
 Idle
   │   InteractionStart              nothing — the shell only starts counting
 Interaction
   │   PromptStart(Primary)          ESC ] 133 ; A ; aid=<pid>-<started> ST
 Prompt                                                       ← A and B are drawn with the
   │   InputStart                    ESC ] 133 ; B ; aid=… ST   block, not at the transition
 Input
   │   PromptStart(Continuation)     ESC ] 133 ; A ; k=s ; aid=… ST
   │      └── back to Prompt, same interaction (opt-in; see below)
   │   CommandStart                  ESC ] 133 ; C ; aid=… ST
 Output
   │   CommandEnd { status }         ESC ] 133 ; D ; <status> ; aid=… ST
 Idle

 Interaction | Prompt | Input ──InteractionAbort──► ESC ] 133 ; D ; aid=… ST  (no status)
```

## A and B are drawn with the block, so they repeat

`A` and `B` name rows. The line editor replaces its block by erasing from the block's first row to
the end of the screen and writing the rows again, and a row it recreates has lost the mark it wore
— so both marks are part of the frame. Every repaint puts them back, and a line typed one key at a
time carries one `A`/`B` pair per frame. The transcript row that replaces a finished block carries
them too: that row is the command that was typed.

`C` and `D` are written once, at the transition, since nothing redraws over them.

For anything reading the stream: a repeated pair is a redraw, and a redrawn continuation prompt is
byte-identical to a second one. Count blocks, not pairs.

A blank line, a `Ctrl-C`, and EOF all take the abort arm, which closes the interaction with a `D`
that carries **no** status and no preceding `C`. Inventing a status there would tell the terminal a
command ran and failed. The status that is reported is put through `rem_euclid(256)` first, because
that is what a shell status is.

`aid` is the process id and the second the shell started, filtered down to alphanumerics, `-`, `_`
and `.`. It is stable for the life of the process, which is what lets a terminal attribute two
interleaved panes to the right shells.

### The sequences, and who they are for

| sequence | says | audience |
|---|---|---|
| `OSC 7` | cwd as `file://host/percent-encoded-path` | terminals that open a new tab or split where you were; the source names kitty, foot, WezTerm, Ghostty and multiplexers |
| `OSC 0` | window and tab title | anything with a title bar |
| `OSC 133 A/B/C/D` | prompt, input, output, exit status | any terminal that reads the portable marks |
| `OSC 633 A/B/E/C/D`, `P;Cwd` | the same lifecycle, VS Code's spelling | `TERM_PROGRAM=vscode` only |
| `OSC 8` | a hyperlink around text **oslo itself printed** | terminals with link support; others show the text |
| `OSC 52` | put this on the clipboard | the `copy` builtin, and it works over SSH |
| `OSC 99` | a rich notification, title and body | only when the terminal answered the `p=?` query |
| `OSC 777 ; notify` | title-and-body notification, the urxvt spelling | the fallback when OSC 99 was not verified |
| `OSC 9;4` | indeterminate / complete / clear progress | `TERM_PROGRAM=iTerm.app` |
| `CSI ? 2004 h/l` | bracketed paste on, off | every terminal, while the editor owns the line |
| `CSI > 5 u` / `CSI < u` | push and pop Kitty keyboard enhancements | verified Kitty-protocol terminals |
| `CSI ? 2026 h/l` | synchronised frame | verified, or `OSLO_SYNC_OUTPUT=1` |

Every field is escaped before it is emitted: `C` percent-encodes the command, VS Code's fields
escape `;` as `\x3b`, `OSC 99` base64-encodes its title and body while the `OSC 777` fallback
replaces `;` with `,` and drops control characters, and titles lose control characters. A
half-terminated OSC swallows whatever text follows it, so this is a correctness rule and not a tidy
one. **A command run with a leading space keeps its whole lifecycle
but publishes no command text**: the mark is about timing, and the text is the private part.

### Input, while the editor owns the line

Bracketed paste is enabled only for `Screen::Line` — the line editor — and disabled again when the
guard drops. A pasted newline is text in the buffer; nothing runs until Enter. A paste is capped at
1 MiB and rejected whole if it is not valid UTF-8, and an `ESC [ 201 ~` prefix appearing inside the
pasted text is kept rather than treated as the end.

Kitty enhancements are pushed with `CSI > 5 u` — flag 1 disambiguates escape codes, flag 4 reports
the alternate (shifted) key. The second flag is why `Shift+3` produces `#`: without it the terminal
names only the base key, and which character the shift layer produces is a property of the keymap,
not of the codepoint. The push is popped before the command runs and re-pushed afterwards, and a
full-screen view opened mid-line (the finder, a picker) does the same, tracked by a thread-local.

`f1` to `f12` decode from every spelling at once: `SS3 P/Q/R/S`, `CSI 11~`–`CSI 24~`, `CSI 1;1P`-style
parameterised forms, and Kitty's disambiguated `CSI P`, `CSI Q`, `CSI S`. **`CSI R` is deliberately
not F3**, because it is also the cursor-position report; F3 arrives as `CSI 13~` under
disambiguation. A modified or repeat/release F-key decodes to `Ignored`, not to the plain key.

### Asking the terminal, and telling a config what it said

oslo does not guess what a terminal supports from `$TERM`. It asks — the kitty keyboard query
(`CSI ? u`) and the rest go out behind a Primary DA barrier, which every terminal answers, so a
terminal that ignores a query is detected by the barrier arriving without it rather than by a
timeout.

What that negotiation learned is readable from Lua, because a config has the same problem the shell
does and no way to answer it otherwise:

```lua
oslo.term.kitty_keyboard()      --> true — Ctrl+Enter and Ctrl+Tab exist at all
oslo.term.all()                 --> { kitty_keyboard = true, bracketed_paste = true, … }
oslo.term.origins()             --> where each answer came from: asked, assumed, or configured
oslo.term.negotiated()          --> whether the asking finished
oslo.term.semantic()            --> "osc133" | "vscode633" | "none"
oslo.term.size()                --> { columns = 120, rows = 40 }
oslo.term.colors()              --> "truecolor" | "256" | "16" | "none"
oslo.term.background()          --> the terminal's own background colour, if it answered
```

Fourteen capabilities have a predicate each: `kitty_keyboard`, `synchronized_output`,
`bracketed_paste`, `semantic_clicks`, `legacy_clicks`, `osc99_notifications`, `osc99_unfocused`,
`osc9_progress`, `osc133_cmdline_url`, `osc1337_user_vars`, `semantic_basic`, `semantic_secondary`,
`semantic_right_prompt`, `vscode_rich`. They come from one table in the source, so `all()`,
`origins()` and the individual predicates cannot drift apart.

**`kitty_keyboard` is the one that decides things**, because Ctrl-I *is* Tab and Ctrl-M *is* Enter in
the legacy encoding — a chord involving either is indistinguishable from the plain key before it
leaves the terminal. That is why the Lua prompt's Enter behaviour is a config decision rather than
something oslo infers:

```lua
if oslo.term.kitty_keyboard() then
  oslo.lua.enter = "newline"   -- Enter adds a line, Ctrl+Enter sends
end
```

oslo does **not** make that choice itself, and the reason is worth recording: it briefly did, from
this very probe, and it chose wrong whenever Ctrl+Enter was *grabbed* by the terminal or the window
manager. No probe can see a grab. A capability answer says the encoding exists, not that the
keystroke reaches you.

### A prompt starts on a row of its own

A command's output goes from the command straight to the terminal; the shell never sees it. So a
command that ends **without a trailing newline** — `printf x`, a `curl` whose body has none, any
program that stops mid-row — leaves the cursor partway along a row that has text on it. Drawn from
there, the next prompt is written over that row and **the output is gone**: not scrolled, not
hidden, overwritten.

```text
$ printf ONELINE       the command writes 7 characters and no newline
ONELINE▊               the cursor is at column 8 of that row
$ ▊                    the next prompt is drawn from column 0 of the same row
```

There is no way to work it out — the bytes did not pass through oslo, the width is the terminal's,
and a program may have moved the cursor itself. So oslo asks: `ESC[6n` before drawing a prompt, and
a newline first if the answer is not column 1.

**The standard report, not the private one.** `ESC[6n` is answered by everything back to a VT100;
`ESC[?6n` is DECXCPR, which tmux — among others — does not answer at all, and asking it is
indistinguishable from asking nothing.

zsh and fish solve this by printing a marker padded to the width of the row so the terminal wraps,
which needs no reply but spends a blank row every time the cursor was already at column 0 — the
common case. Asking costs a round trip instead, and **a terminal is asked once**: silence is an
answer about the terminal rather than about the prompt, so it is remembered and never asked again.
That makes an unusual terminal cost 60 ms per session rather than 60 ms per prompt, and leaves it
behaving exactly as it did before.

## What makes it different

In bash and zsh, OSC 133 and OSC 7 come from a shell function you source — VS Code, iTerm2 and kitty
each ship their own integration script for exactly this. That works, and it means the marks are
emitted by whichever hook the script could hang itself on. oslo emits them from its own command
loop, through a state machine that refuses illegal transitions, so a cancelled line cannot leave an
unbalanced `C` behind and a prompt redrawn by something else cannot produce a second `A`. The `B`
mark is written by the line editor at the moment the prompt has reached the screen, rather than when
the prompt string was built; a sourced integration only ever has the string.

Neither bash nor zsh has a clipboard builtin. `copy` is one, and being `OSC 52` it reaches the
clipboard of the machine you are sitting at even when the shell is on the far end of an SSH
connection, with no `xsel` or `wl-copy` installed anywhere.

## Configuration

```lua
oslo.prompt.title = function(p)
  return p.command and (p.command .. " — " .. p.cwd) or p.cwd
end
```

`p.command` is the command about to run and is `nil` at a prompt — the same fact fish's `fish_title`
branches on. Unset, the title is the tilde-shortened directory at a prompt and
`<first word> — <directory>` while something runs.

```lua
oslo.notify.after   = 10                    -- seconds; 0 is off. 10 is the default
oslo.notify.title   = "{cmd} on this box"   -- {cmd}, {status}, {duration}
oslo.notify.command = "notify-send '{title}' '{body}'"   -- instead of the escape
```

`oslo.notify.command` runs detached with all three streams sent to `/dev/null`, because anything it
printed would land on the row the next prompt is about to be drawn on.

```lua
oslo.feature.set("marks", false)    -- no OSC 133, no OSC 7, no title, no host adapters
oslo.feature.set("notify", false)   -- no slow-command notice
```

`marks` is one switch over the whole group deliberately: a multiplexer that mishandles the marks in
one project mishandles the titles too. `OSC 52` is *not* under it, because `copy` is something you
asked for by name.

Features with no reliable discovery are exact opt-ins, matched as exact strings:

```sh
OSLO_TERMINAL_EXTENSIONS=kitty   # A;k=s continuations, and cmdline_url on the C mark
OSLO_SYNC_OUTPUT=1               # force DEC 2026 frames; =0 forces off even if verified
OSLO_CLICK_EVENTS=1              # OSC 133 prompt clicks (cl=line, click_events=2)
OSLO_CLICK_EVENTS=legacy         # DECSET 1000/1006, only while the editor has the line
```

`status terminal` prints the installed snapshot and where each selection came from — `disabled`,
`portable`, `host`, `verified` or `opt-in` — without running a single query.

## What it cannot do

- **Make other programs' output clickable.** `OSC 8` only ever wraps text oslo printed itself: a
  path in a diagnostic, a config file named in an error. oslo never sees `ls` or `cargo` output.
- **Know whether `OSC 52` worked.** There is no reply, and several terminals disable clipboard
  writes by default. Reading the clipboard back is not attempted at all.
- **Publish an iTerm2 or WezTerm user variable.** The gated `OSC 1337 SetUserVar` encoder exists and
  is tested, but nothing in oslo calls it yet.
- **Know whether the window is focused** when a slow command finishes. Asking needs a mode the shell
  would have to enable and read replies for, and stray characters at the prompt are a worse failure
  than a notification you did not need. The only focus handling is `:o=unfocused` on `OSC 99`, and
  only when the terminal itself advertised that occasion.
- **Emit continuation marks by default.** `A;k=s` and `cmdline_url` are behind
  `OSLO_TERMINAL_EXTENSIONS=kitty`; a terminal that does not know `k=s` would read the continuation
  as a new prompt.
- **Prove a nonce to VS Code.** oslo receives no documented launch nonce, so `OSC 633;E` is emitted
  without one rather than with an invented one.
- **Say anything in a script, `-c`, or under `TERM=dumb`.** Marks are off, no query is sent, and
  `status terminal` reports the disabled snapshot. A program reading oslo's output must never find
  escape sequences the shell invented in it.
- **Hide command text from the emulator.** Metadata is visible to the terminal process by
  construction; the protection is encoding and omission, not secrecy. Replies are taken at face
  value too — there is no second confirmation round.

## Where it lives

| path | what |
|---|---|
| `crates/oslo-ui/src/marks.rs` | `enable`, `Interaction`, `working_directory`, `title`, `clipboard`, `hyperlink`, `path`, `notify` |
| `crates/oslo-ui/src/term/semantic.rs` | `SemanticSession`, `SemanticEvent`, the phase machine |
| `crates/oslo-ui/src/term/osc133.rs`, `vscode.rs` | the two encoders, and their field escaping |
| `crates/oslo-ui/src/term/capability.rs` | `Capabilities`, `Verified`, `Origin`, the session `OnceLock` |
| `crates/oslo-ui/src/term/negotiate.rs` | `request`, `classify`, `select`, the DA barrier |
| `crates/oslo-ui/src/term/query.rs` | `Broker`, `query_sequences`, `SETTLE_MS`, startup input preservation |
| `crates/oslo-ui/src/term/metadata.rs` | `user_variable`, `progress`, `notification` (OSC 99 / 777) |
| `crates/oslo-ui/src/term/keyboard.rs` | `PUSH_ENHANCEMENTS`, `POP`, CSI-u `decode` |
| `crates/oslo-ui/src/term/input.rs`, `paste.rs` | `function_key`, `parse_csi`, `Keys`, the 1 MiB paste cap |
| `crates/oslo-ui/src/term/mod.rs` | `Restore::enter`, `Screen`, the push/pop bookkeeping |
| `crates/oslo-runtime/src/startup/terminal.rs` | the one call that negotiates and installs the snapshot |
| `crates/oslo-runtime/src/startup/notify.rs` | `slow_command_notice` |
| `crates/oslo-runtime/src/startup/read.rs` | `OSC 7` and the title, written before each prompt |
| `crates/oslo-runtime/src/startup/repl.rs` | `Interaction`, the running title, `output_start` |
| `crates/oslo-shell/src/env/builtins/copy.rs` | the `copy` builtin |
| `crates/oslo-shell/src/env/builtins/status.rs` | `status terminal` |
