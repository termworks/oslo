//! `glob` — pathname expansion as a command, with qualifiers and regex.
//!
//! ```sh
//! glob '**/*.log' --older 7d --larger 1M --by time --rev --first 5
//! glob '**/*' --re '_v[0-9]+\.' --rows | from tsv | where 'size > 1000'
//! glob --match 'a*.txt' abc.txt && echo yes
//! ```
//!
//! **The scriptable half of the prompt's `pattern(older 7d)`.** A new name, so it changes the
//! meaning of no existing script; the qualifiers are the same parser the prompt and
//! `oslo.glob` use (`oslo_base::glob::qualify`). No match prints nothing and answers 1 — never the
//! pattern text, which is the shell convention a script least wants from a command.

use crate::env::scope::Environment;
use oslo_base::error::Result;
use oslo_base::glob::qualify::{self, Entry};
use oslo_base::glob::{ShellPattern, walk};

const USAGE: &str = "glob: usage: glob [-0] [--count] [--rows] [--QUALIFIER [VALUE]]... PATTERN...
       glob --match PATTERN NAME...";

pub fn builtin_glob(_env: &mut Environment, args: &[String]) -> Result<i32> {
    let mut patterns = Vec::new();
    let mut items: Vec<Vec<String>> = Vec::new();
    let (mut nul, mut count, mut rows, mut globstar) = (false, false, false, true);
    let mut test_mode = false;
    let mut rest = args.iter().skip(1);
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "-0" | "--null" => nul = true,
            "-c" | "--count" => count = true,
            "--rows" => rows = true,
            "--no-globstar" => globstar = false,
            "--match" => test_mode = true,
            "--" => {
                patterns.extend(rest.cloned());
                break;
            }
            flag if flag.starts_with("--") => {
                let name = &flag[2..];
                let mut item: Vec<String> = match name {
                    "path-re" => vec!["path".into(), "re".into()],
                    other => vec![other.to_string()],
                };
                if qualify::VALUED.contains(&name) || name == "path-re" {
                    match rest.next() {
                        Some(value) => item.push(value.clone()),
                        None => return usage(&format!("{flag} needs a value")),
                    }
                }
                items.push(item);
            }
            _ => patterns.push(arg.clone()),
        }
    }
    if patterns.is_empty() {
        return usage("needs a pattern");
    }
    let q = match qualify::from_items(&items) {
        Ok(q) => q,
        Err(problem) => {
            eprintln!("glob: {problem}");
            return Ok(2);
        }
    };

    // `glob --match PATTERN NAME...`: the pattern first, as in `[[ NAME == PATTERN ]]` read
    // aloud; true only when every name matches, and nothing on disk is consulted.
    if test_mode {
        let Some((pattern, names)) = patterns.split_first() else {
            return usage("--match needs a pattern and at least one name");
        };
        if names.is_empty() {
            return usage("--match needs a pattern and at least one name");
        }
        let pattern = ShellPattern::from_unquoted(pattern);
        let every = names.iter().all(|n| pattern.matches_case(n, q.nocase));
        return Ok(i32::from(!every));
    }

    let mut found: Vec<(String, String)> = Vec::new();
    for pattern in &patterns {
        let mut options = walk::shell_options();
        options.globstar = globstar;
        options.dotglob |= q.hidden;
        options.nocase |= q.nocase;
        let chars: Vec<(char, bool)> = pattern.chars().map(|c| (c, true)).collect();
        let matched = match walk::expand(&chars, &options) {
            Some(matched) => matched,
            // A pattern with nothing that globs names itself, when it exists.
            None if std::fs::symlink_metadata(oslo_base::lossless::to_os(pattern)).is_ok() => {
                vec![pattern.clone()]
            }
            None => Vec::new(),
        };
        let base = qualify::base_of(pattern);
        for path in qualify::apply(matched, &q, base, options.collate) {
            found.push((path, base.to_string()));
        }
    }

    let mut out = String::new();
    if count {
        out.push_str(&format!("{}\n", found.len()));
    } else if rows {
        out.push_str("path\tname\text\tkind\tsize\tmodified\tmode\tdepth\n");
        for (path, base) in &found {
            let e = Entry::of(path.clone(), base);
            let mode = e.lstat.as_ref().map_or(0, |m| {
                std::os::unix::fs::PermissionsExt::mode(&m.permissions()) & 0o7777
            });
            out.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{:o}\t{}\n",
                e.path,
                e.name(),
                e.ext(),
                e.kind(),
                e.size,
                e.mtime / 1_000_000_000,
                mode,
                e.depth
            ));
        }
    } else {
        let end = if nul { '\0' } else { '\n' };
        for (path, _) in &found {
            out.push_str(path);
            out.push(end);
        }
    }
    let written = super::io::write_stdout("glob", out.as_bytes());
    if written != 0 {
        return Ok(written);
    }
    Ok(i32::from(found.is_empty()))
}

fn usage(problem: &str) -> Result<i32> {
    eprintln!("glob: {problem}");
    eprintln!("{USAGE}");
    Ok(2)
}
