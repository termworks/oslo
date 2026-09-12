//! **A script, a pipe and a test see exactly what they saw before ariadne existed.**
//!
//! This is the safety net for the whole diagnostics change, and it was written before a single
//! site was converted. oslo is POSIX-first, and POSIX says what a shell writes to standard error:
//! a multi-line coloured report there would break `2>&1 | grep`, break every conformance suite,
//! and break scripts written before oslo existed.
//!
//! So the report is the *drawn face* of an error and the one-liner is its transport — the same
//! split `render_display` and `render_transport` are two functions for — and the thing that
//! decides between them is `isatty(2)` on stderr.
//!
//! Every runner here goes through `oslo -c`, whose stderr is a pipe. **Every string below is what
//! the shell printed before the change.** A converted site that alters one of them has broken the
//! rule, whatever it looks like on a terminal.
//!
//! # Why the strings are written out
//!
//! Not computed, not matched against a pattern. A test that asserted "stderr mentions the operand"
//! would pass on a report as happily as on a one-liner, which is exactly the regression it exists
//! to catch. The point is the bytes.

mod common;

use common::oslo_bin;
use std::process::{Command, Stdio};

/// One script, and the whole of the first line of stderr it produces.
///
/// The first line rather than all of stderr, because a few of these also print a usage block, and
/// this is a test about the *diagnostic*.
struct Plain {
    script: &'static str,
    stderr: &'static str,
}

/// The sites this change converts, and what they say to a pipe.
///
/// Grows a row per converted site. A row here is the contract: whatever the terminal gets, this is
/// what everything else gets.
const PLAIN: &[Plain] = &[
    // Phase 0 — the two worked examples.
    Plain {
        script: "kill -s NOPE 1",
        stderr: "oslo: kill: NOPE: invalid signal specification",
    },
    Plain {
        script: "df | cols nmae",
        stderr: "oslo: cols: nmae: no such column",
    },
    // Not converted, and here anyway: a diagnostic with nothing to point at must keep its
    // one-liner on a terminal too, so this row guards the decision as much as the output.
    Plain {
        script: "cd /nope",
        stderr: "oslo: cd: /nope: No such file or directory",
    },
    // The rest of the phase-1 population, pinned before it is touched.
    Plain {
        script: "kill %99",
        stderr: "oslo: kill: %99: no such job",
    },
    Plain {
        script: "printf '%d' abc",
        stderr: "oslo: printf: abc: invalid number",
    },
    Plain {
        script: "read -z x",
        stderr: "oslo: read: -z: invalid option",
    },
    Plain {
        script: "ulimit -Z",
        stderr: "oslo: ulimit: -Z: invalid option",
    },
    // Phase 1 — the builtins. One row per converted site; a row here is the contract.
    //
    // `options::invalid` serves six of these from one function, which is why `export`, `unset`,
    // `readonly`, `alias` and `unalias` all appear: converting the helper converted them, and a
    // row each is what says so.
    Plain {
        script: "export -z",
        stderr: "oslo: export: -z: invalid option",
    },
    Plain {
        script: "unset -z",
        stderr: "oslo: unset: -z: invalid option",
    },
    Plain {
        script: "readonly -z",
        stderr: "oslo: readonly: -z: invalid option",
    },
    Plain {
        script: "alias -z",
        stderr: "oslo: alias: -z: invalid option",
    },
    Plain {
        script: "unalias -z",
        stderr: "oslo: unalias: -z: invalid option",
    },
    Plain {
        script: "shopt -Z",
        stderr: "oslo: shopt: -Z: invalid option",
    },
    Plain {
        script: "hash -Z",
        stderr: "oslo: hash: -Z: invalid option",
    },
    Plain {
        script: "jobs -Z",
        stderr: "oslo: jobs: -Z: invalid option",
    },
    Plain {
        script: "umask -Z",
        stderr: "oslo: umask: -Z: invalid option",
    },
    Plain {
        script: "ulimit -Z",
        stderr: "oslo: ulimit: -Z: invalid option",
    },
    Plain {
        script: "command -Z",
        stderr: "oslo: command: -Z: invalid option",
    },
    Plain {
        script: "suspend -x",
        stderr: "oslo: suspend: -x: invalid option",
    },
    Plain {
        script: "times -p",
        stderr: "oslo: times: -p: invalid option",
    },
    Plain {
        script: "wait -z",
        stderr: "oslo: wait: -z: invalid option",
    },
    Plain {
        script: "mark -z",
        stderr: "oslo: mark: -z: not an option",
    },
    Plain {
        script: "builtin nosuch",
        stderr: "oslo: builtin: nosuch: not a shell builtin",
    },
    Plain {
        script: "chain nope",
        stderr: "oslo: chain: nope: unknown argument",
    },
    Plain {
        script: "ui nosuch",
        stderr: "oslo: ui: nosuch: not a widget",
    },
    Plain {
        script: "ui input --nope",
        stderr: "oslo: ui input: --nope: unknown option",
    },
    Plain {
        script: "while true; do break 0; done",
        stderr: "oslo: break: 0: loop count out of range",
    },
    Plain {
        script: "alias 'a b'=x",
        stderr: "oslo: alias: `a b': invalid alias name",
    },
    Plain {
        script: "shopt -s nosuch",
        stderr: "oslo: shopt: nosuch: invalid shell option name",
    },
    Plain {
        script: "shopt -o nosuch",
        stderr: "oslo: shopt: nosuch: invalid option name",
    },
    Plain {
        script: "unalias nosuch",
        stderr: "oslo: unalias: nosuch: not found",
    },
    Plain {
        script: "hash nosuch",
        stderr: "oslo: hash: nosuch: not found",
    },
    Plain {
        script: "disown -Z",
        stderr: "oslo: disown: -Z: invalid option",
    },
    Plain {
        script: "nav --nope",
        stderr: "oslo: nav: --nope: unknown option",
    },
    Plain {
        script: "kill -TERM %99",
        stderr: "oslo: kill: %99: no such job",
    },
    Plain {
        script: "export 2FOO=x",
        stderr: "oslo: export: `2FOO=x': not a valid identifier",
    },
    Plain {
        script: "unset a-b",
        stderr: "oslo: unset: `a-b': not a valid identifier",
    },
    Plain {
        script: "printf -v 2bad %s x",
        stderr: "oslo: printf: `2bad': not a valid identifier",
    },
    Plain {
        script: "read -t x y",
        stderr: "oslo: read: -t: x: invalid timeout specification",
    },
    Plain {
        script: "read -n",
        stderr: "oslo: read: -n: option requires an argument",
    },
    // Phase 2 — the structured verbs. `too_many`, `count_operand`, `sort_operands`,
    // `unknown_column` and the plan-time refusal are shared helpers, so a row here stands for
    // every verb that reaches one of them.
    Plain {
        script: "df | length extra",
        stderr: "oslo: length: extra: too many arguments",
    },
    Plain {
        script: "df | first many",
        stderr: "oslo: first: many: a count is a whole number",
    },
    Plain {
        script: "df | sort-by -Z size",
        stderr: "oslo: sort-by: -Z: not an option; sort-by knows -r, -n and -i",
    },
    Plain {
        script: "df | where 'size >'",
        stderr: "oslo: where: size >: the expression is not finished",
    },
    Plain {
        script: "df | insert size 1",
        stderr: "oslo: insert: size: already a column; use update to replace it, or upsert for either",
    },
    // Phase 3 — a syntax error, which is the only report that points into the program itself.
    Plain {
        script: "echo \"unterminated",
        stderr: "oslo: syntax error: this `\"` was never closed",
    },
    Plain {
        script: "if true; then",
        stderr: "oslo: syntax error: this `if` was never closed",
    },
    // Phase 5 — the sweep. `tests/diagnostics_point_at_the_word.rs` found these by scanning the
    // source for the shape rather than by anybody remembering they were there.
    Plain {
        script: "cd -Z",
        stderr: "oslo: cd: -Z: invalid option",
    },
    Plain {
        script: "pwd -Z",
        stderr: "oslo: pwd: -Z: invalid option",
    },
    Plain {
        script: "trap -Z",
        stderr: "oslo: trap: -Z: invalid option",
    },
    Plain {
        script: "trap 'echo' NOPE",
        stderr: "oslo: trap: NOPE: invalid signal specification",
    },
    Plain {
        script: "type nosuchcmd",
        stderr: "oslo: type: nosuchcmd: not found",
    },
    Plain {
        script: "pushd +99",
        stderr: "oslo: pushd: +99: directory stack index out of range",
    },
    Plain {
        script: "df | from nope",
        stderr: "oslo: from: nope: unknown format; oslo knows json, csv and tsv",
    },
    Plain {
        script: "df | detect-columns --nope",
        stderr: "oslo: detect-columns: --nope: not an option; it knows --no-headers and --skip",
    },
    // And the ordinary answers, because `bridges.rs` was split out of the dispatch while these
    // were being converted and took the `df` arm with it — for one build `df | length` answered
    // `first: a structured verb, not a command`. Every row above is a *failure*; these two are what
    // says the pipeline still works at all.
    Plain {
        script: "df | length",
        stderr: "",
    },
    Plain {
        script: "seq 1 3 | lines | length",
        stderr: "",
    },
    // The widened sweep — the whole workspace, not two directories. `command not found` is first
    // because it is the commonest diagnostic the shell prints and the narrow sweep never saw it.
    Plain {
        script: "nosuchprogram",
        stderr: "oslo: nosuchprogram: command not found",
    },
    Plain {
        script: "command nosuchxyz",
        stderr: "oslo: nosuchxyz: command not found",
    },
    Plain {
        script: "declare -p NOPE",
        stderr: "oslo: declare: NOPE: not found",
    },
    Plain {
        script: "declare -A x",
        stderr: "oslo: declare: -A: associative arrays are not supported",
    },
    // A name the shell was *given* rather than one written in the script. It reaches stderr, and
    // an escape sequence in it is not text a terminal shows but an instruction it obeys — so what
    // arrives is the spelling, `$'\E…'`, and never the bytes. See `oslo_base::shown`.
    //
    // Every surface, because they are separate call sites and converting one proves nothing about
    // the next: the bug was that all of them wrote the name straight through.
    Plain {
        script: "e=$(printf '\\033[2Jx'); cd \"$e\"",
        stderr: "oslo: cd: $'\\E[2Jx': No such file or directory",
    },
    Plain {
        script: "e=$(printf '\\033[2Jx'); . \"$e\"",
        stderr: "oslo: source: $'\\E[2Jx': No such file or directory",
    },
    Plain {
        script: "e=$(printf '\\033[2Jx'); $e",
        stderr: "oslo: $'\\E[2Jx': command not found",
    },
    Plain {
        script: "e=$(printf '\\033[2Jx'); exec \"$e\"",
        stderr: "oslo: exec: $'\\E[2Jx': not found",
    },
    Plain {
        script: "e=$(printf '\\033[2Jx'); cat < \"$e\"",
        stderr: "oslo: $'\\E[2Jx': No such file or directory",
    },
    Plain {
        script: "e=$(printf '\\033[2Jx'); rm \"$e\"",
        stderr: "oslo: rm: cannot remove $'\\E[2Jx': No such file or directory",
    },
    Plain {
        script: "e=$(printf '\\033[2Jx'); unset \"$e\"",
        stderr: "oslo: unset: `$'\\E[2Jx'': not a valid identifier",
    },
    // An `OSC 8` payload, which is the half that survives being invisible: under a terminal that
    // speaks it, this put an arbitrary hyperlink inside oslo's own error text.
    Plain {
        script: "e=$(printf '\\033]8;;http://evil\\033\\\\click'); cd \"$e\"",
        stderr: "oslo: cd: $'\\E]8;;http://evil\\E\\\\click': No such file or directory",
    },
    // **And an ordinary name is not quoted for it.** A rule that fired on anything would make every
    // diagnostic in the shell unreadable to protect against a name that almost never appears; a
    // name that is merely non-ASCII is an ordinary name.
    Plain {
        script: "cd héllo-☃",
        stderr: "oslo: cd: héllo-☃: No such file or directory",
    },
    Plain {
        script: "rm 'a b'",
        stderr: "oslo: rm: cannot remove 'a b': No such file or directory",
    },
    Plain {
        script: "declare 2bad",
        stderr: "oslo: declare: `2bad': not a valid identifier",
    },
    Plain {
        script: "break 1x",
        stderr: "oslo: break: 1x: numeric argument required",
    },
    Plain {
        script: "pushd +99 /tmp",
        stderr: "oslo: pushd: too many arguments",
    },
    Plain {
        script: "disown -Z",
        stderr: "oslo: disown: -Z: invalid option",
    },
    Plain {
        script: "wait notapid",
        stderr: "oslo: wait: `notapid': not a pid or valid job spec",
    },
    Plain {
        script: "kill notapid",
        stderr: "oslo: kill: `notapid': not a pid or valid job spec",
    },
    Plain {
        script: "printf '%'",
        stderr: "oslo: printf: `%': missing format character",
    },
    Plain {
        script: "type -Z x",
        stderr: "oslo: type: -Z: invalid option",
    },
    Plain {
        script: "readonly R=1; R=2",
        stderr: "oslo: R: is read only",
    },
    Plain {
        script: "readonly R=1; export R=2",
        stderr: "oslo: R: is read only",
    },
    Plain {
        script: "export -f nosuchfn",
        stderr: "oslo: export: nosuchfn: not a function",
    },
    Plain {
        script: "jobs %99",
        stderr: "oslo: jobs: %99: no such job",
    },
    Plain {
        script: "ulimit -n abc",
        stderr: "oslo: ulimit: abc: invalid number",
    },
    Plain {
        script: "unalias -z",
        stderr: "oslo: unalias: -z: invalid option",
    },
    #[cfg(feature = "universal")]
    Plain {
        script: "set -U 'bad name' x",
        stderr: "oslo: set -U: bad name: not a valid name",
    },
    // The structured verbs, refusing for want of an operand. Nothing is *wrong* here — something
    // is missing — so the caret goes on the verb, which is oslo's answer everywhere else a word is
    // absent rather than mistaken.
    Plain {
        script: "df | cols",
        stderr: "oslo: cols: name at least one column",
    },
    Plain {
        script: "df | sort-by",
        stderr: "oslo: sort-by: a column name is required",
    },
    Plain {
        script: "ps | each",
        stderr: "oslo: each: an expression is required",
    },
    Plain {
        script: "ps | to",
        stderr: "oslo: to: a format is required, as in `to json`",
    },
    Plain {
        script: "ps | append --keep x",
        stderr: "oslo: append: --keep is a lookup option",
    },
    // The scalar verbs. Behind the `text` feature, so these rows only mean anything in a build
    // that has it — which `make test` is, and a minimal build answers `command not found` here and
    // is checked by `structured_names_are_oslos_own` instead.
    #[cfg(feature = "text")]
    Plain {
        script: "text nope x",
        stderr: "oslo: text: nope: not a subcommand",
    },
    #[cfg(feature = "text")]
    Plain {
        script: "text upper --bad x",
        stderr: "oslo: text upper: --bad: not an option",
    },
    #[cfg(feature = "text")]
    Plain {
        script: "text sub --start 0 abc",
        stderr: "oslo: text sub: --start counts from 1; 0 names nothing",
    },
    #[cfg(feature = "text")]
    Plain {
        script: "text pad x",
        stderr: "oslo: text pad: --width is required",
    },
    #[cfg(feature = "text")]
    Plain {
        script: "path nope x",
        stderr: "oslo: path: nope: not a subcommand",
    },
    #[cfg(feature = "text")]
    Plain {
        script: "path sort --key size a",
        stderr: "oslo: path sort: --key: size: not basename or dirname",
    },
];

/// stderr of `oslo -c script`, with `OSLO_DIAG` forced to `mode` when one is given.
fn stderr_of(script: &str, mode: Option<&str>) -> String {
    let mut command = Command::new(oslo_bin());
    command
        .arg("-c")
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    if let Some(mode) = mode {
        command.env("OSLO_DIAG", mode);
    }
    let output = command.output().expect("spawn oslo");
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The rule, stated as plainly as it can be.
#[test]
fn a_pipe_sees_the_one_line_message() {
    for case in PLAIN {
        let stderr = stderr_of(case.script, None);
        let first = stderr.lines().next().unwrap_or("");
        assert_eq!(
            first, case.stderr,
            "`{}` changed what a pipe sees",
            case.script
        );
    }
}

/// Not one byte of a report may reach a pipe: no caret, no box, no colour, no `help:` line.
///
/// Checked separately from the string above because a report drawn *after* the one-liner would
/// leave the first line intact and still break every script that reads stderr.
#[test]
fn a_pipe_sees_no_report_at_all() {
    for case in PLAIN {
        let stderr = stderr_of(case.script, None);
        for mark in ['\u{1b}', '│', '─', '╭', '┌', '^'] {
            assert!(
                !stderr.contains(mark),
                "`{}` put {mark:?} on a pipe: {stderr:?}",
                case.script
            );
        }
        assert!(
            !stderr.contains("help:"),
            "`{}` put a help line on a pipe: {stderr:?}",
            case.script
        );
    }
}

/// **`OSLO_DIAG=never` is the escape hatch, and it has to work on a terminal too.**
///
/// A pipe already gets the plain form, so this can only be tested here for what it does *not*
/// change — but the row exists so the variable is wired from the first commit rather than added
/// once something needs turning off.
#[test]
fn never_is_the_same_as_a_pipe() {
    for case in PLAIN {
        assert_eq!(
            stderr_of(case.script, Some("never")),
            stderr_of(case.script, None),
            "`{}` under OSLO_DIAG=never",
            case.script
        );
    }
}

/// The exit status is a separate promise from the message, and just as easy to break: a site that
/// returns early to draw a report must still return the status it returned before.
#[test]
fn the_status_is_unchanged() {
    for (script, want) in [
        ("kill -s NOPE 1", 1),
        ("df | cols nmae", 2),
        ("cd /nope", 1),
        ("read -z x", 2),
    ] {
        let status = Command::new(oslo_bin())
            .arg("-c")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("spawn oslo")
            .code()
            .unwrap_or(-1);
        assert_eq!(status, want, "`{script}`");
    }
}
