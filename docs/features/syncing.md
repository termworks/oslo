# Syncing between machines

One command carries everything this machine keeps to another: the history, the macros, and the
secrets. Both ends come out holding the union, deleting works in all three, and running it twice
moves nothing the second time.

```sh
oslo profile key init                                  # once, on the machine that has the data
oslo profile export | ssh laptop oslo profile import   # once, to say these two are yours
oslo profile sync laptop                                       # from then on
```

```text
history   here +2 ~0 -0   there +2 ~0 -0   unchanged 0
macros    here +1 ~0 -0   there +1 ~0 -0   unchanged 0
secrets   here +1 ~0 -0   there +1 ~0 -0   unchanged 0
```

<!-- demo:begin -->
[![syncing demo](https://asciinema.org/a/1263431.svg)](https://asciinema.org/a/1263431)
<!-- demo:end -->

## Three storages, one rule

The three things that travel are stored in three completely different ways. History is an
event-sourced database of immutable events with random ids. Macros are mutable records in a
key-value store, keyed by something a person chose — `alias/gs`. Secrets are a directory of
encrypted files. Nothing about their storage is shared, and nothing about it should be.

What they do share is the one question a sync has to answer: **when two machines both have a copy of
something, which one wins?** That is written once, in `crates/oslo-base/src/track/stamp.rs`, and
every part is built on it:

| field | what it decides |
|---|---|
| `revision: u64` | every change bumps it, so an edited copy beats one that has not changed |
| `deleted: bool` | a tie goes to the deleted one — deletion wins ties, on purpose |
| `tie_breaker: [u8; 16]` | random, stored *with the record*, rerolled on every change |

Both machines run that comparison over the same two records and reach the same answer without
negotiating. That is what makes the sync **order-independent**: it does not matter which end you
type it on, running it backwards gives the same result, and running it again moves nothing.

**No timestamps anywhere.** Two machines have no shared clock, and the one whose clock was wrong
would win every conflict for as long as it stayed wrong. A revision only ever goes up.

### Why the tie-breaker is rerolled

It settles which of two *different* edits survives when both reached the same revision. Carrying the
old one forward would let whichever machine edited first keep winning every later tie against the
same record.

## Deleting is the part that needs a design

Removing a row is the obvious implementation and the wrong one. A row that is simply gone is
indistinguishable from one this machine never had — and the other end, seeing a record we lack,
hands it straight back on the next sync. Every part therefore writes a **tombstone**: the record
stays, `deleted` is set, and the revision is bumped.

```sh
oslo history delete EVENT_ID --yes    # tombstone
oslo macros remove gs                 # tombstone
oslo secret rm deploy                 # tombstone, and the sealed body is dropped
oslo profile sync laptop                      # the removal travels
```

It does not matter whose machine the thing came from. Ownership is not a concept here: every copy is
equal, and deleting `tron`'s command from `core` is exactly as valid as deleting it on `tron`.

**`oslo history prune` is the deliberate opposite** — local retention only, no tombstones, syncs
nothing away, because trimming this machine's copy is not a decision about anybody else's.

Writing a name somebody deleted brings it back: the new revision clears the tombstone's, so the
write survives the next sync rather than being undone by it.

## What travels, and what it is

| part | scope | what a snapshot is |
|---|---|---|
| `history` | per profile | the database, via `backup_to` |
| `macros` | one per machine | the database, via `backup_to` |
| `secrets` | one store per name | a bundle of sealed files, never decrypted |

`--only history`, `--only macros`, `--only secrets` narrows it; repeat the flag for two of the three.
Without it, everything travels.

**In `oslo-minimal` there are two parts rather than three.** Secrets are behind a cargo feature, so a
build without them has no `secrets` to name — `--only secrets` answers *no such part*, and the help
lists what that build can actually do. A part that could be named and then refused would be worse
than one that is simply absent.

`NAME` decides only which *history* travels, because macros and secrets are one per machine either
way.

### There was briefly an `oslo sync`, and the reason it is gone

A profile is a history — see [One shell, several histories](profiles-and-histories.md) — and macros
are deliberately shared across every profile on a machine, so syncing them under the name `profile`
describes them wrongly. On that argument this started as a separate `oslo sync` tool, with
`oslo profile sync` carrying the history alone.

Both halves of that were mistakes. The history-only command moved a third of the machine, printed
one line about history, and said nothing about the two parts it had left behind — so they were found
missing on the far end, later. And once `oslo profile sync` was fixed to carry everything, the two
were one job under two names, which is one name more than the job has.

**`oslo profile sync` is the only spelling.** The word `profile` describes the *pairing* — the key
that says two machines are yours — which is the thing this command genuinely turns on.

## Secrets cross sealed, and are never opened

The store key is derived from the profile key, so two machines that share a profile derive the same
store key and each can open what the other wrote. Nothing is decrypted to sync — the sealed bodies
move exactly as they stand, and the plaintext is never in memory on either side.

That forces one design decision worth knowing. Each file carries its stamp in a plaintext header
**outside** the ciphertext:

```text
OSLOSEC1 3 live 9f1c…        (32 hex characters)
<the sealed bytes, exactly as the crypto produced them>
```

It has to be outside, because **syncing must not need the key**. A store whose crypto is a YubiKey
works only on the machine the key is plugged into — but the other machine still has to be able to
carry its files, or the store you most want backed up is the one that cannot travel.

**What that costs, said plainly:** the header is not authenticated, because authenticating it would
mean holding the key to read it. Somebody who can *write* to your store could roll a revision back
or flip a tombstone and make a sync carry the wrong answer. They cannot read anything — the body is
still sealed — and somebody with write access could already delete the file outright. The store is
the thing you may commit and copy about; the key is not, and that is the boundary that matters.

The bundle format is deliberately not tar: it carries a flat directory of files whose names oslo
already validates, and tar would bring permissions, ownership, symlinks, device nodes and path
traversal in exchange for nothing this needs. A name with a `/`, a `..`, a control character or a
bidirectional override in it is refused rather than sanitised, and so is a name sent twice. Only
regular files are packed: each entry is looked at without following it, so a link, a FIFO or a
device in the store is passed over unopened, as are the store's `.lock` and a write's scratch
files. What arrives is unpacked into a `0700` directory, each file `0600`.

A header that is damaged — malformed, or a revision above 2^53 — on either side stops the merge
before anything is written, and the error names the file. Both stores are locked for the merge, so a
`set` cannot land in the middle of it.

## The far end is oslo, not scp

```text
oslo profile fingerprint NAME       ssh laptop oslo profile fingerprint NAME
          └──────────── must be equal, or nothing moves ─────────┘

ssh laptop oslo profile send WHAT   ─────────────►  a snapshot of theirs
          merge, both directions                 both copies gain
ssh laptop oslo profile receive WHAT  ◄───────────  the merged copy, merged again over there
```

Every store here is a live database or a directory a shell may be writing to this instant, and
copying the bytes out from under it is how you get half a transaction. `send` takes a proper
snapshot; `receive` **merges** rather than replaces, so something typed on the other machine between
the two steps survives instead of being overwritten.

`$OSLO_SSH` replaces the `ssh` it runs — a wrapper, a jump host, an alternate config, `mosh` — and
`$OSLO_SSH_REMOTE_BIN` names the far end's `oslo` when it is not on the default `$PATH`. Standard
error is inherited, so ssh's own questions and a hardware key asking for a touch reach the terminal
rather than being captured into a sync that appears to hang.

### Both machines need an oslo that has it

The answer from the far end is headed `OSLOSYNC1 <part>`, and one that is not is refused with a
sentence saying so. That header exists because of how the failure actually looked: an oslo without
`sync` does not fail loudly — the word is not one of its tools, so it goes looking for a *program*
called `sync`, and `sync(1)` exists on most systems. It ran, printed nothing, exited 0, and the near
end read that as *that machine has no history*.

```
oslo profile sync: laptop: did not answer as an oslo that can sync.
  Its oslo is most likely too old — `oslo profile sync` needs one on both machines.
```

## Why a key, and not just the hostname

`default` here and `default` on a machine you have an account on are two histories that share a
word. Syncing on the strength of the word would merge a stranger's data into yours, so a profile
carries a key and sync refuses unless both ends hold the same one:

```
default: this machine is 4a2705fd014bc22b and laptop is 91c3e0a7715fe2b8 — they are not the
same profile.
  If they should be: `oslo profile export default | ssh laptop oslo profile import default`
```

The key is at `$XDG_STATE_HOME/oslo/profiles/<name>.key`, mode `0600`, and never in a store — the
stores are what travel, and a key inside one would travel with every copy. What crosses the wire is
a **fingerprint**: sixteen hex characters of a hash, enough to compare by eye and useless to
anybody who intercepts it. The fingerprints are checked once, before anything is copied, for all
three parts.

**It is not the security of the sync.** ssh is already doing that, before any of this is consulted.
What the key answers is *identity* — whose data is this.

## What arrives is published, not just stored

Aliases are also a flat snapshot a starting shell reads and a file another shell sources, and a
merge rewrites both. Scripts are files in `~/.local/sbin`: they cross the wire as records beside the
database's own, stamped against `scripts.ledger`, and the winners are written back as files — so an
edit made to a file by hand travels, and so does deleting one. See
[Macros](macros.md#between-machines).

## What it cannot do

- **Sync with a machine whose oslo predates this.** Both ends need one that has `sync`. It says so
  in a sentence rather than failing obscurely, which it did until the wire header was added.
- **Run itself.** `oslo profile sync` is a command; putting it in a login file or a timer is yours to decide,
  and it is safe there because a second run moves nothing.
- **Rotate or revoke the key.** A machine that has it can read what it already has; taking it off
  the list is `rm` on that machine, not something this can reach.
- **Reach a machine you cannot ssh to.** There is no daemon, no server, no third party, and no
  account anywhere. If ssh works, this works.
- **Merge two different profiles.** The fingerprint check is not a warning; it is a refusal.
- **Sync anything else.** Not the config, not plugins, not the direnv allow list, not the prediction
  model. Those are either version-controlled already or deliberately per-machine.
- **Undo.** A merged store is merged. `--dry-run` is the thing to reach for first, and it asks the
  far end for its copy without writing to either side.
- **Tell you what a conflict was.** When both machines edited the same macro, one wins silently by
  the rule above. Nothing records that the other version existed.

## Where it lives

| path | what |
|---|---|
| `crates/oslo-base/src/track/stamp.rs` | the rule: revision, tombstone, tie-breaker, and `settle` |
| `crates/oslo-base/src/macros/sync.rs` | merging two macro stores, name by name |
| `crates/oslo-base/src/secrets/sync.rs` | the file header, and merging two stores without a key |
| `crates/oslo-base/src/track/sync/admin.rs` | `sync_files`, the history merge |
| `src/cli/profile/sync.rs` | the `sync` subcommand, which hands its words to the parser below |
| `src/cli/sync.rs` | the parser, `--only` and `--dry-run` — not a command of its own |
| `src/cli/sync/part.rs` | the three parts, the wire header, `send` and `receive` |
| `src/cli/sync/part/bundle.rs` | the sealed-file bundle, and the names it refuses |
| `src/cli/sync/ssh.rs` | the transport, and the fingerprint check that comes first |
| `tests/sync_tests.rs` | two machines, through the real binary |
