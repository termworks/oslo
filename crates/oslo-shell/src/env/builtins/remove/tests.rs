//! `rm`, exercised against a real temporary directory.
//!
//! The safety property under test is the one in the module docs: a **script** must see the `rm` it
//! has always seen. Everything else here is ordinary behaviour, but that one is why the builtin is
//! allowed to exist.

use super::*;
use crate::env::Environment;

/// A shell that is, or is not, at a prompt.
fn shell(interactive: bool) -> Environment {
    let mut env = Environment::new();
    env.set_option(ShellOption::Interactive, interactive);
    env
}

fn argv(words: &[&str]) -> Vec<String> {
    std::iter::once("rm")
        .chain(words.iter().copied())
        .map(str::to_string)
        .collect()
}

fn run(env: &mut Environment, words: &[&str]) -> i32 {
    builtin_rm(env, &argv(words)).expect("rm never fails the shell")
}

/// A directory with `file`, `dir/` and `dir/inner` in it.
fn tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a temporary directory");
    std::fs::write(dir.path().join("file"), b"contents").unwrap();
    std::fs::create_dir(dir.path().join("dir")).unwrap();
    std::fs::write(dir.path().join("dir/inner"), b"deeper").unwrap();
    dir
}

fn path(dir: &tempfile::TempDir, name: &str) -> String {
    dir.path().join(name).display().to_string()
}

fn gone(dir: &tempfile::TempDir, name: &str) -> bool {
    std::fs::symlink_metadata(dir.path().join(name)).is_err()
}

/// **The property the whole design rests on.** A script gets POSIX `rm`: a directory without `-r`
/// is an error, and nothing is moved anywhere.
#[test]
fn a_script_gets_the_rm_it_has_always_had() {
    let dir = tree();
    let mut env = shell(false);

    assert_eq!(
        run(&mut env, &[&path(&dir, "dir")]),
        1,
        "a directory without -r must fail, as it does in every other shell"
    );
    assert!(!gone(&dir, "dir"), "and must still be there");

    assert_eq!(run(&mut env, &["-r", &path(&dir, "dir")]), 0);
    assert!(gone(&dir, "dir"), "-r removes it");
}

/// The same shell at a prompt takes the directory without being asked twice.
#[test]
fn a_prompt_removes_a_directory_without_r() {
    let dir = tree();
    assert_eq!(run(&mut shell(true), &[&path(&dir, "dir")]), 0);
    assert!(gone(&dir, "dir"));
}

/// `-s` asks for the script behaviour at a prompt.
#[test]
fn strict_puts_the_prompt_back_under_posix_rules() {
    let dir = tree();
    let mut env = shell(true);
    assert_eq!(run(&mut env, &["-s", &path(&dir, "dir")]), 1);
    assert_eq!(run(&mut env, &["--strict", &path(&dir, "dir")]), 1);
    assert!(!gone(&dir, "dir"));
}

/// Several operands in one line, and the status reflects the whole run rather than the last one.
#[test]
fn every_operand_is_attempted_and_one_failure_shows() {
    let dir = tree();
    let mut env = shell(true);
    let status = run(
        &mut env,
        &[
            &path(&dir, "file"),
            &path(&dir, "missing"),
            &path(&dir, "dir"),
        ],
    );
    assert_eq!(status, 1, "the missing one failed");
    assert!(gone(&dir, "file"), "but the others were still removed");
    assert!(gone(&dir, "dir"), "including the ones after the failure");
}

/// `-f` makes a missing operand a non-event, which is what every `rm -f` in every script relies on.
#[test]
fn force_forgives_what_is_not_there() {
    let dir = tree();
    let mut env = shell(false);
    assert_eq!(run(&mut env, &["-f", &path(&dir, "missing")]), 0);
    assert_eq!(run(&mut env, &["-f"]), 0, "and with no operands at all");
    assert_eq!(run(&mut env, &[]), 1, "without -f that is an error");
}

/// POSIX names `.` and `..` explicitly. `rm -r .` walking the working directory from a line that
/// looks local is the reason.
#[test]
fn dot_and_dotdot_are_refused() {
    let mut env = shell(true);
    assert_eq!(run(&mut env, &["-rf", "."]), 1);
    assert_eq!(run(&mut env, &["-rf", ".."]), 1);
    // Even spelled the long way, which is how it arrives from a glob.
    let dir = tree();
    assert_eq!(run(&mut env, &["-rf", &path(&dir, "dir/..")]), 1);
    assert!(!gone(&dir, "dir"));
}

/// A symlink to a directory is one entry, not the directory it names. Following it would make
/// `rm link` delete whatever it pointed at.
#[test]
fn a_symlink_to_a_directory_is_removed_not_followed() {
    let dir = tree();
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(dir.path().join("dir"), &link).unwrap();

    assert_eq!(run(&mut shell(true), &[&link.display().to_string()]), 0);
    assert!(gone(&dir, "link"), "the link is gone");
    assert!(!gone(&dir, "dir"), "and what it pointed at is not");
    assert!(!gone(&dir, "dir/inner"));
}

/// Under the cap, a removal at a prompt is a move. The file is still readable afterwards, which is
/// the entire promise.
#[test]
fn a_trashed_file_is_moved_rather_than_destroyed() {
    let dir = tree();
    let bin = tempfile::tempdir().unwrap();
    let mode = Mode {
        loose: true,
        trash: Some(trash::Trash::new(&oslo_ui::settings::Rm {
            to_tmp: true,
            max_to_tmp: 100,
            trash: bin.path().display().to_string(),
        })),
    };
    let options = Options {
        force: false,
        interactive: false,
        recursive: false,
        dir: false,
        verbose: false,
        strict: false,
    };

    let file = dir.path().join("file");
    assert!(matches!(
        remove_operand(&file, "file", &options, &mode, "oslo: "),
        Removal::Gone
    ));
    assert!(gone(&dir, "file"), "gone from where it was");
    assert_eq!(
        std::fs::read_to_string(bin.path().join("file")).unwrap(),
        "contents",
        "and intact where it went"
    );
}

/// Over the cap it is destroyed instead, which is the whole point of having a cap.
#[test]
fn a_file_over_the_cap_is_destroyed() {
    let dir = tempfile::tempdir().unwrap();
    let bin = tempfile::tempdir().unwrap();
    let big = dir.path().join("big");
    std::fs::write(&big, vec![0u8; 4096]).unwrap();

    // A cap of zero megabytes: everything with a byte in it is over it.
    let trash = trash::Trash::new(&oslo_ui::settings::Rm {
        to_tmp: true,
        max_to_tmp: 0,
        trash: bin.path().display().to_string(),
    });
    assert!(
        trash.take(&big, "big", false).is_none(),
        "over the cap the trash declines and the caller unlinks"
    );

    let small = dir.path().join("small");
    std::fs::write(&small, b"").unwrap();
    assert!(
        trash.take(&small, "small", false).is_some(),
        "an empty file is not over a cap of zero"
    );
}

/// Two files of the same name from two places must both survive in the trash. A feature that
/// exists to prevent data loss must not lose data itself.
#[test]
fn a_name_already_in_the_trash_does_not_overwrite_it() {
    let bin = tempfile::tempdir().unwrap();
    let one = tempfile::tempdir().unwrap();
    let two = tempfile::tempdir().unwrap();
    std::fs::write(one.path().join("notes.txt"), b"first").unwrap();
    std::fs::write(two.path().join("notes.txt"), b"second").unwrap();

    let trash = trash::Trash::new(&oslo_ui::settings::Rm {
        to_tmp: true,
        max_to_tmp: 100,
        trash: bin.path().display().to_string(),
    });
    trash
        .take(&one.path().join("notes.txt"), "notes.txt", false)
        .unwrap()
        .unwrap();
    trash
        .take(&two.path().join("notes.txt"), "notes.txt", false)
        .unwrap()
        .unwrap();

    assert_eq!(
        std::fs::read_to_string(bin.path().join("notes.txt")).unwrap(),
        "first"
    );
    assert_eq!(
        std::fs::read_to_string(bin.path().join("notes.txt.1")).unwrap(),
        "second",
        "the second is numbered rather than dropped"
    );
}

/// A whole directory goes to the trash with its contents.
#[test]
fn a_trashed_directory_keeps_what_was_in_it() {
    let dir = tree();
    let bin = tempfile::tempdir().unwrap();
    let trash = trash::Trash::new(&oslo_ui::settings::Rm {
        to_tmp: true,
        max_to_tmp: 100,
        trash: bin.path().display().to_string(),
    });
    trash
        .take(&dir.path().join("dir"), "dir", true)
        .unwrap()
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(bin.path().join("dir/inner")).unwrap(),
        "deeper"
    );
}

/// `-d` is `rmdir`: an empty directory goes, a full one does not. It is not a quieter `-r`, and
/// treating it as one deleted a tree that GNU refuses to touch.
#[test]
fn d_removes_an_empty_directory_and_only_an_empty_one() {
    let dir = tree();
    std::fs::create_dir(dir.path().join("empty")).unwrap();
    let mut env = shell(false);

    assert_eq!(run(&mut env, &["-d", &path(&dir, "empty")]), 0);
    assert!(gone(&dir, "empty"));

    assert_eq!(run(&mut env, &["-d", &path(&dir, "dir")]), 1);
    assert!(!gone(&dir, "dir/inner"), "and its contents are untouched");
}

/// The options, including the two that undo each other.
#[test]
fn the_last_of_f_and_i_wins() {
    let line = argv(&["-fi", "x"]);
    let Parsed::Options(o, operands) = parse(&line) else {
        panic!("options");
    };
    assert!(o.interactive && !o.force, "-fi ends interactive");
    assert_eq!(operands.len(), 1);

    let line = argv(&["-if", "x"]);
    let Parsed::Options(o, _) = parse(&line) else {
        panic!("options");
    };
    assert!(o.force && !o.interactive, "-if ends forced");

    let line = argv(&["-rvd", "x"]);
    let Parsed::Options(o, _) = parse(&line) else {
        panic!("options");
    };
    assert!(o.recursive && o.verbose && o.dir, "and they bundle");
}

/// `--` ends the options, and a leading `-` is a filename that needs it.
#[test]
fn a_double_dash_ends_the_options() {
    let line = argv(&["--", "-r", "file"]);
    let Parsed::Options(o, operands) = parse(&line) else {
        panic!("options");
    };
    assert!(!o.recursive, "-r after -- is a filename");
    assert_eq!(operands.len(), 2);

    // A bare `-` has always been an operand rather than an option.
    let line = argv(&["-"]);
    let Parsed::Options(_, operands) = parse(&line) else {
        panic!("options");
    };
    assert_eq!(operands.len(), 1);
}

/// **An option this does not implement is not an error.** A script running `rm --one-file-system`
/// must keep working after oslo takes the name over, so the real `rm` is handed the whole line.
#[test]
fn an_unknown_option_goes_to_the_real_rm() {
    assert!(matches!(
        parse(&argv(&["--one-file-system", "-rf", "x"])),
        Parsed::Delegate
    ));
    assert!(matches!(parse(&argv(&["-I", "x"])), Parsed::Delegate));
    assert!(
        external_rm().is_some(),
        "and there has to be one to hand it to"
    );
}

/// **A name on `$PATH` that is really this shell must not be handed the work.**
///
/// The test used to be against the literal `/usr/bin/oslo`, so it matched exactly one install and
/// missed the shape oslo already ships: a symlink named for another program. `resolve_program`
/// answers the link rather than its target, so the old check never fired and `rm` was handed to
/// oslo — which then reported a *shell* usage error for an `rm` flag.
#[test]
fn this_shell_is_recognised_through_a_symlink() {
    let exe = std::env::current_exe().expect("a test binary has a path");
    assert!(
        super::is_this_shell(&exe),
        "the running binary is this shell"
    );

    let dir = tempfile::tempdir().unwrap();
    let link = dir.path().join("rm");
    std::os::unix::fs::symlink(&exe, &link).unwrap();
    assert!(
        super::is_this_shell(&link),
        "a link named `rm` pointing here is still here"
    );
}

/// A real program is not this shell, and neither is a path that resolves to nothing.
#[test]
fn another_program_is_not_this_shell() {
    for other in ["/usr/bin/rm", "/bin/sh", "/nonexistent/rm"] {
        let path = std::path::Path::new(other);
        assert!(!super::is_this_shell(path), "{other} was taken for oslo");
    }
}

/// **`-i` alone is not a prompt, and this is the worst bug in the file if it is treated as one.**
///
/// `sh -i -c '…'` is an ordinary scripted form — the `bash -ic` trick that loads interactive rc
/// files for nvm and direnv, `SHELL='bash -i'` in a Makefile, ssh and CI shims. With the flag alone
/// as the test, a bare `rm dir` in one of those became `rm -rf dir` and answered **0**: POSIX and
/// bash both refuse a directory without `-r` and exit 1, so the caller could not tell a tree had
/// been deleted — and `to_tmp` is off by default, so there was no trash to recover it from.
#[test]
fn a_command_string_is_not_a_prompt() {
    let dir = tree();
    let mut env = shell(true);
    env.set_option(ShellOption::CommandString, true);
    assert_eq!(
        run(&mut env, &[&path(&dir, "dir")]),
        1,
        "rm dir must refuse"
    );
    assert!(!gone(&dir, "dir"), "the tree must survive");
}

/// And neither is a program read from standard input with `-s`.
#[test]
fn a_script_on_stdin_is_not_a_prompt() {
    let dir = tree();
    let mut env = shell(true);
    env.set_option(ShellOption::StdinInput, true);
    assert_eq!(run(&mut env, &[&path(&dir, "dir")]), 1);
    assert!(!gone(&dir, "dir"));
}
