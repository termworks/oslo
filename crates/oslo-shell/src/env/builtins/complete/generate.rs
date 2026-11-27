//! Turning a completion spec and a word into the list of completions, in bash's order: the
//! actions, `-G`, `-W`, `-F`, `-C`, then the `-X` filter, the `-P`/`-S` affixes, and the
//! `dirnames`/`plusdirs` options.

use super::{ACTIONS, Spec, single_quote};
use crate::env::scope::Environment;
use oslo_base::error::Result;

/// bash's reserved words, in the order `compgen -k` lists them.
const KEYWORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "case", "esac", "for", "select", "while", "until", "do",
    "done", "in", "function", "time", "{", "}", "!", "[[", "]]", "coproc",
];

/// Every completion of `word` the spec produces. `command` is the word being completed for, and
/// `prev` the word before `word`, both handed to `-F` and `-C`.
pub(in crate::env::builtins) fn completions(
    env: &mut Environment,
    spec: &Spec,
    word: &str,
    command: &str,
    prev: &str,
) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for (name, _) in ACTIONS {
        if spec.has(name) {
            out.extend(action(env, name, word));
        }
    }
    if let Some(pattern) = &spec.glob {
        out.extend(glob(pattern));
    }
    if let Some(list) = &spec.words {
        out.extend(
            word_list(env, list)?
                .into_iter()
                .filter(|w| w.starts_with(word)),
        );
    }
    if let Some(function) = &spec.function {
        out.extend(call_function(env, function, command, word, prev)?);
    }
    if let Some(cmd) = &spec.command {
        let line = format!(
            "{cmd} {} {} {}",
            single_quote(command),
            single_quote(word),
            single_quote(prev)
        );
        let output = crate::exec::eval_command_substitution(env, &line)?;
        out.extend(output.lines().map(str::to_string));
    }
    if let Some(filter) = &spec.filter {
        out = filtered(out, filter, word);
    }
    if spec.prefix.is_some() || spec.suffix.is_some() {
        let (pre, suf) = (
            spec.prefix.clone().unwrap_or_default(),
            spec.suffix.clone().unwrap_or_default(),
        );
        out = out.into_iter().map(|w| format!("{pre}{w}{suf}")).collect();
    }
    if out.is_empty() && spec.option("dirnames") {
        out = paths(word, true);
    } else if spec.option("plusdirs") {
        out.extend(paths(word, true));
    }
    Ok(out)
}

/// The names one action offers for `word`.
fn action(env: &mut Environment, name: &str, word: &str) -> Vec<String> {
    let starts = |names: Vec<String>| -> Vec<String> {
        names.into_iter().filter(|n| n.starts_with(word)).collect()
    };
    match name {
        "alias" => starts(sorted(env.get_aliases().keys().cloned())),
        "arrayvar" => starts(sorted(env.array_names().map(str::to_string))),
        "builtin" => starts(builtins(env, None)),
        "enabled" => starts(builtins(env, Some(true))),
        "disabled" => starts(builtins(env, Some(false))),
        "command" => commands(env, word),
        "directory" => paths(word, true),
        "file" => paths(word, false),
        "export" => starts(env.exported_vars().into_iter().map(|(n, _)| n).collect()),
        "function" => starts(sorted(env.get_functions().keys().cloned())),
        "group" => starts(first_fields("/etc/group", ':')),
        "user" => starts(first_fields("/etc/passwd", ':')),
        "service" => starts(first_fields("/etc/services", ' ')),
        "hostname" => starts(hostnames()),
        "helptopic" => starts(super::super::help::topics().map(str::to_string).collect()),
        "keyword" => starts(KEYWORDS.iter().map(|k| k.to_string()).collect()),
        "job" => starts(jobs(None)),
        "running" => starts(jobs(Some(false))),
        "stopped" => starts(jobs(Some(true))),
        "setopt" => starts(sorted(
            crate::env::options::ALL
                .iter()
                .filter_map(|o| o.name.map(str::to_string)),
        )),
        "shopt" => starts(sorted(
            super::super::shopt::option_names().map(str::to_string),
        )),
        "signal" => starts(signals()),
        "variable" => starts(variables(env)),
        _ => Vec::new(),
    }
}

fn sorted(names: impl Iterator<Item = String>) -> Vec<String> {
    let mut names: Vec<String> = names.collect();
    names.sort();
    names.dedup();
    names
}

fn builtins(env: &Environment, enabled: Option<bool>) -> Vec<String> {
    super::super::enable::builtins(env)
        .into_iter()
        .filter(|(_, on)| enabled.is_none_or(|want| *on == want))
        .map(|(name, _)| name)
        .collect()
}

fn variables(env: &Environment) -> Vec<String> {
    let scalars = env.all_vars().into_iter().map(|(name, _, _)| name);
    let arrays = env.array_names().map(str::to_string);
    sorted(scalars.chain(arrays))
}

/// Aliases, reserved words, functions, enabled builtins, then every executable on `$PATH`.
fn commands(env: &Environment, word: &str) -> Vec<String> {
    if word.contains('/') {
        return paths(word, false);
    }
    let starts = |n: &String| n.starts_with(word);
    let mut out: Vec<String> = sorted(env.get_aliases().keys().cloned());
    out.extend(KEYWORDS.iter().map(|k| k.to_string()));
    out.extend(sorted(env.get_functions().keys().cloned()));
    out.extend(builtins(env, Some(true)));
    out.retain(starts);
    let path = env.get_var("PATH").unwrap_or_default().to_string();
    for dir in path.split(':') {
        let dir = if dir.is_empty() { "." } else { dir };
        let mut found: Vec<String> = entries(dir)
            .into_iter()
            .filter(|name| name.starts_with(word))
            .filter(|name| is_executable(&std::path::Path::new(dir).join(name)))
            .collect();
        found.sort();
        out.extend(found);
    }
    out
}

fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The names in `dir`, without `.` and `..`.
fn entries(dir: &str) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|it| {
            it.flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// The paths that begin with `word`, dotfiles included, directories only when `dirs_only`.
pub(in crate::env::builtins) fn paths(word: &str, dirs_only: bool) -> Vec<String> {
    let (dir, base) = match word.rfind('/') {
        Some(at) => (&word[..=at], &word[at + 1..]),
        None => ("", word),
    };
    let expanded = match dir.strip_prefix("~/") {
        Some(rest) => format!("{}/{rest}", std::env::var("HOME").unwrap_or_default()),
        None => dir.to_string(),
    };
    let lookup = if expanded.is_empty() {
        "."
    } else {
        expanded.as_str()
    };
    let mut names: Vec<String> = entries(lookup)
        .into_iter()
        .filter(|name| name.starts_with(base))
        .filter(|name| !dirs_only || std::path::Path::new(lookup).join(name).is_dir())
        .map(|name| format!("{dir}{name}"))
        .collect();
    names.sort();
    names
}

fn glob(pattern: &str) -> Vec<String> {
    use oslo_base::glob::walk;
    let chars: Vec<(char, bool)> = pattern.chars().map(|c| (c, true)).collect();
    match walk::expand(&chars, &walk::shell_options()) {
        Some(matched) => matched,
        None if std::fs::symlink_metadata(pattern).is_ok() => vec![pattern.to_string()],
        None => Vec::new(),
    }
}

/// `-W`'s list: split on `$IFS` outside quotes, then each piece expanded as one word.
fn word_list(env: &mut Environment, list: &str) -> Result<Vec<String>> {
    let ifs = env.get_var("IFS").unwrap_or(" \t\n").to_string();
    let mut pieces = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut chars = list.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (None, '\\') => {
                current.push(c);
                current.extend(chars.next());
            }
            (None, '\'' | '"') => {
                quote = Some(c);
                current.push(c);
            }
            (Some(q), c) if c == q => {
                quote = None;
                current.push(c);
            }
            (None, c) if ifs.contains(c) => {
                if !current.is_empty() {
                    pieces.push(std::mem::take(&mut current));
                }
            }
            // A blank `$IFS` does not split on stays part of the word.
            (None, ' ' | '\t' | '\n') => {
                current.push('\\');
                current.push(c);
            }
            _ => current.push(c),
        }
    }
    if !current.is_empty() {
        pieces.push(current);
    }
    let mut words = Vec::new();
    for piece in pieces {
        let parsed = crate::syntax::lower::words::single_word_from_str(&piece)?;
        words.push(crate::expand::expand_word_to_string(env, &parsed)?);
    }
    Ok(words)
}

/// `-F function`: run it with the command, the word and the previous word; `COMPREPLY` is the answer.
fn call_function(
    env: &mut Environment,
    function: &str,
    command: &str,
    word: &str,
    prev: &str,
) -> Result<Vec<String>> {
    env.unset_var("COMPREPLY");
    let line = format!(
        "{function} {} {} {}",
        single_quote(command),
        single_quote(word),
        single_quote(prev)
    );
    let args = ["eval".to_string(), line];
    crate::env::builtins::builtin_eval(env, &args)?;
    let reply = match env.get_array("COMPREPLY") {
        Some(array) => array.values().map(str::to_string).collect(),
        None => env
            .get_var("COMPREPLY")
            .map(|v| vec![v.to_string()])
            .unwrap_or_default(),
    };
    Ok(reply)
}

/// Drop the words `pattern` matches, or keep only those when it starts with `!`.
///
/// `&` in the pattern stands for the word being completed, `\&` for a literal `&`.
fn filtered(words: Vec<String>, pattern: &str, word: &str) -> Vec<String> {
    let (keep, pattern) = match pattern.strip_prefix('!') {
        Some(rest) => (true, rest),
        None => (false, pattern),
    };
    let mut text = String::new();
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.as_str().starts_with('&') => {
                text.push('&');
                chars.next();
            }
            '&' => text.push_str(word),
            c => text.push(c),
        }
    }
    let extglob = super::super::shopt::option_state("extglob").unwrap_or(false);
    let chars: Vec<(char, bool)> = text.chars().map(|c| (c, true)).collect();
    let compiled = oslo_base::glob::ShellPattern::from_chars_with(&chars, extglob);
    words
        .into_iter()
        .filter(|w| compiled.matches(w) == keep)
        .collect()
}

/// The first field of every non-comment line of `file`.
fn first_fields(file: &str, sep: char) -> Vec<String> {
    let text = std::fs::read_to_string(file).unwrap_or_default();
    text.lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .filter_map(|line| {
            let field = match sep {
                ' ' => line.split_whitespace().next(),
                sep => line.split(sep).next(),
            };
            field.filter(|f| !f.is_empty()).map(str::to_string)
        })
        .collect()
}

fn hostnames() -> Vec<String> {
    let file = std::env::var("HOSTFILE").unwrap_or_else(|_| "/etc/hosts".to_string());
    let text = std::fs::read_to_string(file).unwrap_or_default();
    text.lines()
        .map(|line| line.split('#').next().unwrap_or_default())
        .flat_map(|line| {
            line.split_whitespace()
                .skip(1)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The command name of each job; `Some(true)` only stopped ones, `Some(false)` only running.
fn jobs(stopped: Option<bool>) -> Vec<String> {
    use crate::exec::job::{JobState, with_jobs};
    with_jobs(|jobs| {
        jobs.jobs()
            .iter()
            .filter(|job| match stopped {
                None => !matches!(job.state, JobState::Completed(_)),
                Some(true) => job.state == JobState::Stopped,
                Some(false) => job.state == JobState::Running,
            })
            .filter_map(|job| job.command.split_whitespace().next().map(str::to_string))
            .collect()
    })
}

/// `EXIT`, every signal by its `SIG` name, then the shell's own trap names.
fn signals() -> Vec<String> {
    let mut out = vec!["EXIT".to_string()];
    out.extend(
        super::super::process::signals::all()
            .into_iter()
            .map(|(_, name)| format!("SIG{name}")),
    );
    out.extend(["DEBUG", "ERR", "RETURN"].map(str::to_string));
    out
}
