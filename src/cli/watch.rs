//! `oslo watch` command-line parsing and launch selection.

use oslo::watch::{Policy, WatchSpec};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ScratchChoice {
    Auto,
    Disabled,
    Required(Option<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Request {
    pub spec: WatchSpec,
    pub scratch: ScratchChoice,
    pub attach: bool,
}

pub(crate) fn run(args: &[String]) -> i32 {
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        print!("{HELP}");
        return 0;
    }
    let root = match std::env::current_dir() {
        Ok(root) => root,
        Err(error) => {
            eprintln!("oslo watch: cannot read the working directory: {error}");
            return 1;
        }
    };
    let request = match parse(args, root) {
        Ok(request) => request,
        Err(error) => {
            eprintln!("oslo watch: {error}\n{USAGE}");
            return 2;
        }
    };
    launch(request)
}

pub(crate) fn launch(request: Request) -> i32 {
    match request.scratch {
        ScratchChoice::Auto | ScratchChoice::Disabled => foreground(&request.spec),
        ScratchChoice::Required(_) => {
            eprintln!("oslo watch: this build does not support Scratch-backed watch services");
            2
        }
    }
}

fn foreground(spec: &WatchSpec) -> i32 {
    match oslo::watch::run(spec) {
        Ok(status) => status,
        Err(error) => {
            eprintln!("oslo watch: {error}");
            1
        }
    }
}

pub(crate) fn parse(args: &[String], root: PathBuf) -> Result<Request, String> {
    let split = args
        .iter()
        .position(|arg| arg == "--")
        .ok_or_else(|| "`--` is required before the command".to_string())?;
    let (front, tail) = args.split_at(split);
    let command = tail.get(1..).unwrap_or_default();
    if command.is_empty() {
        return Err("expected a command after `--`".to_string());
    }

    let mut name = None;
    let mut initial = true;
    let mut debounce = 100;
    let mut policy = Policy::Coalesce;
    let mut grace = 1000;
    let mut scratch = ScratchChoice::Auto;
    let mut attach = false;
    let mut paths = Vec::new();
    let mut i = 0;
    while i < front.len() {
        let word = &front[i];
        match word.as_str() {
            "--initial" => initial = true,
            "--postpone" => initial = false,
            "--coalesce" => policy = Policy::Coalesce,
            "--restart" => policy = Policy::Restart,
            "--foreground" => scratch = ScratchChoice::Disabled,
            "--scratch" => scratch = ScratchChoice::Required(None),
            "--attach" => attach = true,
            "--name" | "--debounce" | "--grace" => {
                let value = front
                    .get(i + 1)
                    .ok_or_else(|| format!("{word} needs a value"))?;
                match word.as_str() {
                    "--name" => name = Some(value.clone()),
                    "--debounce" => debounce = milliseconds(word, value)?,
                    "--grace" => grace = milliseconds(word, value)?,
                    _ => unreachable!(),
                }
                i += 1;
            }
            _ if word.starts_with("--name=") => name = Some(word[7..].to_string()),
            _ if word.starts_with("--debounce=") => {
                debounce = milliseconds("--debounce", &word[11..])?
            }
            _ if word.starts_with("--grace=") => grace = milliseconds("--grace", &word[8..])?,
            _ if word.starts_with("--scratch=") => {
                let chosen = &word[10..];
                if chosen.is_empty() {
                    return Err("--scratch needs a non-empty name".to_string());
                }
                scratch = ScratchChoice::Required(Some(chosen.to_string()));
            }
            _ if word.starts_with('-') => return Err(format!("{word}: unknown option")),
            _ => paths.push(word.clone()),
        }
        i += 1;
    }
    if paths.is_empty() {
        return Err("expected at least one path before `--`".to_string());
    }
    if attach && matches!(scratch, ScratchChoice::Disabled) {
        return Err("--attach cannot be combined with --foreground".to_string());
    }
    if attach && matches!(scratch, ScratchChoice::Auto) {
        scratch = ScratchChoice::Required(None);
    }
    let name = name.unwrap_or_else(|| {
        PathBuf::from(&command[0])
            .file_name()
            .and_then(|part| part.to_str())
            .unwrap_or("command")
            .to_string()
    });
    Ok(Request {
        spec: WatchSpec {
            name,
            root,
            patterns: paths,
            argv: command.to_vec(),
            initial,
            debounce: Duration::from_millis(debounce),
            policy,
            grace: Duration::from_millis(grace),
        },
        scratch,
        attach,
    })
}

fn milliseconds(option: &str, value: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .map_err(|_| format!("{option}: expected milliseconds, got {value:?}"))
}

const USAGE: &str = "usage: oslo watch [OPTIONS] PATH... -- COMMAND [ARG...]";

const HELP: &str = "USAGE
  oslo watch [OPTIONS] PATH... -- COMMAND [ARG...]

OPTIONS
  --name NAME       logical service name
  --initial         run before the first event (default)
  --postpone        wait for the first event
  --debounce MS     quiet period after a burst (default: 100)
  --coalesce        finish the child, then run once if dirty (default)
  --restart         replace the running process group after a change
  --grace MS        TERM-to-KILL delay (default: 1000)
  --foreground      run in this terminal
  --scratch[=NAME]  require a generated or exact Scratch
  --attach          start in Scratch and attach immediately
  -h, --help        this text
";

#[cfg(test)]
#[path = "watch/tests.rs"]
mod tests;
