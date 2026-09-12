//! Who is calling whom: the shell-function call chain, and what a script can read of it.
//!
//! Two stacks, and keeping them apart is the point. [`Environment::call_stack`] is the *functions*
//! currently executing, which is what `caller` reports and what it asks "am I inside a function at
//! all" — a question the `while caller $i` idiom depends on. `$FUNCNAME` is nearly the same list
//! and not quite: bash puts the way the code was reached on the end of it, so a function called
//! from a script file reads `f main` and one called from a sourced file reads `f source`. Those
//! are not function frames, so they live in their own list and are added only where the array is
//! published.

use super::{Environment, ShellArray, UNNAMED_FUNCTION};
use oslo_base::error::Result;

impl Environment {
    /// Begin a shell-function call; `Err` once the call chain is too deep to be safe.
    ///
    /// The caller must pair this with [`Self::exit_function`] on every path out of the call,
    /// including an unwinding `return` or error. A refused entry is not entered and must not be
    /// exited.
    ///
    /// Prefer [`Self::enter_function_named`]: `caller` can only report a name that was recorded
    /// on the way in, and this form records the placeholder bash prints when it has none.
    pub fn enter_function(&mut self) -> Result<()> {
        self.enter_function_named(UNNAMED_FUNCTION)
    }

    /// Begin a shell-function call, recording which function it is.
    ///
    /// The name is what `caller n` reports as the second field. Kept beside the depth counter
    /// rather than in a table of its own so the two cannot drift: one push, one pop, both here.
    pub fn enter_function_named(&mut self, name: &str) -> Result<()> {
        self.function_depth.enter()?;
        self.call_stack.push(name.to_string());
        self.publish_call_stack();
        Ok(())
    }

    pub fn exit_function(&mut self) {
        self.function_depth.exit();
        self.call_stack.pop();
        self.publish_call_stack();
    }

    /// Publish `$FUNCNAME`, which is the call stack a script can read.
    ///
    /// **It was empty, and said nothing about being empty.** `f() { echo "$FUNCNAME"; }` printed a
    /// blank line where bash prints `f` — so a log line or an error handler built on it lost the
    /// one piece of information it existed to carry, silently, in every script that used one.
    ///
    /// An array, as in bash: `${FUNCNAME[0]}` is the function running now and `${FUNCNAME[1]}` is
    /// whoever called it, so the order is the reverse of [`Self::call_stack`], which reads
    /// outermost first. A bare `$FUNCNAME` is element 0, which the array machinery already does.
    ///
    /// Rebuilt on entry and exit rather than synthesised when read, because `get_array` hands back
    /// a reference into the table and cannot make one up. The depth is capped at
    /// [`MAX_FUNCTION_DEPTH`], so the copy is bounded and small — the same way `PIPESTATUS` is
    /// published by whoever computes it.
    ///
    /// Unset outside every function, which is also bash: `${FUNCNAME+set}` is how a script asks
    /// whether it is inside one at all.
    fn publish_call_stack(&mut self) {
        if self.call_stack.is_empty() {
            self.arrays.remove("FUNCNAME");
            return;
        }
        let frames: Vec<String> = self
            .call_stack
            .iter()
            .rev()
            .chain(self.script_frames.iter().rev())
            .cloned()
            .collect();
        self.set_array("FUNCNAME", ShellArray::from_values(frames));
    }

    /// Note how the code about to run was reached, for `$FUNCNAME`'s outermost entries.
    ///
    /// `main` for a script file and `source` for a sourced one, which is what bash calls them.
    /// Nothing is pushed for `-c` or for standard input, and bash pushes nothing there either.
    pub fn enter_script_frame(&mut self, kind: &str) {
        self.script_frames.push(kind.to_string());
        self.publish_call_stack();
        self.publish_source_stack();
    }

    /// Leave the frame [`Self::enter_script_frame`] pushed.
    pub fn exit_script_frame(&mut self) {
        self.script_frames.pop();
        self.publish_call_stack();
        self.publish_source_stack();
    }

    /// `$BASH_SOURCE` — the files being executed, innermost first.
    ///
    /// **`dirname "${BASH_SOURCE[0]}"` is how a bash script finds its own directory**, and with
    /// nothing here it expanded to the empty string: the usual
    /// `cd "$(dirname "${BASH_SOURCE[0]}")" && pwd` then answered the *caller's* directory instead
    /// of the script's, silently and with status 0. Every script that loads a file beside itself
    /// depends on this, and it is the reason `$BASH_SOURCE` cannot simply be `$0` — a sourced file
    /// has to name itself, while `$0` deliberately does not change across `source`.
    ///
    /// The stack is the one [`Self::enter_source_file`] already keeps for diagnostics, with the
    /// script itself underneath. Unset where bash leaves it unset: `-c` and standard input push no
    /// script frame and source nothing.
    fn publish_source_stack(&mut self) {
        if self.script_frames.is_empty() && self.source_files.is_empty() {
            self.arrays.remove("BASH_SOURCE");
            return;
        }
        let mut frames: Vec<String> = self.source_files.iter().rev().cloned().collect();
        if !self.script_frames.is_empty() {
            frames.push(self.shell_name.clone());
        }
        self.set_array("BASH_SOURCE", ShellArray::from_values(frames));
    }

    /// Note the file whose commands are about to run, for a diagnostic's location.
    ///
    /// **Its own stack rather than `$0`.** A diagnostic from inside a sourced file names *that*
    /// file — bash reports `inner.sh: line 4:` for a failure there, not the script that sourced it
    /// — but `$0` deliberately does *not* change across `source`, because POSIX says a sourced
    /// file shares the caller's positional parameters and `$0` is one of them. Swapping
    /// `shell_name` would have made the message right and `$0` wrong.
    ///
    /// A stack because a sourced file may source another, and the innermost is the one a failure
    /// belongs to. See [`Environment::origin`](crate::env::Environment::origin).
    pub fn enter_source_file(&mut self, path: &str) {
        self.source_files.push(path.to_string());
        self.publish_source_stack();
        // A sourced file is parsed whole, so its tree already counts from its own line 1. The
        // outer file's offset is put back by [`Self::exit_source_file`] — see `set_line_offset`.
        let outer = self.set_line_offset(0);
        self.source_offsets.push(outer);
    }

    /// Leave the file [`Self::enter_source_file`] pushed.
    pub fn exit_source_file(&mut self) {
        self.source_files.pop();
        self.publish_source_stack();
        if let Some(outer) = self.source_offsets.pop() {
            self.set_line_offset(outer);
        }
    }

    /// The file a diagnostic should name: the innermost sourced one, or `$0` for the script itself.
    pub(crate) fn current_file(&self) -> &str {
        self.source_files.last().unwrap_or(&self.shell_name)
    }

    /// The shell functions currently executing, innermost last.
    ///
    /// A frame entered through [`Self::enter_function`] rather than
    /// [`Self::enter_function_named`] reads as [`UNNAMED_FUNCTION`].
    pub fn call_stack(&self) -> &[String] {
        &self.call_stack
    }

    /// Whether a shell function is currently executing.
    ///
    /// `local` needs this rather than the scope-frame stack, because a prefix assignment
    /// (`FOO=bar cmd`) pushes a frame too — so a non-empty stack does not mean "inside a
    /// function", and `local x=1` at the top level would silently create a global.
    pub fn in_function(&self) -> bool {
        self.function_depth.depth() > 0
    }

    /// Whether a `.`/`source` is running, which is the other place `return` is allowed.
    ///
    /// A *script file* is not one: `bash script.sh` with a top-level `return` refuses it and goes
    /// on to the next command, and so does `-c`. That is the distinction [`enter_script_frame`]
    /// records as `source` versus `main`.
    ///
    /// [`enter_script_frame`]: Self::enter_script_frame
    pub fn in_sourced_script(&self) -> bool {
        self.script_frames.iter().any(|kind| kind == "source")
    }

    /// Note that an `exit` was refused because jobs are stopped.
    pub fn note_exit_warned(&mut self) {
        self.exit_warned = true;
    }

    /// Whether an `exit` has already been refused over stopped jobs, clearing the record.
    ///
    /// **The confirmation lasts until it is used, not until the next command.** bash clears its own
    /// on any intervening command, which is stricter; matching that needs a per-command counter to
    /// compare against, and the warning — not the expiry — is what stops somebody walking away
    /// from a stopped job without knowing.
    pub fn take_exit_warned(&mut self) -> bool {
        std::mem::take(&mut self.exit_warned)
    }
}
