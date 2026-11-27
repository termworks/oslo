//! `bind` — readline key bindings, kept as a table for the scripts and rc files that set them.
//!
//! oslo's line editor is not readline and reads none of these: bindings are stored, printed and
//! removed as bash would, and a shell that is not interactive says first that line editing is off,
//! as bash does. Only bindings made in this shell are listed; readline's defaults are not.

use crate::env::origin_now;
use crate::env::scope::Environment;
use oslo_base::error::Result;
use std::cell::RefCell;

const USAGE: &str = "bind: usage: bind [-lpsvPSVX] [-m keymap] [-f filename] [-q name] [-u name] \
     [-r keyseq] [-x keyseq:shell-command] [keyseq:readline-function or readline-command]";

/// readline's function names, as `bind -l` lists them.
const FUNCTIONS: &str = include_str!("bind_functions.txt");

thread_local! {
    /// `(keyseq, function)`, in the order they were bound.
    static KEYS: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
    /// `bind -x`: `(keyseq, shell command)`.
    static COMMANDS: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
}

pub fn builtin_bind(_env: &mut Environment, args: &[String]) -> Result<i32> {
    if !crate::exec::pipeline::is_interactive() {
        eprintln!("{}bind: warning: line editing not enabled", origin_now());
    }
    let mut status = 0;
    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            i += 1;
            break;
        }
        if arg.len() < 2 || !arg.starts_with('-') {
            break;
        }
        let mut letters = arg[1..].chars();
        while let Some(c) = letters.next() {
            if "mfqurx".contains(c) {
                let rest: String = letters.by_ref().collect();
                let value = match rest.is_empty() {
                    true => {
                        i += 1;
                        args.get(i).cloned()
                    }
                    false => Some(rest),
                };
                let Some(value) = value else {
                    eprintln!("{}bind: -{c}: option requires an argument", origin_now());
                    eprintln!("{USAGE}");
                    return Ok(2);
                };
                status = status.max(with_value(c, &value));
                break;
            }
            match c {
                'l' => FUNCTIONS.lines().for_each(|name| println!("{name}")),
                'p' | 'P' => KEYS.with(|keys| {
                    for (seq, function) in keys.borrow().iter() {
                        println!("\"{seq}\": {function}");
                    }
                }),
                'X' => COMMANDS.with(|commands| {
                    for (seq, command) in commands.borrow().iter() {
                        println!("\"{seq}\" \"{command}\"");
                    }
                }),
                'v' | 'V' | 's' | 'S' => {}
                other => {
                    eprintln!("{}bind: -{other}: invalid option", origin_now());
                    eprintln!("{USAGE}");
                    return Ok(2);
                }
            }
        }
        i += 1;
    }
    for line in &args[i.min(args.len())..] {
        status = status.max(bind_line(line));
    }
    Ok(status)
}

/// The options that take a value: `-m`, `-f`, `-q`, `-u`, `-r` and `-x`.
fn with_value(c: char, value: &str) -> i32 {
    match c {
        'm' => 0,
        'f' => match std::fs::read_to_string(value) {
            Ok(text) => text
                .lines()
                .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
                .map(bind_line)
                .max()
                .unwrap_or(0),
            Err(e) => {
                eprintln!("{}bind: {value}: cannot read: {e}", origin_now());
                1
            }
        },
        'q' => {
            if !known(value) {
                eprintln!("{}bind: `{value}': unknown function name", origin_now());
                return 1;
            }
            let seqs: Vec<String> = KEYS.with(|keys| {
                keys.borrow()
                    .iter()
                    .filter(|(_, function)| function == value)
                    .map(|(seq, _)| format!("\"{seq}\""))
                    .collect()
            });
            match seqs.is_empty() {
                true => println!("{value} is not bound to any keys."),
                false => println!("{value} can be invoked via {}.", seqs.join(", ")),
            }
            0
        }
        'u' => {
            if !known(value) {
                eprintln!("{}bind: `{value}': unknown function name", origin_now());
                return 1;
            }
            KEYS.with(|keys| keys.borrow_mut().retain(|(_, function)| function != value));
            0
        }
        'r' => {
            let seq = unquote(value);
            KEYS.with(|keys| keys.borrow_mut().retain(|(s, _)| *s != seq));
            COMMANDS.with(|commands| commands.borrow_mut().retain(|(s, _)| *s != seq));
            0
        }
        _ => match split_binding(value) {
            Some((seq, command)) => {
                COMMANDS.with(|commands| {
                    let mut commands = commands.borrow_mut();
                    commands.retain(|(s, _)| *s != seq);
                    commands.push((seq, unquote(&command)));
                });
                0
            }
            None => {
                eprintln!("{}bind: {value}: missing colon separator", origin_now());
                1
            }
        },
    }
}

fn known(function: &str) -> bool {
    FUNCTIONS.lines().any(|name| name == function)
}

/// `"keyseq": function`, or a `set variable value` line, which is accepted and has no effect.
fn bind_line(line: &str) -> i32 {
    if line.trim_start().starts_with("set ") || line.trim_start().starts_with('$') {
        return 0;
    }
    match split_binding(line) {
        Some((seq, function)) => {
            KEYS.with(|keys| {
                let mut keys = keys.borrow_mut();
                keys.retain(|(s, _)| *s != seq);
                keys.push((seq, function));
            });
            0
        }
        None => 0,
    }
}

/// Split `"keyseq": rest` at the colon after the key sequence.
fn split_binding(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    let (seq, rest) = match line.strip_prefix('"') {
        Some(quoted) => {
            let end = quoted.find('"')?;
            (
                &quoted[..end],
                quoted[end + 1..].trim_start().strip_prefix(':')?,
            )
        }
        None => line.split_once(':')?,
    };
    Some((seq.to_string(), rest.trim().to_string()))
}

fn unquote(text: &str) -> String {
    let text = text.trim();
    text.strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .unwrap_or(text)
        .to_string()
}
