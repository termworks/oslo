//! Watch-mode integration for `.make.lua`.

#![cfg(all(feature = "make", feature = "watch"))]

mod common;

use common::oslo_bin;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use std::io::BufRead;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

struct Project(tempfile::TempDir);

impl Project {
    fn new(makefile: &str) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(".make.lua"), makefile).expect("make file");
        Self(dir)
    }

    fn path(&self) -> &Path {
        self.0.path()
    }

    fn command(&self) -> Command {
        let mut command = Command::new(oslo_bin());
        command
            .current_dir(self.path())
            .env("HOME", self.path())
            .env("XDG_CONFIG_HOME", self.path().join("config"))
            .env("XDG_DATA_HOME", self.path().join("data"))
            .env_remove("ENV")
            .stdin(Stdio::null());
        command
    }
}

fn lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

fn await_lines(path: &Path, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while lines(path).len() < count && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(lines(path).len(), count, "{}", lines(path).join("\n"));
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn watch_uses_the_plan_imports_future_files_and_exact_target_argv() {
    let project = Project::new(
        r#"
local make = oslo.make
make.import("recipes.lua")
make.recipe { name = "target", deps = { "dependency" }, inputs = { "src/**/*.txt" },
  run = function(a)
    local out = assert(io.open("runs.txt", "a"))
    out:write(a.rest[1] .. "|" .. a.rest[2] .. "\n")
    out:close()
  end }
"#,
    );
    let imported = r#"
oslo.make.recipe { name = "dependency", inputs = { "dep.txt", "dep.txt" },
  run = function()
    local out = assert(io.open("dependency-runs.txt", "a"))
    out:write("ran\n")
    out:close()
  end }
"#;
    std::fs::write(project.path().join("recipes.lua"), imported).expect("import");
    std::fs::write(project.path().join("dep.txt"), "old").expect("dependency input");
    std::fs::create_dir(project.path().join("src")).expect("source root");

    let mut child = project
        .command()
        .args([
            "make",
            "--watch",
            "--postpone",
            "target",
            "alpha beta",
            "literal$?",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("watch make");
    let mut errors = std::io::BufReader::new(child.stderr.take().expect("stderr"));
    let mut ready = String::new();
    errors.read_line(&mut ready).expect("ready");
    assert!(ready.contains("ready"), "{ready:?}");
    assert!(lines(&project.path().join("runs.txt")).is_empty());
    assert!(lines(&project.path().join("dependency-runs.txt")).is_empty());

    std::fs::write(project.path().join("dep.txt"), "changed").expect("dependency change");
    await_lines(&project.path().join("runs.txt"), 1);

    std::fs::create_dir(project.path().join("src/nested")).expect("future directory");
    std::fs::write(project.path().join("src/nested/new.txt"), "new").expect("future input");
    await_lines(&project.path().join("runs.txt"), 2);

    std::fs::write(project.path().join("recipes.lua"), imported).expect("import change");
    await_lines(&project.path().join("runs.txt"), 3);

    let makefile = std::fs::read_to_string(project.path().join(".make.lua")).expect("make file");
    std::fs::write(project.path().join(".make.lua"), makefile).expect("make file change");
    await_lines(&project.path().join("runs.txt"), 4);

    assert_eq!(
        lines(&project.path().join("runs.txt")),
        vec!["alpha beta|literal$?"; 4]
    );
    assert_eq!(lines(&project.path().join("dependency-runs.txt")).len(), 4);
    kill(Pid::from_raw(child.id() as i32), Signal::SIGTERM).expect("terminate");
    assert_eq!(child.wait().expect("wait").code(), Some(143));
}

#[test]
fn watch_refuses_a_plan_without_declared_inputs_without_running_it() {
    let project = Project::new(
        r#"
oslo.make.recipe { name = "empty", run = function() oslo.fs.write("ran", "yes") end }
"#,
    );
    let output = project
        .command()
        .args(["make", "--watch", "empty"])
        .output()
        .expect("make");
    assert_eq!(output.status.code(), Some(2), "{}", text(&output));
    assert!(
        text(&output).contains("declare recipe inputs"),
        "{}",
        text(&output)
    );
    assert!(!project.path().join("ran").exists());
}
