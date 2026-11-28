//! The `oslo` binary: argument handling, script execution, and the interactive REPL.

mod cli;
mod handoff;
mod program;
mod stream;

// Startup, the Lua API and history expansion all live at the top of the stack, in `oslo-runtime`.
// History expansion must stay reachable only from the interactive prompt.
use oslo_runtime::startup;

use cli::Action;
use handoff::{block_every_signal, inherited_stack_limit, restore_signal_mask};
use program::run_program;
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
            run_program(&invocation, text, None)
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
        Ok(script) => match language::detect_asked(invocation.modes, Some(path), &script) {
            Language::Lua => std::process::exit(startup::lua_init::run_lua_source(
                &script,
                path,
                &invocation.positional,
                invocation.modes,
            )),
            // A script file is `main` in `$FUNCNAME`, the frame bash says a function was reached
            // from.
            Language::Shell => run_program(invocation, &script, Some("main")),
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
    match language::detect_asked(invocation.modes, None, &script) {
        Language::Lua => std::process::exit(startup::lua_init::run_lua_source(
            &script,
            "stdin",
            &invocation.positional,
            invocation.modes,
        )),
        Language::Shell => run_program(invocation, &script, None),
    }
}

/// A shell is interactive by default only when it is talking to a person.
///
/// stderr is checked as well as stdin: `oslo < script` has a terminal on stderr but must run the
/// script, and `oslo 2>log` from a terminal must still prompt.
fn stdin_is_a_terminal() -> bool {
    nix::unistd::isatty(0).unwrap_or(false) && nix::unistd::isatty(2).unwrap_or(false)
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
