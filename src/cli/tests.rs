//! What each invocation means.

use super::*;

fn parse_args(args: &[&str]) -> Result<Invocation, Exit> {
    let argv: Vec<String> = std::iter::once("oslo")
        .chain(args.iter().copied())
        .map(str::to_string)
        .collect();
    parse(&argv)
}

#[test]
fn no_arguments_reads_stdin() {
    let inv = parse_args(&[]).expect("parse");
    assert_eq!(inv.action, Action::Stdin);
    assert_eq!(inv.name, "oslo");
    assert!(inv.positional.is_empty());
}

#[test]
fn dash_c_takes_the_next_argument() {
    let inv = parse_args(&["-c", "echo hi"]).expect("parse");
    assert_eq!(inv.action, Action::Command("echo hi".to_string()));
}

#[test]
fn dash_c_takes_an_attached_argument() {
    let inv = parse_args(&["-cecho hi"]).expect("parse");
    assert_eq!(inv.action, Action::Command("echo hi".to_string()));
}

#[test]
fn dash_c_without_an_argument_is_a_usage_error() {
    let err = parse_args(&["-c"]).expect_err("must not start a REPL");
    assert_eq!(err.status, 2);
    assert!(err.to_stderr);
    assert!(
        err.message.contains("requires an argument"),
        "{}",
        err.message
    );
}

#[test]
fn dash_c_operands_supply_zero_and_positionals() {
    let inv = parse_args(&["-c", "echo", "myname", "a", "b"]).expect("parse");
    assert_eq!(inv.name, "myname");
    assert_eq!(inv.positional, vec!["a".to_string(), "b".to_string()]);
}

#[test]
fn script_operand_becomes_zero_and_the_rest_positional() {
    let inv = parse_args(&["run.sh", "one", "two"]).expect("parse");
    assert_eq!(inv.action, Action::Script("run.sh".to_string()));
    assert_eq!(inv.name, "run.sh");
    assert_eq!(inv.positional, vec!["one".to_string(), "two".to_string()]);
}

#[test]
fn dash_c_ends_option_parsing_at_its_command_string() {
    // `find -exec sh -c '…' -- {} +` passes `--` as `$0`; treating it as the end of options
    // would shift every positional by one and silently run the command against nothing.
    let inv = parse_args(&["-c", "echo", "--", "x", "y"]).expect("parse");
    assert_eq!(inv.name, "--");
    assert_eq!(inv.positional, vec!["x".to_string(), "y".to_string()]);

    // Nor is a later `-x` an option any more: it is `$1`, and tracing stays off.
    let inv = parse_args(&["-c", "echo", "name", "-x", "z"]).expect("parse");
    assert_eq!(inv.name, "name");
    assert_eq!(inv.positional, vec!["-x".to_string(), "z".to_string()]);
    assert_eq!(inv.set_options, "");
}

#[test]
fn options_before_dash_c_are_still_options() {
    let inv = parse_args(&["-x", "-c", "echo", "name"]).expect("parse");
    assert_eq!(inv.set_options, "x");
    assert_eq!(inv.name, "name");
}

#[test]
fn clustered_options_are_all_seen() {
    let inv = parse_args(&["-eix", "run.sh"]).expect("parse");
    assert!(inv.force_interactive);
    assert_eq!(inv.set_options, "ex");
    assert_eq!(inv.action, Action::Script("run.sh".to_string()));
}

#[test]
fn double_dash_ends_options() {
    let inv = parse_args(&["--", "-weird-name.sh"]).expect("parse");
    assert_eq!(inv.action, Action::Script("-weird-name.sh".to_string()));
}

#[test]
fn unknown_short_option_is_a_usage_error() {
    let err = parse_args(&["-Z"]).expect_err("unknown option");
    assert_eq!(err.status, 2);
    assert!(
        err.message.contains("-Z: invalid option"),
        "{}",
        err.message
    );
}

#[test]
fn unknown_long_option_is_a_usage_error() {
    let err = parse_args(&["--nope"]).expect_err("unknown option");
    assert_eq!(err.status, 2);
    assert!(
        err.message.contains("--nope: invalid option"),
        "{}",
        err.message
    );
}

#[test]
fn version_and_help_exit_zero_on_stdout() {
    let v = parse_args(&["--version"]).expect_err("terminates");
    assert_eq!(v.status, 0);
    assert!(!v.to_stderr);
    assert!(v.message.contains(env!("CARGO_PKG_VERSION")));

    let h = parse_args(&["--help"]).expect_err("terminates");
    assert_eq!(h.status, 0);
    assert!(!h.to_stderr);
    assert!(h.message.contains("USAGE"), "{}", h.message);
    assert!(h.message.contains("config"), "the tools are listed");
}

/// `--details` is the long form, and works written either side of `--help` — nobody should have
/// to remember an order.
#[test]
fn details_is_the_long_help_in_either_order() {
    let short = parse_args(&["--help"]).expect_err("terminates").message;
    for spelling in [
        vec!["--help", "--details"],
        vec!["--details", "--help"],
        vec!["--details"],
    ] {
        let long = parse_args(&spelling).expect_err("terminates");
        assert_eq!(long.status, 0, "{spelling:?}");
        assert!(!long.to_stderr, "{spelling:?}");
        assert!(
            long.message.len() > short.len(),
            "{spelling:?}: no more than the short help"
        );
        assert!(
            long.message.contains("xtrace"),
            "{spelling:?}: no option reference"
        );
    }
}

/// Past `--`, a `--details` is somebody's argument. Consulting the whole command line for it
/// would make `oslo --help -- --details` mean something different from what was written.
#[test]
fn details_after_a_double_dash_is_not_a_flag() {
    let h = parse_args(&["--help", "--", "--details"]).expect_err("terminates");
    assert!(!h.message.contains("xtrace"), "it was read as a flag");
}

/// `oslo history` reaches the tool, and its arguments come with it.
#[test]
fn a_tool_can_be_named_in_the_operand_slot() {
    let inv = parse_args(&["history", "search", "-n", "5"]).expect("parse");
    assert_eq!(
        inv.action,
        Action::Tool(
            "history".to_string(),
            vec!["search".to_string(), "-n".to_string(), "5".to_string()]
        )
    );
}

/// **`-o name` on the command line.** POSIX's synopsis is
/// `sh [-abCefhimnuvx] [-o option]... [+abCefhimnuvx] [+o option]...`, and it is the *only* way to
/// ask for an option with no letter — `pipefail` and `posix` are both spelled this way.
///
/// oslo refused `-o` outright, so `sh -o pipefail -c '…'` did not start at all. Found by running
/// the invocation shapes another program would use, not the ones a script uses.
#[test]
fn dash_o_names_an_option() {
    let inv = parse_args(&["-o", "pipefail", "-c", "echo hi"]).expect("parse");
    assert!(inv.options().any(|o| o == ShellOption::PipeFail));
    assert_eq!(inv.action, Action::Command("echo hi".to_string()));

    // Attached, as a getopt-style caller may write it.
    let inv = parse_args(&["-onounset", "-c", "echo hi"]).expect("parse");
    assert!(inv.options().any(|o| o == ShellOption::NoUnset));

    // A name that is not an option is refused rather than ignored.
    let err = parse_args(&["-o", "bogus"]).expect_err("must be refused");
    assert_eq!(err.status, 2);
    assert!(err.message.contains("bogus"), "{}", err.message);
}

/// **`+x` and `+o name` turn options off**, and are options rather than operands. Falling through
/// to the operand branch made `sh +x -c 'cmd'` go looking for a *script* named `+x`.
#[test]
fn plus_turns_an_option_off() {
    let inv = parse_args(&["+o", "nounset", "-c", "echo hi"]).expect("parse");
    assert!(inv.unset_options().any(|o| o == ShellOption::NoUnset));
    assert_eq!(
        inv.action,
        Action::Command("echo hi".to_string()),
        "`+o name` was read as an operand"
    );

    let inv = parse_args(&["+x", "-c", "echo hi"]).expect("parse");
    assert!(inv.unset_options().any(|o| o == ShellOption::XTrace));

    // Both spellings of the same option, `-` then `+`: the caller asked for off last.
    let inv = parse_args(&["-x", "+x", "-c", "echo hi"]).expect("parse");
    assert!(inv.options().any(|o| o == ShellOption::XTrace));
    assert!(inv.unset_options().any(|o| o == ShellOption::XTrace));

    let err = parse_args(&["+q"]).expect_err("must be refused");
    assert_eq!(err.status, 2);
}

/// **`-c`'s argument is the first *non-option* argument.** Options may sit between them.
///
/// Both spellings that broke came from programs *calling* the shell rather than from scripts, so
/// no corpus could reach either:
///
/// - `sh -c -l '<cmd>'` — Claude Code invokes its shell exactly this way when it has no
///   environment snapshot to source. oslo ran `-l` and answered "command not found" for **every**
///   command, which is how this was found: it broke the tool being used to write it.
/// - `sh -c -- cmd` — musl's `system(3)` is `execl("/bin/sh", "sh", "-c", "--", cmd, 0)`, so every
///   `system()` call on the machine failed the moment `/bin/sh` pointed at oslo.
#[test]
fn dash_c_takes_the_first_non_option_argument() {
    for before in [vec!["-l"], vec!["-x"], vec!["--"], vec!["-l", "-x"], vec![]] {
        let mut args = vec!["-c"];
        args.extend(before.iter().copied());
        args.push("echo hi");
        let inv = parse_args(&args).expect("parse");
        assert_eq!(
            inv.action,
            Action::Command("echo hi".to_string()),
            "with {before:?} between -c and the program"
        );
    }

    // The options in between still take effect, rather than being skipped over.
    let inv = parse_args(&["-c", "-x", "echo hi"]).expect("parse");
    assert!(inv.options().any(|o| o == ShellOption::XTrace));
    let inv = parse_args(&["-c", "-l", "echo hi"]).expect("parse");
    assert!(inv.login, "-l after -c is still a login shell");

    // And with nothing but options, there is no program: an error, not a shell that runs `-l`.
    let err = parse_args(&["-c", "-l"]).expect_err("must be refused");
    assert_eq!(err.status, 2);
    assert!(
        err.message.contains("requires an argument"),
        "{}",
        err.message
    );
}

/// Operands after the program text are untouched: the first is `$0`, and a `--` among them is an
/// ordinary word — which is what `find -exec sh -c '…' -- {} +` depends on.
#[test]
fn a_double_dash_before_the_command_string_ends_the_options() {
    let inv = parse_args(&["-c", "--", "echo hi"]).expect("parse");
    assert_eq!(inv.action, Action::Command("echo hi".to_string()));

    // The operands after it still land where they did: the next one is `$0`.
    let inv = parse_args(&["-c", "--", "echo $0", "zero", "one"]).expect("parse");
    assert_eq!(inv.action, Action::Command("echo $0".to_string()));
    assert_eq!(inv.name, "zero");
    assert_eq!(inv.positional, vec!["one".to_string()]);
}

/// And a `--` *after* the command string is an ordinary operand that becomes `$0`, which is what
/// `find -exec sh -c '…' -- {} +` depends on. The two positions must not be confused.
#[test]
fn a_double_dash_after_the_command_string_is_still_dollar_zero() {
    let inv = parse_args(&["-c", "echo hi", "--", "x"]).expect("parse");
    assert_eq!(inv.action, Action::Command("echo hi".to_string()));
    assert_eq!(inv.name, "--");
    assert_eq!(inv.positional, vec!["x".to_string()]);
}

/// **`--` says the word is a path.** The escape hatch for the day somebody has a script named
/// `hook`, and the reason the operand slot is not silently narrowed.
#[test]
fn a_double_dash_forces_the_operand_to_be_a_script() {
    let inv = parse_args(&["--", "history"]).expect("parse");
    assert_eq!(inv.action, Action::Script("history".to_string()));
}

/// A tool name after `-c` is `$0`, not a tool. Everything past `-c`'s command string is an
/// operand, and `find -exec sh -c '…' -- {} +` depends on that staying true.
#[test]
fn a_tool_name_is_not_special_after_dash_c() {
    let inv = parse_args(&["-c", "echo hi", "history", "arg"]).expect("parse");
    assert_eq!(inv.action, Action::Command("echo hi".to_string()));
    assert_eq!(inv.name, "history", "it is $0");
    assert_eq!(inv.positional, vec!["arg".to_string()]);
}

/// A path is a script however it is spelled, so a `#!/bin/oslo` script keeps working whatever it
/// is called — the kernel always hands over a slashed path.
#[test]
fn a_slashed_tool_name_is_a_script() {
    let inv = parse_args(&["./history"]).expect("parse");
    assert_eq!(inv.action, Action::Script("./history".to_string()));
}

#[test]
fn dash_s_reads_stdin_with_positionals() {
    let inv = parse_args(&["-s", "a", "b"]).expect("parse");
    assert_eq!(inv.action, Action::Stdin);
    assert_eq!(inv.positional, vec!["a".to_string(), "b".to_string()]);
}

/// `--posix` was not an option at all: `oslo --posix -c 'echo x'` was a usage error, which is
/// what made the whole POSIX-mode code path unreachable in production.
#[test]
fn posix_is_a_long_option_and_reaches_the_option_set() {
    let inv = parse_args(&["--posix", "-c", "echo x"]).expect("parse");
    assert_eq!(inv.action, Action::Command("echo x".to_string()));
    assert!(inv.options().any(|o| o == ShellOption::Posix));
    // It has no letter, so it must not have leaked into the `$-` string either.
    assert_eq!(inv.set_options, "");
}

/// `options()` is the single reader, so a mixed command line yields both spellings.
#[test]
fn options_yields_letters_and_long_names_together() {
    let inv = parse_args(&["--posix", "-ex", "run.sh"]).expect("parse");
    let opts: Vec<_> = inv.options().collect();
    assert!(opts.contains(&ShellOption::ErrExit));
    assert!(opts.contains(&ShellOption::XTrace));
    assert!(opts.contains(&ShellOption::Posix));
}

/// Repeats collapse, as they do for letters.
#[test]
fn a_repeated_long_option_is_recorded_once() {
    let inv = parse_args(&["--posix", "--posix"]).expect("parse");
    assert_eq!(inv.long_options, vec![ShellOption::Posix]);
}

/// The long-name flags are an explicit list, not "everything `set -o` accepts": bash rejects
/// `--errexit`, and accepting it would be inventing an interface.
#[test]
fn a_set_o_name_is_not_automatically_a_long_option() {
    let err = parse_args(&["--errexit"]).expect_err("not a command-line flag");
    assert!(
        err.message.contains("--errexit: invalid option"),
        "{}",
        err.message
    );
}

/// A Lua file is an ordinary operand. There is no flag to run Lua, which is the point:
/// `--lua-script FILE` used to be the only way, in a shell whose scripting language is Lua.
#[test]
fn a_lua_file_is_just_a_script_operand() {
    let inv = parse_args(&["init.lua"]).expect("parse");
    assert_eq!(inv.action, Action::Script("init.lua".to_string()));
    assert_eq!(inv.name, "init.lua");
}

/// **The language flags are gone and are refused.** Detection is the feature — a shebang, then an
/// extension, then the text — and a flag that was accepted and ignored would run a Lua file as
/// shell and report a syntax error in a file that has none.
#[test]
fn the_language_flags_are_gone() {
    for flag in ["--lua", "--sh"] {
        let err = parse_args(&[flag, "script"]).expect_err("must be refused");
        assert!(
            err.message.contains("invalid option"),
            "{flag}: {}",
            err.message
        );
        assert_eq!(err.status, 2);
    }
}

/// **The editing mode is config, not a flag.** `oslo.vi.enabled` is the one place it lives, so
/// there is no way for a command line and a config file to disagree about it.
#[test]
fn the_vi_flags_are_gone() {
    for flag in ["--vi", "--vim", "--no-vi", "--no-vim"] {
        let err = parse_args(&[flag]).expect_err("must be refused");
        assert!(
            err.message.contains("invalid option"),
            "{flag}: {}",
            err.message
        );
        assert_eq!(err.status, 2);
    }
}

/// The flag it replaced must not linger as a silently-accepted no-op.
#[test]
fn the_old_lua_script_flag_is_gone() {
    let err = parse_args(&["--lua-script", "init.lua"]).expect_err("must be rejected");
    assert!(
        err.message.contains("--lua-script: invalid option"),
        "{}",
        err.message
    );
}

/// The flag that used to name a profile is *refused*, not quietly ignored. `$OSLO_PROFILE` is the
/// only spelling now, and a shell that accepted `--profile=claude` and then wrote to the default
/// store would mix an agent's history into yours without ever saying so.
#[test]
fn the_profile_flag_is_gone() {
    for args in [
        vec!["--profile=claude"],
        vec!["--profile"],
        vec!["--profile="],
    ] {
        let err = parse_args(&args).expect_err("must be refused");
        assert_eq!(err.status, 2, "{args:?}");
        assert!(err.message.contains("invalid option"), "{}", err.message);
    }
}

/// **Called as `sh`, oslo is a POSIX shell without being told.** bash has done this since 1989 —
/// the same binary is lax as `bash` and strict as `sh` — and it is what lets a distro point
/// `/bin/sh` at oslo and have every `#!/bin/sh` script get POSIX behaviour with no flag anywhere.
#[test]
fn being_called_sh_means_posix() {
    for argv0 in ["sh", "/bin/sh", "/usr/bin/sh", "-sh"] {
        let inv = parse_with_name(argv0, &[]).expect("parse");
        assert!(
            inv.options().any(|o| o == ShellOption::Posix),
            "{argv0} must imply posix"
        );
    }
}

/// And any other name is not POSIX mode, or `--posix` would mean nothing and the differential
/// suite would have no way to run the same corpus both ways.
#[test]
fn other_names_are_not_posix() {
    for argv0 in ["oslo", "/usr/bin/oslo", "-oslo", "rush", "shell", "bash"] {
        let inv = parse_with_name(argv0, &[]).expect("parse");
        assert!(
            !inv.options().any(|o| o == ShellOption::Posix),
            "{argv0} must not imply posix"
        );
    }
    // The flag still reaches it under any name.
    let inv = parse_with_name("oslo", &["--posix"]).expect("parse");
    assert!(inv.options().any(|o| o == ShellOption::Posix));
}

/// `parse`, with `argv[0]` chosen — the helper above always says `oslo`.
fn parse_with_name(argv0: &str, args: &[&str]) -> Result<Invocation, Exit> {
    let mut argv = vec![argv0.to_string()];
    argv.extend(args.iter().map(|a| a.to_string()));
    parse(&argv)
}

/// **bash's "be only a shell" flags are accepted, and mean what they say.**
///
/// Every program that hands a script to `bash` passes them — direnv runs `.envrc` under
/// `--noprofile --norc` — and a machine where `bash` on `$PATH` is a link to oslo is a supported
/// install. Refusing them by name made oslo unusable for exactly those callers, and the caller had
/// no way to tell "unknown flag" from "about to ignore your request".
#[test]
fn the_bash_startup_flags_are_honoured() {
    let inv = parse_args(&["--norc", "-c", "echo hi"]).expect("parse");
    assert!(inv.no_rc, "--norc must be recorded");
    assert!(!inv.no_profile, "and must not imply the other");

    let inv = parse_args(&["--noprofile", "--norc", "-c", "echo hi"]).expect("parse");
    assert!(inv.no_rc && inv.no_profile, "both, in bash's own order");

    let inv = parse_args(&["-c", "echo hi"]).expect("parse");
    assert!(
        !inv.no_rc && !inv.no_profile,
        "and neither is on by default"
    );
}

/// `--rcfile` and `--init-file` stay refused: they name a *bash* rc file, and pointing oslo at one
/// would source shell syntax into a shell whose configuration is Lua. A refusal is the honest
/// answer, unlike for the two flags above where oslo can do what was asked.
#[test]
fn an_rcfile_is_still_refused() {
    for flag in ["--rcfile", "--init-file"] {
        let err = parse_args(&[flag, "/tmp/x", "-c", "true"]).expect_err("must be refused");
        assert_eq!(err.status, 2, "{flag}");
    }
}

/// `env -S` hands the modes over one word each; a kernel hands them over as one.
#[test]
fn the_modes_arrive_either_way_a_shebang_sends_them() {
    for argv in [
        vec!["--func", "--jit", "pure.lua", "a"],
        vec!["--func --jit", "pure.lua", "a"],
    ] {
        let inv = parse_args(&argv).expect("parse");
        assert_eq!(
            inv.action,
            Action::Script("pure.lua".to_string()),
            "{argv:?}"
        );
        assert_eq!(inv.positional, vec!["a".to_string()], "{argv:?}");
        assert!(inv.modes.func && inv.modes.jit, "{argv:?}");
    }
    let err = parse_args(&["--func --nope", "pure.lua"]).expect_err("not all modes");
    assert!(err.message.contains("invalid option"), "{}", err.message);
}

#[test]
fn a_pure_program_cannot_be_dash_c() {
    let err = parse_args(&["--func", "-c", "return 1"]).expect_err("refused");
    assert!(err.message.contains("--func"), "{}", err.message);
    assert!(
        parse_args(&["--jit", "-c", "true"]).is_ok(),
        "--jit with -c is harmless"
    );
}
