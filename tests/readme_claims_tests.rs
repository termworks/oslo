//! The README's countable claims, checked against the shell that has to honour them.
//!
//! Four of the third sweep's findings were prose that had quietly stopped being true: a tool that
//! never existed, a hook count off by ten, structured examples naming columns their producers do
//! not have. None of them could have survived a test, because none of them had one — nothing in CI
//! compared what the documentation says to what the binary does.
//!
//! This is that comparison, for the claims that are mechanical enough to make it. It is deliberately
//! narrow: a test that tried to parse English would rot faster than the prose it guards.

mod common;

use std::process::Command;

fn tool_output(args: &[&str]) -> String {
    let out = Command::new(common::oslo_bin())
        .args(args)
        .env("TERM", "dumb")
        .output()
        .expect("oslo runs");
    String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr)
}

/// Whether a tool the README names is compiled into *this* binary.
///
/// **`default = []`.** Four tools sit behind cargo features, so a plain `cargo test` builds a
/// shell that truthfully does not list them, while the README describes the binary `make build`
/// produces — which has them all. Asserting against the smaller build made this test fail on
/// `secret` for a README that was perfectly correct.
///
/// The README already says it: *"whichever of `direnv`, `plugin` and `scratch` this build has"*.
/// The prose knew these were conditional and the test did not, so the test was the half that was
/// wrong. It cannot read that sentence, so the same fact is written here.
///
/// Only these four are excused, and only when their own feature is off. Any other name still has
/// to be a real tool, which is the claim this test exists for: `oslo aliases` was advertised for
/// as long as the README had existed and was never a tool at all.
fn built_into_this_binary(name: &str) -> bool {
    match name {
        "direnv" => cfg!(feature = "direnv"),
        "plugin" => cfg!(feature = "plugin"),
        "scratch" => cfg!(feature = "scratch"),
        "secret" => cfg!(feature = "secrets"),
        // Added when `.make.lua` was, and missed here: a default build has no `make` tool, so
        // without this the README naming one failed the very check that is meant to keep the two
        // in step. The list is every gated tool, and a tool that grows a feature belongs in it.
        "make" => cfg!(feature = "make"),
        // And `watch`, missed the same way for the same reason — the list above is only as current
        // as the last tool that grew a feature, which is why this comment keeps being added to.
        "watch" => cfg!(feature = "watch"),
        _ => true,
    }
}

/// Every tool the README names is one `--help` lists, and the reverse.
///
/// `oslo aliases` was advertised for as long as the README has existed and was never a tool; the
/// name is `macros`. Two of the real ones, `userin` and `secret`, went unmentioned.
#[test]
fn the_readme_names_the_tools_that_exist() {
    let readme = std::fs::read_to_string("README.md").expect("README.md");
    let section = readme
        .split("## Tools")
        .nth(1)
        .expect("a Tools section")
        .split("\n## ")
        .next()
        .expect("its end");
    let sentence = section.split(':').next().expect("the opening sentence");

    let help = tool_output(&["--help"]);
    let listed: Vec<String> = help
        .split("TOOLS")
        .nth(1)
        .expect("a TOOLS block")
        .lines()
        .skip(1)
        .take_while(|line| !line.trim().is_empty())
        .filter_map(|line| line.split_whitespace().next().map(str::to_string))
        .collect();
    assert!(
        listed.len() >= 5,
        "could not read the tool list: {listed:?}"
    );

    for tool in &listed {
        assert!(
            sentence.contains(&format!("`{tool}`")),
            "`oslo --help` offers `{tool}` and the README's Tools sentence does not name it"
        );
    }
    // And nothing named there that the binary does not have — the `aliases` case.
    for named in sentence.split('`').skip(1).step_by(2) {
        let named = named.trim();
        if named.is_empty() || named.contains(' ') || named.starts_with("oslo") {
            continue;
        }
        if !built_into_this_binary(named) {
            continue;
        }
        assert!(
            listed.iter().any(|tool| tool == named),
            "the README names a tool `{named}` that `oslo --help` does not list"
        );
    }
}

/// The hook count in the README is the number of hooks there are.
#[test]
fn the_readme_counts_the_hooks_correctly() {
    let readme = std::fs::read_to_string("README.md").expect("README.md");
    let claim = readme
        .split("### The hooks")
        .nth(1)
        .expect("a hooks section")
        .split_whitespace()
        .next()
        .expect("a leading word")
        .to_lowercase();

    let listed = tool_output(&["hook", "list"])
        .lines()
        .filter(|line| {
            let text = line.trim_start_matches(['●', ' ']);
            text.starts_with("pre-") || text.starts_with("post-") || text.starts_with("on-")
        })
        .count();

    let expected = in_words(listed);
    assert_eq!(
        Some(claim.as_str()),
        expected.as_deref(),
        "the README opens the hooks section with {claim:?}, and `oslo hook list` has {listed} — \
         it should read {:?}",
        expected
            .as_deref()
            .unwrap_or("<a number this cannot spell>")
    );
}

/// A count as the word the README would write, for anything under a hundred.
///
/// **Spelled rather than looked up**, because the table this replaced held only the multiples of
/// ten: the moment a thirty-first hook was added the README could not be made to pass at all, and
/// the test said so by failing rather than by naming the word to use. A count is not going to stop
/// at a round number.
fn in_words(n: usize) -> Option<String> {
    const ONES: [&str; 20] = [
        "zero",
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
    ];
    const TENS: [&str; 10] = [
        "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
    ];
    match n {
        0..=19 => Some(ONES[n].to_string()),
        20..=99 => {
            let (tens, ones) = (n / 10, n % 10);
            Some(match ones {
                0 => TENS[tens].to_string(),
                _ => format!("{}-{}", TENS[tens], ONES[ones]),
            })
        }
        _ => None,
    }
}

/// Every structured-pipeline example in the README runs, and names columns that exist.
///
/// `ps | group-by user | count` printed `0` for as long as it was documented, because `ps` has no
/// `user` column — and `group-by` on a column that is not there answers rather than refusing, so
/// nothing ever said so.
#[test]
fn the_readme_structured_examples_produce_something() {
    let readme = std::fs::read_to_string("README.md").expect("README.md");
    let examples: Vec<&str> = readme
        .lines()
        .map(str::trim)
        .filter(|line| {
            // A pipeline built only from producers and verbs, so it is safe to run and its output
            // is this shell's own. Anything reaching for the network or a real `kubectl` is skipped.
            (line.starts_with("ps |") || line.starts_with("ls |") || line.starts_with("df |"))
                && !line.contains("jq")
        })
        .map(|line| line.split('#').next().unwrap_or(line).trim())
        .collect();
    assert!(
        examples.len() >= 3,
        "the README's structured examples moved: {examples:?}"
    );

    for example in examples {
        let run = common::run_in(std::path::Path::new("."), example);
        assert!(
            !run.out().trim().is_empty(),
            "`{example}` from the README produced nothing (stderr: {})",
            run.stderr
        );
        assert!(
            run.out().trim() != "0",
            "`{example}` from the README answered `0` — the column it names does not exist"
        );
    }
}
