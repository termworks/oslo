# POSIX, where it counts

oslo is meant to be `/bin/sh` on a machine, which means every `postinst`, every `configure` and every
`Makefile` recipe on it runs through this shell. What it offers those scripts is not "we were
careful": it is 432 scripts run under oslo and under bash and compared byte for byte, plus a rule
that every extension oslo added is unreachable from shell written before oslo existed.

<!-- demo:begin -->
[![posix-fidelity demo](https://asciinema.org/a/1262744.svg)](https://asciinema.org/a/1262744)
<!-- demo:end -->

## How it works

Correctness is measured rather than asserted, because the failure mode that matters cannot be seen
any other way. A self-referential test compares the implementation to itself, and the dominant defect
in a shell is a **plausible wrong answer with exit status 0** — output nobody flagged, from a command
that reported success. So the expected output comes from bash, the specification every `/bin/sh`
script on the machine was actually written against.

```
tests/corpus/*.sh — 432 scripts, each declaring its oracle on line 1
        │
        ├─ "# mode: posix"  (320) ──► argv  --posix -c  ─┐
        └─ "# mode: bash"   ( 99) ──► argv  -c          ─┤ the SAME argv to both shells
                                                         │
        ┌────────────────────────────────────────────────┴──────────┐
        ▼                                                           ▼
  oslo <argv> <script>                                      bash <argv> <script>
  private scratch dir · stdin /dev/null · LC_ALL=C · no $ENV · 10 s wall clock
        │                                                           │
        └───────────────────────► compare ◄─────────────────────────┘
                                     ├── stdout, byte for byte (scratch path → <TMP>)
                                     ├── exit status, exactly (signal = 128 + signo)
                                     └── stderr: empty vs non-empty, and nothing more
                                     │
                        ┌────────────┴────────────┐
                    matches                    differs
             in EXPECTED_FAIL? ──yes──► FAIL   in EXPECTED_FAIL? ──no──► FAIL
                        └──no──► pass                      └──yes──► still failing
```

The two-way ratchet is the part worth copying. A case that diverges and is not listed fails the
suite, so no new divergence lands unnoticed; a case that is listed and *passes* also fails the suite,
so closing a bug means deleting its line and a stale entry cannot survive. `KNOWN_DIVERGENT` is a
separate list for cases where bash is not a valid oracle at all — "we are wrong" and "the comparison
is meaningless" are different claims, so they are different lists. It is empty today.

Three details were each forced by a bug. The mode flag goes to **both** shells, from one function,
because until Round 11 oslo was given a bare `-c` while bash got `--posix`, and all 304 POSIX cases
were judged against an oracle oslo was never in. A separate test asserts that `--posix` changes what
oslo does, because a flag parsed and then dropped would look identical from the harness. And bash is
a moving specification: five cases carry `# needs-bash: 5.3`, are skipped when the runner's bash is
older, and the skip count is printed on success as well as failure, so an ageing CI image cannot
quietly stop testing things.

### Every extension is behind a name oslo invented, or behind a prompt

The corpus proves oslo answers the same as bash. The second half of the guarantee is that nothing
oslo *added* can be named by a script that predates it, and it is structural rather than remembered.

```
a command word
   │
   ├─ can it carry structure?  an edge is rows only when the producer declares it gives
   │     rows AND the consumer declares it takes them AND both run in this process AND
   │     neither is redirected. The only names that ACCEPT rows are
   │            where  cols  get  sort-by  first  last  length  each  to
   │     so the right-hand end of every structured edge is a name oslo invented; df, ps
   │     and ls produce rows but accept none, and can never be that end.
   │        └─ no such name in the pipeline ─► Sink::Text on every edge, the byte path
   │
   └─ is this shell interactive?  (env.interactive(): the -i flag, or a real session)
         no  ─► =command, @name, \cmd, \\cmd, autocd, ! history expansion, rm's trash
                and its directory-without-`-r`, cd's frecency jump, suggestions,
                "did you mean lsblk?", the tracking store: none of them exist
         yes ─► all of it
```

That argument is worth only as much as its enforcement. With `OSLO_AUDIT_STRUCTURED=1` the shell
registers an `atexit` handler that writes `oslo-audit: structured-edges=<n>` to stderr as the process
ends, and `tests/posix_stays_on_the_byte_path.rs` runs every corpus script and requires that number
to be zero. A script that `exec`s is exempt — it replaced the process image, so there is no oslo left
to report — and anything else that fails to report is a hole in the measurement, not a pass.

`cd` is the clearest of the interactive gates, because it is not really a gate at all: its frecency
jump cannot run without the tracking store, and only the interactive loop installs one, so `cd
nonexistent` in a script gets the same diagnostic and the same status 1 it always got — not because
a flag was checked, but because there is nothing to consult.

### `\command`, and the half of it that is a script

`rm` is a builtin here, and a shell whose `rm` moves things to `/tmp` needs a short way to ask for
the one that does not. `command rm` is not it: `command` bypasses *functions*, and the builtin still
wins. So oslo reads a leading backslash on the command word:

| written | alias | function | builtin | runs |
|---|---|---|---|---|
| `rm` | expanded | used | used | oslo's |
| `\rm` | skipped | skipped | skipped | the `rm` on `$PATH` |
| `\\rm` | expanded | used | skipped | the alias's target, unbuiltin |

**In a script a leading backslash means what POSIX says and nothing more**: it suppresses the alias,
and ordinary command search then finds the function or the builtin as it always has. Changing that
would silently reinterpret every `\ls` and `\echo` already written on the machine, with no error to
notice. `\\cmd` in a script is a command whose name begins with a backslash and is not found, which
is what bash answers too — which is why giving it a meaning at a prompt breaks nothing.

Quoting is not escaping: `"rm"` and `'rm'` run the builtin here as in bash, dash and zsh, because
`"$cmd" "$@"` dispatch tables depend on it. The lexer eats the backslash before the word is ever a
string, so the gate reads the word's *shape* — `WordPart::Escaped` is a separate variant from
`Literal` precisely so escaping stays visible after the character is gone.

### What the language actually has

What the corpus exercises, by category: 94 builtin cases, 71 expansion, 43 shell options, 38
redirections, 35 control flow, 25 arithmetic, 17 exit status, 15 traps, 15 quoting, 13 job control,
12 syntax, 8 array, 8 robustness, 4 signals, 4 `[[ ]]` conditionals.

| construct | notes |
|---|---|
| redirections | `<` `>` `>>` `<>` `<&` `>&` `>\|`, bash's `&>`, `noclobber`, fd swap, `exec` |
| heredocs | `<<` and `<<-`, quoted delimiter suppresses expansion |
| here-strings | `<<< word` — the expanded word plus a newline |
| parameter expansion | `${v:-d}` `${v:=d}` `${v:?m}` `${v:+a}` `${#v}` `${v#p}` `${v%p}` `${v/p/r}` `${v:o:l}` `${v^^}` `${!v}` |
| arithmetic | `$(( ))`, `(( ))`, `for ((i=0;i<n;i++))`, the C operator ladder including `**` `<<=` `?:` `,` |
| control flow | `if` `while` `until` `for` `case`, functions, subshells, groups, `time` |
| traps | signals at command boundaries, `EXIT` on every exit path, `DEBUG`, `trap` listing and restore |
| job control | real process groups and `tcsetpgrp`; `set -m` turns it on in a script that has a terminal |
| options | `errexit` with its exemptions, `nounset`, `pipefail`, `noglob`, `xtrace`, `noexec`, `allexport` |
| globbing | `**` as bash 5.3, `nullglob` `failglob` `dotglob` `nocaseglob` `nocasematch`, `GLOBIGNORE`, `compgen -G`, locale order, non-UTF-8 names, `extglob` — see [globbing.md](globbing.md) |

`--posix` is a real mode rather than a label. What it changes, each verified against
`bash --posix`: a special builtin's *utility* error ends a non-interactive shell (`export BAD-NAME=1`
is fatal, `shift 5` is not — the rule is utility error, never non-zero status); a redirection failure
on a special builtin is fatal the same way; a variable assignment error is fatal with no builtin
involved; special builtins outrank shell functions in command search; `$?` beside a command
substitution in the same word reports the previous *command* rather than the substitution, which is
what bash 5.3 changed to; and `trap` lists `INT` rather than `SIGINT`. Interactive shells are exempt
from the fatal rules, as POSIX itself says and bash agrees — a typo at a prompt must not log you out.

### Ctrl-C, and the shell that never saw it

A key at the terminal is a signal to the **foreground process group**, and while a command runs that
group is the command's, not the shell's — that is what makes Ctrl-C reach `sleep` instead of killing
your session. The consequence is easy to miss: a shell waiting on a child is *not told* that a key
was pressed. Its only evidence is the wait status it reaps afterwards.

Reading that status is what makes three things work, and all three were checked against `bash` and
`dash` on a real pty rather than reasoned about:

```sh
while true; do sleep 0.2; done   # ^C ends the loop, rather than killing one sleep of many
sleep 5; echo hi                 # ^C abandons the rest of the line; `hi` is not printed
echo $?                          # 130, which is 128 + SIGINT
```

**oslo used to do none of them.** The interrupt was polled at every command boundary and the poll had
nothing to find, so a loop went round again with the key thrown away — `^C ^C ^C` and no way out
short of closing the terminal. A single command hid it, because there the child dying *is* the end of
the command; and `while true; do :; done` hid it too, because with no child the shell is the
foreground group and gets the signal itself. It took a body that forks for both halves to fail
together.

A child killed by SIGQUIT is treated the same way, for the same reason.

Reading the wait status is enough for a job that *dies*, and no use at all for one that does not —
`sh -c 'trap "" INT; sleep 300'` takes the terminal and keeps it. Opting into
`oslo.misc.interrupt_escape` puts a watcher inside the job's process group so a repeated Ctrl-C has
somewhere to land; see [the job that will not take a Ctrl-C](interrupt-escape.md). Off by default,
and a shell that has not asked for it behaves exactly as this section describes.

## What makes it different

**`sh` is a personality, not a path.** Invoked as `sh`, oslo enters POSIX mode; invoked as `oslo` it
does not, from the same binary. bash does exactly this, and it was checked against the
real thing rather than taken from the manual: `ln -s bash sh; ./sh -c 'echo $SHELLOPTS'` lists
`posix` where `bash -c` on the identical binary does not. A leading `-` is stripped first, so `su -`
and a display manager's `-sh` get it too. It is the only way a system-wide default can hold — a
distribution that points `/bin/sh` at oslo gets a POSIX shell for every script on it, with no flag
anybody has to remember.

bash, zsh and dash all give a leading backslash on a command word one job — suppress the alias — and
leave no way to say "not the builtin" short of naming a path. oslo adds one and confines it to a
prompt, a shape the other shells have no need for because none of them shadows `rm`.

In bash, zsh and fish a pipe carries bytes and only bytes. oslo keeps that for every name those
shells know and offers the other thing only through names they do not have, so there is no new pipe
operator to learn and none to be a hazard in its own right.

## Configuration

```sh
oslo --posix -c 'readonly r=1; r=2'     # POSIX mode from the command line
ln -s "$(command -v oslo)" /usr/bin/sh  # …or from argv[0], for the whole machine
oslo -c 'set -o posix; …'               # …or mid-script, if line 1 is soon enough
oslo -i -c '…'                          # force the interactive reading without a terminal
OSLO_AUDIT_STRUCTURED=1 oslo script.sh  # stderr: oslo-audit: structured-edges=0
OSLO_ALLHIST=1 oslo -c 'make'           # the one non-interactive shell that records anything
rm -s build                             # POSIX rm at a prompt; -s is --strict
```

```lua
oslo.builtin.rm.to_tmp = true   -- prompt only; a script's rm never moves anything
```

`--posix` is deliberately absent from `oslo --help`; it still works, and `--help --details` lists
`posix` among the shell options. It is a long flag rather than a `set` letter because POSIX mode
cannot be reached any other way *before the first command runs* — `set -o posix` on line 1 of a
script is already too late to have decided how that line's command word was searched for.

`$OSLO_ALLHIST` is an environment variable rather than a Lua setting for a reason that only matters
once oslo is `/bin/sh`: `-c` does not read `init.lua`, so a setting would mean starting an
interpreter and running your config on every `system()` call on the machine. `0`, `false`, `no` and
`off` mean off, rather than merely being non-empty.

## Measurements

`target/release/oslo` at 0.4.2 against bash 5.3.9, every corpus script in its own scratch
directory, comparing stdout and exit status; then the whole corpus again under
`OSLO_AUDIT_STRUCTURED=1`:

| | |
|---|---:|
| corpus scripts | 432 |
| `# mode: posix` / `# mode: bash` | 320 / 99 |
| matching bash | 417 |
| differing | 2 |
| reported a structured-edge count | 418 |
| **reported a non-zero one** | **0** |
| the differential suite, end to end | 2.6 s |
| the byte-path audit over the corpus | 3.8 s |

The two that differ are exactly the two rows in `tests/differential/expected_fail.rs`:
`syntax_unsupported_coproc.sh` and `syntax_unsupported_select.sh`, both refused by name rather than
guessed at. There was a third — `arith_for_unspaced_sections.sh` — and it is gone because the gap
closed; see [known gaps](../known-gaps.md).

The one that did not report is `builtin_exec_replaces_shell.sh` — the exemption the test already
carries, because it replaced the process image and nothing registered at exit could run.

## What it cannot do

- **Three corpus cases still differ from bash.** `for ((;;))` with no space between the section
  separators is a syntax error, because brush's tokenizer takes the longest match and fuses the two
  `;` into the `;;` that terminates a `case` item; `for (( ; ; ))` and `for ((i=0;i<3;i++))` both
  work. `coproc` and `select` are refused by name and deliberately not implemented — one needs job
  control, the other a prompt, `PS3` and `REPLY`.
- **Four `declare` attributes are refused rather than honoured**: `-A` (associative arrays), `-i`,
  `-l`, `-u` and `-n`. Each exits 2 and says which one, because this shell has no representation for
  them and the alternative is worse than the gap. `declare -A m` answered with an *indexed* array
  would put every key on element 0 — the subscript is arithmetic, so `m[alpha]` and `m[beta]` are
  the same slot — and the last write would silently win with nothing on screen looking wrong.
  `-a`, `-r`, `-x`, `-g` and `-p` all work. Indexed arrays are complete.
- **stderr is compared only for emptiness.** Two shells will never agree on diagnostic wording and
  should not be forced to, so a diagnostic that says the wrong thing while being non-empty is
  invisible to the suite.
- **The oracle is bash, not the standard.** Where bash itself departs from POSIX, oslo follows bash
  and the corpus calls it a pass — the deliberate trade, because the scripts on a real machine were
  written against bash.
- **Under `--posix`, a function named after a special builtin is defined and then never reached.**
  bash refuses the definition outright with `is a special builtin`. The net effect agrees — the
  function does not shadow — but the error is not.
- **The corpus can only catch what somebody wrote a case for.** 432 scripts is not the language.
  Three of the divergences it now covers were found by running every `#!/bin/sh` script on a Debian
  system under both oslo and dash, not by anybody enumerating them.
- **A script cannot opt in to the interactive extras.** There is no flag; `-i` is the only switch,
  and it says the shell *is* interactive, which changes far more than one convenience.
- **Job control in a script needs both `set -m` and a controlling terminal.** With no terminal to
  claim it stays off, which is not an error — `set -m` inside a pipeline is legal and can do nothing.
- **A key is only noticed when the child dies of it.** A program that catches SIGINT and keeps
  running keeps running, and the shell waits — which is what a shell is supposed to do, and is why
  a second Ctrl-C reaches the program rather than the loop around it.
- **A running `while read` can still hang.** The harness treats a wall-clock timeout as a first-class
  verdict for exactly that reason: a hang is always a defect, never an accepted difference.

## Where it lives

| path | what |
|---|---|
| `tests/corpus/` | the 432 scripts, mode declared on line 1 |
| `tests/differential_tests.rs` | `compare`, `mode_args`, `execute`, `oracle_version` |
| `tests/differential/expected_fail.rs` | `EXPECTED_FAIL`, `KNOWN_DIVERGENT` — the ratchet |
| `tests/posix_stays_on_the_byte_path.rs` | the zero-structured-edges assertion |
| `tests/command_escape_tests.rs` | `\cmd` and `\\cmd`, run twice: prompt and script |
| `crates/oslo-shell/src/exec/simple/posix.rs` | `exits_on_error`, `resolve_builtin_result`, `assignment_failure` |
| `crates/oslo-shell/src/env/builtins/declare.rs` | `Attributes` — which letters are honoured, and which are refused |
| `tests/declare_builtin_tests.rs` | the refusals through the real binary: status, diagnostic, nothing left behind |
| `crates/oslo-shell/src/exec/simple/external.rs` | `wait_for_child` — where a key the shell never saw is read off the wait status |
| `crates/oslo-shell/src/exec/pipeline/interrupt.rs` | turning that into an unwind, and back into 130 at the top |
| `tests/signal_tests.rs` | the pty tests: a forking loop ends, and the rest of the line does not run |
| `crates/oslo-shell/src/exec/simple/escape.rs` | `Escape`, `intent` — the backslash gate |
| `crates/oslo-shell/src/expand/sugar.rs` | `=command` and `@name`, interactive-only |
| `crates/oslo-shell/src/exec/simple/autocd.rs` | `enabled` — interactive *and* opted in |
| `crates/oslo-shell/src/env/builtins/remove.rs` | `mode_for` — `rm`'s five-line safety argument |
| `crates/oslo-shell/src/env/builtins/directories/jump.rs` | `jump` — no store, no cleverness |
| `crates/oslo-shell/src/data/plan.rs` | `plan`, `STRUCTURED_EDGES`, `entered_structured_path` |
| `crates/oslo-base/src/track/mod.rs` | `install`, `store` — two callers, and no more |
| `src/cli.rs` | `named_sh`, `LONG_FLAGS` |
| `src/main.rs` | `report_structured_audit` |
