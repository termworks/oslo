# Secrets

A value kept encrypted on disk and handed out when something asks for it — with the key deliberately
somewhere else, so the store can go where files go.

```sh
oslo secret set stripe                 # asks for the value, masked; or reads standard input
oslo secret get stripe                 # writes it to standard output, and nowhere else
oslo secret run TOKEN=stripe -- curl … # one value, one child, no shell in between
oslo secret list
oslo secret where                      # the store, the keys it reads, and which may be committed
oslo secret rm stripe
```

## Two features, because they answer different questions

| | | |
|---|---:|---|
| **`secrets`** | +96 KB, 1 package | the *filing*: stores, names, `run`, the lazy variable, `oslo.secret`, the hooks. **No crypto of its own** |
| **`crypt`** | +60 KB, 17 packages | the *built-in mechanism*, so a fresh install encrypts without being told anything |

A distribution shipping `/bin/sh` can take the filing alone and name the machine's own tool —
`age`, `gpg`, `systemd-creds` — in one line of configuration. Someone who wants it self-contained
keeps `crypt` and never thinks about it. Both are off in `oslo-minimal`.

<!-- demo:begin -->
<!-- demo:end -->

## Why this is a shell's problem

Every secret a person keeps ends up in a shell eventually: exported for one command, pasted into a
`curl`, read out of a `.env` that should never have been written. The shell is where secrets are
*used*, and it is the one program in the chain that can hold a value in memory, hand it to a single
command, and never write it down.

Anything outside the shell has the opposite problem. A secrets manager that is not the shell has to
put the value somewhere the shell can read — an environment variable set at login, a file, a
`source`d fragment — and each of those is the plaintext this was supposed to avoid.

## A store is three things

A **directory**, an ordered list of **keys**, and the **mechanism** that seals and opens its files.

```
$XDG_DATA_HOME/oslo/secrets/NAME.sealed       the `user` store
$XDG_DATA_HOME/oslo/secrets/.lock             held while a write reads and replaces a file
$XDG_DATA_HOME/oslo/stores/work/              a named store
$XDG_DATA_HOME/oslo/plugins/notes.secrets/    a plugin's own, beside the .kv oslo.db makes for it
$XDG_STATE_HOME/oslo/profiles/default.key     the profile's key, which every store derives from
$XDG_STATE_HOME/oslo/secrets.conf             which stores exist, and what each is made of
```

**Private from the first write.** A store directory oslo chose, `$XDG_STATE_HOME/oslo` and its
`profiles/` are made at `0700`, and an existing one that is looser is tightened to `0700` the next
time oslo writes into it. A `directory` you named in `secrets.conf` is made `0700` when it is
missing and otherwise left alone: oslo does not change the mode of your directory, and warns on
every write while other users can list it. Every file is `0600` from the moment it exists.

The mechanism is the one that decides **where a store can be used**, so it is worth its own table:

| | does the crypto | reachable from |
|---|---|---|
| default | oslo's own, in this process | anywhere the binary runs |
| `encrypt`/`decrypt command` | another program, down a pipe | anywhere: a subprocess, not a shell |
| `crypto hook` | a Lua handler | only a shell that has read your config |

## The built-in mechanism

**A sealed box: a key you keep, recipients you publish, and a writer every reader checks.** One
random *file key* encrypts the value, and that file key is wrapped once per recipient against their
public half. The key of each wrap comes from two X25519 exchanges with the recipient's public half:
one with a fresh ephemeral secret, and one with the writer's own secret. This is the construction
HPKE calls Auth mode (RFC 9180 §5.1.3). The second exchange can only be computed by someone who holds
the writer's secret or the recipient's, so a published half is not enough to write a file that
opens.

```
OSLO3 │ writer (32) │ n │ n × [ ephemeral (32) │ wrapped file key (48) ] │ nonce (24) │ ciphertext ‖ tag
```

The wrapping key is HKDF-SHA256 over both exchanges, with the three public halves (ephemeral,
recipient, writer) in its info. Each AEAD also authenticates the header and what the file is *for*:

```
each wrap   OSLO3 ‖ writer ‖ n ‖ every stanza's ephemeral half ‖ context
the body    OSLO3 ‖ writer ‖ n ‖ every stanza, whole            ‖ context
context     0x01 ‖ u32 len ‖ store ‖ u32 len ‖ name             a secret in a store
            0x02 ‖ "oslo.secret.seal"                           a seal from Lua, with no name
```

A wrap cannot cover its own ciphertext, so the wraps cover the ephemeral halves and the body covers
the stanzas whole. That is why a file renamed, swapped for another, moved to another store, given a
different count, or with a stanza stripped or added does not open.

**A reader accepts a file only from a writer it lists.** The writer is the key the store writes with,
and its public half is in the header. A file opens only when that writer is one of the store's
recipients *as this machine lists them*, or is the key that is opening it. Somebody who knows only a
published half can still seal a file to it, but they have to seal it as themselves, and the reader
refuses it and names the key that wrote it.

**The secret half is the profile's, not a file of its own.** A store derives its key from the
[profile key](profiles-and-histories.md#why-a-key-and-not-just-the-name) — HKDF over the profile's
key, salted with the store's name — so a machine keeps one secret rather than two, and
`oslo profile export` carries the history *and* the secrets in one step. Two stores under one
profile still have different keys, so a file from one cannot be opened with the other's.

`$OSLO_SECRET_IDENTITY` still wins when it is set, and `key file`/`key command` still override per
store: naming a file is somebody saying *this one*, and that has to keep meaning what it says.

The two halves look different on purpose, because one of them is safe to publish and the other is
not:

```
OSLO-KEY-1:hqFoSGbhpF797BbBSjB+95yipkKjh2g2kEaXOFPlIYk=    the file, mode 0600
OSLO-PUB-1:zyzEW4hwVZdjajit/OwLB+o2647dl8YfoWY+mdCS+QM=    what you hand out
```

Pasting one where the other belongs is refused rather than quietly accepted — a published half read
as a key would make the store openable by everybody who has it. An error never repeats a private
key: `recipient add OSLO-KEY-1:…` names it by its prefix alone, and so does every other message that
might be handed one (`OSLO-KEY-1:`, `OSLO-PROFILE-1:`, `AGE-SECRET-KEY-`).

**A low-order point is never a recipient.** There are a few X25519 public values that every secret
key shares the same exchange with, so anything sealed to one is readable by anybody.
`OSLO-PUB-1:AAAA…=` is one of them. They are refused wherever a public half comes in: on the command
line, in a `--from` file, in a `recipient` line of `secrets.conf`, and as a file's writer. Sealing
fails if an exchange comes out as zero, and opening skips a stanza whose exchange does.

That indirection is what a single symmetric key cannot have: **a store can be readable by several
keys without any of them being shared.**

```sh
oslo profile key init                     # on a machine that has no key yet
oslo secret recipient                     # on each machine: prints its OSLO-PUB-1 half
oslo secret recipient add OSLO-PUB-1:…    # on each machine: the other one's half
oslo secret rotate                        # so what already exists reaches them too
```

`oslo secret key init` is only for a store whose key is a file of its own (`key file`); on a store
that takes the profile's key it says so and points at `oslo profile key init`.

Your laptop keeps one secret, the server keeps another, and the store lists two recipients. Nobody
hands anybody a private key. Taking a machine off the list is a `rotate`, a `recipient rm` and
another `rotate`: a file that machine wrote no longer opens once it is off the list, so the first
`rotate` rewrites everything as this machine's own while it still does.
**Sharing is listed on both sides**, because a reader accepts only writers it lists: until the server
lists the laptop, it refuses what the laptop wrote, and says which key wrote it. Two machines that
share a profile derive the same store key, so they need no list at all.
**Adding a recipient does not re-encrypt anything** — they can read what is written after today, and
`rotate` is the separate, deliberate step, because *who could read this before I changed it* has a
permanent answer.

**Writing needs the writer's own key.** Every write is sealed as this store's key, so a store with
recipients but no key of its own on this machine makes one on the first write (from the profile, or
in its key file), and a store whose key is a `key command` runs that command to write. With
`$OSLO_SECRET_NO_EXEC` set, such a store cannot be written.

`X`ChaCha for the payload, so the 192-bit nonce can be drawn at random on every write with nobody
counting. An X25519 output is a curve point rather than a uniform key, so it always goes through
HKDF. Everything random — nonces, file keys, ephemeral secrets — comes from `getrandom`, never a
seeded generator. The same value written twice produces two different files, so nobody holding your
backups can tell which secrets changed between them.

### What is authenticated, and what is not

| | authenticated? | so an edit to it |
|---|---|---|
| the value | yes, by the body's tag | is refused: *the body fails its check* |
| `OSLO3`, the writer, the count, every stanza | yes, by the body, and the ephemeral halves by every wrap too | is refused |
| the store's name and the secret's name | yes, as the context of every wrap and the body | a swapped, renamed or moved file is refused |
| who wrote it | as *one of the keys this reader lists* | a writer nobody listed is refused, by name |
| the sync header, `OSLOSEC1 …` | **no** | can roll a revision back or flip a tombstone |
| that this is the newest copy | **no** | an older genuine copy of the same name in the same store opens |
| a seal from `oslo.secret.seal` | bound to a fixed label, **not** to a store or a name | opens with any store whose key it was sealed to |

**The sync header stays unauthenticated on purpose.** Sync carries files between machines without
holding the key, so it must be able to read and write the stamp without one; see
[Between machines](#between-machines).

**A recipient is also a writer.** Anybody on the list can write a file every other reader on the
list accepts; there is no read-only recipient. And a reader can always make a file that it alone
would accept as written by any key it lists, because the writer's exchange is the same number from
either side. That only ever fools the reader who made it.

**The writer's public half is in every file**, so whoever can read the store can see which key wrote
each secret. They cannot see who it was written *for*: the stanzas name no recipient.

A file that does not open says which of four things went wrong, and never prints the value:

| | |
|---|---|
| `no key opened it … sealed for other keys, or under another name or store, or its header was changed` | no stanza opens with any key of this store |
| `… sealed by OSLO-PUB-1:…, which is not one of the recipients this reader accepts` | it opens, and its writer is not listed here |
| `… the body fails its check: the file was changed or damaged after it was written` | a stanza opens, and the body does not |
| `not a sealed file this oslo reads: …` | truncated, or not this format |

### Files from before: `OSLO2`

The format before this one had no writer and no AAD:

```
OSLO2 │ n │ n × [ ephemeral public (32) │ wrapped file key (48) ] │ nonce (24) │ ciphertext ‖ tag
```

Anybody with a recipient's public half could write an `OSLO2` file that opened, and one could be
renamed, moved, or have a stanza stripped. **It is read until the store is rotated, and refused
after.**

* A read of an `OSLO2` file works, and prints one warning per process naming it.
* `oslo secret rotate` rewrites every file as `OSLO3`, then writes `refuse OSLO2` into that store's
  section of `secrets.conf`. From then on an `OSLO2` file in that store is refused.
* **Nothing can tell a forged `OSLO2` file from a real one**, so `rotate` upgrades whatever is there.
  It names every `OSLO2` file it rewrote; check those values, because one you do not recognise was
  planted before the upgrade.
* An `OSLO2` file that arrives later, from a machine still running an older oslo, is refused.
  Upgrade oslo there first. Deleting the `refuse OSLO2` line reads them again until the next
  `rotate`.

A store that has never held an `OSLO2` file can be made strict at once: `rotate` on it rewrites
nothing and writes the line.

### Keys in memory

Store keys, profile keys, file keys, wrapping keys, ephemeral secrets and the decrypted value are
held in buffers that are zeroed when they are dropped, on the native path and on `get`, `run` and
`set`. `x25519-dalek` and `chacha20poly1305` zero their own copies. `oslo secret` makes its process
non-dumpable before it does anything, so it writes no core file and no other process can `ptrace` it
or read its memory. Nothing is restored, because the process exits or execs, and an exec'd command
is dumpable again. In a long-lived shell, a seal or an unseal from Lua makes the process non-dumpable
only for that call and then restores it, because a shell that stayed non-dumpable would hide
`/proc/PID/cwd` from the terminal, which uses it to open a tab in the same directory.

What is not wiped: the value once it is a Lua string, which lives until the collector frees it; the
environment of the command `run` execs; `hkdf`'s internal state, which has no `zeroize` feature in
the version used here; and the masked prompt's own line buffer.

### What it deliberately is not

**Not the age format.** `age -d` will not open these files. What was worth having was the key model,
and carrying the format meant bech32, scrypt, a localised error catalogue and thirty-two packages —
against seventeen for the model alone.

No passphrase, and no key derivation from one: the secret is a file at mode `0600`, like
`~/.ssh/id_ed25519`.

**And none of it is compulsory.** The mechanism is one of three, and the other two do not involve
any of this:

```sh
oslo secret --store team cipher encrypt -- age -R ~/.config/age/recipients.txt
oslo secret --store team cipher decrypt -- age -d -i ~/.config/age/identity
```

That is the door to real age files, to `gpg`, to `systemd-creds --with-key=tpm2`, and to a YubiKey
through `age-plugin-yubikey`. A store that names one never reaches the code above. **A store that
names only one half cannot do the other**: with no `decrypt command` a `get` is an error, and with no
`encrypt command` so is a `set`. Neither falls back to the built-in mechanism, which would seal to a
key the store was never meant to use.

## Configuration is a file, not Lua

`oslo secret get` is dispatched as a tool: it never builds an `Environment`, never reads
`init.lua`, never starts a Lua interpreter. That is not an oversight. `$(oslo secret get
gh-token)` has to work from `dash`, from `cron`, from a `Makefile` and from a container, none of
which have run an oslo config — so configuration that only existed after `init.lua` had run would
apply in your interactive shell and silently not apply anywhere else.

So it is a flat file that the process doing the decrypting reads for itself:

```
# ~/.local/state/oslo/secrets.conf
default work

[work]
directory ~/src/dotfiles/secrets              # a store meant to be committed
key file /home/you/.ssh/oslo-work
key command pass show oslo/key
key timeout 30s                               # how long a command here may take
recipient OSLO-PUB-1:jmtSV18HQJ/Ph1RXFGFTX8vbjNYpveSlwm1n9AwJySY=   # you
recipient OSLO-PUB-1:zyzEW4hwVZdjajit/OwLB+o2647dl8YfoWY+mdCS+QM=   # the build server
refuse OSLO2                                 # written by `rotate`: no file from before is read

[team]
directory /home/you/src/team/secrets
encrypt command age -R /home/you/.config/age/recipients.txt
decrypt command age -d -i /home/you/.config/age/identity
```

**Beside the key, not inside the store.** The store is meant to be committable; this file names this
machine's key paths, and a `secrets.conf` restored from somebody else's backup silently changing
where this machine looks for a key is not a thing to ship.

**A malformed line is an error, never a skipped line.** In a build without `crypt` a `key` line is
refused outright rather than parsed and ignored — the key belongs to whatever program that store
names, and a line that quietly did nothing is how somebody comes to believe a store is protected by
something it is not. `key timeout` is the exception: it bounds the store's `encrypt`/`decrypt
command` too, so every build accepts it.

**A path is absolute, or starts with `~/`.** `key file` and `directory` take either, and `~/` means
`$HOME`. A relative path is refused: it would name a different file in every directory the shell is
run from, which is how a key once ended up at `project/~/.ssh/…`. `$OSLO_SECRET_IDENTITY` must be
absolute for the same reason.

Nothing needs to be edited by hand:

```sh
oslo profile key init                      # the key a store derives its own from
oslo secret recipient                      # the half to publish, derived and shown
oslo secret key add file ~/.ssh/oslo-work  # or override it, per store
oslo secret key add command -- pass show oslo/key
oslo secret key list

oslo secret recipient add OSLO-PUB-1:…     # a colleague, or your other machine
oslo secret recipient add --from RECIPIENTS
oslo secret recipient --export > RECIPIENTS
oslo secret recipient rm OSLO-PUB-1:…
oslo secret rotate                         # re-encrypt everything to the list as it now stands

oslo secret cipher encrypt -- age -R ~/.config/age/recipients.txt   # or hand it all to age
oslo secret cipher decrypt -- age -d -i ~/.config/age/identity
```

The commands splice lines rather than re-render the file, so comments and ordering survive.

### Which store an invocation means

| | scope |
|---|---|
| `--store NAME` | this invocation |
| `$OSLO_SECRET_STORE` | this shell, inherited by every child — `OSLO_SECRET_STORE=work make deploy` |
| `default NAME` in `secrets.conf` | this machine |
| `user` | built in |

## Keys, including ones oslo cannot compute

A key is a **file** to read, or a **command** to run whose output is the key. The second exists
because the alternative is compiling every way a person might hold a key into a shell meant to be
`/bin/sh` — a password manager, a smartcard wrapper, a decryption service:

```
key command pass show oslo/key
key command gpg --quiet --decrypt /home/you/oslo-key.gpg
```

**File keys are tried before program keys, always.** A store that opens with a file never runs the
program another key source names — no `$PATH` walk, no fork, and a cron job on a machine that cannot
reach the other key degrades instead of hanging on it.

What fences the command:

* **argv, never a shell string.** Nothing reaches `/bin/sh`, so there is no quoting layer to get
  wrong and no `$(…)` in a configuration file.
* **Never in a `plugin.*` store.** A plugin's store cannot fork, whichever door the line came
  through — the command refuses to write it, and a hand-edited `secrets.conf` is refused when the
  store is opened.
* **`$OSLO_SECRET_NO_EXEC`**, set to anything non-empty, skips every command source and names it in
  the failure. Exported once by a cron job or a container and inherited by every child, it makes
  *this will not fork* something to assert rather than infer.
* **Standard error is yours.** It is inherited, so a *touch your key* or a PIN prompt reaches the
  terminal instead of being captured.
* **A deadline and a cap.** A command gets the store's `key timeout` (30 seconds unless it says) and
  is killed if it is still running then; one that prints more than 64 KiB is killed too. The same
  deadline bounds the store's `encrypt`/`decrypt command`, and every child is waited for on every
  path out, so none is left a zombie.

### A key is made only when nothing is there

The first `set` on a fresh machine makes a key. It does so **only when every key source answered
"not there"**:

* A source that *failed* — a key file that cannot be read, a link to something that is not a regular
  file, a command that exited non-zero or timed out — stops the write, and the error names that
  source. Writing with the next source instead would seal the file to a key its owner may not have
  to hand; making a new key would be worse.
* **A store with a `key command` never has a key made for it.** That key lives somewhere oslo cannot
  see, and a fresh one would silently fork the store in two.
* A key file is created without replacing anything: it is linked into place, so whatever is already
  at that path — readable or not — is refused rather than destroyed. The profile key is made the same
  way, so two first `set`s racing each other agree on one key.

Reading is more forgiving: a source that fails is passed over while another may open the file, and
named in the error only if none does.

A key file under a `.git` is one `git add -A` from being published, and the person that happens to
did not choose it — they moved a directory a year later. Every `oslo secret` checks each key file
the store reads — every `key file`, `$OSLO_SECRET_IDENTITY`, and the profile's key — and says so:

```
oslo secret: the key /home/you/.local/state/oslo/profiles/default.key is inside the git repository at /home/you
oslo secret: move it (`key file` in secrets.conf, or $OSLO_SECRET_IDENTITY), or the next commit publishes it
```

**That check was measured, because the first version cried wolf.** `~/.git` on the machine it was
written on is an empty directory left behind by something, and `git -C ~ rev-parse` calls it *not a
git repository*. A real one is a directory with `HEAD` in it, or a *file* saying where the directory
is, which is what a worktree and a submodule have.

## What happens on a get

```
oslo secret get stripe
   │
   ├─ read secrets.conf                which store, which keys, which mechanism
   ├─ path("stripe")                   a name is a filename: no `/`, no `..`, no leading `.`,
   │                                   no control character — refused, not sanitised
   ├─ read secrets/stripe.sealed      never through a link, only a regular file, at most 16 MiB
   ├─ not there, or a buried stamp? ─► no such secret
   ├─ OSLO2, and `refuse OSLO2`? ─► refused     before any key is touched
   ├─ file keys, in the order written
   │     └─ none opened it? ─► program keys    only then is anything run
   ├─ a stanza opens under user/stripe          the name and store are its context
   ├─ its writer listed, or this key? ─► else refused, naming the writer
   ▼
XChaCha20-Poly1305 ─► the value on stdout, with nothing added
```

**A store is a directory other people's commits reach**, so nothing in it is trusted to be what its
name says. A secret's file is opened with `O_NOFOLLOW|O_NONBLOCK` and accepted only if it is a
regular file, so a committed `deploy.sealed -> ~/.ssh/authorized_keys` is refused by name, and a
FIFO or a link to `/dev/zero` cannot hang a `get` or a `list`. `list` and a sync look at each entry
without following it and pass over anything that is not a regular file. Key files and `secrets.conf`
are yours to place, so a link to one is followed, and its target must still be a regular file.

**Names.** Besides the rules above, a name may not hold a C0 or C1 control character (newline and
escape among them), `DEL`, or a bidirectional override, and is at most 200 bytes. `list` prints names
escaped, so a file somebody committed under a hostile name cannot write to your terminal. **Names
are bytes**: no Unicode normalisation is applied, so `é` precomposed and `e` with a combining accent
are two different names.

Writing goes the other way. Every file — a secret, a key, the profile key, `secrets.conf` — is
written to a scratch file beside it, `.NAME.<random>.new`, created with `O_CREAT|O_EXCL|O_NOFOLLOW` at
mode `0600`. Its bytes are `fsync`ed, it is renamed over the target (or, for a key, linked into place
without replacing), and the directory is `fsync`ed after. So there is no instant at which a file
exists readable by somebody else, a link planted at any name a write uses is never written through,
and a machine that stops leaves the old file or the new one, whole.

**Two writers at once.** Every read-modify-write — `set`, `rm`, `rotate` and a sync's merge — holds
an exclusive `flock` on the store's `.lock`. Sealing happens before the lock is taken, so a
`crypto hook` that calls back into the same store cannot wait on itself; `rotate` re-checks each
file's stamp under the lock and leaves one that somebody else wrote in the meantime.

**A trailing newline is dropped on `set`.** The value came from a line somebody typed or from a
`printf` in a script, and a token with `\n` on the end fails authentication in a way that takes an
hour to find.

**At a terminal, `set` asks rather than reads.** Standard input there is the keyboard, so reading it
to end of file means the value is typed in the clear, into the scrollback, and finished with a
Ctrl-D nobody is told about. Instead it is the shell's own masked [`ui input`](userin.md).

## The two ways a value reaches a program

```sh
export GITHUB_TOKEN=$(oslo secret get gh-token)      # in this shell, and every child, for good
oslo secret run GITHUB_TOKEN=gh-token -- gh pr list  # in one child, and nowhere else
```

A command substitution puts the value in the *calling shell*, where a `set` prints it — typed like
this the variable is not [sensitive](#sensitive-variables) — and `/proc/PID/environ` holds it for
as long as that shell lives. `run` execs the command directly with one extra variable and no
shell in between. `VAR=` with no name means the secret named after the variable, lowercased and
hyphenated: `oslo secret run GH_TOKEN= -- gh` reads `gh-token`.

### More than one value, and values that are files

```sh
oslo secret run --env-file .env -- ./deploy                 # the variables a dotenv file lists
oslo secret run --all --prefix APP_ -- ./serve              # every secret in the store
oslo secret run --file KUBECONFIG=kube -- kubectl get pods  # a file that is never on disk
```

**`--env-file FILE`** reads a dotenv file that can be committed, because it names secrets rather
than holding them:

```text
# a comment; blank lines are skipped too
REGION=eu-west-1                 a plain value, passed through as it is
export LOG_LEVEL=debug           `export ` in front is allowed
GITHUB_TOKEN=secret:gh-token     the secret gh-token, from the store `run` is using
DEPLOY_KEY=secret:work/deploy    the secret deploy, from the store work
GREETING="hello\tworld"          double quotes: \n \t \r \" and \\ are escapes
PATTERN='a $literal\n'           single quotes: nothing is an escape
NOTE=plain value  # trailing     unquoted: a # after a space or tab starts a comment
```

That is the whole format, and anything else is an error naming the file and line:

* one `NAME=VALUE` per line, spaces or tabs allowed around the `=`; `NAME` is
  `[A-Za-z_][A-Za-z0-9_]*`;
* an unquoted value runs to the end of the line or to a `#` that follows a space or tab, and its
  trailing blanks are dropped; `a#b` keeps its `#`;
* a quoted value closes on the same line, and only a comment may follow it; a backslash before any
  other character inside double quotes stays a backslash;
* nothing is interpolated: `$HOME` is five characters;
* a value that starts with `secret:` names a secret, quoted or not, so there is no way to pass the
  literal text `secret:…` through the file;
* a name set twice in the file is an error, and so is a second `--env-file`;
* the file may be at most 1 MiB, and must be UTF-8.

**`--all`** makes every secret of the store a variable: the name upper-cased with `-` turned into
`_`, after `--prefix` if one is given, so `gh-token` becomes `APP_GH_TOKEN`. A name that does not
make a variable name (`deploy.key`, `1st`), or two names that make the same one (`gh-token` and
`gh_token`), stops the run.

**Precedence.** `VAR=NAME` pairs, the file and `--all` combine. A variable named by more than one
takes the most explicit: a pair over the file, the file over `--all`. A variable named by two pairs
is an error.

**Nothing runs until everything opens.** Every secret is read and decrypted before the command is
exec'd, and the first one that fails — not there, not decryptable, not text — exits with an error
and no child at all.

**`--file VAR=NAME`** hands the value over as a file instead, for what a program wants as a path:
a kubeconfig, a credentials JSON, a TLS key. `run` makes a `memfd_create` file with `MFD_CLOEXEC`
and `MFD_ALLOW_SEALING`, writes the value, seals it with `F_SEAL_WRITE`, `F_SEAL_SHRINK`,
`F_SEAL_GROW` and `F_SEAL_SEAL`, clears close-on-exec on that one descriptor, and sets
`VAR=/dev/fd/N` in the child's environment. The bytes are never on a filesystem, nothing can change
them, and any value works — binary included, where a plain `VAR=NAME` refuses a value that is not
UTF-8 and points at `--file`. The descriptor is the child's, so its own children inherit it unless
the program closes it. `VAR=` with no name means the same default as a plain pair.

What follows `--` is the command's own: a `--store` written after it is handed to the command, not
taken by `oslo secret`.

## Tab

`oslo secret <Tab>` completes the subcommands and their flags — the same rows `--help` prints, so
the two cannot drift — and the operands that have answers on disk:

| | offers |
|---|---|
| `get`, `set`, `rm` | the names in the store the line means: its `--store`, else `$OSLO_SECRET_STORE`, the `default` line, `user` |
| `run VAR=<Tab>`, `run --file VAR=<Tab>` | the same names, after the `=` |
| `run GH<Tab>` | `GH_TOKEN=` for `gh-token` — the `VAR=` that reads back as a name |
| `--store <Tab>` | `user`, the sections of `secrets.conf`, then `stores/*` and `plugins/*.secrets` under the data directory |
| `recipient rm <Tab>` | the store's `recipient` lines in `secrets.conf` |
| `--env-file`, `--from` | file names |

**Nothing is decrypted to complete.** A name is read off a file's name and its one-line header,
never out of its sealed body, so completing needs no key and runs no key command.

## The half that makes it a shell feature

A store you have to remember to call is a store you will paste out of. The other half is a stored
[variable](macros.md#a-variable-holds-a-recipe-not-a-value) whose body is the *recipe*:

```sh
oslo macros add --var 'GITHUB_TOKEN=$(oslo secret get gh-token)'
```

Nothing has run yet. The first time something in a shell reads `$GITHUB_TOKEN`, that line is
evaluated — once, in that shell — and from then on it is an exported variable, marked
[sensitive](#sensitive-variables) because the recipe runs `oslo secret get`. A shell that never
mentions the name never decrypts anything. The same line as an `export` in `init.lua` decrypts at
every shell start, on every machine, for ever, whether or not anything wanted it.

### Sensitive variables

A sensitive variable expands, compares and exports exactly like any other — `echo "$GITHUB_TOKEN"`
prints it and a child is handed it unchanged. What changes is what the shell itself prints about
it:

```console
$ set -x
$ curl -H "Authorization: token $GITHUB_TOKEN" https://api.github.com/user
++ oslo secret get gh-token
+ export GITHUB_TOKEN=<secret>
+ GITHUB_TOKEN=<secret>
+ curl -H 'Authorization: token <secret>' https://api.github.com/user
$ set +x
+ set +x
$ declare -p GITHUB_TOKEN
declare -x GITHUB_TOKEN=<secret>
```

The second and third lines are the materialisation itself.

`set -x`, `set`, `declare -p`, `typeset -p`, `export -p`, `readonly -p` and `local` show `<secret>`
where the value would be. The listings write it unquoted, so a listing saved and read back fails on
the line rather than assigning the placeholder. A trace line hides the value wherever it appears in
the line, and an assignment to a sensitive name is traced as `NAME=<secret>` before its new value
is known.

**Only oslo marks a variable**, so a bash script that never asks for this behaves as bash does:

| what | marks |
|---|---|
| a stored variable whose recipe runs `oslo secret get` or `oslo secret run` | that variable, before the recipe runs |
| a value `oslo.secret.get`, `store:get` or `unseal` handed to Lua, assigned from Lua | the variable it is assigned to — through `oslo.env.set`, a Lua global, or a builtin's `set`/`export` |
| `oslo.env.set(name, value, { secret = true })` | that variable; `secret = false` takes the mark off |

The recipe is matched on its words — `oslo` (or a path ending in `/oslo`), `secret`, an optional
`--store NAME`, then `get` or `run` — anywhere in the body. A value from Lua is recognised by a
keyed hash kept for each value `oslo.secret.*` handed out in this process, never by the value
itself.

The mark belongs to the binding: `unset` drops it, a `local` of the same name starts without one,
and a child process — another oslo included — is handed the value and not the mark. Copying the
value into another variable at the prompt does not mark the copy, though a trace line still hides
the text wherever it matches a sensitive value. A value the shell transforms — `${TOKEN:0:8}`,
`base64` — is not the same text, and a trace prints it.

## From Lua

```lua
oslo.secret.get("gh-token")            -- the store this shell would use
oslo.secret.set("gh-token", value)
oslo.secret.forget("gh-token")
oslo.secret.list()   oslo.secret.stores()

local work = oslo.secret.open("work")  -- another store, by name
work:get("deploy")  work:set("deploy", v)  work:forget("deploy")  work:list()  work:where()

local sealed = oslo.secret.seal("anything")   -- the crypto without the filing
oslo.secret.unseal(sealed)

oslo.env.set("GITHUB_TOKEN", oslo.secret.get("gh-token"))   -- marked sensitive on its own
oslo.env.set("SESSION", computed, { secret = true })         -- or by being asked
```

`seal` and `unseal` are base64 of a whole sealed file, so the config can encrypt something it keeps
somewhere else of its own; a plugin does the same with `oslo.secret.mine()`. Base64 because a
sealed file is binary and a Lua string is UTF-8.

**A seal is not bound to a name.** It has no store name and no secret name to bind to, so it is
bound to the fixed label `oslo.secret.seal` instead: it opens with whichever store's key it was
sealed to, and a caller that needs it tied to something has to keep that binding itself. The label
does keep the two apart: a seal does not open as a stored secret, and an `OSLO3` file from a store
does not open through `unseal`.

### What a plugin may reach

A plugin gets **its own encrypted store**, with no name to pass because the name is not the
plugin's to write. `oslo.secret.mine()` answers only while the plugin's file is loading:

```lua
local mine = oslo.secret.mine()
mine:set("cursor", "42")
```

It lives at `$XDG_DATA_HOME/oslo/plugins/<name>.secrets/`, beside the `.kv` that
[`oslo.db`](plugins.md#the-database) already makes for it, so uninstalling stays an `rm -r`. Its key
is derived from your profile key and the store's name, like every other store's, so there is no
plugin key at rest to lose.

**`plugin.` is refused from Lua, to every caller.** `oslo.secret.open("plugin.notes")` is refused,
and so are the bare `get`, `set`, `forget`, `list`, `seal` and `unseal` when `$OSLO_SECRET_STORE`
names a `plugin.` store — whoever set the variable, and whether the call comes from the config, a
plugin, a hook or the prompt. `oslo.secret.mine()` is the only door to a plugin store from Lua. The
command line has no such rule: `oslo secret --store plugin.notes list` works, because you must be
able to see and remove what a plugin kept on your machine.

For **your** secrets, a plugin reads what your config granted it:

```lua
-- ~/.config/oslo/init.lua, which runs before any plugin
oslo.plugin.secrets("gh", { "gh-token", "gh-host" })   -- names, never a wildcard or a prefix
```

**Only your `init.lua` grants.** `oslo.plugin.secrets` raises an error when it is called while a
plugin's file is loading, and once plugins have started loading — from a hook, a timer or the
prompt — so a plugin cannot grant itself or another plugin anything. A plugin nobody granted
anything reads nothing.

**While a plugin's file loads, it reaches only the names granted to it.** That holds for the bare
calls and for every handle `oslo.secret.open` makes, a store `oslo.secret.define` gave Lua storage
included:

* `get`, `set` and `forget` refuse a name that was not granted;
* `list` answers only the granted names that are there;
* `seal` and `unseal` are refused outright. `unseal` opens no `OSLO3` file of a store, which is
  bound to its store and its name, but it does open an `OSLO2` file sealed to the store's key,
  whatever its name. `mine():seal` and `mine():unseal` stay available, on the plugin's own store.

A handle made during the load keeps that filter for as long as it lives, even when it is stashed in
a global and used later.

The config, the prompt and every hook keep the whole API, `seal` and `unseal` included. A hook or a
timer a plugin registered runs after its load, with nothing to say which plugin it came from, so it
is treated like your own code: **a plugin that defers a call into a hook reaches everything you
do.** The rule above is attribution while a plugin loads, not a sandbox.

**This is a boundary on naming, not a sandbox,** and it has these holes, which nothing here closes:

* a plugin can run `oslo secret get` as a subprocess; oslo
  [declines to sandbox plugins by name](plugins.md#trust);
* a plugin can defer any call into a hook, a timer or a completion callback, as above;
* with `$OSLO_SECRET_IDENTITY` set, every store — a plugin's included — opens with that one key
  file, so a plugin can read an `OSLO2` file of yours with `oslo.fs.read` and open it with
  `mine():unseal`, until `rotate` rewrites it. An `OSLO3` file does not open that way: it is bound
  to its own store and name;
* a plugin can `oslo.secret.define` a store's storage, and so decide what `oslo.secret.open` of
  that name answers in this shell.

## Pluggable: hooks do the crypto, Lua does the storage

Two independent axes, either or both replaceable:

| axis | replaced by | declared in |
|---|---|---|
| **crypto** — what the bytes are | `on-secret-encrypt` / `on-secret-decrypt` | `crypto hook` in `secrets.conf` |
| **storage** — where the bytes live | `oslo.secret.define` | Lua, at load |

```lua
oslo.on["on-secret-encrypt"](function(store, name, sealed)
  if store ~= "vault" then return nil end        -- nil is "not mine"
  return my_own_encryption(sealed)
end)

oslo.secret.define("vault", {
  get    = function(name) return db:get(name) end,
  set    = function(name, sealed) db:set(name, sealed) return true end,
  list   = function() return db:keys() end,
  forget = function(name) db:delete(name) return true end,   -- optional
})
```

Everything crossing to Lua is **base64**, both hooks and both storage handlers.

### Calling `age` — and so a YubiKey — from a hook

A handler is handed base64 and must answer base64, while `age` speaks raw bytes on a pipe. A Lua
string cannot carry those, and putting the payload in argv would publish the plaintext to `ps`, so
this is the one thing a hook cannot build for itself. `oslo.secret.through` is the pipe:

```lua
local recipients = os.getenv("HOME") .. "/.config/age/recipients.txt"   -- has age1yubikey1…
local identity   = os.getenv("HOME") .. "/.config/age/yubikey.txt"

oslo.on["on-secret-encrypt"](function(store, name, sealed)
  if store ~= "yubi" then return nil end
  return oslo.secret.through({ "age", "-R", recipients }, sealed)
end)

oslo.on["on-secret-decrypt"](function(store, name, sealed)
  if store ~= "yubi" then return nil end
  return oslo.secret.through({ "age", "--decrypt", "--identity", identity }, sealed)
end)
```

**Standard error is inherited**, so the plugin's *touch your key* prompt reaches your terminal —
captured, it would not, and the shell would appear to hang for reasons nobody could see. It has 30
seconds and 16 MiB of output, and is waited for on every path out.

**A hook is the right route when you want logic** — different keys per secret, a notification before
the touch, a fallback. **When you only want a program run, use `cipher command`**: it is a
subprocess rather than a hook, so it also works from `cron`, from `dash`, and behind
`$(oslo secret get …)` in a macro variable — none of which run Lua.

### The rule that keeps it honest

**The file declares which mechanism a store uses; Lua supplies the mechanism.**

A hook only exists in a process that ran your config. So `crypto hook` is written in `secrets.conf`,
and a process with no Lua that meets it says so:

```
oslo secret: vault: its crypto is `crypto hook`, and nothing is attached to on-secret-decrypt
here. A hook needs a shell that has read your config; `oslo secret` from a script, a Makefile
or cron never does
```

That check runs *before* anything touches a file, so the reason you get is the real one rather than
"no such file". Declaring `crypto hook` and an `encrypt command` in one store is refused outright —
they have different reach, and quietly picking one is how a store becomes readable in one process
and not another.

### `nil` means "not mine"

A handler that returns `nil` declines and the next one is asked. So several plugins can each serve
their own store off one hook, and a store nobody claims is a **refusal** rather than a fall back to
the built-in mechanism — falling back would seal it with a key the store was never meant to use.

### Watching, without seeing

`pre-secret-access` and `post-secret-access` fire on every read and write, on the native path and the
defined one. They are told the store, the name, and `"read"` or `"write"` — and **never the value**,
because a hook that logs is the likeliest thing anybody writes on them and a log of secrets is worse
than no log.

### What may not be replaced

`oslo.secret.define` refuses `user` and anything under `plugin.`. The first is what the command line
means, and shadowing it would make `oslo.secret.get` and `oslo secret get` disagree about your own
secrets; the second is reached only through `oslo.secret.mine()`.

## Configuration

| | |
|---|---|
| `$XDG_DATA_HOME` | where stores live, under `oslo/`. Falls back to `~/.local/share` |
| `$XDG_STATE_HOME` | where the key and `secrets.conf` live. Falls back to `~/.local/state` |
| `$OSLO_SECRET_IDENTITY` | an absolute path to a key file for every store with no `key` line — beats the profile |
| `$OSLO_SECRET_CONF` | the configuration file, absolutely |
| `$OSLO_SECRET_STORE` | which store this shell and its children mean |
| `$OSLO_SECRET_NO_EXEC` | skip every mechanism that would run a program |

| hook | |
|---|---|
| `on-secret-encrypt` | asked to seal, for a `crypto hook` store. Answers base64, or `nil` for "not mine" |
| `on-secret-decrypt` | the same, the other way |
| `pre-secret-access` | a secret is about to be read or written: store, name, `read`/`write` |
| `post-secret-access` | it just was |
| `oslo.secret.through(argv, base64)` | not a hook: the binary-safe pipe a handler calls to reach another program |

There is no `init.lua` for any of this, and that is the point of the section above: a path read
after Lua has run is a path a `cron` job never sees.

## Measurements

What each half costs, measured on the real binary at the release profile:

```text
  every other feature on   6,640,032 bytes   78 packages
  + secrets (the filing)   6,746,528 bytes   79 packages    +104 KB,  +1 package
  + crypt (the mechanism)  6,807,968 bytes   96 packages    + 60 KB, +17 packages
```

**This used to be `age`, at +160 KB and +32 packages.** What age brought was the file format on top
of the same key model — bech32, scrypt, a localised error catalogue, the recipient types for
hardware and post-quantum. Dropping the format and keeping the model costs 60 KB: `x25519-dalek`
and the curve beneath it, `chacha20poly1305`, and `hkdf` against the `sha2` already here.

An earlier version of `crypt` was one symmetric key at 36 KB and 10 packages. It was 24 KB cheaper
and could not let two machines read one store without copying a private key between them, which is
most of what a secrets store is for.

`OSLO3` — the writer's exchange, the AAD, the `OSLO2` reader beside it, the low-order checks, the
`zeroize` features and the dumpable guard — measured **+16,384 bytes** on the static musl release
binary (5,891,808 to 5,908,192), and one package, `zeroize`, which the crypto crates bring anyway
once their own `zeroize` feature is on.

What a read costs, interleaved, min of five runs of three hundred, against the same binary doing a
`secret list` — same process, same directory, no key opened and nothing decrypted:

| | |
|---|---|
| `oslo secret list` | 598 µs |
| `oslo secret get` | **807 µs** |

So reading the key, the X25519 exchange, the HKDF and opening the payload come to about **209 µs**,
and the rest is starting a process. The exchange is what the symmetric version did not pay; it is
also what lets somebody else read the file.
Delegating to a program instead costs a spawn: `systemd-creds` measured **45,254 µs** on this
machine, which is the price of the flexibility and the reason the built-in mechanism is not simply
deleted.

## What it cannot do

- **Recipients are oslo's own, not age's.** A colleague who already has an age identity cannot be
  added; they publish an `OSLO-PUB-1` half or the store hands its crypto to `age`. There is no
  interoperability with the age ecosystem by design.
- **No passphrase, and no key derivation.** The key is protected by the filesystem, mode `0600`,
  like `~/.ssh/id_ed25519`. A passphrase asked on every read is what teaches people to keep the
  value somewhere else instead.
- **A hook-backed store is unreachable outside a shell that ran your config** — no `cron`, no
  `dash`, no `$(oslo secret get …)` in a macro variable. Stated in the file rather than discovered,
  but a real limit: for anything a script must read, use the built-in mechanism or a command.
- **Nothing here is protected from another plugin.** All Lua runs on one interpreter with one `oslo`
  global, and `oslo.fs` reads any file this user can. A plugin's store is encrypted against the
  *disk*; it is not isolated from other plugins.
- **A grant is not enforcement.** `oslo.proc.exec("oslo secret get gh-token")` walks past it, and
  so does any call a plugin defers into a hook.
- **A sensitive variable is hidden from the shell's own output, not from programs.** `env`,
  `printenv`, `/proc/PID/environ` and the child it is exported to all see the value, and a copy
  made with `B=$TOKEN` is not marked.
- **No key rotation, and no revocation of what is already out.** `rotate` re-encrypts to the list as
  it now stands, so removing a recipient stops them reading anything written *afterwards* — it
  cannot reach a copy of the store they already have. Deleting a key makes everything sealed only to
  it unreadable.
- **At most 255 recipients**, which is one byte in the header and more than anybody has.
- **An older copy is not refused.** Nothing in a file says it is the newest, so somebody who can write
  to your store can put back an earlier genuine version of the same secret, and it opens.
- **A writer on the list is trusted completely.** Every recipient can write, and one that wanted to
  could give two recipients two different values in one file, since the file key is not committed
  to. Listing a key means trusting what it writes.
- **`OSLO2` files present before the first `rotate` were trusted as they were.** `rotate` names them;
  it cannot tell which, if any, were forged.
- **A value is not hidden from the process that asked for it.** If you export it, every child gets
  it. `run` is the narrower door, and it is still an environment.
- **No `git`-aware anything.** The repository check is one warning about where the *key* is.
- **The sync header is not authenticated.** It cannot be — reading it would then need the key, and a
  machine that has no key for a store still has to be able to carry it. Somebody who can write to
  your store can therefore roll a revision back or flip a tombstone. They cannot read anything, and
  write access already meant they could delete the file.
- **A timeout kills the program, not what it started.** A `key command` that is a script running
  something else in the background leaves that something running. `exec` the last program in the
  script.
- **A committed store carries its `.lock`.** It is empty and harmless; put `.lock` in that
  repository's `.gitignore`.
- **On a filesystem without hard links** — vfat on a USB stick — a new key file is created in place
  with `O_EXCL` rather than linked. Two racing writers still agree on one key, but a machine that
  stops mid-write can leave a short file there, which then reads as an error rather than a key.
- **A secret's file that is a symbolic link is refused, not repaired.** Remove the link by hand; the
  write that follows makes an ordinary file.

## Between machines

`oslo profile sync laptop` carries the store, and **nothing is decrypted to do it**. The store key is derived
from the profile key, so two machines that share a profile derive the same one and each can open what
the other wrote; the sealed bodies move exactly as they stand, and the plaintext is never in memory
on either side.

Each file therefore carries a one-line header **outside** the ciphertext:

```text
OSLOSEC1 3 live 9f1c…        (32 hex characters)
<the sealed bytes, exactly as the crypto produced them>
```

A revision, a tombstone flag and a tie-breaker — and nothing about what the secret is. It has to be
outside, because **syncing must not need the key**: a store whose crypto is a YubiKey works only on
the machine the key is plugged into, and the other machine still has to be able to carry its files.

**What that costs, said plainly:** the header is not authenticated, because authenticating it would
mean holding the key to read it. Somebody who can *write* to your store could roll a revision back or
flip a tombstone. They cannot read anything, and somebody with write access could already delete the
file outright.

`oslo secret rm` is a tombstone: the sealed body is dropped and the name stays with its stamp buried,
so the removal travels rather than being undone by the other machine on the next sync. `rm` succeeds
only once that tombstone is on disk. A file written before there were headers still opens, and
counts as never having been synced.

**A damaged header is refused, not guessed at.** It must have its three fields, a state of `live` or
`dead`, 32 hex characters, and a revision no higher than 2^53. A revision near `u64::MAX` would
otherwise pin a name for good — nothing could ever be written over it, and `rm` used to report
success while changing nothing. So `get`, `set` and `rm` on such a file fail and name it, and a sync
reads every file on both sides before writing any, so one damaged header stops the merge with
nothing changed. A write that would take a revision past the bound is refused the same way.

[Syncing between machines](syncing.md) has the rest, including the bundle format and why it is not
tar.

## Where it lives

| | |
|---|---|
| `crates/oslo-base/src/secrets.rs` | `Store`: seal, unseal, the key sources, and what a tombstone is |
| `crates/oslo-base/src/secrets/write.rs` | a write under the store's lock, and `rotate` |
| `crates/oslo-base/src/private.rs` | every private file: scratch, fsync, rename or link; reads that take only a regular file; the lock |
| `crates/oslo-base/src/secrets/name.rs` | what a secret may be called, and how a name is printed |
| `crates/oslo-base/src/secrets/child.rs` | running a key or cipher command: stderr inherited, a deadline, a cap |
| `crates/oslo-base/src/secrets/place.rs` | where the store is, where the key is, and why those differ |
| `crates/oslo-base/src/secrets/sync.rs` | the file header, and merging two stores without a key |
| `crates/oslo-base/src/secrets/native.rs` | the sealed box: the writer, the wrap, the AAD, the key file, the format |
| `crates/oslo-base/src/secrets/native/legacy.rs` | reading `OSLO2`, the format before writers were authenticated |
| `crates/oslo-base/src/secrets/natively.rs` | a store's part: which key writes, which writers it accepts, which refusal it reports |
| `crates/oslo-base/src/secrets/dumpable.rs` | no core file and no `ptrace` while a key is in memory |
| `crates/oslo-base/src/secrets/recipient.rs` | who a store is written for, and what a build makes of one |
| `crates/oslo-base/src/secrets/conf.rs` | `secrets.conf`: parsed, and edited line-wise |
| `crates/oslo-base/src/secrets/key.rs` | `KeySource`: the profile, a file, or a program — and what fences the program |
| `crates/oslo-base/src/track/profile/key.rs` | the profile key every store derives from |
| `crates/oslo-base/src/secrets/crypto.rs` | the three mechanisms, and why only one can be unreachable |
| `crates/oslo-base/src/secrets/cipher.rs` | handing encryption and decryption to another program |
| `crates/oslo-base/src/secrets/hooked.rs` | asking Lua to do the crypto, and telling it what was touched |
| `crates/oslo-runtime/src/lua/api/secret.rs` | `oslo.secret`, and what a plugin's handle may reach |
| `crates/oslo-runtime/src/lua/api/secret/defined.rs` | `oslo.secret.define` — a store whose bytes are Lua's |
| `crates/oslo-runtime/src/plugin/mod.rs` | the grant, and when it is refused |
| `crates/oslo-shell/src/env/scope/sensitive.rs` | the sensitive mark, and what recognises a secret |
| `src/cli/secret.rs` | the command line, with `key`, `cipher` and `run` beside it |
| `src/cli/secret/run/` | `run`'s flags, the env file, and the sealed memfd |
| `src/cli/complete/secret.rs` | what `oslo secret <Tab>` offers |
