//! Tab on a word that globs, answered by the shell's own engine.
//!
//! Measured before this against the same tree: `ls src/**/*.rs<Tab>` found one file, `rm *.lo`
//! offered nothing, `ls {a,b}*` offered nothing, and `weird[1` — a real file's name — could not be
//! completed at all.

use super::helper;
use crate::env::Environment;
use crate::ui::OsloHelper;
use std::fs;
use std::path::Path;

/// `main.rs lib.rs build.log run.log alpha.txt beta.txt src/main.rs src/sub/deep.rs weird[1].txt`.
fn tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    fs::create_dir_all(root.join("src/sub")).expect("dirs");
    for name in [
        "main.rs",
        "lib.rs",
        "build.log",
        "run.log",
        "alpha.txt",
        "beta.txt",
        "src/main.rs",
        "src/sub/deep.rs",
        "weird[1].txt",
    ] {
        fs::write(root.join(name), "").expect("file");
    }
    dir
}

/// Tab at the end of `cmd <root>/<pattern>`: every candidate as `(display, replacement)`, with the
/// root taken back off so the answers read as they would from inside the tree.
fn tab(h: &OsloHelper, root: &Path, pattern: &str) -> Vec<(String, String)> {
    let base = format!("{}/", root.display());
    let line = format!("ls {base}{pattern}");
    let (_, candidates) = h.candidates(&line, line.len());
    candidates
        .into_iter()
        .map(|c| {
            (
                c.display.replace(&base, ""),
                c.replacement.replace(&base, ""),
            )
        })
        .collect()
}

fn displays(rows: &[(String, String)]) -> Vec<&str> {
    rows.iter().map(|(display, _)| display.as_str()).collect()
}

#[test]
fn a_globstar_reaches_every_depth_and_offers_them_all_first() {
    let dir = tree();
    let h = helper(Environment::new());
    let rows = tab(&h, dir.path(), "src/**/*.rs");
    // Shown from where the pattern starts globbing; written as the whole path.
    assert_eq!(displays(&rows), ["all 2 matches", "main.rs", "sub/deep.rs"]);
    assert_eq!(
        rows[0].1, "src/main.rs src/sub/deep.rs",
        "the first row is every match"
    );
}

/// zsh's `GLOB_COMPLETE`: nothing ends in `.lo`, so the pattern is taken as a prefix of one.
#[test]
fn a_pattern_that_matches_nothing_is_tried_as_the_start_of_one() {
    let dir = tree();
    let h = helper(Environment::new());
    assert_eq!(
        displays(&tab(&h, dir.path(), "*.lo")),
        ["all 2 matches", "build.log", "run.log"]
    );
}

#[test]
fn closed_braces_expand_before_the_glob() {
    let dir = tree();
    let h = helper(Environment::new());
    assert_eq!(
        displays(&tab(&h, dir.path(), "{a,b}*")),
        ["all 3 matches", "alpha.txt", "beta.txt", "build.log"]
    );
}

#[test]
fn one_match_is_offered_alone() {
    let dir = tree();
    let h = helper(Environment::new());
    assert_eq!(displays(&tab(&h, dir.path(), "l*.rs")), ["lib.rs"]);
}

/// `expand-glob` and `list-glob` answer with where the word starts and every match, quoted.
#[test]
fn the_word_under_the_cursor_expands_to_every_match() {
    let dir = tree();
    let h = helper(Environment::new());
    let base = format!("{}/", dir.path().display());
    let line = format!("ls {base}*.rs");
    let (start, end, words) = h.glob_words(&line, line.len()).expect("it globs");
    assert_eq!((start, end), (3, line.len()));
    let words: Vec<String> = words.iter().map(|w| w.replace(&base, "")).collect();
    assert_eq!(words, ["lib.rs", "main.rs"]);
    assert!(h.glob_words("ls plain", 8).is_none(), "nothing to expand");
}

/// `pattern(qualifiers)` completes whole, right after its `)`, and filters before it offers.
#[test]
fn a_qualified_glob_completes_whole_and_filtered() {
    let dir = tree();
    let h = helper(Environment::new());
    let base = format!("{}/", dir.path().display());

    let line = format!("ls {base}*.log(re '^b')");
    let (start, candidates) = h.candidates(&line, line.len());
    assert_eq!(start, 3, "the whole `pattern(…)` is replaced");
    let shown: Vec<String> = candidates
        .iter()
        .map(|c| c.replacement.replace(&base, ""))
        .collect();
    assert_eq!(shown, ["build.log"]);

    let line = format!("ls {base}**/*.rs(depth 1-9) -l");
    let at = line.find(" -l").expect("flag");
    let (start, end, words) = h.glob_words(&line, at).expect("it expands");
    assert_eq!((start, end), (3, at));
    let words: Vec<String> = words.iter().map(|w| w.replace(&base, "")).collect();
    assert_eq!(words, ["src/main.rs", "src/sub/deep.rs"]);
}

/// A name that holds a glob character is still a name: nothing matches it as a pattern, so it
/// completes by prefix, and comes back escaped so the shell will not glob it either.
#[test]
fn a_real_name_with_a_bracket_completes_literally() {
    let dir = tree();
    let h = helper(Environment::new());
    let rows = tab(&h, dir.path(), "weird[1");
    assert_eq!(displays(&rows), ["weird[1].txt"]);
    assert_eq!(rows[0].1, "weird\\[1\\].txt");
}
