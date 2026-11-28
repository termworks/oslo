# Stream coordinates

A pipeline stage can address what the stage before it printed, by position:

```sh
cat hosts.txt | ssh {0:0} uptime
                     └─ line 0, word 0 — of the text `cat` just produced
```

This is the job `xargs` exists for, without `xargs`: no `-I{}`, no second quoting layer, no separate
process reading a format string. The same grammar reaches back through the pipeline, back through
the session, and at what each stage *was* as well as what it printed.

Coordinates are prompt syntax: they expand in an interactive shell (`oslo -i -c` included). A script
or a plain `oslo -c` reads `{0:0}`, `{1..}` and `x{..3}y` as bash does, as literal words.

<!-- demo:begin -->
[![stream-coordinates demo](https://asciinema.org/a/1263429.svg)](https://asciinema.org/a/1263429)
<!-- demo:end -->

## How it works

A **stream** is text something produced. Two kinds go on one stack, and the sign of the first
dimension says which:

```
  cat hosts.txt | grep web | ssh {0:0}
                       │           └─ 0   this stage's input: what `grep web` printed
                       └───────────── 1   one stage further back: what `cat` printed

  ssh {-1:0:0}   ← -1  the previous prompt, whatever it was
```

**Zero and up walk back through this pipeline; below zero walks back through the session.** Giving
them one axis would mean `{3:…}` silently crossing out of a short pipeline into the prompts behind
it, which is a different question than the one that was asked.

### How many dimensions you write says what you mean

Nothing is marked. Read right to left, the last dimension is always the word, the one before it the
line, and the one before that the stream:

| written | means |
|---|---|
| `{2}` | line 2 |
| `{0:1}` | line 0, word 1 |
| `{1:0:1}` | one stream back, line 0, word 1 |

There is no marker character because there is nowhere to put one. `|` and `;` are lexer
metacharacters, so a word is split before any expansion runs — `{3\|0:4}` does not reach the
substitution at all, it makes the shell try to *execute* `0:4}`. Braces do not protect a
metacharacter.

### Ranges are `..`, and they include both ends

Verified against the three-line fixture `web-01 / web-02 / db-01`:

| written | gives |
|---|---|
| `{0}` | `web-01  10.0.0.1  nginx` — the whole line, spaces intact |
| `{-1}` | `db-01   10.0.0.9  postgres` — negative counts from the end |
| `{0:0}` | `web-01` |
| `{0:-1}` | `nginx` — last word |
| `{0:*}` | `web-01 10.0.0.1 nginx` — every word of line 0 |
| `{*:0}` | `web-01 web-02 db-01` — word 0 of every line |
| `{0..1:0}` | `web-01 web-02` — **both ends included** |
| `{..1:0}` | `web-01 web-02` — no start means from the first |
| `{1..:0}` | `web-02 db-01` — no end means through the last |
| `{*:*}` | every word of every line |

Both ends included is a deliberate departure from Python. The neighbouring syntax here is brace
expansion, where `{0..2}` has meant `0 1 2` in every shell for thirty years, and having the two
disagree by one would be a trap set for whoever just learned the other.

**`{0..2}` itself is unavailable** — it *is* brace expansion, and claiming it would break
`echo {0..2}`. A whole-line range carries a trailing colon: `{0..2:}`.

### An absent word is not the same as every word

`{0}` is the whole of line 0, one value, spaces and all. `{0:*}` is every word of line 0, three
values. The distinction matters because one of them is a filename that might contain a space.

### `{%n}` is the stage, `{n}` is what it printed

Both halves of a stage are addressable, and addressed the same way:

```sh
cat one.txt | echo "ran {%0:0} on {%0:1} and got {*}"
#             →     ran cat      on one.txt  and got …the file…
```

| written | gives |
|---|---|
| `{%0}` | `cat one.txt` — the whole command |
| `{%0:0}` | `cat` — the name |
| `{%0:1}`, `{%0:-1}` | an argument, first or last |
| `{%1:0}` | a stage further back, exactly as `{1:…}` counts |
| `{%-1:0}` | a previous *prompt's* command |

A command line has no line dimension, so `%` shifts the dimensions by one: `{%1:0}` is *one stage
back, word 0*, where `{1:0}` is *line 1, word 0*.

**Why `%` and not `!`.** `!` reads better — it is already the shell's reach-back character. It is
also unusable: history expansion runs over the line first, sees `!0` inside the braces, and fails
the line with `!0: event not found`. `%` survives both the lexer and history expansion, and its
other meaning — `%1` for a job — is only ever a whole word, never something inside a brace.

### Seen before it runs

A coordinate is painted as one, so you can tell it from the brace group it looks like:

```
ssh {0:0} uptime      {0:0}   coloured — a coordinate
mkdir {a,b}           {a,b}   plain    — brace expansion
wc -l {4}             {4}     coloured — line 4 here, a literal `{4}` in bash
echo {1..3}           {1..3}  plain    — brace expansion wins the overlap
```

The lexer asks the same two parsers the shell does, in the same order, so what is coloured is
exactly what will be substituted.

And the **session** axis is resolved as you type, drawn after the line:

```
wc -l {-1:0:1}
                ↳ one.txt
x {%-1:0}
                ↳ cat
```

**Only the session axis**, and the reason is not a limitation to hide: `{n:…}` names a stage of the
pipeline you are still typing, and that stage has not run. Resolving it would mean executing your
upstream on every keystroke. So a pipeline coordinate answers `↳ at run time` rather than a value —
silence would read as "not recognised", which is the one thing the preview exists to disprove. A
coordinate reaching past the ring says `↳ nothing there`, because that is what it will do.

**Drawn, never accepted.** It shares the row with the ghost suggestion and the repair, but the key
that accepts those asks them for their own text and never for what was drawn. There is no keystroke
that can put `↳ one.txt` into the buffer.

### One argument, whatever is in it

Substitution happens on the **syntax tree**, before any expansion runs, and every value becomes a
single-quoted word part. So a line holding a space, a `*` or a `$` arrives at the command whole and
is never field-split or re-globbed:

```sh
cat spaced.txt | printf '[%s]\n' {0}      # [my file.txt  100]     one argument
cat glob.txt   | echo {0:0}               # *.txt                  not re-globbed
```

A shell that field-splits its own substitutions is a shell that executes filenames.

### Quoting is the rule the shell already has

```sh
cat hosts.txt | echo "got {0:0}"     # got web-01     double quotes expand
cat hosts.txt | echo 'got {0:0}'     # got {0:0}      single quotes are text
```

The same split as `"$x"` versus `'$x'`. Inside quotes the values join with a space and the word
stays one word, exactly as `"${a[*]}"` does; unquoted, a lone coordinate is still one argument per
value.

### A coordinate goes where a brace expands

This is the whole rule for *where* substitution happens, and it settles a real collision: `{4}` is
line 4 and also a regex repeat count, `{1..3}` is a range of lines and also a brace sequence.

Brace expansion runs on a word's source text **before the lexer**, so by the time there is a syntax
tree an ordinary command word has already become its several words and has no brace left to mistake.
Whatever still holds a literal brace is somewhere bash deliberately refused to expand one — and a
coordinate has no more business there than `{a,b}` does.

| position | bash expands a brace? | coordinate |
|---|---|---|
| command word, redirection target | yes | yes |
| array literal `a=(x{1,2})` | yes | yes |
| scalar assignment `w=x{1..3}` | no | **no** |
| `[[ … =~ … ]]` right operand | no | **no** |
| array subscript | no | no |

The regex case was the dangerous one. Substituting there turned `^[0-9]{4}` into `^[0-9]`, and the
match then *succeeded* on a short string — `[[ 20 =~ ^[0-9]{4} ]]` was true. A wrong answer with
status 0 is the worst shape a bug can take.

### Running one stage at a time

A stage that reads its upstream by coordinate cannot start until that upstream has finished, so a
pipeline containing one runs **sequentially**, keeping what each stage printed and rewriting the
next stage's words from it. A pipeline with no coordinate — nearly all of them — is asked once,
answers no, and forks concurrently exactly as it always did.

**The last stage is never captured.** Every stage but the last already writes to a pipe, so standing
between them costs nothing and is invisible. The last stage writes to the terminal, and interposing
there would turn `isatty` false — colours off, pagers not paging, progress bars silent.

**A stage is fed from a file, not a pipe.** A pipe would deadlock: the parent cannot write a
megabyte into one while the child it is writing to has not started reading, and the child cannot
start until the parent stops writing.

**One command is not a pipeline.** A lone command runs in *this* shell, not a fork — otherwise every
builtin that changes the shell would change a child that then exits.

## What makes it different

`xargs -I{}` is a separate process with its own quoting rules and its own idea of what a word is;
this is the shell's own grammar, so a value that contains a space needs nothing said about it.
`awk '{print $3}'` needs a second language for what `{0:2}` says in six characters. `!$` reaches one
word of one previous line; this reaches any word of any line of any of the last ten, and works
inside a script, inside a function and inside quotes, where `!$` does not.

Against the tools built for exactly this — `rargs`, `gargs` — the difference is that there is nothing
to install and no `--` separator: the placeholders are in the command line the shell was already
parsing.

## Configuration

There is none, and that is deliberate. The grammar is the interface; a setting that changed which
characters mean a coordinate would mean a script could not be read without knowing the reader's
config. Two constants bound it:

| | |
|---|---|
| `PROMPTS_KEPT` | 10 — how many prompts back `{-n:…}` reaches |
| `STREAM_MAX` | 1 MiB — the most of one stream that is kept, shared with `keep`/`copy --last` |

The cap keeps the **head**, not the tail, because a coordinate counts from the start and `{-1}` on a
truncated stream is honestly the last line *of what was kept*.

## Measurements

| | |
|---|---|
| `seq 1 200000 \| wc -l` | 4 ms — an ordinary pipeline, unchanged; the gate is a scan for `{` followed by a digit |
| `cat 20k-lines \| echo {0:0}` | 3 ms, against 3 ms for the same pipeline with no coordinate |
| `seq 1 500000 \| echo {0:0}` | 8 ms — the 1 MiB cap holds, no wait for the producer to finish |
| `yes \| echo {0:0}` | 18 ms — an endless upstream is cut off, not hung |
| `yes \| head -3 \| echo {0:0}` | 9 ms |

The last two are the ones worth knowing. Reading to EOF and truncating afterwards is the same answer
for a file and a catastrophe for a tap that never closes: both of those hung for ever. The read is
bounded *during* the read, and dropping it at the cap closes it, so the next write the producer
attempts kills it with `SIGPIPE` — which is exactly what `head` does to `yes` in an ordinary
pipeline.

## What it cannot do

- **The previous command's *output* is not available.** `{-1:…}` addresses the previous command
  *line* — its words being the command and its arguments — not what it printed. Capturing a
  foreground command's output means standing between it and the terminal, which needs a pty per
  command and touches job control. `{%-1:0}` is the same word said plainly.
- **A range of streams is not meaningful.** `{0..2:0:0}` would mean "the same line of three different
  commands", which is a question nobody asks; the first is taken so the coordinate still reads.
- **A nested pipeline keeps its own stream.** A coordinate inside `cat f | { a | b; }` belongs to
  that inner pipeline, not the outer one.
- **A function body is not rewritten**, so a coordinate in one resolves when the function *runs*,
  against that call's own stack — which is empty for a function called as a pipeline stage.
- **`{5}` differs from bash on purpose.** bash leaves a one-item brace group alone, so it is still a
  literal brace when the tree is walked, and oslo reads it as line 5. With nothing captured it reads
  empty where bash would echo `{5}` back. This is the one deliberate divergence; the parity corpus
  records it as such.
- **A `{%…}` word is rendered, not re-parsed.** The rendering is the same one the job table uses to
  label a job, and it is deliberately approximate about quoting.
- **`{%…}` names the command as it *ran*, not as it was typed.** An alias is already expanded by the
  time there is a tree to walk, so with `alias cat='head -99'` the stage `cat hosts.txt` reports
  `{%0}` as `head -99 hosts.txt`. That is the truthful answer — it is what executed — but it is not
  the text on your screen, and a hook that rewrites a command has the same effect.
- **Out of range is empty, never an error.** Input is ragged; a three-line file asked for `{9}` gives
  nothing and lets the command decide.
- **No parallelism, and no `each`.** Iteration is `for`. Many values become many *arguments* to one
  command, not many commands — `ping {*:0}` is one process with three arguments.

## Where it lives

| path | what is in it |
| --- | --- |
| `crates/oslo-base/src/coords.rs` | the grammar — `Sel`, `Coord`, `Subject`, `parse`, `select`, `select_words` |
| `crates/oslo-base/src/prompts.rs` | the previous-prompt ring, which the editor previews from |
| `crates/oslo-shell/src/exec/streams.rs` | the stack — `Streams`, `substitute`, `rewrite_command`, `remember_prompt` |
| `crates/oslo-shell/src/exec/streams/gate.rs` | `command_uses_coordinates` — the question asked before anything runs |
| `crates/oslo-shell/src/exec/streams/quoted.rs` | `rewrite_inside_quotes` — the double-quoted half |
| `crates/oslo-shell/src/exec/pipeline/coordinates.rs` | `uses_coordinates`, `run`, `read_bounded` — running the stages one at a time |
| `crates/oslo-shell/src/exec/pipeline/describe.rs` | `describe_word` — what a `{%…}` word is rendered by |
| `crates/oslo-shell/src/syntax/lower/cond.rs` | `Coordinates`, `operand` — why a regex keeps its `{4}` |
| `crates/oslo-ui/src/highlight/lex.rs` | `Role::Coordinate`, `split_coordinate` — what is painted |
| `crates/oslo-ui/src/edit/session/preview.rs` | `preview` — what is drawn after the line, and never accepted |
| `tests/coordinate_tests.rs` | the wiring, driven through the real binary |
| `tests/coordinate_syntax_tests.rs` | what is and is not a coordinate |
| `tests/corpus/syntax_brace_forms_not_coordinates.sh` | the collision, checked against bash |
