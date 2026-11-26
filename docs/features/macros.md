# Macros

Five kinds of small named thing — an alias, an abbreviation, a function, a script, a variable —
managed with one subcommand, so that saving one does not mean editing a file and re-sourcing it.
Four of them live in one database. **A script is a file**: every file in `~/.local/sbin` with a
`#!` line is one, whoever put it there.

```sh
oslo macros add --alias  gs 'git status --short' --tag git
oslo macros add --abbrev gco 'git checkout'      # expanded into your line as you type
oslo macros add --func   mkcd                    # opens your editor
oslo macros add --script deploy                  # writes ~/.local/sbin/deploy, opens it
oslo macros add --var    'GITHUB_TOKEN=$(oslo secret get gh-token)'
oslo macros edit deploy                          # $EDITOR ~/.local/sbin/deploy
oslo macros show                                 # the manager, on the whole screen
```

The kind is required, because five kinds and a silent default is a trap. An inline body is for the
three that fit on a line; a function and a script always open the editor.

An alias in `init.lua` still works, and so does `alias` in a script. This is a second source, not
a replacement — see [Order](#order-config-first-database-last).

<!-- demo:begin -->
[![macros demo](https://asciinema.org/a/1262943.svg)](https://asciinema.org/a/1262943)
<!-- demo:end -->

## The five kinds, and why there are five

They are not variations on one idea; they differ in *when* they act and in *what sees them*.

| kind | acts | what runs is | found |
|---|---|---|---|
| `alias` | before the line is parsed | the replacement, invisibly | at shell start |
| `abbrev` | as you type the space after it | what is in your buffer, visibly | at shell start |
| `func` | when you call the name | shell code, **in this shell** | after `$PATH` fails |
| `script` | when you call the name | its own program, in a child | on `$PATH`, a file |
| `var` | when something reads the name | its body, once, in this shell | at shell start |

The pair that looks redundant is `alias` and `abbrev`, and the difference is the whole reason
[abbreviations](abbreviations.md) exist: an alias replaces the word before anything can see it, so
your history records `gs` and what actually ran is somewhere else; an abbreviation is expanded *into
the line*, so what runs is what you can read and what the history keeps.

The pair that looks like one thing is `func` and `script`. A function runs in the shell that called
it — it can `cd`, set a variable, change the shell it was called from. A script cannot: it is its own
program with its own shebang, and Python is as good an answer as `sh`. That is also why one is a
row and the other a file: a function is shell code this shell reads, a script is a program any
shell, tmux pane or `.desktop` entry may run.

### A variable holds a recipe, not a value

The fifth is the one that is not what it looks like. `oslo macros add --var EDITOR=nvim` stores a
value and the shell exports it at startup, which is unremarkable. `--var
'GITHUB_TOKEN=$(oslo secret get gh-token)'` stores a *line*, and the shell runs it the first time
something reads `$GITHUB_TOKEN` — once, in that shell, and never in a shell that does not mention
the name.

That difference is the whole point of storing one. Written in `init.lua` as an `export`, the same
line decrypts a secret at every shell start, on every machine, for ever, whether or not anything
wanted it; here a session that never touches the token never runs the command at all. It is the same
argument the [secrets](secrets.md) store makes about files, applied to time.

The split is by cost, and it is measured by `is_a_value`: a body with no command in it — `nvim`,
`/srv/data`, `$HOME/bin` — is exported at startup, because it is free and because a program that
reads the environment *itself* (`gh`, `aws`, `docker`) has no `$NAME` for the shell to expand and
would otherwise never see it. A body with `$(…)` in it waits.

Two consequences worth knowing:

* **A stored variable wins.** It overrules the config and the environment the shell was started
  with, the same way a stored alias and a stored abbreviation always have — they are applied in the
  same loop, after the config, and what you store is what you get.

  This ran the other way until it was noticed that it made the common case impossible: `EDITOR`
  arrives from a terminal emulator or a session manager, so `oslo macros add --var EDITOR=nvim`
  reported success and then changed nothing, for ever. The old rule protected `FOO=x oslo`, which
  still means what it says for every name nobody has deliberately stored — and a name you *have*
  stored is one you have already answered the question about. Unset it with `oslo macros remove`
  when you want the parent back.
* **A recipe reaches a program that reads the environment only once the name has been read.** For
  `gh` that means mentioning it — `echo "$GITHUB_TOKEN" >/dev/null` first, or writing the command as
  `gh …` after anything that expands it. A `secret run NAME -- cmd` that puts one value in one
  child's environment is the better answer and is not built yet.

## A script is a file

```
~/.local/sbin/deploy        a script that is on        ($OSLO_MACROS_BIN moves the directory)
~/.local/sbin/.off/deploy   a script that is off
```

Every file in that directory whose first line is a `#!` shebang is a script, and `oslo macros`
lists it — one you wrote with `add --script`, one you copied in by hand, one that arrived from
another machine. There is no registering step and no second copy: the file is the script, and
editing it with any editor at any time is editing the script.

```bash
#!/usr/bin/env bash
# tags: desktop, net

set -euo pipefail
```

What the database would hold for any other kind, the file holds for itself:

| | |
|---|---|
| name | the file's name |
| body | its contents |
| tags | the `# tags: a, b` line in the comment block at the top, commas or spaces between |
| on or off | which of the two directories it is in |
| created | the file's birth time |

`# @tag` would read better and cannot be used: argc refuses an `@` tag it does not know, so every
argc script would stop parsing its own arguments.

A script runs the way any program does: the shell finds the file on `$PATH`. bash, tmux and a
`.desktop` entry find it the same way, so the script that runs at an oslo prompt is the one that
runs everywhere else.

## Where a name resolves

A stored **function** is found **after** `$PATH`, exactly as `functions/*.sh` is, and for the
reason [autoloading](your-own-tools.md) states: a shell that promises a script sees POSIX behaviour
cannot have a database row quietly redefining `test`. Storing a `deploy` when the system already has
one gets you the system's. The database costs nothing on the ordinary path: it is opened only once a
command has already failed to resolve, and the first question asked is whether the file exists at
all.

A **script** is wherever its directory is on `$PATH`. An interactive shell puts the directory at the
front when `$PATH` does not hold it and leaves it alone when it does, so where you put it in your own
`$PATH` is the order you get.

```
deploy alpha
   │
   ├─ alias, function, builtin                       → no
   ├─ $PATH search          ~/.local/sbin/deploy     → run it, like any program
   │                        (skipped when this session turned `deploy` off)
   ▼
exec::stored::try_call
   ├─ does macros.db exist at all?    no ──► "command not found"
   └─ func deploy?                    yes ─► run it in THIS shell
```

`type`, `command -v`, `which` and `whereis` print a script's path, as for any file, and call a stored
function a *stored function*.

## When an alias loads — measured, and settled

Aliases and abbreviations have to be in place *before* the first line is parsed, which puts them on
the startup path, where the numbers are unforgiving:

| | |
|---|---|
| a shell start | 842 µs |
| opening the key–value store | **2.61 ms** |
| reading a flat file of the same entries | **3.6 µs** |

Opening a second store before the first prompt would cost three times the whole of startup. So the
database is not read at startup at all: every mutation republishes a **snapshot** — one flat file
beside the database — and a starting shell reads that.

```
oslo macros add --alias gs …
   │
   ├─ put into macros/macros.db          the store, for the manager
   └─ publish → macros/macros.snapshot   a flat file, for a starting shell

a new interactive shell
   │
   ├─ init.lua                          alias gs = …
   ├─ write macros/elsewhere.snapshot     ← what the *config* defined, for the manager
   └─ read macros.snapshot (3.6 µs)       the database wins, see below
```

**Interactive shells only.** This runs beside `load_config` and nowhere else, so `oslo -c` and a
script see none of it. That takes nothing away: `init.lua` is read by the same loop and by nothing
else either, so a non-interactive shell has never had aliases to expand.

### And a change reaches the terminal beside this one

Universal variables already solve this problem — *write it from anywhere, see it everywhere* — and
the mechanism is one `stat` per prompt on a file that is read only when it has moved. Macros use the
same one, deliberately:

```
per prompt:  stat macros.snapshot   ─┐
             stat session/<id>.off  ─┴─ moved? ─► rebuild:  what the config defined,
                                                            then the database on top,
                                                            minus what is off here
```

So `oslo macros add` in one terminal is live in the others before their next prompt, and a shell
where nothing changed pays two syscalls and no parse.

**The rebuild is a whole set, not a patch**, because the interesting case is *removal*: when `gs`
leaves the database the right answer is whatever your configuration said about `gs` — which may be
nothing, or may be a different alias. Composing both sources and diffing against what this shell was
last given answers that. An alias you typed at the prompt is never in that set, so it is never in the
difference, and a rebuild cannot take it away from you.

## Order: config first, database last

Three things define an alias — `alias` in a script, `oslo.alias` in `init.lua`, `oslo macros add`
— and the ordinary shell rule is that the last definition wins. That rule is applied to the sources:
the database is applied after the configuration, so **the database wins**.

That is the deliberate half. The database is the one you can change without editing a file, so
`oslo macros add --alias gco …` taking effect is what you asked for. The cost is that a stored entry
can shadow one you wrote in `init.lua` — which is why a shell writes down what its config defined,
and why the manager can show you both: **Tab** moves between `[stored]` and `[elsewhere]`.

`[elsewhere]` is aliases, abbreviations and **every variable this shell has** — which is what makes
it worth opening: a stored `EDITOR` that never applies because a profile already exported one is
invisible until the two lists are on the same screen. A function is a file on disk and a Lua function
is a name in a table; there is nothing to enumerate, so that source can never show either.

**Nothing there is editable.** An inherited row is a fact about this shell, not a record in the
database: there is no body to open and nothing to delete, so Enter and Delete do nothing and the
status line says `not editable` before either is pressed. Space still works, because turning one off
is a decision this shell can carry out — the off list is applied to the merged set, so an alias your
config defined can be turned off exactly like a stored one.

## The manager

`oslo macros show` on a terminal is a full screen, and it is the history finder's screen: the same
look, the same striping, the same search bar, the same delete confirmation. A second full-screen list
that behaved differently would be a second thing to learn.

**Alt+\\ opens it from the prompt** — beside Ctrl+\\ for the [scratch finder](scratch.md), the same
key with the other modifier for the other list of things you keep. `oslo.macros.key` moves it, and
takes any key name a config can write.

Alt rather than a Ctrl+Enter, which is the obvious choice and does not work: Ctrl+Enter sends the
same `\r` as Enter unless the terminal speaks the kitty keyboard protocol, and even then oslo
decodes it as Ctrl+M — historically the same key. Alt sends `ESC` and the character, everywhere,
with nothing to negotiate.

```
   3d   alias   gs        git status --short                    #git #system
   3d   abbrev  gco       git checkout                          #git
  12d   script  deploy    [[[ python3 ]]]                       #work
 ! 2d   script  gs        [[[ bash ]]]
 ✗ 1h   func    mkcd      [[[ sh ]]]

  ⬝⬝⬝⬝⬝⬝  >>  de▏                        [stored] @ [#work] || 1/4
```

| key | |
|---|---|
| type | filter — every column; `#git` asks for a tag, in any order and with anything else |
| ← → | **the kind**: all of them, then each one in use. Where the finder's scopes are |
| Tab | **the source**: `[stored]` ↔ `[elsewhere]`. Where the finder's profile is |
| Enter | **the editor**, for every kind, including an alias — `[stored]` only |
| Delete | forget it, after the same question the finder asks — `[stored]` only |
| Space | off **for this session** |
| Space ×3 | off **everywhere**, until three more turn it back on |

Enter is the one that differs from the finder, and it has to: the finder puts a line back on the
prompt because a past command is something to run again, while a macro is something you keep — and
what you want from it is to change it. On an `[elsewhere]` row it does nothing at all: that row has
no record behind it, and an Enter that quietly wrote a *new* macro shadowing the one you were looking
at would not be the key it looks like. To keep a version of your own, add it — the name is on the
screen in front of you. On a script, Enter opens the file itself.

**A `!` marks a name two kinds hold** — a script file dropped in beside a stored alias or function
of the same name. `add` refuses to make one, but nothing can refuse a file, so the list says so
instead, and `show --plain` repeats it on stderr. What runs follows the ordinary rules: an alias is
applied before anything is looked up, and a script on `$PATH` is found before a stored function.

**Off is not gone.** A macro turned off keeps its body, its tags and the day it was made; what
changes is that it stops applying. A script turned off everywhere moves into `.off/` beside the
others, so bash and everything else stop finding it too. Turning off an alias that shadows a
configured one uncovers the configured one, because off means *this one does not apply* rather than
*this name does nothing*. A row that is off is drawn muted with a marker, because a list where a
disabled row looks exactly like a live one answers the wrong question.

Both switches have a spelling for something that is not a person:

```sh
oslo macros off gs             # everywhere, now and next time
oslo macros off gs --session   # this shell only, until it closes
oslo macros on  gs
```

A session's list is a file named for the session, and the session is `$OSLO_SESSION` — which a shell
exports precisely so that a child process can name the session it is part of. `oslo macros` is a
child of the shell whose macros it manages; without a name both agree on, "off for this session"
would be written down for a session nobody is running.

## Editing, and getting back out

`--func` and `--script` always open an editor, because neither fits on a command line and pretending
otherwise invites a function written as one. The editor is `$VISUAL`, then `$EDITOR`, then `nvim`,
then `vi` — the two variables first because they are what you have already told every other program
on the machine. A script opens as the file itself, under its own name; `add --script` writes the
starter there first and deletes it again if the editor saved nothing. Every other kind goes through
a temporary file named for the language it holds (`.sh`, `.lua`, `.py`), because syntax
highlighting is most of the reason to want a real editor. Quitting without saving stores nothing and
says `unchanged`.

The screen closes while the editor runs and comes back with the query it had. Keeping it open around
a child that also drives the terminal is how two programs end up fighting over the same termios.

Piped, or with `--plain`, there is no widget and no colour: one tab-separated row per macro, every
field included. A manager only a person can read is one you cannot script.

A database is not a dotfiles repository: an alias in `init.lua` is version-controlled, diffable and
copied to a new machine with the rest of your configuration, and one in here is none of those.
`export` and `import` are the way back out, in a format meant to be read and hand-edited:

```
alias gs #git #system
	git status --short
func mkcd off
	mkdir -p "$1" && cd "$1"
```

The header carries the kind, the name, `off` if it is turned off, and `#tag` for each tag; the body
follows, indented by one tab. A script is exported from its file and imported back into one, its
tags written into its `# tags:` line.

## Tags, where a profile would be

A history is per profile because it is a record of what a particular shell did, and an agent's
commands must not pollute the ranking of yours. A macro is not a record of anything, so there is
**one database for the user** and a tag is a label on a row — several of them, if you like, which is
the thing a profile could never be.

```sh
oslo macros add --alias gs 'git status --short' --tag git --tag system
```

A script's tags are the `# tags:` line in its own header, so changing them is editing the file;
`add --script NAME --tag x` rewrites the line for you.

**A tag is asked for by typing it**, `#git`, in the search bar with everything else — in any order,
`#ai tool` or `tool #ai` or `#ai` alone, and scoped to the kind you are looking at. The tags are
taken out of the query and the rest is fuzzy-matched.

That is a deliberate swap. ← and → used to walk the tags, which spent the arrow keys — the most
obvious pair on the keyboard — on the one filter that already has a spelling you can type. They walk
the [kinds](#the-five-kinds-and-why-there-are-five) instead, which is the division a list of five
kinds is actually navigated by and the one thing you cannot type.

## Between machines

`oslo profile sync laptop` carries the macros as well as the history and the secrets, and all five kinds
travel — a function and a script included, not only the two a starting shell reads. The merge is by
name: for `alias/gs` one of the two copies wins outright, decided by the rule in
[Syncing between machines](syncing.md).

**Removing one is a tombstone rather than an erasure.** The record stays with its stamp buried, so
that `oslo macros remove gs` on one machine removes it on the other and the machine that lost it does
not hand it back on the next sync. That is a different switch from `off`, which is a setting *of* a
macro that still exists.

What arrives is **published**, not just stored: the flat snapshot a starting shell reads, and the
aliases another shell sources.

**Scripts travel as records and land as files.** Each side turns its directory into records
stamped against `scripts.ledger`, which remembers what the last sync saw; the records merge by the
same rule, and the winners are written back into `~/.local/sbin` or `.off/`.

| a file, against the ledger | its stamp |
|---|---|
| unchanged | the ledger's |
| changed, or never synced | revision `max(ledger + 1, mtime)`, tie-breaker from its contents |
| gone | the ledger's revision + 1, buried |

A revision is a modification time, so of two edits the newer wins, and an edit since the last sync
beats a deletion of the version before it. Deleting the file is all a removal takes: the next sync
sees it missing and carries the tombstone.

The record format carries three extra fields for this — a revision, a tombstone flag and a
tie-breaker — after the ones a person might read:

```text
2 1754870400 on system,git 3 live 9f1c…
git status --short
```

A record written before those existed is still read, and takes a fresh stamp: it has never been
synced, so revision one is the truth about it.

## Where it lives

| | |
|---|---|
| `crates/oslo-base/src/macros.rs` | the store, the five kinds, the record, `is_a_value` |
| `crates/oslo-base/src/macros/scripts.rs` | the scripts directory: listing, tags, on and off |
| `crates/oslo-base/src/macros/scripts/ledger.rs` | the scripts' stamps, for sync |
| `crates/oslo-base/src/macros/sync.rs` | merging two machines' macros, name by name |
| `crates/oslo-base/src/macros/live.rs` | the two sources, the rebuild, the session list |
| `crates/oslo-runtime/src/startup/stored.rs` | applying it to a shell, and the scripts on `$PATH` |
| `crates/oslo-shell/src/exec/stored.rs` | running a stored function |
| `crates/oslo-ui/src/manager.rs` | the screen: rows, keys, and what it asks the caller to do |
| `crates/oslo-runtime/src/macros.rs` | the screen over the real database, for the key and the CLI |
| `crates/oslo-runtime/src/editor.rs` | handing text to `$EDITOR` and taking it back |
| `src/cli/macros.rs` | `add`, `remove`, `show`, `off`, `on`, `export`, `import` |
| `~/.local/share/oslo/macros/` | `macros.db`, `macros.snapshot`, `elsewhere.snapshot`, `scripts.ledger`, `session/` |
| `~/.local/sbin/` | the scripts, and `.off/` for the ones that are off |

## What it cannot do

- **`[elsewhere]` cannot show a function.** There is nothing to enumerate: one is a file on disk or
  a name in a Lua table.
- **No aliases or abbreviations in a non-interactive shell.** `oslo -c` and a script never read
  `init.lua` either. Functions and scripts are found when you call them, so those work anywhere.
- **A configured alias cannot be edited in place** — only copied into the database and edited there.
  The file it came from is yours, and writing to it would be this deciding how your config is
  formatted.
- **No tag editing after the fact, except for a script.** `--tag` on `add` sets them; changing them
  means adding again. A script's tags are a line in its file.
- **No new list widget, and no editor of its own.** The screen is the finder's look; the editor is
  `$VISUAL`/`$EDITOR`, with `nvim` only as a fallback.
- **A stored function never shadows anything on `$PATH`.** A script is on `$PATH`, so its place
  there decides, as for any program.
- **A script off for one session is still highlighted as a command at that prompt.** The highlighter
  knows `$PATH`, not the session list.
