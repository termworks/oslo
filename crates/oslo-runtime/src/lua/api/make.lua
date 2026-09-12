-- oslo.make, the recipe runner — written in Lua so that it can be read and replaced.
--
-- Everything a `.make.lua` talks to is here. The Rust half is four accessors and the exit status;
-- see `make.rs`. A config or a project overwrites any of these by assigning to `oslo.make.<name>`.
--
-- The vocabulary, in one place:
--
--   recipe    a named piece of work: deps, a body, and optionally what it reads and writes
--   phony     a recipe with no `outputs` — it always runs, which is the default
--   stale     an output missing, or older than an input; the only reason a non-phony recipe runs
--   private   a name beginning with `_`; runnable, but left out of the listing

return function(make)
  local recipes, order, aliases = {}, {}, {}
  local imported = { make.__file() }
  local settings = { quiet = false, keep_going = false, stale = "mtime" }

  -- A message that is oslo's rather than Lua's: level 0, so no file:line is prepended. A recipe
  -- body raising on its own still gets the traceback, which is the case where it helps.
  local function fail(fmt, ...)
    error("oslo make: " .. string.format(fmt, ...), 0)
  end

  local function is_list(v) return type(v) == "table" end

  -- A list of strings, from a field that is allowed to be absent or to be one string.
  local function strings(value, what)
    if value == nil then return {} end
    if type(value) == "string" then return { value } end
    if not is_list(value) then fail("%s must be a string or a list of them", what) end
    local out = {}
    for i = 1, #value do
      if type(value[i]) ~= "string" then
        fail("%s[%d] must be a string, got %s", what, i, type(value[i]))
      end
      out[i] = value[i]
    end
    return out
  end

  ---------------------------------------------------------------------------- declaring

  function make.settings(t)
    if type(t) ~= "table" then fail("settings expects a table") end
    for k, v in pairs(t) do settings[k] = v end
    return settings
  end

  -- The declaration. Nothing here runs; `run` is kept as a function and called later, which is
  -- what lets the listing and the completion cache read a file without executing its work.
  function make.recipe(spec)
    if type(spec) ~= "table" then fail("recipe expects a table") end
    local name = spec.name
    if type(name) ~= "string" or name == "" then
      fail("a recipe needs a name")
    end
    if name:find("%s") then fail("recipe %q: a name may not hold whitespace", name) end
    if recipes[name] then fail("recipe %q is declared twice", name) end
    if spec.run ~= nil and type(spec.run) ~= "function" then
      fail("recipe %q: `run` must be a function", name)
    end
    if spec.run == nil and spec.deps == nil then
      fail("recipe %q has neither a body nor dependencies", name)
    end
    local recipe = {
      name = name,
      desc = spec.desc or "",
      deps = strings(spec.deps, ("recipe %q: deps"):format(name)),
      inputs = strings(spec.inputs, ("recipe %q: inputs"):format(name)),
      outputs = strings(spec.outputs, ("recipe %q: outputs"):format(name)),
      params = spec.params or {},
      stale = spec.stale or settings.stale,
      quiet = spec.quiet,
      run = spec.run,
    }
    if #recipe.outputs > 0 and #recipe.inputs == 0 then
      fail("recipe %q declares outputs but no inputs, so it could never be up to date", name)
    end
    recipes[name] = recipe
    order[#order + 1] = name
    return recipe
  end

  function make.alias(from, to)
    if type(from) ~= "string" or type(to) ~= "string" then
      fail("alias expects two names")
    end
    aliases[from] = to
  end

  -- Another file's recipes, in this file's graph. Relative to the importing file's directory.
  function make.import(path)
    if type(path) ~= "string" then fail("import expects a path") end
    local full = path
    if not full:match("^/") then full = make.__root() .. "/" .. full end
    local chunk, err = loadfile(full)
    if not chunk then fail("import %q: %s", path, err or "cannot read it") end
    local result = table.pack(chunk())
    imported[#imported + 1] = full
    return table.unpack(result, 1, result.n)
  end

  ---------------------------------------------------------------------------- strict sh

  -- `oslo.run` never raises, on purpose: at a prompt a failing command is Tuesday. In a build it is
  -- the end of the build, so inside `oslo make` the `sh` sugar raises and `oslo.run{…}` keeps its
  -- ordinary manners for the caller who wants to look at `r.status` themselves.
  --
  -- Installed by `__main`, so an interactive shell that has this file loaded is untouched.
  function make.strict()
    -- **The plain table is captured, not looked up.** `__main` assigns the result of this call to
    -- the global `sh`, so an `__index` that read `sh` would be reading itself: one `sh.echo` and the
    -- stack is gone. Taking it as an upvalue here fixes it to what `sh` was before the swap.
    local plain_sh = sh
    return setmetatable({}, {
      __index = function(_, name)
        local plain = plain_sh[name]
        if type(plain) ~= "function" then return plain end
        return function(...)
          local r = plain(...)
          -- A command oslo answers in rows gives a list, not a result, and has no status to check.
          if type(r) == "table" and type(r.status) == "number" and r.status ~= 0 then
            error(("%s exited %d"):format(name, r.status), 0)
          end
          return r
        end
      end,
    })
  end

  ---------------------------------------------------------------------------- staleness

  -- `a/**/b.rs` — every `b.rs` at any depth under `a`.
  --
  -- **`oslo.fs.glob` cannot do this, and must not.** It is the shell's own globber, where `**`
  -- means exactly what `*` means: one path element. That is POSIX, it is what `sh` does without
  -- `globstar`, and oslo's differential corpus holds it to that. A *build system* is not a shell,
  -- though, and `inputs = { "crates/**/*.rs" }` can only sensibly mean "walk".
  --
  -- Getting this wrong was silent and expensive: `crates/**/*.rs` matched nothing, so `expand`
  -- below took it for a literal filename, that name never existed, its fingerprint never changed,
  -- and `oslo make build` answered `up to date` for a tree whose every source file had been
  -- edited. 681 files under `crates/` were invisible to the staleness check.
  local function walked(pattern)
    local head, tail = pattern:match("^(.-)%*%*/(.+)$")
    if not head then return nil end
    head = (head == "") and "." or (head:gsub("/$", ""))

    -- The head itself first, so `a/**/x` also matches `a/x` — the same reading every tool that
    -- has `**` gives it.
    local out = {}
    local function gather(dir)
      for _, hit in ipairs(oslo.fs.glob(dir .. "/" .. tail) or {}) do
        out[#out + 1] = hit
      end
    end
    gather(head)
    for path in oslo.fs.walk(head) do
      local found = oslo.fs.stat(path)
      if found and found.type == "directory" then gather(path) end
    end
    return out
  end

  local function expand(patterns)
    local seen, out = {}, {}
    for _, pattern in ipairs(patterns) do
      local hits = walked(pattern) or oslo.fs.glob(pattern) or {}
      -- A pattern that matched nothing is a literal path, which is what every shell does without
      -- `nullglob` — and here it is also how a not-yet-created file gets counted as missing.
      if #hits == 0 then hits = { pattern } end
      for _, path in ipairs(hits) do
        if not seen[path] then
          seen[path] = true
          out[#out + 1] = path
        end
      end
    end
    return out
  end

  -- **Nanoseconds, not seconds.** At second resolution an input edited and an output written inside
  -- the same second compare equal, so a recipe that had *not* been rebuilt reported "up to date" —
  -- the fast inner loop is exactly where a build finishes inside one second, so this was worst
  -- where it mattered most. `oslo.fs.stat().mtime` is still the seconds a person prints.
  local function mtime_of(path)
    local st = oslo.fs.stat(path)
    return st and st.mtime_ns or nil
  end

  local function fingerprint(paths)
    table.sort(paths)
    local parts = {}
    for _, path in ipairs(paths) do
      parts[#parts + 1] = path .. ":" .. (oslo.hash.file(path) or "-")
    end
    return oslo.hash.sha256(table.concat(parts, "\n"))
  end

  local function store()
    local db = oslo.db.open("make")
    return db
  end

  local function stamp_key(recipe, argv)
    return oslo.hash.sha256(make.__root() .. "|" .. recipe.name .. "|" .. table.concat(argv, " "))
  end

  -- Remember what the inputs were, now that the body has run and not raised.
  --
  -- **After the work, never during the check.** Stamping inside `why_stale` meant a build that
  -- failed had already recorded its inputs as done, so the next run reported "up to date" for
  -- something that had never succeeded — the one answer a build tool must never give.
  local function record(recipe, argv)
    if #recipe.outputs == 0 or recipe.stale ~= "content" then return end
    local db = store()
    if not db then return end
    db:set(stamp_key(recipe, argv), fingerprint(expand(recipe.inputs)))
  end

  -- Why this recipe has to run, or nil when it does not. Asks, and changes nothing.
  local function why_stale(recipe, argv, force)
    if #recipe.outputs == 0 then return "phony" end
    if force then return "forced" end
    for _, out in ipairs(recipe.outputs) do
      if not mtime_of(out) then return "missing " .. make.__relative(out) end
    end
    local inputs = expand(recipe.inputs)
    if recipe.stale == "content" then
      local db = store()
      if not db then return "no cache" end
      local before = db:get(stamp_key(recipe, argv))
      if not before then return "never built" end
      if before ~= fingerprint(inputs) then return "inputs changed" end
      return nil
    end
    local newest, which = nil, nil
    for _, path in ipairs(inputs) do
      local t = mtime_of(path)
      if t and (not newest or t > newest) then newest, which = t, path end
    end
    if not newest then return nil end
    -- **An output has to be strictly newer, so equal timestamps mean stale.**
    --
    -- Nanoseconds are not enough on their own, because the filesystem decides the resolution and
    -- some do not offer any. tmpfs stamps to the jiffy — a few milliseconds — so an output written
    -- and an input edited in the same tick come back byte-identical, and `newest > t` then reads a
    -- rebuild that has not happened as up to date. Measured on `/tmp`: two writes a fraction of a
    -- millisecond apart both landed on …647243224191.
    --
    -- `>=` costs a spurious rebuild when a build genuinely finishes inside one tick, and buys never
    -- shipping a stale artifact. That is the right side to be wrong on.
    for _, out in ipairs(recipe.outputs) do
      local t = mtime_of(out)
      if t and newest >= t then return make.__relative(which) .. " is not older" end
    end
    return nil
  end

  ---------------------------------------------------------------------------- the graph

  -- Declared before use: `plan` calls `suggestion`, and `make.run` calls `execute`. Without these
  -- two lines both become globals in `_G`, which is a strange place for a runner's internals.
  local suggestion, execute

  local function resolve(name)
    return aliases[name] or name
  end

  -- Depth-first, deps before the recipe that wants them, each name once. `state` carries the cycle
  -- check: a name reached while it is still being visited is a cycle, and the stack names it.
  local function plan(name, state, stack, out)
    name = resolve(name)
    local recipe = recipes[name]
    if not recipe then
      fail("no recipe called %q%s", name, suggestion(name))
    end
    if state[name] == "done" then return end
    if state[name] == "visiting" then
      stack[#stack + 1] = name
      fail("dependency cycle: %s", table.concat(stack, " → "))
    end
    state[name] = "visiting"
    stack[#stack + 1] = name
    for _, dep in ipairs(recipe.deps) do plan(dep, state, stack, out) end
    stack[#stack] = nil
    state[name] = "done"
    out[#out + 1] = recipe
  end

  -- The names that share a prefix with what was typed. Cheap, and it catches the case that actually
  -- happens: a recipe half-remembered rather than misspelled in the middle.
  function suggestion(name)
    local near = {}
    for _, known in ipairs(order) do
      if known:sub(1, 1) == name:sub(1, 1) or known:find(name, 1, true) then
        near[#near + 1] = known
      end
    end
    if #near == 0 then return "" end
    return ". Did you mean: " .. table.concat(near, ", ") .. "?"
  end

  ---------------------------------------------------------------------------- parameters

  -- `--type minimal`, `--type=minimal` and a bare `--dry` all reach the body as fields on one
  -- table, with declared defaults filled in. Undeclared flags are kept too — a recipe that wants to
  -- pass its arguments straight through should not have to declare them first.
  local function parse_params(recipe, argv)
    local args = { [0] = recipe.name }
    for _, spec in ipairs(recipe.params or {}) do
      local flag = spec[1] or spec.name
      if flag then
        local key = flag:gsub("^%-+", ""):gsub("%-", "_")
        args[key] = spec.default
      end
    end
    local rest, i = {}, 1
    while i <= #argv do
      local word = argv[i]
      local flag, value = word:match("^%-%-([%w%-_]+)=(.*)$")
      if flag then
        args[flag:gsub("%-", "_")] = value
      elseif word:match("^%-%-[%w%-_]+$") then
        local key = word:sub(3):gsub("%-", "_")
        local next_word = argv[i + 1]
        if next_word and not next_word:match("^%-") then
          args[key] = next_word
          i = i + 1
        else
          args[key] = true
        end
      else
        rest[#rest + 1] = word
      end
      i = i + 1
    end
    args.rest = rest
    return args
  end

  ---------------------------------------------------------------------------- reporting

  local paint = oslo.ui and oslo.ui.is_tty and oslo.ui.is_tty()

  local function say(mark, name, note)
    if settings.quiet then return end
    local line = mark .. " " .. name
    if note and note ~= "" then line = line .. "  " .. note end
    print(line)
  end

  local function listing()
    local width = 0
    for _, name in ipairs(order) do
      if name:sub(1, 1) ~= "_" and #name > width then width = #name end
    end
    local shown = 0
    for _, name in ipairs(order) do
      if name:sub(1, 1) ~= "_" then
        shown = shown + 1
        print(string.format("%-" .. width .. "s  %s", name, recipes[name].desc))
      end
    end
    for from, to in pairs(aliases) do
      print(string.format("%-" .. width .. "s  → %s", from, to))
    end
    if shown == 0 then
      print("no recipes in " .. make.__relative(make.__file()))
    end
    return 0
  end

  -- The option list, which the `oslo make --help` page also prints — so the flags this parses and
  -- the flags the help names are one piece of text.
  local OPTIONS = [==[
OPTIONS
  -l, --list        the recipes and what they say they do
  -n, --dry-run     name every recipe that would run, and run none
  -f, --force       run even a recipe that is up to date
  -k, --keep-going  carry on after a recipe fails
  -q, --quiet       no progress lines, only what the recipes print
      --watch       watch the resolved recipe inputs and rerun the target
      --postpone    wait for a change before the first watched run
      --restart     restart a running watched target after a change
  -h, --help        this text
]==]

  function make.__options() return OPTIONS end

  ---------------------------------------------------------------------------- running one

  local ran = {}

  -- Run one recipe by name from inside another. The same memo the plan uses, so a shared dependency
  -- asked for by hand does not run a second time.
  function make.run(name, argv)
    name = resolve(name)
    local recipe = recipes[name]
    if not recipe then fail("no recipe called %q%s", name, suggestion(name)) end
    if ran[name] then return end
    local state, stack, out = {}, {}, {}
    plan(name, state, stack, out)
    for _, step in ipairs(out) do
      execute(step, step.name == name and (argv or {}) or {}, false, false)
    end
  end

  function execute(recipe, argv, dry, force)
    if ran[recipe.name] then return end
    local reason = why_stale(recipe, argv, force)
    if not reason then
      ran[recipe.name] = true
      say("·", recipe.name, "up to date")
      return
    end
    if dry then
      ran[recipe.name] = true
      say("→", recipe.name, reason == "phony" and "" or "(" .. reason .. ")")
      return
    end
    ran[recipe.name] = true
    if not recipe.quiet then say("→", recipe.name, "") end
    if recipe.run then recipe.run(parse_params(recipe, argv)) end
    -- Only here: the body returned, so whatever it read is what its outputs were built from. A
    -- body that raised skips this line entirely and the next run finds the recipe still stale.
    record(recipe, argv)
  end

  ---------------------------------------------------------------------------- the entry point

  function make.__main()
    sh = make.strict()
    local argv = make.__argv()
    local dry, force, list, target, rest = false, false, false, nil, {}
    local watching, initial, policy = false, true, "coalesce"

    local i = 1
    while i <= #argv do
      local word = argv[i]
      if target then
        rest[#rest + 1] = word
      elseif word == "--" then
        target = argv[i + 1]
        for j = i + 2, #argv do rest[#rest + 1] = argv[j] end
        break
      elseif word == "-h" or word == "--help" then
        io.write(OPTIONS)
        return make.__status(0)
      elseif word == "-l" or word == "--list" then
        list = true
      elseif word == "-n" or word == "--dry-run" then
        dry = true
      elseif word == "-f" or word == "--force" then
        force = true
      elseif word == "-k" or word == "--keep-going" then
        settings.keep_going = true
      elseif word == "-q" or word == "--quiet" then
        settings.quiet = true
      elseif word == "--watch" then
        watching = true
      elseif word == "--postpone" then
        initial = false
      elseif word == "--restart" then
        policy = "restart"
      elseif word:match("^%-") then
        io.stderr:write("oslo make: " .. word .. ": unknown option\n")
        io.write(OPTIONS)
        return make.__status(2)
      else
        target = word
      end
      i = i + 1
    end

    if watching and not target then
      io.stderr:write("oslo make: --watch needs a recipe\n")
      return make.__status(2)
    end
    if list or not target then return make.__status(listing()) end

    local ok_plan, planned = pcall(function()
      local state, stack, out = {}, {}, {}
      plan(target, state, stack, out)
      return out
    end)
    if not ok_plan then
      io.stderr:write(tostring(planned) .. "\n")
      return make.__status(2)
    end

    if watching then
      if type(make.__watch) ~= "function" then
        io.stderr:write("oslo make: this build does not support recipe watching\n")
        return make.__status(2)
      end
      local paths, seen, declared = {}, {}, 0
      local function add(path)
        if not seen[path] then
          seen[path] = true
          paths[#paths + 1] = path
        end
      end
      for _, recipe in ipairs(planned) do
        for _, path in ipairs(recipe.inputs) do
          declared = declared + 1
          add(path)
        end
      end
      if declared == 0 then
        io.stderr:write("oslo make: the resolved plan has no inputs; declare recipe inputs to watch\n")
        return make.__status(2)
      end
      for _, path in ipairs(imported) do add(path) end
      make.__watch {
        target = target,
        args = rest,
        paths = paths,
        initial = initial,
        policy = policy,
      }
      return make.__status(0)
    end

    local status = 0
    for _, recipe in ipairs(planned) do
      local args = recipe.name == resolve(target) and rest or {}
      local fine, err = pcall(execute, recipe, args, dry, force)
      if not fine then
        io.stderr:write(("oslo make: %s: %s\n"):format(recipe.name, tostring(err)))
        status = 1
        if not settings.keep_going then break end
      end
    end
    return make.__status(status)
  end

  ---------------------------------------------------------------------------- what a caller reads

  -- The declared names, for the listing cache and for completion. Data, never the bodies.
  --
  -- `params` comes with them, flattened to what a completion needs: the flag as written, whether
  -- it takes a value, and the one line describing it. A recipe's parameters are the other half of
  -- what somebody types after `make <name>`, and leaving them out would mean the names completed
  -- and the flags did not.
  function make.names()
    local out = {}
    for _, name in ipairs(order) do
      local recipe = recipes[name]
      local params = {}
      for _, spec in ipairs(recipe.params or {}) do
        local flag = spec[1] or spec.name
        if type(flag) == "string" and flag ~= "" then
          params[#params + 1] = {
            name = flag,
            desc = spec.desc or "",
            -- `flag = true` is a switch; everything else is read as taking a value, which is what
            -- `parse_params` does with a word that does not begin with a dash.
            takes_value = not spec.flag,
          }
        end
      end
      out[#out + 1] = {
        name = name,
        desc = recipe.desc,
        private = name:sub(1, 1) == "_",
        params = params,
      }
    end
    return out
  end
end
