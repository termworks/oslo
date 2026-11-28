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
    /// Whether the marked command stands in for the whole process, so `$SHLVL` drops by one as
    /// `exec` drops it: the end of a list, not a pipeline stage or a background job.
    static LOWERS: Cell<bool> = const { Cell::new(false) };
}

/// Which list a process ends with, which decides whether its last command may redirect.
#[derive(Clone, Copy)]
pub(crate) enum Tail {
    /// A `-c` program or a command substitution: the command may not redirect.
    Program,
    /// A `( … )` body, whose command may redirect when it is the only one.
    Subshell,
    /// A process substitution, whose last command may redirect.
    ProcSub,
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

/// The process ends with `simple`: a pipeline stage or a background job.
pub(crate) fn mark(simple: &SimpleCommand) {
    TAIL.with(|tail| tail.set(address(simple)));
    LOWERS.with(|lowers| lowers.set(false));
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
        Tail::Program => false,
        Tail::Subshell => alone && item.and_or.rest.is_empty(),
        Tail::ProcSub => true,
    };
    let trapped = env.get_traps().values().any(|action| action != "-");
    if env.interactive()
        || item.op == ListOp::Background
        || pipeline.negated
        || pipeline.timed
        || trapped
        || (!redirects && !simple.redirections.is_empty())
    {
        return;
    }
    mark(simple);
    LOWERS.with(|lowers| lowers.set(true));
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

/// Whether the command running now, being its process's last, lowers `$SHLVL` as it replaces it.
pub(super) fn lowers_level() -> bool {
    current() && LOWERS.with(Cell::get)
}
