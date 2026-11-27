//! Differential suite: every corpus script, run through oslo and through bash.
//!
//! The audit that produced PLAN.md found one dominant failure mode — a plausible *wrong answer*
//! with exit status 0. No self-referential test can see that class of bug, because the expected
//! output has to come from somewhere other than the implementation under test. So it comes from
//! bash, which is the specification every `/bin/sh` script was actually written against.
//!
//! What is compared, and why only this much:
//!
//! * **stdout** — byte for byte, after the scratch directory is rewritten to `<TMP>` so the two
//!   shells' private working directories do not show up as a difference.
//! * **exit status** — exactly, with a signal death recorded as `128 + signo` the way a shell
//!   reports one.
//! * **stderr shape** — empty versus non-empty, nothing more. Two different shells will never
//!   agree on diagnostic wording and should not be forced to; what matters is that a shell that
//!   should complain does, and a shell that should stay quiet does.
//!
//! Every case runs with stdin on `/dev/null`, in its own scratch directory, under a wall-clock
//! timeout. oslo has live hangs (`while read` never terminates), and an unguarded suite would
//! wedge CI instead of failing it — so a timeout is a first-class verdict here, not an accident.
//!
//! Corpus scripts declare their oracle on the first line: `# mode: posix` runs `--posix -c` for
//! POSIX semantics, `# mode: bash` runs a plain `-c` for the bash extensions (arrays, `[[ ]]`,
//! `(( ))`, brace expansion) that oslo also aims to support. The mode goes to **both** shells —
//! see [`compare`] for what it cost to get that wrong.

mod common;

#[path = "differential/expected_fail.rs"]
mod expected_fail;

use expected_fail::{EXPECTED_FAIL, KNOWN_DIVERGENT};

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Wall-clock budget per shell invocation. Generous: it exists to convert a hang into a failure,
/// not to measure performance.
fn timeout() -> Duration {
    let secs = std::env::var("OSLO_DIFF_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    Duration::from_secs(secs)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Oracle {
    Posix,
    Bash,
}

struct Case {
    name: String,
    oracle: Oracle,
    script: String,
    /// Lowest bash `(major, minor)` that arbitrates this case, from a `# needs-bash: 5.3` header.
    ///
    /// Bash is a moving specification, not a fixed one: four of its behaviours changed between
    /// 5.2 and 5.3, and oslo follows the newer answer. Running those cases against an older oracle
    /// compares oslo to a bash that has since been corrected, so the case is skipped and counted
    /// rather than reported as a oslo defect. This is deliberately *not* a third escape hatch
    /// alongside `EXPECTED_FAIL` and `KNOWN_DIVERGENT` — it says "this runner's oracle is too old
    /// to answer", which is a fact about the machine, and the count is printed so a CI image that
    /// silently ages cannot quietly stop testing things.
    needs_bash: Option<(u32, u32)>,
}

/// Reads a `# needs-bash: 5.3` line out of a script's leading comment block.
fn parse_needs_bash(name: &str, script: &str) -> Option<(u32, u32)> {
    let raw = script
        .lines()
        .take_while(|l| l.starts_with('#') || l.trim().is_empty())
        .find_map(|l| l.trim().strip_prefix("# needs-bash:"))?
        .trim();
    let (major, minor) = raw.split_once('.').unwrap_or_else(|| {
        panic!("{name}: `# needs-bash:` wants a major.minor version, found {raw:?}")
    });
    let parse = |s: &str, which| {
        s.trim()
            .parse()
            .unwrap_or_else(|_| panic!("{name}: `# needs-bash:` {which} is not a number: {raw:?}"))
    };
    Some((parse(major, "major"), parse(minor, "minor")))
}

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus")
}

fn load_corpus() -> Vec<Case> {
    let dir = corpus_dir();
    let mut cases: Vec<Case> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "sh"))
        .map(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            let script = fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {name}: {e}"));
            let oracle = match script.lines().next().unwrap_or_default().trim() {
                "# mode: posix" => Oracle::Posix,
                "# mode: bash" => Oracle::Bash,
                other => panic!(
                    "{name}: first line must be `# mode: posix` or `# mode: bash`, found {other:?}"
                ),
            };
            let needs_bash = parse_needs_bash(&name, &script);
            Case {
                name,
                oracle,
                script,
                needs_bash,
            }
        })
        .collect();
    cases.sort_by(|a, b| a.name.cmp(&b.name));
    cases
}

struct Outcome {
    stdout: String,
    stderr_empty: bool,
    status: i32,
    timed_out: bool,
}

/// Run one shell on one script in a private scratch directory.
///
/// Output goes to files rather than pipes: a script that writes more than a pipe buffer would
/// otherwise block on a reader this function does not have, and look like a hang in the shell.
/// The capture files live *beside* the working directory, not in it, so `echo *` sees only what
/// the script itself created.
fn execute(program: &Path, args: &[&str], script: &str) -> io::Result<Outcome> {
    let root = tempfile::tempdir()?;
    let cwd = root.path().join("cwd");
    fs::create_dir(&cwd)?;
    let out_path = root.path().join("stdout");
    let err_path = root.path().join("stderr");

    let mut cmd = Command::new(program);
    cmd.args(args)
        .arg(script)
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(fs::File::create(&out_path)?)
        .stderr(fs::File::create(&err_path)?)
        // A user's $ENV or $BASH_ENV would be sourced by bash and ignored by oslo, which is a
        // difference in the harness rather than in the shells.
        .env_remove("ENV")
        .env_remove("BASH_ENV")
        .env("LC_ALL", "C");

    let mut child = cmd.spawn()?;
    let deadline = Instant::now() + timeout();
    let (status, timed_out) = loop {
        match child.try_wait()? {
            Some(status) => break (status, false),
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                break (child.wait()?, true);
            }
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    };

    let stdout = String::from_utf8_lossy(&fs::read(&out_path)?).into_owned();
    let stderr_empty = fs::metadata(&err_path)?.len() == 0;

    // Both shells get a different scratch directory, so the path itself must not be a difference.
    let mut stdout = stdout.replace(&cwd.to_string_lossy().into_owned(), "<TMP>");
    if let Ok(real) = cwd.canonicalize() {
        stdout = stdout.replace(&real.to_string_lossy().into_owned(), "<TMP>");
    }

    Ok(Outcome {
        stdout,
        stderr_empty,
        status: status
            .code()
            .unwrap_or_else(|| 128 + status.signal().unwrap_or(0)),
        timed_out,
    })
}

enum Verdict {
    Match,
    /// oslo never terminated. Always a defect, never an acceptable difference.
    Hung,
    Differ(String),
}

/// The argv prefix a case's declared mode asks for — the *same* one for both shells.
///
/// It is a function rather than two literals at the call site because that is the shape the bug
/// took: until Round 11 oslo was always given a bare `-c` while bash got `--posix` for a
/// `# mode: posix` case, so all 304 of those cases were judged against an oracle oslo was never
/// in. A POSIX-only behaviour could then neither be tested nor regress here, and a case that
/// passed only because oslo had stayed in bash mode looked green.
fn mode_args(oracle: Oracle) -> &'static [&'static str] {
    match oracle {
        Oracle::Posix => &["--posix", "-c"],
        Oracle::Bash => &["-c"],
    }
}

fn compare(case: &Case) -> Verdict {
    let args = mode_args(case.oracle);
    let oslo = execute(&common::oslo_bin(), args, &case.script).expect("spawn oslo");
    let bash = execute(Path::new("bash"), args, &case.script)
        .expect("spawn bash — the differential suite needs bash on PATH as its oracle");

    if bash.timed_out {
        return Verdict::Differ(format!(
            "the oracle itself timed out; {} is not a usable corpus case",
            case.name
        ));
    }
    if oslo.timed_out {
        return Verdict::Hung;
    }

    let mut report = String::new();
    if oslo.stdout != bash.stdout {
        report.push_str("  stdout:\n");
        report.push_str(&diff_lines(&bash.stdout, &oslo.stdout));
    }
    if oslo.status != bash.status {
        report.push_str(&format!(
            "  status: bash {} vs oslo {}\n",
            bash.status, oslo.status
        ));
    }
    if oslo.stderr_empty != bash.stderr_empty {
        report.push_str(&format!(
            "  stderr: bash {} vs oslo {}\n",
            shape(bash.stderr_empty),
            shape(oslo.stderr_empty)
        ));
    }

    if report.is_empty() {
        Verdict::Match
    } else {
        Verdict::Differ(report)
    }
}

fn shape(empty: bool) -> &'static str {
    if empty { "empty" } else { "non-empty" }
}

/// First few differing lines, aligned by position. Enough to identify the defect in a CI log
/// without turning the failure report into the corpus itself.
fn diff_lines(expected: &str, actual: &str) -> String {
    let exp: Vec<&str> = expected.lines().collect();
    let act: Vec<&str> = actual.lines().collect();
    let mut out = String::new();
    let mut shown = 0;
    for i in 0..exp.len().max(act.len()) {
        let (e, a) = (exp.get(i), act.get(i));
        if e == a {
            continue;
        }
        if shown == 6 {
            out.push_str("    …\n");
            break;
        }
        out.push_str(&format!(
            "    line {}: bash {:?} vs oslo {:?}\n",
            i + 1,
            e.unwrap_or(&"<no line>"),
            a.unwrap_or(&"<no line>")
        ));
        shown += 1;
    }
    out
}

/// Run every case, spreading them over a few threads. Each case is its own process pair in its
/// own directory, so there is nothing to serialise.
fn run_all(cases: &[Case]) -> Vec<(String, Verdict)> {
    const THREADS: usize = 8;
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..THREADS)
            .map(|slot| {
                scope.spawn(move || {
                    cases
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| i % THREADS == slot)
                        .map(|(_, case)| (case.name.clone(), compare(case)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("worker thread"))
            .collect()
    })
}

/// The oracle's `(major, minor)`, asserting it is new enough — and that it is bash at all.
///
/// The corpus uses constructs bash grew in 4.x (`${v^^}`, `&>`), so an ancient oracle would
/// report differences that say nothing about oslo — so fail with the reason rather than with 40
/// mystery divergences. The minor version matters too: it decides which cases carry a
/// `# needs-bash:` line this runner cannot honour.
///
/// The oracle is also checked to *be* bash — see `common::assert_oracle_is_bash`.
fn oracle_version() -> (u32, u32) {
    common::assert_oracle_is_bash();
    let out = Command::new("bash")
        .args(["-c", "echo ${BASH_VERSINFO[0]}.${BASH_VERSINFO[1]}"])
        .output()
        .expect("bash must be on PATH: it is this suite's oracle");
    let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    let version = text
        .split_once('.')
        .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)));
    let (major, minor) = version.unwrap_or_else(|| {
        panic!("could not read the oracle's version from `bash --version` output {text:?}")
    });
    assert!(
        major >= 4,
        "the oracle must be bash 4 or newer (found {major}.{minor}); \
         install a current bash and put it ahead of the old one on PATH"
    );
    (major, minor)
}

#[test]
fn corpus_matches_bash() {
    let oracle = oracle_version();

    let cases = load_corpus();
    assert!(
        cases.len() >= 60,
        "the corpus is supposed to be substantial; found only {}",
        cases.len()
    );

    let expected: BTreeMap<&str, (&str, &str)> = EXPECTED_FAIL
        .iter()
        .map(|(file, id, why)| (*file, (*id, *why)))
        .collect();
    let divergent: BTreeSet<&str> = KNOWN_DIVERGENT.iter().map(|(file, _)| *file).collect();

    let mut too_new_for_oracle: Vec<String> = Vec::new();
    let cases: Vec<Case> = cases
        .into_iter()
        .filter(|c| !divergent.contains(c.name.as_str()))
        .filter(|c| match c.needs_bash {
            Some(want) if want > oracle => {
                too_new_for_oracle.push(format!("{} (needs bash {}.{})", c.name, want.0, want.1));
                false
            }
            _ => true,
        })
        .collect();

    let mut unexpected_failures = Vec::new();
    let mut unexpected_passes = Vec::new();
    let mut matched = 0;
    let mut still_failing = 0;

    let mut results = run_all(&cases);
    results.sort_by(|a, b| a.0.cmp(&b.0));

    for (name, verdict) in results {
        let listed = expected.get(name.as_str());
        match (verdict, listed) {
            (Verdict::Match, None) => matched += 1,
            (Verdict::Match, Some((id, why))) => {
                unexpected_passes.push(format!(
                    "  {name} — listed as {id} (\"{why}\") but now matches bash: delete that line \
                     from tests/differential/expected_fail.rs"
                ));
            }
            (Verdict::Hung, Some(_)) | (Verdict::Differ(_), Some(_)) => still_failing += 1,
            (Verdict::Hung, None) => unexpected_failures
                .push(format!("  {name}\n    oslo never terminated (timed out)\n")),
            (Verdict::Differ(detail), None) => {
                unexpected_failures.push(format!("  {name}\n{detail}"))
            }
        }
    }

    let mut report = String::new();
    if !unexpected_failures.is_empty() {
        report.push_str(&format!(
            "\n{} corpus case(s) diverge from bash and are not in EXPECTED_FAIL:\n\n{}",
            unexpected_failures.len(),
            unexpected_failures.join("\n")
        ));
    }
    if !unexpected_passes.is_empty() {
        report.push_str(&format!(
            "\n{} corpus case(s) pass while still listed as expected failures:\n\n{}\n",
            unexpected_passes.len(),
            unexpected_passes.join("\n")
        ));
    }
    // Printed on success as well as failure, and always naming the oracle. A runner image whose
    // bash falls behind stops exercising cases without any test turning red, so the only defence
    // is that the number is visible in every run.
    let skipped = if too_new_for_oracle.is_empty() {
        String::new()
    } else {
        format!(
            "\n{} case(s) skipped — the oracle is bash {}.{}:\n  {}",
            too_new_for_oracle.len(),
            oracle.0,
            oracle.1,
            too_new_for_oracle.join("\n  ")
        )
    };

    assert!(
        report.is_empty(),
        "{report}\n(oracle bash {}.{}: {matched} matching, {still_failing} known-failing, \
         {} known-divergent){skipped}\n",
        oracle.0,
        oracle.1,
        divergent.len()
    );

    eprintln!(
        "differential corpus vs bash {}.{}: {matched} matching, {still_failing} known-failing, \
         {} known-divergent{skipped}",
        oracle.0,
        oracle.1,
        divergent.len()
    );
}

/// The mode flag has to *reach* oslo, not merely sit on its command line.
///
/// [`mode_args`] sends the same argv to both shells, but a `--posix` that oslo parsed and then
/// dropped would look identical from here — which is what the suite could not tell apart before
/// Round 11. So assert the observable difference directly: under POSIX a failed variable
/// assignment ends the shell, and outside it the shell abandons that line and carries on with the
/// next. Without this, a regression that made `--posix` inert would show up only as
/// `# mode: posix` cases quietly agreeing with a bash-mode oslo.
#[test]
fn the_posix_flag_changes_what_oslo_does() {
    const SCRIPT: &str = "readonly r=1\nr=2\necho alive";

    let posix = execute(&common::oslo_bin(), mode_args(Oracle::Posix), SCRIPT).expect("spawn oslo");
    assert_eq!(
        posix.stdout, "",
        "under --posix a refused assignment must end the shell before `echo alive`"
    );
    assert!(!posix.stderr_empty, "the refusal must still be reported");

    let bash_mode = execute(&common::oslo_bin(), mode_args(Oracle::Bash), SCRIPT).expect("spawn");
    assert_eq!(
        bash_mode.stdout, "alive\n",
        "without --posix the same refusal abandons only its own line"
    );
}

/// The lists are only a ratchet if they cannot drift away from the corpus they name.
#[test]
fn failure_lists_name_real_corpus_files() {
    let present: BTreeSet<String> = load_corpus().into_iter().map(|c| c.name).collect();

    let mut seen = BTreeSet::new();
    for (file, id, _) in EXPECTED_FAIL {
        assert!(
            present.contains(*file),
            "EXPECTED_FAIL names {file} ({id}), which is not in tests/corpus"
        );
        assert!(seen.insert(*file), "EXPECTED_FAIL lists {file} twice");
        assert!(
            id.starts_with('R') || *id == "UNFILED" || *id == "BRUSH",
            "{file}: {id} is not a PLAN.md finding ID, UNFILED, or BRUSH"
        );
    }
    for (file, why) in KNOWN_DIVERGENT {
        assert!(
            present.contains(*file),
            "KNOWN_DIVERGENT names {file}, which is not in tests/corpus"
        );
        assert!(!why.is_empty(), "{file}: a known divergence needs a reason");
        assert!(
            !seen.contains(*file),
            "{file} is in both EXPECTED_FAIL and KNOWN_DIVERGENT; it can only be one"
        );
    }
}
