//! `compgen`, for the scripts and completion functions that call it.
//!
//! The actions that are a word list or a filesystem question — `-G pattern`, `-W words`, `-f` and
//! `-d`, with an optional word to complete. bash's other actions answer from readline's own tables
//! (`-A alias`, `-k`), which oslo keeps elsewhere; they are refused by name rather than guessed at.

use crate::env::scope::Environment;
use oslo_base::error::Result;
use oslo_base::glob::walk;

pub fn builtin_compgen(env: &mut Environment, args: &[String]) -> Result<i32> {
    let mut pattern = None;
    let mut list = None;
    let (mut files, mut dirs) = (false, false);
    let mut word = String::new();
    let mut rest = args.iter().skip(1);
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "-G" => pattern = rest.next().cloned(),
            "-W" => list = rest.next().cloned(),
            "-f" => files = true,
            "-d" => dirs = true,
            "--" => {
                word = rest.next().cloned().unwrap_or_default();
                break;
            }
            flag if flag.starts_with('-') && flag.len() > 1 => {
                eprintln!("compgen: {flag}: not supported; oslo answers -G, -W, -f and -d");
                return Ok(2);
            }
            other => word = other.to_string(),
        }
    }

    let mut out = Vec::new();
    if let Some(pattern) = pattern {
        out.extend(glob(&pattern));
    }
    if let Some(list) = list {
        let ifs = env.get_var("IFS").unwrap_or(" \t\n").to_string();
        out.extend(
            list.split(|c: char| ifs.contains(c))
                .filter(|w| !w.is_empty() && w.starts_with(&word))
                .map(str::to_string),
        );
    }
    if files || dirs {
        out.extend(
            starting_with(&word)
                .into_iter()
                .filter(|path| files || std::path::Path::new(path).is_dir()),
        );
    }
    for line in &out {
        println!("{line}");
    }
    Ok(if out.is_empty() { 1 } else { 0 })
}

/// A pattern's matches, or the name itself when it is plain text naming something that exists.
fn glob(pattern: &str) -> Vec<String> {
    let chars: Vec<(char, bool)> = pattern.chars().map(|c| (c, true)).collect();
    match walk::expand(&chars, &walk::shell_options()) {
        Some(matched) => matched,
        None if std::fs::symlink_metadata(pattern).is_ok() => vec![pattern.to_string()],
        None => Vec::new(),
    }
}

/// Every path that begins with `word`, hidden ones only when `word`'s last part asks for a dot.
fn starting_with(word: &str) -> Vec<String> {
    let mut chars: Vec<(char, bool)> = word.chars().map(|c| (c, false)).collect();
    chars.push(('*', true));
    let mut options = walk::shell_options();
    if word
        .rsplit('/')
        .next()
        .is_some_and(|last| last.starts_with('.'))
    {
        options.dotglob = true;
    }
    walk::expand(&chars, &options).unwrap_or_default()
}
