//! A forked child can use the `$PATH` index while another thread is scanning.
//!
//! This is the shape of the bug, not an analogy. `command_index::warm` scans `$PATH` on a detached
//! thread at every interactive startup, and the cache lock used to be held across the whole read.
//! A subshell forked in that window — `$(typpo)`, which reaches `command_index::nearest` in the
//! child — inherited a locked mutex that nothing in the child would ever release, so the child
//! stopped there and the shell sat in `waitpid` behind it for good.
//!
//! The scan is now outside the lock and both the lock and the fork go through
//! `oslo_base::forksafe`.

use oslo_ui::command_index::CommandIndex;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Enough names that a scan is long enough to fork into the middle of.
const NAMES: usize = 4_000;

fn make_exe(dir: &std::path::Path, name: &str) {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    let mut file = std::fs::File::create(&path).unwrap();
    file.write_all(b"#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// `pid`'s exit status, or `None` if it had not exited within `budget`.
fn exit_status_within(pid: i32, budget: Duration) -> Option<i32> {
    let deadline = Instant::now() + budget;
    let mut raw = 0;
    while Instant::now() < deadline {
        if unsafe { libc::waitpid(pid, &mut raw, libc::WNOHANG) } == pid {
            return Some(libc::WEXITSTATUS(raw));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    None
}

#[test]
fn a_child_forked_during_a_scan_can_read_the_index() {
    let big = tempfile::tempdir().unwrap();
    for n in 0..NAMES {
        make_exe(big.path(), &format!("cmd{n}"));
    }
    let small = tempfile::tempdir().unwrap();
    make_exe(small.path(), "mine");
    let scanned = big.path().to_str().unwrap().to_string();
    let wanted = small.path().to_str().unwrap().to_string();

    // Scanning without pause, so the fork below lands inside a scan rather than beside one.
    let stop = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&stop);
    let scanning = std::thread::spawn(move || {
        while !signal.load(Ordering::Relaxed) {
            oslo_ui::command_index::invalidate();
            let _ = CommandIndex::executables(&scanned);
        }
    });
    std::thread::sleep(Duration::from_millis(50));

    let mut stuck = Vec::new();
    for round in 0..8 {
        let pid = oslo_base::forksafe::quiesced(|| unsafe { libc::fork() });
        if pid == 0 {
            let found = CommandIndex::contains(&wanted, "mine");
            unsafe { libc::_exit(if found { 7 } else { 1 }) }
        }
        match exit_status_within(pid, Duration::from_secs(15)) {
            Some(7) => {}
            Some(other) => panic!("round {round}: the child answered {other}, not 7"),
            None => {
                unsafe { libc::kill(pid, libc::SIGKILL) };
                let _ = exit_status_within(pid, Duration::from_secs(5));
                stuck.push(round);
            }
        }
    }

    stop.store(true, Ordering::Relaxed);
    scanning.join().unwrap();
    assert!(
        stuck.is_empty(),
        "rounds {stuck:?} forked a child that never got the $PATH cache lock"
    );
}
