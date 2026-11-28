//! One program run for a key or a cipher: standard error to the terminal, a deadline, and a cap on
//! what it prints.
//!
//! * **Standard error is inherited**, so a "touch your key" or a PIN prompt reaches the person it is
//!   for instead of being captured.
//! * **A deadline**, [`DEFAULT_TIMEOUT`] unless a store's `key timeout` says otherwise. A program
//!   still running at it is killed.
//! * **A cap on standard output.** A program that prints more is killed.
//! * **Always reaped.** Every way out of [`run`] waits for the child, so no error path leaves a
//!   zombie.

use std::io::{Read, Write};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How long a key or cipher command may take when its store does not say.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// A child that is killed and waited for however the function holding it is left.
struct Reaped(Child);

impl Drop for Reaped {
    fn drop(&mut self) {
        if let Ok(None) = self.0.try_wait() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

/// Run `argv` with `input` on standard input (or none), and answer what it printed.
pub fn run(
    argv: &[String],
    input: Option<&[u8]>,
    timeout: Duration,
    cap: usize,
) -> Result<Vec<u8>, String> {
    let (program, rest) = argv.split_first().ok_or("a command with no program")?;
    let spawned = Command::new(program)
        .args(rest)
        .stdin(match input {
            Some(_) => Stdio::piped(),
            None => Stdio::null(),
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("{program}: {e}"))?;
    let mut child = Reaped(spawned);

    if let (Some(bytes), Some(mut stdin)) = (input, child.0.stdin.take()) {
        let bytes = bytes.to_vec();
        // A program that exits without reading leaves this failing with a broken pipe, which is
        // its business; the thread is not joined, so one that never reads cannot hold us.
        std::thread::spawn(move || {
            quiet_broken_pipes();
            let _ = stdin.write_all(&bytes);
        });
    }
    let stdout = child.0.stdout.take().ok_or("no standard output to read")?;
    let (send, printed) = mpsc::channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let read = stdout.take(cap as u64 + 1).read_to_end(&mut out);
        let _ = send.send(read.map(|_| out));
    });

    let deadline = Instant::now() + timeout;
    let mut output = None;
    let mut status = None;
    let mut pause = Duration::from_millis(1);
    let (output, status) = loop {
        if output.is_none()
            && let Ok(read) = printed.try_recv()
        {
            let read = read.map_err(|e| format!("{program}: {e}"))?;
            if read.len() > cap {
                return Err(format!(
                    "{program}: printed more than {cap} bytes, so it was stopped"
                ));
            }
            output = Some(read);
        }
        if status.is_none() {
            status = child.0.try_wait().map_err(|e| format!("{program}: {e}"))?;
        }
        if output.is_some()
            && let Some(ended) = status
        {
            break (output.unwrap_or_default(), ended);
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "{program}: no answer after {}, so it was stopped",
                seconds(timeout)
            ));
        }
        std::thread::sleep(pause);
        pause = (pause * 2).min(Duration::from_millis(20));
    };
    if !status.success() {
        return Err(format!("{program}: {}", ended(status)));
    }
    Ok(output)
}

/// Block `SIGPIPE` in the calling thread, so a write to a program that has exited fails with
/// `EPIPE` instead of killing a shell that runs with `SIGPIPE` at its default. The signal a write
/// raises is the writing thread's own, and is dropped when that thread ends.
fn quiet_broken_pipes() {
    use nix::libc;
    // Safety: `set` is initialised by `sigemptyset` before use, and `pthread_sigmask` changes only
    // the mask of the thread that calls it.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGPIPE);
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
    }
}

/// How a program that failed ended, without pretending to know more: what it said went straight to
/// the terminal on its way past.
fn ended(status: ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(code), _) => format!("exited {code}"),
        (None, Some(signal)) => format!("killed by signal {signal}"),
        (None, None) => "failed".to_string(),
    }
}

/// A duration as `key timeout` writes it.
pub fn seconds(timeout: Duration) -> String {
    match timeout.subsec_millis() {
        0 => format!("{}s", timeout.as_secs()),
        _ => format!("{}ms", timeout.as_millis()),
    }
}

/// `30s`, `2m`, `500ms`, or a bare number of seconds, as `key timeout` takes it.
pub fn parse_timeout(text: &str) -> Result<Duration, String> {
    let (digits, unit) = text
        .find(|c: char| !c.is_ascii_digit())
        .map_or((text, ""), |at| text.split_at(at));
    let count: u64 = digits
        .parse()
        .map_err(|_| format!("`key timeout` takes a duration like 30s, not {text:?}"))?;
    let timeout = match unit {
        "" | "s" => Duration::from_secs(count),
        "ms" => Duration::from_millis(count),
        "m" => Duration::from_secs(count.saturating_mul(60)),
        _ => return Err(format!("`key timeout` takes s, ms or m, not {unit:?}")),
    };
    if timeout.is_zero() {
        return Err("`key timeout` must be longer than nothing".to_string());
    }
    Ok(timeout)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Vec<String> {
        vec!["/bin/sh".to_string(), "-c".to_string(), script.to_string()]
    }

    #[test]
    fn what_it_prints_comes_back_and_a_failure_says_how() {
        let out = run(&sh("cat"), Some(b"through"), DEFAULT_TIMEOUT, 64).expect("run");
        assert_eq!(out, b"through");
        let failed = run(&sh("exit 3"), None, DEFAULT_TIMEOUT, 64).expect_err("failed");
        assert!(failed.ends_with("exited 3"), "{failed}");
    }

    #[test]
    fn a_program_past_its_deadline_is_stopped() {
        let started = Instant::now();
        let slow = run(
            &sh("echo 'touch your key' >&2; sleep 5"),
            None,
            Duration::from_millis(300),
            64,
        )
        .expect_err("it waited");
        assert!(slow.contains("no answer after 300ms"), "{slow}");
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn a_program_that_prints_too_much_is_stopped() {
        let loud = run(&sh("yes"), None, DEFAULT_TIMEOUT, 1024).expect_err("it read on");
        assert!(loud.contains("more than 1024 bytes"), "{loud}");
    }

    #[test]
    fn a_program_that_never_reads_cannot_hold_the_writer() {
        let big = vec![b'x'; 1 << 20];
        let out = run(&sh("echo done"), Some(&big), DEFAULT_TIMEOUT, 64).expect("run");
        assert_eq!(out, b"done\n");
    }

    #[test]
    fn a_timeout_is_written_the_way_it_is_read() {
        assert_eq!(parse_timeout("30s"), Ok(Duration::from_secs(30)));
        assert_eq!(parse_timeout("45"), Ok(Duration::from_secs(45)));
        assert_eq!(parse_timeout("2m"), Ok(Duration::from_secs(120)));
        assert_eq!(parse_timeout("250ms"), Ok(Duration::from_millis(250)));
        for bad in ["", "0", "s", "10h", "-1s", "1.5s"] {
            assert!(parse_timeout(bad).is_err(), "{bad:?} was accepted");
        }
        assert_eq!(seconds(Duration::from_secs(30)), "30s");
        assert_eq!(seconds(Duration::from_millis(1500)), "1500ms");
    }
}
