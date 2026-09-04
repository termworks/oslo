use super::*;
use crate::spec::action::Query;

fn nothing() -> Query {
    Query::default()
}

/// **Every source answers, and nothing else does.** The dispatch is the contract between a spec
/// naming `$pids` and the code that knows what a pid is; a name missing from it silently falls
/// through to the shell, where it becomes an attempt to run a command called `pids`.
#[test]
fn every_source_is_reachable_by_name() {
    for name in [
        "hosts",
        "pids",
        "signals",
        "users",
        "groups",
        "variables",
        "interfaces",
        "mounts",
        "services",
        "shells",
        "timezones",
        "terminals",
        "ports",
        "fstab",
        "devices",
        "filesystems",
        "swaps",
        "modules",
        "sysctls",
        "branches",
        "tags",
        "remotes",
        "revisions",
    ] {
        assert!(
            offers(name, &nothing()).is_some(),
            "${name} is not dispatched"
        );
    }
}

/// A name that is not a source has to answer `None` and not an empty list: the caller tells them
/// apart, and `$(git branch)` still has to reach the shell.
#[test]
fn something_that_is_not_a_source_falls_through() {
    assert!(offers("files", &nothing()).is_none());
    assert!(offers("git branch", &nothing()).is_none());
    assert!(offers("", &nothing()).is_none());
    assert!(offers("nonsense", &nothing()).is_none());
}

/// Every row the menu draws needs a kind for its column, whatever the source.
#[test]
fn every_offer_names_what_it_is() {
    for name in [
        "pids",
        "signals",
        "users",
        "variables",
        "mounts",
        "filesystems",
        "shells",
    ] {
        for one in offers(name, &nothing()).unwrap_or_default() {
            assert!(!one.kind.is_empty(), "${name} offered a row with no kind");
            assert!(!one.value.is_empty(), "${name} offered an empty value");
        }
    }
}
