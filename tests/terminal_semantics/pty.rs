//! The pty a terminal-semantics case is driven through, and the marks read back out of it.
//!
//! Split from the cases when the file crossed the 600-line limit: the harness is one thing — open a
//! pty, start the shell in it, type, and parse `OSC 133` / `OSC 633` out of the transcript — and
//! what each case asserts about those marks is another.

use nix::pty::openpty;
use std::fs::File;

/// A temporary `$HOME` on a **short** path, whatever `$TMPDIR` says.
///
/// **A long `$TMPDIR` silently turns green tests red, and it is not obvious why.** These are
/// transcript tests: several wait for an absolute path to appear on screen, and the widgets that
/// print one — `nav`'s header among them — truncate it to the terminal's width. Move the temporary
/// home somewhere with a longer name and the path arrives as `/…/.tmpXXXX/fir…`, the text being
/// waited for never appears, and the pty times out. Nothing is wrong with the shell.
///
/// Measured, by running this suite with nothing changed but the variable:
///
/// ```text
///   TMPDIR=/tmp                             (4 chars)   46 passed, 0 failed
///   TMPDIR=/tmp/aaaaaaaaaa                 (15 chars)   45 passed, 1 failed
///   TMPDIR=/var/tmp/oslo-tmptest           (21 chars)   44 passed, 2 failed
///   TMPDIR=/tmp/aaaaaaaaaaaaaaaaaaaaaaaa   (35 chars)   43 passed, 3 failed
/// ```
///
/// A failure that tracks an environment variable nobody thought to vary is worse than a flake,
/// because it looks stable: run it three times, or at three commits, and it fails identically every
/// time. So the suite stops depending on the variable rather than asking anyone to remember.
fn short_lived_home() -> tempfile::TempDir {
    // `/tmp` rather than `std::env::temp_dir()`, which is exactly the thing being avoided. A system
    // without it falls back, and the suite is no worse off than it was.
    match std::path::Path::new("/tmp").is_dir() {
        true => tempfile::TempDir::new_in("/tmp").expect("temporary home"),
        false => tempfile::tempdir().expect("temporary home"),
    }
}
use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

pub(crate) const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub(crate) struct Mark {
    pub(crate) kind: u8,
    pub(crate) body: String,
}

impl Mark {
    pub(crate) fn status(&self) -> Option<i32> {
        (self.kind == b'D')
            .then(|| self.body.split(';').nth(1)?.parse().ok())
            .flatten()
    }

    pub(crate) fn aid(&self) -> Option<&str> {
        self.body
            .split(';')
            .find_map(|field| field.strip_prefix("aid="))
    }
}

pub(crate) struct PtyShell {
    pub(crate) child: Child,
    pub(crate) input: File,
    pub(crate) output: Receiver<Vec<u8>>,
    pub(crate) transcript: Vec<u8>,
    pub(crate) home: tempfile::TempDir,
}

impl PtyShell {
    pub(crate) fn spawn(term: &str) -> Self {
        Self::spawn_with_options(term, false, None)
    }

    pub(crate) fn spawn_with_options(
        term: &str,
        semantic_extensions: bool,
        term_program: Option<&str>,
    ) -> Self {
        Self::spawn_with_config(term, semantic_extensions, term_program, None)
    }

    pub(crate) fn configured(term: &str, semantic_extensions: bool, config: &str) -> Self {
        Self::spawn_with_config(term, semantic_extensions, None, Some(config))
    }

    pub(crate) fn spawn_with_config(
        term: &str,
        semantic_extensions: bool,
        term_program: Option<&str>,
        config: Option<&str>,
    ) -> Self {
        Self::spawn_with_config_and_env(term, semantic_extensions, term_program, config, &[])
    }

    pub(crate) fn spawn_with_environment(term: &str, environment: &[(&str, &str)]) -> Self {
        Self::spawn_with_config_and_env(term, false, None, None, environment)
    }
    pub(crate) fn spawn_with_config_and_env(
        term: &str,
        semantic_extensions: bool,
        term_program: Option<&str>,
        config: Option<&str>,
        environment: &[(&str, &str)],
    ) -> Self {
        let pty = openpty(None, None).expect("open pty");
        let master: File = owned_file(pty.master);
        let slave: File = owned_file(pty.slave);
        let stdin = slave.try_clone().expect("clone pty slave");
        let stdout = slave.try_clone().expect("clone pty slave");
        let home = short_lived_home();
        if let Some(config) = config {
            let directory = home.path().join(".config/oslo");
            std::fs::create_dir_all(&directory).expect("create config directory");
            std::fs::write(directory.join("init.lua"), config).expect("write config");
        }
        let mut command = Command::new(crate::common::oslo_bin());
        command
            .arg("-i")
            .env_clear()
            .env("HOME", home.path())
            .env("PATH", "/usr/bin:/bin")
            .env("TERM", term)
            .env("COLORFGBG", "15;0")
            .current_dir(home.path())
            .stdin(Stdio::from(stdin))
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(slave));
        if semantic_extensions {
            command.env("OSLO_TERMINAL_EXTENSIONS", "kitty");
        }
        if let Some(term_program) = term_program {
            command.env("TERM_PROGRAM", term_program);
        }
        command.envs(environment.iter().copied());
        // SAFETY: this runs after fork and only calls async-signal-safe system interfaces.
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
        let child = command.spawn().expect("spawn oslo on pty");
        let input = master.try_clone().expect("clone pty master");
        let (send, output) = mpsc::channel();
        std::thread::spawn(move || {
            let mut master = master;
            loop {
                let mut bytes = vec![0; 4096];
                match master.read(&mut bytes) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        bytes.truncate(read);
                        if send.send(bytes).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Self {
            child,
            input,
            output,
            transcript: Vec::new(),
            home,
        }
    }

    pub(crate) fn send(&mut self, bytes: &[u8]) {
        self.input.write_all(bytes).expect("write pty input");
        self.input.flush().expect("flush pty input");
    }

    pub(crate) fn wait_for_marks(&mut self, count: usize) -> Vec<Mark> {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let marks = marks(&self.transcript);
            if marks.len() >= count {
                return marks;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "timed out: {:?}",
                visible(&self.transcript)
            );
            match self.output.recv_timeout(remaining) {
                Ok(bytes) => self.transcript.extend(bytes),
                Err(error) => panic!("pty ended before {count} marks: {error:?}"),
            }
        }
    }

    pub(crate) fn wait_for_text(&mut self, text: &str) {
        let deadline = Instant::now() + TIMEOUT;
        while !String::from_utf8_lossy(&self.transcript).contains(text) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "timed out: {:?}",
                visible(&self.transcript)
            );
            match self.output.recv_timeout(remaining) {
                Ok(bytes) => self.transcript.extend(bytes),
                Err(error) => panic!(
                    "pty ended before {text:?}: {error:?}: {}",
                    visible(&self.transcript)
                ),
            }
        }
    }

    /// Wait until `text` has appeared `times` times, not merely once.
    ///
    /// **[`Self::wait_for_text`] scans the whole transcript**, so waiting for something the shell
    /// emits once per prompt returns instantly the second time you ask — the *first* prompt's copy
    /// is still there. A test that then reads the transcript is really relying on a drain to
    /// happen to catch the rest, which is a race: `vscode_selects_one_rich_lifecycle` passed on a
    /// quiet machine for months and failed on a loaded CI runner, one prompt's marks short.
    pub(crate) fn wait_for_text_count(&mut self, text: &str, times: usize) {
        let deadline = Instant::now() + TIMEOUT;
        while String::from_utf8_lossy(&self.transcript)
            .matches(text)
            .count()
            < times
        {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "timed out waiting for {times} of {text:?}: {:?}",
                visible(&self.transcript)
            );
            match self.output.recv_timeout(remaining) {
                Ok(bytes) => self.transcript.extend(bytes),
                Err(error) => panic!(
                    "pty ended before {times} of {text:?}: {error:?}: {}",
                    visible(&self.transcript)
                ),
            }
        }
    }

    pub(crate) fn wait_for_plain_text(&mut self, text: &str) {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let rendered = String::from_utf8_lossy(&self.transcript);
            let plain = oslo::ui::dropdown::width::without_escapes(&rendered);
            if plain.contains(text) {
                return;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "timed out before {text:?}: {}",
                visible(&self.transcript)
            );
            match self.output.recv_timeout(remaining) {
                Ok(bytes) => self.transcript.extend(bytes),
                Err(error) => panic!(
                    "pty ended before {text:?}: {error:?}: {}",
                    visible(&self.transcript)
                ),
            }
        }
    }

    pub(crate) fn wait_for_occurrences(&mut self, bytes: &[u8], count: usize) {
        let deadline = Instant::now() + TIMEOUT;
        while self
            .transcript
            .windows(bytes.len())
            .filter(|window| *window == bytes)
            .count()
            < count
        {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "timed out: {:?}",
                visible(&self.transcript)
            );
            match self.output.recv_timeout(remaining) {
                Ok(output) => self.transcript.extend(output),
                Err(error) => panic!("pty ended before {count} matches: {error:?}"),
            }
        }
    }

    pub(crate) fn wait_for_exit(&mut self) {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            while let Ok(bytes) = self.output.try_recv() {
                self.transcript.extend(bytes);
            }
            if self.child.try_wait().expect("wait for oslo").is_some() {
                while let Ok(bytes) = self.output.recv_timeout(Duration::from_millis(20)) {
                    self.transcript.extend(bytes);
                }
                return;
            }
            assert!(Instant::now() < deadline, "oslo did not exit");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub(crate) fn drain_for(&mut self, duration: Duration) {
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.output.recv_timeout(remaining) {
                Ok(bytes) => self.transcript.extend(bytes),
                Err(_) => break,
            }
        }
    }
}

impl Drop for PtyShell {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub(crate) fn owned_file(fd: OwnedFd) -> File {
    fd.into()
}

pub(crate) fn marks(bytes: &[u8]) -> Vec<Mark> {
    let mut found = Vec::new();
    let mut at = 0;
    while at + 6 <= bytes.len() {
        if bytes[at..].starts_with(b"\x1b]133;")
            && let Some(end) = bytes[at + 6..]
                .windows(2)
                .position(|window| window == b"\x1b\\")
        {
            let body = &bytes[at + 6..at + 6 + end];
            if let Some(kind) = body.first().copied() {
                found.push(Mark {
                    kind,
                    body: String::from_utf8_lossy(body).into_owned(),
                });
            }
            at += 6 + end + 2;
        } else {
            at += 1;
        }
    }
    found
}

pub(crate) fn visible(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).escape_debug().to_string()
}

pub(crate) fn kinds(marks: &[Mark]) -> String {
    marks.iter().map(|mark| mark.kind as char).collect()
}

pub(crate) fn vscode_kinds(bytes: &[u8]) -> String {
    let mut kinds = String::new();
    let mut at = 0;
    while at + 6 <= bytes.len() {
        if bytes[at..].starts_with(b"\x1b]633;")
            && let Some(end) = bytes[at + 6..]
                .windows(2)
                .position(|window| window == b"\x1b\\")
        {
            if let Some(kind @ (b'A' | b'B' | b'C' | b'D' | b'E')) = bytes.get(at + 6) {
                kinds.push(*kind as char);
            }
            at += 6 + end + 2;
        } else {
            at += 1;
        }
    }
    kinds
}
