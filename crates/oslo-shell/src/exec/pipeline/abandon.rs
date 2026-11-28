//! What an error that abandons the top-level command leaves, and where it stops.
//!
//! bash catches these at two kinds of top level. Its reading loop, a script's or a prompt's, and a
//! forked compound command keep the status the error set. `parse_and_execute`, which runs a `-c`
//! program, an `eval`, a `source` and a command substitution, reports 1 instead, and in a child
//! process it takes the rest of that child with it.

use crate::env::Environment;
use crate::env::options::ShellOption;
use oslo_base::error::{Result, ShellError};

/// Whether `err` abandons only the top-level command it happened in, rather than the shell: a
/// refused assignment, a `failglob` miss, and an expansion error other than the ones a
/// non-interactive POSIX-mode shell treats as fatal.
pub(super) fn abandons_line(env: &Environment, err: &ShellError) -> bool {
    match err {
        ShellError::NoMatch(_)
        | ShellError::Abandoned
        | ShellError::ExpansionError(_)
        | ShellError::AssignmentRefused(_) => true,
        ShellError::MalformedExpansion(_) => !env.posix() || env.interactive(),
        _ => false,
    }
}

/// The status a list that caught `err` reports: what the error set at the top of a script or a
/// prompt (`top`), 1 at the top of a `-c` program and of an `eval` or `source`.
pub(super) fn line_status(env: &Environment, err: &ShellError, top: bool) -> i32 {
    match err {
        ShellError::AssignmentRefused(_) if top && !env.option(ShellOption::CommandString) => 2,
        _ => 1,
    }
}

/// Whether this process is a child of the shell: a subshell, a substitution, a stage or a job.
pub(super) fn in_child(env: &Environment) -> bool {
    env.get_param("$")
        .is_some_and(|shell| shell != env.current_pid().to_string())
}

/// `result` of a forked stage or job, `simple` when it is one simple command: that runs where its
/// parent would have, so in a `-c` program a refused assignment reports 1 there too.
pub(super) fn forked_simple(env: &Environment, simple: bool, result: Result<i32>) -> Result<i32> {
    match simple && env.option(ShellOption::CommandString) {
        true => as_string_program(result),
        false => result,
    }
}

/// `result` as `parse_and_execute` leaves it in a child: a refused assignment reports 1.
pub(crate) fn as_string_program(result: Result<i32>) -> Result<i32> {
    match result {
        Err(err @ ShellError::AssignmentRefused(_)) => {
            eprintln!("oslo: {err}");
            Err(ShellError::Abandoned)
        }
        other => other,
    }
}
