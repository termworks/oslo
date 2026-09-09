//! `argc` — a script's own arguments, parsed from the comments that declare them.
//!
//! ```sh
//! #!/usr/bin/env oslo
//! # @describe   Send it somewhere
//! # @option -t --tries <NUM>   how many times
//! # @arg     target!           where to
//! argc "$@"                    # ← and now $argc_tries and $argc_target are set
//! ```
//!
//! # Why this is a builtin and not the `eval` line
//!
//! In bash the same feature is spelled `eval "$(argc --argc-eval "$0" "$@")"`, which is a program to
//! find, a fork, a pipe, a quoting round trip and an `eval` of whatever came back. oslo has the
//! parser linked in, so the builtin runs it and assigns the results **directly**:
//!
//! ```text
//! bash:  argc(1) ──► text ──► eval ──► variables
//! oslo:  argc     ──────────────────► variables
//! ```
//!
//! What that buys, beyond the fork: errors are reported the way every other builtin reports them,
//! and it works for a script that has **no file**. A macro-stored script runs from memory with `$0`
//! set to its own name, so `--argc-eval "$0"` has no path to read — the idiom bash needs cannot work
//! there, and this needs no path at all.
//!
//! `oslo --argc-eval` exists too, for bash scripts; see `src/cli.rs`. Both are the same parse.

mod call;
pub mod complete;
mod runtime;

use crate::env::Environment;
use argc::ArgcValue;

pub use runtime::Shell;

/// `argc "$@"` — parse this script's arguments and apply them.
///
/// The status is the shell's: `0` when the parse succeeded, and whatever `argc` asked for when it
/// did not — `0` for `--help`, which printed what was wanted, and `1` for a real mistake.
pub fn builtin_argc(env: &mut Environment, args: &[String]) -> oslo_base::error::Result<i32> {
    // **`eval "$(argc --argc-eval "$0" "$@")"` reaches here too**, and has to be answered rather
    // than parsed as arguments. It is what every bash script written against `argc` says, and oslo
    // is what runs one whenever `#!/usr/bin/env bash` finds an oslo named `bash` on `$PATH` — or
    // whenever a stored script is run at all. Parsed as arguments it produced `unexpected argument
    // `--argc-eval``, on a script that works in every other shell.
    if args.get(1).is_some_and(|word| word == "--argc-eval") {
        return Ok(eval_text(env, args.get(2..).unwrap_or_default()));
    }
    // `$0` is the script's name, which is what a stored macro has instead of a path and what a file
    // on disk has as well as one. The runtime tries the macro store first and the filesystem after.
    let name = env.shell_name.clone();
    let source = match source_of(env, &name) {
        Some(source) => source,
        // **Nothing to parse means this was not called from a script**, which at a prompt is what
        // `argc` on its own is: `$0` is the shell. Every other builtin answers `--help` with what it
        // is for, and reporting "cannot read the script" about the shell binary explains nothing.
        None => {
            let asked = args
                .iter()
                .skip(1)
                .any(|word| word == "--help" || word == "-h");
            let usage = self_help(&name);
            if asked {
                println!("{usage}");
                return Ok(0);
            }
            eprintln!("{usage}");
            return Ok(2);
        }
    };

    // The words `argc` matches: the script's name, then the arguments as given. **`args[0]` is the
    // builtin's own name** — every builtin here is called with it, the way `argv[0]` works — so it
    // is dropped: what `argc` wants after the name is what the script was called with.
    //
    // The *base* name, because this word is what the generated help calls the command. `$0` for a
    // script found on `$PATH` is the path it was found at, and `USAGE: /usr/local/bin/deploy` names
    // something nobody types. The path is still passed separately, which is what reads the file.
    let mut words = vec![basename(&name)];
    words.extend(args.iter().skip(1).cloned());

    let runtime = Shell::new(env);
    let values = match argc::eval(runtime, &source, &words, Some(&name), width()) {
        Ok(values) => values,
        Err(problem) => {
            eprintln!("oslo: argc: {problem}");
            return Ok(1);
        }
    };
    apply(env, &values)
}

/// Set what the parse decided, and answer with the status the script carries on with.
///
/// **`--help` and a usage error end the script**, as `ShellError::Exit`. That is not a convenience:
/// the bash rendering ends its text with `exit 0` or `exit 1`, so a script that asked for help stops
/// there rather than running its body with nothing set. A builtin that merely returned a status
/// would leave the next line to run — observed, before this returned the error instead.
fn apply(env: &mut Environment, values: &[ArgcValue]) -> oslo_base::error::Result<i32> {
    let mut call_this: Option<String> = None;
    for value in values {
        match value {
            ArgcValue::Single(name, value) => {
                env.set_var(&argc_name(name), value, false);
            }
            ArgcValue::SingleFn(name, function) => {
                let out = crate::exec::substitution::eval_command_substitution(env, function)
                    .unwrap_or_default();
                env.set_var(&argc_name(name), &out, false);
            }
            ArgcValue::Multiple(name, values) => set_array(env, &argc_name(name), values),
            ArgcValue::PositionalSingle(name, value) => {
                env.set_var(&argc_name(name), value, false);
            }
            ArgcValue::PositionalSingleFn(name, function) => {
                let out = crate::exec::substitution::eval_command_substitution(env, function)
                    .unwrap_or_default();
                env.set_var(&argc_name(name), &out, false);
            }
            ArgcValue::PositionalMultiple(name, values) => set_array(env, &argc_name(name), values),
            ArgcValue::ExtraPositionalMultiple(values) => {
                set_array(env, "argc__positionals", values);
            }
            ArgcValue::Env(name, value) | ArgcValue::EnvFn(name, value) => {
                env.set_var(name, value, true);
            }
            ArgcValue::Map(name, map) => {
                // A `@option --set <K=V>` collected as pairs. Written flat, one array of `k=v`:
                // the shell has no associative array a script could read back portably.
                let flat: Vec<String> = map
                    .iter()
                    .map(|(key, values)| format!("{key}={}", values.join(",")))
                    .collect();
                set_array(env, &argc_name(name), &flat);
            }
            ArgcValue::CommandFn(function) => call_this = Some(function.clone()),
            ArgcValue::ParamFn(_) | ArgcValue::Hook(_) => {}
            ArgcValue::Dotenv(_) | ArgcValue::RequireTools(_) => {}
            ArgcValue::ExternalSubcommand(..) => {}
            // **Printed and done.** `--help` and a usage error both arrive here; the status is the
            // one `argc` chose, and `0` for help is what makes `script --help` succeed.
            ArgcValue::Error((message, status)) => {
                if *status == 0 {
                    println!("{message}");
                } else {
                    eprintln!("{message}");
                }
                return Err(oslo_base::error::ShellError::Exit(*status));
            }
        }
    }

    // The subcommand the arguments selected. Called here so a script is `argc "$@"` and nothing
    // else — the same thing the `eval` line does by leaving a call at the end of what it prints.
    match call_this {
        Some(function) => match crate::syntax::parse_bash_script(&function) {
            Ok(program) => crate::exec::eval_command_list(env, &program),
            Err(problem) => {
                eprintln!("oslo: argc: {problem}");
                Ok(1)
            }
        },
        None => Ok(0),
    }
}

/// What `argc` is, for somebody who typed it at a prompt.
///
/// The name is included because `$0` is the only reason this is being printed: it says which script
/// was looked for and did not exist, which is the useful half of the old error message.
fn self_help(name: &str) -> String {
    format!(
        "usage: argc [ARG]...        — from inside a script, as `argc \"$@\"`\n\
         \n\
         Parses the script's own `# @option`, `# @flag`, `# @arg` and `# @cmd` comments and sets\n\
         `$argc_*` from them, generating `--help` and reporting a bad argument.\n\
         \n\
         There is no script here: `$0` is {name}. In a bash script the same thing is spelled\n\
         `eval \"$(oslo --argc-eval \"$0\" \"$@\")\"`."
    )
}

/// The last component of a path, which is what a command is called.
fn basename(name: &str) -> String {
    name.rsplit('/').next().unwrap_or(name).to_string()
}

/// What a parse decided, with no `argc` types in it.
///
/// **The point of the shape is what it keeps out.** `ArgcValue` is the vendored crate's enum, and
/// naming it in `oslo-runtime` would put the whole dependency — and its feature gate — in a second
/// crate for the sake of one binding. This is the same information as a list of names to values.
pub enum Parsed {
    /// One entry per option or argument that was given a value.
    Values(Vec<(String, Vec<String>)>),
    /// `--help`, or a usage mistake: the text argc rendered, and the status it chose. Zero is help
    /// that was asked for; anything else is the mistake.
    Message(String, i32),
}

/// Parse `words` against the declaration in `source`, without touching any shell.
///
/// The declaration is the same `# @option` comment block the `argc` builtin reads — one syntax, so
/// a script, a recipe and a registered builtin describe their arguments the same way. See
/// [`Shell::detached`] for what a declaration loses by having no shell: a `` `_fn` `` default
/// computes to nothing.
///
/// `words[0]` is the command's name, as argc expects — it is what the rendered help calls it.
pub fn parse_words(source: &str, words: &[String], width: Option<usize>) -> Result<Parsed, String> {
    let values = argc::eval(Shell::detached(), source, words, None, width)
        .map_err(|problem| problem.to_string())?;
    let mut out = Vec::new();
    for value in &values {
        match value {
            ArgcValue::Single(name, value)
            | ArgcValue::SingleFn(name, value)
            | ArgcValue::PositionalSingle(name, value)
            | ArgcValue::PositionalSingleFn(name, value) => {
                out.push((name.clone(), vec![value.clone()]));
            }
            ArgcValue::Multiple(name, values) | ArgcValue::PositionalMultiple(name, values) => {
                out.push((name.clone(), values.clone()));
            }
            ArgcValue::ExtraPositionalMultiple(values) => {
                out.push(("__positionals".to_string(), values.clone()));
            }
            ArgcValue::Env(name, value) | ArgcValue::EnvFn(name, value) => {
                out.push((name.clone(), vec![value.clone()]));
            }
            ArgcValue::Map(name, map) => {
                let flat = map
                    .iter()
                    .map(|(key, values)| format!("{key}={}", values.join(",")))
                    .collect();
                out.push((name.clone(), flat));
            }
            // A subcommand the arguments selected: named, not called. **A binding must not run
            // anything** — the caller asked what the words mean, not for them to be acted on.
            ArgcValue::CommandFn(function) => {
                out.push(("__command".to_string(), vec![function.clone()]));
            }
            ArgcValue::ParamFn(_) | ArgcValue::Hook(_) => {}
            ArgcValue::Dotenv(_) | ArgcValue::RequireTools(_) => {}
            ArgcValue::ExternalSubcommand(..) => {}
            ArgcValue::Error((message, status)) => {
                return Ok(Parsed::Message(message.clone(), *status));
            }
        }
    }
    Ok(Parsed::Values(out))
}

/// The usage text a declaration renders, without parsing anything.
pub fn usage_of(source: &str, name: &str, width: Option<usize>) -> Result<String, String> {
    let words = vec![name.to_string(), "--help".to_string()];
    match parse_words(source, &words, width)? {
        Parsed::Message(text, _) => Ok(text),
        // `--help` always ends in an `Error` carrying the text, so this is the declaration having
        // declared a `--help` of its own — in which case there is nothing generated to show.
        Parsed::Values(_) => Err("the declaration defines its own --help".to_string()),
    }
}

/// `argc_tries`, from `tries`. The prefix is argc's, and scripts are written against it.
fn argc_name(id: &str) -> String {
    format!("argc_{}", id.replace('-', "_"))
}

fn set_array(env: &mut Environment, name: &str, values: &[String]) {
    // An indexed array, which is what `argc_files=( a b )` is in the bash rendering.
    let mut array = crate::env::scope::ShellArray::default();
    for (at, value) in values.iter().enumerate() {
        array.set(at as i64, value.clone());
    }
    env.set_array(name, array);
}

/// The script's source, by name.
/// `argc --argc-eval <script> [arg]…` — the parse as *text*, for an `eval` to apply.
///
/// The difference from the builtin above is only what happens to the answer: bash cannot be handed
/// a parse, so the program prints assignments and the `eval` around the call runs them. Same parser,
/// same store-then-disk lookup, so a script gets the same answer whichever idiom it was written
/// with.
fn eval_text(env: &mut Environment, words: &[String]) -> i32 {
    let Some(path) = words.first().cloned() else {
        eprintln!("usage: argc --argc-eval <SCRIPT> [ARG]...");
        return 1;
    };
    let Some(source) = source_of(env, &path) else {
        eprintln!("oslo: argc: {path}: cannot be read");
        return 1;
    };
    // The base name, for the same reason as above: it is what the generated usage calls the command.
    let mut words = words.to_vec();
    words[0] = basename(&path);

    let runtime = Shell::new(env);
    match argc::eval(runtime, &source, &words, Some(&path), width()) {
        Ok(values) => {
            print!("{}", argc::ArgcValue::to_bash(&values));
            0
        }
        // Not reported here: `argc` renders a usage error as shell code that prints it and exits, so
        // the script complains in its own name rather than oslo complaining in ours.
        Err(problem) => {
            eprintln!("oslo: argc: {problem}");
            1
        }
    }
}

fn source_of(env: &mut Environment, name: &str) -> Option<String> {
    use argc::Runtime;
    Shell::new(env).read_to_string(name)
}

/// How wide help text may be, from the terminal.
fn width() -> Option<usize> {
    oslo_ui::dropdown::width::terminal_cols().checked_sub(2)
}

#[cfg(test)]
#[path = "argc/tests.rs"]
mod tests;
