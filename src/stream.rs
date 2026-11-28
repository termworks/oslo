//! Running a program a command at a time, and the options that need it: `set -v` echoes each
//! command's lines as they are read, and `set -t` ends the shell after one command.

use super::program::run_chunk;
use oslo::env::Environment;
use oslo::env::options::ShellOption;
use oslo::exec::chunks::chunks;

/// Read a command at a time, running each before looking at the next.
///
/// Each chunk is parsed on its own, so its tree counts from 1; the line offset is what keeps
/// `$LINENO` and every diagnostic talking about the whole program rather than the piece.
pub(super) fn run_line_at_a_time(env: &mut Environment, script: &str) -> i32 {
    let mut status = 0;
    for chunk in chunks(script) {
        env.set_line_offset(chunk.offset);
        if env.option(ShellOption::Verbose) {
            chunk.echo();
        }
        if chunk.is_blank() {
            continue;
        }
        match run_chunk(env, chunk.text, script) {
            Ok(next) => status = next,
            Err(failed) => return failed,
        }
        // `set -t`: the command just run was the one. bash ignores it in a `-c` string.
        if env.option(ShellOption::OneCmd) && !env.option(ShellOption::CommandString) {
            return status;
        }
    }
    env.set_line_offset(0);
    status
}
