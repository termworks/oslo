# Globbing

`*.rs`, `src/**/*.rs`, `[[ $f == *.@(txt) ]]` — pathname expansion and pattern matching. In a script
it means exactly what it means in bash, measured against bash rather than read from its manual. At
the prompt it goes further: Tab expands a glob, the highlighter says when one matches nothing, and a
qualifier list — `**/*.log(older 7d, larger 1M)` — filters by age, size, owner, git status or a
regex before the line runs.

## One engine

There used to be five implementations of "does this name match": the executor's walker, the
completion walker, the highlighter's check, `watch`'s own `**` and PATH-list editing. An audit of 397
cases against bash 5.3 found them disagreeing with bash *and* with each other — a final `**`
returning directories only, `**` walking into `.git`, a non-UTF-8 name coming back as a path that did
not exist, so `rm b*` aimed at the wrong file.

Now there is one matcher and one walker, in `oslo_base::glob`, and everything asks them: words,
`case`, `[[ ]]`, `${v#pattern}`, redirection targets, `GLOBIGNORE`, `compgen -G`, Tab, the
highlighter, the `glob` builtin and Lua. The menu, the colour and the command cannot disagree about
what a pattern names, because only one piece of code answers.

## What a script gets

The walker reads each directory once per expansion, from a cache shared by `**` and the component
after it; bash reads it twice. `**` is bash 5.3's:

| pattern | result |
|---|---|
| `**` (globstar on) | every file and directory below, recursively; `dir/**` includes `dir/` itself |
| `**/` | every directory below, each with a trailing `/`, symlinked ones included |
| `**/*.rs` | matches at every depth, depth 0 included |
| `nosuch/**` | no match, so the word stays as written (or the `nullglob`/`failglob` rule) |
| `**/**/x` | the same set as `**/x`, each path once |
| `**` (globstar off) | `*` |

Dot entries are skipped unless `dotglob`, and pruned *during* the walk, so `**` never reads `.git`
for nothing. Symlinked directories are listed and not entered: a loop is one `ln -s ..` away.

**Quoting is per character.** A quoted `*` is a literal star wherever it came from, including the
right side of `[[ == ]]` with only part of it quoted — `[[ abc == "a*"c ]]` is false, as in bash. A
backslash inside an unquoted expansion escapes, which is bash 5.2's rule: with `v='star\*n*'`,
`echo $v` matches `star*name`. `=` inside `[[ ]]` is `==`, a pattern match. `[[.x.]]` is the
character `x`, and so is `[[=x=]]`: bash does not fold `é` into `e` there either, measured.

### The options

| option | what it does |
|---|---|
| `globstar` | `**` crosses directories |
| `nullglob` | a pattern that matches nothing expands to nothing: `x=(zz*)` has no elements |
| `failglob` | it is an error instead — `no match: zz*`, status 1 |
| `dotglob` | `*` and `**` include dot entries; `.` and `..` never |
| `nocaseglob` | pathname matching ignores case |
| `nocasematch` | `case`, `[[ == ]]` and `[[ =~ ]]` ignore case |
| `GLOBIGNORE` | colon-separated patterns removed from every result; setting it implies `dotglob` |
| `extglob` | the five groups below |

`shopt -p` prints them; `BASHOPTS` is not kept up to date. `compgen -G`, `-W`, `-f` and `-d`
answer from the same engine, because scripts and bash completion functions call them.

**`failglob` abandons one command, not the script.** bash drops the *top-level* command the miss
was in — the rest of that line, the whole `if` around it, the function it was called from — reports
it, sets `$?` to 1 and carries on with the next line:

```sh
shopt -s failglob
echo a*; f() { echo zz*; echo inside; }; f; echo same line
echo next $?
```

```console
a1
script.sh: line 2: no match: zz*
next 1
```

`echo a*` had already run. `inside` and `same line` never do.

oslo does the same, in a script file and under `-c`: the outermost command list catches the miss
and skips the items that share its line. A `( … )`, a `$( … )` or a pipeline stage it happened in
all come apart with it, exactly as in bash.

### Extended patterns

With `shopt -s extglob`, five groups join the pattern language — in pathnames, `case`, `${v#…}` and
`GLOBIGNORE` alike:

| group | matches |
|---|---|
| `?(a\|b)` | zero or one of them |
| `*(a\|b)` | zero or more |
| `+(a\|b)` | one or more |
| `@(a\|b)` | exactly one |
| `!(a\|b)` | anything none of them matches: `!(*.txt)` is every name but the text files |

Inside `[[ == ]]` they work whatever `shopt` says, as in bash. With the option off, a group the
script wrote is refused with bash's `` syntax error near unexpected token `(' ``, and one that
arrived in a variable is plain text — also what bash does.

**The parser reads a group whether or not the option is on**, because it reads the whole script
before any of it runs; bash refuses these with the option off, so no working script is read
differently. `!(` is the one ambiguity: where a command can start, `!(cmd)` is `!` and a subshell,
exactly as bash reads it with `extglob` off. Matching backtracks with a memo on each piece and span,
so `+(a|aa)+(a|aa)b` against two hundred `a`s answers at once rather than exponentially.

### Order

Results are sorted once, at the end. In the `C` or `POSIX` locale that is byte order. Otherwise —
the first of `LC_ALL`, `LC_COLLATE`, `LANG` that is set names a real locale — it is a collation key
built the way glibc's ISO 14651 tables order names: letters and digits first with punctuation
ignored, then accents, then case with lower before upper, then the bytes as the last word. So
`a1 a10 a2 abc Abc ABC` comes out as bash prints it, rather than `ABC Abc a1 …`.

The key is oslo's own rather than `strcoll`: it needs no `setlocale` in a process that is also a
line editor, and it gives one answer on a system with no locales installed, which is what a static
musl binary usually finds. It agrees with bash on everything the corpus sorts; it is not glibc's
table for every script in Unicode.

## Filenames are bytes

A filename is bytes, and nothing between `readdir` and `execve` may change one. The shell's words are
strings, so a name that is not UTF-8 is carried losslessly instead: each invalid byte `0xXY` becomes
the private-use character `U+10FFXY` — the last 256 code points of the last plane — and is turned
back into that byte wherever a string becomes a path or an argument:

```
readdir → "bad\u{10FFFF}name" → word, variable, array, `for f in b*` → argv / open / stat → bad\xffname
```

The decoding points are the argv of a command, every redirection, the file tests of `[ ]` and
`[[ ]]`, `rm`, `cd`, `source`, and standard output, so `printf '%s\n' b*` writes the original byte
as bash does. `rm b*` removes the file that is there.

**The one limit:** a real filename that contains characters in `U+10FF00`–`U+10FFFF` is read back as
the bytes they stand for. Those are private-use code points in the last plane, and no input method
produces them. `cd -P` into a directory whose *resolved* path is not UTF-8 is not covered.

## At the prompt

### Tab on a glob

```text
  rm *.log<Tab>        all 3 matches        ← every match on the line, each quoted
                       build.log
                       run.log
                       old/x.log
```

The first row puts every match on the line; the rest are the matches, relative to where the pattern
starts. Closed braces are expanded first, `**` is recursive whatever `globstar` says, and a pattern
that matches nothing is retried with a `*` on the end, so `rm *.lo<Tab>` offers `build.log` and
`run.log`. `oslo.completion.glob` picks the behaviour:

| value | Tab on a glob |
|---|---|
| `"menu"` (default) | the menu above |
| `"expand"` | every match at once, as zsh does |
| `"literal"` | the word is completed as a path, glob characters and all |

**A budget, not a hope.** A Tab reads at most 20,000 directory entries, the highlighter 2,000 per
word, brace branches included. Past that it stops reading and offers what it found, rather than
freezing the editor on `/**`.

### Seeing it before it runs

A glob that matches nothing is drawn in the `glob_nomatch` theme colour, so `rm *.nomatch` is visibly
wrong before Enter. Two actions show the expansion without choosing from a menu:

| action | default key | does |
|---|---|---|
| `expand-glob` | `alt-*` | replace the word under the cursor with every match, quoted — bash's `C-x *` |
| `list-glob` | `alt-g` | show the matches below the line until the next key — bash's `C-x g` |

Single keys, because oslo binds single keys: there is no chord mechanism, and `ctrl-x` is taken.
bash's names, `glob-expand-word` and `glob-list-expansions`, are accepted too.

### Qualifiers

A parenthesised list directly after a glob word filters what it matches:

```console
$ rm **/*.log(older 7d, larger 1M)        # Tab, alt-*, or Enter →
$ rm build/a.log build/old/b.log
```

| qualifier | meaning |
|---|---|
| `file` `dir` `link` `exec` `socket` `fifo` `empty` | what the entry is |
| `older 7d` `newer 2h` | modification time; units `s m h d w M y` |
| `larger 1M` `smaller 10k` | size; `k M G T`, powers of 1024 |
| `mine` `user bob` `group dev` `perm 644` | ownership and mode |
| `hidden` `nocase` `depth 1-3` | how the walk goes |
| `not PATTERN` | drop what a second glob matches |
| `re 'REGEX'` `path re 'REGEX'` | the name, or the whole relative path, matches a regex |
| `num 1-100` | the first number in the name is in range |
| `by name\|size\|time\|ext\|depth` `rev` | the order |
| `first N` `last N` `newest N` `oldest N` `largest N` `smallest N` | a slice, after sorting |
| `tracked` `untracked` `modified` `ignored` `gitignore` | git's view, from one `git ls-files` |

**Why this is safe to add.** An unquoted `(` in the middle of a word is a syntax error in sh and in
bash, so no working command line contains one — and the list never reaches the parser. Tab,
`expand-glob` or Enter turn it into literal filenames first, so what runs and what history records
is the names, never the qualifier. A script never passes through the editor: in a file or under
`-c`, `*.log(older 7d)` is the syntax error it is in bash.

**Regex lives only here**, and in the two places below. `foo.*` in a bare word is a glob and a
perfectly good filename; reading it as a regex would change what existing scripts mean.
`*(re '^test_[0-9]+\.rs$')` is "any name, filtered by a regex", which is regex matching with no new
syntax. A group that does not open with a qualifier's name — `*(a|b)` — is bash's `extglob`, and is
left for the shell.

## In scripts and in Lua

The scriptable form is a new command, `glob`, so no existing script changes meaning. Its options are
the qualifiers, parsed by the same code:

```sh
glob '**/*.rs'                                       # one path per line; -0 for NUL
glob '**/*.log' --older 7d --larger 1M --by time --rev --first 5
glob --count 'src/**/*.rs'
glob '**/*' --re '_v[0-9]+\.' --rows | from tsv | where 'size > 1000'
glob --match 'a*.txt' abc.txt && echo yes           # test names; nothing on disk is read
```

No match prints nothing and answers 1 — never the pattern text, which is the last thing a script
wants from a command. `--rows` writes `path name ext kind size modified mode depth` as TSV.

```lua
oslo.glob("**/*.log", { older = "7d", larger = "1M", newest = 5 })
oslo.glob("src/**/*.rs", { rows = true })   -- { path, name, ext, kind, size, modified, depth }
oslo.fs.match("abc.txt", "a*.txt")          -- true
oslo.shopt("nullglob", true)                -- shopt -s nullglob; oslo.shopt("nullglob") reads it
```

`oslo.fs.glob` is the same function. A file named exactly like the pattern is found — the old
wrapper read "the result equals the pattern" as no match — and `**` crosses directories unless
`globstar = false`, whatever `shopt` the shell last ran.

## Measurements

`cargo bench --bench glob`, release, on a generated tree of 4,662 entries (six directories per level,
four levels, twelve files in each):

| pattern | matches | per expansion |
|---|---:|---:|
| `**/*` | 4,662 | 5.7 ms |
| `**/*.d` | 777 | 4.8 ms |
| `**/` | 1,555 | 4.4 ms |
| `*/*/*` | 648 | 0.3 ms |
| `**/*(file, re …, by size, rev, first 100)` | 100 | 11.0 ms |

The qualifier row costs a `stat` per candidate on top of the walk, which is what it is measuring.

The bash-exactness is measured separately, not asserted: `tests/corpus/glob_globstar.sh` and the
other `glob_` scripts beside it are run through bash and oslo by the differential suite and compared
on output and status.

## What it cannot do

A group refused because `extglob` is off exits 2 from a script file, as bash does, but 127 under
`-c`, where bash still answers 2: the refusal is made when the line runs rather than when it is
parsed, and `-c` reports a syntax error found that late the way it reports one inside `$( )`.

The shell options are process-wide rather than per shell: a subshell inherits them as it should, but
two interpreters in one process would share them.

One bash quirk is deliberately not copied. With a symlinked directory `loop/up` in the tree, bash's
`./**/*.md` goes through it and finds `./loop/up/f.md`, while its `**/*.md` in the same place does
not. oslo gives the `**/*.md` answer both ways: a `./` in front should not change which links are
followed.

Tab does not complete qualifier names inside the parentheses, and there is no match-count badge in
the hint area yet. Enter expands a qualifier however many names it produces; it does not stop and
ask first.

## Where it lives

| path | what is in it |
|---|---|
| `crates/oslo-base/src/glob.rs` | the matcher: `ShellPattern`, classes, quoting per character |
| `crates/oslo-base/src/glob/ext.rs` | `extglob` groups and their memoised matcher |
| `crates/oslo-base/src/glob/walk.rs` | the walker: `**`, the readdir cache, budgets, dedup |
| `crates/oslo-base/src/glob/collate.rs` | the collation key and when it applies |
| `crates/oslo-base/src/glob/qualify.rs` | the qualifier parser and filter, for all three surfaces |
| `crates/oslo-base/src/lossless.rs` | the `U+10FFXY` scheme: `encode`, `decode`, `to_os` |
| `crates/oslo-shell/src/expand/glob.rs` | word runs to pattern, `GLOBIGNORE`, the no-match rules |
| `crates/oslo-shell/src/exec/pipeline/mod.rs` | where a `failglob` miss is caught |
| `crates/oslo-shell/src/env/builtins/glob.rs` | the `glob` builtin |
| `crates/oslo-shell/src/env/builtins/compgen.rs` | `compgen -G -W -f -d` |
| `crates/oslo-ui/src/completion/glob.rs` | Tab and the highlighter on a glob word |
| `crates/oslo-ui/src/completion/qualified.rs` | `pattern(qualifiers)` at the prompt |
| `crates/oslo-runtime/src/lua/api/glob.rs` | `oslo.glob`, `oslo.fs.match`, `oslo.shopt` |
| `bench/glob.rs` | the measurement above |
