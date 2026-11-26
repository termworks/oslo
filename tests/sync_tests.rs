//! `oslo sync` — history, macros and secrets carried to another machine in one command.
//!
//! Driven through the real binary against two data directories with a stand-in for `ssh`, because
//! what is under test is two *machines*. An in-process test would exercise each merge and miss
//! every one of the things that actually broke: an empty snapshot from a machine that has none of
//! something, and the files a merge has to write afterwards.

mod common;

use common::oslo_bin;
use std::path::Path;
use std::process::Command;

/// Whether this build has a secret store to carry.
///
/// **CI runs `cargo test --all-targets` with default features, and `default = []`** — so the binary
/// under test there has no `secret` tool at all, and a test that assumed one failed on four counts
/// while passing locally under `--all-features`. A runtime `cfg!` rather than an attribute on every
/// line: both branches then have to keep compiling, which is the point.
const SECRETS: bool = cfg!(feature = "secrets");

/// One machine: its own data, state and home.
struct Machine {
    home: tempfile::TempDir,
}

impl Machine {
    fn new() -> Machine {
        Machine {
            home: tempfile::tempdir().expect("tempdir"),
        }
    }

    fn path(&self) -> &Path {
        self.home.path()
    }

    fn run(&self, args: &[&str]) -> (String, String, i32) {
        self.run_with(args, &[], &[])
    }

    fn feed(&self, args: &[&str], input: &[u8]) -> (String, String, i32) {
        self.run_with(args, &[], input)
    }

    fn run_with(
        &self,
        args: &[&str],
        extra: &[(&str, String)],
        input: &[u8],
    ) -> (String, String, i32) {
        use std::io::Write;
        let mut command = Command::new(oslo_bin());
        command
            .args(args)
            .env("HOME", self.path())
            .env("XDG_DATA_HOME", self.path())
            .env("XDG_STATE_HOME", self.path().join("state"))
            .env_remove("OSLO_PROFILE")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        for (name, value) in extra {
            command.env(name, value);
        }
        let mut child = command.spawn().expect("spawn oslo");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(input)
            .expect("write");
        let out = child.wait_with_output().expect("wait");
        (
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
            out.status.code().unwrap_or(-1),
        )
    }

    fn remember(&self, lines: &[&str]) {
        let file = self.path().join("seed.txt");
        std::fs::write(&file, format!("{}\n", lines.join("\n"))).expect("write");
        let (_, err, status) = self.run(&["history", "import", &file.to_string_lossy()]);
        assert_eq!(status, 0, "{err}");
    }

    /// Seed every kind of macro through the import format, which needs no editor.
    fn seed_macros(&self, text: &str) {
        let file = self.path().join("macros.txt");
        std::fs::write(&file, text).expect("write");
        let (_, err, status) = self.run(&["macros", "import", &file.to_string_lossy()]);
        assert_eq!(status, 0, "{err}");
    }

    fn macro_names(&self) -> Vec<String> {
        let (out, err, status) = self.run(&["macros", "show", "--plain"]);
        assert_eq!(status, 0, "{err}");
        let mut found: Vec<String> = out
            .lines()
            .filter_map(|row| {
                let mut fields = row.split('\t');
                let kind = fields.next()?;
                let name = fields.next()?;
                Some(format!("{kind}/{name}"))
            })
            .collect();
        found.sort();
        found
    }

    /// Every secret this machine holds, or nothing at all in a build that has none.
    fn secret_names(&self) -> Vec<String> {
        if !SECRETS {
            return Vec::new();
        }
        let (out, _, _) = self.run(&["secret", "list"]);
        let mut found: Vec<String> = out.lines().map(str::to_string).collect();
        found.sort();
        found
    }

    /// Keep a secret, where the build has somewhere to keep one.
    fn keep_secret(&self, name: &str, value: &[u8]) {
        if !SECRETS {
            return;
        }
        let (_, err, status) = self.feed(&["secret", "set", name], value);
        assert_eq!(status, 0, "{err}");
    }

    fn lines(&self) -> Vec<String> {
        let (out, err, status) = self.run(&["history", "list"]);
        assert_eq!(status, 0, "{err}");
        let mut found: Vec<String> = out
            .lines()
            .filter_map(|row| row.rsplit('\t').next().map(str::to_string))
            .collect();
        found.sort();
        found
    }
}

fn fake_ssh(at: &Path, machine: &Path) -> String {
    let script = at.join("fake-ssh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nshift\nexec env HOME={home} XDG_DATA_HOME={home} \
             XDG_STATE_HOME={home}/state \"$@\"\n",
            home = machine.display()
        ),
    )
    .expect("write");
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .expect("chmod");
    script.to_string_lossy().to_string()
}

/// Carry the key across, which is the one step a person does by hand.
fn pair(here: &Machine, there: &Machine) {
    here.run(&["profile", "key", "init"]);
    let (exported, err, status) = here.run(&["profile", "export"]);
    assert_eq!(status, 0, "{err}");
    let (_, err, status) = there.feed(&["profile", "import"], exported.as_bytes());
    assert_eq!(status, 0, "{err}");
}

fn syncing(here: &Machine, there: &Machine, extra: &[&str]) -> (String, String, i32) {
    let ssh = fake_ssh(here.path(), there.path());
    let mut args = vec!["profile", "sync", "buildbox"];
    args.extend_from_slice(extra);
    here.run_with(
        &args,
        &[
            ("OSLO_SSH", ssh),
            (
                "OSLO_SSH_REMOTE_BIN",
                oslo_bin().to_string_lossy().to_string(),
            ),
        ],
        &[],
    )
}

/// Every kind of macro, written the way `oslo macros export` writes them.
const FIVE_KINDS: &str = "alias gs\n\tgit status\n\
                          abbrev gco\n\tgit checkout\n\
                          var GREETING\n\thello\n\
                          func hello\n\techo hi\n\
                          script deploy\n\t#!/bin/sh\n\techo deploying\n";

/// The whole point: one command, and all three arrive.
#[test]
fn one_command_carries_history_macros_and_secrets() {
    let (here, there) = (Machine::new(), Machine::new());
    pair(&here, &there);

    here.remember(&["cargo build"]);
    there.remember(&["kubectl get pods"]);
    here.seed_macros("alias gs\n\tgit status\n");
    there.seed_macros("alias ll\n\tls -la\n");
    here.keep_secret("deploy", b"deploy-value");
    there.keep_secret("api", b"api-value");

    let (said, err, status) = syncing(&here, &there, &[]);
    assert_eq!(status, 0, "{err}");
    for part in ["history", "macros"] {
        assert!(said.contains(part), "{part} was not reported:\n{said}");
    }
    assert_eq!(
        said.contains("secrets"),
        SECRETS,
        "the secrets part was reported by a build that has none, or missed by one that has:\n{said}"
    );

    assert_eq!(here.lines(), there.lines());
    assert_eq!(here.macro_names(), there.macro_names());
    assert_eq!(here.secret_names(), there.secret_names());

    if SECRETS {
        // **Carried sealed and opened on the other side.** The value never crosses as plaintext;
        // both machines derive the same store key from the profile key they share.
        let (value, err, status) = there.run(&["secret", "get", "deploy"]);
        assert_eq!(status, 0, "{err}");
        assert_eq!(value, "deploy-value");
    }
}

/// **A machine that has none of something can still sync.** It sends no bytes at all, and a
/// zero-length file is not a database — writing one made the merge fail with "the macros that
/// arrived cannot be opened".
#[test]
fn a_machine_with_nothing_yet_is_not_an_empty_file() {
    let (here, there) = (Machine::new(), Machine::new());
    pair(&here, &there);

    // `there` has never made a macro or a secret, and only `here` has any history.
    here.remember(&["cargo build"]);
    there.remember(&["kubectl get pods"]);
    here.seed_macros(FIVE_KINDS);
    here.keep_secret("deploy", b"deploy-value");

    let (_, err, status) = syncing(&here, &there, &[]);
    assert_eq!(status, 0, "{err}");
    assert_eq!(there.macro_names(), here.macro_names());
    assert_eq!(there.secret_names(), here.secret_names());
}

/// Every kind travels, not only the two a starting shell reads.
#[test]
fn all_five_kinds_of_macro_travel() {
    let (here, there) = (Machine::new(), Machine::new());
    pair(&here, &there);
    here.remember(&["cargo build"]);
    there.remember(&["kubectl get pods"]);
    here.seed_macros(FIVE_KINDS);

    let (_, err, status) = syncing(&here, &there, &["--only", "macros"]);
    assert_eq!(status, 0, "{err}");

    let arrived = there.macro_names();
    for wanted in [
        "alias/gs",
        "abbrev/gco",
        "var/GREETING",
        "func/hello",
        "script/deploy",
    ] {
        assert!(
            arrived.iter().any(|name| name == wanted),
            "{wanted} did not travel: {arrived:?}"
        );
    }
}

/// An arriving script is a file in `~/.local/sbin` on the machine that received it, and the
/// startup kinds reach the snapshot a starting shell reads.
#[test]
fn an_arriving_script_is_a_file_where_other_programs_find_it() {
    let (here, there) = (Machine::new(), Machine::new());
    pair(&here, &there);
    here.remember(&["cargo build"]);
    there.remember(&["kubectl get pods"]);
    here.seed_macros(FIVE_KINDS);

    let (_, err, status) = syncing(&here, &there, &["--only", "macros"]);
    assert_eq!(status, 0, "{err}");

    let arrived = std::fs::read_to_string(there.path().join(".local/sbin/deploy"))
        .expect("the script did not arrive as a file");
    assert_eq!(arrived, "#!/bin/sh\necho deploying\n");

    let snapshot =
        std::fs::read_to_string(there.path().join("oslo/macros/macros.snapshot")).expect("read");
    for wanted in ["gs", "gco", "GREETING"] {
        assert!(snapshot.contains(wanted), "{wanted} is not in the snapshot");
    }
}

/// A script deleted on one machine is deleted on the other, and an edit made by hand to the file
/// reaches the far end.
#[test]
fn a_script_deletion_and_an_edit_travel() {
    let (here, there) = (Machine::new(), Machine::new());
    pair(&here, &there);
    here.remember(&["cargo build"]);
    there.remember(&["kubectl get pods"]);
    here.seed_macros(
        "script keep\n\t#!/bin/sh\n\techo one\n\
         script drop\n\t#!/bin/sh\n\techo gone\n",
    );
    let (_, err, status) = syncing(&here, &there, &["--only", "macros"]);
    assert_eq!(status, 0, "{err}");
    let theirs = there.path().join(".local/sbin");
    assert!(
        theirs.join("drop").exists(),
        "the first sync carried nothing"
    );

    let mine = here.path().join(".local/sbin");
    std::fs::remove_file(mine.join("drop")).expect("rm");
    std::fs::write(mine.join("keep"), "#!/bin/sh\necho two\n").expect("edit");
    let (_, err, status) = syncing(&here, &there, &["--only", "macros"]);
    assert_eq!(status, 0, "{err}");

    assert!(!theirs.join("drop").exists(), "the deletion did not travel");
    assert_eq!(
        std::fs::read_to_string(theirs.join("keep")).expect("read"),
        "#!/bin/sh\necho two\n"
    );
    assert!(!mine.join("drop").exists(), "the deletion came back");
}

/// Deleting on one machine removes it on the other, in all three — and the machine that lost it
/// does not hand it back.
#[test]
fn a_deletion_travels_in_every_part() {
    let (here, there) = (Machine::new(), Machine::new());
    pair(&here, &there);
    here.remember(&["cargo build"]);
    there.remember(&["kubectl get pods"]);
    here.seed_macros("alias gs\n\tgit status\n");
    here.keep_secret("deploy", b"deploy-value");
    syncing(&here, &there, &[]);
    assert_eq!(
        there.secret_names().iter().any(|name| name == "deploy"),
        SECRETS,
        "the secret did not travel"
    );

    // Delete on `here`, including a history event that was run on `there`.
    let (listed, _, _) = here.run(&["history", "list"]);
    let id = listed
        .lines()
        .find(|row| row.contains("kubectl"))
        .and_then(|row| row.split('\t').next())
        .expect("the event that came from there")
        .to_string();
    here.run(&["history", "delete", &id, "--yes"]);
    here.run(&["macros", "remove", "gs"]);
    here.run(&["secret", "rm", "deploy"]);

    let (_, err, status) = syncing(&here, &there, &[]);
    assert_eq!(status, 0, "{err}");
    assert!(!there.lines().iter().any(|line| line.contains("kubectl")));
    assert!(!there.macro_names().iter().any(|name| name == "alias/gs"));
    assert!(!there.secret_names().iter().any(|name| name == "deploy"));

    // And nothing comes back.
    let (said, err, status) = syncing(&here, &there, &[]);
    assert_eq!(status, 0, "{err}");
    assert!(said.contains("unchanged"), "{said}");
    assert!(!there.macro_names().iter().any(|name| name == "alias/gs"));
    assert!(!there.secret_names().iter().any(|name| name == "deploy"));
}

/// `--only` narrows it, and nothing else moves.
#[test]
fn only_carries_the_part_it_names() {
    let (here, there) = (Machine::new(), Machine::new());
    pair(&here, &there);
    here.remember(&["cargo build"]);
    there.remember(&["kubectl get pods"]);
    here.seed_macros("alias gs\n\tgit status\n");

    let (said, err, status) = syncing(&here, &there, &["--only", "macros"]);
    assert_eq!(status, 0, "{err}");
    assert!(said.contains("macros"), "{said}");
    assert!(!said.contains("secrets"), "{said}");

    assert!(there.macro_names().iter().any(|name| name == "alias/gs"));
    // The history stayed where it was.
    assert!(
        !there
            .lines()
            .iter()
            .any(|line| line.contains("cargo build"))
    );
}

/// **`oslo profile sync` carries everything too.**
///
/// It synced the history alone for a while, on the argument that macros and secrets are not part of
/// a profile — so the command somebody actually types moved a third of the machine and said nothing
/// about the rest. The two spellings take the same words and do the same work.
#[test]
fn profile_sync_is_the_same_command() {
    let (here, there) = (Machine::new(), Machine::new());
    pair(&here, &there);
    here.remember(&["cargo build"]);
    there.remember(&["kubectl get pods"]);
    here.seed_macros(FIVE_KINDS);
    here.keep_secret("deploy", b"deploy-value");

    let ssh = fake_ssh(here.path(), there.path());
    let (said, err, status) = here.run_with(
        &["profile", "sync", "buildbox"],
        &[
            ("OSLO_SSH", ssh),
            (
                "OSLO_SSH_REMOTE_BIN",
                oslo_bin().to_string_lossy().to_string(),
            ),
        ],
        &[],
    );
    assert_eq!(status, 0, "{err}");
    assert!(said.contains("macros"), "macros were not carried:\n{said}");
    assert_eq!(there.macro_names(), here.macro_names());
    assert_eq!(there.secret_names(), here.secret_names());

    // And it takes the same flags, or "the same command" is a claim the help cannot keep.
    let ssh = fake_ssh(here.path(), there.path());
    let (said, err, status) = here.run_with(
        &[
            "profile",
            "sync",
            "buildbox",
            "--only",
            "history",
            "--dry-run",
        ],
        &[
            ("OSLO_SSH", ssh),
            (
                "OSLO_SSH_REMOTE_BIN",
                oslo_bin().to_string_lossy().to_string(),
            ),
        ],
        &[],
    );
    assert_eq!(status, 0, "{err}");
    assert!(said.contains("dry run"), "{said}");
    assert!(!said.contains("macros"), "--only was ignored:\n{said}");
}

/// A part nobody has is refused by name rather than silently ignored.
#[test]
fn an_unknown_part_is_a_usage_error() {
    let (here, there) = (Machine::new(), Machine::new());
    pair(&here, &there);
    let (_, err, status) = syncing(&here, &there, &["--only", "nonsense"]);
    assert_eq!(status, 2, "{err}");
    assert!(err.contains("no such part"), "{err:?}");
}

/// Syncing twice moves nothing the second time, which is what makes it safe in a login file.
#[test]
fn syncing_again_moves_nothing() {
    let (here, there) = (Machine::new(), Machine::new());
    pair(&here, &there);
    here.remember(&["cargo build"]);
    there.remember(&["kubectl get pods"]);
    here.seed_macros("alias gs\n\tgit status\n");
    here.keep_secret("deploy", b"deploy-value");

    syncing(&here, &there, &[]);
    let (said, err, status) = syncing(&here, &there, &[]);
    assert_eq!(status, 0, "{err}");
    for part in ["history   unchanged", "macros    unchanged"] {
        assert!(said.contains(part), "{part:?} missing from:\n{said}");
    }
    assert_eq!(said.contains("secrets   unchanged"), SECRETS, "{said}");
}

/// A dry run says what would move and writes to neither machine.
#[test]
fn a_dry_run_writes_to_neither_machine() {
    let (here, there) = (Machine::new(), Machine::new());
    pair(&here, &there);
    here.remember(&["cargo build"]);
    there.remember(&["kubectl get pods"]);
    here.seed_macros("alias gs\n\tgit status\n");

    let (said, err, status) = syncing(&here, &there, &["--dry-run"]);
    assert_eq!(status, 0, "{err}");
    assert!(said.contains("dry run"), "{said}");
    assert!(!there.macro_names().iter().any(|name| name == "alias/gs"));
    assert!(
        !there
            .lines()
            .iter()
            .any(|line| line.contains("cargo build"))
    );
}

/// **A far end that cannot sync says so, rather than looking like an empty machine.**
///
/// An oslo without `sync` does not fail loudly: the word is not one of its tools, so it looks for a
/// program called `sync` — and `sync(1)` exists on most systems. It ran, printed nothing and exited
/// 0, which read as *that machine has no history* and then died somewhere else entirely.
#[test]
fn a_far_end_that_does_not_speak_sync_is_named_as_the_problem() {
    let (here, there) = (Machine::new(), Machine::new());
    pair(&here, &there);
    here.remember(&["cargo build"]);

    // A far end that answers every command with silence and success, as a stale oslo does.
    let quiet = here.path().join("quiet-ssh");
    std::fs::write(
        &quiet,
        format!(
            "#!/bin/sh\nshift\ncase \"$*\" in\n  *fingerprint*) exec env HOME={home} \
             XDG_DATA_HOME={home} XDG_STATE_HOME={home}/state \"$@\" ;;\n  *) exit 0 ;;\nesac\n",
            home = there.path().display()
        ),
    )
    .expect("write");
    std::fs::set_permissions(&quiet, std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .expect("chmod");

    let (_, err, status) = here.run_with(
        &["profile", "sync", "buildbox"],
        &[
            ("OSLO_SSH", quiet.to_string_lossy().to_string()),
            (
                "OSLO_SSH_REMOTE_BIN",
                oslo_bin().to_string_lossy().to_string(),
            ),
        ],
        &[],
    );
    assert_ne!(status, 0, "a silent far end was taken for an empty one");
    assert!(
        err.contains("did not answer as an oslo that can sync"),
        "{err:?}"
    );
}

/// Without a shared key nothing moves, and it says so before anything is copied.
#[test]
fn no_shared_key_means_no_sync() {
    let (here, there) = (Machine::new(), Machine::new());
    here.remember(&["cargo build"]);
    there.remember(&["kubectl get pods"]);

    let (_, err, status) = syncing(&here, &there, &[]);
    assert_ne!(status, 0, "it synced without a key");
    assert!(err.contains("has no key here"), "{err:?}");
}
