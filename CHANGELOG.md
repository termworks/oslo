# Changelog

## [0.7.1] - 2026-09-17

### <!-- 0 -->⛰️  Features

- Bash's alt-. alt-< alt-> and alt-#
- Ask once for read-only files, offer sudo

### <!-- 1 -->🐛 Bug Fixes

- A recipe runs in the directory's env
- Rm in the trash dir deletes, not renames
- Y and n answer a confirm
- A stale Ctrl-C no longer answers rm
- An empty .git is not a workspace
- Record what runs in /tmp and .git

## [0.7.0] - 2026-09-12

### <!-- 0 -->⛰️  Features

- Extglob patterns and the shopt option
- Glob qualifiers at the prompt
- One oslo.glob with qualifiers, and oslo.shopt
- Qualifiers, regex and a glob builtin
- Expand-glob, list-glob and a no-match colour
- Tab expands a glob with the shell's engine
- Nullglob, failglob, dotglob and friends
- Open history on the workspace in a repo
- Run the ERR condition
- List the other machine over ssh
- Hosts from the history you typed
- Complete git refs without running git
- Complete jobs, aliases and functions
- Wire the sources into many more specs
- Eleven more sources for the machine
- Point shipped specs at the sources
- Eight sources for what the machine knows
- Watch recipe inputs
- Declare watch commands
- Select scratch launch
- Host watch workers
- Add watch tool
- Add command runner
- Add event source
- Hostname completion for the ssh family

### <!-- 1 -->🐛 Bug Fixes

- Extglob groups keep quotes and $ inside
- Failglob drops one command, not the script
- Glob menu rows read from where the pattern starts
- A name that is not UTF-8 keeps its bytes
- Quoting decides per character everywhere
- Sort matches the way bash does
- A bash-exact globstar in one walker
- An escaped command word skips its alias
- Reread hosts when their files change
- Route stored argc-eval scripts to oslo
- Give ssh listings 10s and show why
- A poisoned lock is not a second failure
- A poisoned lock is not a second failure
- Guard nesting on the stack, not a count
- Spell control bytes in diagnostics
- Let a function recurse past a hundred
- Keep a quoted csv field as text
- Hand a deep table back without recursing
- Free a deep table without recursing
- Bound bracket nesting instead of overflowing
- Build and test without built-in crypto
- Start without a worker thread if need be
- Join a word across a line continuation
- Key the allow gate on path bytes
- Honour -t and end on a fatal signal
- Run the EXIT trap when a signal ends the shell
- Give a sourced file its own arguments
- End the wait on a trapped signal
- Let an ignored signal reach children
- Keep what a command did to the terminal
- An interactive shell ignores SIGQUIT
- Trap - INT keeps the shell's own handler
- A byte that is not UTF-8 no longer aborts
- Settle waits only in the process that queued
- Three aborts reachable from a format
- Bound the read from an external tool
- Trim $HISTFILE to its own lines
- Refuse an oversized socket path early
- Offer nothing after host:
- BASH_SOURCE names the running file
- $(<file) reads the file
- An unreadable directory may still be empty
- -i alone is not a prompt
- Errexit carries the command status
- Recognise this shell through a symlink
- Set $_ to the last argument
- Set $BASH so a script can re-exec
- Answer the --argc-eval idiom
- A stored macro is one thing, not two
- Judge an assignment on its value
- Repair rustdoc links
- Reject coproc by name
- Reject adjacent commands
- Keep oversized fd word
- Create autopair namespace
- Detach persistent services
- Scan new recursive trees
- Isolate watch bootstrap
- Satisfy strict lint
- TERM=dumb turns the line editor off
- No animation clock where nothing is drawn

### <!-- 2 -->🚜 Refactor

- Move the format tests to their own file

### <!-- 3 -->📚 Documentation

- Globbing, its options and its gaps
- Condense future plans, keep the substance
- Restore full plan details under the ranking
- Rank and shorten future plans
- Drop the fzf preview from future plans
- Correct the nesting overflow report
- Record what the queue does not fix
- Fix the links rustdoc refuses
- Record what a blocked shell still misses
- Record what opt-level z costs
- List every built-in tool
- Remove missing example links
- Explain watch services

### <!-- 4 -->⚡ Performance

- Size the stack for the release build
- Bring tables across without recursing
- Every crate at opt-level z

### <!-- 6 -->🧪 Testing

- The glob corpus and a benchmark
- Count the glob_nomatch theme role
- A temporary home on a short path
- Guard suites CI cannot build
- Guard against exponential backtracking
- Walk every shipped spec for panics
- Cover ERR and keep the width guard
- Prove the quoting against a real shell
- Pin the rm builtin against the program
- Pin $_ and $BASH against bash
- Catch completion keys naming no flag
- Cover scratch daemon
- Isolate animation clock
- Disable autopair in multiline case
- Drop removed examples
- Use runtimepath grants
- Isolate piped terminal mode
- Use drawable terminal
- Accept prompted Lua output
- Record parse name collision
- Refresh kept messages
- Cover process policies
- Cover event and path cases

### <!-- 7 -->⚙️ Miscellaneous Tasks

- Future plans

### Build

- Rune v0.1.4
- Keep the rune pin in one place

## [0.6.2] - 2026-09-02

### <!-- 1 -->🐛 Bug Fixes

- Set -U points at the name it refused
- A ui flag's value gets the caret
- The structured verbs point at the verb
- The scalar verbs and fmt get a caret too

## [0.6.1] - 2026-09-01

### <!-- 1 -->🐛 Bug Fixes

- A prompt no longer overwrites output with no newline

## [0.6.0] - 2026-09-01

### <!-- 0 -->⛰️  Features

- Ctrl-x opens the line in $EDITOR
- Text objects for every operator
- A bracket or a quote closes itself
- Argc declarations line up in columns
- Set -U, one file every session shares
- Completions read out of man pages
- Oslo fmt, on rune's lossless tree
- Filename verbs under the text feature
- String verbs, behind the text feature
- Funced and vared
- A script can fire a user event too
- Ui key names the key you pressed
- Rune replaces the vendored parser
- Truecolour, so a palette cannot move it
- A script names its file, its line and its code
- Ariadne, and a caret in sixty places
- The sweep, and eleven sites it found
- A syntax error points into the program
- A config mistake points into init.lua
- A caret in the structured verbs
- A caret for names, options and identifiers
- A caret in twenty-odd builtin diagnostics
- A caret under the word that was wrong

### <!-- 1 -->🐛 Bug Fixes

- The description column sits three spaces out
- An unfinished construct names where it began
- A script keeps its line numbers when it will not parse
- No filter and no legend
- No underline on the cursor cell
- The cursor is a cell, not a row

### <!-- 2 -->🚜 Refactor

- A key's name gets its own file
- The spec feature becomes compgen

### <!-- 3 -->📚 Documentation

- What landed, and what was decided differently
- Scripts, Lua, and the widened sweep
- The two faces, and how to add a caret

### <!-- 5 -->🎨 Styling

- The finder's look, drawn by its own code

### <!-- 6 -->🧪 Testing

- The scalar verbs are not row-to-row

### <!-- 7 -->⚙️ Miscellaneous Tasks

- Argc is all that is left in there

## [0.5.4] - 2026-09-01

### <!-- 0 -->⛰️  Features

- A viewer for the rows you piped it
- Numbers align right and a border is drawable
- The past is a table
- A cell may name its own kind
- Every row verb is also a function
- A delimited document streams by record
- A verb says it is one, not a typo
- Delete in the finder ends one
- Folding verbs stream within a bound
- A sourced file may be Lua

### <!-- 1 -->🐛 Bug Fixes

- A nested cell is described, not spelled out
- A bridge at the tail is a pipeline
- Reset asks the terminal again
- The prompt is not drawn through leftovers
- A verb in a pipeline outranks an alias
- A quoted verb name is still that verb
- A second attach says so
- A quoted operand is still a known one
- A stderr redirect is not an output one

### <!-- 2 -->🚜 Refactor

- Five oversized files split by meaning

### <!-- 3 -->📚 Documentation

- A tool may answer with a size or a failure
- The settled decisions outlive the plan

### <!-- 6 -->🧪 Testing

- The parse bridge is held to the same rule
- The two paths must answer the same

### Config

- Rounded borders on the drawn table

## [0.5.3] - 2026-08-29

### <!-- 0 -->⛰️  Features

- An upstream with no end is read as it arrives
- A column inside a filter is offered too
- A config's tool may declare its columns
- Oslo.table configures the drawn face only
- The menu offers the columns a stage has
- The planner knows what columns a stage has
- Lookup, append and merge take a second stream
- Describe, histogram and reduce
- A time is a date, and a row is one line
- A pattern may be a regular expression
- Detect-columns, and csv and tsv
- Twelve verbs for columns and rows
- Sort-by takes flags and several keys
- Map, and a column name may be a path

### <!-- 1 -->🐛 Bug Fixes

- A cap once reached no longer refuses for ever
- A clobbered socket no longer spins a core
- A mount that will not answer still has a row
- Writing a column understands a path too
- A re-record can be published again
- The musl release builds without a warning
- Oslo.table is a namespace a config can write
- An error names what you typed
- An endless upstream cannot eat the machine
- A cell cannot break the framing
- One converter each way, not four

### <!-- 3 -->📚 Documentation

- The structured recording is the current one
- The structured demo runs again, and shows the new verbs
- The page catches up with the column contract
- The page catches up with every phase
- Units, summarise verbs and a real corpus count

### <!-- 6 -->🧪 Testing

- Tests that share a global take turns
- The drawing tests take turns over settings
- Detect-columns joins the declared list

## [0.5.2] - 2026-08-29

### <!-- 0 -->⛰️  Features

- Prompt before leaving a hexe pane
- The arrival names the functions too
- The row is stamped with the time
- Animate from a filmstrip
- A project may define abbreviations and functions
- Pre-exit may keep the shell open
- Shrink the explorer when previewing

### <!-- 1 -->🐛 Bug Fixes

- Drop the workspace member that was deleted
- The command leads, the status trails
- An undescribed row gives its column up
- Two path markers mean both, and cd means folders
- Enter ends a word too

### <!-- 2 -->🚜 Refactor

- [**breaking**] A runtimepath, the way neovim does it

### <!-- 4 -->⚡ Performance

- One linker, one set of flags, 623 KB less

### <!-- 6 -->🧪 Testing

- The sequence is the arithmetic, not the global

### <!-- 7 -->⚙️ Miscellaneous Tasks

- Cleanup
- Docs
- Install the config with the binary

## [0.5.1] - 2026-08-27

### <!-- 1 -->🐛 Bug Fixes

- No plugins is not a stale index

## [0.5.0] - 2026-08-27

### <!-- 0 -->⛰️  Features

- Completions, and a long hardening pass
- -i makes a name's assignments arithmetic
- Recipes and tools complete themselves
- Ship converted Fig and argc specs
- Carapace specs, files and macros
- Keep the prompt alive while a browser is open

### <!-- 1 -->🐛 Bug Fixes

- Ctrl-C on an empty line gives its rows back
- Ctrl-C draws one prompt, not two
- Posix mode escalates a malformed expansion
- A format with no conversion is an error
- Past the end is quiet outside posix mode
- A break that cannot break says so
- One category per diagnostic, in lower case
- The failures that end a script say where
- 127 is for the three failures bash gives it to
- A transient prompt stands in after Ctrl-C too
- Ctrl-C leaves the line the way Enter does
- An edited manifest is reported, not ignored
- A serve clears the sockets nobody answers
- -p reports the integer attribute
- One printer, so one definition per function
- A completion macro cannot stop the editor
- Rows cross into a verb that reads bytes
- A filter's text survives the unit rewrite
- A timed command reports what it did
- A scoped readonly leaves with its scope
- SECONDS re-bases and RANDOM seeds
- A tombstone carries no command
- Kept output is readable only by its owner
- $ENV is for interactive shells only
- A panic behind the redirect still reaches you
- A stop finishes before it returns
- A config's hook is read live, not snapshotted
- A superseded request is not left outstanding
- The notice's child is reaped
- A predicate runs with no registry borrow held
- A number from a config cannot abort the shell
- A config's candidates are quoted and filterable
- A captured field is trimmed either way
- Os.exit goes through the shell's exit path
- A float prints the way Lua prints it
- A unit answer wears the unit it is in
- Return and hashall do what they say
- A child inherits no stray descriptors
- A quoted payload keeps its own quotes
- & in a replacement is what matched
- A scalar is an array of one
- Take back the builtins the directory added
- One ENOEXEC policy for every exec site
- A reserved word is an ordinary element
- Copy in before deleting what is there
- Keep the pipe out of the script's fds
- A target is one filename or a mistake
- Let ctrl-s reach the editor
- Find HEAD in a linked worktree
- Report why a script would not run
- Give the terminal back when the shell panics
- Tell the hooks outside the table's lock
- Bound the door back into arithmetic
- A signal is not the end of a stream
- Cut a capped stream at a character
- Stdout comes back when a stage gives up
- Scan text a character at a time, not a byte
- Declare -f prints bodies, break n is clamped
- -F answers bash's two forms, -f says so
- A tilde after = or : is a tilde prefix
- A directory must not shadow a tool
- Clusters, separators, depth and repeats
- Scoped filters and variadic flag values
- Escaped quotes and dashed subcommands
- Tell pixy the directory oslo moved to
- The blank row is the block's, on both sides
- Ask the terminal where the block begins
- A repaint must give the block back
- The blank row belongs to the block

### <!-- 2 -->🚜 Refactor

- One place where a finished line leaves
- Six more pub items nothing calls
- Delete twelve pub items nothing calls
- Drop a traceback that was never built
- The fourth copy of human_size, gone
- One base64, one percent-encode, one size
- One clamp, not a name that forwards to it
- Split the draining half into its own file
- Name the folder for what it holds

### <!-- 3 -->📚 Documentation

- Three links that pointed at nothing
- Say what the sweep bounds and what it does not
- The example shows the form that works
- The measured binary sizes
- A page of its own

### <!-- 4 -->⚡ Performance

- The parser and argc can afford opt-level z
- Drop a megabyte of unreachable metadata
- Stop rendering variants nobody reads
- A replacement is linear, not cubic
- One glob budget for the whole word
- Resolve descriptions for drawn rows only
- Oslo-shell at opt-level z, 172 KB

### <!-- 6 -->🧪 Testing

- SECONDS may tick between two commands
- The rescue tests share one terminal slot
- The vocabulary tests take turns
- Two mutexes is no exclusion at all
- Hashall is on and return needs a frame
- The gate runs every crate's own tests
- Pin the timeout behaviour without the delivery queue

### <!-- 7 -->⚙️ Miscellaneous Tasks

- Cleanup
- Keep the prompt alive during a float

## [0.4.13] - 2026-08-25

### <!-- 0 -->⛰️  Features

- A live shell, an animated prompt and a transcript
- No blank row on a cleared screen
- A blank row before the prompt
- A blank row on each side of the block
- Hand the whole row to the renderer
- Draw the divider in an indexed colour
- Lead the exit code in with the rule
- Open a frame with the last exit code
- Brackets on every row of a command
- A rule that runs into the command
- A command line and a rule under it
- Mark and delegate the transcript block
- A transcript rule in place of the prompt
- Every and $frame for an external prompt
- Let a segment animate on its own clock
- Let a peer move the shell
- Run a bound line without leaving it
- Browse with a configured command
- Open trek in a sized viewport
- Hand the browser to trek when installed
- Fetch() for a peer with no daemon

### <!-- 1 -->🐛 Bug Fixes

- Ask the renderer one row at a time
- Tell pixy the width oslo laid out with
- Do not re-run an external prompt on a frame
- Keep the prompt up while an erased line runs
- Fall back when the browser is missing
- Drop the shadowed verbs method

### <!-- 6 -->🧪 Testing

- Run the repl in its throwaway home

### <!-- 7 -->⚙️ Miscellaneous Tasks

- Browse with trek, erase the nav line

### <!-- 9 -->◀️ Revert

- Leave the right prompt to pixy

## [0.4.12] - 2026-08-24

### <!-- 0 -->⛰️  Features

- Make configs installs config/ into the config dir

### <!-- 3 -->📚 Documentation

- Name Cargo.toml in the metadata assert

## [0.4.11] - 2026-08-23

### <!-- 1 -->🐛 Bug Fixes

- Find sockets from inside a sibling host

## [0.4.10] - 2026-08-23

### <!-- 0 -->⛰️  Features

- Export $OSLO_SOCK so a child finds its own shell
- A control socket, and the Lua client for it
- Oslo.stream, the socket a client library needs

### <!-- 1 -->🐛 Bug Fixes

- A source under crates/ makes the build stale
- A replaced binary can still start itself
- A self-pipe closes the Ctrl-C race for good
- A Ctrl-C before the read is not lost
- The tagdata hash is for the version in the lock

### <!-- 3 -->📚 Documentation

- Record the control socket
- The control socket

### <!-- 7 -->⚙️ Miscellaneous Tasks

- The repository moved to termworks

## [0.4.9] - 2026-08-22

### <!-- 1 -->🐛 Bug Fixes

- Every $PATH search reads the visibility mask

### <!-- 3 -->📚 Documentation

- Guard the hook with type, not command -v

## [0.4.8] - 2026-08-22

### <!-- 0 -->⛰️  Features

- Oslo reads .env.lua, direnv reads .envrc
- Honour bash's --norc and --noprofile
- Oslo.command.when hides a program per directory
- Oslo.fs.find_up

### <!-- 1 -->🐛 Bug Fixes

- A configured variable is not re-read as a recipe

### <!-- 3 -->📚 Documentation

- The last reference to the deleted stdlib
- Three claims that outlived .envrc support
- Fix two unresolved intra-doc links
- The hook line is shell, not Lua
- Remeasure the direnv and plugin feature sizes
- Drop the Lua style guide, now implemented

## [0.4.7] - 2026-08-22

### <!-- 0 -->⛰️  Features

- A hook under oslo.on does not repeat the prefix

### <!-- 1 -->🐛 Bug Fixes

- An unterminated construct is an error, not a panic

### <!-- 3 -->📚 Documentation

- Use the un-stuttered spelling in an example

### <!-- 6 -->🧪 Testing

- A child shell reads nobody's configuration
- The writer needs the binary under test on $PATH

## [0.4.6] - 2026-08-22

### Build

- One bootstrap script, and no Makefile

## [0.4.5] - 2026-08-22

### <!-- 0 -->⛰️  Features

- Install to $PREFIX/bin and /usr/bin

### <!-- 1 -->🐛 Bug Fixes

- A space does not make a value a command

### <!-- 7 -->⚙️ Miscellaneous Tasks

- Cleanup

## [0.4.4] - 2026-08-22

### <!-- 0 -->⛰️  Features

- Colouring by name, and a heading to put it under
- The structured verbs a pipeline cannot reach
- One declaration a config can parse with
- Waiting for work where there is no prompt
- A variable that outlives the shell
- The scorer a completion provider can reach
- A command that may not take forever
- A builtin is handed the shell and returns effects
- The shell as a record a builtin can be handed
- A moment to clean up, and what else to watch
- A variable a project keeps to itself
- The word rules a locked surface can still ask

### <!-- 1 -->🐛 Bug Fixes

- Add says what edit says, and refuses what it should
- A stored variable wins, like every other kind
- Accepts = "bytes" is handed its bytes
- Oslo.lines keeps stderr and the exit status
- .env.lua runs in its own directory
- Equal mtimes mean stale, not fresh

### <!-- 2 -->🚜 Refactor

- The macro store replaces universal variables
- Oslo.env gets a file of its own

### <!-- 6 -->🧪 Testing

- Walk the oslo.* surface and the lock boundary

### <!-- 7 -->⚙️ Miscellaneous Tasks

- Aux files
- Merge develop to main
- Cleanup
- Merge develop to main
- Cleanup

### <!-- 9 -->◀️ Revert

- Drop the universal Lua bindings

## [0.4.3] - 2026-08-21

## [0.4.2] - 2026-08-21

### <!-- 1 -->🐛 Bug Fixes

- Prevent screen content from overwriting previous output

## [0.4.1] - 2026-08-20

### <!-- 1 -->🐛 Bug Fixes

- A ctrl-c escapes the prompt

## [0.4.0] - 2026-08-20

### <!-- 0 -->⛰️  Features

- Pixy dust
- Paint a coordinate, and preview what it will become
- .. ranges words the way a coordinate does
- A digit after ! is an event, a space means lua
- {%n} is the stage, {n} is what it printed
- Substitute wherever a word can appear
- A negative stream is the previous command line
- Run a coordinate pipeline one stage at a time
- Rewrite a command's words from the stack
- The stack a coordinate reads from
- Parse and select stream coordinates
- Paint lua as lua, and show block depth
- A continuation marker per language
- Oslo.term exposes negotiated capabilities
- Sh_sources and lua_sources for ghost and tab
- The lua prompt gets its own suggestion sources
- Enter follows what the terminal reports
- Ctrl+enter sends, ctrl+tab switches
- Enter adds a line, alt+enter sends
- Double tab is on by default
- Ctrl+space switches language too
- The REPL completes Lua, not shell
- ! runs one Lua line, prefix configurable
- A calculator with units, behind the math feature
- The prompt evaluates expressions and shows them
- Assignments name the script and line too
- -v assigns, unknown options refused
- Diagnostics name the script and line
- A watcher API — action, notice, report and hook
- Interrupt escape stops a job instead of killing it
- Three interrupts kill a job that ignores them
- Proc.info, proc.children and fs.disk
- History as rows, fs.touch and fs.usage
- Oslo.fs.watch, and $PATH as a list
- Oslo.git reads the repository without running git
- A string is bytes, so a binary read is the file
- A load runs under a memory ceiling
- A failure carries its kind, code and path
- Fs.walk and fs.lines stream, iterators close
- Handles are objects, with <close>
- The config file is init.lua
- One config, joined by require
- Run on a pinned luna, and re-impose the names oslo refuses
- The shell runs on the vm, and the tree walker is gone
- The shell's variables as lua's global namespace
- Nested writes, compiled chunks and line completeness
- The ui and the shell run on the vm
- The engine boundary natives and hooks cross
- The boundary between the shell's values and the vm's
- Vendor luna, a pure-rust stackless lua vm
- Names that $PATH never heard of
- Autoload a function from its own file
- Mark a directory, reach it by @name
- Primitives the prompt also needs
- $FUNCNAME is the call stack, as in bash
- Oslo hook and oslo direnv actually exist
- Stage, signal and pipestatus in the payloads
- On-variable-change, with local and remote
- On-focus-change for terminal focus
- Timers and spawn callbacks fire at a waiting prompt
- Universals arrive without a keystroke
- An idle prompt notices a job finishing
- On-process-exit and on-job-state

### <!-- 1 -->🐛 Bug Fixes

- _VERSION is the language, not the vm
- Restore the claims the readme rewrite dropped
- A tool may hand over to a byte stage
- Put back the globals a row borrowed
- A coordinate goes where a brace expands
- A conditional agrees with test
- Obey pipefail on the coordinate path
- Replace every coordinate, and bound the read
- Only adjacent parens open an arithmetic command
- The marker sits at column zero
- Drop the renamed-key warning
- A leading minus is arithmetic, not a flag
- A conversion target is a unit, not the rest
- A global that changes type moves between homes
- Require detects a loop, os.setlocale exists
- The README check honours the build's features
- Subcommands refuse operands they cannot read
- \cmd and \\cmd are commands, not escapes
- Verbs refuse arguments they cannot read
- Widen the fd limit so deep trees still go
- Usage lines and quoting match bash
- Print the message, not its category
- Name the real reason a path will not run
- Traverse by descriptor, not by path
- Refuse arguments instead of ignoring
- An unknown option is not the action
- Walk the tree and name what failed
- Fs.walk skips what it cannot read instead of raising
- A script diagnostic names its file and line
- Fs.usage counts what it cannot read instead of failing
- No __gc backstop, because luna runs none
- A mistyped mark is named too
- A refused =name offers the one you meant
- A mistyped =name is told, and quotes keep it literal
- A value that crosses the boundary keeps its identity
- The lua helpers reach the table the vm actually holds
- A redirection keeps the rows flowing
- A brace list is what it expands to
- A terminal read answers Ctrl-C
- ~name completes to a user
- @name expands wherever a tilde does
- Chrome gives way to the labels
- No verdict on a path behind a $VAR
- A record keeps the document's column order
- -c does not write itself back
- The config is not overruled by its echo
- @name gets a path hint like ~ does
- A resize gives the line back its paint
- What runs is what the shell admits runs
- The erase never starts at row zero
- The edges answer like the middle
- A quoted @name stays a literal
- A reserved word is already finished
- Never replace a command that runs
- No way out of a shell loses history
- Finish the half-wired api corners
- A pipeline larger than a pipe
- A resize no longer eats the prompt
- A correction that reads assignments
- Globs, braces and marks complete
- Every key name has a canonical form
- Unstage a peer's hook my commit swept up
- A head-of-pipeline tool reads stdin
- For ((;;)) parses, fused separator and all
- A regex built from a variable is a regex
- Every standard name exists, twenty did not
- Os.time reads its table, error level 0 is bare
- Ls flags are flags, not a directory
- A child's SIGINT is not a script's
- A heredoc predates the command's assignments
- The sign and alternate flags, and a real %g
- A special builtin's error ends the shell
- Refuse before making a session nobody can enter
- Gsub honours the ^ anchor
- A bare jobs lists every job
- The shipped aliases are a prompt's, not a script's
- A brace item is not the whole path
- An absent universal store is not a change
- An escape's value is printed, not run
- : breaks a word too, as in bash
- = ends a word, as it does in bash
- A transposition is one mistake, not two
- Replace the store instead of editing it
- The idle wait now survives its own wake
- A disowned child is still ours to bury

### <!-- 2 -->🚜 Refactor

- Drop the two env vars nobody asked for
- One completion source, two answers
- Require-ability moves to its own module
- The shell's values stop coming from the lua crate
- The shared value type leaves the lua crate
- Glob and tilde come from oslo-base
- The call chain gets its own module

### <!-- 3 -->📚 Documentation

- 1611 lines down to 177, and every example run
- Sweep every page against the binary
- A recording for the six pages that had none
- Embed the recording
- A demo for the coordinate page
- A page for stream coordinates, and the renames it exposed
- A page for the interrupt escape
- What §5 and §6 actually cost
- Finalizers do run; say why no handle sets one
- What a richer lua api would contain
- Record what panic=abort would cost
- The interpreter page describes the vm that is there
- The prose is checked against the binary
- The features index catches up
- Point the design references at the page that exists
- A tool runs alone and sees its input
- The two pages the README promised
- Say what -h actually does

### <!-- 4 -->⚡ Performance

- Abort on panic, now that the store cannot
- Size over speed for ui and parser
- Drop the regex literal accelerator
- A literal key looks itself up in one place
- Fold identical functions with mold
- Drop regex's general-category tables
- Pack relative relocations in the shipped binary
- The two cold members compile for size
- The worktree is found once a frame
- The directory is read once, not per key
- History writes leave the prompt thread

### <!-- 6 -->🧪 Testing

- The library's answers leave the language's file
- One guard, both suites
- Refuse an oracle that is not bash
- Split the stdlib answers into their own file
- Wait longer, and say what the shell was doing
- Assert the error, not the folded status
- A prompt that changes while idle is redrawn

### <!-- 7 -->⚙️ Miscellaneous Tasks

- Fold the withdrawn 0.4.x releases
- Ignore a build directory at any depth
- Refactor Nix flake to simplify and optimize
- Drop a stray file from testing
- One version for the whole workspace
- Strip the manifest comments
- Pin the v0.5.0 release
- Pin the size-optimised vm
- Luna's gsub anchor fix
- Trim rand's default features in luna
- Re-vendor luna under its own name

### Build

- Pin the toolchain, raise the msrv to 1.90

### Spike

- Measure the reference VM against the tree walker

## [0.3.13] - 2026-08-14

## [0.3.13] - 2026-08-14

### <!-- 2 -->🚜 Refactor

- One command, and it is oslo profile sync

## [0.3.12] - 2026-08-14

### <!-- 1 -->🐛 Bug Fixes

- Profile sync carries macros and secrets too

## [0.3.11] - 2026-08-14

### <!-- 0 -->⛰️  Features

- Macros and secrets travel, deletes included
- The profile key is the store key too
- A key per profile, and two-way sync over ssh

### <!-- 1 -->🐛 Bug Fixes

- Refuse an inline body before opening the store
- Empty stores, derived copies, and a stale far end
- The nested-shell ask was eating the profile act

### <!-- 2 -->🚜 Refactor

- One help renderer for every submenu

### <!-- 3 -->📚 Documentation

- The rule, the tombstones, and what crosses sealed
- The key, the sync, and what secrets derive

### <!-- 6 -->🧪 Testing

- The secret parts only where the build has them
- Pin the refused attributes and the key collision

### <!-- 7 -->⚙️ Miscellaneous Tasks

- Back to 0.3.10, the 0.4 and 0.5 tags are gone

## [0.3.10] - 2026-08-14

## [0.3.9] - 2026-08-14

### <!-- 0 -->⛰️  Features

- A key you keep, recipients you publish
- Ctrl-space marks rows, delete twice confirms
- Split the filing from the crypto, drop age
- Hooks can call age over a pipe
- Hooks do the crypto, lua does the storage
- A store can hand its crypto to a program
- Stores, keys and recipients as an api
- Arrows walk the kinds, #tag filters
- A fifth kind — variables that hold a recipe
- Age behind a switch, and what it costs

### <!-- 1 -->🐛 Bug Fixes

- A fake token that no scanner mistakes for a real one
- Ctrl-c ends a loop whose body forks
- Record against the binary under test
- An inherited row is not yours to edit
- A script directory belongs to one store
- The key lives away from the store

### <!-- 3 -->📚 Documentation

- Re-record the demo for the key model
- What ctrl-c does, and why it is not obvious
- The split, and the cipher that replaced age
- The pluggable half, and a demo for it
- Stores, keys, recipients and the lua api
- The page, and a masked prompt at a tty

### <!-- 4 -->⚡ Performance

- Nucleo, and acronyms that read through flags
- Vendor age, cut what it cannot use

## [0.3.8] - 2026-08-14

## [0.3.7] - 2026-08-14

### <!-- 0 -->⛰️  Features

- A key you keep, recipients you publish
- Ctrl-space marks rows, delete twice confirms
- Split the filing from the crypto, drop age
- Hooks can call age over a pipe
- Hooks do the crypto, lua does the storage
- A store can hand its crypto to a program
- Stores, keys and recipients as an api
- Arrows walk the kinds, #tag filters
- A fifth kind — variables that hold a recipe
- Age behind a switch, and what it costs

### <!-- 1 -->🐛 Bug Fixes

- Ctrl-c ends a loop whose body forks
- Record against the binary under test
- An inherited row is not yours to edit
- A script directory belongs to one store
- The key lives away from the store

### <!-- 3 -->📚 Documentation

- Re-record the demo for the key model
- What ctrl-c does, and why it is not obvious
- The split, and the cipher that replaced age
- The pluggable half, and a demo for it
- Stores, keys, recipients and the lua api
- The page, and a masked prompt at a tty

### <!-- 4 -->⚡ Performance

- Nucleo, and acronyms that read through flags
- Vendor age, cut what it cannot use

## [0.3.6] - 2026-08-13

### <!-- 1 -->🐛 Bug Fixes

- A -c wrapper is not a shell you are in

## [0.3.5] - 2026-08-13

### <!-- 1 -->🐛 Bug Fixes

- The shell above must still be there

## [0.3.4] - 2026-08-13

### <!-- 1 -->🐛 Bug Fixes

- Name the terminal, not /dev/tty

## [0.3.3] - 2026-08-13

### <!-- 0 -->⛰️  Features

- The widgets, for shells that are not oslo
- Ask before nesting, count with OSLO_NESTED
- Keep a command's output, copy --last
- Which and whereis know this shell
- Aliases as a file another shell can source
- The database is canonical, the files are derived
- Edit, and a word about what $PATH answers first

### <!-- 1 -->🐛 Bug Fixes

- One terminal, one stack
- A foreign /bin/sh is a hint, not a warning
- One entry per thing, not per file
- Type and command -v see stored macros

### <!-- 3 -->📚 Documentation

- The widgets, and the three doors

## [0.3.2] - 2026-08-13

### <!-- 0 -->⛰️  Features

- Name the command, and two demos
- Say what the builtin is when there is no script
- Complete a script from its own comments
- Vendor argc, and parse a script's own arguments

### <!-- 1 -->🐛 Bug Fixes

- Flag completion, an oversized fd, and a pattern expanded per element
- A declared choice outranks a filename

### <!-- 3 -->📚 Documentation

- Re-record argc off camera, on a bigger screen
- Keep the finished argc plan in plans/

### <!-- 4 -->⚡ Performance

- Build the vendored parser for size

## [0.3.1] - 2026-08-12

### <!-- 0 -->⛰️  Features

- Alt-\ opens the manager at the prompt
- A script's language, bracketed and faint
- Colour the tags, align the columns
- Off for a session, off everywhere
- The manager screen, on the finder's shape
- A change reaches every running shell
- Created, tags, active, and a required kind
- Pick from the list, and open it
- Run a stored function or script
- Stored entries reach a starting shell
- The oslo aliases subcommand
- The store, and the snapshot a shell reads

### <!-- 1 -->🐛 Bug Fixes

- A path in KEY= is not a key
- Open the screen even with nothing stored

### <!-- 2 -->🚜 Refactor

- Aliases become macros, four kinds

### <!-- 3 -->📚 Documentation

- Publish the macros demo
- Keep the demo off the recorder's own aliases
- A demo for the macro manager
- Keep the finished macros plan in plans/
- The macro manager
- Plan the macros manager
- The alias manager
- Measure what an alias database would cost a script shell
- Keep the finished providers plan in plans/
- One store, and config loses to the database
- Plan an alias manager, and how to run a script from a database

## [0.3.0] - 2026-08-12

### <!-- 0 -->⛰️  Features

- When to ask, and two worked plugins
- Candidates a plugin can add
- Who wins when the slow answer lands
- A provider that answers when it can
- A ghost source a plugin can supply
- Specs a config can declare
- Oslo plugin test, a harness for authors
- Messages, what this session said
- Oslo config timing, what startup costs
- Group-by, count, uniq and stats
- Oslo.spawn, work off the prompt with a callback
- Load_on, waking a plugin that has nothing to type
- Oslo config files and which, for provenance
- Oslo.state, and a description on a keybinding
- Oslo plugin doctor, and a plugin's own checks
- A builtin may describe itself and its completion
- Oslo.after and oslo.every, fired between commands
- Events a plugin names itself
- A Lua tool may consume the rows that reached it
- Requires, against one authoritative version
- Manifest, index, trust gate and oslo plugin
- Pre-cmd may decline to have a line recorded
- Oslo.db, a database a config owns

### <!-- 1 -->🐛 Bug Fixes

- Say what a shell says, not what Rust says
- Final, not last — util-linux ships that name
- Distinct, not uniq — the name was coreutils'
- Answer the options a bashrc sets
- Help follows the history subcommand style

### <!-- 2 -->🚜 Refactor

- One channel for an answer that arrives late
- A directory per profile, copied forward
- The database is the history, the file an export

### <!-- 3 -->📚 Documentation

- What the providers cost, and what was not built
- Tiers are a menu idea, not a ghost one
- Plan tunable suggestion providers, after prior art
- Plan pluggable ghost and dropdown suggestions
- The six, in the README and the plan
- Plan six more, after composition
- Plan what a plugin still cannot do
- The feature page and a worked example
- Plan the plugin feature

### <!-- 7 -->⚙️ Miscellaneous Tasks

- Remove the ssh feature and its dependencies

### <!-- 9 -->◀️ Revert

- Drop the flat-layout migration

### Build

- Compile oslo for size, except the editor
- Vista v0.1.2, tagged rather than pinned by hash

## [0.2.29] - 2026-08-11

### <!-- 0 -->⛰️  Features

- Complete flake outputs for the nix command
- The named helpers, written in Lua
- Cache a json document until the flake moves
- Oslo.nix.run, any nix json command as Lua

### <!-- 1 -->🐛 Bug Fixes

- Reload drops both caches; name the shells a flake has
- Run a config-registered tool on its own

### <!-- 3 -->📚 Documentation

- Document the feature and fix for_command
- Plan the nix feature

## [0.2.28] - 2026-08-11

### <!-- 0 -->⛰️  Features

- Import what a dev shell's functions read
- Put directory environments behind a feature
- What a dev shell's build system needs
- Indirect expansion composes with an operator
- Import a dev shell's functions on request
- Run shellHook when a project asks
- Full width, with the history list's stripes
- The filter box, on top of the list
- Draw the finder the way tab-rs does
- Oslo tab, for whatever is not an oslo prompt
- A tab builtin, in the help behind the feature
- Draw the finder like the history one
- A daemon backend behind oslo.tab.daemon
- Open the finder from a prompt
- A create row in the list widget
- The client, matching the key in both encodings
- The keeper, its pty and the output log
- The runtime directory, store and wire format
- Add the feature flag and oslo.tab settings
- Put the model behind a vista feature, off
- Fix the command that just failed
- Bracket only the words that changed
- Show the correction as you type
- Repair failed lines from history
- Vendor vista and build the model

### <!-- 1 -->🐛 Bug Fixes

- Scratch is a tool row, not a section
- The same margin on both sides of a full-width list
- Keep the filter box, drop what is around it
- Filter box no narrower than its legend
- Ascii chevron, caret on its surface, narrow panel
- Keep the scrollback of the shell you left
- Give each tab the screen to itself
- Never return into the caller from a failed keeper
- Decode ctrl chords above the alphabet

### <!-- 2 -->🚜 Refactor

- Split nix out of direnv into its own feature
- Rename the tab feature to scratch

### <!-- 3 -->📚 Documentation

- Say which binary reads a directory file
- A dev shell's phases run here now
- Correct what stops a dev shell's functions
- Re-record the demo for the new finder
- Record and embed the demo
- The tab feature
- Plan the tab feature
- Say which binary has the model
- Record a demo for every feature
- One document per feature

### <!-- 6 -->🧪 Testing

- Wait for the second prompt mark, not the first

### <!-- 7 -->⚙️ Miscellaneous Tasks

- Release both oslo and oslo-minimal

### Build

- Run the tests behind a feature
- Testing helpers are not a cargo feature
- Add a full feature and make build-full
- Depend on vista-recall by commit, drop the copy
- Pin maki to a commit, record vista's

## [0.2.27] - 2026-08-09

### <!-- 0 -->⛰️  Features

- Provide parsed shell command data to Lua

## [0.2.26] - 2026-08-09

### <!-- 1 -->🐛 Bug Fixes

- Database issue n cleanup

## [0.2.25] - 2026-08-09

### <!-- 0 -->⛰️  Features

- Align history subcommand help
- Add portable sync tooling
- Show a slow rc file working as it works
- Switched the DB
- Type a name and walk into it
- A configurable mark instead of mode and kind

### <!-- 1 -->🐛 Bug Fixes

- Rm never offers a path it already deleted
- Keep room for the +N a counted row adds
- Read one scan, not two halves of different ones

## Unreleased

### <!-- 2 -->🚜 Refactor

- Move tracking storage from jammdb to Tagdata

## [0.2.24] - 2026-08-09

### <!-- 1 -->🐛 Bug Fixes

- Pass use flake arguments through to nix

## [0.2.23] - 2026-08-09

### <!-- 1 -->🐛 Bug Fixes

- Never swap in a prompt of another width
- Add the source file a global ignore swallowed

## [0.2.22] - 2026-08-09

### <!-- 1 -->🐛 Bug Fixes

- Two correctness bugs in for and compare
- Stale interrupt no longer eats a command

### <!-- 2 -->🚜 Refactor

- The top of the stack becomes oslo-runtime
- The shell becomes oslo-shell
- The interface layer becomes oslo-ui
- Ask the shell through a trait, not a store
- The bottom of the stack becomes oslo-base
- The interpreter becomes oslo-lua
- Reach a hook without knowing Lua exists

### <!-- 3 -->📚 Documentation

- Note what the dependency glob does not match

### <!-- 4 -->⚡ Performance

- An async prompt lands without a keystroke
- Stop rendering a spawned prompt 4 times
- Find a prefix by search, not by scan
- Stop rebuilding the world on every keystroke
- Drop five copies from the command path
- A plain word skips the lexer
- Publish LINENO and PIPESTATUS only on change
- A word with no wildcard skips the walk
- Share a function body instead of copying
- Share a function body across closures
- A call statement no longer clones its AST
- Score only the names that match
- Honour the redraw flag a key returns
- Skip the scan when nothing is aliased
- Stop asking the kernel twice per command
- An async prompt waits, briefly, for a fresh one
- Fat LTO now that the shell is six crates

### <!-- 6 -->🧪 Testing

- Measure what one keystroke costs

### <!-- 7 -->⚙️ Miscellaneous Tasks

- Keep other people's code in vendor/

## [0.2.21] - 2026-08-08

### <!-- 0 -->⛰️  Features

- Time the phases of a prompt
- Read .envrc with direnv's stdlib
- A directory navigator widget

### <!-- 1 -->🐛 Bug Fixes

- Size the block to what it shows
- The terminal names the shifted key

### <!-- 4 -->⚡ Performance

- Make the async prompt cache usable
- Warm the command table before forking
- Share and cache the nix dev shell

### <!-- 6 -->🧪 Testing

- Widen the margin on the settle window
- Hold the colour depth while asserting
- Stop a test writing the settings global
- Restore the deferred ratchet rows
- A child re-runs the file it inherits

## [0.2.20] - 2026-08-07

### <!-- 0 -->⛰️  Features

- Strange things
- Strange things

### <!-- 1 -->🐛 Bug Fixes

- A reply ends the wait, not the listening
- Ask nothing through a multiplexer
- Rank by the kind of match, then by recency

### <!-- 4 -->⚡ Performance

- Size-optimise dependencies, except the parser

### Build

- The lua derive lives inside full_moon
- Drop thiserror from both parsers and oslo
- Full_moon_derive on syn 2, without indexmap
- Vendor both parsers and strip what they dragged in
- Full_moon without its serde default

## [0.2.19] - 2026-08-07

### <!-- 0 -->⛰️  Features

- The scanner, the badge and the meta columns
- A look for lists, and history is one of them
- Legend, border, fullscreen and placement per widget
- On-report covers all five of the shell's blocks
- Oslo.ui.block, and on-report for direnv

### <!-- 1 -->🐛 Bug Fixes

- Recency orders the list; the caret keeps its surface
- Carry the undo record to child shells
- Reconcile the directory environment at the prompt
- The caret follows the cursor setting, and one atomic frame
- Tabs, the full-width box and the legend rule
- The box, the rule and the spacing around a widget
- Three oslo.ui names that shadowed or dropped
- Oslo.ui.style was installed twice and painted neither

### <!-- 2 -->🚜 Refactor

- Src/interactive is src/ui
- The history screen is a ui look

### <!-- 3 -->📚 Documentation

- Oslo.ui.block and on-report

## [0.2.18] - 2026-08-06

### <!-- 0 -->⛰️  Features

- Pipeline stages, and redaction for a replay
- One read gives a replay everything it needs
- Pre-record decides what is written down
- The log records who typed it and what it did
- Record what each link of a chain did
- Turn parts of the shell off at runtime

### <!-- 1 -->🐛 Bug Fixes

- The last chain survives the command asking about it
- Forget takes a line out of the log too

### <!-- 7 -->⚙️ Miscellaneous Tasks

- Cleanup

## [0.2.17] - 2026-08-06

### <!-- 0 -->⛰️  Features

- \cmd and \\cmd escape the builtin
- Sh.cmd expands globs, oslo.run opts in
- A resize redraws the line

### <!-- 1 -->🐛 Bug Fixes

- A hook that observes may change the shell

## [0.2.16] - 2026-08-06

### <!-- 0 -->⛰️  Features

- A margin at the top edge to match the bottom
- Every context field an external prompt can name
- Add the maki client behind an off-by-default feature
- Twenty named events across the shell
- A builtin rm with a trash at the prompt

### <!-- 1 -->🐛 Bug Fixes

- The cursor is not shown at column one mid-switch
- PWD is set and exported at startup
- The mode is published before the prompt is rebuilt
- Rebuild the prompt when its inputs change
- Export COLUMNS and LINES for child programs
- A burst is not read past the key it needs

### <!-- 6 -->🧪 Testing

- Every documented setting must be assignable

### <!-- 7 -->⚙️ Miscellaneous Tasks

- Cleanup

## [0.2.14] - 2026-08-05

### <!-- 0 -->⛰️  Features

- A lua hook can see every keypress
- Preexec and postexec get the command
- A lua binding can submit the line
- Complete inside a brace list

### <!-- 1 -->🐛 Bug Fixes

- -c takes the first non-option argument

## [0.2.13] - 2026-08-05

### <!-- 1 -->🐛 Bug Fixes

- -o name and the + option forms

### <!-- 3 -->📚 Documentation

- How to make oslo the system /bin/sh

## [0.2.12] - 2026-08-05

### <!-- 1 -->🐛 Bug Fixes

- The finished line drops its ghost
- Sh -c -- cmd runs cmd
- OSLO_ALLHIST=0 means off

## [0.2.11] - 2026-08-05

### <!-- 1 -->🐛 Bug Fixes

- Track src/track/log/tests.rs, drop --lua from the smoke test

## [0.2.10] - 2026-08-05

### <!-- 7 -->⚙️ Miscellaneous Tasks

- Sync Cargo.lock to 0.2.9

## [0.2.9] - 2026-08-05

### <!-- 7 -->⚙️ Miscellaneous Tasks

- Sync Cargo.lock to 0.2.8

## [0.2.8] - 2026-08-05

### <!-- 7 -->⚙️ Miscellaneous Tasks

- Cleanup

## [0.2.7] - 2026-08-05

### <!-- 0 -->⛰️  Features

- A login shell reads /etc/profile and ~/.profile
- Oslo.source runs a shell file in this shell
- OSLO_ALLHIST records sh -c commands
- Run a script a command at a time

### <!-- 1 -->🐛 Bug Fixes

- One warning per file, not per name
- IFS is a set variable; exit keeps the trap status
- Errexit exemption survives a compound
- Comments inside a heredoc substitution

### <!-- 6 -->🧪 Testing

- Assert the command is in the store, not the file
- Boot arch linux with oslo as /bin/sh
- Pin the errexit and-or compound bug

### <!-- 7 -->⚙️ Miscellaneous Tasks

- Rename benches to bench
- Cleanup

## [0.2.6] - 2026-08-04

### <!-- 0 -->⛰️  Features

- Warning box goes last, sized to the help
- Warn about install problems in --help
- Drop --lua, --sh, --no-vi; hide --posix
- Sh implies posix, drop --profile
- Oslo <tool> when no script of that name exists
- Tools by argv0, coloured help, --details
- Drop --vi, vi is the default
- Strict profile names, quicker scanner
- OSLO_PROFILE names a profile too
- Profile in the bar, tab switches it
- Name both stores after a profile, add --profile
- Five scopes on the arrow keys, tracked in the store

### <!-- 1 -->🐛 Bug Fixes

- Right accepts only outside vi normal mode
- Right accepts the ghost suggestion

### <!-- 2 -->🚜 Refactor

- Toggle key is config, not an env var
- Drop the argv0 symlink dispatch
- One store, not two
- Drop the legacy store adoption

### <!-- 3 -->📚 Documentation

- Add an ENVIRONMENT section to --help
- Drop -- from the option list
- Posix mode changes four things, not two
- Two known gaps are no longer gaps

## [0.2.5] - 2026-08-04

### <!-- 0 -->⛰️  Features

- Wider scanner, chevrons in the query colour
- Centre the delete question, bracket the buttons
- Confirm before deleting from history
- Scanner then chevrons in the search bar
- Knight rider scanner in the search bar
- Seed from the line, cursor, marks, delete
- Remove rustyline, oslo owns its line editor
- Finder, abbreviations and lua keys on the native editor
- Vi mode on the native editor
- Multi-line continuation on the native editor
- Completion and ghost hints on the native editor
- Run the shell on the native editor, opt-in
- Example that drives the native editor
- Redraw escapes and the editing state machine
- Ctrl/alt key decoding and the emacs keymap
- Native line buffer and layout engine

### <!-- 1 -->🐛 Bug Fixes

- Atomic redraws and the scanner on its surface
- Drop bold from match marks

### <!-- 6 -->🧪 Testing

- Answer the finder's delete confirmation

## [0.2.4] - 2026-08-04

### <!-- 0 -->⛰️  Features

- Brighter syntax colours, ansi slots untouched
- Universal variables shared between shells
- Autoload functions from functions/NAME.sh
- Title key, status builtin, named frames
- Preexec/prompt hooks, transient prompt, fish settings
- Real facts for every prompt, honest set -o
- Dynamic variables and the config fixes

### <!-- 4 -->⚡ Performance

- Drop duplicated digest stack via sha2 0.10
- Trim regex features, smaller and faster

### <!-- 6 -->🧪 Testing

- Stop a stdin-reading test hanging the suite

## [0.2.3] - 2026-08-04

### <!-- 0 -->⛰️  Features

- The remaining eight gum widgets
- Gum-style input widgets for shell and lua

### <!-- 1 -->🐛 Bug Fixes

- Draw the caret instead of moving the cursor
- An abort is not an ordinary quit
- Ctrl-c cancels and erases the widget
- Run a script the kernel refuses as ENOEXEC
- Stop the widgets eating the transcript

### <!-- 3 -->📚 Documentation

- A tour of the thirteen ui widgets

### <!-- 5 -->🎨 Styling

- A measured rule above every key legend

### Build

- Static musl by default, and accept --login

## [0.2.2] - 2026-08-03

### <!-- 0 -->⛰️  Features

- Full-screen fuzzy history search on up
- Let shell code set the right prompt
- Make bash shell integrations work end to end

### <!-- 1 -->🐛 Bug Fixes

- Swallow escape sequences, restyle the list

### <!-- 2 -->🚜 Refactor

- Remove bind and the command renderer

### <!-- 5 -->🎨 Styling

- Plain rows and a codex-shaped input surface

## [0.2.1] - 2026-08-03

### <!-- 0 -->⛰️  Features

- Run PROMPT_COMMAND before every prompt
- Let shell code claim a keystroke
- Run the DEBUG trap before each command

### <!-- 1 -->🐛 Bug Fixes

- A quoted empty default is still one field

## [0.2.0] - 2026-08-03

### <!-- 0 -->⛰️  Features

- Report what changed, not how many
- A real terminal library, and the escapes to use it
- The api is require-able, and you can write your own
- Nix_develop, the use flake equivalent
- A directory may set the prompt, and give it back
- Restore locals and export flags, add oslo.path_add
- One Lua config file, and direnv helpers from it
- Group and colour what a directory environment reports
- Load directory environments on cd
- The allow gate, env diff and dotenv reader
- The allow gate, the env diff and dotenv
- Fuzzy inline suggestions on by default
- Gap-capped fuzzy matching, off by default inline

### <!-- 1 -->🐛 Bug Fixes

- Signal a job by its spec, not only by pid
- A dev shell must not take your commands away
- The ghost is a continuation again; aliases unload
- Allow and deny take effect where you stand
- A fuzzy match must reach the first character

### <!-- 2 -->🚜 Refactor

- Drop the migrate module
- Group the api into libraries
- Oslo.direnv is a library, not loose functions
- Drop what nothing calls
- One file type, .env.lua and nothing else

### <!-- 3 -->📚 Documentation

- Re-measure size against other shells
- How big oslo is next to seven other shells
- Why --json loses what the bash form keeps
- Oslo's own .env.lua
- Move the known gaps out of the README
- The directory environment design
- The tracking store, the smarter cd and fuzzy matching

### <!-- 4 -->⚡ Performance

- Replace turso with jammdb behind one seam
- Turn off turso features oslo never used
- Tune the release profile, measure the rest
- Cache compiled regexes, fold fuzzy patterns once

### <!-- 6 -->🧪 Testing

- Pin the PATH round-trip

### <!-- 7 -->⚙️ Miscellaneous Tasks

- Drop --locked so a version bump cannot redden every job

## [0.1.4] - 2026-08-02

### <!-- 0 -->⛰️  Features

- *(complete)* gap-capped fuzzy matching, off by default inline
- *(suggest)* fuzzy inline suggestions on by default

### <!-- 2 -->🚜 Refactor

- *(ci)* drop --locked so a version bump cannot redden every job

## [0.1.3] - 2026-08-02

### <!-- 0 -->⛰️  Features

- Pink builtins and a padded sudo field
- Prune the store and seed the ring
- Record every command and suggest by directory
- Jump to a remembered directory
- The store behind a smarter cd
- Frecency scoring and match tiers

### <!-- 1 -->🐛 Bug Fixes

- Record what you run at home, never jump to it
- Checkpoint the log, group heads by tool

### <!-- 3 -->📚 Documentation

- The smart cd design

### <!-- 6 -->🧪 Testing

- Observe a job that is still running
- Do not inherit XDG_CONFIG_HOME

### <!-- 7 -->⚙️ Miscellaneous Tasks

- Install musl-tools for the C deps
- Reuse the cached cargo-fuzz binary
- Check the lockfile at msrv
- Sync Cargo.lock with the 0.1.2 bump

## [0.1.2] - 2026-08-02

### <!-- 7 -->⚙️ Miscellaneous Tasks

- Fix the release smoke test, publish build artifacts

## [0.1.1] - 2026-08-02

### <!-- 0 -->⛰️  Features

- Globstar as a real shopt option
- Match by pieces, not just prefix
- Register_tool for structured commands
- Directory ring with prevd, nextd and cd -N
- Ps and ls as row producers
- To json and the day-one verbs
- Lines, parse and from json
- Df and where, the first structured pipeline
- The planner and the posix byte-path assertion
- The structured pipeline value model
- Abbreviations that expand in the buffer
- Bind a key to a lua function
- Follow aliases, stop claiming a kind
- Command-not-found hook and did-you-mean
- Prefix search on up, keep the composed line
- Named styles and live mode in lua prompts
- Full PS1 escapes and external prompt commands
- Segment-based lua prompts
- One prompt for both languages, with the vi mode
- =command and @name at the prompt
- Mark where the prompt ends and input begins
- Ask the terminal its background and suit the palette
- Notify when a slow command finishes
- Paths in diagnostics are clickable
- Copy builtin puts text on the clipboard over OSC 52
- Report the working directory and set the title
- Vi mode is on by default
- First-class vi mode with per-mode cursor shapes
- Pin syntax colours to rgb and light sudo as danger
- Light globs, numbers, assignments and vars in strings
- A command says which directory it came from
- Host and user left, status and directory right
- Draw a right prompt by default
- Sh.ps() and sh.stat() finish the tool set
- Sh.env() and sh.ls() answer rows
- Sh.df() answers rows instead of text
- HISTSIZE bounds the database and history -c clears it
- Keep history in a database with its language
- Oslo.completion.for_command completes one command
- Oslo.completion.sources filters the kinds offered
- Oslo.completion.sort chooses the order
- Oslo.suggest.accept binds the suggestion keys
- Per-kind info columns, scriptable from Lua
- Refine completion candidate descriptions
- Bind keys from oslo.keys
- Make .oslorc Lua and add the settings surface
- Lua prompts, including a right prompt
- Suggest paths as well as history
- Highlight to fish's depth
- Theme the dropdown and badge the kind
- Emit OSC 133 command boundaries with block ids
- Add oslo.http with curl's certificate rules
- Stream command output with oslo.lines
- Add proc, job and the output converters
- Add introspection, options and hooks
- Read shell or Lua at the prompt
- Share one namespace with shell variables
- Add require, oslo.re and oslo.json
- Add the fs, path and os namespaces
- Add the argv call model and sh sugar
- Run the oslo.* API on oslo's own Lua
- Evaluate Lua without a C interpreter
- Implement process substitution
- Add LINENO, finish round A
- Build printf in, matching bash
- Argv, capture, cd, env, glob, exit
- Detect Lua vs shell, drop --lua-script
- Line editing, rc files, test debt
- Arrays, arithmetic cmds, regex, job control
- Conformance, shell options, traps
- Quoting provenance, params, arithmetic
- Add bash oracle, CI matrix, install

### <!-- 1 -->🐛 Bug Fixes

- Restore terminal modes after a foreground job
- Iterate tables in insertion order
- Never ghost a multi-line history entry
- Seven defects from the audit
- Repaint a wrapped line correctly
- Never write the final column
- Recall follows the language
- Completion and syntax follow the language
- Suggestions follow the live language
- One source of truth for the language
- Redraw the prompt in the current language
- Switch language on the first press
- Do not suggest shell commands in lua
- Keep shell and lua history separate
- Move the line when the prompt changes width
- Hold one width across languages
- Toggle language in place, quiet ctrl-c
- Esc enters normal mode on the first press
- Follow symlinks when indexing PATH
- Mode indicator now matches the editor
- Tab completes in normal mode too
- Repaint only the prompt, never the line
- Repaint the row when the vi mode changes
- Move the vi indicator where it can redraw
- Move back rather than restore the cursor
- Draw the right prompt without save/restore
- Case_sensitive actually decides the match
- A case pattern does not close a substitution
- Export NAME marks it without creating it
- A quoted ! is a class member, not a negation
- Stop the nesting guard refusing real scripts
- Erase rows a shorter page leaves behind
- Tab after accepting no longer undoes it
- Stop the menu eating the prompt; drop the border
- $? sees a command substitution in its word
- Run the shell on a stack oslo reserves
- A hash inside a word is not a comment
- Stop waiting for process substitutions
- Report write errors from echo and printf
- Skip comments when copying a construct
- Only empty parens make a function definition
- Stop substituting aliases twice
- Stop ignoring SIGPIPE
- Substitute aliases before parsing
- Let -- ends the options
- Make set -m turn job control on
- Let break end a loop from its condition
- Stop [[ ]] splitting and globbing operands
- Stop ${x+"$@"} joining its arguments
- Honour quoting inside shell patterns
- Report reserved words from command -v
- Stop nesting guard rejecting configure
- Reap reparented orphans when pid 1
- Stop nesting guard rejecting real scripts
- List inherited traps in subshells
- Honour -n and -a, run subshell EXIT traps
- Drop Linux-only probes and paths
- Use SIGUSR1's number, not Linux's 10
- Bound nesting to fit the smallest stack
- Read limits via getrlimit, not /proc
- Parse PROJECT outside make for 3.81
- Portable PROJECT parsing, richer CI errors
- Close remaining gaps, vendor parser patch
- Correct exit status, fds, subshell state
- Stop crashes, hangs, data-as-code

### <!-- 2 -->🚜 Refactor

- Rename rush to oslo

### <!-- 3 -->📚 Documentation

- Add cliff for changelogs
- Rewrite the readme around what oslo does
- Worked example for the dual-channel pipe
- Shell research and the dual-channel pipe design
- Decide the shape of a built-in tool
- Collapse four plans into one open-work list
- Close out the round C findings
- Locate the isset -x cause in export_var
- Record the nesting-guard and glob fixes
- Record the config and interactive plan
- Record the Lua layer decisions
- Record the builtin.t cause and the suite running
- Fix an unresolvable intra-doc link
- Plan the conformance oracle and pty tests
- Record $(case) gap and its workaround
- Fix readme heading spacing
- Record process substitution results
- Record round C sweep findings
- Note the bash-version gate in the corpus

### <!-- 4 -->⚡ Performance

- Stop blocking on the background query
- Warm the command index at startup

### <!-- 5 -->🎨 Styling

- Ghost text takes colour 240
- Ghost text takes colour 238
- Keep scope.rs inside the line limit
- Recolour the selected row and its badge
- Use colour 241 for the dir badge

### <!-- 6 -->🧪 Testing

- Cover the argv call model in the corpus
- Pin the brush comment bug in the ratchet
- Assert ^Z now the shell is not PID 1
- Boot a real Alpine userland under OpenRC
- Run modernish and job control in the VM
- Cover the four modernish findings
- Boot oslo as alpine pid 1 and /bin/sh
- Add lua corpus with recorded oracles
- Gate corpus cases on oracle bash version

### <!-- 7 -->⚙️ Miscellaneous Tasks

- Drop PROJECT, read metadata from Cargo.toml
- Target Linux only, drop platform gates
- Surface fmt and test failures as annotations
- Split verify into named stages for diagnosis
- Initial commit

### <!-- 9 -->◀️ Revert

- Drop oslo.http in favour of sh.curl

### Build

- Give the dev shell the musl target
- Add tokio and turso for the history database
- Track the brush integration branch
- Track the brush fork until #1253 lands
