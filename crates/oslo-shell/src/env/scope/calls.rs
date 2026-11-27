//! Who is calling whom: the shell-function call chain, and what a script can read of it.
//!
//! Two stacks. [`Environment::call_stack`] is the *functions* currently executing, which answers
//! "am I inside a function at all". [`Environment::frames`] is bash's own call stack: one frame
//! per function call, per `source`, and one `main` frame under a script file. It is published as
//! the three parallel arrays `FUNCNAME`, `BASH_SOURCE` and `BASH_LINENO`, innermost first, and
//! `caller` reads it.

use super::{Environment, ShellArray, UNNAMED_FUNCTION};
use oslo_base::error::Result;

/// What a [`CallFrame`] was entered by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameKind {
    /// The script file itself.
    Main,
    /// A `source` or `.` of a file.
    Source,
    /// A shell function call.
    Function,
}

/// One frame of bash's call stack.
#[derive(Debug, Clone)]
pub struct CallFrame {
    /// The `FUNCNAME` entry: the function's name, `source` or `main`.
    pub name: String,
    /// The `BASH_SOURCE` entry: the file the function was defined in, or the file being run.
    pub file: String,
    /// The `BASH_LINENO` entry: the line the frame was entered from, in the caller's file.
    pub line: u32,
    pub kind: FrameKind,
}

impl Environment {
    /// Begin a shell-function call; `Err` once the call chain is too deep to be safe.
    ///
    /// The caller must pair this with [`Self::exit_function`] on every path out of the call,
    /// including an unwinding `return` or error. A refused entry is not entered and must not be
    /// exited.
    pub fn enter_function(&mut self) -> Result<()> {
        self.enter_function_named(UNNAMED_FUNCTION)
    }

    /// Begin a shell-function call, recording which function it is and where it was called from.
    pub fn enter_function_named(&mut self, name: &str) -> Result<()> {
        self.function_depth.enter()?;
        self.call_stack.push(name.to_string());
        let file = self
            .function_files
            .get(name)
            .cloned()
            .unwrap_or_else(|| self.shell_name.clone());
        self.frames.push(CallFrame {
            name: name.to_string(),
            file,
            line: self.current_line,
            kind: FrameKind::Function,
        });
        self.publish_call_stack();
        Ok(())
    }

    pub fn exit_function(&mut self) {
        self.function_depth.exit();
        self.call_stack.pop();
        self.pop_frame(FrameKind::Function);
    }

    /// Drop the innermost frame if it is of `kind`, and republish the arrays.
    ///
    /// The caller's line is current again, so `$LINENO` names the line the frame was entered from
    /// until the next command publishes its own.
    fn pop_frame(&mut self, kind: FrameKind) {
        if self.frames.last().is_some_and(|frame| frame.kind == kind)
            && let Some(frame) = self.frames.pop()
            && kind != FrameKind::Main
        {
            self.resume_line(frame.line);
        }
        self.publish_call_stack();
    }

    /// The file a function defined now belongs to: the innermost frame's, or `$0` outside any.
    pub(super) fn defining_file(&self) -> String {
        match self.frames.last() {
            Some(frame) => frame.file.clone(),
            None => self.shell_name.clone(),
        }
    }

    /// Publish `FUNCNAME`, `BASH_SOURCE` and `BASH_LINENO`, innermost frame first.
    ///
    /// `FUNCNAME` is visible only while a function is executing, as in bash; the other two
    /// always describe the whole stack.
    fn publish_call_stack(&mut self) {
        let inner_first = || self.frames.iter().rev();
        let sources = ShellArray::from_values(inner_first().map(|f| f.file.clone()));
        let lines = ShellArray::from_values(inner_first().map(|f| f.line.to_string()));
        let names = (!self.call_stack.is_empty())
            .then(|| ShellArray::from_values(inner_first().map(|f| f.name.clone())));
        for (name, array) in [("BASH_SOURCE", Some(sources)), ("BASH_LINENO", Some(lines))]
            .into_iter()
            .chain([("FUNCNAME", names)])
        {
            match array {
                Some(array) => self.publish_array(name, array),
                None => {
                    self.vars.remove(name);
                }
            };
        }
    }

    /// Note how the code about to run was reached: `main` for a script file, `source` for a
    /// sourced one. Nothing is pushed for `-c` or for standard input, as in bash.
    pub fn enter_script_frame(&mut self, kind: &str) {
        self.script_frames.push(kind.to_string());
        if kind == "main" {
            self.frames.push(CallFrame {
                name: kind.to_string(),
                file: self.shell_name.clone(),
                line: 0,
                kind: FrameKind::Main,
            });
        }
        self.publish_call_stack();
    }

    /// Leave the frame [`Self::enter_script_frame`] pushed.
    pub fn exit_script_frame(&mut self) {
        if self.script_frames.pop().as_deref() == Some("main") {
            self.pop_frame(FrameKind::Main);
        }
    }

    /// Note the file whose commands are about to run, for the call stack and for a diagnostic's
    /// location.
    ///
    /// Its own stack rather than `$0`: a diagnostic from inside a sourced file names that file,
    /// while `$0` does not change across `source`. A sourced file is parsed whole, so its tree
    /// counts from its own line 1; the outer offset is put back by [`Self::exit_source_file`].
    pub fn enter_source_file(&mut self, path: &str) {
        self.frames.push(CallFrame {
            name: "source".to_string(),
            file: path.to_string(),
            line: self.current_line,
            kind: FrameKind::Source,
        });
        self.source_files.push(path.to_string());
        self.publish_call_stack();
        let outer = self.set_line_offset(0);
        self.source_offsets.push(outer);
    }

    /// Leave the file [`Self::enter_source_file`] pushed.
    pub fn exit_source_file(&mut self) {
        self.source_files.pop();
        self.pop_frame(FrameKind::Source);
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

    /// bash's call stack, outermost first.
    pub fn frames(&self) -> &[CallFrame] {
        &self.frames
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
    /// on to the next command, and so does `-c`.
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
