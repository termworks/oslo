//! A `.env.lua` runs in its own directory, and can say which one that is.
//!
//! # What this exists to catch
//!
//! The `.envrc` path chdir'd to the rc file's directory before running it; `source_lua` did not.
//! Both facts were written down — the `.envrc` path even carried a comment explaining why the
//! chdir was required — and nothing compared them, so oslo's *own* format was the one
//! that resolved relative paths against the wrong place.
//!
//! The failure is quiet and it persists. `~/proj/.env.lua` saying `path_add("./bin")`, entered as
//! `cd ~/proj/app/src`, put `~/proj/app/src/bin` on `$PATH` — a directory that does not exist — and
//! left it there for as long as you stood in the project. Which answer you got depended on how deep
//! you happened to walk in, so it worked when tested from the project root and only ever failed for
//! somebody in a subdirectory.
//!
//! No integration test drove a real `.env.lua` load before this file, which is the whole reason a
//! divergence this visible lasted.

#![cfg(feature = "direnv")]

mod common;

use common::oslo_bin;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// A project holding a `.env.lua`, in a sandbox of its own.
struct Sandbox {
    home: tempfile::TempDir,
    project: PathBuf,
}

impl Sandbox {
    fn new(body: &str) -> Sandbox {
        let home = tempfile::tempdir().expect("tempdir");
        // Canonical, because the assertions compare against what the shell reports and a temp
        // directory reached through a symlink would differ from it by the link rather than by a bug.
        let root = home.path().canonicalize().expect("canonical");
        let project = root.join("proj");
        std::fs::create_dir_all(project.join("bin")).expect("bin");
        std::fs::create_dir_all(project.join("app/src")).expect("deep");
        std::fs::write(project.join(".env.lua"), body).expect("write .env.lua");
        let sandbox = Sandbox { home, project };
        // Through the real gate rather than by writing a token: an rc file is inert until allowed,
        // and a test that skipped that would be testing a path no user reaches.
        let allowed = sandbox.oslo(&sandbox.project, &["direnv", "allow"]);
        assert!(
            allowed.status.success(),
            "could not allow the file: {}",
            text(&allowed)
        );
        sandbox
    }

    fn oslo(&self, cwd: &Path, args: &[&str]) -> Output {
        Command::new(oslo_bin())
            .args(args)
            .current_dir(cwd)
            .env("HOME", self.home.path())
            .env("XDG_DATA_HOME", self.home.path().join("data"))
            .env("XDG_CONFIG_HOME", self.home.path().join("config"))
            .env("OSLO_SCRATCH_DIR", self.home.path().join("scratch"))
            .env_remove("ENV")
            .stdin(Stdio::null())
            .output()
            .expect("spawn oslo")
    }

    /// Arrive in `cwd` — which loads the environment — and run one line there.
    ///
    /// **Down stdin, not through `-c`.** A directory environment is loaded by the read loop before
    /// each prompt, and `-c` never enters one — so a test written that way passes an empty `$PATH`
    /// assertion for the wrong reason and proves nothing at all.
    fn arriving_in(&self, cwd: &Path, line: &str) -> String {
        use std::io::Write;
        let mut child = Command::new(oslo_bin())
            .arg("-i")
            .current_dir(cwd)
            .env("HOME", self.home.path())
            .env("XDG_DATA_HOME", self.home.path().join("data"))
            .env("XDG_CONFIG_HOME", self.home.path().join("config"))
            .env("OSLO_SCRATCH_DIR", self.home.path().join("scratch"))
            .env_remove("ENV")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn oslo");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(format!("{line}\n").as_bytes())
            .expect("write");
        text(&child.wait_with_output().expect("wait"))
    }
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// **The bug, from the directory it actually bit.** Entered three levels down, `./bin` must still
/// mean the project's `bin` and not one below the shell's feet.
#[test]
fn a_relative_path_resolves_against_the_file_not_the_shell() {
    let sandbox = Sandbox::new("oslo.direnv.path_add(\"./bin\")\n");
    let deep = sandbox.project.join("app/src");
    let out = sandbox.arriving_in(&deep, "echo \"HEAD=${PATH%%:*}\"");

    let expected = sandbox.project.join("bin");
    assert!(
        out.contains(&format!("HEAD={}", expected.display())),
        "`./bin` resolved against the shell rather than the file.\nwanted {}\ngot: {out}",
        expected.display()
    );
    assert!(
        !out.contains("app/src/bin"),
        "the wrong directory reached $PATH: {out}"
    );
}

/// The working directory during the load is the file's own, which is direnv's rule too.
#[test]
fn the_file_runs_in_its_own_directory() {
    let sandbox = Sandbox::new("oslo.env.set(\"WHERE\", oslo.fs.cwd())\n");
    let deep = sandbox.project.join("app/src");
    let out = sandbox.arriving_in(&deep, "echo \"WHERE=$WHERE\"");
    assert!(
        out.contains(&format!("WHERE={}", sandbox.project.display())),
        "wanted {}\ngot: {out}",
        sandbox.project.display()
    );
}

/// **And the shell is left where the user walked to**, not where the file ran. Restoring the
/// directory is the half of the chdir that is easy to forget, and getting it wrong
/// would move somebody's shell under them on every `cd`.
#[test]
fn the_shell_is_not_left_standing_in_the_projects_root() {
    let sandbox = Sandbox::new("oslo.env.set(\"WHERE\", oslo.fs.cwd())\n");
    let deep = sandbox.project.join("app/src");
    let out = sandbox.arriving_in(&deep, "pwd");
    assert!(
        out.contains(&deep.display().to_string()),
        "the shell was moved by the load.\nwanted {}\ngot: {out}",
        deep.display()
    );
}

/// `oslo.direnv.dir()` names the directory whose file is running — for a path being *stored*
/// rather than used, where the working directory will have moved on by the time it is read.
#[test]
fn the_file_can_name_its_own_directory() {
    let sandbox = Sandbox::new("oslo.env.set(\"DIR\", tostring(oslo.direnv.dir()))\n");
    let deep = sandbox.project.join("app/src");
    let out = sandbox.arriving_in(&deep, "echo \"DIR=$DIR\"");
    assert!(
        out.contains(&format!("DIR={}", sandbox.project.display())),
        "wanted {}\ngot: {out}",
        sandbox.project.display()
    );
}

/// Outside a directory environment there is no file loading, so the honest answer is `nil` and the
/// caller falls back to the working directory. A path invented here would be a confident lie.
#[test]
fn there_is_no_directory_when_no_file_is_loading() {
    let home = tempfile::tempdir().expect("tempdir");
    let probe = home.path().join("probe.lua");
    std::fs::write(&probe, "print('dir=' .. tostring(oslo.direnv.dir()))\n").expect("write");
    let out = Command::new(oslo_bin())
        .arg(&probe)
        .current_dir(home.path())
        .env("HOME", home.path())
        .env("XDG_DATA_HOME", home.path().join("data"))
        .env("XDG_CONFIG_HOME", home.path().join("config"))
        .stdin(Stdio::null())
        .output()
        .expect("spawn oslo");
    let said = text(&out);
    assert!(said.contains("dir=nil"), "{said}");
}

/// **A builtin the directory registered goes away with the directory.**
///
/// `Direnv::unload` put back the prompt, the variables and the aliases and knew nothing about
/// builtins — so a `.env.lua` registering one left it callable for the rest of the session, running
/// the project's code with the project's environment already torn down.
/// `Names::unregister_custom_builtin` was written for exactly this and had no callers.
#[test]
fn a_builtin_the_directory_registered_leaves_with_it() {
    let sandbox = Sandbox::new(
        "oslo.register_builtin{ name = \"deploybuiltin\",\n\
           run = function() print(\"the project's own\") end }\n",
    );

    // While standing in it, the builtin is there and runs.
    let inside = sandbox.arriving_in(&sandbox.project, "deploybuiltin");
    assert!(
        inside.contains("the project's own"),
        "it runs while the directory is loaded: {inside}"
    );

    // Leave, and ask again in the same session.
    let after = sandbox.arriving_in(&sandbox.project, "cd ..\ndeploybuiltin");
    assert!(
        !after.contains("the project's own"),
        "and does not still run after leaving: {after}"
    );
    assert!(after.contains("not found"), "and is reported gone: {after}");
}

#[cfg(feature = "watch")]
#[test]
fn a_watch_command_starts_scoped_and_stops_on_leave() {
    let sandbox = Sandbox::new(
        r#"
oslo.direnv.watch_command {
  name = "env-check",
  paths = { "watched.txt" },
  run = { "sh", "-c", "echo ran >> runs.txt" },
  scratch = false,
}
"#,
    );
    std::fs::write(sandbox.project.join("watched.txt"), "").expect("watched");
    let said = sandbox.arriving_in(
        &sandbox.project,
        r#"sleep 0.2
printf first > watched.txt
sleep 0.4
cd ..
printf second > proj/watched.txt
sleep 0.3
"#,
    );
    let runs = std::fs::read_to_string(sandbox.project.join("runs.txt")).unwrap_or_default();
    assert_eq!(runs, "ran\n", "service did not stop on unload\n{said}");
}

#[cfg(feature = "watch")]
#[test]
fn a_watch_command_is_refused_outside_directory_loading() {
    let home = tempfile::tempdir().expect("tempdir");
    let probe = home.path().join("probe.lua");
    std::fs::write(
        &probe,
        r#"
local ok, err = pcall(function()
  oslo.direnv.watch_command { paths = { "x" }, run = { "true" } }
end)
print("refused=" .. tostring(not ok) .. " " .. tostring(err))
"#,
    )
    .expect("probe");
    let out = Command::new(oslo_bin())
        .arg(&probe)
        .current_dir(home.path())
        .env("HOME", home.path())
        .env("XDG_DATA_HOME", home.path().join("data"))
        .env("XDG_CONFIG_HOME", home.path().join("config"))
        .output()
        .expect("oslo");
    let said = text(&out);
    assert!(said.contains("refused=true"), "{said}");
    assert!(said.contains("only while"), "{said}");
}

#[cfg(all(feature = "watch", feature = "scratch"))]
#[test]
fn a_persistent_watch_survives_unload_until_explicitly_stopped() {
    let sandbox = Sandbox::new(
        r#"
oslo.direnv.watch_command {
  name = "persistent",
  paths = { "watched.txt" },
  run = { "sh", "-c", "echo ran >> runs.txt" },
  initial = false,
  persist = true,
  scratch = "env-persist",
}
"#,
    );
    std::fs::write(sandbox.project.join("watched.txt"), "").expect("watched");
    let said = sandbox.arriving_in(
        &sandbox.project,
        r#"sleep 0.5
cd ..
printf changed > proj/watched.txt
sleep 1
"#,
    );
    let runs = std::fs::read_to_string(sandbox.project.join("runs.txt")).unwrap_or_default();
    let log = std::fs::read_to_string(sandbox.home.path().join("scratch/env-persist.log"))
        .unwrap_or_default();
    assert_eq!(
        runs, "ran\n",
        "persistent service stopped on unload\n{said}\nscratch log:\n{log}"
    );
    let killed = sandbox.oslo(&sandbox.project, &["scratch", "-k", "env-persist"]);
    assert!(killed.status.success(), "{}", text(&killed));
}
