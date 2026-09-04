mod common;

use common::oslo_bin;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use std::io::BufRead;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn one_save_runs_once_and_interrupt_returns_130() {
    let dir = tempfile::tempdir().expect("tempdir");
    let watched = dir.path().join("watched.txt");
    let output = dir.path().join("runs.txt");
    std::fs::write(&watched, "").expect("watched");
    let mut child = Command::new(oslo_bin())
        .args([
            "watch",
            "--postpone",
            "--foreground",
            watched.to_str().expect("path"),
            "--",
            "sh",
            "-c",
            r#"echo ran >> "$1""#,
            "--",
            output.to_str().expect("output"),
        ])
        .current_dir(dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("watch");
    let mut errors = std::io::BufReader::new(child.stderr.take().expect("stderr"));
    let mut ready = String::new();
    errors.read_line(&mut ready).expect("ready line");
    assert!(ready.contains("ready"), "{ready:?}");

    std::fs::write(&watched, "saved").expect("save");
    let deadline = Instant::now() + Duration::from_secs(3);
    while !output.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(std::fs::read_to_string(&output).expect("run"), "ran\n");
    std::thread::sleep(Duration::from_millis(250));
    assert_eq!(std::fs::read_to_string(&output).expect("run"), "ran\n");

    kill(Pid::from_raw(child.id() as i32), Signal::SIGINT).expect("interrupt");
    assert_eq!(child.wait().expect("wait").code(), Some(130));
}

#[test]
fn an_atomic_rename_over_a_literal_file_is_observed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let watched = dir.path().join("watched.txt");
    let output = dir.path().join("runs.txt");
    std::fs::write(&watched, "old").expect("watched");
    let mut child = Command::new(oslo_bin())
        .args([
            "watch",
            "--postpone",
            "--foreground",
            watched.to_str().expect("path"),
            "--",
            "sh",
            "-c",
            r#"echo ran >> "$1""#,
            "--",
            output.to_str().expect("output"),
        ])
        .current_dir(dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("watch");
    let mut errors = std::io::BufReader::new(child.stderr.take().expect("stderr"));
    let mut ready = String::new();
    errors.read_line(&mut ready).expect("ready line");

    let temporary = dir.path().join(".watched.tmp");
    std::fs::write(&temporary, "new").expect("temporary");
    std::fs::rename(&temporary, &watched).expect("rename");
    let deadline = Instant::now() + Duration::from_secs(3);
    while !output.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(std::fs::read_to_string(&output).expect("run"), "ran\n");
    kill(Pid::from_raw(child.id() as i32), Signal::SIGTERM).expect("terminate");
    assert_eq!(child.wait().expect("wait").code(), Some(143));
}
