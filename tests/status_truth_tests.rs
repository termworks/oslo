//! Exit-status truthfulness in the cases the differential corpus cannot arbitrate.
//!
//! Two kinds of case live here rather than in `tests/corpus`:
//!
//! * **a child killed by a signal.** Reproducing one means having the victim kill its own parent
//!   (`kill -9 $PPID`), and which process that is depends on how many times the shell forked —
//!   bash execs the last stage of a pipeline in place and would kill *itself*. The number a shell
//!   must report is not in dispute (128 + signo), so it is asserted directly.
//! * **stdin.** Every corpus case runs with stdin on `/dev/null` precisely so a dropped
//!   redirection cannot hang the suite, which is also what makes it blind to a background job
//!   stealing the terminal's input.

mod common;

use common::oslo_bin;
use std::io::Write;
use std::process::{Command, Stdio};

/// Run `script` with `input` on stdin, in `dir`.
fn run_with_stdin(dir: &std::path::Path, script: &str, input: &str) -> (String, i32) {
    let mut child = Command::new(oslo_bin())
        .arg("-c")
        .arg(script)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn oslo");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait oslo");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// The last stage of a pipeline dies on SIGKILL: the pipeline is 137, not 0.
///
/// `waitpid` used to be called once per `if let` arm, so the `Signaled` arm tested a *second*
/// call that could only fail — and a killed stage was reported as a clean exit.
#[test]
fn a_signalled_last_stage_reports_128_plus_the_signal() {
    // The stage is `sh` itself, which the stage's process became, so it kills itself.
    let r = common::run(r#"echo x | sh -c 'kill -9 $$; sleep 5'; echo "status=$?""#);
    assert_eq!(r.out(), "status=137", "stderr: {}", r.stderr);
}

/// A background job started by a non-interactive shell announces nothing, on either stream.
///
/// The notice used to go to stdout unconditionally, so every `$( )` containing a `&` captured
/// `[bg] <pid>` as part of its value.
#[test]
fn a_non_interactive_shell_prints_no_job_notice() {
    let r = common::run("sleep 0 & wait");
    assert_eq!(r.stdout, "");
    assert_eq!(r.stderr, "");

    let captured = common::run(r#"x=$(sleep 0 & echo done); echo "[$x]""#);
    assert_eq!(captured.out(), "[done]", "stderr: {}", captured.stderr);
}

/// A background job does not read the input the shell was given.
///
/// Without job control there is no separate process group keeping the job off the terminal, so
/// POSIX has the shell give it `/dev/null` instead. Before that, `cmd &` and the shell raced for
/// every keystroke and the job usually won.
#[test]
fn a_background_job_does_not_steal_stdin() {
    let dir = tempfile::tempdir().expect("tempdir");
    // `wait` makes this deterministic: `cat` has finished before the shell reads anything, so if
    // it had the shell's stdin it would have drained the lot.
    let script = r#"cat > drained & wait
read a
read b
echo "[$a][$b]"
echo "drained=[$(cat drained)]""#;
    let (stdout, status) = run_with_stdin(dir.path(), script, "hello\nworld\n");
    assert_eq!(status, 0);
    assert_eq!(stdout, "[hello][world]\ndrained=[]\n");
}

/// **What a fatal error exits with, in both ways a shell can be started.**
///
/// Here rather than in the corpus because the corpus runs every case through `-c`, and `-c` is
/// exactly the form that hides this: bash answers 127 from `-c` for three narrow failures and 1
/// (or 2) for the same program in a file, so a suite that only ever says `-c` cannot see a shell
/// that answers 127 to everything. oslo did.
///
/// 127 is not a spare number. It means "command not found", and a Makefile or a CI step reading it
/// off a script that died on `$((1/0))` is told the shell could not find its command when the
/// shell found it and could not expand its arguments.
#[test]
fn a_fatal_error_exits_the_way_bash_exits() {
    common::assert_oracle_is_bash();
    let dir = tempfile::tempdir().expect("temp dir");
    let script = dir.path().join("case.sh");

    // Every fatal error this shell can raise, and `exit`/not-found as controls.
    for program in [
        "echo $((1/0))",
        "set -u; echo ${nope}",
        "unset x; echo ${x:?}",
        "set -u; a=(1); echo ${a[9]}",
        "echo ${!x}",
        "echo ${x!!}",
        "echo ${1x}",
        "v=\"a b\"; echo ${!v}",
        "readonly r=1; r=2",
        "echo $(if)",
        "if",
        "set -o posix; readonly r=1; r=2",
        "definitely-no-such-command-zz",
        "exit 127",
        "echo fine",
    ] {
        std::fs::write(&script, format!("{program}\n")).expect("write the case");
        // **All four combinations**, because bash answers differently in each and oslo answered
        // 127 in all of them. `--posix` escalates a malformed expansion and nothing else; a script
        // file never escalates at all.
        for form in ["-c", "file"] {
            for posix in [false, true] {
                let status = |program_path: &str| -> i32 {
                    let mut command = Command::new(program_path);
                    if posix {
                        command.arg("--posix");
                    }
                    match form {
                        "-c" => command.arg("-c").arg(program),
                        _ => command.arg(&script),
                    };
                    command
                        .current_dir(dir.path())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status()
                        .expect("spawn")
                        .code()
                        .unwrap_or(-1)
                };
                assert_eq!(
                    status(&oslo_bin().to_string_lossy()),
                    status("bash"),
                    "`{program}` as {form}{}",
                    if posix { " --posix" } else { "" }
                );
            }
        }
    }
}
