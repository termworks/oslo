//! The fixture both coordinate suites drive the real binary against.
//!
//! Included by `#[path]` rather than shared through `tests/common`, because the fixture files are
//! this feature's and nobody else's — `hosts.txt` having three columns is load-bearing for these
//! assertions and meaningless to every other suite.

use crate::common::oslo_bin;
use std::process::Command;

/// Run `line` through `-i -c` in a directory holding a small fixture.
///
/// Interactive, because coordinates are prompt syntax and a script reads them as bash does.
#[track_caller]
pub fn shell(line: &str) -> String {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("hosts.txt"),
        "web-01  10.0.0.1  nginx\nweb-02  10.0.0.2  apache\ndb-01   10.0.0.9  postgres\n",
    )
    .expect("fixture");
    std::fs::write(dir.path().join("spaced.txt"), "my file.txt  100\n").expect("fixture");
    std::fs::write(dir.path().join("glob.txt"), "*.txt\n").expect("fixture");
    let out = Command::new(oslo_bin())
        .arg("-i")
        .arg("-c")
        .arg(line)
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .env("PATH", "/usr/bin:/bin")
        .env_remove("ENV")
        .output()
        .expect("spawn oslo");
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    text.trim_end().to_string()
}
