# Known gaps

What oslo does not do, why, and what to write instead. Every entry here was re-run against the
current binary before it was written down — a gap that has quietly closed is worse than a gap,
because it sends people around a detour that is no longer there.

Two did close, and are recorded at the bottom.

## The rule these follow

**A gap says so.** Every one of these fails loudly — a syntax error naming the construct, or a
builtin refusing the option — rather than accepting the line and doing something else with it. A
shell that silently did *nearly* what you asked would be the worse outcome, because the wrong
answer arrives looking like the right one.

---

## `coproc`

```console
$ oslo -c 'coproc cat'
oslo: syntax error: coproc is not supported yet
```

A coprocess is a two-way pipe to a background command plus an array holding its descriptors. What
it is *for* — talking to a long-running program without a temporary file — oslo does with named
pipes, which every shell has:

```sh
mkfifo /tmp/in /tmp/out
cat /tmp/in > /tmp/out &
echo hello > /tmp/in &
read reply < /tmp/out
```

## `select`

```console
$ oslo -c 'select x in a b; do echo $x; done'
oslo: syntax error: select is not supported yet
```

`select` is a menu loop: print a numbered list, read a number, run the body with the variable set.
It is bash's, not POSIX's. In oslo the menu belongs to the interface layer, where it can be drawn
properly and arrowed through:

```sh
x=$(oslo userin choose a b c)
```

In a script that must stay portable, the loop is six lines of POSIX:

```sh
i=1; for opt in a b; do echo "$i) $opt"; i=$((i+1)); done
read -r n
```

## Associative arrays

```console
$ oslo -c 'declare -A m'
oslo: declare: -A: associative arrays are not supported
```

Indexed arrays work. A subscript that is not a number is **arithmetic**, exactly as in bash, so
`m[k]=v` writes index 0 and `m[j]=w` overwrites it — bash prints the same `k=w j=w count=1` for
that line, and oslo matching it is deliberate rather than accidental.

For a real map, `oslo.db` is a table a config owns, and a Lua table is a map:

```lua
local m = {}
m.k, m.j = "v", "w"
```

## Process substitution without `/dev/fd`

`cat <(echo hi)` works, and works by handing the reader a `/dev/fd/N` path. On a system without
`/dev/fd` — a container built without it, or a chroot missing `/proc` — there is no filename to
hand over and the construct cannot be made to work at all. Nothing to work around: a pipe or a
temporary file is the portable spelling, and it is what POSIX offers.

---

## An exit request caught by `pcall` still ends the shell, but not where it was written

`os.exit` and `oslo.proc.exit` unwind as an ordinary Lua error, so that a script can read the
message the same way it reads any other. That makes them catchable:

```lua
local ok, err = pcall(function() os.exit(7) end)
print("still here", ok, err)   -- prints; the shell then leaves with 7
```

The status is honoured — the shell exits 7 rather than swallowing it — but the rest of the
enclosing call runs first, where Lua's own `os.exit` would never have returned. Making it truly
uncatchable needs an unwind the VM will not let `pcall` intercept, which luna does not offer.

**One consequence is worth knowing.** Inside the call where an exit was caught, a *later*
unrelated failure is reported as that exit rather than as itself:

```lua
pcall(function() os.exit(7) end)
error("this message is not printed")   -- the shell leaves with 7, silently
```

Do not catch an exit you did not mean to catch. A `pcall` around code that may exit should
re-raise: `if not ok then error(err, 0) end`.

---

## The history database only grows

Every command adds a row to the sync event log, and nothing removes one. `oslo history prune`
bounds the *ranking* table — the rows behind suggestions — and does not touch the log; `oslo
history clear` marks events deleted rather than removing them, because a deletion has to be a
tombstone that other machines can see when they sync.

Measured against this build:

```console
$ oslo history import 40k-lines.txt
added=40000
$ oslo history status | grep size
size    42074112
$ oslo history clear --yes && oslo history prune --yes
deleted 40000
removed-runs    0
$ oslo history status | grep -E 'size|visible'
size    42074112
visible 0
```

About a kilobyte per command, and a database holding nothing visible still costs what it did
before. `oslo history backup` copies rather than compacts, so it does not reclaim anything either.

**What to do about it today**: `oslo history export` the events you want, delete the file, and
`oslo history import` them into a fresh one. Nothing else shrinks it.

Retiring the log automatically needs two decisions this build has not made — how long a tombstone
must be kept before dropping it is safe (drop one too early and syncing with a machine that has
been away longer brings the deleted line *back*), and a way to compact the file, which the storage
layer has no operation for.

---

## A `SIGSEGV` or `SIGBUS` *sent* to oslo does not kill it

Every other signal behaves: `SIGKILL` answers 137, `SIGABRT` 134, `SIGILL` 132, `SIGFPE` 136, and a
non-oslo child that dies of `SIGSEGV` answers 139, all matching bash and dash. Two do not.

```console
$ bash -c 'kill -SEGV $$; echo alive'; echo $?
139
$ oslo -c 'kill -SEGV $$; echo alive'; echo $?
alive
0
$ oslo -c '( kill -SEGV $$ ); exit $?'; echo $?      # bash and dash: 139
0
```

`SIGBUS` is the same; a subshell is affected because it inherits the disposition across `fork`.

The cause is the Rust runtime, not oslo: `std` installs a `SIGSEGV`/`SIGBUS` handler to recognise a
stack overflow, and that is what prints `fatal runtime error: stack overflow` — the diagnostic the
gap above depends on. When the faulting address is *not* in a guard page the handler returns, which
is right for a real fault (the instruction re-runs and dies) and wrong for a signal that was sent,
where there is no faulting instruction to re-run, so the process simply carries on.

**What to do about it today**: nothing, and the reason is the trade. Restoring the default
disposition would make these two faithful and would take the stack-overflow message with them —
losing the only thing that currently says what happened when the nesting limits are exceeded
together. Keeping both means chaining a handler of oslo's own behind `std`'s, which is a signal
handler running after a memory fault: the least forgiving code in the shell, written to fix the
exit status of a signal nobody sends on purpose.

---

## Closed since this list was first written

| Was | Now |
|---|---|
| `for ((;;))` with touching separators | every spelling runs, empty sections and all |
| `( ( cmd ) )` read as an arithmetic command | only *adjacent* parens open one; spaced parens are nested subshells |
| A structured tool at the head of a pipeline | `printf 'a\nb\n' \| oslo -c 'lines \| length'` answers 2, not 0 |
| Process substitution generally | works wherever `/dev/fd` exists, which is every ordinary Linux system |
| Three nesting limits sharing one stack, each measured alone | the stack is asked directly, so a combined workload gets an error rather than an abort |
| `extglob` — `@(a\|b)`, `!(*.txt)` and the rest | the parser reads a group as part of its word, and `shopt -s extglob` turns matching on; see [globbing.md](features/globbing.md#extended-patterns) |

The nesting one is worth a word too, because the counters looked adequate and were not. A function
calling itself, a `source`/`eval` chain and a nested compound command each had a limit measured
while the other two were idle; together they are not idle. A `source` chain 49 deep, calling a
function recursing 20 deep inside 45 levels of `{ ( … ) }`, overflowed the 16 MiB interpreter stack
and aborted — while the counter still said ninety-odd levels were free. No setting of the three
constants closes that, because how much stack a level costs depends on the *shape* of what is
nested rather than on how many levels there are.

`oslo_base::stack` asks the stack instead: the interpreter thread records where its stack starts,
and the two places that re-enter the evaluator — `DepthGuard::enter` and `eval_command_list` —
refuse when less than 256 KiB of it is left. Twenty-five combined shapes that used to abort now
answer `maximum nesting level exceeded`, and a plain recursion still reaches the depth the counter
permits. A thread that never recorded a base measures nothing and refuses nothing, so the guard can
only ever fire where it knows the answer.

The first two share a shape, which is why it is worth a word: both were the tokenizer's longest
match disagreeing with the grammar. `( (` and `((` produce the same two `(` tokens, so the
arithmetic rule — tried first — matched a subshell that happened to open with another subshell, and
`( ( echo hi ) )` died evaluating `echo hi` as an expression. The fix reads the source positions the
tokens already carry and requires the two parens to touch, so a spaced pair backtracks into the
subshell rule. Guarding in the grammar rather than the tokenizer leaves `$((`, `for ((` and every
other spelling on the path they already take.

The `for` case is the same story told earlier. The
tokenizer takes the longest match, so `for ((;;))` carries **one** `;;` operator — the token that
ends a `case` item — where the grammar reads two separators. `for (( ; ; ))` parsed, which made it
look like a rule about spaces. The fix is two lines in the grammar: an arithmetic section now
*stops* at `;;`, and the condition rule accepts one as an empty condition. Nothing in the tokenizer
changed, because `;;` ends a `case` item far more often than it separates loop sections — and the
corpus case carries a `case` at the bottom to prove that still holds.
