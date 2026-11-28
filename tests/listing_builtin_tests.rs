//! `export`, `unset`, `set`, `alias`, `unalias`, `local` and `readonly` through the real binary.
//!
//! The listings these builtins produce are only worth anything if a shell can read them back, and
//! that is not something an in-process assertion can check: it needs the output of one oslo to be
//! the input of the next. Every round-trip test here therefore writes a listing to a file and
//! sources it.

mod common;

use common::{run, run_in};

/// Two runs of the same script must produce byte-identical listings.
///
/// The listing used to iterate a `HashMap`, so the order changed between runs of the same script
/// — output no diff, no `cmp` and no cache key could ever rely on.
///
/// `$PWD` is excluded, and that is not a workaround. `run` gives each invocation its own temporary
/// directory, so the two shells genuinely *are* somewhere different — a `set` that printed the same
/// `PWD` for both would be the bug. bash lists it too. What this test is about is the *order* of
/// the listing, so the lines that legitimately differ are dropped rather than the comparison being
/// weakened.
///
/// `$OSLO_SESSION` is excluded for the same reason and is the same kind of thing: two runs are two
/// sessions, and a `set` that printed one id for both would mean a shell could not tell its own
/// session from another's. `$OSLO_NESTED_PID` is a third: it names *this* shell, so two runs must
/// disagree about it or nothing below could tell a live shell above from a remembered one.
/// `$BASHPID` names this process as well, and `DIRSTACK`, which `set` lists as bash does, starts
/// with `$PWD`.
#[test]
fn set_listing_is_deterministic() {
    let script = "a=1; b=2; c=3; f() { echo one; }; g() { echo two; }; set";
    let per_process = [
        "PWD=",
        "OSLO_SESSION=",
        "OSLO_NESTED_PID=",
        "BASHPID=",
        "DIRSTACK=",
    ];
    let without_pwd = |text: &str| -> String {
        text.lines()
            .filter(|line| !per_process.iter().any(|name| line.starts_with(name)))
            .map(|line| format!("{line}\n"))
            .collect()
    };
    let first = run(script);
    let second = run(script);
    assert_eq!(
        without_pwd(&first.stdout),
        without_pwd(&second.stdout),
        "set output changed between runs"
    );
    assert!(!first.stdout.is_empty());
    assert!(
        first.stdout.contains("PWD="),
        "and PWD is listed at all, as it is in bash"
    );
}

#[test]
fn set_listing_is_sorted() {
    let r = run("zz_last=1; aa_first=1; set");
    let names: Vec<&str> = r
        .stdout
        .lines()
        .filter_map(|l| l.split_once('=').map(|(n, _)| n))
        .collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted, "set output is not sorted:\n{}", r.stdout);
}

/// A value with spaces and newlines has to survive `set > file; . file`.
#[test]
fn set_listing_round_trips_an_awkward_value() {
    let dir = tempfile::tempdir().expect("tempdir");
    let r = run_in(
        dir.path(),
        "v='a b\nc'; set > saved; unset v; . ./saved; echo \"[$v]\"",
    );
    assert_eq!(r.out(), "[a b\nc]", "stderr: {}", r.stderr);
}

/// `set` lists functions, not just variables, and the definition parses again.
#[test]
fn set_listing_includes_function_definitions() {
    let dir = tempfile::tempdir().expect("tempdir");
    let r = run_in(
        dir.path(),
        "f() { echo from_f; }; set > saved; . ./saved; f",
    );
    assert!(r.out().ends_with("from_f"), "stdout: {}", r.stdout);
}

/// `export -p` names the exported variables; it used to create one called `-p`.
#[test]
fn export_p_lists_and_declares_nothing() {
    let r = run("export EV='a b'; export -p");
    assert!(
        r.stdout.contains("declare -x EV=\"a b\""),
        "stdout tail: {}",
        r.stdout
    );
    assert!(!r.stdout.contains("export -p"), "stdout: {}", r.stdout);
}

#[test]
fn export_listing_round_trips() {
    let dir = tempfile::tempdir().expect("tempdir");
    let r = run_in(
        dir.path(),
        "export EV=\"it's here\"; export -p > saved; unset EV; . ./saved; echo \"[$EV]\"",
    );
    assert_eq!(r.out(), "[it's here]", "stderr: {}", r.stderr);
}

/// The whole point of R5.10: `unset` used to drop the value and leave the read-only mark, so the
/// variable was empty and could never be set again.
#[test]
fn unset_cannot_empty_a_readonly_variable() {
    let r = run("readonly r=kept; unset r; echo \"status=$? value=$r\"");
    assert_eq!(r.out(), "status=1 value=kept", "stderr: {}", r.stderr);
}

#[test]
fn readonly_listing_shows_values() {
    let r = run("readonly RO='a b'; readonly");
    assert!(
        r.stdout.contains("declare -r RO=\"a b\""),
        "stdout: {}",
        r.stdout
    );
}

#[test]
fn readonly_listing_round_trips() {
    let dir = tempfile::tempdir().expect("tempdir");
    let r = run_in(
        dir.path(),
        "readonly RO='x y'; readonly -p > saved; grep ' RO=' saved",
    );
    assert_eq!(r.out(), "declare -r RO=\"x y\"", "stderr: {}", r.stderr);
}

/// The acceptance test named in the plan: `alias > f; . f` must reproduce the table.
#[test]
fn alias_listing_round_trips() {
    let dir = tempfile::tempdir().expect("tempdir");
    let r = run_in(
        dir.path(),
        "alias q='it'\\''s'; alias > one; unalias -a; . ./one; alias > two; cmp one two && echo same",
    );
    assert_eq!(r.out(), "same", "stderr: {}", r.stderr);
}

/// `alias` with no operands prints the table, in the form that can be read back.
///
/// It used to assert on `ll`, which the shell seeded into *every* environment — including every
/// script, where it shadowed a function of the same name. The conveniences now belong to an
/// interactive session, so this defines its own and checks the listing rather than the seeding.
#[test]
fn alias_with_no_operands_prints_the_table() {
    let r = run("alias ll='ls -la'; alias");
    assert!(
        r.stdout.contains("alias ll='ls -la'"),
        "stdout: {}",
        r.stdout
    );
}

#[test]
fn unalias_reports_a_missing_name_and_clears_with_a() {
    assert_eq!(run("unalias nosuch; echo $?").out(), "1");
    assert_eq!(run("unalias -a; alias; echo done").out(), "done");
}

/// `local -r x=1` used to declare a variable called `-r` alongside `x`.
#[test]
fn local_options_are_not_variable_names() {
    let r = run("f() { local -r v=1; echo \"[$v]\"; }; f; echo \"[${dash_r:-none}]\"");
    assert_eq!(r.out(), "[1]\n[none]", "stderr: {}", r.stderr);
}

/// Outside a function there is no frame to pop, so `local` there is a global that outlives its
/// line. Creating one silently is worse than refusing: the script gets a leak it never sees.
#[test]
fn local_outside_a_function_is_refused() {
    let r = run("local x=1; echo status=$?; echo \"[${x:-unset}]\"");
    assert_eq!(r.out(), "status=1\n[unset]", "stderr: {}", r.stderr);
    assert!(
        r.stderr.contains("can only be used in a function"),
        "stderr: {}",
        r.stderr
    );
}

/// A prefix assignment pushes a scope frame of its own, so "the frame stack is non-empty" is not
/// the same question as "we are inside a function".
#[test]
fn a_prefix_assignment_does_not_make_local_legal() {
    let r = run("FOO=bar local x=1; echo status=$?");
    assert_eq!(r.out(), "status=1", "stderr: {}", r.stderr);
}

/// `export -n` keeps the value in the shell and takes it out of the child's environment.
#[test]
fn export_n_unexports_without_losing_the_value() {
    let r = run("export EN=1; export -n EN; echo \"[$EN]\"; sh -c 'echo \"child=${EN:-unset}\"'");
    assert_eq!(r.out(), "[1]\nchild=unset", "stderr: {}", r.stderr);
}

/// `export` used to `trim()` its operand and strip a leading and trailing quote character, so a
/// value that legitimately had either lost it — silently, on the way into the environment.
#[test]
fn export_does_not_edit_the_value_it_was_given() {
    let r = run("export EV=' a '; echo \"[$EV]\"; export EQ=\"'q'\"; echo \"[$EQ]\"");
    assert_eq!(r.out(), "[ a ]\n['q']", "stderr: {}", r.stderr);
}

/// Each frame gets its own binding, and the caller's is restored on the way out.
#[test]
fn local_nests_across_function_calls() {
    let r = run("v=global; inner() { local v=inner; echo \"inner=$v\"; }; \
         outer() { local v=outer; inner; echo \"outer=$v\"; }; outer; echo \"global=$v\"");
    assert_eq!(
        r.out(),
        "inner=inner\nouter=outer\nglobal=global",
        "stderr: {}",
        r.stderr
    );
}
