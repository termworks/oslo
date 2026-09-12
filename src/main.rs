//! The `oslo` binary: argument handling, script execution, and the interactive REPL.

mod cli;
mod handoff;

// Startup, the Lua API and history expansion all live at the top of the stack now, in
// `oslo-runtime`. History expansion in particular must stay reachable only from the interactive
// prompt — it rewrites a line before it is parsed, so a `-c` or a script able to reach it would let
// data turn into a different command — and being above everything the shell can be driven through
// is what makes that impossible rather than merely unlikely.
use oslo_runtime::absorb_loop_control;
use oslo_runtime::startup;

use cli::{Action, Invocation};
use handoff::{block_every_signal, inherited_stack_limit, restore_signal_mask};
use oslo::env::Environment;
use oslo::env::builtins::run_exit_trap;
use oslo::env::options::ShellOption;
use oslo::error::ShellError;
use oslo::exec::eval_command_list;
use oslo::parser::parse_with_aliases;
use startup::language::{self, Language};
use std::env;
use std::fs;
use std::io::Read;

/// Undo the Rust runtime's `SIG_IGN` for SIGPIPE, which a shell must not have.
///
/// Rust ignores SIGPIPE before `main` so that a write to a closed pipe surfaces as an `EPIPE`
/// error rather than killing the process. For an ordinary program that is a kindness; for a shell
/// it is a hang. `oslo -c 'while :; do echo x; done' | head -1` ran for ever, because nothing ever
/// told the loop that its reader had gone — bash exits immediately. It also made `kill -s PIPE $$`
/// a no-op, which is how modernish detects the condition and why it refuses to load `var/loop`
/// on such a shell.
///
/// Children already got `SIG_DFL` back (see [`oslo::exec::JobManager`]); this is the shell's own
/// disposition, and it has to be set here rather than in the library because the test binary links
/// the library and would arm *itself* to die on any write to a closed pipe.
fn restore_default_sigpipe() {
    // Safety: called before any thread is started and before anything is written, and `signal(2)`
    // with `SIG_DFL` touches nothing but this process's own disposition table.
    unsafe {
        let _ = nix::sys::signal::signal(
            nix::sys::signal::Signal::SIGPIPE,
            nix::sys::signal::SigHandler::SigDfl,
        );
    }
}

/// Report how many structured pipeline edges this run planned, when asked.
///
/// The differential corpus runs oslo as a subprocess, so the in-process counter is not visible to
/// it. With `OSLO_AUDIT_STRUCTURED=1` the count is written to stderr as the process ends, and the
/// corpus asserts it is zero for every POSIX script — which is what turns "structure cannot affect
/// a script written before oslo existed" into a test rather than a promise.
fn report_structured_audit() {
    if std::env::var("OSLO_AUDIT_STRUCTURED").is_err() {
        return;
    }
    extern "C" fn report() {
        // `eprintln!` is not signal-safe, but `atexit` handlers run on a normal return from the
        // process rather than from a handler, so this is an ordinary write.
        eprintln!(
            "oslo-audit: structured-edges={}",
            oslo::data::entered_structured_path()
        );
    }
    // Registered rather than called at the end of `main`: nearly every path out of this shell is a
    // `process::exit` from somewhere deeper, and a report that only fires on one of them would
    // give a clean answer for the wrong reason.
    // SAFETY: `report` is `extern "C"`, takes nothing, returns nothing, and touches only an
    // atomic and stderr.
    unsafe {
        nix::libc::atexit(report);
    }
}

fn main() {
    // **The binary's version, for everything that reports one.** Every crate has a
    // `CARGO_PKG_VERSION` of its own and they differ; this is the one a release is tagged with, and
    // installing it here is what stops `oslo --version` and `oslo.version` disagreeing.
    oslo::version::install(env!("CARGO_PKG_VERSION"));
    // Before any thread exists, as the safety note on the function requires.
    restore_default_sigpipe();
    #[cfg(all(feature = "watch", feature = "scratch"))]
    {
        let args: Vec<String> = arguments();
        if let Some(status) = cli::watch::bootstrap(&args) {
            std::process::exit(status);
        }
    }
    report_structured_audit();
    // The names that can carry structure. Declared once, here, for every mode the shell runs in —
    // a script and a prompt must agree about what `df` is.
    oslo::data::tools::register_all();
    // And that a sourced file may be Lua. Beside the line above for the same reason: a script and a
    // prompt must agree about what `source tools.lua` does.
    startup::lua_init::install_source_language();

    // The shell runs on a stack oslo chose rather than one it inherited; see
    // [`oslo::INTERPRETER_STACK`]. `main` itself does nothing afterwards but wait.
    //
    // The signal mask around the handoff is [`handoff`]'s, and why it matters is documented there.
    let inherited = block_every_signal();
    let worker = std::thread::Builder::new()
        .name("oslo".to_string())
        .stack_size(oslo::INTERPRETER_STACK)
        .spawn(move || {
            // First, so the base is the top of this thread rather than partway down it. What it is
            // for is in `oslo_base::stack`: the constructs that re-enter the interpreter ask how
            // much is left instead of each counting its own levels against a limit of its own.
            oslo_base::stack::mark(oslo::INTERPRETER_STACK);
            // Anything raised in the gap is merely pending, and arrives the moment this returns.
            restore_signal_mask(&inherited);
            dispatch();
        });

    // **A shell that cannot spawn a thread still has to be a shell.** Under `ulimit -u` or a low
    // pids cgroup this fails with `EAGAIN`, and `.expect` made that a panic — so oslo would not
    // start where bash and dash both run the script. All the worker buys is a 16 MiB stack, and
    // both ways of recursing are bounded anyway (`nesting::MAX_INPUT_NESTING`, and the
    // nested-script counter), so the fallback runs the same programs with less headroom.
    let Ok(worker) = worker else {
        // The fallback has whatever stack the process was given, which is `ulimit -s` and usually
        // half the worker's. Marked with that rather than with `INTERPRETER_STACK`, or the guard
        // would be measuring against a budget this thread does not have.
        oslo_base::stack::mark(inherited_stack_limit());
        restore_signal_mask(&inherited);
        dispatch();
        return;
    };

    if worker.join().is_err() {
        // The worker panicked and has already printed its message.
        std::process::exit(2);
    }
}

fn dispatch() {
    let args: Vec<String> = arguments();

    let invocation = match cli::parse(&args) {
        Ok(inv) => inv,
        Err(exit) => {
            if exit.to_stderr {
                eprintln!("{}", exit.message.trim_end());
            } else {
                println!("{}", exit.message.trim_end());
            }
            std::process::exit(exit.status);
        }
    };

    match invocation.action {
        // Not a shell invocation: oslo is the `argc` binary for one command, prints and exits.
        #[cfg(feature = "argc")]
        cli::Action::ArgcEval(ref words) => std::process::exit(cli::argc::eval(words)),
        // `oslo history …` — reached only when no file of that name exists, so this never takes
        // an invocation a script could have wanted. See `cli::tools::as_operand`.
        //
        // **A tool starts no session.** It is a child of the shell that ran it and belongs to that
        // shell's session, which it inherits through `$OSLO_SESSION` — see `track::session`. This is
        // why the id is stamped *here*, on the shell arms, rather than when an `Environment` is
        // built: a tool builds one too.
        Action::Tool(ref name, ref args) => {
            let tool = cli::tools::from_name(name).expect("the parser only names tools it found");
            std::process::exit(cli::tools::run(tool, args));
        }
        // **`-c` is always shell.** Every `sh -c` idiom in the world depends on it, and no amount
        // of detection is worth being wrong about that one.
        Action::Command(ref text) => {
            begin_shell(false);
            run_program(&invocation, text)
        }
        // A script operand names a file whose language is worked out from the file itself.
        Action::Script(ref path) => {
            begin_shell(false);
            run_script(&invocation, path)
        }
        Action::Stdin => {
            // The same test `run_stdin` makes a moment later, asked here because the depth stack is
            // a stack of shells somebody is typing at and nothing else belongs on it.
            begin_shell(invocation.force_interactive || stdin_is_a_terminal());
            run_stdin(&invocation)
        }
    }
}

/// What every invocation that *is* a shell says about itself before it runs anything.
///
/// Both are stamped here rather than when an `Environment` is built, because a tool builds one too
/// and is not a shell: it belongs to the session that started it and stands at that session's
/// depth. See `track::session` and `track::nested`.
fn begin_shell(interactive: bool) {
    oslo::track::session::begin();
    oslo::track::nested::begin(interactive);
}

/// A script operand: read it, work out its language, run it.
fn run_script(invocation: &cli::Invocation, path: &str) -> ! {
    match read_script(path) {
        Ok(script) => match language::detect(Some(path), &script) {
            Language::Lua => std::process::exit(startup::lua_init::run_lua_source(
                &script,
                path,
                &invocation.positional,
            )),
            // Streamed: a file is what polyglots and partial execution are about.
            // A script file is `main` in `$FUNCNAME`, which is what bash calls the frame a
            // function was reached from. `-c` and standard input get none, and neither do they in
            // bash.
            Language::Shell => {
                run_program_reading(invocation, &script, Reading::Streamed, Some("main"))
            }
        },
        Err(problem) => {
            let (message, status) = why_it_would_not_run(&problem);
            eprintln!("oslo: {}: {}", path, message);
            std::process::exit(status);
        }
    }
}

/// A script as text, whatever bytes are actually in it.
///
/// `read_to_string` refuses a file holding one non-UTF-8 byte, which a Latin-1 comment or a binary
/// heredoc is enough to produce — and every such script was reported as missing. A shell has no
/// business deciding a valid script is unreadable, so the bytes are taken as they are and the
/// undecodable ones become U+FFFD, exactly where they already were.
fn read_script(path: &str) -> std::io::Result<String> {
    Ok(String::from_utf8_lossy(&fs::read(path)?).into_owned())
}

/// What to say, and what to exit with — bash's answers.
///
/// Every failure used to be "No such file or directory" with 127, which sends somebody looking for
/// a missing file when the real problem is a permission bit or a directory named by mistake.
fn why_it_would_not_run(problem: &std::io::Error) -> (&'static str, i32) {
    use std::io::ErrorKind;
    match problem.kind() {
        ErrorKind::NotFound => ("No such file or directory", 127),
        ErrorKind::PermissionDenied => ("Permission denied", 126),
        ErrorKind::IsADirectory => ("Is a directory", 126),
        _ => ("cannot execute", 126),
    }
}

/// No operand: a prompt if there is somebody there, and the program on standard input if not.
fn run_stdin(invocation: &cli::Invocation) -> ! {
    if invocation.force_interactive || stdin_is_a_terminal() {
        // The tools this build has, offered after `oslo `. Registered here rather than in the
        // runtime because the list lives in this crate — see `cli::complete`.
        cli::complete::register();
        startup::repl::run_repl(invocation.login, invocation.no_rc, invocation.no_profile);
    }
    // Bytes rather than `read_to_string`, for the reason given at [`read_script`]: a program piped
    // in is not required to be valid UTF-8 to be a valid program.
    let mut bytes = Vec::new();
    if let Err(e) = std::io::stdin().read_to_end(&mut bytes) {
        eprintln!("oslo: cannot read standard input: {}", e);
        std::process::exit(1);
    }
    let script = String::from_utf8_lossy(&bytes).into_owned();
    match language::detect(None, &script) {
        Language::Lua => std::process::exit(startup::lua_init::run_lua_source(
            &script,
            "stdin",
            &invocation.positional,
        )),
        // A pipe is a stream, and bash and dash both run what they have read of one before a later
        // syntax error stops them.
        Language::Shell => run_program_reading(invocation, &script, Reading::Streamed, None),
    }
}

/// A shell is interactive by default only when it is talking to a person.
///
/// stderr is checked as well as stdin: `oslo < script` has a terminal on stderr but must run the
/// script, and `oslo 2>log` from a terminal must still prompt.
fn stdin_is_a_terminal() -> bool {
    nix::unistd::isatty(0).unwrap_or(false) && nix::unistd::isatty(2).unwrap_or(false)
}

/// How a program's text reaches the evaluator.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reading {
    /// **A command at a time**, as a shell reads a file or a stream.
    ///
    /// POSIX says the shell reads its input and parses and executes it; every shell does so a
    /// command at a time, and scripts are written for that. Two things depend on it:
    ///
    /// - **`#!/bin/sh` polyglots.** `/usr/bin/pnmflip` and nine of its netpbm siblings are Perl
    ///   below an `exec perl -x -S -- "$0"` on line 23. Reading a command at a time reaches the
    ///   `exec`, and the process is replaced before the Perl is ever looked at. Parsing the file
    ///   first meant a syntax error at line 51 and nothing running at all.
    /// - **Partial execution.** A syntax error on line 500 leaves the first 499 lines run, which
    ///   is what bash, dash and busybox all do. oslo used to run none of them.
    ///
    /// `oslo -n` is still how you check a whole file without running it — that is what it is for,
    /// and it is what makes doing this implicitly on every run unnecessary.
    Streamed,
    /// All at once. `-c` only, because bash and dash both parse a `-c` string whole:
    /// `sh -c 'echo RAN; if true; then'` prints nothing in either.
    Whole,
}

fn run_program(invocation: &Invocation, script: &str) -> ! {
    run_program_reading(invocation, script, Reading::Whole, None)
}

fn run_program_reading(
    invocation: &Invocation,
    script: &str,
    reading: Reading,
    frame: Option<&str>,
) -> ! {
    let mut env = Environment::new();
    // **The name first**, because pushing the frame publishes `$BASH_SOURCE` and the script's own
    // path is its outermost entry — pushed first, the array named `oslo` instead of the script and
    // `dirname "${BASH_SOURCE[0]}"` answered the wrong directory.
    env.shell_name = invocation.name.clone();
    // How this program was reached, for `$FUNCNAME`'s outermost entry. See
    // `Environment::enter_script_frame`; nothing is pushed for `-c` or standard input.
    if let Some(frame) = frame {
        env.enter_script_frame(frame);
    }
    env.set_positional(invocation.positional.clone());
    apply_invocation_options(&mut env, invocation);
    startup::history::register(&mut env);
    // **A script sees them too.** "Every running session" has to include the `-c` a Makefile just
    // started, or a universal variable is a thing only a prompt can read — and setting one from a
    // script that cannot then read it back is the worst of both. Before the startup files, so
    // `$ENV` can read one.
    #[cfg(feature = "universal")]
    oslo::env::universal::sync_into(&mut env);

    // R9.10: a non-interactive shell still reads `$ENV` — that is what POSIX defines it for, and
    // it runs before the program so a function defined there is callable from it.
    if let Some(status) = startup::rc::load_startup_files(
        &mut env,
        invocation.force_interactive,
        invocation.login,
        invocation.no_profile,
    ) {
        std::process::exit(run_exit_trap(&mut env, status));
    }

    // `-c` only, and only when `$OSLO_ALLHIST` asks: a script's *contents* are not commands
    // anybody typed, and recording them would put every line of every `#!/bin/sh` script on the
    // machine into the history the moment oslo becomes `/bin/sh`.
    if reading == Reading::Whole && startup::history::record_commands() {
        startup::history::record_command(&env, script);
    }

    let status = match reading {
        Reading::Whole => run_whole(&mut env, script),
        Reading::Streamed => run_streamed(&mut env, script),
    };
    // R6.5: every way out of a script converges here, so a cleanup handler that fires on a clean
    // finish also fires on `exit 3` and on a fatal error.
    std::process::exit(run_exit_trap(&mut env, status));
}

/// Parse the whole program, then run it.
fn run_whole(env: &mut Environment, script: &str) -> i32 {
    // Parsing is kept out of `run_string` so the two kinds of failure stay distinguishable. A
    // program that does not parse never runs at all and exits 2; anything that goes wrong later
    // happened *during* execution, and gets the 127 below.
    match run_chunk(env, script, script) {
        Ok(status) => status,
        Err(status) => status,
    }
}

/// Run a file or a stream: whole if it parses, a command at a time if it does not.
///
/// **The split is a performance one, and it changes nothing observable.** A program that parses
/// has no syntax error to be partial about, so running it whole and running it a command at a time
/// produce the same thing — and parsing once is enormously cheaper. Classifying after every line
/// re-parses a growing buffer, which is quadratic in the length of a single command: a one-megabyte
/// here-document is one command, and `tests/redirection_tests.rs` timed out at ten seconds on it.
///
/// Only a program that does *not* parse takes the slow path, and there the cost is the point: it
/// is the only way to run the lines before the mistake, which is what
/// [`Reading::Streamed`] exists for.
fn run_streamed(env: &mut Environment, script: &str) -> i32 {
    if let Ok(ast) = parse_with_aliases(script, !env.get_aliases().is_empty(), &|n| {
        env.get_alias(n).map(str::to_string)
    }) {
        return match absorb_loop_control(eval_command_list(env, &ast)) {
            Ok(status) => status,
            Err(e) => exit_error_status(env, e),
        };
    }
    run_line_at_a_time(env, script)
}

/// Read a command at a time, running each before looking at the next.
///
/// The buffer is emptied after every complete command, so each re-parse covers one command's worth
/// of text rather than the whole file.
fn run_line_at_a_time(env: &mut Environment, script: &str) -> i32 {
    use oslo::ui::syntax::{InputStatus, classify};

    let mut buffer = String::new();
    let mut status = 0;
    // **Each chunk is parsed on its own, so its tree counts from 1.** Telling the environment how
    // much of the file came before it is what keeps `$LINENO` and every diagnostic's `line N`
    // talking about the file rather than about the piece. Without it, one syntax error anywhere in
    // a script made every diagnostic before it say `line 1` — worse than saying nothing, because a
    // line number is believed.
    for (consumed, line) in (0_u32..).zip(script.split_inclusive('\n')) {
        // Counted before the chunk runs, and only up to where it *starts*: the offset is added to
        // a line number inside the chunk, and the chunk's own first line is its line 1.
        if buffer.is_empty() {
            env.set_line_offset(consumed);
        }
        buffer.push_str(line);
        match classify(&buffer) {
            // The command has not ended yet — an open `if`, a heredoc still looking for its
            // delimiter, a trailing `|`.
            InputStatus::Incomplete => continue,
            // Not shell at all. Stop reading and let the parse below report it the usual way,
            // *after* everything already read has run — which is the whole point.
            InputStatus::Invalid => break,
            InputStatus::Complete => {
                // A blank line or a comment parses to nothing; there is no command to run and no
                // status to take from it.
                if !buffer.trim().is_empty() {
                    match run_chunk(env, &buffer, script) {
                        Ok(next) => status = next,
                        Err(failed) => return failed,
                    }
                }
                buffer.clear();
            }
        }
    }

    // Whatever is left never became a complete command: an unterminated `if`, or the invalid text
    // the loop stopped at. Running it produces the same diagnostic a whole-file parse would.
    if !buffer.trim().is_empty() {
        match run_chunk(env, &buffer, script) {
            Ok(next) => status = next,
            Err(failed) => return failed,
        }
    }
    env.set_line_offset(0);
    status
}

/// Parse one piece of program text and run it.
///
/// `Err` carries the status the shell should exit with; the caller stops there.
/// `whole` is the file the chunk came from, so a syntax error can be reported against it rather
/// than against the piece: a script run a command at a time would otherwise say `line 1` for a
/// mistake forty lines in. The two are the same string when the program parsed and was run whole.
fn run_chunk(env: &mut Environment, source: &str, whole: &str) -> std::result::Result<i32, i32> {
    let ast = match parse_with_aliases(source, !env.get_aliases().is_empty(), &|n| {
        env.get_alias(n).map(str::to_string)
    }) {
        Ok(ast) => ast,
        Err(e) => {
            // The one place a report points into something that really is a program. The parser's
            // message already carries the line and column; `complain_at` reads them back out and
            // quotes the failing line, and answers `false` for a message with no position in it —
            // `syntax error at end of input` is about the absence of text, not a place in it.
            let body = e.to_string();
            // **The parser knows which line it stopped on, and nothing ran on it.** `origin` reports
            // the last line that *did* run, so a syntax error on line 5 was announced as line 4 —
            // publishing the parser's own line first is what makes the prefix and the report agree.
            // A positioned error names its own line. One with no position — `syntax error at end
            // of input` — names the line the *chunk* began on, which is where the construct that
            // never closed starts, because everything before it parsed and ran. Without this the
            // prefix falls back to the last line that ran, which is a different bug entirely.
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
        // The shell's exit status is that of the last command it ran.
        Ok(status) => Ok(status),
        Err(e) => Err(exit_error_status(env, e)),
    }
}

/// Put the invocation's own flags into the option set, so `$-` describes this shell.
///
/// The two halves are different in kind: `-e`/`-x` are ordinary options a script could also set,
/// while `c` and `s` say where the program came from and no `set` command can change them. Both
/// live in the same bitset because `$-` reports both.
fn apply_invocation_options(env: &mut Environment, invocation: &Invocation) {
    // `Invocation::options` covers both spellings. Walking `set_options` here instead would drop
    // every option that has no letter — `--posix` is exactly that shape.
    for option in invocation.options() {
        env.set_option(option, true);
    }
    // The `+` forms, after the `-` ones so that `sh -x +x` ends up off — last wins, as `set` does.
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
    }
}

/// The status a non-interactive shell ends with after it could not finish its script.
///
/// Returns rather than exiting: the EXIT trap still has to run, because `trap 'rm -f "$tmp"' EXIT`
/// exists precisely for the runs that go wrong.
///
/// **`-c` and a script file end differently, and bash is the reason.** The three failures
/// [`ShellError::fatal_exit_status`] answers 127 for are 127 only from `bash -c`; run as a file,
/// the same program exits 1 for a fatal expansion error and 2 for a syntax error. So the 127 is
/// asked for only where bash gives one, and a script gets the ordinary failure status:
///
/// ```text
///   bash -c 'set -u; echo $nope'   ->  127        bash file  ->  1
///   bash -c 'echo $(if)'           ->  127        bash file  ->  2
/// ```
///
/// A script is where the number is read by something else — a Makefile, a CI step — and 127 there
/// says "command not found" about a shell that found its command and could not expand its
/// arguments.
fn exit_error_status(env: &Environment, err: ShellError) -> i32 {
    match err {
        ShellError::Exit(code) => code,
        // Everything else aborted the script mid-flight. `ShellError::fatal_exit_status` decides
        // what that is worth — deliberately *not* the status the same error produces elsewhere:
        // inside a subshell or a pipeline stage it is just a failed command, worth 1, and an
        // interactive shell only sets `$?` and carries on.
        e => {
            // **`set -e` unwinds as a failed command, and carries that command's status.**
            // Checked against bash 5.3:
            //
            // ```text
            //   bash -c 'set -u;  echo $nope'   ->  127
            //   bash -c 'set -eu; echo $nope'   ->  1
            //   bash -c 'set -e;  echo $(if)'   ->  127
            // ```
            //
            // The unset variable failed the command and `-e` ended the script on that failure, so
            // the status is the failure's. A syntax error keeps 127 either way, because it never
            // became a command that could fail.
            //
            // It matters because 127 means "command not found" to make, to a CI runner and to
            // `case $? in 127)` — and `set -eu` is the standard opening line of a careful script,
            // where an unset variable is the commonest thing to go wrong.
            let unwound_by_errexit =
                env.option(ShellOption::ErrExit) && matches!(e, ShellError::UnsetParameter(_));
            let status = match env.option(ShellOption::CommandString) && !unwound_by_errexit {
                true => e.fatal_exit_status(env.posix()),
                false => e.failure_status(),
            };
            // **Where it happened, not just what.** A `oslo: ` prefix names the shell; a script
            // that died on line 140 of 200 needs the line, and `origin` already knows it because
            // `$LINENO` is published at every command boundary. Three diagnostics carried the
            // location — command not found, a failing builtin, a failing redirection — and the
            // fatal expansion errors did not, which is backwards: those are the ones that end the
            // run. `-c` and a prompt still get `oslo: `, because neither has a file to name.
            eprintln!("{}{}", env.origin(), e);
            status
        }
    }
}

/// This process's arguments, with any byte that is not UTF-8 replaced rather than fatal.
///
/// **`env::args()` panics on such a byte**, before `main` has done anything: `oslo ./café.sh` with
/// the name in Latin-1 died with `SIGABRT` and no message a person could act on, where bash runs
/// the script. A positional parameter cannot be skipped the way an environment variable can —
/// `$2` has to stay `$2` — so the byte is replaced and the argument keeps its place.
fn arguments() -> Vec<String> {
    env::args_os()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}
