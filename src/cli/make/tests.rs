//! The tool's own surface. The runner is exercised end to end in `tests/make_tests.rs`, which can
//! afford a real process; these are the answers that do not need one.

use super::*;

#[test]
fn the_help_names_the_file_it_reads() {
    let text = help(Paint::plain());
    assert!(text.contains(".make.lua"), "{text}");
    assert!(text.contains("oslo make [OPTIONS]"), "{text}");
}

/// The tool is reachable by the name the menu advertises, and only when the feature is on.
#[test]
fn the_tool_is_registered() {
    let tool = crate::cli::tools::from_name("make").expect("registered");
    assert_eq!(tool.name, "make");
    assert!(tool.about.contains(".make.lua"), "{}", tool.about);
}

/// A file of that name still wins the operand slot — the rule every tool obeys.
#[test]
fn a_script_called_make_is_still_a_script() {
    assert!(crate::cli::tools::as_operand("./make").is_none());
}

/// **The two option lists have to agree.** The page asks the runner for its flags, and falls back
/// to [`OPTIONS`] on a build with no working interpreter. Nothing keeps the fallback in step except
/// this: a flag added to `make.lua` and not to the constant is one broken build away from being
/// undocumented.
#[test]
fn the_help_lists_every_flag_the_runner_parses() {
    let live = super::options();
    for flag in [
        "--list",
        "--dry-run",
        "--force",
        "--keep-going",
        "--quiet",
        "--watch",
        "--postpone",
        "--restart",
        "--help",
    ] {
        assert!(
            live.contains(flag),
            "the runner's list is missing {flag}: {live}"
        );
        assert!(
            super::OPTIONS.contains(flag),
            "the fallback list is missing {flag}"
        );
    }
    assert_eq!(
        live.trim(),
        super::OPTIONS.trim(),
        "the runner's option list and the fallback have drifted apart"
    );
}
