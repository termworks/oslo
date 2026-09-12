//! What the shell does with input it cannot run.
//!
//! oslo used to carry a second, hand-written parser and re-parse the whole program with it
//! whenever the parser or the lowering reported an error. That fallback had no here-document support,
//! so it parsed heredoc *bodies* as commands and executed them — a file that merely contained
//! `touch /tmp/pwned` created the file. It also stopped silently at a token it did not know, so
//! half a program could run and the shell would still exit 0.
//!
//! These tests pin the two properties that replaced it: data stays data, and unrunnable input
//! produces a diagnostic and a non-zero status instead of a guess.

mod common;

use common::{run, run_in};

/// The reproduction from PLAN R1.1, in its current form.
///
/// The original used `((x++))` as the unparseable line; R8.2 implemented it, so the trigger is
/// now a `coproc` — still a construct the adapter rejects, which is exactly what used
/// to reroute the whole script to the fallback parser and run the heredoc body as commands.
#[test]
fn a_heredoc_body_is_never_executed_when_the_script_fails_to_parse() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("heredoc_executed_marker");
    let script = format!(
        "coproc echo x\ncat <<EOF\ntouch {}\nEOF\n",
        marker.to_string_lossy()
    );

    let r = run_in(dir.path(), &script);

    assert!(
        !marker.exists(),
        "the heredoc body was executed as a command: {} exists",
        marker.display()
    );
    assert_ne!(r.status, 0, "an unparseable script must not exit 0");
    assert!(!r.stderr.is_empty(), "the failure must be reported");
}

/// The same shape, but with the unsupported construct *after* the heredoc, so the parse of the
/// heredoc itself is what carries the body.
#[test]
fn a_heredoc_body_is_not_executed_when_a_later_line_fails_to_parse() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("later_marker");
    let script = format!(
        "cat <<EOF\ntouch {}\nEOF\ncoproc foo {{ true; }}\n",
        marker.to_string_lossy()
    );

    let r = run_in(dir.path(), &script);

    assert!(!marker.exists(), "the heredoc body was executed");
    assert_ne!(r.status, 0);
}

#[test]
fn a_stray_closing_paren_is_a_syntax_error() {
    let r = run("echo a; )\necho NOT_REACHED");
    assert_eq!(r.status, 2, "a syntax error exits 2\nstderr: {}", r.stderr);
    assert!(
        !r.stdout.contains("NOT_REACHED"),
        "nothing in an unparseable script may run: {:?}",
        r.stdout
    );
    assert!(
        !r.stdout.contains('a'),
        "the program is rejected as a whole, not run up to the bad token: {:?}",
        r.stdout
    );
}

#[test]
fn an_unterminated_quote_is_a_syntax_error() {
    let r = run("echo 'unterminated");
    assert_eq!(r.status, 2, "stderr: {}", r.stderr);
    assert!(!r.stderr.is_empty());
}

#[test]
fn an_incomplete_compound_command_is_a_syntax_error() {
    let r = run("if true; then\necho NOT_REACHED");
    assert_eq!(r.status, 2, "stderr: {}", r.stderr);
    assert!(!r.stdout.contains("NOT_REACHED"));
}

#[test]
fn a_syntax_error_names_the_unclosed_construct() {
    let r = run("echo ok\nfor\n");
    assert_eq!(r.status, 2, "stderr: {}", r.stderr);
    assert!(
        r.stderr.contains("for") && r.stderr.contains("never closed"),
        "the diagnostic should name the unclosed construct: {:?}",
        r.stderr
    );
}

/// Constructs the adapter cannot represent must name themselves. Before the fallback was deleted
/// they silently rerouted the program; a bare "syntax error" would now be almost as unhelpful.
#[test]
fn unsupported_constructs_are_named_in_the_diagnostic() {
    for (script, needle) in [
        ("coproc foo { true; }", "coproc"),
        ("coproc echo hi", "coproc"),
        // R8.6: `select` is absent from brush's grammar, so without the source-text check this
        // would surface as "syntax error at line 1 col 8" — a diagnostic that reads like a typo.
        ("select x in a b; do echo $x; done", "select"),
    ] {
        let r = run(script);
        assert_ne!(r.status, 0, "{script:?} must not succeed");
        assert!(
            r.stderr.contains(needle) && r.stderr.contains("not supported"),
            "{script:?}: stderr should name the construct, got {:?}",
            r.stderr
        );
    }
}

/// A rejected construct rejects the whole program, so nothing after it runs either.
#[test]
fn a_rejected_construct_stops_the_whole_script() {
    for script in [
        "select x in a b; do echo $x; done\necho NOT_REACHED",
        "coproc foo { true; }\necho NOT_REACHED",
        "coproc echo hi\necho NOT_REACHED",
    ] {
        let r = run(script);
        assert_ne!(r.status, 0, "{script:?}");
        assert!(
            !r.stdout.contains("NOT_REACHED"),
            "{script:?}: {:?}",
            r.stdout
        );
    }
}

/// The `select` check reads the source text, so it must not hijack the diagnostic for a script
/// that merely *mentions* the word and fails for an unrelated reason.
#[test]
fn the_select_check_does_not_fire_on_ordinary_uses_of_the_word() {
    assert_eq!(run("echo select").out(), "select");
    assert_eq!(run("x=select; echo $x").out(), "select");
    assert_eq!(run("for w in select; do echo $w; done").out(), "select");
}

// --- R1.5: the substitution body is parsed before the fork ---

#[test]
fn an_unparseable_substitution_body_does_not_panic() {
    for script in ["echo $(if)", "echo $(;;)", "x=$(if); echo $x"] {
        let r = run(script);
        assert!(
            !r.stderr.contains("panicked"),
            "{script:?} panicked: {}",
            r.stderr
        );
        assert_ne!(r.status, 0, "{script:?} must report failure, got 0");
    }
}

#[test]
fn an_unparseable_substitution_body_stops_the_script() {
    let r = run("echo $(if)\necho NOT_REACHED");
    assert!(!r.stdout.contains("NOT_REACHED"), "stdout: {:?}", r.stdout);
}

// --- R1.6 (substitution half): NUL bytes never reach argv ---

#[test]
fn nul_bytes_are_stripped_from_substitution_output() {
    let r = run("x=$(printf 'a\\0b'); printf '[%s]\\n' \"$x\"; echo STILL_ALIVE");
    assert_eq!(r.out(), "[ab]\nSTILL_ALIVE", "stderr: {}", r.stderr);
    assert_eq!(r.status, 0);
}

#[test]
fn nul_bearing_substitution_output_can_be_passed_to_an_external_command() {
    let r = run("x=$(printf 'a\\0b'); /bin/echo \"[$x]\"");
    assert!(!r.stderr.contains("panicked"), "stderr: {}", r.stderr);
    assert_eq!(r.out(), "[ab]");
}

// --- eval and source report their own syntax errors ---

#[test]
fn eval_of_unparseable_text_returns_two_and_the_script_continues() {
    let r = run("eval 'if'; echo AFTER=$?");
    assert_eq!(r.out(), "AFTER=2", "stderr: {}", r.stderr);
    assert_eq!(r.status, 0);
}

#[test]
fn sourcing_an_unparseable_file_returns_two_and_the_script_continues() {
    let r = run("printf 'if\\n' > bad.sh; . ./bad.sh; echo AFTER=$?");
    assert_eq!(r.out(), "AFTER=2", "stderr: {}", r.stderr);
}

/// Round 11 A1: a Unicode blank pasted into a script used to kill the shell at *parse* time.
///
/// `Lexer::skip_whitespace` stepped over a set of characters `scan_word_parts` refused to consume,
/// so the lexer handed back empty words forever and the adapter grew a `Vec` until the allocator
/// aborted — status 134, a core dump, and no command run. A no-break space out of a web page or a
/// PDF was enough. bash treats every one of these as an ordinary word character, and so does oslo.
#[test]
fn a_unicode_blank_is_word_data_and_never_a_hang() {
    for blank in ['\u{0b}', '\u{0c}', '\u{a0}', '\u{85}', '\u{2028}'] {
        let r = run(&format!("echo a{blank}b"));
        assert_eq!(r.status, 0, "U+{:04X}: stderr {}", blank as u32, r.stderr);
        assert_eq!(r.out(), format!("a{blank}b"), "U+{:04X}", blank as u32);
    }

    // The same character everywhere else a word is lexed: an assignment value, an array literal,
    // a `for` list and a quoted run all go through the one scanner.
    let r = run("v=1\u{a0}2; echo \"[$v]\"");
    assert_eq!(r.out(), "[1\u{a0}2]", "stderr: {}", r.stderr);
    let r = run("a=(x\u{0b}y z); echo \"${a[0]}|${a[1]}\"");
    assert_eq!(r.out(), "x\u{0b}y|z", "stderr: {}", r.stderr);
}

/// Round 11 A2: openers that never close made brush backtrack exponentially.
///
/// `brush_parser` is a PEG, so an unmatched `(` doubles the alternatives it re-tries: 25 of them
/// held the shell at 100% CPU indefinitely, and the nesting guard never saw it because 25 is a
/// quarter of the *depth* it bounds. bash rejects all of these in under a millisecond with status
/// 2, which is now also what oslo does.
#[test]
fn unmatched_openers_are_refused_instead_of_backtracked() {
    for opener in [
        "(",
        "{ ",
        "if true; then ",
        "while true; do ",
        "case $x in ",
    ] {
        let script = format!("{}echo NOT_REACHED", opener.repeat(25));
        let r = run(&script);
        assert_eq!(r.status, 2, "{opener:?}: stderr {}", r.stderr);
        assert!(!r.stdout.contains("NOT_REACHED"), "{opener:?} ran its body");
        assert!(
            r.stderr.contains("unmatched openers"),
            "{opener:?}: {:?}",
            r.stderr
        );
    }
}

/// The bound must not touch input that closes what it opens, however wide.
#[test]
fn balanced_input_at_the_same_width_still_runs() {
    for (opener, closer) in [
        ("{ ", "; }"),
        ("if true; then ", "; fi"),
        ("while false; do :; done; ", ""),
    ] {
        let script = format!("{}echo ok{}", opener.repeat(40), closer.repeat(40));
        let r = run(&script);
        assert_eq!(r.out(), "ok", "{opener:?}: stderr {}", r.stderr);
        assert_eq!(r.status, 0, "{opener:?}");
    }
}

/// **A here-document with an empty tag, inside a command substitution, panicked the parser.**
///
/// `$(<< \n)` — found by the nightly fuzzer, which had reported it for six consecutive runs. The
/// tag is empty because of the space, so the here-document machinery reads past the `)` looking for
/// a terminator it will never find and buffers the tokens; the closing token is then replayed from
/// that buffer while the reader underneath is at end of input, and `consume_nested_construct`
/// unwrapped a character that was no longer there.
///
/// `brush_parser` is the shell's only parser, so this killed the shell with a panic rather than
/// reporting a syntax error. What it must do is what it does for every other unterminated
/// construct: say so, and exit non-zero.
#[test]
fn an_empty_here_doc_tag_in_a_substitution_is_an_error_not_a_panic() {
    let r = run("$(<< \n)");
    assert!(
        !r.err().contains("panicked"),
        "the parser panicked instead of reporting\n{}",
        r.err()
    );
    assert_ne!(r.status, 0, "unterminated input reported success");
}

/// The same shape at every nesting depth and in both spellings: one `unwrap` was fixed in three
/// places in that function and five more elsewhere in the tokenizer, reached by other constructs.
#[test]
fn no_unterminated_construct_panics_the_parser() {
    for script in [
        "$(<< \n)",
        "`<< `",
        "$( << \n )",
        "$(<< \n$(<< \n)\n)",
        "$(case a in a) << \n esac)",
        "${x:-$(<< \n)}",
        "$(<<",
        "$(<< ",
    ] {
        let r = run(script);
        assert!(
            !r.err().contains("panicked"),
            "{script:?} panicked:\n{}",
            r.err()
        );
    }
}

/// And the construct the vendored parser was patched *for* still parses, so the panic fix did not
/// take the `case`-pattern handling with it.
#[test]
fn a_case_inside_a_substitution_still_parses() {
    assert_eq!(run("echo $(case a in a) echo Y;; esac)").out(), "Y");
}
