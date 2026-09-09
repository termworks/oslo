//! `oslo --argc-eval` — being the `argc` binary, for a script that is not oslo's.
//!
//! ```sh
//! #!/usr/bin/env bash
//! # @option -t --tries <NUM>
//! eval "$(oslo --argc-eval "$0" "$@")"
//! ```
//!
//! bash cannot be handed a parse; it can only be handed *text to run*. So this prints the same shell
//! code the `argc` binary prints — assignments, arrays, the call to the chosen function — and the
//! `eval` around it does the rest. A script written against `argc` works unchanged with `oslo` in
//! its place, which is the point: one fewer program to install.
//!
//! # Not the same thing as the builtin
//!
//! For a script oslo itself runs, [`oslo_shell::argc`] applies the parse directly and none of this
//! text exists. The two share the parser and differ only in what they do with the answer:
//!
//! ```text
//! bash script   →  oslo --argc-eval  →  text  →  eval  →  variables
//! oslo script   →  argc "$@"                          →  variables
//! ```
//!
//! # `$0` has to be a path here
//!
//! `--argc-eval` is given `"$0"` and reads the script from it. That works for a file on disk and
//! cannot work for a macro-stored script, whose `$0` is its own name because it runs from memory —
//! the builtin is the answer there, and it needs no path at all.

use oslo_shell::argc::Shell;

/// Print the shell code for `words` — the script path, then its arguments.
///
/// The status is the process's: `0` when something was printed to run, `1` when there was nothing
/// to read. **A parse error is not an error here**: `argc` renders "unknown option" as shell code
/// that prints the message and exits, so the script reports it in its own name rather than oslo
/// reporting it in ours.
pub fn eval(words: &[String]) -> i32 {
    let Some(path) = words.first() else {
        eprintln!("usage: oslo --argc-eval <SCRIPT> [ARG]...");
        return 1;
    };
    use argc::Runtime;
    let mut env = oslo_shell::env::Environment::new();
    let runtime = Shell::new(&mut env);
    // **The macro store first, then the disk**, which is what the builtin's runtime already does.
    // Reading the path alone could not see a stored script at all — the one case with no file, and
    // the one this exists to cover: a bash script keeps the `argc` idiom when it is stored, and its
    // `$0` is then a name rather than a path.
    let Some(source) = runtime.read_to_string(path) else {
        eprintln!("oslo: --argc-eval: {path}: cannot be read");
        return 1;
    };

    // What `argc` matches against: the script's own name first, then the arguments as given. The
    // caller passes `"$0" "$@"`, so `words` is already in that order — except that the first word is
    // replaced by its *base* name, because it is what the generated help calls the command and `$0`
    // for a script found on `$PATH` is the path it was found at. `USAGE: /usr/local/bin/release`
    // names something nobody types. The path is still passed separately and is what reads the file.
    let mut words = words.to_vec();
    words[0] = path.rsplit('/').next().unwrap_or(path).to_string();

    match argc::eval(runtime, &source, &words, Some(path), width()) {
        Ok(values) => {
            print!("{}", argc::ArgcValue::to_bash(&values));
            0
        }
        Err(problem) => {
            eprintln!("oslo: --argc-eval: {problem}");
            1
        }
    }
}

/// How wide help text may be. `$COLUMNS` first, because the script's shell knows and this process
/// may have no terminal of its own — it is usually inside a `$(…)` with its output on a pipe.
fn width() -> Option<usize> {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|cols| cols.parse::<usize>().ok())
        .or_else(|| Some(oslo::ui::dropdown::width::terminal_cols()))
        .and_then(|cols| cols.checked_sub(2))
}
