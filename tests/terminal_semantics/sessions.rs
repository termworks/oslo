//! What an interactive session does, beyond emitting marks.
//!
//! The file next door is about the `OSC 133` boundaries themselves — that they balance, that they
//! carry the right status, that two shells do not share an id. This is everything else a session
//! has to get right: how a line ends, what the transcript row above the output says, whether an
//! interrupt reaches the right process, and whether the shell may decline to leave.
//!
//! Split from it when that file crossed the 600-line limit, along the seam it already had.

use super::*;

/// **A shell inside a shell asks first**, and the answer that costs nothing is the one Enter gives.
///
/// On a terminal, because that is the whole condition: `$OSLO_NESTED` says there is an oslo above
/// this one, and there is a person here to ask. The test above turns this off to nest on purpose.
#[test]
fn a_nested_interactive_shell_asks_before_it_starts() {
    let mut shell = PtyShell::spawn("xterm-256color");
    shell.wait_for_marks(2);
    shell.send(format!("{} -i\n", common::oslo_bin().display()).as_bytes());
    shell.wait_for_text("Start a nested shell?");

    // Enter takes the default, which is to stay where you are — so the nested shell never starts
    // and the outer one is still the shell taking input.
    shell.send(b"\r");
    shell.send(b"echo STILL-OUTER=$OSLO_NESTED\n");
    shell.wait_for_text("STILL-OUTER=0");
}

/// **A Ctrl-C must escape `rm`'s prompt**, and this took six presses and a walk away from the desk.
///
/// ```text
/// oslo: rm: remove write-protected regular file '…/objects/a6/b653…'? ^C^C^C^C^C^C
/// ```
///
/// `rm` is a *builtin*: it runs in the shell process, so there is no child for the terminal driver
/// to kill and the shell itself has to arrange the ending. SIGINT is installed without `SA_RESTART`
/// exactly so the blocking read fails with `EINTR` — but the prompt used `BufRead::read_line`,
/// which is `read_until`, which treats `Interrupted` as "try again". The signal arrived, the flag
/// was set, the read went straight back to waiting, and nothing looked at the flag until an answer
/// came that never would.
///
/// Driven through a pty because that is the only place the keystroke is real: the terminal driver
/// turns `^C` into the signal, and a test writing the byte down a pipe would prove nothing.
#[test]
fn a_ctrl_c_escapes_the_rm_prompt() {
    let mut shell = PtyShell::configured("xterm-256color", false, "oslo.misc.welcome = false\n");
    shell.wait_for_marks(2);

    // A file the user cannot write to, which is what makes `rm` ask without `-i`.
    let tree = shell.home.path().join("tree");
    std::fs::create_dir_all(&tree).expect("tree");
    let guarded = tree.join("guarded");
    std::fs::write(&guarded, b"x").expect("file");
    let mut mode = std::fs::metadata(&guarded).expect("stat").permissions();
    mode.set_readonly(true);
    std::fs::set_permissions(&guarded, mode).expect("chmod");

    shell.send(b"rm -r tree\n");
    shell.wait_for_text("write-protected");

    // **One press**, which is the whole point — the old code survived six.
    shell.send(&[0x03]);

    // The prompt comes back rather than the question being asked again for ever.
    let after = shell.wait_for_marks(6);
    assert_eq!(
        kinds(&after[..6]),
        "ABCDAB",
        "{:?}",
        visible(&shell.transcript)
    );

    // And nothing was removed on the way out.
    assert!(
        guarded.exists(),
        "the interrupted rm removed the file anyway"
    );
}

/// **Stdout comes back when a structured pipeline gives up part-way.**
///
/// The tools half of a pipeline points this process's stdout at a scratch file and puts it back in
/// `finish`. Any error between the two — an expansion failure in a later stage, either fallback
/// arm — used to skip `finish` entirely, leaving stdout dup2'd onto a *deleted* temporary file for
/// the rest of the session. The prompt still drew, so the shell looked alive; nothing said after it
/// ever reached the terminal again, builtin or external alike.
#[test]
fn a_pipeline_that_fails_part_way_gives_the_terminal_back() {
    let mut shell = PtyShell::spawn("xterm-256color");
    shell.wait_for_marks(2);
    // `first` is a structured tool, so the tools half takes stdout; the unset-parameter error then
    // aborts the line before the half that hands it back.
    shell.send(b"ls | first ${nope?boom} | cat\n");
    // **Asserted on output the input cannot contain.** `$?` is written literally in what is typed
    // and expands only in what is printed, so `ttycheck=0` can only be the answer — where matching
    // the typed word itself would pass against the terminal echo whether or not stdout came back.
    shell.send(b"test -t 1; echo \"ttycheck=$?\"\n");
    shell.wait_for_plain_text("ttycheck=0");
}

/// **A job hook may ask the shell about its jobs.**
///
/// `on-job-finish` fired from inside the job table's lock, which is a plain non-reentrant `Mutex` —
/// so a handler calling `oslo.job.list()` waited for a lock its own caller held, and the shell
/// wedged the first time any background command finished. The rule was already written down for
/// the sibling `announce`; these two hooks were simply on the wrong side of it.
///
/// Interactive because that is the only place the notice is drawn: `announce_changes` returns
/// early when the shell is not interactive, so a non-interactive test cannot reach the deadlock.
#[test]
fn a_job_finish_handler_may_ask_about_jobs() {
    let mut shell = PtyShell::spawn_with_config(
        "xterm-256color",
        false,
        None,
        Some("oslo.on.job_finish(function() local _ = #oslo.job.list() end)\n"),
    );
    shell.wait_for_marks(2);
    shell.send(b"sleep 0.1 &\n");
    shell.send(b"sleep 0.4\n");
    // Asserted on output the typed line cannot contain — the arithmetic is literal in the input
    // and only the answer appears in the output. A wedged shell never gets here.
    shell.send(b"echo \"alive=$((6*7))\"\n");
    shell.wait_for_plain_text("alive=42");
}

/// **The slow-command notification is reaped.**
///
/// `Child` does not reap when it is dropped, SIGCHLD is caught rather than ignored so the kernel
/// does not either, and the reaper asks only about pids the job table names — so every command over
/// `oslo.notify.after` left an `[sh] <defunct>` behind, one per slow command, holding a pid slot
/// against `RLIMIT_NPROC` until something typed `wait`.
///
/// Driven through a pty because the notice is a between-commands side effect of an interactive
/// shell, which is the only place it fires.
#[test]
fn a_slow_command_notice_leaves_no_zombie() {
    let config = r#"
oslo.misc.welcome = false
oslo.notify.after = 1
oslo.notify.command = "true"
"#;
    let mut shell = PtyShell::configured("xterm-256color", true, config);
    shell.wait_for_marks(2);

    // Three commands over the threshold, so three notices are spawned.
    for _ in 0..3 {
        shell.send(b"sleep 1.2\n");
    }
    shell.wait_for_marks(14);

    // Ask the shell itself what it has left behind — `ps` sees its own children. The marker is
    // *built* by the command rather than typed, so waiting for it cannot match the echo of the
    // line: what was typed contains `$(`, and only the answer contains `ZOMBIES=0`.
    shell.send(b"echo \"ZOMBIES=$(ps --ppid $$ -o stat= 2>/dev/null | grep -c '^Z')\"\n");
    shell.wait_for_plain_text("ZOMBIES=0");
}

/// **An abandoned line leaves the screen the way a run one does.**
///
/// With `oslo.transcript.rule` set, a finished prompt block is replaced by one row holding the
/// command — that is the record of the line, and it is what the next prompt is drawn under.
/// Ctrl-C took a different exit that skipped the replacement entirely, so what stayed in the
/// scrollback was the whole prompt: two or three rows of it, reading like a prompt still waiting
/// for something to be typed into it.
///
/// Checked on the transcript rather than on marks, because this is about what is drawn.
#[test]
fn an_interrupted_line_is_recorded_like_any_other() {
    let config = "oslo.misc.welcome = false\noslo.transcript.rule = \"-\"\n\
                  oslo.prompt.left = function() return \"OSLOPROMPT> \" end\n";
    let mut shell = PtyShell::configured("xterm-256color", false, config);
    shell.wait_for_marks(2);

    // The control: a line that runs is replaced by one row holding the command.
    let ran_from = shell.transcript.len();
    shell.send(b"echo aardvark\n");
    shell.wait_for_marks(6);
    let ran = visible(&shell.transcript[ran_from..]);
    assert!(
        ran.contains("echo aardvark\\u{1b}[38;5;1m ]"),
        "a run command is drawn between rules:\n{ran}"
    );

    // And the line that is abandoned instead of run.
    shell.send(b"echo bandicoot");
    shell.drain_for(Duration::from_millis(500));
    let before = shell.transcript.len();
    shell.send(&[0x03]);
    shell.wait_for_marks(9);
    let after = visible(&shell.transcript[before..]);

    assert!(
        after.contains("echo bandicoot\\u{1b}[38;5;1m ]"),
        "an abandoned line is drawn the same way:\n{after}"
    );
    // And the prompt it replaces is gone rather than left standing above the next one: past the
    // end of the transcript frame there is one prompt, and it is the one being drawn now.
    let (_, past) = after
        .split_once("frame;end")
        .expect("the transcript frame closes");
    assert_eq!(
        past.matches("OSLOPROMPT>").count(),
        1,
        "only the next prompt should be left:\n{past}"
    );
}

/// The other way a config replaces a finished prompt, with the same hole in it.
///
/// `oslo.transcript.rule` is drawn by the editor; `oslo.prompt.transient` is drawn by the read
/// loop after the editor returns — and the read loop returned on Ctrl-C before reaching it. Both
/// stand for a prompt whose line is over, and a line abandoned is as over as one that ran.
#[test]
fn an_interrupted_line_gets_the_transient_prompt_too() {
    let config = "oslo.misc.welcome = false\n\
                  oslo.prompt.left = function() return \"OSLOPROMPT> \" end\n\
                  oslo.prompt.transient = function() return \"SHORT> \" end\n";
    let mut shell = PtyShell::configured("xterm-256color", false, config);
    shell.wait_for_marks(2);

    // The control: a line that runs gets the short prompt in place of the tall one.
    let ran_from = shell.transcript.len();
    shell.send(b"echo aardvark\n");
    shell.wait_for_marks(6);
    let ran = visible(&shell.transcript[ran_from..]);
    assert!(
        ran.contains("SHORT> echo aardvark"),
        "a run command gets the transient prompt:\n{ran}"
    );

    // And the line that is abandoned instead of run.
    shell.send(b"echo bandicoot");
    shell.drain_for(Duration::from_millis(500));
    let before = shell.transcript.len();
    shell.send(&[0x03]);
    shell.wait_for_marks(9);
    let after = visible(&shell.transcript[before..]);
    assert!(
        after.contains("SHORT> echo bandicoot"),
        "and so does an abandoned one:\n{after}"
    );
}

/// **Ctrl-C on an empty line gives its rows back instead of taking more.**
///
/// There is no command to record and nothing was typed, so the rows the prompt is standing on are
/// still the right rows for the next one. Moving down instead left the dead prompt on the screen
/// and opened another underneath it: four presses, four prompts, each frozen at whatever the
/// spinner in it happened to be showing.
///
/// Asserted on the escape that does it — `ESC[nA CR`, back to the top of the block — rather than on
/// how many times the prompt was *drawn*, which is two per press either way: the block is repainted
/// and then the next prompt is drawn over the very same rows.
#[test]
fn a_ctrl_c_on_an_empty_line_gives_the_block_back() {
    // Two rows, because a one-row prompt hides the drift.
    let config = "oslo.misc.welcome = false\noslo.transcript.rule = \"-\"\n\
                  oslo.prompt.left = function() return \"\\nOSLOPROMPT> \" end\n";
    let mut shell = PtyShell::configured("xterm-256color", false, config);
    shell.wait_for_marks(2);
    shell.send(b"echo aardvark\n");
    shell.wait_for_marks(6);

    let before = shell.transcript.len();
    for _ in 0..4 {
        shell.send(&[0x03]);
        shell.drain_for(Duration::from_millis(400));
    }
    let after = visible(&shell.transcript[before..]);

    // Once per press: the block is handed back, and the command-end mark follows immediately.
    assert_eq!(
        after.matches("\\u{1b}[2A\\r\\u{1b}[?2004l").count(),
        4,
        "each Ctrl-C should park the block it was standing on:\n{after}"
    );
    // And nothing was recorded for a line that was never typed.
    assert!(
        !after.contains("---["),
        "an empty line has no command to record:\n{after}"
    );
}

/// **`pre-exit` may keep the shell open**, which is the one thing a hook can refuse that the user
/// asked for directly. `exit` and Ctrl-D sit a keystroke from the command above them, and the shell
/// they close is often the last pane of a multiplexer — where the answer used to be to have the
/// multiplexer start another shell, because a shell could not decline to die.
#[test]
fn a_pre_exit_hook_can_refuse_to_leave() {
    let config = "oslo.misc.welcome = false\n\
                  oslo.prompt.left = function() return \"OSLOPROMPT> \" end\n\
                  local asked = 0\n\
                  oslo.on.pre_exit(function(c)\n\
                    asked = asked + 1\n\
                    print(\"ASKED-\" .. c.reason .. \"-\" .. tostring(c.status))\n\
                    if asked < 2 then return false end\n\
                  end)\n";
    let mut shell = PtyShell::configured("xterm-256color", false, config);
    shell.wait_for_marks(2);

    // A typed `exit` is refused, and the shell runs the next command.
    shell.send(b"exit\n");
    shell.drain_for(Duration::from_millis(700));
    shell.send(b"echo STILL-HERE\n");
    shell.drain_for(Duration::from_millis(700));
    let so_far = visible(&shell.transcript);
    assert!(
        so_far.contains("ASKED-exit-0"),
        "the hook is told which and what:\n{so_far}"
    );
    assert!(
        so_far.contains("STILL-HERE"),
        "the shell should still be running:\n{so_far}"
    );

    // The second time it agrees, and the shell goes.
    shell.send(b"exit\n");
    shell.wait_for_exit();
}

/// Ctrl-D asks the same question, and says which it was.
#[test]
fn a_pre_exit_hook_is_told_that_ctrl_d_was_ctrl_d() {
    let config = "oslo.misc.welcome = false\n\
                  oslo.prompt.left = function() return \"OSLOPROMPT> \" end\n\
                  oslo.on.pre_exit(function(c)\n\
                    print(\"ASKED-\" .. c.reason)\n\
                    return c.reason ~= \"eof\"\n\
                  end)\n";
    let mut shell = PtyShell::configured("xterm-256color", false, config);
    shell.wait_for_marks(2);

    shell.send(&[0x04]);
    shell.drain_for(Duration::from_millis(700));
    shell.send(b"echo SURVIVED-EOF\n");
    shell.drain_for(Duration::from_millis(700));
    let said = visible(&shell.transcript);
    assert!(
        said.contains("ASKED-eof"),
        "Ctrl-D is `eof`, not `exit`:\n{said}"
    );
    assert!(
        said.contains("SURVIVED-EOF"),
        "refusing an EOF keeps the shell:\n{said}"
    );

    // `exit` is allowed through by the same handler, so the test cannot hang.
    shell.send(b"exit\n");
    shell.wait_for_exit();
}

/// **A script is never asked whether it may exit.**
///
/// `pre-exit` keeps an *interactive* shell open. A shell whose input is a file or a pipe reaches
/// its end because the input genuinely ended, and refusing there is not a second chance — it is a
/// loop reading the same end-of-file for ever.
#[test]
fn a_pre_exit_hook_cannot_hold_a_script_open() {
    let home = tempfile::tempdir().expect("home");
    let config = home.path().join(".config/oslo");
    std::fs::create_dir_all(&config).expect("mkdir");
    std::fs::write(
        config.join("init.lua"),
        "oslo.misc.welcome = false\noslo.on.pre_exit(function() return false end)\n",
    )
    .expect("config");

    let ran = Command::new(common::oslo_bin())
        .arg("-c")
        .arg("echo in-a-script; exit 3")
        .env("HOME", home.path())
        .env_remove("ENV")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn oslo");
    assert_eq!(
        ran.status.code(),
        Some(3),
        "a script's `exit` is not vetoable"
    );
    assert!(String::from_utf8_lossy(&ran.stdout).contains("in-a-script"));
}

/// **An abbreviation ends at Enter, not only at space.**
///
/// `gco` then Enter used to run `gco` — the one word the abbreviation exists so you never type —
/// and the failure is silent, because `gco` is a perfectly good command name for the shell to
/// report as missing. The expansion happens *before* the line is taken, so what runs, what is
/// written to history and what the transcript row above the output shows are all the command.
#[test]
fn an_abbreviation_expands_on_enter_and_is_what_the_row_shows() {
    let config = "oslo.misc.welcome = false\n\
                  oslo.transcript.rule = \"-\"\n\
                  oslo.prompt.left = function() return \"OSLOPROMPT> \" end\n\
                  oslo.abbr.zz = \"echo EXPANDED-BY-ENTER\"\n";
    let mut shell = PtyShell::configured("xterm-256color", false, config);
    shell.wait_for_marks(2);

    let before = shell.transcript.len();
    shell.send(b"zz\n");
    shell.wait_for_plain_text("EXPANDED-BY-ENTER");
    let after = visible(&shell.transcript[before..]);

    // The transcript row names the command, not the abbreviation that stood for it.
    assert!(
        after.contains("echo EXPANDED-BY-ENTER"),
        "the row should show the expansion:\n{after}"
    );
    assert!(
        !after.contains("[ \\u{1b}[38;5;15;1mzz"),
        "and not the abbreviation:\n{after}"
    );
}

/// Space still expands, and still supplies the space itself — the two are one act.
#[test]
fn an_abbreviation_still_expands_on_space() {
    let config = "oslo.misc.welcome = false\n\
                  oslo.prompt.left = function() return \"OSLOPROMPT> \" end\n\
                  oslo.abbr.zz = \"echo EXPANDED-BY-SPACE\"\n";
    let mut shell = PtyShell::configured("xterm-256color", false, config);
    shell.wait_for_marks(2);
    shell.send(b"zz ");
    shell.drain_for(Duration::from_millis(700));
    let drawn = visible(&shell.transcript);
    assert!(
        drawn.contains("EXPANDED-BY-SPACE"),
        "space should still expand:\n{drawn}"
    );
}

/// **A `.env.lua` may set an abbreviation**, and leaving the directory takes it back.
///
/// `oslo.abbr` is an ordinary Lua table a config assigns into, and the reader that turns it into
/// abbreviations had run once at startup — so a directory assigning to it did nothing at all. It is
/// read again once the file has run, and what the directory added or changed is recorded so that
/// leaving puts it back.
///
/// On a pty, because expanding an abbreviation is the line editor's job and a shell fed from a pipe
/// has no editor to do it.
///
/// Needs `direnv`: the test allows the directory through the `direnv` builtin, which a
/// default-feature build does not have — and CI builds with default features.
#[test]
#[cfg(feature = "direnv")]
fn a_directory_may_define_an_abbreviation_and_leaving_takes_it_back() {
    let project = tempfile::tempdir().expect("dir");
    std::fs::write(
        project.path().join(".env.lua"),
        "oslo.abbr.zz = \"echo ZZ-RAN\"\n",
    )
    .expect("env.lua");

    let mut shell = PtyShell::configured(
        "xterm-256color",
        false,
        "oslo.misc.welcome = false\noslo.prompt.left = function() return \"P> \" end\n",
    );
    shell.wait_for_marks(2);
    shell.send(format!("cd {}\n", project.path().display()).as_bytes());
    shell.drain_for(Duration::from_millis(600));
    shell.send(b"direnv allow\n");
    shell.drain_for(Duration::from_millis(1500));

    let inside_from = shell.transcript.len();
    shell.send(b"zz\n");
    shell.drain_for(Duration::from_millis(1200));
    let inside = visible(&shell.transcript[inside_from..]);
    assert!(
        inside.contains("ZZ-RAN"),
        "it should expand here:\n{inside}"
    );

    shell.send(b"cd /\n");
    shell.drain_for(Duration::from_millis(1000));
    let outside_from = shell.transcript.len();
    shell.send(b"zz\n");
    shell.drain_for(Duration::from_millis(1200));
    let outside = visible(&shell.transcript[outside_from..]);
    // Asserted on the *refusal*, not on the absence of the expansion: the history hint offers the
    // line that ran a moment ago, so `echo ZZ-RAN` is drawn on the row either way.
    assert!(
        outside.contains("zz: command not found"),
        "it should have left with the directory:\n{outside}"
    );
}

/// **A spec's suggestions do not take the position away from path completion.**
///
/// Two failures, both from the converted Fig specs and both leaving Tab worse than it had been
/// before the spec existed:
///
/// * `cd` offers `-` and `~`, because Fig listed the folders with a generator and a generator is
///   the one thing the converter cannot bring across. Those two answers claimed the position and
///   took every directory off the menu.
/// * `ls` says `["$files", "$directories"]`, and the second marker overwrote the first — so
///   "anything on disk" became "directories only" and every file vanished. 225 specs say it that
///   way.
///
/// Needs `compgen`: without it the shell has no spec reader, so there is no spec for the assertion
/// to be about. CI builds with default features — see `tests/spec_file_tests.rs`, which gates the
/// whole file the same way.
#[test]
#[cfg(feature = "compgen")]
fn a_spec_that_names_no_paths_still_completes_them() {
    let data = tempfile::tempdir().expect("data");
    let comp = data.path().join("oslo/completion");
    std::fs::create_dir_all(&comp).expect("mkdir");
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("share/completion");
    for spec in ["cd.yaml", "ls.yaml"] {
        std::fs::copy(repo.join(spec), comp.join(spec)).expect("the shipped spec");
    }

    let dir = tempfile::tempdir().expect("dir");
    std::fs::create_dir(dir.path().join("alpha")).expect("mkdir");
    std::fs::write(dir.path().join("afile.txt"), "x").expect("file");

    let mut shell = PtyShell::spawn_with_config_and_env(
        "xterm-256color",
        false,
        None,
        Some("oslo.misc.welcome = false\noslo.prompt.left = function() return \"P> \" end\n"),
        &[("XDG_DATA_HOME", data.path().to_str().expect("utf-8"))],
    );
    shell.wait_for_marks(2);
    shell.send(format!("cd {}\n", dir.path().display()).as_bytes());
    shell.drain_for(Duration::from_millis(700));

    // `cd`: the spec's two answers *and* the directories, and no files — `cd` refuses those.
    let from = shell.transcript.len();
    shell.send(b"cd \t");
    shell.drain_for(Duration::from_millis(1000));
    let shown = visible(&shell.transcript[from..]);
    assert!(
        shown.contains("alpha"),
        "cd should offer directories:\n{shown}"
    );
    assert!(
        shown.contains("Switch to the last"),
        "and still what the spec knows:\n{shown}"
    );
    assert!(
        !shown.contains("afile"),
        "cd takes only directories:\n{shown}"
    );

    shell.send(&[0x15]);
    shell.drain_for(Duration::from_millis(300));

    // `ls`: both markers, so both kinds.
    let from = shell.transcript.len();
    shell.send(b"ls \t");
    shell.drain_for(Duration::from_millis(1000));
    let shown = visible(&shell.transcript[from..]);
    assert!(
        shown.contains("alpha"),
        "ls should offer directories:\n{shown}"
    );
    assert!(
        shown.contains("afile"),
        "ls should offer files too:\n{shown}"
    );
}
