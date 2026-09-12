//! The glob options scripts switch on — `nullglob`, `failglob`, `dotglob`, `nocaseglob`,
//! `nocasematch` — with `GLOBIGNORE` and `compgen -G`.
//!
//! Every one used to be refused: `shopt -s nullglob` printed an error and changed nothing, so a
//! script counting `x=(*.log)` saw one element for an empty directory. Every expectation was run
//! against GNU bash 5.3.9 in the same tree. Where several names come back, the check is a count or
//! a single name, so the answer does not depend on the machine's collating locale.

mod common;

use common::{Run, run_in};

/// `a1 b1 ABC .h` and a directory, `sub`.
fn sh(line: &str) -> Run {
    let dir = tempfile::tempdir().expect("tempdir");
    for name in ["a1", "b1", "ABC", ".h"] {
        std::fs::write(dir.path().join(name), "").expect("file");
    }
    std::fs::create_dir(dir.path().join("sub")).expect("dir");
    run_in(dir.path(), line)
}

fn out(line: &str) -> String {
    let r = sh(line);
    assert!(r.status != 127, "`{line}`: {}", r.stderr);
    r.out().to_string()
}

#[test]
fn nullglob_expands_a_pattern_that_matches_nothing_to_nothing() {
    assert_eq!(out("shopt -s nullglob; set -- zz*; echo $#"), "0");
    assert_eq!(out("set -- zz*; echo $#"), "1");
    assert_eq!(
        out("shopt -s nullglob && shopt -q nullglob && echo on"),
        "on"
    );
}

/// bash's `no match:` — and the rest of the line does not run.
#[test]
fn failglob_makes_a_pattern_that_matches_nothing_an_error() {
    let r = sh("shopt -s failglob; echo zz*; echo rc=$?");
    assert_eq!(r.out(), "");
    assert!(r.err().contains("no match: zz*"), "{}", r.stderr);
    assert_ne!(r.status, 0);
}

#[test]
fn dotglob_lets_a_wildcard_match_a_hidden_name() {
    assert_eq!(out("printf '%s\\n' * | grep -c '^\\.h$'"), "0");
    assert_eq!(
        out("shopt -s dotglob; printf '%s\\n' * | grep -c '^\\.h$'"),
        "1"
    );
}

#[test]
fn nocaseglob_matches_names_in_either_case() {
    assert_eq!(out("echo [a]BC"), "[a]BC");
    assert_eq!(out("shopt -s nocaseglob; echo [a]BC"), "ABC");
}

/// Setting `GLOBIGNORE` also turns `dotglob` on, as bash does.
#[test]
fn globignore_removes_matches_and_implies_dotglob() {
    assert_eq!(out("GLOBIGNORE='a*'; echo [ab]*"), "b1");
    assert_eq!(
        out("GLOBIGNORE='a*'; printf '%s\\n' * | grep -c '^\\.h$'"),
        "1"
    );
}

#[test]
fn nocasematch_reaches_case_and_both_double_bracket_matches() {
    for line in [
        "shopt -s nocasematch; case ABC in abc) echo y;; *) echo n;; esac",
        "shopt -s nocasematch; [[ ABC == a* ]] && echo y || echo n",
        "shopt -s nocasematch; [[ ABC =~ ^a ]] && echo y || echo n",
    ] {
        assert_eq!(out(line), "y", "{line}");
    }
    assert_eq!(out("[[ ABC == a* ]] && echo y || echo n"), "n");
}

#[test]
fn compgen_answers_the_actions_scripts_use() {
    assert_eq!(out("compgen -G 'a*'"), "a1");
    assert_eq!(out("compgen -W 'alpha beta apple' a"), "alpha\napple");
    assert_eq!(out("compgen -d"), "sub");
    assert_eq!(out("compgen -f a"), "a1");
    assert_eq!(out("compgen -G 'zz*'; echo rc=$?"), "rc=1");
    assert_eq!(out("compgen -A alias 2>/dev/null; echo rc=$?"), "rc=2");
}
