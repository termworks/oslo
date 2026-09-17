# Where you have been

`cd` with the directories you have actually been in behind it: one step back, *N* steps back, the
top of the working tree, and a name you have visited before. All of it hangs off the failure of an
ordinary `cd`, so nothing a script does changes meaning.

<!-- demo:begin -->
[![where-you-have-been demo](https://asciinema.org/a/1262755.svg)](https://asciinema.org/a/1262755)
<!-- demo:end -->

## How it works

`builtin_cd` is a ladder and the order of its rungs is the whole safety argument. Options, arity,
`$HOME`, `$OLDPWD` and the directory ring are settled first and answer directly; the operand is then
tried as what it says it is, against `$PWD` and `CDPATH` and the kernel. **Only once that has failed
is a remembered directory of that name considered**, and only in a shell that has a store.

```
cd [-L|-P] [--] [operand]
  │
  ├─ parse_mode      -L / -P, last wins;  `--` ends options
  │                  `-3` is NOT an option — the digits arm stops the parser
  │
  ├─ (none) ────────────────→ $HOME                                    ─┐
  ├─ "-" ───────────────────→ $OLDPWD                    announce      ├─ the shell
  ├─ "-N" ──────────────────→ ring::nth_back(N)          announce      ┘  chose these
  │
  └─ a name ─→ attempt_directory():  CDPATH search → chdir(2) → $PWD, $OLDPWD, ring
                 │
                 ├─ ok ─────────────────────────────────────────→ status 0
                 └─ Err(e)
                      │   named operands only, and only if track::store() exists
                      ├─ jump() ── found ─→ print the destination      → status 0
                      └─ nothing ────────→ report `e`, unchanged       → status 1
```

`$HOME`, `$OLDPWD` and a ring entry are places the shell itself chose, so a failure there is never
answered by a guess: if one of them has gone, proposing a replacement would be a lie about where the
shell was sending you. Only an operand a person typed as a name is a jump candidate.

### The ring, and why `cd -N` exists

`cd -` is a one-deep toggle and useless the moment you are three wrong turns from where you meant to
be. Every move the shell makes is recorded — in `attempt_directory`, which is where `cd`, `pushd`,
`popd`, `oslo.sys.cd` and the jump itself all land, so a `cd` inside a shell function is in the ring
like any other. The interactive loop seeds it with the directory the session began in, which is what
makes `cd -1` and `cd -` name the same place from the first move rather than from the second; a
test pins that agreement, because they are two different mechanisms (`$OLDPWD` and the ring) and the
user's model requires them to coincide.

The ring holds 32 entries, drops the oldest, and collapses a move to the directory it is already in.
`dirh` prints it newest first, numbered by how far back each one is — which is the number `cd -N`
takes, and without it `cd -3` is a guess. It is deliberately not the `pushd`/`popd` stack: that one
is explicit and scripts depend on it, and a shared store would have `popd` finding directories
nobody pushed.

### `cd root`

`root` means the top of this working tree, found by walking parents for a `.git` that exists —
`.exists()` rather than `.is_dir()`, so a linked worktree, where `.git` is a file naming the real
one, resolves correctly. No `git` subprocess anywhere. A real `./root` still wins, because the plain
move is tried first and only its failure reaches the word; outside a repository the word means
nothing in particular and falls through to being a name like any other.

### The jump, and the ranking

```
cd notes            here = logical $PWD        workspace = git_root() or None
   │
   ├─ directories_named("notes")   range over DirByBase, the folded final component
   │      ≤ 200 rows in frecency order; $PWD and $HOME are dropped in the store,
   │      not by the caller afterwards
   │      └─ rank() → first hit that is still a directory on disk  ──→ chdir
   │
   └─ nothing survived
        directories_ranked()       one scan of every dir row, ≤ 200 by frecency
        └─ rank() → same           Contains and Path can only be reached here

rank():  drop rows with no tier, and rows that fail eligible(tier, visits),
         then sort by

   1  tier      Exact > Prefix > Contains > Path        ← the primary key
   2  local     inside the workspace beats outside it
   3  score     visits / (1 + ln(1 + age_hours))
   4  length    the shorter path
   5  path      lexicographic — the same store must answer the same way twice
```

**Match quality is the primary key and frecency only orders candidates that matched equally well.**
That is the fix for one defect that wears four hats in zoxide: there the keywords are a pure filter
and take no part in the score, so the most-visited candidate wins however badly it matched. Each of
the four is a named test in `track/score.rs` — `zoxide_956_an_exact_name_beats_a_frequent_partial`,
`zoxide_247_an_exact_name_beats_a_frequent_prefix`, `zoxide_929_the_src_of_the_project_you_are_in_wins`
and `zoxide_260_frequency_still_decides_inside_a_tier`. The last of those is the one that says what
the design is *not*: where every candidate matched equally, frequency is exactly the right decider
and nothing overrides it.

Two guards sit under the ordering. A directory seen once is reachable only by naming its final
component exactly (`MIN_VISITS = 2`), so one stray `cd` into a typo does not become a permanent
destination. And a remembered path that is no longer a directory is skipped rather than jumped to
and rather than deleted on sight — an unplugged drive is not a directory that stopped existing, and
deciding that is the prune sweep's job.

The curve is `visits / (1 + ln(1 + age_hours))`, the same expression the completion ranker uses, so
the two agree about what "best" means. Normalised to a fresh visit, and asserted in
`the_house_curve_decays_smoothly`:

| age of the last visit | 1 hour | 1 day | 1 week | 1 month | 1 year |
|---|---:|---:|---:|---:|---:|
| what one visit is worth | 0.591 | 0.237 | 0.163 | 0.132 | 0.099 |

Smooth, with no boundary for two directories to trade places across. zoxide instead multiplies a raw
counter by one of four constants chosen from four age buckets, which is a 2011 shell-script artefact:
a directory loses three quarters of its standing the instant it crosses a bucket edge, for no reason
the user can perceive.

## What makes it different

bash's `cd` accepts `-` and nothing else numbered; the counted form there belongs to `pushd +N` and
`popd +N` over a stack you build by hand. oslo keeps both — `pushd`/`popd` are untouched — and adds
a ring nobody has to maintain, because the shell already knows every directory it moved to.

oslo once had `prevd`/`nextd`, walking the ring with a cursor, and deleted them: `cd -` and `cd -N`
already reach every entry, and their `walk_to` assigned `$PWD` by hand without touching `$OLDPWD`,
so walking with `prevd` silently desynchronised `cd -`. That was a bug being removed rather than a
feature.

Against a jump tool bolted on with a shell function: zoxide excludes the current directory in *its
own shell function*, by string equality against `pwd`. Here the exclusion happens inside the store
read, so `cd src` while standing in one `src` is the other one, which is a test rather than a hope.
And the frecency scan zoxide performs on every query is reached here only when the by-name index
found nothing, only for a `cd` a person typed, and never per keystroke.

## Configuration

There is no setting that turns the jump on or off. It is on when there is a store, and there is a
store only in an interactive session that keeps a history:

```sh
CDPATH=$HOME/src:$HOME/work    # POSIX, searched before the jump is ever considered
HISTFILE="" oslo               # no history file → no store → cd is plain POSIX cd
HISTSIZE=0 oslo                # the same switch, said the other way
```

```lua
oslo.history.file = ""         -- the config spelling of HISTFILE=""
oslo.history.size = 0
```

Both directory hooks fire from `attempt_directory`, so they see every move including a jump, a
`pushd` and a `popd` — and `pre-change-dir` is told the destination after it has been resolved,
which is the difference between guarding `/srv` and guarding the word `..`:

```lua
-- refuses the move:
oslo.on.pre_change_dir(function(d) if d.to == "/srv" then return false end end)
oslo.on.post_change_dir(function(d) print(d.from .. " -> " .. d.to) end)
```

`shopt -s autocd` (or `OSLO_AUTOCD=1`) makes a bare word that names a real directory a `cd`. It is
interactive-only and never reaches the jump: it checks `is_dir` first, so a word that is not a
directory here stays `command not found`.

## Measurements

None for the jump itself; nothing in `bench/` measures it. The one recorded number that bounds it is
the store's, in `track/prune/mod.rs`: 400 rows fit in the 128 KiB a fresh store is born with, and
somewhere between 500 and 1,000 rows the file steps to 8.5 MiB and stays there, which is why the
per-directory cap and the ninety-day rule exist. The scan tier walks that file's directory rows.

## What it cannot do

- **Take more than one keyword.** The ranker implements them — order matters, and every keyword but
  the last must be satisfied before the final component — but `cd` has arity for one operand, and
  `cd foo bar` is `too many arguments` with status 2. Only `Query::one` has a caller.
- **Offer you a choice.** There is no picker and no listing: the best candidate that still exists is
  taken, and the destination is printed because you cannot see it on the line you typed.
- **Work anywhere but an interactive session.** A script, `oslo -c` and a subshell never install a
  store, so `jump` finds none and the original diagnostic is the answer. This is structural, not a
  flag someone has to remember to check.
- **Jump to `/tmp` itself, or anything under a `.git` or `node_modules` component.** What runs
  there is recorded and in the history finder like anywhere else; `cd` just never offers the
  directory. `/tmp/build-xyz` is ordinary work and is offered. `$HOME` is the same — recorded, but
  never a jump target, since bare `cd` already goes there.
- **Cross machines.** Directory rows that arrive by sync are written without their by-name index
  entry, so no `cd <name>` can reach one.
- **Survive the shell.** The ring is process-global and unpersisted; a new terminal starts with one
  entry. Two terminals do not share it, and `cd -N` in one says nothing about the other.
- **Notice that `$PWD` and `getcwd()` disagree.** The store writes the physical path the REPL read;
  the jump excludes where you are standing by the *logical* `$PWD`. Standing in a symlinked path
  spelled differently from its target, the exclusion can miss and the answer can be the directory
  you are already in.
- **Find a directory by a name that is not a substring of it.** The tiers are exact, prefix,
  contains and the whole-path rule. There is no edit distance, so `cd projcts` reaches nothing.
- **Reach a multi-component operand through the index.** `cd work/proj` folds to one keyword with a
  separator in it, which the final-component index cannot serve; it is answered by the scan, at the
  lowest tier, and so needs two visits.

## Where it lives

| path | what |
|---|---|
| `crates/oslo-shell/src/env/builtins/directories/cd.rs` | `builtin_cd`, `parse_mode`, the ladder |
| `crates/oslo-shell/src/env/builtins/directories/chdir.rs` | `attempt_directory` — the one place the shell moves, `-L`/`-P`, `CDPATH`, the hooks |
| `crates/oslo-shell/src/env/builtins/directories/ring.rs` | the ring, `nth_back`, `builtin_dirh` |
| `crates/oslo-shell/src/env/builtins/directories/jump.rs` | `jump`, `destination`, `pick`, `REPOSITORY` |
| `crates/oslo-base/src/track/score.rs` | `Tier`, `Query::tier_of`, `score`, `compare`, `rank`, `eligible` |
| `crates/oslo-base/src/track/query.rs` | `directories_named`, `directories_ranked` |
| `crates/oslo-base/src/track/write.rs` | `prime`, `record`, `arrive` — where a visit is counted |
| `crates/oslo-base/src/track/redact.rs` | `is_excluded`, the directory exclusion list |
| `crates/oslo-base/src/track/mod.rs` | `install`/`store` — the gate the whole feature hangs on |
| `crates/oslo-ui/src/prompt.rs` | `git_root_of`, the walk that makes `cd root` free |
| `crates/oslo-runtime/src/startup/repl.rs` | seeds the ring with the session's first directory |
