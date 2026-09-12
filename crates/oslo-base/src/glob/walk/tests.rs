//! Every expectation here was read off GNU bash 5.3.9 over the same tree, in byte order.

use super::*;
use std::os::unix::fs::symlink;
use std::path::Path;

const GLOBSTAR: Options = Options {
    globstar: true,
    dotglob: false,
    collate: false,
};

/// The audit's tree: nested dirs, a hidden dir, an empty dir, and links to a dir, a file and nowhere.
fn tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    for d in ["dir1/sub1/deep1", "dir2", ".hdir/hsub", "emptydir"] {
        fs::create_dir_all(root.join(d)).expect("dir");
    }
    for f in [
        "f.md",
        "top.txt",
        "dir1/f1.txt",
        "dir1/sub1/f2.txt",
        "dir1/sub1/deep1/f3.txt",
        "dir2/g.md",
        ".hdir/h.txt",
        ".hdir/hsub/h2.txt",
        ".hidden",
    ] {
        fs::write(root.join(f), "").expect("file");
    }
    symlink("dir2", root.join("lnk")).expect("link");
    symlink("top.txt", root.join("flink")).expect("link");
    symlink("nowhere", root.join("dangle")).expect("link");
    dir
}

/// Expand `pattern` under `root`, with the root spelled as quoted text and stripped off again, so
/// the answers read as bash's do from inside the tree. The root itself, which a named base
/// contributes and bash's unnamed working directory does not, comes back as `""` and is dropped.
fn glob(root: &Path, pattern: &str, options: Options) -> Vec<String> {
    let prefix = format!("{}/", root.display());
    let chars: Vec<(char, bool)> = prefix
        .chars()
        .map(|c| (c, false))
        .chain(pattern.chars().map(|c| (c, true)))
        .collect();
    expand(&chars, &options)
        .unwrap_or_default()
        .into_iter()
        .map(|p| p.strip_prefix(&prefix).unwrap_or(&p).to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

#[test]
fn a_final_globstar_lists_every_entry_and_enters_no_link() {
    let dir = tree();
    assert_eq!(
        glob(dir.path(), "**", GLOBSTAR),
        [
            "dangle",
            "dir1",
            "dir1/f1.txt",
            "dir1/sub1",
            "dir1/sub1/deep1",
            "dir1/sub1/deep1/f3.txt",
            "dir1/sub1/f2.txt",
            "dir2",
            "dir2/g.md",
            "emptydir",
            "f.md",
            "flink",
            "lnk",
            "top.txt",
        ]
    );
}

#[test]
fn a_named_base_is_part_of_the_answer_with_its_slash() {
    let dir = tree();
    assert_eq!(
        glob(dir.path(), "dir1/**", GLOBSTAR),
        [
            "dir1/",
            "dir1/f1.txt",
            "dir1/sub1",
            "dir1/sub1/deep1",
            "dir1/sub1/deep1/f3.txt",
            "dir1/sub1/f2.txt",
        ]
    );
    assert_eq!(glob(dir.path(), "emptydir/**", GLOBSTAR), ["emptydir/"]);
}

#[test]
fn a_trailing_slash_lists_directories_linked_ones_included() {
    let dir = tree();
    assert_eq!(
        glob(dir.path(), "**/", GLOBSTAR),
        [
            "dir1/",
            "dir1/sub1/",
            "dir1/sub1/deep1/",
            "dir2/",
            "emptydir/",
            "lnk/"
        ]
    );
    assert_eq!(
        glob(dir.path(), "dir1/**/", GLOBSTAR),
        ["dir1/", "dir1/sub1/", "dir1/sub1/deep1/"]
    );
    assert_eq!(glob(dir.path(), "emptydir/**/", GLOBSTAR), ["emptydir/"]);
}

#[test]
fn a_middle_globstar_matches_at_every_depth_including_none() {
    let dir = tree();
    assert_eq!(glob(dir.path(), "**/*.md", GLOBSTAR), ["dir2/g.md", "f.md"]);
    assert_eq!(
        glob(dir.path(), "**/d*", GLOBSTAR),
        ["dangle", "dir1", "dir1/sub1/deep1", "dir2"]
    );
    assert_eq!(
        glob(dir.path(), "*/**/f*", GLOBSTAR),
        ["dir1/f1.txt", "dir1/sub1/deep1/f3.txt", "dir1/sub1/f2.txt"]
    );
}

/// A base a pattern *matched* is spelled without its slash, and a linked one is entered.
#[test]
fn a_matched_base_has_no_slash_and_is_entered() {
    let dir = tree();
    assert_eq!(
        glob(dir.path(), "*/**", GLOBSTAR),
        [
            "dir1",
            "dir1/f1.txt",
            "dir1/sub1",
            "dir1/sub1/deep1",
            "dir1/sub1/deep1/f3.txt",
            "dir1/sub1/f2.txt",
            "dir2",
            "dir2/g.md",
            "emptydir",
            "lnk",
            "lnk/g.md",
        ]
    );
}

/// A directory the pattern names is entered whatever it is — a link, or hidden.
#[test]
fn a_named_link_or_hidden_directory_is_entered() {
    let dir = tree();
    assert_eq!(glob(dir.path(), "lnk/**", GLOBSTAR), ["lnk/", "lnk/g.md"]);
    assert_eq!(
        glob(dir.path(), ".hdir/**", GLOBSTAR),
        [".hdir/", ".hdir/h.txt", ".hdir/hsub", ".hdir/hsub/h2.txt"]
    );
}

#[test]
fn a_missing_base_matches_nothing_rather_than_inventing_a_path() {
    let dir = tree();
    assert!(glob(dir.path(), "nosuch/**", GLOBSTAR).is_empty());
}

#[test]
fn two_globstars_do_not_duplicate_a_match() {
    let dir = tree();
    assert_eq!(
        glob(dir.path(), "**/**/*.md", GLOBSTAR),
        ["dir2/g.md", "f.md"]
    );
}

#[test]
fn dotglob_opens_hidden_entries_at_every_depth() {
    let dir = tree();
    let both = Options {
        globstar: true,
        dotglob: true,
        collate: false,
    };
    assert_eq!(
        glob(dir.path(), "**/*.txt", both),
        [
            ".hdir/h.txt",
            ".hdir/hsub/h2.txt",
            "dir1/f1.txt",
            "dir1/sub1/deep1/f3.txt",
            "dir1/sub1/f2.txt",
            "top.txt",
        ]
    );
    let star = glob(dir.path(), "*", both);
    assert!(star.contains(&".hidden".to_string()), "{star:?}");
    assert!(!star.iter().any(|p| p == "." || p == ".."), "{star:?}");
}

/// Off, `**` is an ordinary `*`: one level, and it goes through a linked directory like any `*`.
#[test]
fn without_globstar_a_double_star_is_one_star() {
    let dir = tree();
    assert_eq!(
        glob(dir.path(), "**/*.md", Options::default()),
        ["dir2/g.md", "lnk/g.md"]
    );
}

#[test]
fn a_pattern_with_nothing_that_globs_is_not_walked() {
    assert_eq!(expand(&[('a', true), ('b', true)], &GLOBSTAR), None);
    let quoted: Vec<(char, bool)> = "*.rs".chars().map(|c| (c, false)).collect();
    assert_eq!(expand(&quoted, &GLOBSTAR), None);
}

/// The leading dot is a pathname rule: spelled out, or `dotglob`.
#[test]
fn a_leading_dot_is_matched_only_when_written_or_with_dotglob() {
    let items = |p: &str| compile_items(&p.chars().map(|c| (c, true)).collect::<Vec<_>>()).0;
    let plain = Options::default();
    assert!(!name_matches(&items("*"), ".hidden", &plain));
    assert!(!name_matches(&items("?hidden"), ".hidden", &plain));
    assert!(!name_matches(&items("[.]hidden"), ".hidden", &plain));
    assert!(name_matches(&items(".*"), ".hidden", &plain));
    assert!(name_matches(&items("a*"), "a.b", &plain));
    let dot = Options {
        dotglob: true,
        ..plain
    };
    assert!(name_matches(&items("*"), ".hidden", &dot));
}

#[test]
fn only_a_lone_unquoted_double_star_is_globstar() {
    let on = GLOBSTAR;
    assert_eq!(
        compile(&[('*', true), ('*', true)], &on),
        Component::Globstar
    );
    assert_ne!(
        compile(&[('*', true), ('*', true)], &Options::default()),
        Component::Globstar
    );
    assert_ne!(
        compile(&[('*', false), ('*', false)], &on),
        Component::Globstar
    );
    assert_ne!(
        compile(&[('a', true), ('*', true), ('*', true)], &on),
        Component::Globstar
    );
}
