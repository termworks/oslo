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

#[test]
fn lua_service_runs_exact_argv_and_stops_idempotently() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("service.lua");
    std::fs::write(
        &script,
        r#"
local service = oslo.watch.start {
  name = "lua-check",
  paths = { "watched.txt" },
  run = { "sh", "-c", "printf '%s' \"$1\" > result.txt", "--", "exact value" },
  initial = false,
  scratch = false,
}
print("name=" .. service:name())
print("mode=" .. service:mode())
oslo.proc.exec("sleep 0.1")
oslo.fs.write("watched.txt", "changed")
oslo.proc.exec("sleep 0.4")
print("first=" .. tostring(service:stop()))
print("second=" .. tostring(service:stop()))
"#,
    )
    .expect("script");
    std::fs::write(dir.path().join("watched.txt"), "").expect("watched");
    let output = Command::new(oslo_bin())
        .arg(&script)
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .env("XDG_DATA_HOME", dir.path().join("data"))
        .env("XDG_CONFIG_HOME", dir.path().join("config"))
        .output()
        .expect("lua");
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "{said}");
    assert!(said.contains("name=lua-check"), "{said}");
    assert!(said.contains("mode=process"), "{said}");
    assert!(said.contains("first=true"), "{said}");
    assert!(said.contains("second=false"), "{said}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("result.txt")).expect("result"),
        "exact value"
    );
}

#[test]
fn a_lua_scripts_scoped_service_stops_on_exit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("scoped.lua");
    std::fs::write(
        &script,
        r#"
oslo.watch.start {
  name = "scoped",
  paths = { "watched.txt" },
  run = { "sh", "-c", "echo $$ > child.pid; sleep 30" },
  initial = true,
  scratch = false,
}
oslo.proc.exec("sleep 0.2")
"#,
    )
    .expect("script");
    std::fs::write(dir.path().join("watched.txt"), "").expect("watched");
    let output = Command::new(oslo_bin())
        .arg(&script)
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .env("XDG_DATA_HOME", dir.path().join("data"))
        .env("XDG_CONFIG_HOME", dir.path().join("config"))
        .output()
        .expect("lua");
    assert!(output.status.success(), "{output:?}");
    let pid: i32 = std::fs::read_to_string(dir.path().join("child.pid"))
        .expect("child pid")
        .trim()
        .parse()
        .expect("pid");
    assert_ne!(
        unsafe { nix::libc::kill(pid, 0) },
        0,
        "scoped child survived"
    );
}

#[cfg(feature = "scratch")]
#[test]
fn explicit_scratch_is_listable_replayable_and_killable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scratch_dir = dir.path().join("scratches");
    let watched = dir.path().join("watched.txt");
    let descendant = dir.path().join("descendant.pid");
    std::fs::write(&watched, "").expect("watched");
    let started = Command::new(oslo_bin())
        .args([
            "watch",
            "--postpone",
            "--scratch=watch-test",
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

    let deadline = Instant::now() + Duration::from_secs(3);
    while !scratch_dir.join("watch-test.sock").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let listed = Command::new(oslo_bin())
        .args(["scratch", "-l"])
        .env("OSLO_SCRATCH_DIR", &scratch_dir)
        .output()
        .expect("list");
    assert_eq!(String::from_utf8_lossy(&listed.stdout).trim(), "watch-test");
    let collision = Command::new(oslo_bin())
        .args([
            "watch",
            "--postpone",
            "--scratch=watch-test",
            watched.to_str().expect("path"),
            "--",
            "true",
        ])
        .current_dir(dir.path())
        .env("OSLO_SCRATCH_DIR", &scratch_dir)
        .output()
        .expect("collision");
    assert_eq!(collision.status.code(), Some(1));
    let collision_error = String::from_utf8_lossy(&collision.stderr);
    assert!(collision_error.contains("attach:"), "{collision_error}");
    assert!(collision_error.contains("kill:"), "{collision_error}");

    std::fs::write(&watched, "changed").expect("change");
    let log = scratch_dir.join("watch-test.log");
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        if text.contains("observed") && descendant.exists() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "watch output did not reach log: {text}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let killed = Command::new(oslo_bin())
        .args(["scratch", "-k", "watch-test"])
        .env("OSLO_SCRATCH_DIR", &scratch_dir)
        .output()
        .expect("kill");
    assert!(killed.status.success(), "{killed:?}");
    assert!(!scratch_dir.join("watch-test.lock").exists());
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
}
