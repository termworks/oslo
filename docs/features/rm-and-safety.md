# rm, and the things that can bite

`rm` is a builtin. At a prompt it can move what it removes to a trash directory instead of
unlinking it, and it takes a directory without `-r`. **In a script it is POSIX `rm` and nothing
else**, because a builtin by that name shadows `/bin/rm` for everything the shell runs, and oslo is
meant to be `/bin/sh` on a distribution.

<!-- demo:begin -->
[![rm-and-safety demo](https://asciinema.org/a/1262747.svg)](https://asciinema.org/a/1262747)
<!-- demo:end -->

## How it works

Three decisions, in order: what the options are, what this shell is allowed to do, and what happens
to each operand.

```
rm -rf build notes.txt
  │
  ├─ parse ─── an option oslo does not implement? ──yes──→ run the other rm with the whole
  │                                                        line, unchanged. $PATH first,
  │                                                        else /usr/bin/rm, else /bin/rm
  │
  ├─ mode_for(env, options)
  │      not interactive ────→ loose = false, trash = None   ← every script on the machine
  │      -s / --strict ──────→ loose = false, trash = None
  │      otherwise ──────────→ loose = true,  trash = Trash::new(…) if to_tmp
  │
  └─ for each operand
        symlink_metadata        ← never metadata: a symlink is one entry, not its target
         │
         ├─ refuse?  last component is . or ..            → skipped
         │           canonicalises to /                   → refused
         │           a directory, no -r/-d, not loose     → "Is a directory"
         │
         ├─ -i and not -f → ask: oslo's yes/no at a prompt, a stdin line anywhere else
         │
         ├─ trash?  larger than the cap ──yes──→ decline, and the caller destroys it
         │          otherwise  rename(2) ─ EXDEV ─→ copy across, then unlink
         │                     the move failed ─→ report it and fail, never destroy
         │
         └─ remove_dir_all (recursive or loose) / remove_dir (-d) / remove_file
```

`mode_for` is the whole safety argument, and it is five lines of code. `ShellOption::Interactive`
comes from the invocation and **`set -i` cannot fabricate it** — `ShellOption::from_letter` rejects
the invocation flags outright, so no script can claim to be a prompt and unlock the extensions.

### Write-protected files, and permission denied

GNU `rm` asks once *per file* before removing one you cannot write to. A git repository's objects
are all read-only, so `rm -r checkout` was two hundred questions — which nobody answers; they press
Ctrl-C and reach for `sudo`. At a prompt, with a terminal, oslo asks **once per `rm`** with its own
yes/no — "Remove them" or "Skip them" — and the answer holds for every write-protected file the
same `rm` meets. Removing a read-only file only needs its directory to be writable, so this is not
a `sudo` question.

A real "Permission denied" is: a file in a directory you cannot write to. Those failures are
reported as they happen, the rest of the tree still goes, and at the end **one** question offers
to remove what is left of those operands with `sudo rm -f` — starting on no, since it cannot be
undone. Esc or Ctrl-C on either question stops the `rm`.

In a script, under `-s`, or with stdin not a terminal, none of this happens: the stdin line GNU
prints is asked per file, and a write-protected file with nobody to ask goes.

### The size cap, and why it is a cap and not a mount check

The trash is usually on a different filesystem from the file. `/tmp` almost always is, and on most
distributions it is `tmpfs`, which is RAM. `rename(2)` cannot cross a filesystem, so a move to
`/tmp` is a copy followed by an unlink: trashing a 4 GB file copies 4 GB and then holds it in
memory until the next reboot. `max_to_tmp` bounds that. Under it the cost is not worth noticing;
over it, the file is destroyed as `rm` has always destroyed things.

Where the trash happens to be on the same filesystem the move is a plain rename and costs nothing
at any size — **the cap is applied anyway**, because a rule that changes with the mount table is a
rule nobody can predict.

The crossing itself is detected by `errno == EXDEV` rather than by comparing `st_dev`, because the
kernel is the authority on what counts as one filesystem and device numbers are not once bind
mounts exist. A symlink is copied as a link; following it would turn trashing a symlink into
copying whatever it aimed at.

A directory is measured before it is moved, which means walking it. The walk stops as soon as the
running total passes the cap, so the expensive case — a huge tree, which is destroyed anyway — is
also the one that gives up early.

### Names in the trash

The trash name is the **basename only**, and a name already taken is numbered rather than
overwritten: the second `notes.txt` becomes `notes.txt.1`, up to `.9999`. Deleting two files of the
same name from two directories must not have the second silently replace the first — that would be
a data loss committed by the feature whose entire purpose is preventing one.

Numbered rather than timestamped so the name stays readable. **The original path is not recorded
anywhere**, and there is no restore command: finding a file again means recognising it in the trash
directory and moving it back yourself.

### A failed move is a failure

If the move cannot be made, `rm` reports it and returns non-zero. It does not fall back to
unlinking. The point of the trash is that a removal is recoverable, and quietly destroying a file
the move could not save would be the one failure the user is relying on this not to have.

## What makes it different

bash, zsh, dash and fish have no `rm` builtin at all; `rm` there is `/bin/rm` and behaves the same
in a script and at a prompt. oslo's whole design problem is that a builtin does not get that
separation for free, so it is built in explicitly.

Two consequences fall out of shadowing a name everything depends on:

- **An unknown option is not an error.** GNU `rm` has options this does not implement —
  `--one-file-system`, `--preserve-root=all`, `-I`. Any option the parser does not recognise hands
  the whole invocation to the real `rm` — `$PATH` first, then `/usr/bin/rm` and `/bin/rm`, so a
  shell that has just become `/bin/sh` still finds one — and the builtin can never be *less*
  capable than the system's.
- **`\rm` gets you the program.** `command rm` is not the escape hatch people expect: `command`
  bypasses functions, and a builtin still wins. A leading backslash in oslo skips alias, function
  and builtin together, and `\\rm` skips only the builtin so an `alias rm='rm -i'` still applies.
  Both forms are prompt-only; in a script a leading backslash keeps its POSIX meaning exactly.

`rm` is also a feature, so `oslo.feature.set("rm", false)` hands the name back to `$PATH` for the
dispatcher, `type`, `command -v` and completion at once.

## What can bite

| at a prompt | in a script |
|---|---|
| `rm dir` removes a non-empty directory, recursively, without asking | error: `Is a directory` |
| `rm file` may move it to the trash | the file is gone |
| `rm big-file` destroys it if it is over `max_to_tmp`, even with `to_tmp` on | the file is gone |
| `\rm file` runs the `rm` program | `\rm` is POSIX: alias suppressed, builtin still used |

The first row is the sharp one. `loose` is on at every prompt whether or not `to_tmp` is, and the
default for `to_tmp` is **off** — so on a stock configuration `rm build` is `rm -r build` with no
trash behind it. `-s` is the way to ask for the old strictness for one line.

Two refusals are unconditional. An operand whose last component is `.` or `..` is skipped, and it
is checked **on the text rather than on a `Path`**, because `Path::file_name` answers `None` for
anything ending in `..` and `Path::components` drops a trailing `.` — both of the cases this exists
to catch are invisible to the path API, and `a/..` reached `remove_dir_all` on the parent directory
until the check was written against the string. A directory that canonicalises to `/` is refused,
so a symlink pointing at `/` is caught too.

`-d` is `rmdir`, not a quieter `-r`: it removes an empty directory and fails on one with anything
in it. Reaching for `remove_dir_all` there deleted a tree that GNU refuses to touch.

## Configuration

```lua
oslo.builtin.rm.to_tmp     = false    -- move removals aside instead of destroying them
oslo.builtin.rm.max_to_tmp = 100      -- MB; anything larger is destroyed
oslo.builtin.rm.trash      = "/tmp"   -- created on first use
```

Those are the defaults, read from the source. `to_tmp` is off because an `rm` that does not free
space is not what the name has meant since 1971, and a default that silently fills a filesystem is
not a default. A `trash` that is not a string is reported as a problem rather than ignored — a
trash directory that silently stayed `/tmp` would send files somewhere the config plainly said it
did not want them.

The settings are only ever read when the shell is interactive, which is why `oslo.builtin` is a
separate group from the rest: everything else in the settings describes an interactive session,
these are read by code that also runs in scripts.

One related default lives elsewhere:

```lua
oslo.suggest.skip_history = { "rm" }   -- the default; {} offers history for every command
```

`rm z` once suggested `rm zzz-old-notes` back from history — a path that no longer existed,
*because the suggested command had deleted it*. Accepting a ghost is one keystroke, and for `rm`
that is one keystroke aiming a destructive command at whatever the name happens to match now. Only
the history source is skipped; filesystem completion still fills the argument in.

## What it cannot do

- **Restore anything.** There is no `unrm`. The original directory is not recorded, only the
  basename, so a trashed file cannot be put back automatically.
- **Survive a reboot, on the default settings.** The default trash is `/tmp`, which is usually
  cleared at boot and is usually tmpfs. Point `trash` somewhere on the same filesystem as your work
  if you want both durability and free renames.
- **Keep a trashed name private.** `/tmp` is a directory other users can list. File permissions are
  preserved by the move, but the name is not hidden.
- **Protect `rm -rf /*`.** The `/` refusal matches an operand that resolves to `/`; a glob hands
  `rm` its children instead, and each of those is an ordinary directory.
- **Refuse `--no-preserve-root`.** It is not an option oslo implements, so it delegates — the whole
  line goes to the real `rm`, which will do what it is told.
- **Trash beyond 10,000 collisions.** After `name.9999` the fallback is `name.full`, which is not
  checked for existence and can be overwritten.
- **Help a script.** No trash, no loose directories, no ghost filtering. That is the point.

## Where it lives

| path | what |
|---|---|
| `crates/oslo-shell/src/env/builtins/remove.rs` | `builtin_rm`, `mode_for`, `refuse`, `ends_in_dot`, `parse`, `delegate` |
| `crates/oslo-shell/src/env/builtins/remove/trash.rs` | `Trash::take`, `free_name`, `size_over`, `copy_across` |
| `crates/oslo-shell/src/env/builtins/remove/tests.rs` | the script-gets-POSIX-rm test, the cap, the collision |
| `crates/oslo-ui/src/settings/mod.rs` | `Rm` and its defaults |
| `crates/oslo-ui/src/settings/from_lua.rs` | reading `oslo.builtin.rm` |
| `crates/oslo-shell/src/env/builtins/mod.rs` | where `rm` is registered |
| `crates/oslo-shell/src/env/scope/registry.rs` | the one place a feature turns a builtin off |
| `crates/oslo-shell/src/exec/simple/escape.rs` | `\rm` and `\\rm` |
| `crates/oslo-shell/src/env/builtins/nav.rs` | `nav`'s Delete calls `builtin_rm` |
| `crates/oslo-base/src/feature.rs` | the `rm` feature, `provides: &["rm"]` |
