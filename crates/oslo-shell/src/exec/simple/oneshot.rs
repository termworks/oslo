//! bash's unforked last command: the final simple command of a `-c` program, a `( … )`, a command
//! or process substitution, a pipeline stage or a background job runs in the process that would
//! otherwise have waited for it. A program it names is `exec`ed without a fork, and a
//! `command_not_found_handle` it reaches runs in that process.

use crate::env::Environment;
use oslo_base::ast::{Command, CommandList, CompoundCommand, ListItem, ListOp, SimpleCommand};
use std::cell::Cell;

thread_local! {
    /// What the next command list this process runs is, when the process ends with it.
    static TAIL_LIST: Cell<Option<Tail>> = const { Cell::new(None) };
    /// The address of the simple command this process ends with, or 0.
    static TAIL: Cell<usize> = const { Cell::new(0) };
    /// The simple command running now is that one, and is not a function call.
    static CURRENT: Cell<bool> = const { Cell::new(false) };
    /// What the marked command ends: a list or a background job, or `None` for a pipeline stage.
    static KIND: Cell<Option<Tail>> = const { Cell::new(None) };
    /// What kind of child this process is, as bash's `subshell_environment` records it.
    static ENCLOSURE: Cell<Enclosure> = const { Cell::new(Enclosure::Shell) };
    /// Whether the `-c` program has reached its last line, so nothing is left to read.
    static LAST_LINE: Cell<bool> = const { Cell::new(false) };
}

/// The kind of process a command runs in, for whether `exec` lowers `$SHLVL`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Enclosure {
    /// The shell itself, a background job, or a substitution of either.
    Shell,
    /// A `( … )` or a coprocess, or a substitution inside one: `exec` keeps the level.
    Paren,
    /// A pipeline stage, or a substitution inside one: the last command keeps the level.
    Pipe,
}

/// This process became a child of `kind`; command and process substitutions keep their parent's.
pub(crate) fn enclose(kind: Enclosure) {
    ENCLOSURE.with(|enclosure| enclosure.set(kind));
}

/// Whether `exec` with a program lowers `$SHLVL` here, which it does outside a `( … )`.
pub(crate) fn exec_lowers_level() -> bool {
    ENCLOSURE.with(Cell::get) != Enclosure::Paren
}

/// Which list a process ends with, which decides whether its last command may redirect.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tail {
    /// A `-c` program: the command may not redirect.
    Program,
    /// A command substitution, whose command may not redirect either.
    Substitution,
    /// A `( … )` body, whose command may redirect when it is the only one.
    Subshell,
    /// A process substitution, whose last command may redirect.
    ProcSub,
    /// A background job's lone command, or the end of the function body it calls.
    Async,
}

fn address(simple: &SimpleCommand) -> usize {
    std::ptr::from_ref(simple) as usize
}

/// The next command list this process runs is the one it ends with.
pub(crate) fn tail_list_next(kind: Tail) {
    TAIL_LIST.with(|tail| tail.set(Some(kind)));
}

/// Take the mark [`tail_list_next`] left, for the list starting now.
pub(crate) fn take_tail_list() -> Option<Tail> {
    TAIL_LIST.with(Cell::take)
}

/// The `-c` program is running a command on its last line.
pub(crate) fn reach_last_line() {
    LAST_LINE.with(|last| last.set(true));
}

/// The process ends with `simple`, the end of `kind`, or a pipeline stage when `None`.
pub(crate) fn mark(simple: &SimpleCommand, kind: Option<Tail>) {
    TAIL.with(|tail| tail.set(address(simple)));
    KIND.with(|cell| cell.set(kind));
}

/// Whether a trap keeps bash from replacing the process: EXIT or ERR, RETURN inside a function,
/// or a signal caught rather than ignored.
fn trapped(env: &Environment) -> bool {
    env.get_traps()
        .iter()
        .any(|(name, action)| match name.as_str() {
            _ if action == "-" => false,
            "EXIT" | "ERR" => true,
            "RETURN" => env.in_function(),
            "DEBUG" => false,
            _ => !action.is_empty(),
        })
}

/// Mark the command `item`, the last of a list of `kind`, ends with, when bash would not fork it:
/// a simple command with no `&`, `!` or `time`, no trap set, and a redirection only where `kind`
/// allows one. `alone` says the item is the list's only command.
pub(crate) fn arm(env: &Environment, item: &ListItem, kind: Tail, alone: bool) {
    let pipeline = item
        .and_or
        .rest
        .last()
        .map_or(&item.and_or.first, |(_, p)| p);
    let [Command::Simple(simple)] = pipeline.commands.as_slice() else {
        return;
    };
    let redirects = match kind {
        Tail::Program | Tail::Substitution | Tail::Async => false,
        Tail::Subshell => alone && item.and_or.rest.is_empty(),
        Tail::ProcSub => true,
    };
    if env.interactive()
        || item.op == ListOp::Background
        || pipeline.negated
        || pipeline.timed
        || trapped(env)
        || (!redirects && !simple.redirections.is_empty())
    {
        return;
    }
    mark(simple, Some(kind));
}

/// The list a function body ends its process with, when the call is the last command of a
/// substitution or a background job: a `{ … }` whose last command follows `;`, `&&` or `||`, or
/// is its only one. A background job's lone command waits for the last line of a `-c` program,
/// and its joined one does too there.
pub(super) fn function_tail(env: &Environment, body: &Command) -> Option<Tail> {
    let kind = KIND.with(Cell::get).filter(|_| current())?;
    let Command::Compound {
        kind: CompoundCommand::Group(list),
        ..
    } = body
    else {
        return None;
    };
    let joined = match list.items.as_slice() {
        [] => return None,
        [only] => !only.and_or.rest.is_empty(),
        [.., before, last] => {
            if before.op == ListOp::Background || !last.and_or.rest.is_empty() {
                return None;
            }
            true
        }
    };
    let program = env.option(crate::env::options::ShellOption::CommandString);
    let ready = match kind {
        Tail::Substitution | Tail::ProcSub => true,
        Tail::Async if program => LAST_LINE.with(Cell::get),
        Tail::Async => joined,
        Tail::Program | Tail::Subshell => false,
    };
    ready.then_some(kind)
}

thread_local! {
    /// The address of a `( … )` body to run in this process rather than in a child of it.
    static IN_PLACE: Cell<usize> = const { Cell::new(0) };
}

/// A `( … )` body that is a lone `( … )` itself runs that inner one without forking again.
pub(crate) fn mark_lone_subshell(body: &CommandList) {
    let [item] = body.items.as_slice() else {
        return;
    };
    let pipeline = &item.and_or.first;
    if let [
        Command::Compound {
            kind: CompoundCommand::Subshell(inner),
            ..
        },
    ] = pipeline.commands.as_slice()
        && item.op != ListOp::Background
        && item.and_or.rest.is_empty()
        && !pipeline.negated
        && !pipeline.timed
    {
        IN_PLACE.with(|in_place| in_place.set(std::ptr::from_ref(inner) as usize));
    }
}

/// Whether the `( … )` with this body is the one [`mark_lone_subshell`] chose.
pub(crate) fn runs_in_place(body: &CommandList) -> bool {
    let chosen = IN_PLACE.with(|in_place| in_place.replace(0));
    chosen != 0 && chosen == std::ptr::from_ref(body) as usize
}

/// A simple command starts: it is the last one if it is the one marked.
pub(super) fn begin(simple: &SimpleCommand) {
    let tail = TAIL.with(|tail| tail.get()) == address(simple);
    if tail {
        TAIL.with(|tail| tail.set(0));
    }
    CURRENT.with(|current| current.set(tail));
}

/// The simple command is done, or has called a function whose body is not the last command.
pub(super) fn end() {
    CURRENT.with(|current| current.set(false));
}

/// Whether the command running now is the one its process ends with.
pub(super) fn current() -> bool {
    CURRENT.with(Cell::get)
}

/// Set the running command aside while a trap runs commands of its own, and say whether it was
/// the last; [`resume`] puts it back.
pub(crate) fn suspend() -> bool {
    CURRENT.with(|current| current.replace(false))
}

/// The command [`suspend`] set aside is running again.
pub(crate) fn resume(last: bool) {
    CURRENT.with(|current| current.set(last));
}

/// Whether the command running now, being its process's last, lowers `$SHLVL` as it replaces it.
pub(super) fn lowers_level() -> bool {
    current() && KIND.with(Cell::get).is_some() && ENCLOSURE.with(Cell::get) != Enclosure::Pipe
}
