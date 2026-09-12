use super::*;

fn word<'a>(line: &'a str, stem: &str) -> Word<'a> {
    Word {
        start: 0,
        text: line,
        stem: stem.to_string(),
        quote: Quote::None,
        command_position: false,
        prior_words: Vec::new(),
        carried: 0,
        prefix: "",
    }
}

/// **The stem is cut, not merely offset.** `(,)` on `one,src/ma` completes `src/ma`, and a
/// stem left whole would send the path builder looking for a directory called `one,src`.
#[test]
fn retargeting_cuts_the_word_and_moves_where_it_is_written() {
    let whole = word("one,src/ma", "one,src/ma");
    let piece = retarget(&whole, 4).expect("a plain word can be cut");
    assert_eq!(piece.stem, "src/ma");
    assert_eq!(piece.text, "src/ma");
    assert_eq!(piece.start, 4);
    assert_eq!(piece.carried, 0);
}

#[test]
fn a_word_that_is_not_its_own_text_is_left_alone() {
    let escaped = Word {
        text: r"one,my\ file",
        ..word(r"one,my\ file", "one,my file")
    };
    assert!(retarget(&escaped, 4).is_none());
    let quoted = Word {
        quote: Quote::Double,
        ..word("one,x", "one,x")
    };
    assert!(retarget(&quoted, 4).is_none());
}

#[test]
fn nothing_to_cut_gives_the_word_back() {
    let whole = word("plain", "plain");
    assert_eq!(retarget(&whole, 0).map(|w| w.stem), Some("plain".into()));
}

/// A position past the end of what was declared falls to the one declared for every other.
#[test]
fn positions_past_the_declared_ones_use_the_catch_all() {
    let declared = vec![Action::list(["first"]), Action::list(["second"])];
    let any = Action::list(["rest"]);
    assert!(matches!(position(&declared, &any, 0), Action::List(l) if l == &["first"]));
    assert!(matches!(position(&declared, &any, 2), Action::List(l) if l == &["rest"]));
    // …and with nothing declared for every other, nothing at all.
    assert!(position(&declared, &Action::None, 9).is_none());
}

/// A word with `host:` already on it, as the `:` word break leaves it.
fn after_a_colon<'a>(prefix: &'a str, stem: &str) -> Word<'a> {
    Word {
        prefix,
        ..word("", stem)
    }
}

/// **`scp f host:/etc/` listed the local `/etc`** — 265 entries off this machine, offered as
/// though they were the other one's. Nothing in a menu row says which filesystem it came from, so
/// the name it inserts exists and the copy that uses it fails somewhere else entirely.
#[test]
fn a_path_on_another_machine_is_not_answered_from_this_one() {
    let remote = Action::list(["$hosts", "$files", "$directories"]);
    for (prefix, stem) in [
        ("host:", "/etc/"),
        ("tron.netbird:", ""),
        ("ci@tron:", "/var/"),
    ] {
        assert!(
            names_another_machine(&remote, &after_a_colon(prefix, stem)),
            "{prefix}{stem} names another machine"
        );
    }
}

/// The three shapes that look similar and are not a machine.
#[test]
fn a_colon_that_names_no_machine_still_completes_here() {
    let remote = Action::list(["$hosts", "$files"]);
    // Nothing before the colon.
    assert!(!names_another_machine(
        &remote,
        &after_a_colon(":", "/tmp/")
    ));
    // A local file whose name has a colon in it.
    assert!(!names_another_machine(&remote, &after_a_colon("./a:", "b")));
    assert!(!names_another_machine(
        &remote,
        &after_a_colon("/srv/a:", "b")
    ));
    // No colon at all — the ordinary case, where hosts and files both answer.
    assert!(!names_another_machine(&remote, &word("tro", "tro")));
}

/// **Only where the spec says a machine may go.** `$hosts` marks that position; everywhere else a
/// colon is an ordinary character and `make target:` still completes from disk.
#[test]
fn a_position_that_takes_no_host_is_unaffected() {
    let local = Action::list(["$files", "$directories"]);
    assert!(!names_another_machine(
        &local,
        &after_a_colon("host:", "/etc/")
    ));
    assert!(!names_another_machine(
        &Action::None,
        &after_a_colon("host:", "/etc/")
    ));
}
