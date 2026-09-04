//! A carapace spec file, from the disk to the Tab key.
//!
//! The unit tests take the reader and the walk apart; this is the whole path in one piece — a
//! `.yaml` on disk, found by the name of the command being typed, its flags parsed, its positions
//! answered. It is the claim `docs/features/completion-and-matching.md` makes, tested rather than
//! asserted.
//!
//! Requires the `spec` feature: without it there is no reader, and the file is a file.
#![cfg(feature = "compgen")]

use oslo::env::Environment;
use oslo::ui::OsloHelper;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// The example from the front page of the carapace-spec book, plus the positions a shell cares
/// about most.
const SPEC: &str = r#"
name: mycmd
description: my command
flags:
  --optarg?: optarg flag
  -r, --repeatable*: repeatable flag
  -v=: flag with value
persistentflags:
  --config=: which config
completion:
  flag:
    optarg: ["one", "two\twith description"]
    v: ["alpha", "beta"]
    config: ["a.toml", "b.toml"]
  positional:
    - ["first-a", "first-b"]
    - ["$directories"]
  positionalany: ["rest"]
commands:
  - name: sub
    description: subcommand
    aliases: [s]
    completion:
      positional:
        - ["deep"]
      dash:
        - ["after"]
"#;

fn helper() -> OsloHelper {
    let mut h = OsloHelper::new(Arc::new(Mutex::new(Environment::new())));
    h.set_menu(false);
    h
}

fn displays(h: &OsloHelper, line: &str) -> Vec<String> {
    let (_, cands) = h.candidates(line, line.len());
    cands.into_iter().map(|c| c.display).collect()
}

/// One directory of completions, pointed at by `$OSLO_COMPLETION`, for the whole file.
///
/// One test rather than several: `$OSLO_COMPLETION` is process-wide, and two tests setting it would be
/// taking turns with each other's environment. The spec cache is per name and per thread, so a
/// second helper in the same test sees the same answers without re-reading the file.
#[test]
fn a_spec_file_is_found_by_name_and_answers_for_every_position() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("mycmd.yaml"), SPEC).unwrap();
    std::fs::create_dir(dir.path().join("adir")).unwrap();
    std::fs::write(dir.path().join("afile"), "").unwrap();
    // SAFETY: the variable is read by `oslo_shell::spec::directories` and by nothing else, and
    // this is the only test in the binary that touches it.
    unsafe { std::env::set_var("OSLO_COMPLETION", dir.path()) };
    oslo::ui::spec::custom::set_loader(Some(std::rc::Rc::new(oslo::spec::find)));

    let h = helper();

    // The subcommand and its alias, from `commands`.
    let first = displays(&h, "mycmd ");
    assert!(first.contains(&"sub".to_string()), "{first:?}");
    assert!(first.contains(&"s".to_string()), "{first:?}");
    // …beside the first declared position.
    assert!(first.contains(&"first-a".to_string()), "{first:?}");

    // Flags, with their modifiers read: `--optarg?` takes a value, `-v=` takes one.
    let flags = displays(&h, "mycmd -");
    for name in ["--optarg", "-r", "--repeatable", "-v", "--config"] {
        assert!(flags.contains(&name.to_string()), "{name} in {flags:?}");
    }

    // `completion.flag`, keyed on the longhand and reached through either spelling.
    assert_eq!(displays(&h, "mycmd -v "), vec!["alpha", "beta"]);
    assert_eq!(
        displays(&h, "mycmd --optarg=t"),
        vec!["two".to_string()],
        "an optional argument is completed where it is written"
    );

    // A persistent flag, at a depth it was not declared at.
    assert_eq!(
        displays(&h, "mycmd sub --config "),
        vec!["a.toml", "b.toml"]
    );

    // Positions count past a flag that took a value: `alpha` belongs to `-v`, so this is still
    // the first position — subcommands and all.
    assert_eq!(
        displays(&h, "mycmd -v alpha "),
        vec!["first-a", "first-b", "s", "sub"]
    );
    // `$directories` is oslo's own path completion, filtered.
    let second = displays(&h, &format!("mycmd first-a {}/", dir.path().display()));
    assert!(second.contains(&"adir/".to_string()), "{second:?}");
    assert!(!second.contains(&"afile".to_string()), "{second:?}");
    // …and everything past the declared positions falls to `positionalany`.
    assert_eq!(displays(&h, "mycmd first-a x "), vec!["rest"]);

    // The subcommand's own positions, under its alias, and its dash position.
    assert_eq!(displays(&h, "mycmd s "), vec!["deep"]);
    assert_eq!(displays(&h, "mycmd sub -- "), vec!["after"]);

    oslo::ui::spec::custom::set_loader(None);
    oslo::ui::spec::custom::forget();
    unsafe { std::env::remove_var("OSLO_COMPLETION") };
}

/// **Every spec shipped in `share/completion` parses, and parses into something.**
///
/// There are ~1,200 of them and they are *generated* — from Fig's TypeScript and from argc's shell
/// comments — so nobody reads them. A converter that starts emitting something the reader cannot
/// take is a silent, total failure for whichever commands it touched, and this is the only place
/// that would notice.
///
/// It also fixes the reader's cost per file at a number somebody has to look at: these are read on
/// a keystroke path, and the largest is a third of a megabyte.
#[test]
fn every_shipped_spec_parses() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("share/completion");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return; // A checkout that has not run `scripts/completion.sh` has nothing to check.
    };

    let start = std::time::Instant::now();
    let (mut seen, mut flags, mut biggest) = (0usize, 0usize, (0u64, String::new()));
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
            continue;
        }
        let bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
        if bytes > biggest.0 {
            biggest = (bytes, path.display().to_string());
        }
        let source = std::fs::read_to_string(&path).expect("readable");
        let spec = oslo::spec::read::spec(&source)
            .unwrap_or_else(|problem| panic!("{}: {problem}", path.display()));
        // A spec that parses to nothing at all is a conversion that quietly produced an empty
        // file — which reads as "this command has no completions" rather than as a failure.
        assert!(!spec.name.is_empty(), "{}: no name", path.display());
        assert!(
            !spec.options.is_empty()
                || !spec.persistent.is_empty()
                || !spec.subcommands.is_empty()
                || !spec.positional.is_empty()
                || !spec.positional_any.is_none(),
            "{}: parsed to an empty spec",
            path.display()
        );
        flags += spec.options.len();
        seen += 1;
    }

    assert!(
        seen > 500,
        "only {seen} specs; the corpus is meant to be ~1,200"
    );
    println!(
        "{seen} specs, {flags} top-level flags, largest {} bytes ({}), {:?} total",
        biggest.0,
        biggest.1,
        start.elapsed()
    );
}
