//! The prompt's two languages, driven through the real binary.
//!
//! `-i` forces the interactive loop even though stdin is a pipe, which is what lets these run
//! without a pty. Two things are out of reach that way and are *not* covered here: the toggle
//! key, because no key is ever pressed down a pipe, and the prompt string, because rustyline
//! does not draw one for a terminal it considers unsupported. Both need the pty harness in
//! `scripts/alpine-vm-*.sh`.
//!
//! What is pinned here is everything else: which language a line is read as, the one-line
//! escapes, per-mode completeness, and how a failure in each is reported.

mod common;

use common::oslo_bin;
use std::io::Write;
use std::process::{Command, Stdio};

/// Type `input` at an interactive prompt and return everything it printed.
#[track_caller]
fn typed(input: &str, env: &[(&str, &str)]) -> String {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut child = Command::new(oslo_bin())
        .arg("-i")
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .env_remove("ENV")
        .env_remove("TERM")
        .envs(env.iter().copied())
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
    let output = child.wait_with_output().expect("wait");
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text
}

/// The same, with `config` installed as the session's `init.lua`.
///
/// **How Lua runs before a shell command now.** There is no prefix that reaches Lua from a shell
/// prompt, so anything that has to be in place *before* a command runs — a hook, a variable —
/// goes where a user would really put it: the config.
#[track_caller]
fn typed_with_config(config: &str, input: &str, env: &[(&str, &str)]) -> String {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join(".config/oslo")).expect("config dir");
    std::fs::write(dir.path().join(".config/oslo/init.lua"), config).expect("write config");
    let mut child = Command::new(oslo_bin())
        .arg("-i")
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .env_remove("ENV")
        .env_remove("TERM")
        .env_remove("XDG_CONFIG_HOME")
        .envs(env.iter().copied())
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
    let output = child.wait_with_output().expect("wait");
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text
}

#[test]
fn a_session_starts_in_shell_mode() {
    let out = typed("echo hello\n", &[]);
    assert!(out.contains("hello"), "{out}");
}

/// **`!` runs one line as Lua from a shell prompt**, without changing the mode.
#[test]
fn the_bang_prefix_runs_one_lua_line_from_shell_mode() {
    let out = typed("echo one\n!print(1 + 1)\necho two\n", &[]);
    let expected = ["one", "2", "two"];
    let lines: Vec<&str> = out
        .lines()
        .filter_map(|line| expected.iter().find(|want| line.ends_with(**want)).copied())
        .collect();
    assert_eq!(lines, vec!["one", "2", "two"], "{out}");
}

/// **What `!` still shares with history**: the characters no Lua expression can begin with, and —
/// by choice rather than deduction — every digit. A space is how you say you meant Lua.
#[test]
fn history_keeps_the_forms_no_lua_expression_can_start_with() {
    // `!!` re-runs the line before it: typed once, echoed by the expansion, run twice.
    let bang = typed("echo remembered\n!!\n", &[]);
    assert_eq!(
        bang.matches("remembered").count(),
        3,
        "!! should have echoed and re-run the line: {bang}"
    );

    // **A digit is an event number.** `!1` is the first line, not the Lua expression `1`.
    let first = typed("echo alpha\necho beta\n!1\n", &[]);
    assert_eq!(
        first.matches("alpha").count(),
        3,
        "!1 should have echoed and re-run the first line: {first}"
    );
    let back = typed("echo alpha\necho beta\n!-2\n", &[]);
    assert_eq!(
        back.matches("alpha").count(),
        3,
        "!-2 should reach two events back: {back}"
    );

    // **A space is the escape**, and it is the only way to write a Lua line that opens with a
    // number. It needs no rule of its own: a space is not a character history claims.
    let sum = typed("! 5 + 5\n", &[]);
    assert!(sum.lines().any(|line| line.trim() == "10"), "{sum}");
    let negative = typed("! -3 + 1\n", &[]);
    assert!(
        negative.lines().any(|line| line.trim() == "-2"),
        "{negative}"
    );

    // A letter was never ambiguous, so it still needs no space.
    let lua = typed("!print(6 * 7)\n", &[]);
    assert!(lua.lines().any(|line| line.trim() == "42"), "{lua}");

    // And `=` is oslo's own shorthand for where a program lives, which the old `=` prefix ate.
    let equals = typed("echo =ls\n", &[]);
    assert!(equals.contains("/ls"), "{equals}");
}

#[test]
fn the_default_mode_is_configurable() {
    let out = typed("print('from lua')\n", &[("OSLO_DEFAULT_MODE", "lua")]);
    assert!(out.contains("from lua"), "{out}");
}

/// **The Lua prompt has no prefix.** It is a REPL: every line is Lua, and a `!` is a syntax error
/// there exactly as it is in any other Lua interpreter. There used to be a `!` meaning "one shell
/// line"; running a program from Lua is `oslo.run`, and a second syntax for it was a second thing
/// to know.
#[test]
fn the_lua_prompt_takes_no_prefix() {
    let out = typed(
        "print('lua one')\n!echo shell\nprint('lua two')\n",
        &[("OSLO_DEFAULT_MODE", "lua")],
    );
    assert!(out.contains("lua one"), "{out}");
    assert!(out.contains("lua two"), "{out}");
    // The `!` line neither ran a command nor silently did nothing: it is Lua, and it does not parse.
    assert!(
        !out.contains("\nshell"),
        "the bang escaped to the shell: {out}"
    );
    assert!(out.contains("syntax error"), "{out}");
}

/// Running a program from a Lua prompt is `oslo.run`, which is the one way and always was.
#[test]
fn a_program_runs_from_lua_through_the_api() {
    let out = typed(
        "oslo.run{\"echo\", \"from-lua\"}\n",
        &[("OSLO_DEFAULT_MODE", "lua")],
    );
    assert!(out.contains("from-lua"), "{out}");
}

/// The two languages share one namespace, so a variable set in one is readable in the other. That
/// is the whole reason switching mid-session is useful.
///
/// **Shown without mixing languages on one line**, because nothing mixes them any more: the
/// shell's side is set in the environment or by a config, and the other side reads it as itself.
/// Every name here is one nothing else uses — an inherited variable of the same name would make
/// this pass or fail for a reason that has nothing to do with the crossing.
#[test]
fn a_variable_crosses_between_the_modes() {
    // Shell to Lua: what the session inherits is a Lua global.
    let out = typed(
        "print(crossing_in_zz)\n",
        &[("OSLO_DEFAULT_MODE", "lua"), ("crossing_in_zz", "hello")],
    );
    assert!(out.contains("hello"), "{out}");

    // Lua to shell: a global a config assigns is a shell variable.
    let back = typed_with_config(
        "crossing_out_zz = 'world'\n",
        "echo $crossing_out_zz\n",
        &[],
    );
    assert!(back.contains("world"), "{back}");
}

/// **An unfinished Lua chunk asks for another line, and an empty line ends the block.**
///
/// Python's rule, and it is there for Python's reason: after `if true then` the parser is satisfied
/// again at `end`, so running the moment it is satisfied would mean no line after `end` could ever
/// be typed — a block could never be extended, and `end` followed by anything was impossible.
#[test]
fn lua_mode_continues_an_unfinished_chunk() {
    let out = typed(
        "if true then\n  print('inside')\nend\n\n",
        &[("OSLO_DEFAULT_MODE", "lua")],
    );
    assert!(out.contains("inside"), "{out}");
}

/// **A one-liner still runs on Enter.** The empty line is what ends a block that has *begun*; it is
/// not a second key you have to press for `1 + 1`.
#[test]
fn a_lua_one_liner_runs_on_enter() {
    let out = typed("print(6 * 7)\n", &[("OSLO_DEFAULT_MODE", "lua")]);
    assert!(out.contains("42"), "{out}");
}

/// And a genuine mistake is reported instead of wedging the prompt waiting for more.
#[test]
fn a_lua_syntax_error_comes_back_rather_than_hanging() {
    let out = typed(
        "x = = 2\necho still here\n",
        &[("OSLO_DEFAULT_MODE", "lua")],
    );
    assert!(out.contains("syntax error"), "{out}");
}

#[test]
fn the_current_mode_is_published_for_the_prompt_to_read() {
    let out = typed("echo mode is $OSLO_MODE\n", &[]);
    assert!(out.contains("mode is sh"), "{out}");

    // From Lua, the same question is asked of the environment directly — there is no escape.
    let lua = typed(
        "print('mode is ' .. oslo.env.get('OSLO_MODE'))\n",
        &[("OSLO_DEFAULT_MODE", "lua")],
    );
    assert!(lua.contains("mode is lua"), "{lua}");
}

/// A Lua line that fails must not take the shell down — an interactive shell survives what would
/// end a script.
#[test]
fn a_failing_lua_line_leaves_the_prompt_up() {
    let out = typed(
        "error('boom')\nprint('still here')\n",
        &[("OSLO_DEFAULT_MODE", "lua")],
    );
    assert!(out.contains("boom"), "{out}");
    assert!(out.contains("still here"), "{out}");
}

/// `oslo.proc.exit` from the prompt ends the shell with the status it names.
#[test]
fn oslo_exit_from_lua_mode_ends_the_session() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut child = Command::new(oslo_bin())
        .arg("-i")
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .env("OSLO_DEFAULT_MODE", "lua")
        .env_remove("ENV")
        .env_remove("TERM")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn oslo");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(b"oslo.proc.exit(7)\nprint('not reached')\n")
        .expect("write");
    let output = child.wait_with_output().expect("wait");
    assert_eq!(output.status.code(), Some(7));
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("not reached"),
        "the shell kept reading after oslo.proc.exit"
    );
}

/// History expansion is shell syntax, and rewriting a Lua line against the history would corrupt
/// it — a `!` in Lua is half of `~=` and can appear inside any string.
#[test]
fn a_lua_line_is_not_rewritten_by_history_expansion() {
    let out = typed("print('a!b')\n", &[("OSLO_DEFAULT_MODE", "lua")]);
    assert!(out.contains("a!b"), "{out}");
}

#[test]
fn a_precmd_hook_sees_each_command_as_typed() {
    let out = typed_with_config(
        "oslo.on.precmd(function(c) print('PRE ' .. c.text .. ' in ' .. c.cwd) end)\n",
        "echo hi\n",
        &[],
    );
    assert!(out.contains("PRE echo hi in /"), "{out}");
    // The command still runs; a hook observes, it does not intercept.
    assert!(out.contains("\nhi"), "{out}");
}

/// The status, and the rest of what a hook needs to report a command: how long it took, where it
/// ended, and whether it worked. The status alone was all this used to be handed.
#[test]
fn a_postcmd_hook_is_handed_the_status() {
    let out = typed_with_config(
        "oslo.on.postcmd(function(c) print('POST ' .. c.text .. ' ' .. c.status \
         .. ' ' .. tostring(c.ok) .. ' ' .. type(c.duration_ms)) end)\n",
        "false\n",
        &[],
    );
    assert!(out.contains("POST false 1 false number"), "{out}");
}

/// **A command that failed still fires the hook.** It used to fire only on success, which left the
/// hook silent for exactly the commands one is usually installed to notice.
#[test]
fn a_postcmd_hook_fires_for_a_command_that_failed() {
    let out = typed_with_config(
        "oslo.on.postcmd(function(c) print('POST ' .. c.status) end)\n",
        "no-such-command-anywhere\n",
        &[],
    );
    assert!(out.contains("POST 127"), "{out}");
}

#[test]
fn a_cd_hook_fires_only_when_the_directory_changed() {
    let out = typed_with_config(
        "oslo.on.cd(function(d) print('CD ' .. d.to .. ' from ' .. d.from) end)\n",
        "echo not a cd\ncd /tmp\n",
        &[],
    );
    assert!(out.contains("CD /tmp"), "{out}");
    assert_eq!(out.matches("CD ").count(), 1, "{out}");
}

/// A handler written inline still has to be removable, which is the whole reason `oslo.on.*`
/// returns a handle rather than taking a name.
#[test]
fn a_hook_handle_removes_the_handler_it_stands_for() {
    // The handle is kept as a global by the config, so a later Lua line can reach it. Removing it
    // happens from a Lua prompt, which is where Lua now lives.
    let out = typed_with_config(
        "h = oslo.on.precmd(function(c) print('PRE ' .. c.text) end)\n",
        "echo one\n",
        &[],
    );
    assert!(out.contains("PRE echo one"), "{out}");

    // The hook fires for the removal line too — it runs *before* the command, and the command is
    // the removal. What has to stop is everything after it.
    let removed = typed_with_config(
        "h = oslo.on.precmd(function(c) print('PRE ' .. c.text) end)\n",
        "h:remove()\nprint('after')\n",
        &[("OSLO_DEFAULT_MODE", "lua")],
    );
    assert!(removed.contains("after"), "{removed}");
    assert!(
        !removed.contains("PRE print('after')"),
        "the handler fired after it was removed: {removed}"
    );
}

/// One broken hook must not disable the others, or silently stop the command from running.
#[test]
fn a_failing_hook_is_reported_and_the_rest_still_run() {
    let out = typed_with_config(
        "oslo.on.precmd(function() error('broken') end)\n\
         oslo.on.precmd(function() print('SECOND') end)\n",
        "echo ran\n",
        &[],
    );
    assert!(out.contains("broken"), "{out}");
    assert!(out.contains("SECOND"), "{out}");
    assert!(out.contains("\nran"), "{out}");
}

/// **The Lua prompt evaluates expressions and shows what they came to.**
///
/// It could not. A Lua chunk is a sequence of *statements*, so every one of `1 + 1`, `x * 2`,
/// `os.time`, `"a string"` and `{1,2}` was a syntax error — "expression is not a statement" — and
/// the one thing a prompt is mostly used for was the one thing it refused. Every Lua REPL solves
/// it the same way: try the line as the tail of a `return` first.
#[test]
fn the_lua_prompt_evaluates_an_expression_and_prints_it() {
    let lua = &[("OSLO_DEFAULT_MODE", "lua")];
    for (typed_line, wanted) in [
        ("1 + 1", "2"),
        ("2 ^ 10", "1024.0"),
        ("\"a \" .. \"string\"", "a string"),
        ("#(\"abc\")", "3"),
        ("math.max(3, 9)", "9"),
        ("(\"x\"):rep(3)", "xxx"),
    ] {
        let out = typed(&format!("{typed_line}\n"), lua);
        assert!(
            out.lines().any(|line| line.trim() == wanted),
            "`{typed_line}` should print {wanted:?}, printed {out:?}"
        );
    }
}

/// Several values print the way Lua prints several values, and none prints nothing.
#[test]
fn the_lua_prompt_shows_every_value_and_stays_quiet_for_none() {
    let lua = &[("OSLO_DEFAULT_MODE", "lua")];

    let several = typed("1, 2, 3\n", lua);
    assert!(
        several.lines().any(|line| line.trim() == "1\t2\t3"),
        "{several}"
    );

    // A statement is not an expression and must not grow an answer: `x = 5` prints nothing, and
    // `print` already printed, so wrapping it must not add a `nil` under what it said.
    let assigned = typed("x = 5\nx * 2\n", lua);
    assert!(
        assigned.lines().any(|line| line.trim() == "10"),
        "{assigned}"
    );
    assert!(
        !assigned.lines().any(|line| line.trim() == "nil"),
        "an assignment answered something: {assigned}"
    );

    // `print` already printed, so wrapping the line must not put a `nil` under what it said.
    let printed = typed("print(\"said\")\n", lua);
    assert_eq!(
        printed.lines().filter(|line| line.trim() == "said").count(),
        1,
        "print said it more than once: {printed}"
    );
    assert!(
        !printed.lines().any(|line| line.trim() == "nil"),
        "print grew an answer: {printed}"
    );
}

/// `nil` and `false` are answers, not silence — a prompt that hid them would be lying about what
/// the expression came to.
#[test]
fn the_lua_prompt_shows_a_falsey_answer() {
    let lua = &[("OSLO_DEFAULT_MODE", "lua")];
    for (typed_line, wanted) in [("nil", "nil"), ("false", "false"), ("1 == 2", "false")] {
        let out = typed(&format!("{typed_line}\n"), lua);
        assert!(
            out.lines().any(|line| line.trim() == wanted),
            "`{typed_line}` should print {wanted:?}: {out}"
        );
    }
}

/// **`exit` leaves a Lua prompt**, which is the word every shell answers and Lua has no meaning
/// for. As an expression it is an unset global, so the prompt printed `nil` and stayed open —
/// there was no way out that a shell user would guess.
#[test]
fn exit_leaves_the_lua_prompt() {
    let lua = &[("OSLO_DEFAULT_MODE", "lua")];
    let out = typed("exit\nprint(\"AFTER\")\n", lua);
    assert!(
        !out.contains("AFTER"),
        "the shell kept reading after exit: {out}"
    );
    assert!(!out.contains("nil"), "exit answered a value: {out}");
}

/// A line that is a whole statement still runs as one, and a failure still says where.
#[test]
fn a_statement_still_runs_and_a_failure_still_says_where() {
    let lua = &[("OSLO_DEFAULT_MODE", "lua")];
    let out = typed("local t = {} ; t.x = 1 ; print(t.x)\n", lua);
    assert!(out.lines().any(|line| line.trim() == "1"), "{out}");

    // One label and one position, where this used to read
    // `Lua error: (oslo lua): runtime error: (oslo lua):1: …`.
    let failed = typed("nosuchfn_zz()\n", lua);
    assert!(failed.contains("(oslo lua):1:"), "{failed}");
    assert!(
        !failed.contains("runtime error:"),
        "the VM's own category leaked: {failed}"
    );
    assert_eq!(
        failed.matches("(oslo lua)").count(),
        1,
        "the chunk was named twice: {failed}"
    );
}
