//! `help [-dms] [pattern …]` — the synopsis and one-line description of each bash help topic.
//!
//! The topics are bash's own: every builtin and the keywords bash documents under `help`, with
//! the usage line and the short description bash prints for `help -s` and `help -d`. Without a
//! pattern, `help` lists every synopsis in two columns half of `$COLUMNS` wide.

use crate::env::scope::Environment;
use oslo_base::error::Result;

/// `name \t synopsis \t description`, one topic per line, in bash's order.
const TOPICS: &str = include_str!("help/topics.tsv");

struct Topic {
    name: &'static str,
    synopsis: &'static str,
    description: &'static str,
}

/// Every help topic, in the order bash lists them.
pub fn topics() -> impl Iterator<Item = &'static str> {
    all().map(|topic| topic.name)
}

fn all() -> impl Iterator<Item = Topic> {
    TOPICS.lines().filter_map(|line| {
        let mut fields = line.splitn(3, '\t');
        Some(Topic {
            name: fields.next()?,
            synopsis: fields.next()?,
            description: fields.next()?,
        })
    })
}

#[derive(Default)]
struct Flags {
    description: bool,
    manpage: bool,
    synopsis: bool,
}

pub fn builtin_help(env: &mut Environment, args: &[String]) -> Result<i32> {
    let mut flags = Flags::default();
    let mut rest = &args[1.min(args.len())..];
    while let Some(arg) = rest.first() {
        if arg == "--" {
            rest = &rest[1..];
            break;
        }
        if arg.len() < 2 || !arg.starts_with('-') {
            break;
        }
        for c in arg[1..].chars() {
            match c {
                'd' => flags.description = true,
                'm' => flags.manpage = true,
                's' => flags.synopsis = true,
                other => {
                    crate::env::complain_option(
                        args,
                        other,
                        &format!("help: -{other}: invalid option"),
                        "help: usage: help [-dms] [pattern ...]",
                    );
                    return Ok(2);
                }
            }
        }
        rest = &rest[1..];
    }

    if rest.is_empty() {
        list(env);
        return Ok(0);
    }

    if rest[0].contains(['*', '?', '[']) {
        let plural = if rest.len() > 1 {
            "keywords"
        } else {
            "keyword"
        };
        println!("Shell commands matching {plural} `{}'\n", rest.join(", "));
    }
    let mut status = 0;
    for pattern in rest {
        if !show(env, pattern, &flags) {
            let body = format!(
                "help: no help topics match `{pattern}'.  Try `help help' or `man -k {pattern}' or `info {pattern}'."
            );
            crate::env::complain(args, pattern, &body, "no such topic", None);
            status = 1;
        }
    }
    Ok(status)
}

/// `cd --help`: bash prints the builtin's help to stdout and fails it as a usage error, which is
/// fatal for a special builtin in POSIX mode. The builtins that take no options are exempt.
pub(crate) fn long_option(name: &str, args: &[String]) -> Option<Result<i32>> {
    const NO_OPTIONS: [&str; 6] = [":", "[", "echo", "false", "test", "true"];
    if args.get(1).map(String::as_str) != Some("--help") || NO_OPTIONS.contains(&name) {
        return None;
    }
    let topic = all().find(|topic| topic.name == name)?;
    // `bind` warns before it reads any option, `--help` included.
    if name == "bind" && !crate::exec::pipeline::is_interactive() {
        eprintln!(
            "{}bind: warning: line editing not enabled",
            crate::env::origin_now()
        );
    }
    println!("{}: {}", topic.name, topic.synopsis);
    println!("    {}", topic.description);
    Some(Err(oslo_base::error::ShellError::utility_error(name, 2)))
}

/// Print every topic `pattern` names: exact and glob matches, else prefix matches.
fn show(env: &Environment, pattern: &str, flags: &Flags) -> bool {
    let glob = oslo_base::glob::ShellPattern::from_unquoted(pattern);
    let mut matched: Vec<Topic> = all()
        .filter(|topic| topic.name == pattern || glob.matches(topic.name))
        .collect();
    if matched.is_empty() {
        matched = all()
            .filter(|topic| topic.name.starts_with(pattern))
            .collect();
    }
    for topic in &matched {
        if flags.description {
            println!("{} - {}", topic.name, topic.description);
        } else if flags.manpage {
            manpage(env, topic);
        } else {
            println!("{}: {}", topic.name, topic.synopsis);
            if !flags.synopsis {
                println!("    {}", topic.description);
            }
        }
    }
    !matched.is_empty()
}

fn manpage(env: &Environment, topic: &Topic) {
    println!("NAME\n    {} - {}\n", topic.name, topic.description);
    println!("SYNOPSIS\n    {}\n", topic.synopsis);
    println!("DESCRIPTION\n    {}\n", topic.description);
    println!("SEE ALSO\n    bash(1)\n");
    println!("IMPLEMENTATION\n    {}\n", version_line(env));
}

fn version_line(env: &Environment) -> String {
    let version = env.get_var("BASH_VERSION").unwrap_or_default();
    let machine = env.get_var("MACHTYPE").unwrap_or("x86_64-pc-linux-gnu");
    format!("GNU bash, version {version} ({machine})")
}

/// The header and the two-column list of synopses, a star marking a disabled builtin.
fn list(env: &Environment) {
    println!("{}", version_line(env));
    println!(
        "These shell commands are defined internally.  Type `help' to see this list.\n\
         Type `help name' to find out more about the function `name'.\n\
         Use `info bash' to find out more about the shell in general.\n\
         Use `man -k' or `info' to find out more about commands not in this list.\n\
         \n\
         A star (*) next to a name means that the command is disabled.\n"
    );
    let columns = env
        .get_var("COLUMNS")
        .and_then(|c| c.trim().parse::<usize>().ok())
        .filter(|c| *c > 0)
        .unwrap_or(80);
    let width = match (columns / 2).min(128) {
        w if w <= 3 => 40,
        w => w,
    };
    let topics: Vec<Topic> = all().collect();
    let height = topics.len().div_ceil(2);
    let cell = |topic: &Topic, room: usize| {
        let mark = match disabled(env, topic.name) {
            true => '*',
            false => ' ',
        };
        let text = format!("{mark}{}", topic.synopsis);
        match text.len() > room {
            true => format!("{}>", &text[..room - 1]),
            false => text,
        }
    };
    for row in 0..height {
        let left = cell(&topics[row], width - 1);
        match topics.get(row + height) {
            Some(right) => println!("{left:<width$}{}", cell(right, width - 2)),
            None => println!("{left}"),
        }
    }
}

/// Whether `name` is a builtin `enable -n` has turned off.
fn disabled(env: &Environment, name: &str) -> bool {
    env.builtin_states()
        .iter()
        .any(|(builtin, on)| builtin == name && !on)
}
