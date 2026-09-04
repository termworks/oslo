# Scratches

Named sessions that keep running when the terminal they were opened in goes away. Start a build,
walk away, close the laptop lid, and pick the same shell up somewhere else with the build still
running in it.

One key reaches all of it, and it means the same thing wherever you press it.

> ## This is in `oslo`, not in `oslo-minimal`
>
> Everything on this page is behind the **`scratch`** cargo feature, which is off by default. A release
> publishes two binaries per architecture and they differ in exactly this:
>
> | | the key | `oslo.scratch` settings |
> |---|---|---|
> | `oslo` | opens the finder | acted on |
> | `oslo-minimal` | unbound — does whatever it otherwise would | **read and ignored** |
>
> ```sh
> scripts/build.sh              # the full binary, every feature
> scripts/build.sh --minimal    # the floor: no scratches, no keeper, no runtime directory
> ```
>
> It costs **44 KB** — 6,275,872 bytes without it against 6,320,928 with — and **no dependency at
> all**: it is `nix`, which oslo already links. Startup is unchanged, because nothing runs until the
> key is pressed: 681 µs ± 133 with it against 707 µs ± 149 without, over 300 runs.
>
> The settings are read in both builds on purpose. A config is shared between machines, and making
> it ask `if oslo.scratch then` before setting a key would be a question with only one useful answer.

<!-- demo:begin -->
[![scratch demo](https://asciinema.org/a/1262816.svg)](https://asciinema.org/a/1262816)
<!-- demo:end -->

## What the key does

`^\` by default. Press it anywhere.

| where | you press | what happens |
|---|---|---|
| outside a scratch | `^\` | the finder: every scratch, plus a row for the name you type |
| outside, in the finder | Esc | nothing — back to your prompt, line intact, no scratch made |
| outside, in the finder | pick one | attach to it |
| outside, in the finder | type a name, Enter | make it, attach |
| **inside** a scratch | `^\` | the same finder, **this scratch listed too** |
| inside, in the finder | Esc | leave — back to the shell you pressed the key in, scratch still running |
| inside, in the finder | pick the same one | straight back in, nothing changed |
| inside, in the finder | pick another | leave this one running, attach to that |
| inside, in the finder | type a name, Enter | make it and attach; the old scratch keeps running |
| either, in the finder | Delete | ask about the highlighted one — Delete again ends it |

**Nothing is ever killed by navigating.** A scratch ends when the shell inside it exits, which is what
`exit` has always meant. Leaving one is not ending it. Delete is the one key here that is not
navigation, and it says so before it acts.

Nothing is auto-named, either: the create row offers only what you typed, so every scratch is called
what somebody meant it to be called. `$SCRATCH` holds the current name, for a prompt to show.

```text
         ┌── pick an existing one ──► attach to it
         ├── type a name ───────────► make it, attach
  ^\ ──► finder ── Delete, twice ───► end it, and stay in the finder
         └── Esc ───────────────────► outside: nothing. inside: leave it running.
```

## Ending one from outside

`exit` inside a scratch is the ordinary way, and it is the only one that needs no explanation. But a
shell that has stopped answering cannot be `exit`ed, and the whole point of a scratch is that
closing the terminal does not touch it — so without a way in from outside, a wedged scratch is
forever.

**Delete in the finder, twice.** The first press arms the row it is standing on, which repaints to
say what the next press does; anything else typed puts it back. The second press ends it and the
finder stays open, relisted, because ending one of several is not a decision about the others. It
works on the scratch you are inside as well as on any other.

```text
    work                        Delete                work — delete again to kill
  > api          ─────────────────────────────────►   api
    docs                                              docs
```

The ask is [the history finder's](history-finder.md) — `oslo.finder.confirm_delete = false` turns
it off there and here, and one press does it.

```sh
scratch -k work        # the same ending, for a name you already know
scratch -k work api    # every one of them, even if the first will not go
```

What actually happens is a hangup to the shell inside, which is the same signal a real terminal
sends when it is closed. The keeper then sees the pty close, sweeps the files and exits — the
identical path an `exit` takes. A shell that ignores the hangup is killed, and a keeper that
outlives its shell is killed after it; each step waits to see whether the one before it worked.

A pid file is usually a lie waiting to happen, because the process it names can be gone and its
number given to somebody else. Not here: the lock proves the keeper is alive, and the keeper never
reaps the shell it forked — so while a scratch is listed, the pids in its `.meta` are still its own.

## The finder is oslo's own list widget

The same one `ui filter` uses, so it carries the theme, the border and the key handling everything
else in oslo has — arrows, `^n`/`^p`, fuzzy matching as you type.

The row offering the name you typed is **a row, not a second prompt**. It appears only when what you have typed
names nothing already in the list, and it sorts last, so Enter still takes a real match by default
and making a new one is the answer you walk to. Offering to create `work` while a scratch called `work`
is right there would make Enter ambiguous at the exact moment it matters.

## How it holds a shell

```text
  you press ^\
       │ fork
       ▼
     keeper ── setsid, holds an flock, owns the pty master, serves a socket, writes the log
       │ fork
       ▼
     oslo ── setsid, TIOCSCTTY on the slave, stdin/stdout/stderr on it
```

Two forks and two sessions, and both matter. The keeper `setsid`s so the terminal it came from is no
longer its controlling terminal — closing that terminal sends it no `SIGHUP`, which is the entire
promise. The shell `setsid`s again and claims the pty, so *it* is a session leader on a terminal of
its own, which is what makes `fg`, `bg`, `^C` and `^Z` inside a scratch ordinary rather than forwarded.

The keeper never interprets a byte. Input arrives from a client and goes to the pty; output comes off
the pty and goes to the client and to a log. Every decision about what a keystroke *means* belongs to
the pty's line discipline and the shell standing on it.

The shell inside is `exec`d rather than forked on, which costs one config read when a scratch is made.
The key is pressed at a prompt, long after oslo has started the threads that warm the `$PATH` index,
and `fork` carries only the calling thread — a child that carried on would hold locks belonging to
threads that do not exist in it.

The same keeper can host an exact program for [`watch services`](watch.md). A generated watch
Scratch is still a normal named Scratch: it appears in the finder and `scratch -l`, its output is
replayed on attach, and `scratch -k NAME` stops the worker and its current command group. Only an
explicitly requested watch service is auto-named; ordinary Scratch creation still requires the name
the user typed.

Program-backed launch goes through a private one-use marker and execs before normal Oslo threads
start. Non-standard inherited descriptors are closed before the keeper fork, so a persistent watch
cannot keep the pipes of the Lua script or shell that launched it open.

## Where it lives

`/tmp/oslo-$UID/scratch`, mode `0700`, or wherever `oslo.scratch.dir` says.

`/tmp` rather than `$XDG_RUNTIME_DIR` because systemd deletes the latter at last logout unless
lingering is enabled, and a scratch dying at logout defeats the point. tmux made the same trade for the
same reason and has lived at `/tmp/tmux-$UID` for twenty years.

**`/tmp` is `1777`, so anybody can create `/tmp/oslo-1000` first and wait.** What would land in it is
a socket carrying a terminal's input and output. So the directory is proven before use — a real
directory, our uid, mode `0700`, not a symlink — and proven on the *descriptor*, not the path, so
nothing can be swapped underneath between the check and the open. A directory that fails is refused
and said so at the prompt:

```
oslo: scratch: /tmp/oslo-1000/scratch is mode 775, and must be 700 — refusing to use it
```

Each scratch is five files: a socket, two locks, its metadata and its output log. Liveness is the
first lock — asked by trying to take it — so a scratch whose keeper was killed is swept by whoever
next lists.

## One terminal at a time

A scratch is one shell on one pty. Two terminals on it would share one window size and one line
being typed into, so the second is refused:

```
oslo: scratch: work is open in another terminal
```

The refusal is the **second lock**, held by the terminal looking at the scratch rather than by the
keeper. It has to be, because the keeper cannot say this: everything it writes is the pty's output,
and a keeper that spoke for itself would be a terminal that lies. So it drops a second client
without a word — and a client that learned of the refusal only from that silence could not tell it
from the shell inside having exited, which is exactly what used to happen: the screen came back and
nothing was said.

Asking the lock first turns the silence into a sentence. And because it is an `flock`, the kernel
drops it when the terminal holding it goes, however it goes — a terminal killed outright leaves the
scratch attachable again, with nothing to clean up.

## Two backends

```lua
oslo.scratch.daemon = false   -- default: a keeper per scratch
oslo.scratch.daemon = true    -- one registry process, as scratch-rs does it
```

Everything on this page is true of both. What changes is who a client talks to:

```text
  daemon = false                        daemon = true

  client ──socket──► keeper             client ──socket──► daemon
                       │ pty                                 │
                       ▼                                     ├─► keeper ─► oslo
                     oslo                                    └─► keeper ─► oslo

  list  = read the directory            list  = ask the daemon
  kill  = signal what .meta names       kill  = ask the daemon to
```

**The keeper is the same process in both.** The daemon is a registry and a splice in front of
machinery that already worked — it reads none of what it copies, because the framing between a
client and a keeper is settled elsewhere and a middleman that understood it would be a second place
to keep in step.

There is no service to install. The first client that needs a daemon forks one, exactly as a client
forks a keeper without one, and a daemon nothing has asked for in ten minutes gives up its socket and
exits. A machine with no scratches has no oslo process running either way.

The daemon uses a unix socket rather than scratch-rs's localhost TCP port and auth token: on Linux the
filesystem already answers the question the token exists to answer, and a port is reachable by every
process on the machine.

## The key, and why it is spelled twice

```lua
oslo.scratch.key = "ctrl-\\"   -- any control chord
```

Whatever this is gets **swallowed from every program running inside a scratch** — that is what a proxy
watching for a key means. It rules out `^X`, which is nano's Exit and emacs' prefix. `^\` is the
default because nothing in common use binds it.

Outside a scratch the shell owns the terminal, so this is an ordinary editor binding. Inside, the client
owns it and the shell is behind a pty, so the client takes the byte out of the stream before
forwarding. Same key, same finder, two mechanisms.

And the byte is not always a byte. The shell inside a scratch asks the terminal for the Kitty keyboard
protocol — `\x1b[>5u`, so its line editor can tell `^I` from Scratch — and that request is *output*, so
it travels out through the client to the real terminal. From then on the chord arrives as
`\x1b[92;5u` rather than as `0x1c`:

```text
  legacy     ^\  ──►  1c
  kitty      ^\  ──►  1b 5b 39 32 3b 35 75        "\x1b[92;5u"
```

Both are matched. A client watching for one byte works right up until the first prompt is drawn and
then never fires again, which is as confusing as it sounds because nothing looks broken.

## Settings

```lua
oslo.scratch.key = "ctrl-\\"       -- opens the finder, in a scratch or out of one
oslo.scratch.daemon = false        -- one keeper per scratch, rather than a registry process
oslo.scratch.dir = ""              -- empty means /tmp/oslo-$UID/scratch
oslo.scratch.log_bytes = 1048576   -- how much output to keep for the replay on attaching
```

A key name that cannot be one byte is reported against the line that wrote it and the default is
kept — the alternative is a scratch with no way out.

## What it does not do

No panes, no splits, no status bar, no layouts, no scrollback of its own. A scratch is one shell that
outlives its terminal, and that is the whole idea. Anything that wants two things on one screen
wants a multiplexer, and there are good ones.
