use super::*;

fn alias(name: &str, body: &str) -> Entry {
    Entry::new(Kind::Alias, name, body)
}

#[test]
fn an_alias_is_written_the_way_a_shell_reads_it_back() {
    let text = text(&[alias("gs", "git status --short")]);
    assert!(text.contains("alias gs='git status --short'\n"), "{text}");
}

/// **Single quotes, and the body is not expanded when the file is sourced.** `alias dot='git
/// --git-dir=$HOME/.dot'` has to reach bash with the `$HOME` intact — it is expanded when the alias
/// runs, not when it is defined, and double quotes would expand it at the wrong moment.
#[test]
fn a_dollar_survives_being_written_out() {
    let text = text(&[alias("dot", "/usr/bin/git --git-dir=$HOME/.dot")]);
    assert!(
        text.contains("alias dot='/usr/bin/git --git-dir=$HOME/.dot'"),
        "{text}"
    );
}

/// The one character single quotes cannot hold, spelled the way every shell spells it.
#[test]
fn a_single_quote_is_escaped_the_only_way_there_is() {
    let text = text(&[alias("say", "echo it's here")]);
    assert!(text.contains(r"alias say='echo it'\''s here'"), "{text}");
}

/// A function travels as a *definition*, which is the one kind a `$PATH` file cannot carry: it has
/// to run in the calling shell to be worth anything.
#[test]
fn a_shell_function_becomes_a_function() {
    let text = text(&[Entry::new(
        Kind::Func,
        "mkcd",
        "mkdir -p \"$1\" && cd \"$1\"",
    )]);
    assert!(
        text.contains("mkcd() {\nmkdir -p \"$1\" && cd \"$1\"\n}\n"),
        "{text}"
    );
}

/// **Not everything belongs in a file bash reads.** An abbreviation is expanded by the line editor
/// as you type, so there is nothing to tell bash; a script is a file in `macros::scripts` already; a
/// Lua function is not shell.
#[test]
fn what_bash_cannot_use_is_left_out() {
    let text = text(&[
        Entry::new(Kind::Abbrev, "gco", "git checkout"),
        Entry::new(Kind::Script, "deploy", "#!/bin/sh\necho hi\n"),
        Entry::new(Kind::Func, "lua_one", "-- a lua comment\nlocal x = 1\n"),
    ]);
    assert!(
        !text.contains("gco"),
        "an abbreviation is the editor's: {text}"
    );
    assert!(!text.contains("deploy"), "a script is a file: {text}");
    assert!(!text.contains("lua_one"), "bash has no Lua: {text}");
}

/// One turned off does not apply, here as everywhere else.
#[test]
fn one_turned_off_is_not_written() {
    let mut off = alias("gs", "git status");
    off.active = false;
    let text = text(&[off, alias("gl", "git log")]);
    assert!(!text.contains("alias gs="), "{text}");
    assert!(text.contains("alias gl="), "{text}");
}
