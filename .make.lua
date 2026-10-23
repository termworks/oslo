-- oslo's own build, as recipes.
--
-- **This replaced the `Makefile`, which is gone.** The two said the same thing in two places, and
-- keeping both meant every change had to be made twice or the second one quietly rotted.
--
-- What a `Makefile` was still needed for is the bootstrap: this file is run by the `make` builtin,
-- which lives inside the shell it builds, so a fresh checkout cannot start here. `scripts/build.sh`
-- is that one rung — cargo and nothing else — and it builds the binary and stops. Everything past
-- the first build is a recipe. See `docs/features/build-recipes.md`.
--
--   oslo make            the recipes, with what each of them says it does
--   oslo make build      the static release binary
--   oslo make verify     the whole local gate
--
-- At an oslo prompt in this directory, `make` is enough — the builtin hands the word over to the
-- program everywhere else. See `crates/oslo-shell/src/env/builtins/make.rs`.

local make = oslo.make

---------------------------------------------------------------------------- what the build is

-- Shared with `scripts/build.sh`, which reads the same two lines from the same script — so the
-- bootstrap and the recipes cannot disagree about what they are building.
local meta = oslo.run{ "./scripts/project-meta.sh", capture = true }
assert(meta.ok, "scripts/project-meta.sh: " .. (meta.err or "failed"))
local NAME, VERSION = meta.out:match("(%S+)%s+(%S+)")
assert(NAME, "no name in Cargo.toml; is this an oslo checkout?")

local TARGET = os.getenv("TARGET") or "x86_64-unknown-linux-musl"
local PREFIX = os.getenv("PREFIX") or (os.getenv("HOME") .. "/.local")
local BIN = ("target/%s/release/%s"):format(TARGET, NAME)

-- oslo's own crates, and not the vendored ones. The crates under `vendor/` carry upstream test
-- modules whose dev-dependencies were never vendored, so a plain `--workspace` fails to compile
-- before it runs anything of ours.
--
-- Only what `[workspace] members` actually lists: `--exclude` for a name that is not a member is
-- a warning cargo prints and nothing else, so `full_moon` and `full_moon_derive` sat here saying
-- nothing long after the vendored Lua parser left.
local OURS = { "--workspace", "--exclude", "argc" }

-- Every `.rs` the build depends on, for the recipes that declare staleness.
local SOURCES = { "src/**/*.rs", "crates/**/*.rs", "Cargo.toml", "Cargo.lock" }

-- `cargo(...)` with lists spliced in wherever they appear.
--
-- **This exists because of a real Lua trap, and it bit this file first.** A call that expands to
-- several values is truncated to one everywhere but the last position, so writing the exclusions
-- as `cargo("clippy", ..., table.unpack(OURS), "--", "-D", "warnings")` passed `--workspace` and
-- silently dropped every `--exclude` — clippy then tried to compile the vendored crates and died
-- on their missing dev-dependencies. The same truncation is written up in
-- `docs/features/directory-environments.md` for table constructors; arguments have it too.
--
-- Splicing lists instead means the trap cannot be written down: a table is one value wherever it
-- appears, and this is what flattens it.
local function flat(...)
  local out = {}
  for _, item in ipairs({ ... }) do
    if type(item) == "table" then
      for _, word in ipairs(item) do out[#out + 1] = word end
    else
      out[#out + 1] = item
    end
  end
  return out
end

local function cargo(...)
  return sh.cargo(table.unpack(flat(...)))
end

-- Whether this process can already write `/usr/bin`, so a root build does not shell out to a `sudo`
-- that may not be installed. Asked of the kernel rather than of `$USER`, which a `su` leaves stale.
local function as_root()
  return oslo.run{ "id", "-u", capture = true }.out:match("^0%s*$") ~= nil
end

local function absolute(path)
  if oslo.path.is_absolute(path) then return oslo.path.normalize(path) end
  return oslo.path.normalize(oslo.path.join(oslo.fs.cwd(), path))
end

-- Whether `dir` is somewhere `$PATH` already looks.
--
-- Compared as absolute paths, because `$PATH` carries whatever spelling was put in it and
-- `./target/…` and `/home/…/target/…` are the same directory.
local function on_path(dir)
  local want = absolute(dir)
  for entry in ((os.getenv("PATH") or "") .. ":"):gmatch("([^:]*):") do
    if entry ~= "" and absolute(entry) == want then return true end
  end
  return false
end

-- `6423296` → `6,423,296`. A number this long is read in groups or not at all.
local function grouped(n)
  local text = tostring(math.floor(n))
  local out = text:sub(-3)
  local at = #text - 3
  while at > 0 do
    out = text:sub(math.max(1, at - 2), at) .. "," .. out
    at = at - 3
  end
  return out
end

-- Painted through the shell's own styling, so `NO_COLOR` and a pipe both answer the plain text and
-- the caller never checks. See `docs/features/drawing.md`.
local function dim(text)
  return oslo.ui.style(text, { dim = true })
end

local function line(label, value)
  print(dim(oslo.ui.pad(label, 8)) .. value)
end

-- Where the binary is, what it weighs, and whether you can actually run it by name.
--
-- The last row is the one that earns its place: a build that succeeded and a `$PATH` that does not
-- reach it look identical until you type the name and get the *old* one from somewhere else.
local function report(path)
  local stat = oslo.fs.stat(path)
  if not stat then return end
  local dir = oslo.path.parent(path)
  local megabytes = ("%.2f MB"):format(stat.size / 1048576)

  print("")
  print(oslo.ui.title(("%s %s   %s"):format(NAME, VERSION, megabytes)))
  line("binary", path)
  -- Bytes beside megabytes: the README argues about kilobytes, and `6.16 MB` cannot be subtracted
  -- from last week's `6.13 MB` to get one.
  line("size", megabytes .. dim("   " .. grouped(stat.size) .. " bytes"))
  if on_path(dir) then
    line("path", oslo.ui.style("✓ on $PATH", { fg = "green" }) .. dim("  " .. dir))
  else
    line("path", oslo.ui.style("✗ not on $PATH", { fg = "yellow" }))
    print(oslo.ui.subtitle(('         add to .env.lua:  oslo.direnv.path_add("%s")'):format(dir)))
  end
  print("")
end

---------------------------------------------------------------------------- building

-- The version is asked for, not announced. A build tool that prints a banner on every target prints
-- it into the output of the ones meant to be piped, too. A recipe is the honest place for it: ask,
-- and it answers.
make.recipe{ name = "version", desc = "what this checkout calls itself",
             run = function() print(("%s v%s"):format(NAME, VERSION)) end }

-- **Two recipes, because a skipped recipe prints nothing.**
--
-- `_release` declares the outputs, so it is the one that can be up to date — and when it is, its
-- body never runs. Everything worth saying about the artifact was in that body, so `oslo make build`
-- on an unchanged tree said `· build  up to date` and not one word about the binary. `build` itself
-- declares no outputs, so it is phony, so it always runs and always reports.
--
-- The parameter is handed over explicitly rather than through `deps`: a dependency is executed with
-- an empty argv (`make.lua`'s `execute(step, step.name == name and argv or {})`), so `--type minimal`
-- listed as a dep would be silently dropped and a full build done instead.
make.recipe{
  name = "build",
  desc = "the static release binary, all features (--type minimal for none)",
  params = { { "--type", desc = "minimal | full", default = "full" } },
  run = function(a)
    make.run("_release", { "--type", a.type })
    report(BIN)
  end,
}

make.recipe{
  name = "_release",
  desc = "compile the static release binary",
  deps = { "_static-check-deps" },
  inputs = SOURCES,
  outputs = { BIN },
  stale = "content",
  params = { { "--type", desc = "minimal | full", default = "full" } },
  run = function(a)
    -- A login shell linked against a /nix/store glibc stops existing the day
    -- `nix-collect-garbage` runs, and there is no recovering from that from inside the session it
    -- breaks. So the release is one file that runs anywhere.
    --
    -- **The same flags CI releases with**, so what is built here is what ships. They had drifted:
    -- this used `relocation-model=static`, which drops the PIE relocation machinery and 300 KB
    -- with it, and gives up ASLR of oslo's own image to do it. `pack-relative-relocs` stores the
    -- same relocations as a bitmap instead and costs 29,344 bytes against that — which is a
    -- cheaper price for address-space layout randomisation than any other line in this file.
    --
    -- Safe *because* the binary is static-pie: the code that reads `.relr.dyn` is musl's own
    -- startup, linked in rather than found on the machine, so there is no loader on the target to
    -- be older than the format.
    --
    -- **And the default linker, deliberately.** CI linked with mold for `--icf=safe`, which folds
    -- identical functions and is worth 55 KB of `.text` — and left a 688,335-byte hole between
    -- `.dynstr` and `.relr.dyn` that no section owns, for a net loss of 623 KB. GNU ld's largest
    -- gap in the same binary is 3,913 bytes.
    oslo.env.set(
      "RUSTFLAGS",
      "-C target-feature=+crt-static -C link-arg=-Wl,-z,pack-relative-relocs"
    )
    local features = a.type == "minimal" and {} or { "--all-features" }
    cargo("build", "--release", "--target", TARGET, "--bin", NAME, features)
    -- **Unwinding tables for a binary that cannot unwind.** `panic = "abort"` means no landing pad
    -- is ever reached, and nothing in the tree asks for a backtrace — which `strip = "symbols"`
    -- has already made nameless in any case. That leaves half a megabyte, 11% of the binary,
    -- describing frames no one walks. `-C force-unwind-tables=no` does not remove it: almost all
    -- of it comes from the precompiled `std` and the crt, so it goes after the link.
    --
    -- Verified rather than assumed: `.text`, `.rodata` and `.data` come out byte-identical, and
    -- the program headers stay valid — only `PT_GNU_EH_FRAME` is left empty, and its one reader is
    -- the unwinder this profile does not have.
    oslo.run{ "objcopy", "--remove-section=.eh_frame", "--remove-section=.eh_frame_hdr",
              "--remove-section=.gcc_except_table", BIN }
    make.run("check-static")
    -- The reporting is `build`'s, not this one's: this recipe is the half that gets skipped.
  end,
}

make.alias("b", "build")

make.recipe{
  name = "check-static",
  desc = "fail if the release ELF asks for a loader",
  run = function()
    -- "Static" is a claim about the ELF, so check the ELF. `ldd` is not enough: it prints
    -- "statically linked" for a musl binary that still has an INTERP and will not start.
    local segments = oslo.run{ "readelf", "-l", BIN, capture = true }
    assert(not segments.out:find("program interpreter"),
           BIN .. " requests a dynamic loader; it is not static")
    local dynamic = oslo.run{ "readelf", "-d", BIN, capture = true }
    assert(not (dynamic.out or ""):find("NEEDED"),
           BIN .. " has NEEDED entries; it is not static")
    print("static: no INTERP, no NEEDED")
  end,
}

-- Nothing to do, and that is the point: `build` depends on it so that a missing toolchain is one
-- clear message rather than a wall of linker output.
make.recipe{
  name = "_static-check-deps",
  desc = "the tools the static link needs",
  run = function()
    for _, tool in ipairs({ "cargo", "readelf", "objcopy" }) do
      -- `capture` so the path does not land on stdout: this runs before every build, and two
      -- `/nix/store/…` lines are not what anybody asked `build` for.
      assert(oslo.run{ "sh", "-c", "command -v " .. tool, capture = true }.ok,
             tool .. " is not installed, and the static build needs it")
    end
  end,
}

make.recipe{ name = "dev", desc = "the fast inner loop: a debug build",
             run = function() cargo("build") end }

make.recipe{ name = "clean", desc = "remove the Cargo artifacts",
             run = function() cargo("clean") end }

make.recipe{ name = "compile", desc = "clean, then build", deps = { "clean", "build" } }
make.alias("c", "compile")

---------------------------------------------------------------------------- the gate

make.recipe{ name = "check", desc = "cargo check, every target",
             run = function() cargo("check", "--all-targets", OURS) end }

make.recipe{ name = "check-all", desc = "cargo check, every target and every feature",
             run = function() cargo("check", "--all-targets", "--all-features") end }

-- `--all-features`, like `check` and `clippy` beside it. Without it the tests behind a feature are
-- compiled and then never *run*, so a suite that looked green covered neither.
make.recipe{ name = "test", desc = "the suite: every target, every feature, our crates",
             run = function() cargo("test", "--all-targets", "--all-features", OURS) end }
make.alias("t", "test")

make.recipe{ name = "test-terminal", desc = "the PTY transcript tests alone",
             run = function() cargo("test", "--test", "terminal_semantics_tests") end }

make.recipe{ name = "clippy", desc = "clippy, warnings denied",
             run = function()
               cargo("clippy", "--all-targets", "--all-features", OURS, "--", "-D", "warnings")
             end }

make.recipe{ name = "fmt", desc = "format the workspace",
             run = function() cargo("fmt", "--all") end }

make.recipe{ name = "fmt-check", desc = "fail if anything is unformatted",
             run = function() cargo("fmt", "--all", "--", "--check") end }

make.recipe{ name = "rustdoc", desc = "build the docs, warnings denied",
             run = function()
               oslo.env.set("RUSTDOCFLAGS", "-Dwarnings")
               cargo("doc", "--all-features", "--no-deps", OURS)
             end }

make.recipe{ name = "check-loc", desc = "fail if any source file exceeds 600 lines",
             run = function() sh.sh("./scripts/check-loc.sh") end }

make.recipe{ name = "check-readme", desc = "fail if the readme names a file that does not exist",
             run = function() sh.sh("./scripts/check-readme.sh") end }

make.recipe{ name = "check-fork", desc = "fail if any fork bypasses the forksafe gate",
             run = function() sh.sh("./scripts/check-fork.sh") end }

make.recipe{
  name = "verify",
  desc = "the whole local gate",
  deps = { "fmt-check", "check-loc", "check-readme", "check-fork", "check", "test", "clippy", "rustdoc" },
}

---------------------------------------------------------------------------- installing

make.recipe{
  name = "install",
  desc = "put the release binary in $PREFIX/bin and in /usr/bin, and config/ where it reads it",
  deps = { "build" },
  params = { { "--system", desc = "yes | no — also install to /usr/bin", default = "yes" } },
  run = function(a)
    local dest = (os.getenv("DESTDIR") or "") .. PREFIX .. "/bin"
    sh.install("-d", dest)
    sh.install("-m", "755", BIN, dest .. "/" .. NAME)
    print(oslo.ui.style("✓ ", { fg = "green" }) .. dest .. "/" .. NAME)

    -- A branch rather than an early return, so what follows it runs either way. `--system no`
    -- skips the system copy, not the configuration.
    if a.system ~= "no" then
      -- **`$SHELL` lives here.** A login shell is started from `/etc/passwd`, which names an
      -- absolute path, so a copy under `$HOME` is the one you run by name and the system one is
      -- the one you *are*. Installing only the first is how a fixed shell keeps not being fixed.
      local system = (os.getenv("DESTDIR") or "") .. "/usr/bin/" .. NAME
      local sudo = as_root() and {} or { "sudo" }
      local put = oslo.run{ table.unpack(flat(sudo, "install", "-m", "755", BIN, system)) }
      if put.ok then
        print(oslo.ui.style("✓ ", { fg = "green" }) .. system)
        -- The `(deleted)` inode is not cosmetic: `current_exe` is how the `make` builtin finds the
        -- runner, so a shell whose binary was replaced under it answers `make: cannot execute`.
        print(oslo.ui.subtitle("  shells already running keep the old binary — restart them"))
      else
        print(oslo.ui.style("✗ ", { fg = "yellow" }) .. system ..
              oslo.ui.subtitle("  (not installed; --system no to skip)"))
      end
    end

    -- Last, and part of the install rather than a step to remember: a binary newer than the config
    -- it reads is how a setting that shipped together with it silently does nothing. Run alone,
    -- `configs` still installs only the config.
    make.run("configs")
  end,
}

make.recipe{
  name = "uninstall",
  desc = "take it back out of $PREFIX/bin and /usr/bin",
  params = { { "--system", desc = "yes | no — also remove /usr/bin", default = "yes" } },
  run = function(a)
    -- Both, because `install` puts it in both: an uninstall that left the system copy behind would
    -- leave you running the thing you just removed.
    local dest = (os.getenv("DESTDIR") or "") .. PREFIX .. "/bin/" .. NAME
    sh.rm("-f", dest)
    print(oslo.ui.style("✓ ", { fg = "green" }) .. "removed " .. dest)

    if a.system == "no" then return end
    local system = (os.getenv("DESTDIR") or "") .. "/usr/bin/" .. NAME
    if not oslo.fs.stat(system) then return end
    local sudo = as_root() and {} or { "sudo" }
    local gone = oslo.run{ table.unpack(flat(sudo, "rm", "-f", system)) }
    print(gone.ok
      and oslo.ui.style("✓ ", { fg = "green" }) .. "removed " .. system
      or oslo.ui.style("✗ ", { fg = "yellow" }) .. system .. oslo.ui.subtitle("  (still there)"))
  end,
}

---------------------------------------------------------------------------- the VMs

-- Deliberately not in `verify`: each needs a musl toolchain, qemu and the network, and takes
-- minutes. They answer questions a checkout cannot — whether the artifact runs as PID 1 on a
-- foreign userland, and whether a distro's own init system runs on it.
--
-- The two distros disagree with each other on purpose. Alpine is musl, OpenRC and a busybox
-- `/bin/sh`; Arch is glibc, systemd and a *bash* `/bin/sh`, so standing in for it is a
-- bash-compatibility test rather than a POSIX one.
for _, vm in ipairs({
  { "vm", "scripts/alpine-vm.sh", "boot oslo as PID 1 in an Alpine minirootfs" },
  { "vm-distro", "scripts/alpine-distro-vm.sh", "a real Alpine userland with oslo as /bin/sh" },
  { "vm-arch", "scripts/arch-vm.sh", "glibc, systemd, and a bash /bin/sh" },
}) do
  make.recipe{ name = vm[1], desc = vm[3], run = function() sh.bash(vm[2]) end }
end

---------------------------------------------------------------------------- releasing

---------------------------------------------------------------------------- configuration

-- **The completions are generated, and committed.** `share/completion` holds one carapace spec
-- per command, converted from Fig's TypeScript and from argc's annotated shell scripts — ~1,170
-- commands, 17MB on disk and 2.2MB packed. They are in the repository rather than fetched at
-- install time because a shell that completes `kubectl` only after a network call is a shell that
-- does not complete `kubectl`.
--
-- This regenerates them, and is run when upstream moves rather than as part of any build. It needs
-- `git` and `bun`; nothing else in the tree does, which is why it is a recipe and not a step.
make.recipe{
  name = "completion",
  desc = "regenerate share/completion from the upstream corpora",
  params = { { "--with-giants", flag = true, desc = "include aws and gcloud (3.6MB packed, two commands)" } },
  run = function(a)
    local args = { "./scripts/completion.sh" }
    if a.with_giants then table.insert(args, "--with-giants") end
    sh.sh(table.unpack(args))
  end,
}

---------------------------------------------------------------------------- configuration
-- oslo's own configuration lives in `config/`, and this installs it: `config/*` becomes
-- `~/.config/oslo/*`. The shell reads `init.lua` from there on startup, so this is how a checkout's
-- configuration becomes the one a running shell uses.
--
-- **`share/` goes to the data directory, and that separation is load-bearing.** Each entry is
-- mirrored with `--delete`, which is right for something oslo owns and catastrophic for a directory
-- you keep your own work in. The generated completions are oslo's, so they install to
-- `$XDG_DATA_HOME/oslo`; `~/.config/oslo/completion` is yours and this never touches it.
make.recipe{
  name = "configs",
  desc = "install config/ and share/ into their XDG directories",
  params = { { "--dest", desc = "somewhere other than the config directory" } },
  run = function(a)
    assert(oslo.run{ "sh", "-c", "command -v rsync", capture = true }.ok,
           "rsync is not installed; install it first")
    -- Asked of git rather than assumed from the working directory, so this works from anywhere in
    -- the tree. Outside a repository, where the command was run is the best answer available.
    local top = oslo.run{ "git", "rev-parse", "--show-toplevel", capture = true }
    local root = top.ok and (top.out or ""):match("^%s*(.-)%s*$") or ""
    if root == "" then root = oslo.sys.pwd() end
    assert(oslo.fs.stat(root .. "/config/"), "there is no config/ directory in " .. root)

    local function home(var, fallback)
      local dir = os.getenv(var)
      if not dir or dir == "" then dir = os.getenv("HOME") .. fallback end
      return dir .. "/" .. NAME
    end

    -- One entry at a time, each mirrored with --delete, rather than one --delete over the whole
    -- tree: the destination is where anything else you keep beside init.lua lives, and a tree-wide
    -- mirror would take it with it.
    local function install(source, dest)
      if not oslo.fs.stat(source .. "/") then return 0 end
      sh.mkdir("-p", dest)
      local synced = 0
      for _, path in ipairs(oslo.fs.glob(source .. "/*")) do
        local name = oslo.path.name(path)
        if oslo.fs.stat(path .. "/") then
          sh.mkdir("-p", dest .. "/" .. name)
          sh.rsync("-a", "--delete", path .. "/", dest .. "/" .. name .. "/")
        else
          sh.rsync("-a", path, dest .. "/" .. name)
        end
        synced = synced + 1
      end
      print(oslo.ui.style("✓ ", { fg = "green" }) ..
            ("%d entr%s -> %s"):format(synced, synced == 1 and "y" or "ies", dest))
      return synced
    end

    install(root .. "/config", a.dest or home("XDG_CONFIG_HOME", "/.config"))
    -- Only when `--dest` was not given: a caller redirecting the config has not asked for the data.
    if not a.dest then
      install(root .. "/share", home("XDG_DATA_HOME", "/.local/share"))
    end
    print(oslo.ui.subtitle("  anything else in those directories is left alone"))
  end,
}

make.recipe{
  name = "release",
  desc = "cut a version: --type patch | minor | major | M.m.p",
  params = { { "--type", desc = "patch | minor | major | M.m.p" } },
  run = function(a)
    assert(oslo.run{ "sh", "-c", "command -v git-rel" }.ok,
           "git-rel is not installed; install it first")
    assert(type(a.type) == "string",
           "which release? oslo make release --type patch|minor|major|M.m.p")
    sh.git("rel", a.type)
  end,
}

---------------------------------------------------------------------------- the corpus

-- **Never run the corpus from the repository root.** These are real scripts and many of them
-- create files where they are run; doing it by hand has twice left ~70 stray files in the tree,
-- and one of them dropping a file called `f` changes what a *different* script does afterwards.
make.recipe{
  name = "corpus",
  desc = "time the script corpus, in a scratch directory",
  run = function()
    cargo("build", "--release")
    local root = oslo.fs.cwd()
    local dir = oslo.fs.mktempdir()
    for _, script in ipairs(oslo.fs.glob("tests/corpus/*.sh")) do
      oslo.fs.copy(script, dir .. "/" .. oslo.fs.name(script))
    end
    local scripts = oslo.fs.glob(dir .. "/*.sh")
    local began = os.time()
    for _, script in ipairs(scripts) do
      oslo.run{ root .. "/target/release/" .. NAME, script }
    end
    print(("corpus: %d scripts in %ds"):format(#scripts, os.time() - began))
    oslo.fs.remove(dir)
  end,
}
