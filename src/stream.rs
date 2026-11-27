//! Running a file or a stream a command at a time, and the options that need it: `set -v` echoes
//! each line as it is read, and `set -t` ends the shell after one command.

use super::run_chunk;
use oslo::env::Environment;
use oslo::env::options::ShellOption;

/// Whether `script` has to be read a command at a time even though it parses: `-v` or `-t` is
/// on already, or the text may turn one of them on.
pub(super) fn needs_line_at_a_time(env: &Environment, script: &str) -> bool {
    let options = [
        (ShellOption::Verbose, 'v', "o verbose"),
        (ShellOption::OneCmd, 't', "o onecmd"),
    ];
    options.iter().any(|&(option, letter, name)| {
        env.option(option) || script.contains(name) || sets_letter(script, letter)
    })
}

/// Whether a `set -…` word in `script` carries `letter`; `unset -v` is not one.
fn sets_letter(script: &str, letter: char) -> bool {
    script.match_indices("set -").any(|(at, found)| {
        let word_start = script[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_');
        word_start
            && script[at + found.len()..]
                .chars()
                .take_while(char::is_ascii_alphabetic)
                .any(|c| c == letter)
    })
}

/// Read a command at a time, running each before looking at the next.
///
/// The buffer is emptied after every complete command, so each re-parse covers one command's worth
/// of text rather than the whole file.
pub(super) fn run_line_at_a_time(env: &mut Environment, script: &str) -> i32 {
    use oslo::ui::syntax::{InputStatus, classify};

    let mut buffer = String::new();
    let mut status = 0;
    // **Each chunk is parsed on its own, so its tree counts from 1.** Telling the environment how
    // much of the file came before it is what keeps `$LINENO` and every diagnostic's `line N`
    // talking about the file rather than about the piece.
    for (consumed, line) in (0_u32..).zip(script.split_inclusive('\n')) {
        // Counted before the chunk runs, and only up to where it *starts*: the offset is added to
        // a line number inside the chunk, and the chunk's own first line is its line 1.
        if buffer.is_empty() {
            env.set_line_offset(consumed);
        }
        // `set -v`: every line goes to stderr as it is read.
        if env.option(ShellOption::Verbose) {
            eprint!("{line}");
        }
        buffer.push_str(line);
        match classify(&buffer) {
            // The command has not ended yet — an open `if`, a heredoc still looking for its
            // delimiter, a trailing `|`.
            InputStatus::Incomplete => continue,
            // Not shell at all. Stop reading and let the parse below report it the usual way,
            // *after* everything already read has run — which is the whole point.
            InputStatus::Invalid => break,
            InputStatus::Complete => {
                // A blank line or a comment parses to nothing; there is no command to run and no
                // status to take from it.
                if !buffer.trim().is_empty() {
                    match run_chunk(env, &buffer, script) {
                        Ok(next) => status = next,
                        Err(failed) => return failed,
                    }
                    // `set -t`: the command just run was the one.
                    if env.option(ShellOption::OneCmd) {
                        return status;
                    }
                }
                buffer.clear();
            }
        }
    }

    // Whatever is left never became a complete command: an unterminated `if`, or the invalid text
    // the loop stopped at. Running it produces the same diagnostic a whole-file parse would.
    if !buffer.trim().is_empty() {
        match run_chunk(env, &buffer, script) {
            Ok(next) => status = next,
            Err(failed) => return failed,
        }
    }
    env.set_line_offset(0);
    status
}
