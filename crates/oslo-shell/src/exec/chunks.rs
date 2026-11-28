//! Program text read a complete command at a time, as bash reads a file, a `-c` string, an `eval`
//! string and a sourced file: each command runs before the next one is parsed, so the lines before
//! a syntax error run and the error ends the reading there.

use crate::env::Environment;
use crate::env::options::ShellOption;
use oslo_base::error::{Result, ShellError};
use oslo_ui::syntax::{InputStatus, classify};

/// One complete command's lines, or the lines the reading stopped at.
pub struct Chunk<'a> {
    /// The lines, each with its newline.
    pub text: &'a str,
    /// How many lines of the whole text come before it.
    pub offset: u32,
}

impl Chunk<'_> {
    /// A blank line or a comment, which parses to nothing and is not run.
    pub fn is_blank(&self) -> bool {
        self.text.trim().is_empty()
    }

    /// `set -v`: the lines go to stderr as they are read, each ending in a newline.
    pub fn echo(&self) {
        match self.text.ends_with('\n') {
            true => eprint!("{}", self.text),
            false => eprintln!("{}", self.text),
        }
    }
}

/// The complete commands of `text` in order, ending with the first one that cannot be shell.
pub fn chunks(text: &str) -> Chunks<'_> {
    Chunks {
        text,
        pos: 0,
        line: 0,
        done: false,
    }
}

/// The iterator [`chunks`] answers.
pub struct Chunks<'a> {
    text: &'a str,
    pos: usize,
    line: u32,
    done: bool,
}

impl<'a> Iterator for Chunks<'a> {
    type Item = Chunk<'a>;

    fn next(&mut self) -> Option<Chunk<'a>> {
        if self.done || self.pos >= self.text.len() {
            return None;
        }
        let (start, offset) = (self.pos, self.line);
        for line in self.text[start..].split_inclusive('\n') {
            self.pos += line.len();
            self.line += 1;
            match classify(&self.text[start..self.pos]) {
                InputStatus::Incomplete => continue,
                InputStatus::Invalid => {
                    self.done = true;
                    break;
                }
                InputStatus::Complete => break,
            }
        }
        Some(Chunk {
            text: &self.text[start..self.pos],
            offset,
        })
    }
}

/// Whether `text` has to be read a command at a time even though it parses: `-v` or `-t` is on
/// already, or the text may turn one of them on.
pub fn needs_chunks(env: &Environment, text: &str) -> bool {
    let options = [
        (ShellOption::Verbose, 'v', "o verbose"),
        (ShellOption::OneCmd, 't', "o onecmd"),
    ];
    options.iter().any(|&(option, letter, name)| {
        env.option(option) || text.contains(name) || sets_letter(text, letter)
    })
}

/// Whether a `set -…` word in `text` carries `letter`; `unset -v` is not one.
fn sets_letter(text: &str, letter: char) -> bool {
    text.match_indices("set -").any(|(at, found)| {
        let word_start = text[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_');
        word_start
            && text[at + found.len()..]
                .chars()
                .take_while(char::is_ascii_alphabetic)
                .any(|c| c == letter)
    })
}

/// Run the text of an `eval` or a sourced file a command at a time, each chunk as its own unit.
///
/// `report` prints a chunk's syntax error. The reading ends there with status 2, or with a utility
/// error when POSIX mode makes it fatal: bash ends the shell only when nothing in the text ran.
pub fn run_unit_chunks(
    env: &mut Environment,
    name: &str,
    text: &str,
    report: &mut dyn FnMut(&Environment, &ShellError),
) -> Result<i32> {
    let base = env.line_offset();
    let mut status = 0;
    let mut ran = false;
    for chunk in chunks(text) {
        if env.option(ShellOption::Verbose) {
            chunk.echo();
        }
        if chunk.is_blank() {
            continue;
        }
        env.set_line_offset(base + chunk.offset);
        let parsed =
            crate::syntax::parse_with_aliases(chunk.text, !env.get_aliases().is_empty(), &|n| {
                env.get_alias(n).map(str::to_string)
            });
        let ast = match parsed {
            Ok(ast) => ast,
            Err(e) => {
                report(env, &e);
                env.set_line_offset(base);
                return match ran {
                    true => Ok(2),
                    false => Err(ShellError::utility_error(name, 2)),
                };
            }
        };
        ran = true;
        let result = crate::exec::eval_command_unit(env, &ast);
        env.set_line_offset(base);
        status = result?;
    }
    Ok(status)
}
