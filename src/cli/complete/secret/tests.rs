//! The spec's shape, through the completer, and the answers it computes from what is on disk.
//!
//! Nothing here reads the real store: the line-level tests ask only for subcommands and flags, and
//! the answers that read a store are handed a directory of their own.

use super::*;
use std::sync::{Arc, Mutex};

fn offered(line: &str) -> Vec<String> {
    super::super::register();
    let mut helper = oslo_ui::OsloHelper::new(Arc::new(Mutex::new(oslo::env::Environment::new())));
    helper.set_menu(false);
    helper
        .candidates(line, line.len())
        .1
        .into_iter()
        .map(|candidate| candidate.display)
        .collect()
}

/// **The rows are `--help`'s rows**, so a subcommand added there is completed with no second list.
#[test]
fn every_help_row_is_a_subcommand() {
    let spec = spec("about");
    for row in help::MENU.subs {
        let offered = spec.subcommands.iter().any(|sub| sub.name == row.name);
        let built = cfg!(feature = "crypt") || !matches!(row.name, "key" | "recipient");
        assert_eq!(offered, built, "{}", row.name);
    }
    let at_the_prompt = offered("oslo secret ");
    for name in [
        "set", "get", "run", "list", "rm", "rotate", "stores", "where",
    ] {
        assert!(
            at_the_prompt.iter().any(|o| o == name),
            "{name}: {at_the_prompt:?}"
        );
    }
}

/// `run`'s flags, and the `--store` every subcommand inherits.
#[test]
fn run_offers_its_flags_and_the_store() {
    let flags = offered("oslo secret run --");
    for flag in ["--file", "--env-file", "--all", "--prefix", "--store"] {
        assert!(flags.iter().any(|o| o == flag), "{flag}: {flags:?}");
    }
    let run = spec("about")
        .subcommands
        .into_iter()
        .find(|sub| sub.name == "run")
        .expect("run");
    let file = run
        .options
        .iter()
        .find(|f| f.names[0] == "--file")
        .expect("--file");
    assert_eq!(file.takes, Arg::Required, "--file takes VAR=NAME");
    assert!(matches!(file.values, Action::Call(_)));
    assert!(!run.positional_any.is_none(), "VAR=<Tab> is answered");
}

/// The nested menus: `key`, `recipient`, `cipher`, each with its own rows.
#[test]
#[cfg(feature = "crypt")]
fn the_nested_menus_complete_their_own_rows() {
    let recipient = offered("oslo secret recipient ");
    for name in ["list", "add", "rm"] {
        assert!(recipient.iter().any(|o| o == name), "{recipient:?}");
    }
    assert!(offered("oslo secret recipient rm --").contains(&"--from".to_string()));
    let key = offered("oslo secret key add ");
    for word in ["profile", "file", "command"] {
        assert!(key.iter().any(|o| o == word), "{key:?}");
    }
    let cipher = offered("oslo secret cipher ");
    assert!(cipher.contains(&"encrypt".to_string()), "{cipher:?}");
}

/// **After `VAR=` a name; before it, the `VAR=` that reads back as a name.**
#[test]
fn a_run_word_is_a_name_after_the_equals() {
    let names = || {
        vec![
            "gh-token".to_string(),
            "Mixed".to_string(),
            "a.b".to_string(),
        ]
    };
    let values =
        |offers: Vec<Offer>| -> Vec<String> { offers.into_iter().map(|o| o.value).collect() };
    assert_eq!(
        values(variable_offers(names(), "GH=")),
        ["gh-token", "Mixed", "a.b"]
    );
    assert_eq!(values(variable_offers(names(), "")), ["GH_TOKEN="]);
    assert!(variable_offers(names(), "--file=").is_empty());
    assert!(variable_offers(names(), "host:").is_empty());
    let colon = vec!["aws:prod".to_string(), "aws:dev".to_string()];
    assert_eq!(
        values(variable_offers(colon, "AWS=aws:")),
        ["prod", "dev"],
        "a `:` in the name is a second break"
    );
}

/// **A break is context, not text to replace.** `OSLO-PUB-1:` is cut off the word at the `:`, so a
/// recipient is offered as what follows it.
#[test]
fn a_value_is_offered_past_the_break() {
    let listed = vec![
        "OSLO-PUB-1:zyzEW4hw=".to_string(),
        "OSLO-PUB-1:jmtSV18H=".to_string(),
    ];
    let values: Vec<String> = past(listed.clone(), "OSLO-PUB-1:")
        .into_iter()
        .map(|o| o.value)
        .collect();
    assert_eq!(values, ["zyzEW4hw=", "jmtSV18H="]);
    assert_eq!(past(listed, "").len(), 2, "a whole word takes them whole");
}

/// `--store <Tab>`: `user`, the declared ones, and the directories a store or a plugin left.
#[test]
fn the_stores_are_the_declared_ones_and_the_ones_on_disk() {
    let data = tempfile::tempdir().expect("tempdir");
    for directory in [
        "stores/work",
        "stores/team",
        "stores/.hidden",
        "plugins/notes.secrets",
    ] {
        std::fs::create_dir_all(data.path().join(directory)).expect("mkdir");
    }
    std::fs::write(data.path().join("plugins/notes.kv"), b"").expect("write");
    let offers = store_offers(
        vec!["team".to_string(), "far".to_string()],
        Some(data.path()),
    );
    let names: Vec<&str> = offers.iter().map(|o| o.value.as_str()).collect();
    assert_eq!(names, ["user", "team", "far", "work", "plugin.notes"]);
    assert_eq!(offers[1].description.as_deref(), Some("in secrets.conf"));
    assert_eq!(offers[3].description.as_deref(), Some("on disk"));
}
