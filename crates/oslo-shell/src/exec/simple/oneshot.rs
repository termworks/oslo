//! bash's last-command rule for `-c`: the final simple command of the program is not forked, so a
//! `command_not_found_handle` it reaches runs in the shell itself rather than in a child.

use crate::env::Environment;
use crate::env::options::ShellOption;
use oslo_base::ast::{Command, ListItem, ListOp};
use std::cell::Cell;

thread_local! {
    /// The next simple command evaluated is the program's last.
    static ARMED: Cell<bool> = const { Cell::new(false) };
    /// The simple command running now is the program's last, and is not a function call.
    static CURRENT: Cell<bool> = const { Cell::new(false) };
}

/// Note whether `item`, about to run, is the one command bash would not fork: the last of a `-c`
/// program, a lone simple command with no redirection, `!` or `time`, and no trap set.
pub(crate) fn arm(env: &Environment, item: &ListItem, last: bool, outermost: bool) {
    let pipeline = &item.and_or.first;
    let plain = matches!(pipeline.commands.as_slice(), [Command::Simple(simple)]
        if simple.redirections.is_empty());
    let trapped = env.get_traps().values().any(|action| action != "-");
    let armed = last
        && outermost
        && env.option(ShellOption::CommandString)
        && !env.interactive()
        && item.op != ListOp::Background
        && item.and_or.rest.is_empty()
        && !pipeline.negated
        && !pipeline.timed
        && plain
        && !trapped;
    ARMED.with(|a| a.set(armed));
}

/// A simple command starts: it is the last one if the list armed this.
pub(super) fn begin() {
    CURRENT.with(|c| c.set(ARMED.with(|a| a.replace(false))));
}

/// The simple command is done, or has called a function whose body is not the last command.
pub(super) fn end() {
    CURRENT.with(|c| c.set(false));
}

/// Whether the command running now is the program's last.
pub(super) fn current() -> bool {
    CURRENT.with(Cell::get)
}
