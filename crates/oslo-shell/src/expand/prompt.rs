//! Prompt expansion, for `${x@P}`: the backslash escapes of `PS1`, then the expansions a prompt
//! string undergoes.
//!
//! What an escape produces is protected from the expansion that follows, so a directory called
//! `$(rm -rf ~)` shown by `\w` stays text. `\[` and `\]` mark non-printing runs and vanish.

use crate::env::Environment;
use oslo_base::error::Result;
use std::sync::atomic::{AtomicU32, Ordering};

/// How many top-level commands the shell has read from a script or standard input, which `\#`
/// shows: a `-c` string, an `eval` and a sourced file read none.
static COMMANDS_READ: AtomicU32 = AtomicU32::new(0);

/// Count one more top-level command read.
pub(crate) fn count_command_read() {
    COMMANDS_READ.fetch_add(1, Ordering::Relaxed);
}

/// Expand `text` as bash expands a prompt string for `${x@P}`.
pub fn expand(env: &mut Environment, text: &str) -> Result<String> {
    expand_counting(env, text, 1)
}

/// Expand `PS4`, whose `\#` does not count the command it traces.
pub fn expand_ps4(env: &mut Environment, text: &str) -> Result<String> {
    expand_counting(env, text, 0)
}

/// `beyond` is what `\#` adds to the commands read so far.
fn expand_counting(env: &mut Environment, text: &str, beyond: u32) -> Result<String> {
    let decoded = decode(env, text, COMMANDS_READ.load(Ordering::Relaxed) + beyond);
    let body = crate::lexer::parse_heredoc_body(&decoded)?;
    crate::expand::word::expand_word_to_string(env, &body)
}

/// Replace the escapes, quoting what they produce for the expansion after.
fn decode(env: &Environment, raw: &str, number: u32) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        let Some(esc) = chars.next() else {
            out.push('\\');
            break;
        };
        match esc {
            'a' => out.push('\u{7}'),
            'e' => out.push('\u{1b}'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            '\\' => out.push_str("\\\\"),
            '$' if nix::unistd::geteuid().is_root() => out.push('#'),
            '$' => out.push_str("\\$"),
            '[' | ']' => {}
            'u' => protect(&mut out, &user_name(env)),
            'h' => protect(&mut out, host_name().split('.').next().unwrap_or_default()),
            'H' => protect(&mut out, &host_name()),
            'w' => protect(&mut out, &tilde_pwd(env)),
            'W' => {
                let pwd = tilde_pwd(env);
                let base = match pwd.as_str() {
                    "/" | "~" => pwd.as_str(),
                    _ => pwd.rsplit('/').next().unwrap_or(&pwd),
                };
                protect(&mut out, base);
            }
            's' => {
                let name = std::env::args_os()
                    .next()
                    .map(|name| oslo_base::lossless::encode(&name))
                    .unwrap_or_default();
                protect(&mut out, name.rsplit('/').next().unwrap_or(&name));
            }
            'v' | 'V' => protect(&mut out, &version(env, esc == 'V')),
            't' => protect(&mut out, &oslo_base::clock::local("%H:%M:%S")),
            'T' => protect(&mut out, &oslo_base::clock::local("%I:%M:%S")),
            '@' => protect(&mut out, &oslo_base::clock::local("%I:%M %p")),
            'A' => protect(&mut out, &oslo_base::clock::local("%H:%M")),
            'd' => protect(&mut out, &oslo_base::clock::local("%a %b %d")),
            'D' if chars.peek() == Some(&'{') => {
                chars.next();
                let format: String = chars.by_ref().take_while(|&c| c != '}').collect();
                let format = if format.is_empty() { "%X" } else { &format };
                protect(&mut out, &oslo_base::clock::local(format));
            }
            '#' => out.push_str(&number.to_string()),
            '!' => out.push('1'),
            'j' => out.push('0'),
            d @ '0'..='7' => {
                let mut value = d.to_digit(8).unwrap_or(0);
                for _ in 0..2 {
                    match chars.peek().and_then(|c| c.to_digit(8)) {
                        Some(digit) => {
                            value = value * 8 + digit;
                            chars.next();
                        }
                        None => break,
                    }
                }
                out.extend(char::from_u32(value & 0xff));
            }
            other => {
                out.push('\\');
                out.push(other);
            }
        }
    }
    out
}

/// Append `value` with the characters the later expansion would act on escaped.
fn protect(out: &mut String, value: &str) {
    for ch in value.chars() {
        if matches!(ch, '$' | '`' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
}

fn user_name(env: &Environment) -> String {
    nix::unistd::User::from_uid(nix::unistd::geteuid())
        .ok()
        .flatten()
        .map(|user| user.name)
        .or_else(|| env.get_var("USER").map(str::to_string))
        .unwrap_or_default()
}

fn host_name() -> String {
    nix::unistd::gethostname()
        .ok()
        .and_then(|name| name.into_string().ok())
        .unwrap_or_default()
}

/// `$PWD` with `$HOME` written as `~`.
fn tilde_pwd(env: &Environment) -> String {
    let pwd = env.get_var("PWD").map(str::to_string).unwrap_or_else(|| {
        std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    match env
        .get_var("HOME")
        .filter(|home| !home.is_empty() && *home != "/")
    {
        Some(home) if pwd == home => "~".to_string(),
        Some(home) if pwd.starts_with(&format!("{home}/")) => format!("~{}", &pwd[home.len()..]),
        _ => pwd,
    }
}

/// `\v` is the release (`5.3`), `\V` adds the patch level (`5.3.9`).
fn version(env: &Environment, patch: bool) -> String {
    let full = env.get_var("BASH_VERSION").unwrap_or_default();
    let release = full.split('(').next().unwrap_or_default();
    match patch {
        true => release.to_string(),
        false => release.splitn(3, '.').take(2).collect::<Vec<_>>().join("."),
    }
}
