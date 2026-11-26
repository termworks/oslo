//! `oslo macros` through the real binary: scripts as files in their directory, and where a name
//! resolves.

mod common;

use common::oslo_bin;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A macro store and a scripts directory of its own.
struct Dirs {
    data: tempfile::TempDir,
    bin: tempfile::TempDir,
}

fn dirs() -> Dirs {
    Dirs {
        data: tempfile::tempdir().expect("data"),
        bin: tempfile::tempdir().expect("bin"),
    }
}

impl Dirs {
    fn bin(&self) -> &Path {
        self.bin.path()
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(oslo_bin());
        command
            .args(args)
            .env("XDG_DATA_HOME", self.data.path())
            .env("XDG_CONFIG_HOME", self.data.path())
            .env("OSLO_MACROS_BIN", self.bin())
            .env_remove("VISUAL")
            .stdin(Stdio::null());
        command
    }

    /// `$PATH` with the scripts directory first.
    fn path_first(&self) -> String {
        format!("{}:{}", self.bin().display(), system_path())
    }

    /// `$PATH` with the scripts directory last.
    fn path_last(&self) -> String {
        format!("{}:{}", system_path(), self.bin().display())
    }

    /// `oslo -c LINE` under `path`, stdout only.
    fn run(&self, line: &str, path: &str) -> String {
        let out = self
            .command(&["-c", line])
            .env("PATH", path)
            .output()
            .expect("spawn oslo");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// `oslo macros …`, stdout and stderr together.
    fn said(&self, args: &[&str]) -> String {
        self.said_with(args, &[])
    }

    fn said_with(&self, args: &[&str], env: &[(&str, &str)]) -> String {
        let mut command = self.command(args);
        command.envs(env.iter().copied());
        let out = command.output().expect("spawn oslo");
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    }

    /// `oslo macros import`, from `text`.
    fn import(&self, text: &str) {
        use std::io::Write;
        let mut child = self
            .command(&["macros", "import"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("spawn oslo");
        let stdin = child.stdin.as_mut().expect("stdin");
        stdin.write_all(text.as_bytes()).expect("write");
        child.wait().expect("wait");
    }

    /// A script dropped into the directory by hand.
    fn script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.bin().join(name);
        std::fs::write(&path, body).expect("write");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        path
    }

    /// An editor that appends `# edited` to the file it is given.
    fn appending_editor(&self) -> String {
        let path = self.data.path().join("editor");
        std::fs::write(&path, "#!/bin/sh\necho '# edited' >> \"$1\"\n").expect("write");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        path.display().to_string()
    }
}

fn system_path() -> String {
    std::env::var("PATH").unwrap_or_default()
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).expect("stat").permissions().mode() & 0o777
}

/// A file in the directory runs by name, through `$PATH`, like any program.
#[test]
fn a_script_in_the_directory_runs_from_path() {
    let at = dirs();
    at.script("oslo-probe", "#!/bin/sh\necho ran-the-script\n");
    assert_eq!(at.run("oslo-probe", &at.path_first()), "ran-the-script");
    assert_eq!(
        at.said(&["macros", "run", "oslo-probe"]).trim(),
        "ran-the-script",
        "`run` needs no `$PATH` entry"
    );
}

/// The directory is one more entry on `$PATH`: a program earlier on it wins.
#[test]
fn a_program_earlier_on_path_wins() {
    let at = dirs();
    at.script("date", "#!/bin/sh\necho I-AM-THE-SCRIPT\n");
    let out = at.run("date", &at.path_last());
    assert!(!out.contains("I-AM-THE-SCRIPT"), "{out:?}");
}

/// `type` and `command -v` report a script as the file it is.
#[test]
fn a_script_is_reported_as_its_file() {
    let at = dirs();
    let path = at
        .script("oslo-probe", "#!/bin/sh\necho hi\n")
        .display()
        .to_string();
    let first = at.path_first();
    assert_eq!(at.run("command -v oslo-probe", &first), path);
    assert_eq!(
        at.run("command -V oslo-probe", &first),
        format!("oslo-probe is {path}")
    );
    assert_eq!(at.run("type -t oslo-probe", &first), "file");
}

/// A stored function is still the database's, found after `$PATH` and named as one.
#[test]
fn a_stored_function_is_reported_as_one() {
    let at = dirs();
    at.import("func oslo-fn\n\techo from-the-function\n");
    let path = system_path();
    assert_eq!(at.run("oslo-fn", &path), "from-the-function");
    assert_eq!(
        at.run("command -V oslo-fn", &path),
        "oslo-fn is a stored function"
    );
    assert_eq!(at.run("type -t oslo-fn", &path), "function");
    assert_eq!(at.run("command -v oslo-fn", &path), "oslo-fn");
}

/// Type `line` at an interactive oslo under `path` and collect what it prints.
fn at_a_prompt(at: &Dirs, line: &str, path: &str) -> String {
    use std::io::Write;
    let mut child = at
        .command(&["-i"])
        .env("PATH", path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn oslo");
    let stdin = child.stdin.as_mut().expect("stdin");
    stdin.write_all(line.as_bytes()).expect("write");
    let out = child.wait_with_output().expect("wait");
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// `which` and `whereis` print a script's path, and say what a stored function is.
#[test]
fn which_and_whereis_tell_a_script_from_a_function() {
    let at = dirs();
    let path = at
        .script("oslo-probe", "#!/bin/sh\necho hi\n")
        .display()
        .to_string();
    at.import("func oslo-fn\n\techo fn\n");

    let out = at_a_prompt(
        &at,
        "which oslo-probe\nwhereis oslo-probe\n",
        &at.path_first(),
    );
    assert!(out.contains(&path), "{out:?}");
    assert!(!out.contains("stored script"), "{out:?}");
    let out = at_a_prompt(&at, "which oslo-fn\n", &at.path_first());
    assert!(out.contains("oslo-fn: stored function"), "{out:?}");
}

/// An interactive shell puts the directory at the front of `$PATH` when `$PATH` lacks it, and
/// leaves a `$PATH` that already holds it alone.
#[test]
fn an_interactive_shell_puts_the_directory_on_path() {
    let at = dirs();
    let bin = at.bin().display().to_string();
    let out = at_a_prompt(&at, "echo \"[$PATH]\"\n", &system_path());
    assert!(out.contains(&format!("[{bin}:")), "{out:?}");
    let out = at_a_prompt(&at, "echo \"[$PATH]\"\n", &at.path_last());
    assert!(out.contains(&format!(":{bin}]")), "moved: {out:?}");
}

/// Any file with a shebang dropped into the directory is listed, tags and all; others are not.
#[test]
fn a_dropped_in_file_is_listed_with_its_tags() {
    let at = dirs();
    at.script(
        "oslo-probe",
        "#!/usr/bin/env bash\n# tags: net, work\necho hi\n",
    );
    std::fs::write(at.bin().join("notes"), "no shebang here\n").expect("write");

    let listed = at.said(&["macros", "show", "--plain"]);
    assert!(
        listed.contains("script\toslo-probe\ton\tnet,work\t"),
        "{listed:?}"
    );
    assert!(!listed.contains("notes"), "{listed:?}");
    let shown = at.said(&["macros", "show", "oslo-probe"]);
    assert!(shown.contains("# tags: net, work"), "{shown:?}");
}

/// Off moves the file into `.off/`, where nothing finds it, and on moves it back.
#[test]
fn off_moves_the_file_out_of_reach() {
    let at = dirs();
    at.script("oslo-probe", "#!/bin/sh\necho hi\n");
    at.said(&["macros", "off", "oslo-probe"]);

    assert!(!at.bin().join("oslo-probe").exists());
    assert!(at.bin().join(".off/oslo-probe").exists());
    assert_eq!(at.run("command -v oslo-probe", &at.path_first()), "");
    let listed = at.said(&["macros", "show", "--plain"]);
    assert!(listed.contains("script\toslo-probe\toff\t"), "{listed:?}");

    at.said(&["macros", "on", "oslo-probe"]);
    assert!(at.bin().join("oslo-probe").exists());
    assert_eq!(at.run("oslo-probe", &at.path_first()), "hi");
}

/// Removing a script deletes its file.
#[test]
fn removing_a_script_deletes_the_file() {
    let at = dirs();
    at.script("oslo-probe", "#!/bin/sh\n");
    let out = at.said(&["macros", "remove", "oslo-probe"]);
    assert!(out.contains("removed script oslo-probe"), "{out:?}");
    assert!(!at.bin().join("oslo-probe").exists());
}

/// `edit` opens the file itself, and leaves its mode as it was.
#[test]
fn edit_opens_the_file_itself() {
    let at = dirs();
    let path = at.script("oslo-probe", "#!/bin/sh\necho hi\n");
    let editor = at.appending_editor();
    let out = at.said_with(&["macros", "edit", "oslo-probe"], &[("EDITOR", &editor)]);
    assert!(out.contains("script oslo-probe"), "{out:?}");
    let body = std::fs::read_to_string(&path).expect("read");
    assert_eq!(body, "#!/bin/sh\necho hi\n# edited\n");
    assert_eq!(mode(&path), 0o755);

    let out = at.said_with(&["macros", "edit", "oslo-probe"], &[("EDITOR", "true")]);
    assert!(out.contains("unchanged"), "{out:?}");
}

/// `add --script` writes the file from the starter, tags in its header; a script the editor
/// never saved is not kept.
#[test]
fn add_writes_the_file_and_keeps_only_a_saved_one() {
    let at = dirs();
    let out = at.said_with(
        &["macros", "add", "--script", "oslo-new"],
        &[("EDITOR", "true")],
    );
    assert!(out.contains("unchanged"), "{out:?}");
    assert!(!at.bin().join("oslo-new").exists());

    let editor = at.appending_editor();
    let out = at.said_with(
        &["macros", "add", "--script", "oslo-new", "--tag", "net"],
        &[("EDITOR", &editor)],
    );
    assert!(out.contains("script oslo-new"), "{out:?}");
    let path = at.bin().join("oslo-new");
    let body = std::fs::read_to_string(&path).expect("written");
    assert!(
        body.starts_with("#!/usr/bin/env bash\n# tags: net\n"),
        "{body:?}"
    );
    assert!(body.ends_with("# edited\n"), "{body:?}");
    assert_eq!(mode(&path), 0o700);
}

/// `import` writes a script as its file, with its tags in the header, for any shell to run.
#[test]
fn an_imported_script_is_a_file_another_shell_runs() {
    let at = dirs();
    at.import("script greet-from-file #net\n\t#!/bin/sh\n\techo from-the-file\n");
    let body = std::fs::read_to_string(at.bin().join("greet-from-file")).expect("written");
    assert_eq!(body, "#!/bin/sh\n# tags: net\necho from-the-file\n");

    let out = Command::new("/bin/sh")
        .arg("-c")
        .arg("greet-from-file")
        .env("PATH", at.path_first())
        .output()
        .expect("sh");
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "from-the-file");
}

/// A name a script file and a stored alias share is reported, and `add` refuses to make one.
#[test]
fn a_name_two_kinds_hold_is_reported() {
    let at = dirs();
    at.import("alias oslo-probe\n\techo alias\n");
    at.script("oslo-probe", "#!/bin/sh\n");
    let listed = at.said(&["macros", "show", "--plain"]);
    assert!(
        listed.contains("oslo-probe is more than one kind: alias, script"),
        "{listed:?}"
    );

    at.script("oslo-other", "#!/bin/sh\n");
    let refused = at.said(&["macros", "add", "--alias", "oslo-other", "echo x"]);
    assert!(refused.contains("already a script"), "{refused:?}");
}

// ─────────────────────────────────────────── what `add` means, made the same for every kind

/// **`add` was the one command that could create an ambiguous name.** `remove` and `edit` both
/// refuse a name owned by more than one kind and ask which was meant, so `add` refuses to make one.
#[test]
fn adding_a_name_another_kind_owns_is_refused() {
    let at = dirs();
    at.said(&["macros", "add", "--var", "DUP=1"]);

    let refused = at.said(&["macros", "add", "--alias", "DUP", "echo x"]);
    assert!(refused.contains("already a var"), "{refused:?}");

    // Storing over the *same* kind is an ordinary update and must not be caught by this.
    let update = at.said(&["macros", "add", "--var", "DUP=2"]);
    assert!(
        !update.contains("already a"),
        "an update was refused\n{update:?}"
    );
}

/// A flag that silently does nothing is worse than one that does not exist. `--replace` is read
/// only by `import`, and every other subcommand accepted it and ignored it.
#[test]
fn replace_is_refused_where_it_means_nothing() {
    let at = dirs();
    let refused = at.said(&["macros", "add", "--replace", "--var", "A=1"]);
    assert!(refused.contains("--replace is only for"), "{refused:?}");
    assert!(refused.contains("import"), "{refused:?}");
}
