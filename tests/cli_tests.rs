//! The shell's own command line (PLAN R1.10).
//!
//! Every one of these used to start a REPL: `main` recognised `-c`, `--lua-script` and a bare
//! script path, and treated everything else — `--version`, `--help`, `-Z`, even a bare `-c` with
//! no argument — as "no arguments", which read the caller's stdin and exited 0. A shell that
//! swallows its input on a typo is worse than one that refuses to start.

mod common;

use common::oslo_bin;
use std::io::Write;
use std::process::{Command, Output, Stdio};

fn oslo(args: &[&str]) -> Output {
    Command::new(oslo_bin())
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("spawn oslo")
}

fn stdout_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn status_of(out: &Output) -> i32 {
    out.status.code().unwrap_or(-1)
}

#[test]
fn version_prints_the_crate_version_and_exits_zero() {
    let out = oslo(&["--version"]);
    assert_eq!(status_of(&out), 0);
    let text = stdout_of(&out);
    assert!(text.contains("oslo"), "{text:?}");
    assert!(
        text.contains(env!("CARGO_PKG_VERSION")),
        "the version must come from Cargo, not a hardcoded string: {text:?}"
    );
}

#[test]
fn help_prints_usage_on_stdout_and_exits_zero() {
    let out = oslo(&["--help"]);
    assert_eq!(status_of(&out), 0);
    let text = stdout_of(&out);
    assert!(text.contains("USAGE"), "{text:?}");
    assert!(text.contains("-c COMMAND"), "{text:?}");
    assert!(stderr_of(&out).is_empty());
}

/// **A captured `--help` has no escape sequences in it.** The whole colour decision rests on
/// stdout not being a terminal here, and only a spawned binary can prove it — the in-process
/// tests share libtest's captured stdout and would pass even if the check were removed.
#[test]
fn a_redirected_help_is_never_coloured() {
    for args in [vec!["--help"], vec!["--help", "--details"]] {
        let text = stdout_of(&oslo(&args));
        assert!(!text.contains('\x1b'), "{args:?} was painted into a pipe");
    }
}

/// `--details` is the reference, and it is longer than the summary by more than a line.
#[test]
fn details_prints_the_option_reference() {
    let short = stdout_of(&oslo(&["--help"]));
    let long = stdout_of(&oslo(&["--help", "--details"]));
    assert!(long.contains("xtrace"), "{long:?}");
    assert!(long.contains("not implemented:"), "{long:?}");
    assert!(long.lines().count() > short.lines().count() + 10);
}

#[test]
fn an_unknown_option_prints_usage_on_stderr_and_exits_two() {
    let out = oslo(&["-Z"]);
    assert_eq!(status_of(&out), 2);
    let err = stderr_of(&out);
    assert!(err.contains("-Z"), "{err:?}");
    assert!(err.contains("usage: oslo"), "{err:?}");
    assert!(
        stdout_of(&out).is_empty(),
        "usage errors do not go to stdout"
    );
}

#[test]
fn an_unknown_long_option_exits_two() {
    let out = oslo(&["--nonesuch"]);
    assert_eq!(status_of(&out), 2);
    assert!(stderr_of(&out).contains("--nonesuch"));
}

#[test]
fn dash_c_without_an_argument_exits_two() {
    let out = oslo(&["-c"]);
    assert_eq!(status_of(&out), 2);
    assert!(
        stderr_of(&out).contains("requires an argument"),
        "{:?}",
        stderr_of(&out)
    );
}

#[test]
fn dash_c_still_runs_a_command() {
    let out = oslo(&["-c", "echo hi"]);
    assert_eq!(status_of(&out), 0);
    assert_eq!(stdout_of(&out), "hi\n");
}

#[test]
fn dash_c_takes_name_and_positional_operands() {
    let out = oslo(&["-c", "echo $0 $# $1 $2", "myname", "one", "two"]);
    assert_eq!(stdout_of(&out).trim_end(), "myname 2 one two");
}

#[test]
fn a_script_operand_gets_its_own_positionals() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("s.sh");
    std::fs::write(&script, "echo $# $1 $2\n").unwrap();

    let out = oslo(&[script.to_str().unwrap(), "alpha", "beta"]);
    assert_eq!(stdout_of(&out).trim_end(), "2 alpha beta");
    assert_eq!(status_of(&out), 0);
}

#[test]
fn a_missing_script_exits_one_hundred_and_twenty_seven() {
    let out = oslo(&["/nonexistent/script/xyz.sh"]);
    assert_eq!(status_of(&out), 127);
    assert!(!stderr_of(&out).is_empty());
}

#[test]
fn double_dash_ends_option_parsing() {
    let out = oslo(&["--", "/nonexistent/script/xyz.sh"]);
    assert_eq!(
        status_of(&out),
        127,
        "the operand after -- is a script name, not an option"
    );
}

/// A non-tty stdin means a script to run, not a session to start. With no input there is nothing
/// to run, so the shell exits 0 immediately instead of blocking on a prompt.
#[test]
fn stdin_from_dev_null_exits_zero_without_a_banner() {
    let out = oslo(&[]);
    assert_eq!(status_of(&out), 0);
    assert!(
        !stdout_of(&out).contains("POSIX Compatible Shell"),
        "the banner belongs to interactive sessions only: {:?}",
        stdout_of(&out)
    );
}

#[test]
fn a_program_piped_into_stdin_is_executed() {
    for args in [vec![], vec!["-s"]] {
        let mut child = Command::new(oslo_bin())
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn oslo");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(b"x=7\necho piped $x\n")
            .expect("write script");
        let out = child.wait_with_output().expect("wait");
        assert_eq!(stdout_of(&out), "piped 7\n", "args: {args:?}");
        assert_eq!(status_of(&out), 0);
    }
}

#[test]
fn a_syntax_error_from_stdin_exits_two() {
    let mut child = Command::new(oslo_bin())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn oslo");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"echo a; )\n")
        .expect("write script");
    let out = child.wait_with_output().expect("wait");
    assert_eq!(status_of(&out), 2);
}

/// The invocation's own flags are part of `$-`, which is how a script tells how it was started.
#[test]
fn dollar_dash_reports_the_invocation_and_the_command_line_options() {
    let out = oslo(&["-fu", "-c", "echo \"[$-]\""]);
    assert_eq!(status_of(&out), 0, "stderr: {}", stderr_of(&out));
    // `f` and `u` from the command line, `h` and `B` (hashall, braceexpand) on by default, and `c`
    // because the program came from `-c` — in `ALL` order, which is bash's: `bash -fu -c` answers
    // the same.
    assert_eq!(stdout_of(&out), "[fhuBc]\n");
}

/// An option letter `set` would refuse is refused here too, with the same usage status.
#[test]
fn an_unknown_command_line_option_is_still_rejected() {
    let out = oslo(&["-Z", "-c", "echo ran"]);
    assert_eq!(status_of(&out), 2);
    assert!(stdout_of(&out).is_empty());
}

#[test]
fn recorded_set_options_do_not_prevent_a_script_from_running() {
    // -e and -x reach the shell's option set (PLAN R6.1) and must not be mistaken for a script
    // name or an unknown option. Their *behaviour* arrives with R6.2/R6.3.
    let out = oslo(&["-ex", "-c", "echo ran"]);
    assert_eq!(status_of(&out), 0, "stderr: {}", stderr_of(&out));
    assert_eq!(stdout_of(&out), "ran\n");
}

/// `--login` beside `-l`.
///
/// The long form is what a display manager, a terminal emulator's "run as login shell" setting and
/// `su --login` reach for — so a shell that took only `-l` failed to start under exactly the things
/// that start a login shell. Found while making oslo usable with `chsh`.
#[test]
fn the_login_flag_has_both_spellings() {
    for flag in ["-l", "--login"] {
        let out = oslo(&[flag, "-c", "echo started"]);
        assert_eq!(status_of(&out), 0, "{flag}: {}", stderr_of(&out));
        assert_eq!(stdout_of(&out).trim(), "started", "{flag}");
    }
}

/// A file with no `#!` line is run by the shell itself.
///
/// `execve` answers `ENOEXEC` for it, which means "not a binary" rather than "cannot run" — POSIX
/// says to fall back to interpreting the file, and bash, dash and zsh all do. Without it
/// `./script.sh` on a shebang-less script is a dead end: the file is executable, the shell can read
/// it, and the only thing missing is two bytes nobody has needed to write since the seventies.
///
/// `$0` and the positional parameters have to survive the fallback, or a script that inspects its
/// own name sees the shell's instead.
#[test]
fn a_script_without_a_shebang_is_run_by_the_shell() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("noshebang.sh");
    std::fs::write(&script, "echo ran\necho \"zero=$0\"\necho \"args=$*\"\n").expect("write");
    let mut perms = std::fs::metadata(&script).expect("stat").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&script, perms).expect("chmod");

    let path = script.display().to_string();
    let out = oslo(&["-c", &format!("{path} alpha beta")]);
    assert_eq!(status_of(&out), 0, "stderr: {}", stderr_of(&out));
    let text = stdout_of(&out);
    assert!(text.contains("ran"), "{text:?}");
    assert!(text.contains(&format!("zero={path}")), "{text:?}");
    assert!(text.contains("args=alpha beta"), "{text:?}");
}

/// A file that is not executable is still refused — the fallback is about the *format*, not about
/// permission, and running something the user did not mark runnable would be worse than failing.
#[test]
fn an_unexecutable_file_is_still_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("notexec.sh");
    std::fs::write(&script, "echo should-not-run\n").expect("write");
    let out = oslo(&["-c", &script.display().to_string()]);
    assert_ne!(status_of(&out), 0);
    assert!(
        !stdout_of(&out).contains("should-not-run"),
        "it ran anyway: {:?}",
        stdout_of(&out)
    );
}
