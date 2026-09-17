//! `oslo make` runs a recipe in the environment the directory declares.
//!
//! # What this exists to catch
//!
//! `oslo make` never evaluated `.env.lua` at all. A recipe therefore ran in whatever the calling
//! shell happened to be holding — the interactive session's cached load from whenever it last
//! entered the project, or nothing whatsoever when the command was typed from somewhere else. A
//! `.make.lua` could not rely on a single thing its own directory computes, and the failure is
//! silent: the value is simply absent, or stale, and the recipe carries on with it.
//!
//! The second case here is the other half of the same report, which had already been fixed by the
//! time it arrived: `oslo.env.set` reaches the real environment, so `os.getenv` and a child process
//! both see it. It is kept because nothing else asserts it from outside.

#![cfg(all(feature = "make", feature = "direnv"))]

mod common;

use common::oslo_bin;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// A project with a `.make.lua`, and whatever `.env.lua` the case wants, in a sandbox of its own.
struct Project {
    home: tempfile::TempDir,
    root: PathBuf,
}

impl Project {
    fn new(env_lua: Option<&str>, make_lua: &str) -> Project {
        let home = tempfile::tempdir().expect("tempdir");
        let root = home.path().canonicalize().expect("canonical").join("proj");
        std::fs::create_dir_all(&root).expect("project");
        std::fs::write(root.join(".make.lua"), make_lua).expect("write .make.lua");
        let project = Project { home, root };
        if let Some(body) = env_lua {
            std::fs::write(project.root.join(".env.lua"), body).expect("write .env.lua");
        }
        project
    }

    /// Approve the `.env.lua` through the real gate, as a person would.
    fn allow(&self) {
        let allowed = self.oslo(&["direnv", "allow"]);
        assert!(
            allowed.status.success(),
            "could not allow the file: {}",
            text(&allowed)
        );
    }

    fn oslo(&self, args: &[&str]) -> Output {
        self.oslo_in(&self.root, args)
    }

    fn oslo_in(&self, cwd: &Path, args: &[&str]) -> Output {
        Command::new(oslo_bin())
            .args(args)
            .current_dir(cwd)
            .env("HOME", self.home.path())
            .env("XDG_DATA_HOME", self.home.path().join("data"))
            .env("XDG_CONFIG_HOME", self.home.path().join("config"))
            .env("OSLO_SCRATCH_DIR", self.home.path().join("scratch"))
            .env_remove("ENV")
            .env_remove("FROM_THE_DIRECTORY")
            .stdin(Stdio::null())
            .output()
            .expect("spawn oslo")
    }
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The recipe every case runs: it reports one variable, however it was set.
const REPORTS_THE_VARIABLE: &str = r#"
oslo.make.recipe{ name = "show", desc = "report the variable", run = function()
  print("value=" .. tostring(oslo.env.get("FROM_THE_DIRECTORY")))
end }
"#;

/// **A recipe runs in the environment the directory declares.**
#[test]
fn a_recipe_sees_what_the_directory_environment_set() {
    let project = Project::new(
        Some("oslo.env.set('FROM_THE_DIRECTORY', 'yes')\n"),
        REPORTS_THE_VARIABLE,
    );
    project.allow();

    let out = project.oslo(&["make", "show"]);
    assert!(
        text(&out).contains("value=yes"),
        "the recipe ran without the directory's environment: {}",
        text(&out)
    );
}

/// From a subdirectory too: a `.env.lua` governs everything below it, and `oslo make` walks up to
/// the project for the recipe file anyway.
#[test]
fn a_recipe_run_from_below_sees_it_too() {
    let project = Project::new(
        Some("oslo.env.set('FROM_THE_DIRECTORY', 'deep')\n"),
        REPORTS_THE_VARIABLE,
    );
    project.allow();
    let deep = project.root.join("app/src");
    std::fs::create_dir_all(&deep).expect("deep");

    let out = project.oslo_in(&deep, &["make", "show"]);
    assert!(
        text(&out).contains("value=deep"),
        "a recipe run from a subdirectory missed the environment: {}",
        text(&out)
    );
}

/// **The allow list still governs.** An `.env.lua` nobody approved is not read, and the recipe
/// still runs — the same terms as at a prompt.
#[test]
fn an_unapproved_directory_environment_is_not_read() {
    let project = Project::new(
        Some("oslo.env.set('FROM_THE_DIRECTORY', 'should not happen')\n"),
        REPORTS_THE_VARIABLE,
    );

    let out = project.oslo(&["make", "show"]);
    let said = text(&out);
    assert!(
        said.contains("value=nil"),
        "an unapproved file was read: {said}"
    );
    assert!(
        said.contains("direnv") || said.contains("allow"),
        "nothing said the file was blocked: {said}"
    );
}

/// `oslo.env.set` reaches the real environment: `os.getenv` and a child both see it at once, in the
/// same script that set it.
#[test]
fn env_set_is_visible_to_getenv_and_to_a_child() {
    let project = Project::new(
        None,
        r#"
oslo.make.recipe{ name = "probe", desc = "set and read back", run = function()
  oslo.env.set("SET_WHILE_RUNNING", "here")
  print("getenv=" .. tostring(os.getenv("SET_WHILE_RUNNING")))
  local r = oslo.run{ "sh", "-c", "echo child=$SET_WHILE_RUNNING", capture = true }
  print(r.out)
end }
"#,
    );

    let out = project.oslo(&["make", "probe"]);
    let said = text(&out);
    assert!(said.contains("getenv=here"), "os.getenv missed it: {said}");
    assert!(said.contains("child=here"), "the child missed it: {said}");
}
