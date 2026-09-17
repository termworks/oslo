# `oslo make` runs in a different environment than the shell it's executing from

## Summary

`oslo make <recipe>` (and, by extension, plain `make <recipe>` typed at an oslo prompt)
does not put the project's `.env.lua` environment in effect the way an interactive `cd`
into the same directory does. Two separate gaps combine to produce this:

1. **`.env.lua` is not evaluated at all during `oslo make <recipe>`.** It only runs as
   part of the interactive shell's own directory-entry hook.
2. **`oslo.env.set()` does not mutate the current process's live environment.** It only
   queues a value to export to the *calling* shell once the current script finishes and
   returns — so even code that *does* run inside `.env.lua` cannot see a variable that an
   earlier line of that same script just set with `oslo.env.set()`.

Either gap alone is surprising; together they mean **a `.make.lua` recipe run via
`oslo make` cannot rely on anything `.env.lua` computes**, and **`.env.lua` cannot use a
value it computes for itself within the same evaluation pass**. The environment a recipe
actually runs in silently diverges from the environment the directory declares.

## Reproduction

### 1. `.env.lua` is never evaluated by `oslo make`

```console
$ cat .env.lua
error("ENV_LUA_DEFINITELY_RAN")

$ cat .make.lua
local make = oslo.make
make.recipe{ name = "t", desc = "noop", run = function() print("ran") end }

$ oslo make t
→ t
ran
```

An unconditional `error()` at the top of `.env.lua` should abort immediately if the file
is loaded at all. It never fires. `oslo make t` succeeds and prints `ran` as if
`.env.lua` did not exist.

### 2. `oslo.env.set()` doesn't take effect within the same script

```console
$ cat .make.lua
local make = oslo.make
make.recipe{ name = "t", desc = "check same-script visibility", run = function()
  print("before: " .. tostring(os.getenv("MY_VAR")))
  oslo.env.set("MY_VAR", "hello")
  print("after oslo.env.set, os.getenv: " .. tostring(os.getenv("MY_VAR")))
  print("after oslo.env.set, oslo.env.get: " .. tostring(oslo.env.get("MY_VAR")))
  local r = oslo.run{ "sh", "-c", "echo shell-sees=$MY_VAR", capture = true }
  print("subprocess sees: " .. tostring(r.out))
end }

$ oslo make t
→ t
before: nil
after oslo.env.set, os.getenv: nil
after oslo.env.set, oslo.env.get: hello
subprocess sees: shell-sees=
```

`oslo.env.get()` sees the new value immediately (so oslo is tracking it *somewhere*), but
neither `os.getenv()` — a real libc call — nor a subprocess spawned via `oslo.run{}` sees
it. The value only reaches a real process environment variable later, presumably once the
whole `.env.lua`/`.make.lua` evaluation finishes and oslo diffs its internal tracked state
against what it exports to the parent shell.

### Compounded: a value can't reach `nix_develop()` in the same file

```lua
-- .env.lua
oslo.env.set("MY_PROBE_VAR", "detected123")
oslo.direnv.nix_develop()
```

```nix
# flake.nix
{
  outputs = { self, nixpkgs }: let
    v = builtins.getEnv "MY_PROBE_VAR";
  in {
    devShells.x86_64-linux.default =
      (import nixpkgs { system = "x86_64-linux"; }).mkShell {}
      // (if v != "detected123" then throw "MISMATCH got=[${v}]" else {});
  };
}
```

Expected: the `mkShell` derivation evaluates with `v = "detected123"`, no throw.
Actual: `nix_develop()` either doesn't see the flake as changed (gap #1 territory — this
whole file may not run for `oslo make`) or `builtins.getEnv` reads an empty string (gap
#2 — `oslo.env.set()` hasn't reached the real environment yet when `nix_develop()`'s own
subprocess evaluates the flake). We were unable to get a value set earlier in `.env.lua`
in front of a `nix_develop()` call later in that same file, by any means.

## Real-world impact

A project's `flake.nix` decides, once, via Nix's impure `builtins.getEnv`, which of two
GPU-vendor wrapper packages a convenience alias should point at (based on whatever the
directory's `.env.lua` detects about the host's hardware). This is exactly the shape of
thing `.env.lua` is supposedly for: computing something about the environment once per
directory-entry and making it available to the tools that directory's commands use. It
cannot be made to work:

- Setting the value in `.env.lua` before calling `oslo.direnv.nix_develop()` — the
  documented way to influence devshell evaluation — never reaches the Nix evaluation,
  because of gap #2.
- Even if it did, `oslo make run` (the actual command a user runs) doesn't evaluate
  `.env.lua` again to pick it up — gap #1 — so the *interactive shell's* cached
  devshell (built whenever `.env.lua` last ran, correctly or not) is what a later
  `oslo make run` actually gets, with no way for that command to correct a bad value.
- The failure mode is silent and looks like flaky hardware detection: the same script,
  on the same machine, computes the right answer every time it's tested standalone
  (`oslo make <recipe>` from a plain shell — which incidentally also never runs
  `.env.lua`, so any apparent success there is coincidental, e.g. a leftover real
  environment variable from something else entirely) and the wrong one every time a real
  user runs it from their actual interactive session.

## What we'd expect instead

Pick one (both would be reasonable, together they'd remove the surprise entirely):

1. `oslo make <recipe>` (and any other one-shot `oslo <subcommand>`) evaluates
   `.env.lua` for the current directory tree first, the same way entering the directory
   interactively would, so a recipe runs in the environment the directory declares
   rather than whatever was last cached by an unrelated interactive session.
2. `oslo.env.set()` (and `unset`) mutate the calling process's real environment
   immediately — a real `setenv(3)`, not a deferred export — so that code later in the
   *same* script (including a subsequent `oslo.direnv.nix_develop()` call) observes it,
   and so that `os.getenv()`/subprocesses spawned via `oslo.run{}` see it right away
   too. The current "queue it for the parent shell" behavior is presumably intentional
   for the *interactive* case (so the shell prompt's own env updates atomically once
   `.env.lua` finishes), but it should not be the *only* effect — the running oslo
   process should also see its own change.

## Workaround in use today

We stopped routing anything time-sensitive through `.env.lua`/`oslo.env.set()`. The
`.make.lua` recipe that needs a freshly-detected value in front of a Nix evaluation now
does it all in one self-contained POSIX shell script, invoked via `sh.sh("-c", ...)`:
detect the value, `export` it as a real shell variable, and call `nix develop --impure -c
...` *in that same script* — a real `export` immediately followed by a real child-process
invocation reliably passes the value through, which is what led us to isolate this as an
oslo-level gap rather than a Nix or project-config problem.
