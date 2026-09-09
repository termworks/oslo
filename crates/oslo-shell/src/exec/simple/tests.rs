//! Evaluating a simple command, checked at the seams where the order of operations shows.
//!
//! Split out of `simple.rs` for the 600-line limit. The file above is the order in which a word
//! becomes an alias, a function, a builtin or a program; this is what pins that order down.

use crate::env::Environment;

/// Run a snippet in a fresh environment and hand back the environment to inspect.
///
/// `Environment::new()` snapshots the *process* environment and `export` writes back into it,
/// so an exported name set by one test is visible to every environment built afterwards.
/// Tests here therefore use names unique to each test rather than a shared `v`.
fn run(src: &str) -> Environment {
    let mut env = Environment::new();
    run_in(&mut env, src).expect("exec");
    env
}

/// Run a snippet in an environment the caller has already configured, and hand back the
/// result rather than unwrapping it — the POSIX-mode cases are about the error itself.
fn run_in(env: &mut Environment, src: &str) -> oslo_base::error::Result<i32> {
    let script = crate::syntax::parse_bash_script(src).expect("parse");
    crate::exec::eval_command_list(env, &script)
}

fn var(src: &str, name: &str) -> String {
    run(src).get_var(name).unwrap_or_default().to_string()
}

/// POSIX 2.9.1: the assignment RHS gets tilde, parameter, command and arithmetic expansion,
/// but *not* field splitting and *not* pathname expansion. Globbing it would make the value
/// depend on the working directory's contents; splitting it would collapse any IFS character
/// or newline the value legitimately contains.
///
/// `Cargo.*` is used rather than a scratch directory on purpose: unit tests run in the crate
/// root, where that pattern really does match files, so a regression to `expand_word` would
/// show up as `Cargo.lock Cargo.toml` instead of a silently-unchanged literal.
#[test]
fn assignment_rhs_is_not_globbed() {
    assert_eq!(var("oslo_g1=Cargo.*", "oslo_g1"), "Cargo.*");
    assert_eq!(var("oslo_g2=Cargo.* true", "oslo_g2"), "");
    assert_eq!(var("export oslo_g3=Cargo.*", "oslo_g3"), "Cargo.*");
}

#[test]
fn assignment_rhs_is_not_field_split() {
    assert_eq!(var("IFS=:\noslo_s1=a:b:c", "oslo_s1"), "a:b:c");
    assert_eq!(var("IFS=:\nexport oslo_s2=a:b:c", "oslo_s2"), "a:b:c");
    // Interior whitespace from an unquoted expansion survives too.
    assert_eq!(var("oslo_s3='a  b'\noslo_s4=$oslo_s3", "oslo_s4"), "a  b");
}

// The third leg of R2.9 — `x=$(printf 'a\nb')` keeps its newline — is deliberately *not*
// tested here. Command substitution forks (`exec/substitution.rs`), and libtest runs unit
// tests on a pool of threads: a child forked out of a multi-threaded process inherits any
// mutex another thread happened to hold, so the child deadlocks in the allocator before it
// can write to the pipe and the parent blocks forever in `waitpid`. That is a property of
// the harness, not of the shell (oslo itself is single-threaded), so the case lives in
// `tests/expansion_tests.rs`, which spawns the real binary.

/// The `words.is_empty()` fallback path — a command word that expands to nothing leaves only
/// the assignments — must apply the same rule as the ordinary one.
#[test]
fn assignment_survives_an_empty_command_word() {
    assert_eq!(
        var("IFS=:\noslo_e1=\noslo_e2=a:b $oslo_e1", "oslo_e2"),
        "a:b"
    );
}

/// A prefix assignment is scoped to its command and must not leak back out.
#[test]
fn prefix_assignment_does_not_outlive_its_command() {
    assert_eq!(run("oslo_p1=a:b true").get_var("oslo_p1"), None);
}

/// A function must be found before the builtin of the same name (PLAN R5.6). Asserted on a
/// side effect rather than on stdout, because the unit-test harness captures the shell's
/// `println!` but not what a builtin writes; if the wrapper never ran, the variable is unset.
#[test]
fn a_function_shadows_a_regular_builtin() {
    let env = run("cd() { oslo_shadow=called; }\ncd /nonexistent-dir");
    assert_eq!(env.get_var("oslo_shadow"), Some("called"));
}

/// …including the ones whose names the dispatcher used to hardcode.
#[test]
fn a_function_shadows_echo_and_test() {
    let env = run("echo() { oslo_shadow_echo=1; }\necho hi");
    assert_eq!(env.get_var("oslo_shadow_echo"), Some("1"));
    let env = run("test() { oslo_shadow_test=1; }\ntest -f /etc/hosts");
    assert_eq!(env.get_var("oslo_shadow_test"), Some("1"));
}

/// POSIX mode is the only mode in which a special builtin outranks a function.
///
/// The mode lives on the `Environment` now, not in a process-global that every other test in
/// this binary had to be protected from: the second half here simply runs in a different
/// environment, and nothing has to be restored afterwards.
#[test]
fn posix_mode_puts_special_builtins_ahead_of_functions() {
    let env = run("export() { oslo_shadow_export=1; }\nexport oslo_ignored=x");
    assert_eq!(env.get_var("oslo_shadow_export"), Some("1"));

    let mut env = Environment::new();
    env.set_option(crate::env::options::ShellOption::Posix, true);
    run_in(
        &mut env,
        "export() { oslo_shadow_export2=1; }\nexport oslo_special=y",
    )
    .expect("exec");
    assert_eq!(env.get_var("oslo_shadow_export2"), None);
    assert_eq!(env.get_var("oslo_special"), Some("y"));
}

/// C3: a refused assignment is a *failed* command, not a silent success.
///
/// Outside POSIX mode the shell carries on with status 1, which is what
/// `bash -c $'readonly r=1\nr=2\necho "$?"\necho after'` does.
#[test]
fn a_refused_assignment_fails_the_command_and_the_shell_carries_on() {
    let mut env = Environment::new();
    env.set_var("oslo_ro_a", "1", false);
    env.set_readonly("oslo_ro_a");
    assert_eq!(run_in(&mut env, "oslo_ro_a=2").expect("not fatal"), 1);
    assert_eq!(env.get_var("oslo_ro_a"), Some("1"));
    // …and the next command still runs.
    assert_eq!(run_in(&mut env, "oslo_ro_b=ok").expect("not fatal"), 0);
    assert_eq!(env.get_var("oslo_ro_b"), Some("ok"));
}

/// …and in POSIX mode the same assignment ends the shell, as POSIX 2.8.1 requires and
/// `bash --posix -c` does. It unwinds as `exit`, so the EXIT trap still runs.
#[test]
fn a_refused_assignment_exits_a_posix_shell() {
    let mut env = Environment::new();
    env.set_option(crate::env::options::ShellOption::Posix, true);
    // Started with `-c`, which is the form bash answers 127 for; a script file answers 1.
    env.set_option(crate::env::options::ShellOption::CommandString, true);
    env.set_var("oslo_ro_c", "1", false);
    env.set_readonly("oslo_ro_c");
    match run_in(&mut env, "oslo_ro_c=2\noslo_never=reached") {
        Err(oslo_base::error::ShellError::Exit(status)) => {
            assert_eq!(status, oslo_base::error::FATAL_EXIT_STATUS);
        }
        other => panic!("expected the shell to exit, got {:?}", other),
    }
    assert_eq!(env.get_var("oslo_never"), None);
}

/// An ordinary non-zero status must *not* end a POSIX shell, even from a special builtin.
/// `bash --posix -c 'shift 5; echo alive'` prints `alive` and exits 0; a check written as
/// `status != 0` would have killed the shell here.
#[test]
fn a_failing_special_builtin_that_is_not_a_utility_error_is_survivable() {
    let mut env = Environment::new();
    env.set_option(crate::env::options::ShellOption::Posix, true);
    assert_eq!(
        run_in(&mut env, "shift 5\noslo_still_alive=yes").expect("not fatal"),
        0
    );
    assert_eq!(env.get_var("oslo_still_alive"), Some("yes"));
}

/// Every builtin now dispatches through the registry, so a name the registry does not have
/// is not a builtin at all — it used to reach the `_` arm and return 0 without running.
#[test]
fn an_unregistered_name_is_not_a_builtin() {
    let env = Environment::new();
    assert!(!env.is_builtin("oslo-not-a-builtin"));
    assert!(env.is_builtin("type"));
}

/// A function frame records the function's **name**.
///
/// `enter_function` stores `NULL`, and that placeholder was what every frame held: `caller` printed
/// it as the source of every frame, and any question about "which function am I in" could only be
/// answered with it. `enter_function_named` existed the whole time and nothing outside its own
/// tests called it.
///
/// Recorded from *inside* the call, because the frame is popped on the way out — which is also
/// why this went unnoticed: from the outside the stack is always empty.
#[test]
fn a_function_frame_knows_its_name() {
    thread_local! {
        static SEEN: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    // A builtin rather than a closure: registration takes a plain function pointer, so the only
    // way back out of the call is thread-local state.
    fn record(env: &mut Environment, _args: &[String]) -> oslo_base::error::Result<i32> {
        SEEN.with(|seen| *seen.borrow_mut() = env.call_stack().to_vec());
        Ok(0)
    }

    let mut env = Environment::new();
    env.register_custom_builtin("record-stack", record);
    run_in(
        &mut env,
        "outer() { inner; }\ninner() { record-stack; }\nouter",
    )
    .expect("exec");
    SEEN.with(|seen| {
        assert_eq!(
            *seen.borrow(),
            vec!["outer".to_string(), "inner".to_string()],
            "frames must carry their names, outermost first"
        )
    });
}

/// **`$_` is the last argument of the command that just ran.**
///
/// `mkdir -p a/b && cd $_` is what it exists for. Unset, `cd` was called with no argument and the
/// shell went to `$HOME` — a wrong directory reported as success, with every later command running
/// on the wrong files. Each case here was checked against bash 5.3 before it was written down.
#[test]
fn the_last_argument_is_remembered() {
    assert_eq!(var("true alpha", "_"), "alpha");
    assert_eq!(var(": one two three", "_"), "three");
    // A command with no arguments is its own last word.
    assert_eq!(var("true", "_"), "true");
    // A function call is a command like any other, and its argument is the one that counts.
    assert_eq!(var("f(){ :; }; f q", "_"), "q");
    // The most recent command wins.
    assert_eq!(var("true first; true second", "_"), "second");
}

/// An assignment on its own is not a command, so it leaves `$_` alone — as bash does.
#[test]
fn an_assignment_alone_does_not_touch_it() {
    assert_eq!(var("true kept; x=1", "_"), "kept");
}

/// It describes this shell's own history, so a child is not told it.
#[test]
fn the_last_argument_is_not_exported() {
    let env = run("true alpha");
    assert!(!env.get_exported_vars().contains_key("_"));
}
