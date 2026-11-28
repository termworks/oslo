//! Attribution and the secrets grant. Loading itself needs a live interpreter and a real home, and
//! is covered end to end by `tests/plugin_tests.rs`.

use super::*;

/// **A plugin is attributed by where it lives, not by a name it chose.** This decides which of the
/// user's secrets it may read, and a plugin naming itself would be a plugin naming somebody else.
#[test]
fn a_plugin_is_named_by_its_root_directory() {
    assert_eq!(
        plugin_name(Path::new(
            "/home/me/.local/share/oslo/site/pack/mine/start/notes"
        )),
        "notes"
    );
    assert_eq!(plugin_name(Path::new("/home/me/.config/oslo")), "oslo");
}

/// Keyed, so granting twice replaces rather than accumulates — and an ungranted plugin gets nothing
/// rather than everything. Denying by default is the only safe way round: `oslo.secret` reads an
/// empty list as "none of them", and no list at all as "the config or the prompt, so all of them".
#[test]
fn a_grant_is_keyed_and_absent_means_none() {
    assert!(granted("never-granted").is_empty());

    grant_secrets("notes", vec!["gh-token".to_string()]);
    assert_eq!(granted("notes"), ["gh-token"]);

    grant_secrets("notes", vec!["other".to_string()]);
    assert_eq!(
        granted("notes"),
        ["other"],
        "granting again replaces, so a config can be re-read without stacking"
    );
}

/// Only the config grants: a plugin's load is refused, and so is anything once loading began.
#[test]
fn a_grant_is_refused_while_a_plugin_loads_and_afterwards() {
    assert!(grant_refused().is_none(), "the config, before any plugin");
    loading::while_loading("sneaky", &[], || {
        assert!(grant_refused().is_some(), "a plugin granting itself");
    });
    assert!(grant_refused().is_none());
    GRANTS_CLOSED.with(|closed| closed.set(true));
    assert!(grant_refused().is_some(), "a hook, after plugins loaded");
}
