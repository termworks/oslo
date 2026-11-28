//! Running a shell program — a `-c` string, a script file or standard input — and the status the
//! shell ends with.

use super::stream;
use crate::cli::{Action, Invocation};
use oslo::env::Environment;
use oslo::env::builtins::run_exit_trap;
use oslo::env::options::ShellOption;
use oslo::error::ShellError;
use oslo::exec::eval_command_list;
use oslo::parser::parse_with_aliases;
use oslo_runtime::absorb_loop_control;
use oslo_runtime::startup;

/// Run `script` as this shell's program and exit with its status.
///
/// Every program is read a command at a time, as bash reads a file, a stream and a `-c` string:
/// the commands before a syntax error run. `frame` is the outermost `$FUNCNAME` entry, `main` for a
/// script file; `-c` and standard input get none, as in bash.
pub(super) fn run_program(invocation: &Invocation, script: &str, frame: Option<&str>) -> ! {
    let command_string = matches!(invocation.action, Action::Command(_));
    let mut env = Environment::new();
    if command_string {
        env.set_var("BASH_EXECUTION_STRING", script, false);
    }
    // The name first: pushing the frame publishes `$BASH_SOURCE`, whose outermost entry it is.
    env.shell_name = invocation.name.clone();
    if let Some(frame) = frame {
        env.enter_script_frame(frame);
    }
    env.set_positional(invocation.positional.clone());
    env.publish_startup_args();
    apply_invocation_options(&mut env, invocation);
    // A signal ignored when the shell started stays ignored, and `trap` lists it so.
    if !invocation.force_interactive {
        oslo::exec::job::adopt_ignored_at_entry(&mut env);
    }
    startup::history::register(&mut env);
    // Universal variables reach a script too, before the startup files so `$ENV` can read one.
    #[cfg(feature = "universal")]
    oslo::env::universal::sync_into(&mut env);

    // R9.10: a non-interactive shell still reads `$ENV`, before the program.
    if let Some(status) = startup::rc::load_startup_files(
        &mut env,
        invocation.force_interactive,
        invocation.login,
        invocation.no_profile,
    ) {
        std::process::exit(run_exit_trap(&mut env, status));
    }

    // `-c` only, and only when `$OSLO_ALLHIST` asks: a script's contents are not typed commands.
    if command_string && startup::history::record_commands() {
        startup::history::record_command(&env, script);
    }

    let status = run_streamed(&mut env, script);
    // R6.5: every way out of a script converges here, so the EXIT trap fires on each of them.
    std::process::exit(run_exit_trap(&mut env, status));
}

/// Run a program: whole if it parses, a command at a time if it does not.
///
/// A program that parses has no syntax error to be partial about, so running it whole is the same
/// as running it a command at a time and parsing once is far cheaper — a one-megabyte
/// here-document is one command, and re-classifying it line by line is quadratic. `-v` and `-t`
/// are the exceptions, since each acts on the reading itself.
fn run_streamed(env: &mut Environment, script: &str) -> i32 {
    if !oslo::exec::chunks::needs_chunks(env, script)
        && let Ok(ast) = parse_with_aliases(script, !env.get_aliases().is_empty(), &|n| {
            env.get_alias(n).map(str::to_string)
        })
    {
        if env.option(ShellOption::CommandString) {
            oslo::exec::simple::program_ends_the_shell();
        }
        return match absorb_loop_control(eval_command_list(env, &ast)) {
            Ok(status) => status,
            Err(e) => exit_error_status(env, e),
        };
    }
    stream::run_line_at_a_time(env, script)
}

/// Parse one piece of program text and run it.
///
/// `Err` carries the status the shell should exit with. `whole` is the program the chunk came
/// from, so a syntax error is reported against the program rather than against the piece.
pub(super) fn run_chunk(
    env: &mut Environment,
    source: &str,
    whole: &str,
) -> std::result::Result<i32, i32> {
    let ast = match parse_with_aliases(source, !env.get_aliases().is_empty(), &|n| {
        env.get_alias(n).map(str::to_string)
    }) {
        Ok(ast) => ast,
        Err(e) => {
            // A positioned error names its own line; one at end of input names the line the chunk
            // began on, where the construct that never closed starts.
            let body = e.to_string();
            match oslo_shell::env::parsed_position(&body) {
                Some((line, _)) => env.note_line(line as u32),
                None => env.note_line(1),
            }
            if !oslo_shell::env::complain_at(
                &env.origin(),
                env.diagnostic_source(),
                whole,
                env.line_offset(),
                &body,
            ) {
                eprintln!("{}{}", env.origin(), e);
            }
            return Err(e.failure_status());
        }
    };
    match absorb_loop_control(eval_command_list(env, &ast)) {
        Ok(status) => Ok(status),
        Err(e) => Err(exit_error_status(env, e)),
    }
}

/// Put the invocation's own flags into the option set, so `$-` describes this shell.
///
/// `-e`/`-x` are ordinary options a script could also set, while `c` and `s` say where the program
/// came from and no `set` command can change them.
fn apply_invocation_options(env: &mut Environment, invocation: &Invocation) {
    for option in invocation.options() {
        env.set_option(option, true);
    }
    // The `+` forms after the `-` ones, so `sh -x +x` ends up off.
    for option in invocation.unset_options() {
        env.set_option(option, false);
    }
    match invocation.action {
        Action::Command(_) => env.set_option(ShellOption::CommandString, true),
        Action::Stdin => env.set_option(ShellOption::StdinInput, true),
        _ => {}
    }
    if invocation.force_interactive {
        env.set_option(ShellOption::Interactive, true);
        oslo::env::builtins::force_shopt("expand_aliases", true);
    }
    oslo::env::builtins::force_shopt("login_shell", invocation.login);
    env.publish_option_vars();
}

/// The status a non-interactive shell ends with after it could not finish its script.
///
/// Returns rather than exiting, so the EXIT trap still runs. `-c` and a script file end
/// differently, as in bash:
///
/// ```text
///   bash -c 'set -u; echo $nope'   ->  127        bash file  ->  1
///   bash -c 'echo $(if)'           ->  127        bash file  ->  2
///   bash -c 'set -eu; echo $nope'  ->  1
/// ```
pub(super) fn exit_error_status(env: &Environment, err: ShellError) -> i32 {
    match err {
        ShellError::Exit(code) => code,
        e => {
            // `set -e` unwinds an unset variable as a failed command, with that command's status.
            let unwound_by_errexit =
                env.option(ShellOption::ErrExit) && matches!(e, ShellError::UnsetParameter(_));
            let status = match env.option(ShellOption::CommandString) && !unwound_by_errexit {
                true => e.fatal_exit_status(env.posix()),
                false => e.failure_status(),
            };
            eprintln!("{}{}", env.origin(), e);
            status
        }
    }
}
