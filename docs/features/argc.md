# Arguments, declared in comments

A script says what it takes in comments, and the shell parses it:

```sh
#!/usr/bin/env oslo
# @describe Deploy a thing
# @flag     -n --dry-run     say what would happen
# @option   -t --tries <N>   how many times
# @arg      target!          where to
argc "$@"

echo "deploying $argc_target, $argc_tries tries, dry=$argc_dry_run"
```

```console
$ deploy --help
Deploy a thing

USAGE: deploy [OPTIONS] <TARGET>

ARGS:
  <TARGET>  where to

OPTIONS:
  -n, --dry-run    say what would happen
  -t, --tries <N>  how many times
  -h, --help       Print help
```

The declaration language is [argc](https://github.com/sigoden/argc)'s, vendored in `vendor/argc`.
Everything already written against it keeps working; what oslo adds is a shell that speaks it
natively rather than a program you have to install beside one.

**Behind the `argc` cargo feature**, which a release build has and `oslo-minimal` does not.

**[`oslo fmt`](formatting.md#argc-declarations-lined-up) lines the block up** — the columns above are
what it produces, and it produces them from whatever spacing you typed. Padding cannot change what
argc parses, because `parse_tail` discards the whitespace between two tokens and trims every
description; the formatter splits on whitespace and joins with whitespace and never has to know what
a field means.

<!-- demo:begin -->
[![argc demo](https://asciinema.org/a/1262961.svg)](https://asciinema.org/a/1262961)
<!-- demo:end -->

## Three doors, one parser

```text
bash script   →  oslo --argc-eval  →  shell code  →  eval  →  variables
oslo script   →  argc "$@"                              →  variables
Lua           →  oslo.args.parse(spec, argv)            →  a table
```

### `oslo --argc-eval`, for a script that is not oslo's

[![argc-eval demo](https://asciinema.org/a/1262962.svg)](https://asciinema.org/a/1262962)


```sh
#!/usr/bin/env bash
# @option -t --tries <NUM>
eval "$(oslo --argc-eval "$0" "$@")"
```

bash cannot be handed a parse; it can only be handed text to run. So this prints exactly what the
`argc` binary prints — assignments, arrays, the call to the chosen function — and the `eval` does the
rest. **oslo is a drop-in for `argc`**: a script written against it works unchanged, with one fewer
program on the machine.

### The `argc` builtin, for a script oslo runs

`argc "$@"` is the same parse with the middle removed. No program to find, no fork, no pipe, no
quoting round trip, no `eval` of text that was a data structure a moment earlier — the builtin
applies the values to the environment directly.

What that buys beyond the fork:

- **Errors are the shell's.** A bad flag is reported by the shell that read it.
- **`--help` ends the script.** The bash rendering finishes with `exit 0`; the builtin raises the
  shell's own exit, so the body does not run with nothing set. That is a real bug this had, caught
  by a test that is still there.
- **It works for code with no file.** See below.

## Code with no file

A stored [macro](macros.md) function runs from the database, and its `$0` is its own name rather
than a path. So the bash idiom cannot work there:

```sh
eval "$(oslo --argc-eval "$0" "$@")"    # $0 is `deploy`, which is not a path to read
```

The builtin needs no path: the shell already holds the source. A stored *script* is a file in
`~/.local/sbin` and needs none of this — its `$0` is its path, and either spelling works:

```sh
oslo macros add --script deploy      # ~/.local/sbin/deploy, in $EDITOR, with @option comments
deploy --help                        # the help its own comments declare
deploy --<Tab>                       # completed from those comments
```

## Completion nobody has to install

`argc` ships generated completion scripts for nine shells. oslo generates nothing:
`crates/oslo-shell/src/argc/complete.rs` is a completion provider like any other, so
`deploy --<Tab>` reads the script's comments at the moment you press Tab.

It costs nothing on a machine with no argc-shaped scripts: the command word is checked first, then
the source is read, and the parser runs only once the source is seen to contain a `# @` at all.

`deploy --env <Tab>` offers the values the option declares — `dev staging prod` — and they are
**scored above a filename**: the provider carries a `score_offset` because a row it offers exists
only because the script declared it, and losing to whatever happens to be in the current directory
is how `--env <Tab>` came to complete `src/`. Found by recording it, not by reading it.

The provider is named `argc` and badged `argc`, so `oslo.completion.sh_sources` can filter it and a
config that wants to replace it declares a provider of the same name.

### `oslo.args`, for a config

The same declaration, from Lua — so a registered builtin, a `.env.lua` and a `.make.lua` describe
their arguments the way a script already does, rather than each inventing a table format.

```lua
local SPEC = [[
# @describe Put a build somewhere
# @option   -t --tries <NUM>   how many times to retry
# @flag     -n --dry-run       say what would happen
# @arg      target!            where to
]]

oslo.register_builtin{ name = "deploy", run = function(argv, shell)
  local got, why, status = oslo.args.parse(SPEC, argv)
  if not got then io.write(why, "\n") return status end
  --> got.target == "prod", got.tries == "3", got.dry_run set only when given
end }

print(oslo.args.usage(SPEC, "deploy"))   -- the rendered help, without parsing
oslo.args.check(SPEC)                    -- true, or nil + what is wrong with the declaration
```

| answer | means |
|---|---|
| a table | it parsed; a dash in a name is an underscore, so `--dry-run` is `got.dry_run` |
| `nil, text, 0` | `--help` was asked for; `text` is the page |
| `nil, text, 1` | a usage mistake; `text` says which |

**Nothing here touches the shell**, which is the point: a builtin and a completion provider both run
while the shell holds its own state, and every call that borrows it raises there. The parse uses a
detached runtime — so it works in those places, and the one thing it gives up is a default computed
by a shell function, which has no evaluator to run in and comes back empty.

`argv` arrives with the builtin's own name at `argv[1]`, which is exactly what argc wants at
`words[0]` — so a builtin passes what it was handed, unchanged.

Present only in a build with the `argc` feature. A config asks the documented way: `if oslo.args
then`.

## A default computed by a function

A declaration can name a function instead of a literal:

```sh
# @option --dir=`_default_dir`     the default is whatever this prints
# @arg    host[`_choice_host`]     the choices are whatever this prints, one per line
```

Upstream answers those by exec'ing `bash script.sh ___internal___ _default_dir …` and reading its
stdout. **oslo runs them in itself**, in a command substitution — one fork instead of a bash to find
and start, no bash needed on the machine at all, and it works for a script that has no file. The
subshell means a helper that `cd`s or exports changes nothing in the shell that asked, which is the
same guarantee the separate process gave.

## What it costs

Measured on the real binary, built both ways at the release profile:

```text
every feature but this one   6,160,768 bytes
with it                      6,476,160 bytes    +315,392   (308 KB)
```

An isolated test binary had predicted 439 KB; fat LTO shares the difference with crates oslo already
links. It is still the largest optional feature in the tree — larger than `direnv` and `plugin`
together — and it brings five crates oslo does not otherwise have, `nom` among them: a second
parser-combinator library in a tree whose argument is that it owns its parsers. It parses *comments*,
which is where that line was drawn.

**`vendor/argc` is built at `opt-level = 3`** while the rest of the release profile is `"s"`. The
parse runs once per invocation of a script that uses it, which is the shape that pays for speed;
saving bytes there would be saving them on a feature whose cost is already paid the moment it is
switched on.

## What it cannot do

- **Nothing in `oslo-minimal`.** The word `argc` falls through to `$PATH`, so the real one still
  works if it is installed.
- **`--argc-eval` needs a real path.** `$0` in a stored function is a name; the builtin is the answer
  there.
- **No `--argc-build`, `--argc-mangen`, `--argc-completions` or `--argc-export` yet.** They are the
  same shape as `--argc-eval` and can follow one at a time; `--argc-parallel` needs a thread pool
  oslo has no other use for and is not planned.
- **No `Argcfile.sh` runner.** `argc` with no arguments finds an `Argcfile.sh` and runs a task from
  it. That is a task runner rather than an argument parser, and oslo has not decided it wants one.

## Where it lives

| | |
|---|---|
| `vendor/argc` | the parser, at 1.24.0, MIT OR Apache-2.0 |
| `crates/oslo-shell/src/argc.rs` | the builtin: applying a parse to the environment |
| `crates/oslo-shell/src/argc/runtime.rs` | `argc::Runtime` over this shell |
| `crates/oslo-shell/src/argc/call.rs` | running a script's own helper functions |
| `crates/oslo-shell/src/argc/complete.rs` | the completion provider |
| `src/cli/argc.rs` | `oslo --argc-eval` |
| `crates/oslo-runtime/src/lua/api/args.rs` | `oslo.args` — the Lua binding |
