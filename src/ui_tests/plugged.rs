//! What a config or a plugin can add to completion, and what it cannot take over.
//!
//! Split from `ui_tests.rs` at the 600-line limit. These are the tests about *extension* — declared
//! specs, ghost providers, completion providers — as opposed to the built-in behaviour above them.

use super::{displays, env_with_path, helper, make_exe};
use crate::env::Environment;

// ------------------------------------------------------------- spec-driven completion

#[test]
fn subcommands_come_from_the_spec_for_this_command_not_the_first_one() {
    let h = helper(Environment::new());
    // `ls |` starts a new command, so the spec looked up must be `git`, not `ls`.
    let names = displays(&h, "ls | git comm");
    assert!(names.contains(&"commit".to_string()), "{names:?}");

    let flags = displays(&h, "git commit --a");
    assert!(flags.iter().any(|f| f.starts_with("--a")), "{flags:?}");
}

/// **A provider's ghost reaches the line, in the position the config gave it.** The whole point of
/// `Source::Provider`: a plugin joins the order rather than jumping it.
#[test]
fn a_registered_provider_answers_the_ghost() {
    use crate::ui::settings::{self, Source};
    use crate::ui::suggest::{self, Ask, Only, Provider};

    let mut with_provider = settings::current().as_ref().clone();
    with_provider.suggest.sh_sources = vec![Source::Provider];
    settings::install(with_provider);

    suggest::forget();
    suggest::register(Provider {
        name: "tldr".into(),
        ask: Ask::Now(std::rc::Rc::new(|ctx| {
            ctx.line
                .starts_with("git com")
                .then(|| "git commit --amend".to_string())
        })),
        only: Only::default(),
    });

    let h = helper(Environment::new());
    assert_eq!(h.suggest("git com", 7).as_deref(), Some("mit --amend"));
    // Declining leaves the line alone rather than offering something else's answer.
    assert_eq!(h.suggest("ls -l", 5), None);

    suggest::forget();
    assert_eq!(h.suggest("git com", 7), None, "and gone once forgotten");
    settings::install(settings::Settings::default());
}

/// **A spec declared at runtime completes exactly like one compiled in.** This is the whole point of
/// `CommandSpec` owning its strings: before it, the only route was a `for_command` function that had
/// to re-implement subcommand matching by hand.
#[test]
fn a_spec_declared_from_outside_reaches_the_tab_key() {
    use crate::ui::spec::{CommandSpec, OptionSpec, SubcommandSpec, custom};
    custom::forget();
    custom::register(CommandSpec {
        name: "notes".into(),
        description: "notes kept in the shell".into(),
        subcommands: vec![SubcommandSpec {
            name: "list".into(),
            description: "every note".into(),
            options: vec![OptionSpec::new(
                vec!["--since".into()],
                "only newer than".into(),
            )],
            ..SubcommandSpec::default()
        }],
        ..CommandSpec::default()
    });
    let h = helper(Environment::new());

    assert!(displays(&h, "notes li").contains(&"list".to_string()));
    assert!(displays(&h, "notes list --si").contains(&"--since".to_string()));
    custom::forget();

    // And it is gone once forgotten, rather than living on in a registry built at startup.
    assert!(!displays(&h, "notes li").contains(&"list".to_string()));
}

#[test]
fn a_name_that_is_both_a_builtin_and_a_file_is_offered_once() {
    let dir = tempfile::tempdir().unwrap();
    make_exe(dir.path(), "echo");
    let h = helper(env_with_path(dir.path()));

    let names = displays(&h, "ech");
    assert_eq!(
        names.iter().filter(|n| *n == "echo").count(),
        1,
        "{names:?}"
    );
}

// ------------------------------------------------------------- path suggestions

/// fish's third autosuggestion source: the argument, which neither history nor the command index
/// can answer for.
#[test]
fn a_path_argument_is_suggested_from_the_filesystem() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("notes.txt"), b"x").unwrap();
    std::fs::create_dir(dir.path().join("build")).unwrap();

    let h = helper(Environment::new());
    let base = dir.path().display().to_string();

    let line = format!("cat {base}/not");
    assert_eq!(
        h.path_hint(&line, line.len()).as_deref(),
        Some("es.txt"),
        "no suggestion for {line}"
    );

    // A directory is suggested with its trailing slash, so the next keystroke continues into it.
    let line = format!("ls {base}/bui");
    assert_eq!(h.path_hint(&line, line.len()).as_deref(), Some("ld/"));
}

/// A bare word at the start of a line is a command to look up, not a file in the working
/// directory — suggesting `./notes.txt` for `no` would be nonsense.
///
/// Absolute paths throughout: `set_current_dir` is process-wide, and this binary runs its tests on
/// sixteen threads at once, so changing the working directory here would move it under every
/// other test that happened to be running.
#[test]
fn a_command_word_is_never_suggested_as_a_path() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("notes.txt"), b"x").unwrap();
    let base = dir.path().display().to_string();
    let h = helper(Environment::new());

    // In command position, even a stem that names a real file suggests nothing as a path — a data
    // file is not something that could run.
    let line = format!("{base}/not");
    assert_eq!(h.path_hint(&line, line.len()), None);

    // The same stem as an argument is fair game.
    let line = format!("cat {base}/not");
    assert_eq!(h.path_hint(&line, line.len()).as_deref(), Some("es.txt"));

    // **But a command written as a path is still a command**, and one that runs is exactly what
    // was meant: `./bui` reaches nothing on `$PATH`, so without this it had no ghost from any
    // source at all. Executables and directories only, which is the rule bash follows.
    use std::os::unix::fs::PermissionsExt;
    let script = dir.path().join("build.sh");
    std::fs::write(&script, b"#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let line = format!("{base}/bui");
    assert_eq!(h.path_hint(&line, line.len()).as_deref(), Some("ld.sh"));
}

/// Every argument would otherwise suggest `.git`, which is never what was meant.
#[test]
fn a_dotfile_is_only_suggested_once_the_dot_is_typed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".hidden"), b"x").unwrap();
    let base = dir.path().display().to_string();
    let h = helper(Environment::new());

    let line = format!("cat {base}/");
    assert_eq!(h.path_hint(&line, line.len()), None);

    let line = format!("cat {base}/.hid");
    assert_eq!(h.path_hint(&line, line.len()).as_deref(), Some("den"));
}

// ------------------------------------------------------------- badge and description

/// A description must never restate the badge beside it.
///
/// `examples/  [ dir ]  Directory` is the same fact written twice, and it costs the width the
/// name could have used. IRIS's rule is the one followed here: where the *kind* is the whole
/// story the badge carries it alone; only a kind that leaves something unsaid gets both.
#[test]
fn a_candidate_does_not_repeat_its_kind_in_its_description() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("examples")).unwrap();
    std::fs::write(dir.path().join("notes.txt"), b"x").unwrap();

    let h = helper(Environment::new());
    let line = format!("cat {}/", dir.path().display());
    let (_, candidates) = h.candidates(&line, line.len());
    assert!(!candidates.is_empty(), "no candidates for {line}");

    for candidate in &candidates {
        let Some(description) = candidate.description.as_deref() else {
            continue;
        };
        let kind = candidate.kind.as_deref().unwrap_or("");
        assert!(
            !description.to_lowercase().contains(kind),
            "{:?} describes itself as {description:?}, which its {kind:?} badge already says",
            candidate.display
        );
    }
}

/// A file or directory says everything it has to say in its badge, so the description column is
/// left out entirely — which is what gives the names the width back.
#[test]
fn file_and_directory_candidates_carry_no_description() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("build")).unwrap();
    std::fs::write(dir.path().join("readme"), b"x").unwrap();

    let h = helper(Environment::new());
    let line = format!("cat {}/", dir.path().display());
    let (_, candidates) = h.candidates(&line, line.len());

    let kinds: Vec<&str> = candidates
        .iter()
        .filter_map(|c| c.kind.as_deref())
        .collect();
    assert!(kinds.contains(&"dir"), "{kinds:?}");
    assert!(kinds.contains(&"file"), "{kinds:?}");
    assert!(
        candidates.iter().all(|c| c.description.is_none()),
        "{:?}",
        candidates
            .iter()
            .map(|c| (&c.display, &c.description))
            .collect::<Vec<_>>()
    );
}

/// A description that says something the badge cannot is still shown — the rule is about
/// redundancy, not about suppressing descriptions.
#[test]
fn a_description_that_adds_something_survives() {
    let h = helper(Environment::new());
    let candidates = h.candidates("git comm", 8).1;
    let commit = candidates
        .iter()
        .find(|c| c.display == "commit")
        .expect("git commit should be offered");
    assert!(
        commit.description.is_some(),
        "a subcommand's description is not implied by its badge: {commit:?}"
    );
}

/// **The tldr case, end to end.** A provider's offers appear *beside* what oslo already knows about
/// the command rather than instead of it — which is the whole difference from `for_command`.
#[test]
fn a_completion_provider_adds_to_what_oslo_already_offers() {
    use crate::ui::completion::provider::{self, DEFAULT_MAX_ITEMS, Offer, Provider};

    provider::forget();
    provider::register(Provider {
        name: "tldr".into(),
        kind: "example".into(),
        when: Some("git".into()),
        score_offset: 0.0,
        max_items: DEFAULT_MAX_ITEMS,
        min_chars: 0,
        enabled: None,
        answer: std::rc::Rc::new(|_| {
            vec![Offer {
                display: "commit --amend".into(),
                description: Some("change the last commit".into()),
            }]
        }),
    });

    let h = helper(Environment::new());
    let names = displays(&h, "git com");
    assert!(names.contains(&"commit --amend".to_string()), "{names:?}");
    // oslo's own git spec still answers — the provider added, it did not take over.
    assert!(names.contains(&"commit".to_string()), "{names:?}");

    // Its kind is its own, so `oslo.completion.sources` can name it.
    let (_, candidates) = h.candidates("git com", 7);
    let offered = candidates
        .iter()
        .find(|c| c.display == "commit --amend")
        .expect("offered");
    assert_eq!(offered.kind.as_deref(), Some("example"));
    assert_eq!(
        offered.description.as_deref(),
        Some("change the last commit")
    );

    // Only what continues the word being typed.
    assert!(!displays(&h, "git pu").contains(&"commit --amend".to_string()));

    provider::forget();
    assert!(!displays(&h, "git com").contains(&"commit --amend".to_string()));
}

// ------------------------------------------- the carapace model: positions, flag values, dashes

/// `deploy [-v] [--env=dev|staging] build|clean <target> [--] <rest>`, with `--config=` inherited.
fn deploy_spec() -> crate::ui::spec::CommandSpec {
    use crate::ui::spec::{Action, Arg, CommandSpec, OptionSpec, SubcommandSpec};
    let with_values = |names: &[&str], values: &[&str]| OptionSpec {
        names: names.iter().map(|n| n.to_string()).collect(),
        takes: Arg::Required,
        values: Action::list(values.iter().copied()),
        ..OptionSpec::default()
    };
    CommandSpec {
        name: "deploy".into(),
        options: vec![OptionSpec::new(vec!["-v".into()], "say more".into())],
        persistent: vec![with_values(&["--config"], &["a.toml", "b.toml"])],
        subcommands: vec![SubcommandSpec {
            name: "build".into(),
            aliases: vec!["b".into()],
            description: "make it".into(),
            options: vec![with_values(&["--env"], &["dev", "staging\tthe shared one"])],
            positional: vec![Action::list(["alpha", "beta"])],
            positional_any: Action::list(["more"]),
            dash: vec![Action::list(["after-one"])],
            ..SubcommandSpec::default()
        }],
        ..CommandSpec::default()
    }
}

fn with_deploy() -> crate::ui::OsloHelper {
    crate::ui::spec::custom::forget();
    crate::ui::spec::custom::register(deploy_spec());
    helper(Environment::new())
}

/// **The word after a flag that takes a value is that flag's value.** Before the walk parsed
/// flags, this was the first positional argument and the menu offered the wrong list.
#[test]
fn a_flags_declared_values_are_what_follows_it() {
    let h = with_deploy();
    assert_eq!(displays(&h, "deploy build --env "), vec!["dev", "staging"]);
    // …and the position after it is still the first one.
    assert_eq!(
        displays(&h, "deploy build --env dev "),
        vec!["alpha", "beta"]
    );
    crate::ui::spec::custom::forget();
}

/// The same value list, written into the flag's own word.
#[test]
fn an_inline_flag_value_completes_after_the_equals() {
    let h = with_deploy();
    let line = "deploy build --env=st";
    let (at, cands) = h.candidates(line, line.len());
    let names: Vec<String> = cands.into_iter().map(|c| c.display).collect();
    assert_eq!(names, vec!["staging".to_string()]);
    // Written over the value alone: the flag and its `=` stay on the line.
    assert_eq!(at, "deploy build --env=".len());
    crate::ui::spec::custom::forget();
}

#[test]
fn positions_are_counted_and_the_catch_all_takes_the_rest() {
    let h = with_deploy();
    assert_eq!(displays(&h, "deploy build "), vec!["alpha", "beta"]);
    assert_eq!(displays(&h, "deploy build alpha "), vec!["more"]);
    assert_eq!(displays(&h, "deploy build alpha beta "), vec!["more"]);
    // A switch consumes nothing, so it does not move the count.
    assert_eq!(displays(&h, "deploy -v build "), vec!["alpha", "beta"]);
    crate::ui::spec::custom::forget();
}

#[test]
fn a_bare_dash_dash_starts_its_own_positions() {
    let h = with_deploy();
    assert_eq!(displays(&h, "deploy build -- "), vec!["after-one"]);
    crate::ui::spec::custom::forget();
}

/// A persistent flag is answered for at every depth, and only declared once.
#[test]
fn a_persistent_flag_reaches_the_subcommands() {
    let h = with_deploy();
    assert!(displays(&h, "deploy build --con").contains(&"--config".to_string()));
    assert_eq!(
        displays(&h, "deploy build --config "),
        vec!["a.toml", "b.toml"]
    );
    crate::ui::spec::custom::forget();
}

#[test]
fn a_subcommand_completes_under_its_alias_and_answers_to_it() {
    let h = with_deploy();
    assert!(displays(&h, "deploy b").contains(&"b".to_string()));
    assert_eq!(displays(&h, "deploy b "), vec!["alpha", "beta"]);
    crate::ui::spec::custom::forget();
}

/// **`$files` is oslo's own path completion, not a second one.** The badge, the trailing slash and
/// the suffix filter all come from the builder every other path in the shell goes through.
#[test]
fn a_files_macro_offers_real_paths_filtered_the_way_it_asked() {
    use crate::ui::spec::{Action, CommandSpec, custom};
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("one.rs"), "").unwrap();
    std::fs::write(dir.path().join("two.txt"), "").unwrap();
    std::fs::create_dir(dir.path().join("inner")).unwrap();

    custom::forget();
    custom::register(CommandSpec {
        name: "sources".into(),
        positional: vec![Action::list(["$files([.rs])"])],
        ..CommandSpec::default()
    });
    let h = helper(Environment::new());
    let line = format!("sources {}/", dir.path().display());
    let names = displays(&h, &line);
    assert!(names.contains(&"one.rs".to_string()), "{names:?}");
    assert!(!names.contains(&"two.txt".to_string()), "{names:?}");
    // A directory stays whatever the filter says: it is the way to reach the files under it.
    assert!(names.contains(&"inner/".to_string()), "{names:?}");
    custom::forget();
}

/// **A declared position that came back empty is still an answer.** Falling through to the
/// filenames there would offer the working directory where the flag wanted one of two words.
#[test]
fn a_declared_position_does_not_fall_through_to_the_filesystem() {
    use crate::ui::spec::{Action, CommandSpec, custom};
    custom::forget();
    custom::register(CommandSpec {
        name: "narrow".into(),
        positional: vec![Action::list(["only-this"])],
        ..CommandSpec::default()
    });
    let h = helper(Environment::new());
    assert!(displays(&h, "narrow zzz").is_empty());
    // …where a command with no spec at all still completes paths.
    assert!(!displays(&h, "unknown-command ").is_empty());
    custom::forget();
}

/// **`\cp` is the command, not the alias.** A backslash or quotes are how a shell is told to skip
/// the alias table, and completion followed the alias anyway — `\cp <Tab>` offered the hosts of the
/// `rsync` that `cp` was aliased to.
#[test]
fn a_quoted_command_word_does_not_complete_as_its_alias() {
    let mut env = Environment::new();
    env.set_alias("g", "git");
    let h = helper(env);
    assert!(displays(&h, "g comm").contains(&"commit".to_string()));
    for line in [r"\g comm", "'g' comm", "\"g\" comm"] {
        let names = displays(&h, line);
        assert!(!names.contains(&"commit".to_string()), "{line}: {names:?}");
    }
}
