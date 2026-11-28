//! What a command oslo launches inherits, and what it must not.
//!
//! These go through the real binary because the defect they cover lives in the window between
//! `fork` and `execv`: an in-process test can inspect `Environment` all it likes and never see
//! that the program on the other side of `execv` started life with SIGPIPE ignored.

mod common;

use common::run;
use std::fs;
use std::io::Read;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

/// The kernel reports the ignored-signal set as a hex bitmask in `/proc/self/status`.
///
/// Anything non-zero here is a disposition the child did not ask for and cannot see: the Rust
/// runtime's `SIG_IGN` for SIGPIPE (bit 13, `0x1000`), and — in the REPL — the shell's own
/// `SIG_IGN` for SIGTSTP/SIGTTIN/SIGTTOU. bash leaves this field at zero.
fn sig_ign_mask(script: &str) -> u64 {
    let r = run(script);
    let line = r
        .stdout
        .lines()
        .find(|l| l.starts_with("SigIgn:"))
        .unwrap_or_else(|| panic!("no SigIgn line in:\n{}\nstderr: {}", r.stdout, r.stderr))
        .to_string();
    let hex = line.split_whitespace().nth(1).expect("SigIgn value");
    u64::from_str_radix(hex, 16).expect("hex mask")
}

#[test]
fn an_exec_ed_child_ignores_nothing() {
    assert_eq!(sig_ign_mask("grep SigIgn /proc/self/status"), 0);
}

/// A pipeline stage forks twice — once for the stage, once for the command — so it is the path
/// most likely to lose the reset.
#[test]
fn a_pipeline_stage_ignores_nothing() {
    assert_eq!(sig_ign_mask("cat /proc/self/status | grep SigIgn"), 0);
}

/// The point of restoring SIG_DFL for SIGPIPE: `yes` must be killed by the closed pipe rather
/// than surviving the failed write and complaining about it.
#[test]
fn a_writer_to_a_closed_pipe_dies_quietly() {
    let r = run("yes | head -1");
    assert_eq!(r.out(), "y");
    assert_eq!(r.stderr, "", "SIGPIPE was still ignored in the child");
    assert_eq!(r.status, 0);
}

/// The other half of restoring SIG_DFL for SIGTSTP: a child that *does* stop must not take the
/// shell down with it.
///
/// `waitpid` without `WUNTRACED` only reports termination, so a suspended job would leave the
/// shell blocked forever on a process nothing can resume — a suspend that turns into a hang is
/// worse than a Ctrl-Z that does nothing. The command is run under a watchdog so a regression
/// fails this test instead of wedging the whole test run.
#[test]
fn a_stopped_child_does_not_wedge_the_shell() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = "sh -c 'echo $$ > pid; kill -STOP $$'\necho \"s=$?\"\necho CONTINUE";

    let mut child = Command::new(common::oslo_bin())
        .arg("-c")
        .arg(script)
        .current_dir(dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn oslo");

    let deadline = Instant::now() + Duration::from_secs(10);
    let wedged = loop {
        match child.try_wait().expect("try_wait") {
            Some(_) => break false,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                break true;
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };

    // The stopped process is deliberately abandoned by the shell, so this test owns it: without
    // this it survives as a stopped orphan for as long as the machine is up.
    if let Ok(pid) = fs::read_to_string(dir.path().join("pid")) {
        let _ = Command::new("kill").arg("-KILL").arg(pid.trim()).status();
    }

    let mut stdout = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut stdout);
    }

    assert!(!wedged, "the shell blocked on a stopped child");
    // 128 + SIGSTOP, the status a shell reports for a job it left suspended.
    let stopped = 128 + nix::sys::signal::Signal::SIGSTOP as i32;
    assert!(
        stdout.contains(&format!("s={stopped}")),
        "expected the stop to be reported as {stopped}, got:\n{stdout}"
    );
    assert!(stdout.contains("CONTINUE"), "script did not continue");
}

/// A blocked signal survives `exec` too, so the mask has to be cleared as well as the handlers.
#[test]
fn an_exec_ed_child_blocks_nothing() {
    let r = run("grep SigBlk /proc/self/status");
    let hex = r
        .stdout
        .split_whitespace()
        .nth(1)
        .expect("SigBlk value")
        .to_string();
    assert_eq!(u64::from_str_radix(&hex, 16).expect("hex mask"), 0);
}

/// R7.2: a SIGINT that arrives while a loop is *already running* still reaches the shell.
///
/// A shell that polls for interrupts only before entering a loop leaves `while :; do :; done`
/// spinning past Ctrl-C forever — the finding this covers. The equivalent used to be a unit test
/// that `fork()`ed from libtest's thread pool and deadlocked about one run in ten; see the note in
/// `src/exec/pipeline/interrupt.rs`.
///
/// The trap is what makes this a test of the poll rather than of the kernel. With SIGINT at its
/// default disposition the process dies no matter what the shell does, so asserting on that would
/// pass just as happily with the interrupt check removed. A handler means the signal *cannot* end
/// the loop on its own: only the shell noticing it between commands can, so a regression shows up
/// as this test timing out instead of as a green run.
///
/// The loop is pure shell — no `sleep`, nothing entering the kernel — because a shell blocked in a
/// syscall is interrupted by `EINTR`, which is the easy case and not the one that was broken.
/// bash exits 42 here and prints `interrupted`, which is what oslo is checked against.
#[test]
fn a_running_loop_sees_a_trapped_sigint() {
    let mut child = Command::new(common::oslo_bin())
        .arg("-c")
        .arg(r#"trap 'echo interrupted; exit 42' INT; while :; do :; done"#)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn oslo");

    // Long enough to be inside the loop rather than still parsing: interrupting a loop that has
    // not started yet is the case that always worked.
    sleep(Duration::from_millis(300));
    assert!(
        child.try_wait().expect("try_wait").is_none(),
        "the shell exited before it could be interrupted"
    );

    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(child.id() as i32),
        nix::sys::signal::Signal::SIGINT,
    )
    .expect("send SIGINT");

    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        match child.try_wait().expect("try_wait") {
            Some(status) => break Some(status),
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            None => sleep(Duration::from_millis(20)),
        }
    };

    let status = status.expect("the loop spun past SIGINT: the trap never ran");
    let mut out = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = pipe.read_to_string(&mut out);
    }
    assert_eq!(
        status.code(),
        Some(42),
        "the INT trap should have exited 42, output was {out:?}"
    );
    assert!(
        out.contains("interrupted"),
        "the INT trap did not run; output was {out:?}"
    );
}

/// **Ctrl-C must end a loop whose body forks, not just kill the child.**
///
/// The terminal sends SIGINT to the foreground *child*, and a shell waiting on one is not in that
/// group — so nothing in the interrupt machinery hears the key, and `while true; do sleep 1; done`
/// ran forever under a keyboard full of `^C`. The evidence a key was pressed is the child's wait
/// status and nothing else, which is where it is now noticed.
///
/// Driven through a real pty because that is the only place the signal comes from the terminal
/// rather than from a `kill`: sending SIGINT by hand would go to the shell and pass whatever the
/// bug was.
/// **A child that dies by SIGINT does not end a script.**
///
/// The shell infers a keyboard interrupt from a child's wait status, because an interactive shell
/// is not in the foreground process group and never sees the signal itself. A *script* is in that
/// group — a real Ctrl-C reaches it directly — so there is nothing to infer, and inferring anyway
/// meant a child that raised SIGINT on itself silently abandoned the rest of the file. bash and
/// dash both carry on; measured against both.
#[test]
fn a_child_dying_by_sigint_does_not_abandon_a_script() {
    let r = run("echo before\n/bin/sh -c 'kill -INT $$'\necho \"after=$?\"\n");
    assert_eq!(
        r.lines(),
        vec!["before", "after=130"],
        "stderr: {}",
        r.stderr
    );
    assert_eq!(r.status, 0, "the script did not run to the end");

    // SIGQUIT travels the same path.
    let r = run("echo before\n/bin/sh -c 'kill -QUIT $$'\necho after\n");
    assert_eq!(r.lines(), vec!["before", "after"], "stderr: {}", r.stderr);
}

mod interrupt {
    use super::*;
    use nix::pty::openpty;
    use std::io::Write;
    use std::os::fd::OwnedFd;
    use std::os::unix::process::CommandExt;

    fn owned(fd: OwnedFd) -> fs::File {
        fs::File::from(fd)
    }

    /// Run `line` at an interactive prompt, press Ctrl-C, and answer what the shell did next.
    fn interrupted(line: &str) -> String {
        let pty = openpty(None, None).expect("open pty");
        let master = owned(pty.master);
        let slave = owned(pty.slave);
        let home = tempfile::tempdir().expect("temporary home");
        let mut command = Command::new(common::oslo_bin());
        command
            .arg("-i")
            .env_clear()
            .env("HOME", home.path())
            .env("PATH", "/usr/bin:/bin")
            .env("TERM", "dumb")
            .current_dir(home.path())
            .stdin(Stdio::from(slave.try_clone().expect("clone")))
            .stdout(Stdio::from(slave.try_clone().expect("clone")))
            .stderr(Stdio::from(slave.try_clone().expect("clone")));
        // The shell needs the pty as its *controlling* terminal, or the key never becomes a signal
        // — which is the whole mechanism under test.
        //
        // SAFETY: runs after fork and calls only async-signal-safe system interfaces.
        unsafe {
            command.pre_exec(|| {
                if nix::libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if nix::libc::ioctl(0, nix::libc::TIOCSCTTY, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn().expect("spawn oslo on a pty");

        // **Dropped here.** The parent holding a copy of the slave means the master never reaches
        // end of file, so anything waiting for one waits for ever.
        drop(slave);

        let mut input = master.try_clone().expect("clone master");
        // **A shared buffer, never joined.** A `sleep` orphaned by the killed shell keeps the slave
        // open, so the reader can outlive the test — which is fine as long as nothing waits for it.
        let seen: std::sync::Arc<std::sync::Mutex<Vec<u8>>> = Default::default();
        let filling = std::sync::Arc::clone(&seen);
        std::thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            while let Ok(n) = std::io::Read::read(&mut (&master), &mut buffer) {
                if n == 0 {
                    break;
                }
                filling
                    .lock()
                    .expect("transcript")
                    .extend_from_slice(&buffer[..n]);
            }
        });

        // **Waiting for evidence, not for a duration.** Fixed sleeps passed alone and failed under
        // the full suite, where twenty test binaries share the machine and a shell takes longer to
        // reach its first prompt than any number picked in advance.
        let text = || String::from_utf8_lossy(&seen.lock().expect("transcript")).into_owned();
        let until = |what: &dyn Fn(&str) -> bool, why: &str| {
            let deadline = Instant::now() + Duration::from_secs(15);
            while Instant::now() < deadline {
                if what(&text()) {
                    return;
                }
                sleep(Duration::from_millis(25));
            }
            panic!("timed out waiting for {why}:\n{}", text());
        };

        until(&|seen| !seen.trim().is_empty(), "the first prompt");
        input.write_all(line.as_bytes()).expect("write");
        input.write_all(b"\n").expect("write");

        // The editor repaints the row as it is typed, so the tail of the line appearing means the
        // shell read it. A short grace after that puts the Ctrl-C inside the loop rather than
        // before it, which is the case that was broken.
        let tail = line[line.len().saturating_sub(12)..].to_string();
        until(&|seen| seen.contains(&tail), "the line to be read");
        sleep(Duration::from_millis(400));
        input.write_all(b"\x03").expect("write");

        sleep(Duration::from_millis(300));
        // If the shell is stuck in the loop this is never read, and the marker never comes back.
        input.write_all(b"echo RECOVERED\n").expect("write");
        let recovered = Instant::now() + Duration::from_secs(15);
        while Instant::now() < recovered && !printed(&text(), "RECOVERED") {
            sleep(Duration::from_millis(25));
        }

        let _ = child.kill();
        let _ = child.wait();
        drop(input);

        text()
    }

    /// Did `word` appear as a *command's output* rather than as part of the line being typed?
    ///
    /// **Counting occurrences cannot answer this.** The line editor repaints the whole row on every
    /// keystroke, so a word being typed appears once per character — in the transcript below,
    /// `RECOVERED` shows up fourteen times before it has been run once. What only output produces
    /// is a screen segment that is *nothing but* the word.
    fn printed(transcript: &str, word: &str) -> bool {
        let mut plain = String::with_capacity(transcript.len());
        let mut chars = transcript.chars();
        while let Some(c) = chars.next() {
            if c != '\u{1b}' {
                plain.push(c);
                continue;
            }
            // Skip the escape and everything up to the byte that ends it.
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() || c == '\u{7}' {
                    break;
                }
            }
        }
        plain
            .split(['\r', '\n'])
            .any(|segment| segment.trim() == word)
    }

    /// A loop that forks each iteration ends, rather than eating the key and going round again.
    #[test]
    fn a_loop_whose_body_forks_ends_on_ctrl_c() {
        let seen = interrupted("while true; do sleep 0.2; done");
        assert!(
            printed(&seen, "RECOVERED"),
            "the shell never came back:\n{seen}"
        );
    }

    /// **And the rest of the line does not run.** bash and dash both abandon it; oslo used to
    /// carry on to the next command, which is the same missed interrupt wearing a different hat.
    #[test]
    fn what_follows_an_interrupted_command_is_abandoned() {
        let seen = interrupted("sleep 5; echo NOTTHIS");
        assert!(
            !printed(&seen, "NOTTHIS"),
            "the rest of the line ran after Ctrl-C:\n{seen}"
        );
        assert!(
            printed(&seen, "RECOVERED"),
            "the shell never came back:\n{seen}"
        );
    }
}

/// **A signal is not the end of a stream.**
///
/// oslo installs SIGWINCH with no `SA_RESTART` (`term/resize.rs`), so an ordinary window resize
/// interrupts a blocking read with `EINTR`. Three capture loops folded that into their end-of-file
/// arm, so the captured text came back silently short — `cat hosts.txt | ssh {0:0} uptime` then ran
/// against no host at all and reported success.
///
/// The trap makes the signal arrive while the producer is still writing, which is the window the
/// bug lived in.
#[test]
fn a_capture_interrupted_by_a_signal_is_not_truncated() {
    let script = r#"
        trap 'true' USR1
        { sleep 0.2; kill -USR1 $$; sleep 0.1; } &
        { printf 'first\n'; sleep 0.4; printf 'last\n'; } | echo "[{*:0}]"
        wait
    "#;
    // Interactive, because coordinates are prompt syntax.
    let out = std::process::Command::new(common::oslo_bin())
        .arg("-i")
        .arg("-c")
        .arg(script)
        .output()
        .expect("spawn oslo");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("first") && text.contains("last"),
        "the capture lost data to a signal: {text:?}"
    );
}
