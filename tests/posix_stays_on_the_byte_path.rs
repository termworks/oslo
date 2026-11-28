//! The POSIX guarantee, as a test rather than a promise.
//!
//! oslo's structured pipeline is designed so a script written before oslo existed cannot reach it:
//! structure flows only between two stages that *both* declare they understand it, and every name
//! that can carry a declaration is either invented by oslo or a builtin deliberately declared as
//! bytes. See `docs/features/structured-pipelines.md`.
//!
//! That argument is worth only as much as its enforcement. This runs every script in the
//! differential corpus — the same corpus checked against bash byte for byte — with the shell
//! reporting how many structured edges it planned, and requires the answer to be zero every time.
//!
//! When that stops being true, this fails, and whoever made it stop has to say why.

use std::path::{Path, PathBuf};
use std::process::Command;

fn oslo_binary() -> PathBuf {
    // The test binary lives beside the shell it is testing.
    let mut path = std::env::current_exe().expect("test binary path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("oslo")
}

fn corpus_scripts() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "sh"))
        .collect();
    out.sort();
    out
}

/// How many structured edges a run reported, or `None` if it did not report at all.
///
/// The report may follow a prompt left without its newline, such as `select`'s `#? `.
fn structured_edges(stderr: &str) -> Option<u64> {
    stderr
        .lines()
        .rev()
        .find_map(|line| line.rsplit_once("oslo-audit: structured-edges="))
        .and_then(|(_, n)| n.trim().parse().ok())
}

/// Whether a script hands its process over to another program, leaving no oslo to report.
fn replaces_itself(script: &Path) -> bool {
    let Ok(source) = std::fs::read_to_string(script) else {
        return false;
    };
    source
        .lines()
        .map(str::trim_start)
        .any(|line| line == "exec" || line.starts_with("exec "))
}

#[test]
fn no_corpus_script_ever_enters_the_structured_path() {
    let shell = oslo_binary();
    assert!(shell.exists(), "build the shell first: {}", shell.display());

    let scripts = corpus_scripts();
    assert!(
        scripts.len() >= 60,
        "the corpus is supposed to be substantial; found {}",
        scripts.len()
    );

    // **Each run gets a directory of its own.** These are real scripts and many create files; run
    // in the repository they leave litter behind, and one of them dropping a file called `f` is
    // enough to change what a *different* script does afterwards. That is not hypothetical — it is
    // how this test broke the differential suite the first time it was written.
    let sandbox = std::env::temp_dir().join(format!("oslo-bytepath-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&sandbox);

    let mut unreported = Vec::new();
    let mut offenders = Vec::new();
    for script in &scripts {
        let output = Command::new(&shell)
            .arg(script)
            .current_dir(&sandbox)
            .env("OSLO_AUDIT_STRUCTURED", "1")
            // The scripts are checked against bash elsewhere; here only the audit line matters, so
            // a script that fails outright is still a valid measurement of the path it took.
            .output()
            .expect("run the shell");
        let stderr = String::from_utf8_lossy(&output.stderr);
        let name = script.file_name().unwrap_or_default().to_string_lossy();
        match structured_edges(&stderr) {
            Some(0) => {}
            Some(n) => offenders.push(format!("{name}: {n} structured edges")),
            // A script that `exec`s replaces the process image, so nothing registered to run at
            // exit can run. That is the mechanism working, not a gap: there is no oslo left to
            // report. Anything else failing to report is a real hole in the measurement.
            None if replaces_itself(script) => {}
            None => unreported.push(name.to_string()),
        }
    }

    let _ = std::fs::remove_dir_all(&sandbox);

    assert!(
        unreported.is_empty(),
        "these scripts produced no audit line, so nothing was measured: {unreported:?}"
    );
    assert!(
        offenders.is_empty(),
        "POSIX scripts must never reach the structured pipeline path.\n\
         Structure is supposed to be unreachable without naming something oslo invented, so one of\n\
         these is true: a builtin was declared structured when it should be bytes, or the planner\n\
         stopped requiring a declaration on both ends.\n{offenders:#?}"
    );
}

/// **A bytes-taking tool at the head of a pipeline reads the shell's own input.**
///
/// There is no stage before it, so it used to be handed the empty string and answer for an empty
/// input: `printf 'a\nb\nc\n' | oslo -c 'lines | length'` said **0**. Not a refusal — a wrong
/// answer, silently, which is the one failure `docs/known-gaps.md` opens by saying oslo does not
/// have. `cat | lines | length` said 3, which is what made it look like a rule about pipelines.
#[test]
fn a_bytes_tool_at_the_head_reads_standard_input() {
    for (line, expected) in [
        ("lines | length", "3"),
        // Unchanged where there *is* an upstream stage.
        ("cat | lines | length", "3"),
    ] {
        let out = piped("a\nb\nc\n", line);
        assert_eq!(out.trim(), expected, "`{line}` answered wrongly");
    }
}

/// A tool that takes nothing still takes nothing — `ls` must not consume the shell's input.
///
/// Five files in the directory and three lines on standard input, so the two answers cannot be
/// confused for one another.
#[test]
fn a_tool_that_reads_nothing_leaves_standard_input_alone() {
    let dir = tempfile::tempdir().expect("tempdir");
    for n in 0..5 {
        std::fs::write(dir.path().join(format!("f{n}")), b"x").expect("write");
    }
    let out = piped_in(dir.path(), "a\nb\nc\n", "ls | length");
    assert_eq!(
        out.trim(),
        "5",
        "`ls` answered for the piped lines rather than for the directory"
    );
}

/// Run `line` with `input` on standard input, in a fresh directory.
fn piped(input: &str, line: &str) -> String {
    let dir = tempfile::tempdir().expect("tempdir");
    piped_in(dir.path(), input, line)
}

/// The same, somewhere the caller controls.
fn piped_in(dir: &Path, input: &str, line: &str) -> String {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = Command::new(oslo_binary())
        .arg("-c")
        .arg(line)
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
        .expect("write");
    let out = child.wait_with_output().expect("wait");
    String::from_utf8_lossy(&out.stdout).into_owned()
}
