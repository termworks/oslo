//! Which stream a coordinate reaches, and what a word becomes.

use super::*;

const HOSTS: &str = "web-01  10.0.0.1\nweb-02  10.0.0.2\ndb-01   10.0.0.9\n";

fn words(text: &str, streams: &Streams) -> Vec<String> {
    substitute(text, streams)
        .expect("a coordinate")
        .iter()
        .map(shown)
        .collect()
}

/// A word as its characters, ignoring which parts were quoted.
fn shown(word: &Word) -> String {
    word.parts
        .iter()
        .map(|part| match part {
            WordPart::Literal(text) | WordPart::SingleQuoted(text) => text.as_str(),
            _ => "?",
        })
        .collect()
}

fn literal(text: &str) -> Word {
    Word {
        parts: vec![WordPart::Literal(text.to_string())],
    }
}

/// **Zero and up walk back through this pipeline.** `{0:…}` is the stage that just finished, which
/// is the one feeding the command being built.
#[test]
fn a_positive_stream_walks_back_through_the_pipeline() {
    let mut streams = Streams::default();
    streams.push_stage(HOSTS);
    streams.push_stage("web-01  10.0.0.1\n");

    // 0 is the newest stage.
    assert_eq!(words("{0:0}", &streams), vec!["web-01"]);
    // 1 steps past it to the one before.
    assert_eq!(
        words("{1:*:0}", &streams),
        vec!["web-01", "web-02", "db-01"]
    );
    // Past the end of the pipeline is empty, not an error.
    assert_eq!(words("{9:0:0}", &streams), Vec::<String>::new());
}

/// The prompt ring is one per process — see [`oslo_base::prompts`] — so the tests that seed it
/// take turns.
fn serialised() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// **Below zero walks back through the session**, which is a different collection on purpose: one
/// axis would mean `{3:…}` silently crossing out of a short pipeline into the prompts behind it.
#[test]
fn a_negative_stream_walks_back_through_the_prompts() {
    let _order = serialised();
    super::forget_prompts();
    super::remember_prompt(HOSTS);
    super::remember_prompt("only-this\n");
    let streams = Streams::for_this_pipeline();

    assert_eq!(words("{-1:0:0}", &streams), vec!["only-this"]);
    assert_eq!(words("{-2:0:0}", &streams), vec!["web-01"]);
    assert_eq!(words("{-9:0:0}", &streams), Vec::<String>::new());
    super::forget_prompts();
}

/// **A new pipeline starts with no stages of its own**, because a coordinate counting forward in
/// this command must not reach into the pipeline that is over. Nothing clears them: a `Streams` is
/// built per pipeline and takes only the prompt ring with it.
#[test]
fn a_new_pipeline_sees_the_prompts_and_none_of_the_last_stages() {
    let _order = serialised();
    super::forget_prompts();
    let mut streams = Streams::for_this_pipeline();
    streams.push_stage(HOSTS);
    assert_eq!(words("{0:0}", &streams), vec!["web-01"]);

    super::remember_prompt("after\n");
    let next = Streams::for_this_pipeline();
    assert_eq!(words("{0:0}", &next), Vec::<String>::new());
    assert_eq!(words("{-1:0:0}", &next), vec!["after"]);
    super::forget_prompts();
}

/// **A bare coordinate is one argument per value**, the way `"$@"` is — which is what makes
/// `ping {*:1}` one process with three arguments.
#[test]
fn a_bare_coordinate_becomes_one_word_per_value() {
    let mut streams = Streams::default();
    streams.push_stage(HOSTS);
    assert_eq!(
        words("{*:1}", &streams),
        vec!["10.0.0.1", "10.0.0.2", "10.0.0.9"]
    );
}

/// With text around it, the values join — `pre{*:0}post` has to stay one word to mean anything.
#[test]
fn a_coordinate_with_text_around_it_stays_one_word() {
    let mut streams = Streams::default();
    streams.push_stage(HOSTS);
    assert_eq!(words("host-{0:0}.lan", &streams), vec!["host-web-01.lan"]);
    assert_eq!(words("[{*:0}]", &streams), vec!["[web-01 web-02 db-01]"]);
}

/// **Every value is `Quoted`**, so a line with a space or a `*` in it arrives as one argument and
/// is never re-split or re-globbed. A shell that field-splits its own substitutions executes
/// filenames.
#[test]
fn a_value_is_one_argument_and_never_globs() {
    let mut streams = Streams::default();
    streams.push_stage("my file.txt  100\n*.rs  200\n");

    let out = substitute("{0}", &streams).expect("a coordinate");
    assert_eq!(out.len(), 1, "one line is one argument: {out:?}");
    assert_eq!(
        out[0].parts,
        vec![WordPart::SingleQuoted("my file.txt  100".to_string())],
        "single-quoted, so it is never split or globbed"
    );

    // And the glob character survives as text rather than matching anything.
    let out = substitute("{1:0}", &streams).expect("a coordinate");
    assert_eq!(
        out[0].parts,
        vec![WordPart::SingleQuoted("*.rs".to_string())]
    );
}

/// The cheap pre-scan must not miss a coordinate, and must not claim an ordinary brace group.
#[test]
fn the_scan_claims_coordinates_and_leaves_brace_groups() {
    for yes in [
        "{0}", "{0:1}", "{-1}", "{*:0}", "x{0:1}y", "{..2:1}", "{:1}",
    ] {
        assert!(looks_like_a_coordinate(yes), "{yes:?} should be scanned");
    }
    for no in ["{a,b}", "{a..e}", "plain", "{}", "no braces here"] {
        assert!(!looks_like_a_coordinate(no), "{no:?} should be left alone");
    }
}

/// A brace group that is not a coordinate is refused outright, so brace expansion still gets it.
#[test]
fn a_brace_group_is_not_substituted() {
    let streams = Streams::default();
    assert!(substitute("{a,b}", &streams).is_none());
    assert!(substitute("{a..e}", &streams).is_none());
    assert!(substitute("plain", &streams).is_none());
}

/// Nothing captured reads empty rather than refusing to run.
#[test]
fn an_empty_stack_reads_empty() {
    let streams = Streams::default();
    assert_eq!(words("{0:0}", &streams), Vec::<String>::new());
    assert_eq!(words("{-1:0:0}", &streams), Vec::<String>::new());
}

/// A stream longer than the cap keeps its head, so `{0}` still answers and `{-1}` is honestly the
/// last line of what was kept.
#[test]
fn a_huge_stream_is_capped_at_its_head() {
    let mut streams = Streams::default();
    let huge = format!("first\n{}\n", "x".repeat(STREAM_MAX * 2));
    streams.push_stage(huge);
    assert_eq!(words("{0}", &streams), vec!["first"]);
}

/// Only ten prompts are kept; the eleventh pushes the oldest out.
///
/// Note the three dimensions throughout: `{-1:0:}` is *stream* −1, line 0, whole line. Two
/// dimensions would be line −1 and word 0 of this command's own input, which is a different
/// question and answers nothing here — the grammar is unambiguous but it does have to be counted.
#[test]
fn the_prompt_ring_is_bounded() {
    let _order = serialised();
    super::forget_prompts();
    for n in 0..PROMPTS_KEPT + 5 {
        super::remember_prompt(&format!("line-{n}\n"));
    }
    let streams = Streams::for_this_pipeline();
    let last = PROMPTS_KEPT + 4;
    assert_eq!(words("{-1:0:}", &streams), vec![format!("line-{last}")]);
    let oldest_kept = last - (PROMPTS_KEPT - 1);
    assert_eq!(
        words(&format!("{{-{PROMPTS_KEPT}:0:}}"), &streams),
        vec![format!("line-{oldest_kept}")]
    );
    // One past the ring is gone.
    assert_eq!(
        words(&format!("{{-{}:0:}}", PROMPTS_KEPT + 1), &streams),
        Vec::<String>::new()
    );
    super::forget_prompts();
}

/// **Two dimensions never reach a stream**, which is the counting rule stated as a test rather
/// than a comment: `{-1:0}` is line −1 word 0 of *this* input, not "the previous prompt".
#[test]
fn two_dimensions_stay_in_this_stream() {
    let _order = serialised();
    super::forget_prompts();
    super::remember_prompt("from-the-prompt\n");
    let mut streams = Streams::for_this_pipeline();
    streams.push_stage("a b\nc d\n");

    assert_eq!(words("{-1:0}", &streams), vec!["c"], "line -1, word 0");
    assert_eq!(
        words("{-1:0:}", &streams),
        vec!["from-the-prompt"],
        "stream -1, line 0"
    );
}

/// **Rewriting walks a command's words**, replacing the coordinates and leaving everything else
/// exactly as it was.
#[test]
fn rewriting_replaces_only_the_coordinates() {
    let mut streams = Streams::default();
    streams.push_stage(HOSTS);

    let mut words = vec![literal("ssh"), literal("{0:0}"), literal("uptime")];
    assert!(rewrite(&mut words, &streams));
    assert_eq!(
        words.iter().map(shown).collect::<Vec<_>>(),
        vec!["ssh", "web-01", "uptime"]
    );

    // One word becoming three arguments, in the middle of a command.
    let mut words = vec![literal("ping"), literal("{*:1}"), literal("-c1")];
    assert!(rewrite(&mut words, &streams));
    assert_eq!(
        words.iter().map(shown).collect::<Vec<_>>(),
        vec!["ping", "10.0.0.1", "10.0.0.2", "10.0.0.9", "-c1"]
    );
}

/// **A quoted coordinate is text.** Every other expansion offers a way to write the characters
/// themselves, and this one does too.
#[test]
fn a_quoted_coordinate_is_left_alone() {
    let mut streams = Streams::default();
    streams.push_stage(HOSTS);

    let mut words = vec![
        literal("echo"),
        Word {
            parts: vec![WordPart::SingleQuoted("{0:0}".to_string())],
        },
    ];
    assert!(!rewrite(&mut words, &streams), "nothing to rewrite");
    assert_eq!(
        words.iter().map(shown).collect::<Vec<_>>(),
        vec!["echo", "{0:0}"]
    );
}

/// **The gate.** A command with no coordinate must answer `false`, because that is what keeps every
/// ordinary pipeline on the path it always took.
#[test]
fn the_gate_is_closed_for_ordinary_commands() {
    assert!(!command_uses_coordinates(&simple(&["ls", "-la"])));
    assert!(!command_uses_coordinates(&simple(&["mkdir", "{a,b}"])));
    assert!(command_uses_coordinates(&simple(&["ssh", "{0:0}"])));
    assert!(command_uses_coordinates(&simple(&["cat", "x{-1:0:}y"])));
}

/// A simple command from its words, for the gate tests.
fn simple(words: &[&str]) -> Command {
    Command::Simple(oslo_base::ast::SimpleCommand {
        assignments: Vec::new(),
        words: words.iter().map(|w| literal(w)).collect(),
        redirections: Vec::new(),
        unbraced: None,
    })
}

/// **Every coordinate in a word is replaced, not just the first**, and a brace group in front of
/// one does not stop the scan.
#[test]
fn a_word_may_hold_several_coordinates() {
    let mut streams = Streams::default();
    streams.push_stage(HOSTS);

    assert_eq!(words("{0:0}-{1:0}", &streams), vec!["web-01-web-02"]);
    assert_eq!(words("a{b}c{0:0}", &streams), vec!["a{b}cweb-01"]);
    assert_eq!(
        words("{0:0}/{1:0}/{-1:0}", &streams),
        vec!["web-01/web-02/db-01"]
    );
    // Several coordinates make one word, so a `*` among them joins rather than multiplying.
    assert_eq!(
        words("[{*:0}]-[{0:0}]", &streams),
        vec!["[web-01 web-02 db-01]-[web-01]"]
    );
}

/// A word that is a coordinate and nothing else still becomes one argument per value — the rule
/// above must not have taken that away.
#[test]
fn a_lone_coordinate_still_multiplies() {
    let mut streams = Streams::default();
    streams.push_stage(HOSTS);
    assert_eq!(words("{*:0}", &streams), vec!["web-01", "web-02", "db-01"]);
}

/// **A coordinate goes where a brace expands**, and the gate must agree with the rewriter about it.
///
/// A scalar right-hand side is text in bash — `w=x{1..3}` stays `x{1..3}` — and reading a
/// coordinate there emptied it. An array literal is the other half: bash *does* expand
/// `a=(x{1,2})`, so a coordinate belongs there. If these two answers ever drift apart, a pipeline
/// leaves the concurrent path to be rewritten in a way that changes nothing.
#[test]
fn an_assignment_is_claimed_only_where_a_brace_expands() {
    let scalar = assigned(AssignmentValue::Scalar(literal("x{0:0}")));
    assert!(
        !command_uses_coordinates(&scalar),
        "a scalar right-hand side is not a word list"
    );

    let array = assigned(AssignmentValue::Array(vec![oslo_base::ast::ArrayElement {
        index: None,
        value: literal("{0:0}"),
    }]));
    assert!(
        command_uses_coordinates(&array),
        "an array literal is a word list"
    );

    // And the rewriter makes the same two calls.
    let mut streams = Streams::default();
    streams.push_stage(HOSTS);
    for (mut command, expected) in [(scalar, "x{0:0}"), (array, "web-01")] {
        rewrite_command(&mut command, &streams);
        let Command::Simple(simple) = &command else {
            unreachable!("built as simple")
        };
        let word = match &simple.assignments[0].value {
            AssignmentValue::Scalar(word) => word,
            AssignmentValue::Array(elements) => &elements[0].value,
        };
        assert_eq!(shown(word), expected);
    }
}

/// A simple command that is one assignment, for the test above.
fn assigned(value: AssignmentValue) -> Command {
    Command::Simple(oslo_base::ast::SimpleCommand {
        assignments: vec![oslo_base::ast::Assignment {
            target: oslo_base::ast::AssignmentTarget::Name("w".to_string()),
            value,
            append: false,
        }],
        words: Vec::new(),
        redirections: Vec::new(),
        unbraced: None,
    })
}
