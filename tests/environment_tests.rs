//! Aliases, exit status, prefix assignments and variable scoping.
//!
//! Spawns the real binary; see `common/mod.rs` for why.

mod common;

use oslo::Environment;

use common::{assert_out, run};

// --- aliases ---
//
// A shell that is not interactive expands aliases only once `expand_aliases` is set, as bash's
// does, so each script below sets it first.

/// The line a script needs before any alias it defines is expanded.
const EXPAND: &str = "shopt -s expand_aliases\n";

#[test]
fn multi_word_alias_expands_to_multiple_arguments() {
    assert_out(&format!("{EXPAND}alias e='echo one two'\ne"), "one two");
}

#[test]
fn alias_arguments_are_appended() {
    assert_out(
        &format!("{EXPAND}alias e='echo prefix'\ne suffix"),
        "prefix suffix",
    );
}

#[test]
fn alias_body_keeps_quoted_grouping() {
    assert_out(&format!("{EXPAND}alias e='echo \"a  b\"'\ne"), "a  b");
}

/// Without the option a script's alias is never expanded.
#[test]
fn a_script_does_not_expand_aliases_by_default() {
    let r = run("alias e='echo hi'\ne");
    assert_eq!(r.status, 127, "stdout: {}", r.stdout);
}

/// A definition and its use on the *same* line: bash answers `e: command not found`, because an
/// alias is not available until the next command is read. These three tests used to be written
/// this way and passed only because oslo expanded aliases after parsing.
#[test]
fn an_alias_is_not_available_on_the_line_that_defines_it() {
    let r = run("alias e='echo hi'; e");
    assert_ne!(r.status, 0, "stdout: {}", r.stdout);
    assert!(r.stderr.contains("not found"), "stderr: {}", r.stderr);
}

/// The reason alias substitution moved ahead of the parser: an alias body is source text, and may
/// open a construct the word it replaced could not.
#[test]
fn an_alias_may_open_a_compound_command() {
    assert_out(
        &format!(
            "{EXPAND}alias forever='while :; do'\nn=0\nforever\n  n=$((n+1))\n  [ $n -ge 3 ] && break\ndone\necho $n"
        ),
        "3",
    );
}

/// An alias body is expanded as *source text*, not as one argv[0].
///
/// `alias ll='ls -la'` then `ll` has to become two words. Expanded as a single word it would be a
/// program literally named `ls -la`, and "command not found".
#[test]
fn an_alias_body_becomes_several_words() {
    let r = run(&format!("{EXPAND}alias ll='ls -la'\nll >/dev/null"));
    assert_eq!(r.status, 0, "stderr: {}", r.stderr);
}

/// **The conveniences oslo ships are a person's, and a script must not meet them.**
///
/// `ll`, `la` and `l` used to be seeded into every `Environment`, which is every script, every
/// `sh -c` and every subshell — so a script that defined `l() { … }` silently got `ls -CF`, because
/// an alias is resolved before a function. bash and dash both run the function.
#[test]
fn the_shipped_aliases_do_not_exist_in_a_script() {
    for word in ["ll", "la", "l"] {
        let r = run(word);
        assert_eq!(r.status, 127, "`{word}` still expands: {}", r.stderr);
    }
    // And the name is the script's own to use.
    let r = run("l() { echo MINE; }\nl");
    assert_eq!(r.stdout.trim(), "MINE", "stderr: {}", r.stderr);
}

// --- exit status ---

#[test]
fn exit_status_reflects_the_last_command() {
    assert_eq!(run("true").status, 0);
    assert_eq!(run("false").status, 1);
    assert_eq!(run("true; false").status, 1);
    assert_eq!(run("false; true").status, 0);
}

#[test]
fn unknown_command_exits_127() {
    assert_eq!(run("nosuchcommand_xyz").status, 127);
}

#[test]
fn explicit_exit_code_is_honoured() {
    assert_eq!(run("exit 7").status, 7);
}

#[test]
fn function_return_becomes_the_shell_status() {
    assert_eq!(run("f() { return 3; }; f").status, 3);
}

#[test]
fn pipeline_status_is_that_of_the_last_stage() {
    assert_eq!(run("false | true").status, 0);
    assert_eq!(run("true | false").status, 1);
}

#[test]
fn negated_pipeline_inverts_the_status() {
    assert_eq!(run("! false").status, 0);
    assert_eq!(run("! true").status, 1);
}

#[test]
fn status_is_readable_as_a_parameter() {
    assert_out("false; echo $?", "1");
    assert_out("true; echo $?", "0");
}

// --- prefix assignments ---

#[test]
fn prefix_assignment_reaches_the_child_environment() {
    assert_out(r#"FOO=bar sh -c 'echo $FOO'"#, "bar");
}

#[test]
fn prefix_assignment_does_not_outlive_the_command() {
    assert_out("FOO=bar true; echo \"after=[$FOO]\"", "after=[]");
}

#[test]
fn prefix_assignment_restores_a_previous_value() {
    assert_out(
        "FOO=orig; FOO=temp true; echo \"after=[$FOO]\"",
        "after=[orig]",
    );
}

#[test]
fn prefix_assignment_does_not_leak_into_later_children() {
    // The temporary export must be removed from the process environment afterwards, not merely
    // from the shell's own variable table.
    assert_out(
        r#"FOO=orig; FOO=temp true; sh -c 'echo child=[$FOO]'"#,
        "child=[]",
    );
}

#[test]
fn prefix_assignment_keeps_quoted_values_intact() {
    assert_out(r#"FOO="a b" sh -c 'echo [$FOO]'"#, "[a b]");
}

// --- scoping regressions ---

#[test]
fn local_shadows_and_restores() {
    assert_out(
        "V=outer; f() { local V=inner; echo $V; }; f; echo $V",
        "inner\nouter",
    );
}

#[test]
fn local_without_a_value_still_scopes() {
    assert_out(
        "f() { local Z; Z=set; echo $Z; }; f; echo \"after=[$Z]\"",
        "set\nafter=[]",
    );
}

#[test]
fn readonly_is_enforced() {
    // The refused assignment abandons its own line, as bash does, so the check is on the next.
    let r = run("readonly R=1; R=2\necho $R");
    assert_eq!(r.out(), "1");
}

// --- `export NAME` with no value: marked for export, still unset (PLAN.md round C) ---

/// `export V` with no value marks `V` for export and leaves it **unset**.
///
/// bash gives an empty `${V+set}` and no `V=` in a child's environment; oslo used to create it
/// empty, so every `${V+set}` answered "set" and every child saw a spurious `V=`. A later
/// assignment must still be exported, which is the whole reason the intention is recorded.
#[test]
fn exporting_a_name_that_does_not_exist_yet_does_not_create_it() {
    let mut env = Environment::new();
    assert!(env.export_var("PENDING_ONE"));
    assert!(env.get_var("PENDING_ONE").is_none(), "must still be unset");

    // Assigning later honours the pending export.
    assert!(env.set_var("PENDING_ONE", "1", false));
    assert_eq!(env.get_var("PENDING_ONE"), Some("1"));
    assert!(env.exported_vars().iter().any(|(n, _)| n == "PENDING_ONE"));
}

/// `unset` undoes the intention, or `export V; unset V; V=1` would still export.
#[test]
fn unsetting_forgets_a_pending_export() {
    let mut env = Environment::new();
    assert!(env.export_var("PENDING_TWO"));
    env.unset_var("PENDING_TWO");
    assert!(env.set_var("PENDING_TWO", "9", false));
    assert!(!env.exported_vars().iter().any(|(n, _)| n == "PENDING_TWO"));
}

/// The clock, spelled as a variable.
///
/// Every prompt tool measures command duration by reading a high-resolution clock in preexec and
/// again in precmd. Without these, starship forks `starship time`, oh-my-posh forks
/// `oh-my-posh get millis` and hexe forks `date +%s%3N` — once per command, on the path between
/// pressing Enter and seeing a prompt.
#[test]
fn the_clock_variables_exist_and_advance() {
    let r = run("echo $EPOCHREALTIME\necho $EPOCHSECONDS");
    let out = r.out().to_string();
    let mut lines = out.lines();
    let real = lines.next().expect("EPOCHREALTIME").to_string();
    let whole = lines.next().expect("EPOCHSECONDS").to_string();

    let (secs, micros) = real.split_once('.').expect("always a dot, never a comma");
    assert!(
        secs.parse::<u64>().expect("seconds") > 1_600_000_000,
        "{real}"
    );
    assert_eq!(micros.len(), 6, "six places always: {real}");
    assert_eq!(secs, whole.trim(), "the two must agree");
}

/// An assignment always wins — `SECONDS=0` is an idiom and `RANDOM=42` asks for a fixed sequence.
#[test]
fn a_set_value_beats_the_clock() {
    // **`SECONDS=n` re-bases the count**, so reading it straight back gives `n` and reading it a
    // minute later gives `n + 60`. It used to *store* the string, which pinned it at `n` for the
    // life of the shell — the opposite of what the idiom `SECONDS=0` is for.
    //
    // `99` or `100`: the assignment and the read are two commands, and the clock ticks between
    // them whenever the pair happens to straddle a second boundary. Pinning it to `99` failed
    // about one run in a hundred, which is a test measuring the scheduler rather than the shell.
    let rebased = run("SECONDS=99\necho $SECONDS").out().to_string();
    assert!(
        rebased == "99" || rebased == "100",
        "expected the count to resume from 99, got {rebased:?}"
    );

    // **`RANDOM=n` seeds the generator**, so what comes back is a draw and not the seed. bash
    // answers 19344 for `RANDOM=7`; the number is its generator's, not a contract, but *not the
    // seed* is. Storing the assignment made every later `$RANDOM` answer 7.
    let seeded = run("RANDOM=7\necho $RANDOM").out().to_string();
    let drawn: u32 = seeded.trim().parse().expect("a number");
    assert!(drawn < 32768, "in bash's range: {drawn}");
    assert_ne!(drawn, 7, "a draw from the seed, not the seed");
    // And seeding twice the same way gives the same sequence, which is why a script writes it.
    assert_eq!(run("RANDOM=7\necho $RANDOM").out(), seeded);

    // An assignment to `$EPOCHSECONDS` is discarded, as bash discards it.
    assert_ne!(run("EPOCHSECONDS=1\necho $EPOCHSECONDS").out(), "1");
}

/// `$RANDOM` is a sequence in bash's range, not one number repeated.
#[test]
fn random_is_a_sequence() {
    let out = run("for i in 1 2 3 4 5 6; do echo $RANDOM; done")
        .out()
        .to_string();
    let draws: Vec<u32> = out.lines().filter_map(|l| l.trim().parse().ok()).collect();
    assert_eq!(draws.len(), 6, "{out:?}");
    assert!(
        draws.iter().all(|&n| n < 32768),
        "outside 0..32767: {draws:?}"
    );
    assert!(
        draws.windows(2).any(|w| w[0] != w[1]),
        "constant: {draws:?}"
    );
}

/// `$SHLVL` counts nesting, which means it has to reach the child's environment — not just
/// oslo's own table, or every nested shell reads the grandparent's depth.
#[test]
fn shlvl_counts_nesting() {
    // Both read in one process tree, so the harness's own nesting cancels out.
    let bin = common::oslo_bin();
    let r = run(&format!("echo $SHLVL\n{} -c 'echo $SHLVL'", bin.display()));
    let mut lines = r.out().lines();
    let outer: u32 = lines.next().expect("outer").parse().expect("a number");
    let nested: u32 = lines.next().expect("nested").parse().expect("a number");
    assert_eq!(
        nested,
        outer + 1,
        "outer {outer}, nested {nested}: {}",
        r.stderr
    );
}

/// Every option bash's `set -o` has is accepted by a script, as bash accepts it.
#[test]
fn every_bash_option_is_accepted_by_a_script() {
    for name in [
        "notify", "keyword", "nolog", "vi", "emacs", "errtrace", "physical",
    ] {
        // No `=` in the word echoed: under `keyword` it would be an assignment.
        let r = run(&format!("set -o {name}\necho status $?"));
        assert_eq!(r.out(), "status 0", "{name}: stderr was {:?}", r.stderr);
    }
    let r = run("set +o notify\necho status=$?");
    assert_eq!(r.out(), "status=0", "{:?}", r.stderr);
}

/// The ones that are implemented still work, or the refusal has cost more than it bought.
#[test]
fn the_implemented_options_still_take() {
    for name in [
        "pipefail",
        "errexit",
        "nounset",
        "posix",
        "noclobber",
        "noglob",
    ] {
        let r = run(&format!("set -o {name}\necho status=$?"));
        assert_eq!(r.out(), "status=0", "{name}: {:?}", r.stderr);
    }
}
