//! **Every diagnostic that names something gets a caret, or says why not.**
//!
//! The two other suites hold the *behaviour*: `diagnostics_stay_plain.rs` that a pipe sees what it
//! always saw, `diagnostics_draw_a_caret.rs` that a terminal sees the report. Neither notices the
//! thing most likely to happen next — a diagnostic written six months from now, printing
//! `mycmd: {operand}: invalid whatever` with an `eprintln!` because that is what the line above it
//! used to do.
//!
//! There is nothing to see when that happens. The message is perfectly ordinary; it is only wrong
//! next to the report the message beside it draws. So this scans the source instead.
//!
//! # It scans the whole workspace, and that is the point
//!
//! The first version of this test scanned `env/builtins/` and `data/` — the two places the
//! conversion had started from — and passed. It was missing `{}: command not found`, the single
//! most common diagnostic the shell prints, because that lives in `exec/simple/notfound.rs`.
//!
//! A sweep that only looks where you already looked is not a sweep. So the scan is every `.rs` file
//! under `crates/*/src` and `src/`, and the exclusions are named one at a time below.
//!
//! # What counts as naming something
//!
//! A format string that opens with `{}` — the origin prefix every diagnostic carries — and holds at
//! least one more placeholder. The origin says a *shell diagnostic*; the second placeholder says it
//! is about a value rather than a condition. `{}cd: HOME not set` has none and is not scanned;
//! `{}{}: command not found` has one and is.

use std::path::{Path, PathBuf};

/// Every crate's source, and the binary's.
///
/// Not `vendor/`: somebody else's code, kept close to upstream, and not `tests/`, which is this.
const SCANNED: [&str; 7] = [
    "crates/oslo-base/src",
    "crates/oslo-shell/src",
    "crates/oslo-runtime/src",
    "crates/oslo-ui/src",
    "crates/oslo-luavm/src",
    "crates/oslo-math/src",
    "src",
];

/// The ones still printing a bare `eprintln!`, and why each one is not converted.
///
/// **A reason per entry, not a list.** "Not done yet" is not a reason; every line below says what
/// about that site makes a caret unavailable or unhelpful, and a site that gains what it lacks
/// should be converted and its row taken out.
///
/// The great majority are one of two kinds: a failure that carries an *errno or a subsystem's
/// message* rather than a word somebody typed, and a value that has already been consumed by the
/// time it is complained about.
const KEPT: &[(&str, &str)] = &[
    // ── An error from somewhere else, passed through. There is no word: the placeholder is a
    //    message, and pointing at it would be pointing at a sentence.
    ("{e}", "a subsystem's own message, passed through"),
    ("{}", "the same, positionally"),
    ("{}: {}", "a name and a message the name did not choose"),
    ("{problem}", "a config or macro problem, already a sentence"),
    (
        "{body}",
        "the refusing helper has no operand when this branch is reached",
    ),
    (
        "funced: {name}: {problem}",
        "the parser is reporting edited function content, not the command-line name",
    ),
    (
        "set -U: {problem}",
        "the universal-variable store's complete error message",
    ),
    ("{name}: {e}", "a verb and the error its Lua raised"),
    ("{name}: {message}", "the same"),
    ("{label}: {}", "a redirection failure, named by its label"),
    ("mark: {problem}", "the macro store's own words"),
    ("scratch: {err}", "the scratch store's own words"),
    ("math: {why}", "the calculator's own words"),
    ("ui: {why}", "a widget's own words"),
    ("umask: {e}", "an errno"),
    ("trap: DEBUG: {e}", "an errno from the handler"),
    ("suspend: cannot suspend: {}", "an errno"),
    ("set: {}", "the option parser's own message"),
    ("let: {}", "the arithmetic evaluator's own message"),
    (
        "eval: {}",
        "the parser's own message; `complain_at` covers the outer one",
    ),
    ("exec: {}", "an errno from `execve`"),
    ("unset: {}", "the assignment machinery's own message"),
    ("printf: {message}", "the formatter's own message"),
    ("mapfile: {}", "an errno"),
    ("mapfile: read error: {}", "an errno"),
    ("{name}: write error: {}", "an errno on a descriptor"),
    (
        "from {format}: {e}",
        "the parser's message; the format is already named in it",
    ),
    (
        "kill: ({operand}) - {}",
        "an errno from `kill(2)`: the pid is fine, the call failed",
    ),
    (
        "rm: cannot remove '{shown}': {}",
        "an errno; the path is already quoted in the message",
    ),
    (
        "read: {}: {err}",
        "an I/O failure on a descriptor, not about the name being read into",
    ),
    (
        "nav: {program}: {error}",
        "the program is one oslo chose to run, not one you typed",
    ),
    // ── A value already consumed, or a function with neither the word nor a line to put it in.
    (
        "printf: {}: invalid number",
        "`Spec::render` is handed one argument and no argv — the words are three frames up, and \
         threading them through a formatter called once per conversion is a lot of signature for \
         a caret under the only word in the message",
    ),
    (
        "printf: {value}: invalid number",
        "the same function, the same reason",
    ),
    (
        "printf: {width}: width is too large",
        "the same formatter and the same reason — the padding guard sits inside the conversion, \
         which is handed a width and no argv",
    ),
    (
        "getopts: {} -- {}",
        "the message is `getopts`'s own OPTERR text, not a word of the command",
    ),
    (
        "{}: current: no such job",
        "`current` is not a word anybody typed",
    ),
    ("{}: no job control", "a condition of the shell, not a word"),
    (
        "scratch: -k takes a name\\n{USAGE}",
        "the value is missing, so there is nothing to point at",
    ),
    (
        "{name}: a column name and an expression are required",
        "the same: both are absent",
    ),
    (
        "{name}: the other stream is required, as a Lua expression answering rows",
        "the operand is absent",
    ),
    (
        "{builtin}: only meaningful in a `for', `while', or `until' loop",
        "a condition of where the shell is, not a word in the command",
    ),
    (
        "pwd: error retrieving current directory: {e}",
        "an errno from `getcwd`",
    ),
    (
        "printf: `%{}': invalid format character",
        "`Spec::render` has the conversion character and not the format string it came from; the
         sibling `missing format character`, which does have it, points into the format",
    ),
    (
        "mark: {here} has no name to mark it by; `mark NAME` chooses one",
        "`here` is $PWD, which nobody typed on this line",
    ),
    (
        "{}: registered builtin could not be resolved",
        "an internal inconsistency — a builtin dispatched under a name it is not registered as;
         there is no user word involved at all",
    ),
];

/// Every `.rs` file under the scanned directories, except the test modules beside them.
fn sources() -> Vec<(PathBuf, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut found = Vec::new();
    let mut stack: Vec<PathBuf> = SCANNED.iter().map(|d| root.join(d)).collect();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("read {dir:?}: {e}")) {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs")
                && path.file_name().is_some_and(|n| n != "tests.rs")
            {
                let text =
                    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
                found.push((path, text));
            }
        }
    }
    assert!(
        found.len() > 200,
        "the scan found almost nothing: {}",
        found.len()
    );
    found
}

/// The `{}…{…}…` format strings a file passes to `eprintln!` — an origin, and a value.
fn naming_diagnostics(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (at, _) in text.match_indices("eprintln!(") {
        let rest = &text[at..];
        // The first string literal after the macro name, which is the format string. Bounded so a
        // macro with no literal at all cannot run to the end of the file looking for one.
        let window = &rest[..rest.len().min(400)];
        let Some(open) = window.find('"') else {
            continue;
        };
        let Some(close) = window[open + 1..].find('"') else {
            continue;
        };
        let format = &window[open + 1..open + 1 + close];
        // The origin prefix, then something else that is interpolated.
        let Some(body) = format.strip_prefix("{}") else {
            continue;
        };
        // `{{` is an escaped brace, not a placeholder: `parse '{{user}}:{{uid}}'` names nothing.
        if body.replace("{{", "").replace("}}", "").contains('{') {
            found.push(body.to_string());
        }
    }
    found
}

/// The rule.
#[test]
fn a_diagnostic_that_names_something_draws_a_caret_or_says_why_not() {
    let mut unexplained: Vec<String> = Vec::new();
    for (path, text) in sources() {
        for message in naming_diagnostics(&text) {
            if KEPT.iter().any(|(kept, _)| *kept == message) {
                continue;
            }
            unexplained.push(format!("{}: {message}", path.display()));
        }
    }
    unexplained.sort();
    unexplained.dedup();
    assert!(
        unexplained.is_empty(),
        "these print a bare `eprintln!` about something they name.\n\
         Give each one `crate::env::complain(...)`, or add it to KEPT with a reason:\n  {}",
        unexplained.join("\n  ")
    );
}

/// **`KEPT` may not rot.** An entry for a message nobody prints any more is a reason that reads as
/// current and is not, which is worse than no entry — and the way it happens is a site being
/// converted without its row being taken out.
#[test]
fn every_kept_entry_names_a_message_that_exists() {
    let sources = sources();
    for (message, reason) in KEPT {
        assert!(
            !reason.trim().is_empty(),
            "`{message}` is kept with no reason"
        );
        let found = sources
            .iter()
            .any(|(_, text)| text.contains(&format!("\"{{}}{message}\"")));
        assert!(
            found,
            "KEPT names `{message}`, which nothing prints any more"
        );
    }
}

/// The scanner has to see the shape it is looking for, and not see the ones it is not — otherwise
/// it passes by finding nothing, which is how a source-scanning test usually fails.
#[test]
fn the_scanner_recognises_the_shape() {
    let found = naming_diagnostics(
        r#"
        eprintln!("{}kill: {spec}: invalid signal specification", origin_now());
        eprintln!("{}{}: command not found", env.origin(), name);
        eprintln!("{}cd: HOME not set", origin_now());
        eprintln!("{USAGE}");
        println!("{}not: {a}: an eprintln", x);
        "#,
    );
    assert_eq!(
        found,
        vec![
            "kill: {spec}: invalid signal specification".to_string(),
            "{}: command not found".to_string(),
        ],
        "a condition, a usage block and a println are not diagnostics about a value"
    );
}

/// **The scan reaches the file the first version of it missed.** `command not found` lives outside
/// both directories that version looked in, which is how it went unconverted through six commits.
#[test]
fn the_scan_reaches_outside_the_builtins() {
    let seen: Vec<String> = sources()
        .iter()
        .map(|(path, _)| path.display().to_string())
        .collect();
    for expected in [
        "exec/simple/notfound.rs",
        "env/scope/vars.rs",
        "startup/config.rs",
        "src/main.rs",
    ] {
        assert!(
            seen.iter().any(|path| path.ends_with(expected)),
            "the scan does not reach {expected}"
        );
    }
}
