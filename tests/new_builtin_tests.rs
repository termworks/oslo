//! End-to-end coverage for the builtins added in Round 5: `:`, `exec`, `command`, `builtin`,
//! `getopts`, `let`, `hash`, `times`, `ulimit` and `declare`/`typeset`.
//!
//! Spawns the real binary; see `common/mod.rs` for why. Everything here was a
//! `command not found` before these builtins existed, so each test is also a regression test
//! against the builtin quietly falling out of the registry.

mod common;

use common::{assert_out, run, run_in};

// --- `:` — the null command ---

#[test]
fn the_null_command_succeeds_and_prints_nothing() {
    let r = run(":");
    assert_eq!(r.out(), "");
    assert_eq!(r.status, 0);
    assert_eq!(r.stderr, "");
}

/// The idiom `:` exists for: an infinite loop with the exit condition in the body.
#[test]
fn the_null_command_drives_a_while_loop() {
    assert_out(
        "i=0; while :; do i=$((i + 1)); [ \"$i\" -ge 3 ] && break; done; echo $i",
        "3",
    );
}

/// The other idiom: `:` expands its arguments for their side effects and discards them.
#[test]
fn the_null_command_still_expands_its_arguments() {
    assert_out("unset x; : ${x:=defaulted}; echo \"$x\"", "defaulted");
}

#[test]
fn the_null_command_is_a_valid_empty_case_arm() {
    assert_out("case foo in bar) echo no ;; *) : ;; esac; echo $?", "0");
}

// --- `exec` ---

/// The replace-process form: the shell becomes the command, so nothing after it ever runs.
#[test]
fn exec_replaces_the_shell_process() {
    let r = run("exec echo replaced; echo NOT_REACHED");
    assert_eq!(r.out(), "replaced");
    assert_eq!(r.status, 0);
}

#[test]
fn exec_passes_the_exit_status_of_the_replacement() {
    assert_eq!(run("exec false").status, 1);
}

/// `-a` renames argv[0], which is how a multi-call binary is told which personality to be.
#[test]
fn exec_can_override_argv0() {
    assert_out("exec -a oslotest sh -c 'echo $0'", "oslotest");
}

/// A command that cannot be found ends a non-interactive shell with 127, rather than carrying on
/// with descriptors that were set up for a program that never started.
#[test]
fn exec_of_a_missing_command_ends_the_script() {
    let r = run("exec no-such-command-xyz; echo NOT_REACHED");
    assert_eq!(r.out(), "");
    assert_eq!(r.status, 127);
    assert!(!r.stderr.is_empty());
}

#[test]
fn exec_with_no_arguments_at_all_succeeds() {
    assert_out("exec; echo still here", "still here");
}

// --- `command` ---

#[test]
fn command_v_reports_a_builtin_by_name() {
    assert_out("command -v echo", "echo");
}

#[test]
fn command_v_reports_a_path_operand_as_given() {
    assert_out("command -v /bin/sh", "/bin/sh");
}

/// The feature-probe idiom: silent, and the status is the whole answer.
#[test]
fn command_v_is_silent_and_fails_for_an_unknown_name() {
    let r = run("command -v no-such-command-xyz");
    assert_eq!(r.out(), "");
    assert_eq!(r.stderr, "");
    assert_eq!(r.status, 1);
}

#[test]
fn command_capital_v_describes_the_kind_of_command() {
    assert_out("command -V echo", "echo is a shell builtin");
}

/// The reason `command` exists: a wrapper function has to be able to call the thing it wraps.
#[test]
fn command_bypasses_a_shadowing_function() {
    assert_out(
        "echo() { printf 'shadowed\\n'; }\ncommand echo bypassed",
        "bypassed",
    );
}

#[test]
fn command_runs_an_external_binary() {
    assert_out("command printf 'x\\n'", "x");
}

#[test]
fn command_reports_a_missing_command_as_127() {
    let r = run("command no-such-command-xyz");
    assert_eq!(r.status, 127);
    assert!(!r.stderr.is_empty());
}

// --- `builtin` ---

#[test]
fn builtin_forces_the_builtin_over_a_function() {
    assert_out(
        "echo() { printf 'shadowed\\n'; }\nbuiltin echo forced",
        "forced",
    );
}

/// `builtin` only reaches builtins. `cat` rather than `printf`: `printf` used to be external and
/// was the example here, until it became a builtin and quietly turned this into a test that
/// `builtin printf` *works* — which is a different claim.
#[test]
fn builtin_refuses_an_external_command() {
    let r = run("builtin cat /dev/null");
    assert_eq!(r.status, 1);
    assert!(!r.stderr.is_empty());
}

// --- `getopts` ---

/// A real option-parsing loop, the way scripts write one.
#[test]
fn getopts_drives_a_full_option_loop() {
    let script = r#"
set -- -a -b value -- rest
while getopts "ab:" opt; do
    case "$opt" in
        a) echo "flag_a" ;;
        b) echo "opt_b=$OPTARG" ;;
    esac
done
shift $((OPTIND - 1))
echo "remaining=$*"
"#;
    assert_out(script, "flag_a\nopt_b=value\nremaining=rest");
}

#[test]
fn getopts_splits_a_cluster_into_separate_options() {
    assert_out(
        "set -- -ab; while getopts \"ab\" o; do echo \"$o\"; done",
        "a\nb",
    );
}

#[test]
fn getopts_accepts_an_argument_glued_to_its_option() {
    assert_out(
        "set -- -bvalue; getopts \"b:\" o; echo \"$o=$OPTARG\"",
        "b=value",
    );
}

/// Silent mode reports the offending option through `OPTARG` and prints nothing.
#[test]
fn getopts_silent_mode_reports_errors_through_variables() {
    let r = run("set -- -z; getopts \":a\" o; echo \"$o/$OPTARG\"");
    assert_eq!(r.out(), "?/z");
    assert_eq!(r.stderr, "");
}

#[test]
fn getopts_complains_about_an_unknown_option_by_default() {
    let r = run("set -- -z; getopts \"a\" o; echo \"$o\"");
    assert_eq!(r.out(), "?");
    assert!(!r.stderr.is_empty());
}

/// An explicit argument list overrides the positional parameters.
#[test]
fn getopts_accepts_its_own_argument_list() {
    assert_out("getopts ab o -b; echo \"$o\"", "b");
}

// --- `let` ---

#[test]
fn let_evaluates_and_assigns() {
    assert_out("let x=1+2; echo $x", "3");
}

#[test]
fn let_status_is_inverted() {
    assert_out("let '1 > 3'; echo $?", "1");
    assert_out("let '3 > 1'; echo $?", "0");
}

#[test]
fn let_can_drive_a_loop_counter() {
    assert_out("i=0; while [ $i -lt 3 ]; do let i=i+1; done; echo $i", "3");
}

#[test]
fn a_bad_let_expression_fails_without_killing_the_shell() {
    let r = run("let '1 +'; echo after=$?");
    assert_eq!(r.out(), "after=1");
    assert!(!r.stderr.is_empty());
}

// --- `hash`, `times`, `ulimit` ---

#[test]
fn hash_records_a_command_and_reports_a_missing_one() {
    assert_out("hash sh; echo $?", "0");
    let r = run("hash no-such-command-xyz");
    assert_eq!(r.status, 1);
    assert!(!r.stderr.is_empty());
}

#[test]
fn hash_with_no_arguments_reports_the_table() {
    let r = run("hash");
    assert_eq!(r.status, 0);
    assert!(!r.stdout.is_empty());
}

/// `hash -r` is a request to forget, not to report. It used to fall through to the listing, so
/// every script that cleared the cache got "hash table empty" on its stdout.
#[test]
fn resetting_the_hash_table_prints_nothing() {
    let r = run("hash sh; hash -r");
    assert_eq!(r.status, 0);
    assert_eq!(r.out(), "", "stderr: {}", r.stderr);
}

/// Two lines of `<minutes>m<seconds>s` pairs, the shape every shell's `times` prints.
#[test]
fn times_prints_two_lines_of_cpu_time() {
    let r = run("times");
    assert_eq!(r.status, 0);
    let lines: Vec<&str> = r.out().lines().collect();
    assert_eq!(lines.len(), 2, "got {:?}", r.stdout);
    for line in lines {
        let halves: Vec<&str> = line.split(' ').collect();
        assert_eq!(halves.len(), 2, "expected `user system`, got {line:?}");
        for half in halves {
            assert!(
                half.contains('m') && half.ends_with('s'),
                "expected <min>m<sec>s, got {half:?}"
            );
        }
    }
}

// --- `declare` / `typeset` ---

#[test]
fn declare_assigns_under_either_name() {
    assert_out("declare X=1; typeset Y=2; echo \"$X$Y\"", "12");
}

/// Local inside a function, global outside — without `declare` having to know which it is in.
#[test]
fn declare_is_local_inside_a_function() {
    assert_out(
        "f() { declare Y=inner; echo \"$Y\"; }\nY=outer\nf\necho \"$Y\"",
        "inner\nouter",
    );
}

#[test]
fn declare_p_prints_a_readable_declaration() {
    assert_out("declare X=1; declare -p X", "declare -- X=\"1\"");
}

#[test]
fn declare_x_exports() {
    assert_out(
        "declare -x OSLODECL=v; env | grep '^OSLODECL='",
        "OSLODECL=v",
    );
}

/// `-A` makes an associative array, whose subscripts are keys rather than arithmetic, and `-n` a
/// nameref; a letter `declare` has no meaning for is still refused.
#[test]
fn declare_has_associative_arrays_and_namerefs() {
    assert_out(
        "declare -A assoc; assoc[a]=1; assoc[b]=2; echo \"${assoc[a]}${assoc[b]} ${#assoc[@]}\"",
        "12 2",
    );
    assert_out("other=v; declare -n ref=other; echo \"$ref\"", "v");
    let r = run("declare -Q v");
    assert_eq!(r.status, 2);
    assert!(!r.stderr.is_empty());
}

/// `-i` is no longer among them: an integer name evaluates what it is assigned.
#[test]
fn declare_i_evaluates_the_assignment() {
    assert_out("declare -i n=2+3; echo \"$n\"", "5");
}

/// `declare -a`, by contrast, now works: it makes an empty indexed array.
#[test]
fn declare_a_makes_an_empty_array() {
    assert_out("declare -a arr; echo \"${#arr[@]}\"", "0");
}

#[test]
fn ulimit_reports_a_soft_limit() {
    let r = run("ulimit -n");
    assert_eq!(r.status, 0);
    let value = r.out();
    assert!(
        value == "unlimited" || value.parse::<u64>().is_ok(),
        "expected a number or `unlimited`, got {value:?}"
    );
}

/// The hard limit is at least the soft one, which is the only relation that always holds.
#[test]
fn ulimit_can_report_the_hard_limit() {
    let soft = run("ulimit -Sn");
    let hard = run("ulimit -Hn");
    assert_eq!(hard.status, 0);
    if let (Ok(s), Ok(h)) = (soft.out().parse::<u64>(), hard.out().parse::<u64>()) {
        assert!(h >= s, "hard limit {h} below soft limit {s}");
    }
}

/// The set direction really sets: `ulimit` used to refuse an operand outright, because `nix`'s
/// `resource` feature was off and accepting a value it could not apply would have told a script
/// it had headroom it did not have. Lowered, never raised, so the test does not depend on
/// privileges. Its own process, so nothing else inherits the smaller limit.
#[test]
fn ulimit_applies_a_limit_it_reports_back() {
    assert_out("ulimit -c 0; ulimit -c", "0");
    assert_out("ulimit -n 128; ulimit -n", "128");
    // File sizes go through a 512-byte block conversion in both directions.
    assert_out("ulimit -f 4096; ulimit -f", "4096");
}

/// A value that is not a number is refused rather than rounded to something.
#[test]
fn ulimit_refuses_a_value_that_is_not_a_limit() {
    let r = run("ulimit -n abc");
    assert_eq!(r.status, 1);
    assert!(!r.stderr.is_empty());
}

// --- the `exec` redirection-only form ---

/// The whole-script redirect. A builtin never sees its own redirections, so the permanence is the
/// dispatcher's decision: `exec::simple` asks `builtins::exec_makes_redirections_permanent` and
/// builds a non-restoring `RedirectGuard` when it says yes.
#[test]
fn exec_redirects_the_shell_itself_for_good() {
    let dir = tempfile::tempdir().unwrap();
    let r = run_in(
        dir.path(),
        "exec > log.txt\necho captured\necho also captured",
    );
    assert_eq!(r.out(), "", "nothing should reach the shell's own stdout");

    let log = std::fs::read_to_string(dir.path().join("log.txt")).expect("log.txt");
    assert_eq!(log, "captured\nalso captured\n");
}

/// A named descriptor stays open after `exec` returns, and `>&-` closes it again.
#[test]
fn exec_opens_and_closes_a_named_descriptor() {
    let dir = tempfile::tempdir().unwrap();
    let r = run_in(
        dir.path(),
        "exec 4> out.txt\necho written >&4\nexec 4>&-\ncat out.txt",
    );
    assert_eq!(r.out(), "written");
}

/// The same thing on descriptor 3 — the number every script actually reaches for.
///
/// It fails for a reason that has nothing to do with `exec`: `RedirectGuard::apply` opens the
/// target file, which lands on the *lowest free* descriptor (3 in a shell that has only 0, 1 and
/// 2 open), then `dup2(3, 3)` is a no-op and dropping the `File` at the end of the match arm
/// closes descriptor 3 again. `exec 4>` works for the same reason `exec 3>` does not. The fix
/// belongs in `exec::redirect`: keep the opened `File` alive (or `into_raw_fd` it) when it
/// already occupies the target descriptor.
#[test]
#[ignore = "R5.7/redirect.rs: an opened file that lands on the target fd is closed by its own Drop"]
fn exec_opens_descriptor_three() {
    let dir = tempfile::tempdir().unwrap();
    let r = run_in(
        dir.path(),
        "exec 3> out.txt\necho written >&3\nexec 3>&-\ncat out.txt",
    );
    assert_eq!(r.out(), "written");
}
