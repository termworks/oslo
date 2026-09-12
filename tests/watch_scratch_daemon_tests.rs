#![cfg(all(feature = "watch", feature = "scratch"))]

mod common;

use common::oslo_bin;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

struct Daemon(Pid);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = kill(self.0, Signal::SIGTERM);
    }
}

struct Scratch(Option<&'static str>);

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Some(name) = self.0 {
            let _ = oslo::scratch::store::kill(name);
        }
    }
}

fn daemon_pid() -> Pid {
    let stream = UnixStream::connect(oslo::scratch::daemon::socket()).expect("daemon socket");
    let mut credentials = std::mem::MaybeUninit::<nix::libc::ucred>::uninit();
    let mut length = std::mem::size_of::<nix::libc::ucred>() as nix::libc::socklen_t;
    // SAFETY: the kernel writes one `ucred` into the correctly sized output buffer.
    let status = unsafe {
        nix::libc::getsockopt(
            stream.as_raw_fd(),
            nix::libc::SOL_SOCKET,
            nix::libc::SO_PEERCRED,
            credentials.as_mut_ptr().cast(),
            &mut length,
        )
    };
    assert_eq!(status, 0, "SO_PEERCRED");
    assert_eq!(length as usize, std::mem::size_of::<nix::libc::ucred>());
    // SAFETY: a successful `getsockopt` initialized the complete structure.
    Pid::from_raw(unsafe { credentials.assume_init() }.pid)
}

#[test]
fn daemon_backend_sees_and_controls_a_watch_scratch() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scratch_dir = dir.path().join("scratches");
    // SAFETY: this integration-test executable contains one test, and no other thread reads the
    // process environment while it runs.
    unsafe { std::env::set_var("OSLO_SCRATCH_DIR", &scratch_dir) };

    let watched = dir.path().join("watched.txt");
    let descendant = dir.path().join("descendant.pid");
    std::fs::write(&watched, "").expect("watched");
    let started = Command::new(oslo_bin())
        .args([
            "watch",
            "--postpone",
            "--scratch=watch-daemon-test",
            watched.to_str().expect("path"),
            "--",
            "sh",
            "-c",
            r#"echo observed; sleep 30 & echo $! > "$1"; wait"#,
            "--",
            descendant.to_str().expect("pid file"),
        ])
        .current_dir(dir.path())
        .env("OSLO_SCRATCH_DIR", &scratch_dir)
        .stdin(Stdio::null())
        .output()
        .expect("start");
    assert!(started.status.success(), "{started:?}");
    let mut scratch = Scratch(Some("watch-daemon-test"));

    let listed = oslo::scratch::daemon::ask_list().expect("daemon list");
    let daemon = Daemon(daemon_pid());
    assert_eq!(listed, ["watch-daemon-test"]);

    let mut attached = oslo::scratch::daemon::attach_through("watch-daemon-test").expect("attach");
    attached
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    std::fs::write(&watched, "changed").expect("change");
    let mut seen = Vec::new();
    let mut buffer = [0u8; 1024];
    while !String::from_utf8_lossy(&seen).contains("observed") {
        let count = attached.read(&mut buffer).expect("scratch output");
        assert_ne!(count, 0, "scratch ended before producing output");
        seen.extend_from_slice(&buffer[..count]);
    }
    drop(attached);

    let replay = oslo::scratch::log::tail(
        &oslo::scratch::store::Paths::new("watch-daemon-test").log(),
        8192,
    )
    .expect("replay log");
    assert!(String::from_utf8_lossy(&replay).contains("observed"));

    oslo::scratch::daemon::ask_kill("watch-daemon-test").expect("daemon kill");
    scratch.0 = None;
    assert!(!oslo::scratch::store::alive("watch-daemon-test"));
    let pid: i32 = std::fs::read_to_string(&descendant)
        .expect("descendant pid")
        .trim()
        .parse()
        .expect("pid");
    let deadline = Instant::now() + Duration::from_secs(2);
    while unsafe { nix::libc::kill(pid, 0) } == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_ne!(unsafe { nix::libc::kill(pid, 0) }, 0, "descendant survived");
    drop(daemon);
}
