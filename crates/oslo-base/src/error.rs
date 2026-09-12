//! The one error type the evaluator unwinds with, and how it becomes an exit status.
//!
//! Four of the variants are not failures at all: `exit`, `return`, `break` and `continue` travel
//! as errors only because that is how a value escapes an arbitrarily deep evaluation. Anything
//! that turns an error into a number has to tell the two kinds apart, which is what
//! [`ShellError::control_flow_status`] is for — collapsing them all to 1 is exactly the bug
//! behind `( exit 3 )` reporting 1.

/// Written out rather than derived.
///
/// `thiserror` was one derive, in this one file, and it brings `syn`, `quote` and `proc-macro2`
/// into every build of the crate — three of the slowest dependencies there are, compiled before
/// anything else can start, to generate the sixty lines below. A shell with one error type does
/// not need a macro to write them.
#[derive(Debug)]
pub enum ShellError {
    SyntaxError(String),

    ExpansionError(String),

    /// An expansion that failed **because a parameter was unset or null** — `${v:?word}`, and any
    /// name at all under `set -u`.
    ///
    /// Its own variant because bash gives this one class a different exit status from every other
    /// fatal expansion error, and nothing else about it differs: it renders like an
    /// [`ShellError::ExpansionError`] and is caught wherever one is. See
    /// [`ShellError::fatal_exit_status`] for the measurements.
    UnsetParameter(String),

    /// An expansion the shell could not make sense of at all — `${x!!}`, `$((1/0))`.
    ///
    /// Also its own variant, and for the same reason: bash escalates this class, and only this
    /// class, when the shell is in POSIX mode. A lookup that came back with the wrong thing —
    /// `${!x}` on an unset `x`, `${!v}` through a value that is not a name — does not escalate,
    /// which is why "the expansion failed" is three variants and not one.
    MalformedExpansion(String),

    /// A pattern that matched nothing while `failglob` is on, as the text it was written as.
    ///
    /// Its own variant because bash abandons **the top-level command** it occurred in — the rest
    /// of that line, the whole `if` around it, the function it was called from — and then carries
    /// on with the next one, where every other expansion error ends a script. Only the outermost
    /// command list catches it; see `exec::pipeline`.
    NoMatch(String),

    ExecutionError(String),

    Io(std::io::Error),

    Lua(crate::value::LuaError),

    Nix(nix::Error),

    Exit(i32),

    Return(i32),

    Break(usize),

    Continue(usize),

    /// A **utility error**: the command failed in one of the ways POSIX 2.8.1 calls fatal to a
    /// non-interactive POSIX-mode shell — a bad option, a bad operand, a variable-assignment
    /// error or a redirection error — as opposed to an ordinary non-zero exit status.
    ///
    /// The distinction is the whole point, and bash draws it exactly here:
    ///
    /// ```text
    /// bash --posix -c 'shift 5; echo alive'            -> diagnostic, "alive", status 0
    /// bash --posix -c 'export BAD-NAME=1; echo alive'  -> diagnostic, no "alive", status 1
    /// ```
    ///
    /// `shift 5` is an ordinary failure; `export BAD-NAME=1` is a utility error. Builtins return
    /// `Result<i32>` and a usage error is an indistinguishable `Ok(2)`, so a shell that keyed off
    /// "status != 0" would kill itself on `shift`. This variant is the marker that tells them
    /// apart; `crate::exec::simple::posix` is the only place that acts on it.
    ///
    /// **The diagnostic is already on stderr when this is raised.** Whoever detected the error
    /// printed it, in the wording that error deserves; `context` exists so that a path which
    /// renders the error anyway says something true rather than nothing.
    UtilityError {
        /// What went wrong, for rendering only — never the primary report.
        context: String,
        /// The status the failed *command* takes on where the shell carries on.
        status: i32,
        /// The status the *shell* exits with where POSIX says it must not carry on.
        fatal: i32,
    },
}

/// An `io::Error`'s reason, without the number Rust appends to it.
///
/// **`Display` for `io::Error` ends in ` (os error 30)`, and no shell prints that.** These strings
/// reach the user as `oslo: /etc/thing: Read-only file system`, which is the shape every other shell
/// uses and the shape that ends up in a package build log; the errno is an implementation detail of
/// the language oslo happens to be written in.
///
/// Found by running 816 Ubuntu maintainer scripts under `dash` and under oslo and diffing: nothing
/// *behaved* differently, and this was most of what the diff was full of.
pub fn reason(e: &std::io::Error) -> String {
    let said = e.to_string();
    let Some(at) = said.rfind(" (os error ") else {
        return said;
    };
    // **The number has to be a number.** Trimming on the phrase alone turned an error that merely
    // mentioned it — `failed (os error reading the manual)` — into `failed`, which is a shell
    // swallowing the half of a message that said what happened. Its own test caught that.
    let inside = &said[at + " (os error ".len()..];
    match inside.strip_suffix(')') {
        Some(digits) if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) => {
            said[..at].to_string()
        }
        _ => said,
    }
}

impl std::fmt::Display for ShellError {
    /// The wording reaches the user through `oslo: {error}`, so it is part of the interface.
    ///
    /// **`ExecutionError` prints no category of its own.** Its message is already
    /// `what: why` — `/etc/default/alsa: Read-only file system` — and bash writes exactly that.
    /// "Execution error: " in front of it named the enum variant rather than telling anybody
    /// anything. `Syntax error` keeps its own, because that *is* what went wrong and two tests use
    /// it as the signal that a script parsed.
    ///
    /// **`ExpansionError` prints no category either, for the same reason as `ExecutionError`.** Its
    /// messages are already `what: why` — `NOPE: unbound variable` — and the one shape that made
    /// the prefix indefensible is `${x:?message}`, where POSIX says the *user's own* words go to
    /// stderr and oslo was writing `Expansion error: x: my message` in front of them.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // **Lower case, and only once.** bash writes `file: line 3: syntax error: …`, and so
            // does every diagnostic here now that they carry a location — a capital in the middle
            // of one reads as a new sentence. The guard is for the vendored parser, whose own
            // wording already opens `syntax error at …`: with an unconditional prefix that came
            // out as `Syntax error: syntax error at end of input`.
            ShellError::SyntaxError(m) if m.starts_with("syntax error") => write!(f, "{m}"),
            ShellError::SyntaxError(m) => write!(f, "syntax error: {m}"),
            ShellError::ExpansionError(m)
            | ShellError::UnsetParameter(m)
            | ShellError::MalformedExpansion(m) => write!(f, "{m}"),
            ShellError::NoMatch(pattern) => write!(f, "no match: {pattern}"),
            ShellError::ExecutionError(m) => write!(f, "{m}"),
            ShellError::Io(e) => write!(f, "{}", reason(e)),
            ShellError::Lua(e) => write!(f, "Lua error: {e}"),
            ShellError::Nix(e) => write!(f, "POSIX error: {e}"),
            ShellError::Exit(c) => write!(f, "Builtin exit requested with code: {c}"),
            ShellError::Return(c) => write!(f, "Return called with code: {c}"),
            ShellError::Break(d) => write!(f, "Break called with depth: {d}"),
            ShellError::Continue(d) => write!(f, "Continue called with depth: {d}"),
            ShellError::UtilityError { context, .. } => write!(f, "{context}"),
        }
    }
}

impl std::error::Error for ShellError {
    /// The three wrapped errors keep their chain, which is what `#[from]` gave them. Everything
    /// else carries a `String` and has no source to point at.
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ShellError::Io(e) => Some(e),
            ShellError::Lua(e) => Some(e),
            ShellError::Nix(e) => Some(e),
            _ => None,
        }
    }
}

// The three `#[from]` conversions, so `?` still works on the errors the shell actually propagates.
impl From<std::io::Error> for ShellError {
    fn from(e: std::io::Error) -> Self {
        ShellError::Io(e)
    }
}

impl From<crate::value::LuaError> for ShellError {
    fn from(e: crate::value::LuaError) -> Self {
        ShellError::Lua(e)
    }
}

impl From<nix::Error> for ShellError {
    fn from(e: nix::Error) -> Self {
        ShellError::Nix(e)
    }
}

impl ShellError {
    /// A utility error raised by a builtin: `status` is what the command reports, and it is also
    /// what the shell exits with in POSIX mode — `bash --posix -c 'export BAD-NAME=1'` exits 1,
    /// which is `export`'s own status, and `set -o nosuchopt` exits 2, which is `set`'s.
    pub fn utility_error(context: impl Into<String>, status: i32) -> Self {
        ShellError::UtilityError {
            context: context.into(),
            status,
            fatal: status,
        }
    }

    /// A *variable assignment error*: `r=2` where `r` is read-only, or a name `environ` cannot
    /// hold.
    ///
    /// The two statuses genuinely differ here, which is why the variant carries both. The failed
    /// assignment is worth 1 (`bash -c $'readonly r=1\nr=2\necho "$?"'` prints 1), but a POSIX
    /// shell does not carry on at all, and it abandons its program with the status every other
    /// abandoned program gets — see [`ShellError::fatal_exit_status`].
    pub fn assignment_error(context: impl Into<String>) -> Self {
        ShellError::UtilityError {
            context: context.into(),
            status: 1,
            fatal: FATAL_EXIT_STATUS,
        }
    }

    /// The status this error *carries*, for the variants that are control flow rather than
    /// failure — `None` for a genuine error.
    ///
    /// `exit n` and `return n` carry `n`. `break`/`continue` carry nothing: when one escapes the
    /// last enclosing loop there is no loop left to complain to, and every POSIX shell treats it
    /// as a silent no-op with the status of whatever ran before it, so 0 here.
    pub fn control_flow_status(&self) -> Option<i32> {
        match self {
            ShellError::Exit(code) | ShellError::Return(code) => Some(*code),
            ShellError::Break(_) | ShellError::Continue(_) => Some(0),
            _ => None,
        }
    }

    /// The status a genuine failure reports where it is caught.
    ///
    /// 2 for a syntax error — the number POSIX reserves for one and the number `make` and CI
    /// scripts test for — and 1 for everything else. A *fatal* expansion error aborting a
    /// non-interactive shell is a different question, answered by `main`: bash exits 127 there,
    /// but the same error inside a subshell or a pipeline stage still reports 1.
    pub fn failure_status(&self) -> i32 {
        match self {
            ShellError::SyntaxError(_) => 2,
            ShellError::UtilityError { status, .. } => *status,
            _ => 1,
        }
    }

    /// The status a fatal error gives a *non-interactive* shell that had to abandon its script.
    ///
    /// **127 is for three narrow cases, not for fatal errors in general.** It used to be the
    /// answer to all of them, on the strength of `bash -c 'unset v; echo ${v:?}'` answering 127 —
    /// true, and not a rule. Measured against bash 5.3 across every fatal error this shell can
    /// raise, `bash -c` answers:
    ///
    /// ```text
    ///   unset x; echo ${x:?}          127     echo $((1/0))              1
    ///   set -u; echo ${nope}          127     echo ${!x}                 1
    ///   echo $(if)                    127     echo ${x!!}                1
    ///   set -o posix; readonly r=1; r=2   127 echo ${1x}                 1
    /// ```
    ///
    /// So the 127 group is: a parameter that is unset or null, a syntax error from inside a
    /// `$( )`, and a POSIX-mode assignment error — which is what [`FATAL_EXIT_STATUS`] was named
    /// for. Every other fatal expansion error is 1, and oslo was answering 127 to all of them:
    /// `$((1/0))` reported the status of a missing command.
    ///
    /// **A script file is a different question again.** Run as a file rather than through `-c`,
    /// bash answers 1 for every one of the eight above and 2 for a bare syntax error — the `-c`
    /// path exits by a route that leaves 127 behind. `main` narrows this to 1 for a script, which
    /// is where the number is actually read.
    ///
    /// The remaining exception is `${!name}` through a value that is not a name, which bash
    /// unwinds by a different path and exits 1 for. Recognising it by its message is a stopgap —
    /// the honest fix is a distinct variant raised where `expand::param` handles indirection —
    /// but that message is built in exactly one place, and the alternative is knowingly reporting
    /// the wrong number.
    /// The status a [`ShellError::UtilityError`] leaves behind when the caller has decided the
    /// shell survives it — `None` for every other error.
    ///
    /// The point is the *diagnostic*: a utility error's message is already on stderr, so a caller
    /// that let it reach `exec::pipeline::report_error_status` would print it twice.
    /// `command` and `builtin` reach a builtin without going through
    /// `crate::exec::simple::posix`, and POSIX 2.9.1.1 says `command` strips a special builtin of
    /// exactly the property that would have made the error fatal, so folding here is also the
    /// right answer and not merely the quiet one.
    pub fn survivable_utility_status(&self) -> Option<i32> {
        match self {
            ShellError::UtilityError { status, .. } => Some(*status),
            _ => None,
        }
    }

    pub fn fatal_exit_status(&self, posix: bool) -> i32 {
        match self {
            ShellError::SyntaxError(_) | ShellError::UnsetParameter(_) => FATAL_EXIT_STATUS,
            // The one class POSIX mode escalates, and only there.
            ShellError::MalformedExpansion(_) if posix => FATAL_EXIT_STATUS,
            ShellError::UtilityError { fatal, .. } => *fatal,
            other => other.failure_status(),
        }
    }
}

/// The status a non-interactive shell reports when it abandons the program it was given.
///
/// Named because three unrelated failures share it and must keep sharing it: a fatal expansion
/// error, a syntax error raised from a `$( )` body, and a POSIX-mode variable assignment error.
/// `bash -c` answers 127 for all three.
pub const FATAL_EXIT_STATUS: i32 = 127;

/// Tail of the diagnostic `expand::param` raises for `${!name}` when `name`'s value is not a
/// name. Lives here because [`ShellError::fatal_exit_status`] is its only reader.
pub const INVALID_NAME_SUFFIX: &str = ": invalid variable name";

pub type Result<T> = std::result::Result<T, ShellError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_and_return_carry_their_code() {
        assert_eq!(ShellError::Exit(3).control_flow_status(), Some(3));
        assert_eq!(ShellError::Exit(255).control_flow_status(), Some(255));
        assert_eq!(ShellError::Return(2).control_flow_status(), Some(2));
    }

    #[test]
    fn loop_control_is_a_silent_zero() {
        assert_eq!(ShellError::Break(1).control_flow_status(), Some(0));
        assert_eq!(ShellError::Continue(4).control_flow_status(), Some(0));
    }

    #[test]
    fn real_errors_carry_no_status() {
        assert_eq!(
            ShellError::ExpansionError("x".into()).control_flow_status(),
            None
        );
        assert_eq!(ShellError::SyntaxError("x".into()).failure_status(), 2);
        assert_eq!(ShellError::ExpansionError("x".into()).failure_status(), 1);
        assert_eq!(ShellError::ExecutionError("x".into()).failure_status(), 1);
    }

    /// **Three classes, three answers**, which is what one `ExpansionError` could not express.
    /// The numbers are bash's, measured; see [`ShellError::fatal_exit_status`].
    #[test]
    fn each_class_of_fatal_expansion_gives_up_with_its_own_status() {
        // A parameter that is unset or null: 127 whatever the mode.
        let unset = ShellError::UnsetParameter("v: is unset".into());
        assert_eq!(unset.failure_status(), 1);
        assert_eq!(unset.fatal_exit_status(false), 127);
        assert_eq!(unset.fatal_exit_status(true), 127);

        // An expansion that makes no sense: 1, and 127 only under POSIX mode.
        let malformed = ShellError::MalformedExpansion("division by 0".into());
        assert_eq!(malformed.failure_status(), 1);
        assert_eq!(malformed.fatal_exit_status(false), 1);
        assert_eq!(malformed.fatal_exit_status(true), 127);

        // A lookup that came back wrong: 1, and POSIX mode does not escalate it.
        let lookup = ShellError::ExpansionError(format!("not a name{INVALID_NAME_SUFFIX}"));
        assert_eq!(lookup.failure_status(), 1);
        assert_eq!(lookup.fatal_exit_status(false), 1);
        assert_eq!(lookup.fatal_exit_status(true), 1);

        // A syntax error from a `$( )` body: 127, like the unset class.
        let syntax = ShellError::SyntaxError("in a $( ) body".into());
        assert_eq!(syntax.fatal_exit_status(false), 127);
    }

    /// A builtin's utility error reports the builtin's own status on both paths: `export` fails
    /// with 1 and ends a POSIX shell with 1, `set` with 2 and 2.
    #[test]
    fn a_builtin_utility_error_reports_one_status() {
        let err = ShellError::utility_error("export: `BAD-NAME=1': not a valid identifier", 1);
        assert_eq!(err.failure_status(), 1);
        assert_eq!(err.fatal_exit_status(false), 1);
        assert_eq!(
            ShellError::utility_error("x", 2).fatal_exit_status(false),
            2
        );
    }

    /// An assignment error reports two: the command failed (1), but the shell gave up (127).
    /// Collapsing them is how `readonly r=1; r=2` ends up reporting the wrong number on one side
    /// or the other.
    #[test]
    fn an_assignment_error_reports_two_statuses() {
        let err = ShellError::assignment_error("r: is read only");
        assert_eq!(err.failure_status(), 1);
        assert_eq!(err.fatal_exit_status(false), FATAL_EXIT_STATUS);
        assert_eq!(err.control_flow_status(), None);
        assert_eq!(err.to_string(), "r: is read only");
    }

    #[test]
    fn a_non_expansion_failure_keeps_its_own_status() {
        assert_eq!(
            ShellError::ExecutionError("x".into()).fatal_exit_status(false),
            1
        );
    }

    /// **No shell prints an errno.** These strings end up in package build logs, and
    /// `Permission denied (os error 13)` there says oslo is written in Rust rather than saying
    /// what went wrong.
    #[test]
    fn an_io_reason_carries_no_errno() {
        let denied = std::io::Error::from_raw_os_error(13);
        assert!(denied.to_string().contains("os error"), "the leak is real");
        assert_eq!(reason(&denied), "Permission denied");

        let missing = std::io::Error::from_raw_os_error(2);
        assert_eq!(reason(&missing), "No such file or directory");
    }

    /// An error that never had a code keeps every word of what it said.
    #[test]
    fn an_error_without_an_errno_is_left_alone() {
        let made_up = std::io::Error::other("the tape ran out");
        assert_eq!(reason(&made_up), "the tape ran out");
        // And a message that merely mentions the words is not truncated.
        let awkward = std::io::Error::other("failed (os error reading the manual)");
        assert_eq!(reason(&awkward), "failed (os error reading the manual)");
    }

    /// **`ExecutionError` names no category.** Its message is already `what: why`, which is what
    /// bash writes; the variant's name in front of it told nobody anything.
    #[test]
    fn an_execution_error_reads_the_way_bash_writes_one() {
        let failed = ShellError::ExecutionError("/etc/thing: Read-only file system".to_string());
        assert_eq!(failed.to_string(), "/etc/thing: Read-only file system");

        // The one that *is* a category keeps it, in lower case, the way bash writes it.
        let syntax = ShellError::SyntaxError("unexpected end of input".to_string());
        assert_eq!(syntax.to_string(), "syntax error: unexpected end of input");

        // And it is written once. The vendored parser's own wording opens with the category, so
        // an unconditional prefix produced `Syntax error: syntax error at end of input`.
        let parser = ShellError::SyntaxError("syntax error at end of input".to_string());
        assert_eq!(parser.to_string(), "syntax error at end of input");
    }

    /// An expansion error is `what: why` already, so it prints as itself.
    ///
    /// **The case that settled it**: `${x:?my message}` is POSIX's way of saying "fail with *this*
    /// wording", and a category in front of the user's own words is the one thing it must not do.
    #[test]
    fn an_expansion_error_does_not_name_its_own_category() {
        let unbound = ShellError::ExpansionError("NOPE: unbound variable".to_string());
        assert_eq!(unbound.to_string(), "NOPE: unbound variable");

        let asked_for = ShellError::ExpansionError("x: my message".to_string());
        assert_eq!(asked_for.to_string(), "x: my message");
    }
}
