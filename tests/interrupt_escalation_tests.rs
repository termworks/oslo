//! Ctrl-C enough times takes the terminal back from a job that will not give it up.
//!
//! **A real pty, because there is nowhere else this exists.** The whole mechanism is about which
//! process group the terminal driver signals, and a pipe has no terminal driver — a test that drove
//! oslo through a pipe would be testing nothing at all.
//!
//! Each case runs a job that `trap`s `INT` away, so the ordinary interrupt provably does not end
//! it, and then counts keystrokes. The marker in the job's command line is unique per case so that
//! `pgrep` cannot see another case's leftovers.
//!
//! **The job is stopped, not killed**, so every case checks that the process is still there in
//! state `T` — that is what makes `fg`, `bg` and `kill %1` mean something afterwards, and it is the
//! difference between handing a decision back to the person and making it for them.

mod common;

use nix::pty::openpty;
use std::fs;
use std::io::Write;
use std::os::fd::OwnedFd;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

fn owned(fd: OwnedFd) -> fs::File {
    fs::File::from(fd)
}

/// Where the job's `sh` is to look for `sleep`.
///
/// `/usr/bin:/bin` alone is empty on a Nix system, where the job then died of `sleep: command not
/// found` and every case here failed for a reason that was not the one under test.
fn utilities() -> String {
    match std::env::var("PATH") {
        Ok(ambient) if !ambient.is_empty() => format!("/usr/bin:/bin:{ambient}"),
        _ => "/usr/bin:/bin".to_string(),
    }
}

struct Shell {
    input: fs::File,
    seen: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    _home: tempfile::TempDir,
    child: std::process::Child,
}

impl Shell {
    fn start() -> Shell {
        Shell::with_config("oslo.misc.welcome = false\noslo.misc.interrupt_escape = 3\n")
    }

    fn with_config(config: &str) -> Shell {
        let pty = openpty(None, None).expect("open pty");
        let master = owned(pty.master);
        let slave = owned(pty.slave);
        let home = tempfile::tempdir().expect("temporary home");
        let dir = home.path().join("config/oslo");
        fs::create_dir_all(&dir).expect("config directory");
        fs::write(dir.join("init.lua"), config).expect("config");

        let mut command = Command::new(common::oslo_bin());
        command
            .arg("-i")
            .env_clear()
            .env("HOME", home.path())
            .env("XDG_DATA_HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path().join("config"))
            .env("PATH", utilities())
            .env("TERM", "dumb")
            .current_dir(home.path())
            .stdin(Stdio::from(slave.try_clone().expect("clone")))
            .stdout(Stdio::from(slave.try_clone().expect("clone")))
            .stderr(Stdio::from(slave.try_clone().expect("clone")));
        // SAFETY: runs after fork and calls only async-signal-safe interfaces.
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
        let child = command.spawn().expect("spawn oslo on a pty");
        drop(slave);

        let input = master.try_clone().expect("clone master");
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
        Shell {
            input,
            seen,
            _home: home,
            child,
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.seen.lock().expect("transcript")).into_owned()
    }

    /// Wait for evidence rather than a duration — a debug build under a loaded suite is slow to
    /// reach its first prompt, and a passing case returns the moment its evidence appears.
    fn until(&self, what: impl Fn(&str) -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            if what(&self.text()) {
                return true;
            }
            sleep(Duration::from_millis(25));
        }
        false
    }

    fn type_line(&mut self, line: &str) {
        self.input.write_all(line.as_bytes()).expect("write");
        self.input.write_all(b"\n").expect("write");
    }

    /// Send the interrupt character, which the terminal driver turns into a `SIGINT` for whichever
    /// process group currently owns the terminal.
    fn interrupt(&mut self) {
        self.input.write_all(b"\x03").expect("write");
        self.input.flush().expect("flush");
        // Far enough apart that the three are three deliveries and not one coalesced one.
        sleep(Duration::from_millis(400));
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Start a job that ignores `INT` and `TERM`, carrying `marker` in its command line.
fn stubborn(shell: &mut Shell, marker: &str) {
    shell.type_line(&format!(
        "sh -c 'trap \"\" INT TERM; echo READY-{marker}; sleep {marker}'"
    ));
    assert!(
        shell.until(|seen| seen.contains(&format!("READY-{marker}"))),
        "the job never started: {}",
        shell.text()
    );
    sleep(Duration::from_millis(400));
}

/// Whether a process carrying `marker` exists at all.
fn running(marker: &str) -> bool {
    !pids(marker).is_empty()
}

/// The process ids carrying `marker`.
fn pids(marker: &str) -> Vec<String> {
    let Ok(found) = Command::new("pgrep")
        .arg("-f")
        .arg(format!("sleep {marker}"))
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&found.stdout)
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect()
}

/// Whether a process carrying `marker` is in state `T` — stopped rather than gone.
fn stopped(marker: &str) -> bool {
    pids(marker)
        .iter()
        .any(|pid| state_of(pid).as_deref() == Some("T"))
}

/// A process's state letter, from `/proc/<pid>/stat`.
///
/// Split on the *last* `)`: field two is the command name in parentheses and may itself contain
/// one, so anything splitting on whitespace from the left reads the wrong field.
fn state_of(pid: &str) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after = stat.rsplit_once(')')?.1;
    after.split_whitespace().next().map(str::to_string)
}

fn cleanup(marker: &str) {
    let _ = Command::new("pkill")
        .arg("-9")
        .arg("-f")
        .arg(format!("sleep {marker}"))
        .status();
}

/// **The point.** Three interrupts take the terminal back from a job that will not give it up: the
/// prompt returns, and the job is *stopped* rather than destroyed.
#[test]
fn three_interrupts_take_the_terminal_back() {
    let marker = "3111";
    let mut shell = Shell::start();
    assert!(shell.until(|seen| seen.contains('$') || seen.contains('>')));
    stubborn(&mut shell, marker);

    for _ in 0..3 {
        shell.interrupt();
    }
    let halted = (0..40).any(|_| {
        if stopped(marker) {
            return true;
        }
        sleep(Duration::from_millis(100));
        false
    });
    // The half that makes it a feature rather than a way to lose a shell.
    shell.type_line("echo STILL''-HERE");
    let prompt_back = shell.until(|seen| seen.contains("STILL-HERE"));

    // And it is a *job*, not an orphan: the shell knows about it and can say so.
    shell.type_line("jobs");
    let listed = shell.until(|seen| seen.contains(&format!("sleep {marker}")));
    cleanup(marker);

    assert!(
        halted,
        "the job was not stopped by three interrupts:\n{}",
        shell.text()
    );
    assert!(
        prompt_back,
        "the prompt did not come back:\n{}",
        shell.text()
    );
    assert!(
        listed,
        "the stopped job is not in the job table — it was orphaned:\n{}",
        shell.text()
    );
}

/// **Nothing happens without the setting.** The default is off, and off must mean the sentinel is
/// never forked and the job is left entirely alone.
#[test]
fn without_the_setting_nothing_escalates() {
    let marker = "3444";
    let mut shell = Shell::with_config("oslo.misc.welcome = false\n");
    assert!(shell.until(|seen| seen.contains('$') || seen.contains('>')));
    stubborn(&mut shell, marker);

    for _ in 0..5 {
        shell.interrupt();
    }
    sleep(Duration::from_millis(800));

    let untouched = running(marker) && !stopped(marker);
    cleanup(marker);
    assert!(
        untouched,
        "a shell that did not ask for escalation escalated anyway:\n{}",
        shell.text()
    );
}

/// **Two is not three.** A job that ignores the interrupt is still running after two, because the
/// escalation is something you have to mean.
#[test]
fn two_interrupts_leave_the_job_alone() {
    let marker = "3222";
    let mut shell = Shell::start();
    assert!(shell.until(|seen| seen.contains('$') || seen.contains('>')));
    stubborn(&mut shell, marker);

    shell.interrupt();
    shell.interrupt();
    sleep(Duration::from_millis(800));

    let untouched = running(marker) && !stopped(marker);
    cleanup(marker);
    assert!(
        untouched,
        "two interrupts acted on a job that traps them:\n{}",
        shell.text()
    );
}

/// **The ordinary case is unchanged**: a job that does not trap the interrupt still dies of the
/// first one, and the shell comes straight back. The escalation must not have made Ctrl-C slower.
#[test]
fn one_interrupt_still_ends_an_ordinary_job() {
    let marker = "3333";
    let mut shell = Shell::start();
    assert!(shell.until(|seen| seen.contains('$') || seen.contains('>')));

    shell.type_line(&format!("sh -c 'echo READY-{marker}; sleep {marker}'"));
    assert!(
        shell.until(|seen| seen.contains(&format!("READY-{marker}"))),
        "the job never started: {}",
        shell.text()
    );
    sleep(Duration::from_millis(400));

    shell.interrupt();
    let gone = (0..40).any(|_| {
        if !running(marker) {
            return true;
        }
        sleep(Duration::from_millis(100));
        false
    });
    shell.type_line("echo BACK''-AT-IT");
    let back = shell.until(|seen| seen.contains("BACK-AT-IT"));
    cleanup(marker);

    assert!(
        gone,
        "one interrupt no longer ends a job:\n{}",
        shell.text()
    );
    assert!(back, "the prompt did not return:\n{}", shell.text());
}

/// **Leaving is a decision, not an accident.** A stopped job is invisible; walking out on one
/// means it is either killed or silently orphaned, and neither should happen without being asked.
/// The first `exit` warns and stays, and the second leaves — bash's rule, and it matters more here
/// because a job stopped by the escalation is one the shell stopped *for* you.
#[test]
fn exiting_over_a_stopped_job_asks_first() {
    let marker = "3555";
    let mut shell = Shell::start();
    assert!(shell.until(|seen| seen.contains('$') || seen.contains('>')));
    stubborn(&mut shell, marker);

    for _ in 0..3 {
        shell.interrupt();
    }
    assert!(
        (0..40).any(|_| {
            if stopped(marker) {
                return true;
            }
            sleep(Duration::from_millis(100));
            false
        }),
        "the job was never stopped:\n{}",
        shell.text()
    );

    shell.type_line("exit");
    let warned = shell.until(|seen| seen.contains("stopped jobs"));
    // And it really did stay: a shell that had exited could not answer this.
    shell.type_line("echo STILL''-RUNNING");
    let stayed = shell.until(|seen| seen.contains("STILL-RUNNING"));
    cleanup(marker);

    assert!(
        warned,
        "exit said nothing about the stopped job:\n{}",
        shell.text()
    );
    assert!(stayed, "exit left despite the warning:\n{}", shell.text());
}

/// **The count is the short form of a table**, and both have to work: almost everybody writes the
/// number, so a bare `= 3` cannot be a documented alias that quietly does nothing.
#[test]
fn the_table_form_configures_the_action() {
    let marker = "3666";
    let mut shell = Shell::with_config(
        "oslo.misc.welcome = false\n\
         oslo.misc.interrupt_escape = { after = 3, action = \"kill\", notify = false }\n",
    );
    assert!(shell.until(|seen| seen.contains('$') || seen.contains('>')));
    stubborn(&mut shell, marker);

    for _ in 0..3 {
        shell.interrupt();
    }
    let gone = (0..40).any(|_| {
        if !running(marker) {
            return true;
        }
        sleep(Duration::from_millis(100));
        false
    });
    // `notify = false` means the press before the last says nothing.
    let quiet = !shell.text().contains("press ^C again");
    cleanup(marker);

    assert!(gone, "action = kill did not kill:\n{}", shell.text());
    assert!(quiet, "notify = false still announced:\n{}", shell.text());
}

/// **A feature nobody knows fired is a feature nobody has.** Two Ctrl-C into a job that is
/// ignoring them is exactly when a person is deciding whether anything is listening.
#[test]
fn the_press_before_the_last_says_what_is_coming() {
    let marker = "3777";
    let mut shell = Shell::start();
    assert!(shell.until(|seen| seen.contains('$') || seen.contains('>')));
    stubborn(&mut shell, marker);

    shell.interrupt();
    shell.interrupt();
    let told = shell.until(|seen| seen.contains("press ^C again"));
    // And it has not acted yet — the notice is a warning, not the thing itself.
    let untouched = running(marker) && !stopped(marker);
    cleanup(marker);

    assert!(told, "no notice on the second press:\n{}", shell.text());
    assert!(
        untouched,
        "the notice came with an action:\n{}",
        shell.text()
    );
}

/// The shell says what happened in its own voice, and a config can act on it.
///
/// Both matter: a stopped job otherwise looks exactly like one somebody typed Ctrl-Z at, and
/// `on-job-escalated` is how a configuration gets to decide what that is worth.
#[test]
fn the_shell_reports_it_and_a_hook_can_see_it() {
    let marker = "3888";
    let mut shell = Shell::with_config(
        "oslo.misc.welcome = false\n\
         oslo.misc.interrupt_escape = 3\n\
         oslo.on[\"on-job-escalated\"](function(e)\n\
         \x20 print(\"SAW \" .. tostring(e.action) .. \" \" .. tostring(e.presses))\n\
         end)\n",
    );
    assert!(shell.until(|seen| seen.contains('$') || seen.contains('>')));
    stubborn(&mut shell, marker);

    for _ in 0..3 {
        shell.interrupt();
    }
    let said = shell.until(|seen| seen.contains("stopped after 3 interrupts"));
    // The hook is deferred to the next safe point, so give the shell one.
    shell.type_line("echo ONWARDS");
    let hooked = shell.until(|seen| seen.contains("SAW stopped 3"));
    cleanup(marker);

    assert!(said, "the shell said nothing about it:\n{}", shell.text());
    assert!(hooked, "on-job-escalated never fired:\n{}", shell.text());
}

/// `oslo.job.watcher()` reports the setting *and* whether anything is doing it — which come apart
/// in a shell with no job control, where the watcher is never forked.
#[test]
fn the_watcher_reports_itself() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("w.lua");
    std::fs::write(
        &script,
        "local w = oslo.job.watcher()\n\
         print(w.after, w.action, w.notify, w.running)\n",
    )
    .expect("write");
    let out = Command::new(common::oslo_bin())
        .arg(&script)
        .env("HOME", dir.path())
        .env_remove("ENV")
        .output()
        .expect("spawn oslo");
    let seen = String::from_utf8_lossy(&out.stdout).trim_end().to_string();
    // A script has no job control, so nothing is watching whatever the setting says.
    assert_eq!(seen, "0\tstop\ttrue\tfalse", "{seen}");
}
