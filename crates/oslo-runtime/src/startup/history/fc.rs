//! `fc [-e ename] [-lnr] [first] [last]` and `fc -s [pat=rep] [command]` over the session history.
//!
//! The history is the one the `history` builtin reads, so a shell that is not interactive has an
//! empty one: `fc -l` lists nothing and `fc -s` finds no command, as in bash.

use oslo_base::error::Result;
use oslo_shell::Environment;

const USAGE: &str = "fc: usage: fc [-e ename] [-lnr] [first] [last] or fc -s [pat=rep] [command]";

#[derive(Default)]
struct Options {
    editor: Option<String>,
    list: bool,
    numbers: bool,
    reverse: bool,
    substitute: bool,
}

pub(super) fn builtin_fc(env: &mut Environment, args: &[String]) -> Result<i32> {
    let mut opts = Options {
        numbers: true,
        ..Options::default()
    };
    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            i += 1;
            break;
        }
        // `-5` is a history offset, not an option cluster.
        if arg.len() < 2
            || !arg.starts_with('-')
            || arg[1..].starts_with(|c: char| c.is_ascii_digit())
        {
            break;
        }
        let mut letters = arg[1..].chars();
        while let Some(c) = letters.next() {
            match c {
                'l' => opts.list = true,
                'n' => opts.numbers = false,
                'r' => opts.reverse = true,
                's' => opts.substitute = true,
                'e' => {
                    let rest: String = letters.by_ref().collect();
                    opts.editor = match rest.is_empty() {
                        true => {
                            i += 1;
                            args.get(i).cloned()
                        }
                        false => Some(rest),
                    };
                    if opts.editor.is_none() {
                        let body = "fc: -e: option requires an argument";
                        oslo_shell::env::complain_option(args, 'e', body, USAGE);
                        return Ok(2);
                    }
                }
                other => {
                    let body = format!("fc: -{other}: invalid option");
                    oslo_shell::env::complain_option(args, other, &body, USAGE);
                    return Ok(2);
                }
            }
        }
        i += 1;
    }
    let operands = &args[i.min(args.len())..];
    let history = super::entries();
    if opts.editor.as_deref() == Some("-") {
        opts.substitute = true;
    }

    if opts.list {
        return Ok(list(&history, operands, &opts));
    }
    if opts.substitute {
        return substitute(env, &history, operands);
    }
    edit(env, &history, operands, opts.editor)
}

/// The 0-based index `spec` names: a number, a negative offset, or the newest command it prefixes.
fn locate(history: &[String], spec: &str) -> Option<usize> {
    if let Ok(n) = spec.parse::<i64>() {
        let len = history.len() as i64;
        let index = match n {
            n if n < 0 => len + n,
            0 => len - 1,
            n => n - 1,
        };
        return (0..len).contains(&index).then_some(index as usize);
    }
    history.iter().rposition(|line| line.starts_with(spec))
}

fn list(history: &[String], operands: &[String], opts: &Options) -> i32 {
    if history.is_empty() {
        return 0;
    }
    let first = operands.first().map_or("-16", String::as_str);
    let last = operands.get(1).map_or(
        match operands.len() {
            0 => "-1",
            _ => first,
        },
        String::as_str,
    );
    let clamp = |spec: &str| match spec.parse::<i64>() {
        Ok(n) if n < 0 => Some((history.len() as i64 + n).max(0) as usize),
        _ => locate(history, spec),
    };
    let (Some(from), Some(to)) = (clamp(first), clamp(last)) else {
        complain("history specification out of range");
        return 1;
    };
    let (low, high) = (from.min(to), from.max(to));
    let mut picked: Vec<usize> = (low..=high).collect();
    if opts.reverse != (from > to) {
        picked.reverse();
    }
    for index in picked {
        match opts.numbers {
            true => println!("{}\t {}", index + 1, history[index]),
            false => println!("\t {}", history[index]),
        }
    }
    0
}

/// `fc -s [pat=rep] [command]`: run a command again, with the first `pat` replaced.
fn substitute(env: &mut Environment, history: &[String], operands: &[String]) -> Result<i32> {
    let (edits, rest): (Vec<&String>, Vec<&String>) =
        operands.iter().partition(|word| word.contains('='));
    let index = match rest.first() {
        Some(spec) => locate(history, spec),
        None => history.len().checked_sub(1),
    };
    let Some(index) = index else {
        complain("no command found");
        return Ok(1);
    };
    let mut command = history[index].clone();
    for edit in edits {
        if let Some((pat, rep)) = edit.split_once('=') {
            command = command.replacen(pat, rep, 1);
        }
    }
    println!("{command}");
    run(env, &command)
}

/// Edit the chosen commands in `$FCEDIT`, `$EDITOR` or `vi`, then run what was saved.
fn edit(
    env: &mut Environment,
    history: &[String],
    operands: &[String],
    editor: Option<String>,
) -> Result<i32> {
    if history.is_empty() {
        return Ok(0);
    }
    let first = operands.first().and_then(|spec| locate(history, spec));
    let first = match operands.is_empty() {
        true => history.len().checked_sub(1),
        false => first,
    };
    let last = match operands.get(1) {
        Some(spec) => locate(history, spec),
        None => first,
    };
    let (Some(first), Some(last)) = (first, last) else {
        complain("no command found");
        return Ok(1);
    };
    let text: String = history[first.min(last)..=first.max(last)]
        .iter()
        .map(|line| format!("{line}\n"))
        .collect();
    let file = std::env::temp_dir().join(format!("oslo-fc-{}", std::process::id()));
    if let Err(e) = std::fs::write(&file, text) {
        complain(&format!("{}: {e}", file.display()));
        return Ok(1);
    }
    let editor = editor
        .or_else(|| env.get_var("FCEDIT").map(str::to_string))
        .or_else(|| env.get_var("EDITOR").map(str::to_string))
        .unwrap_or_else(|| "vi".to_string());
    let status = run(env, &format!("{editor} {}", file.display()))?;
    let edited = std::fs::read_to_string(&file).unwrap_or_default();
    let _ = std::fs::remove_file(&file);
    if status != 0 {
        return Ok(status);
    }
    print!("{edited}");
    run(env, &edited)
}

fn complain(message: &str) {
    eprintln!("{}fc: {message}", oslo_shell::env::origin_now());
}

fn run(env: &mut Environment, command: &str) -> Result<i32> {
    let args = ["eval".to_string(), command.to_string()];
    oslo_shell::env::builtins::builtin_eval(env, &args)
}
