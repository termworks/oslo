use super::*;
use std::fs;
use std::path::Path;

/// `a.log` (100 B, 10 days old), `b.log` (5 KiB, new), `c.txt` (empty), `img7.png`, `img42.png`,
/// `sub/` (empty directory), `deep/x/y.log`, and `lnk -> a.log`.
fn tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    fs::write(root.join("a.log"), vec![b'x'; 100]).unwrap();
    fs::write(root.join("b.log"), vec![b'x'; 5 * 1024]).unwrap();
    fs::write(root.join("c.txt"), "").unwrap();
    fs::write(root.join("img7.png"), "x").unwrap();
    fs::write(root.join("img42.png"), "xx").unwrap();
    fs::create_dir(root.join("sub")).unwrap();
    fs::create_dir_all(root.join("deep/x")).unwrap();
    fs::write(root.join("deep/x/y.log"), "y").unwrap();
    std::os::unix::fs::symlink("a.log", root.join("lnk")).unwrap();
    // Ten days ago, so `older 7d` has something to find.
    let past = nix::sys::time::TimeSpec::new(
        (SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - 864_000) as i64,
        0,
    );
    nix::sys::stat::utimensat(
        None,
        &root.join("a.log"),
        &past,
        &past,
        nix::sys::stat::UtimensatFlags::NoFollowSymlink,
    )
    .unwrap();
    dir
}

/// Every name directly in `root` plus `deep/x/y.log`, run through `text`, answered relative.
fn keep(root: &Path, text: &str) -> Vec<String> {
    let base = format!("{}/", root.display());
    let names = [
        "a.log",
        "b.log",
        "c.txt",
        "img7.png",
        "img42.png",
        "sub",
        "lnk",
        "deep/x/y.log",
    ];
    let paths = names.iter().map(|n| format!("{base}{n}")).collect();
    let q = parse(text).expect("parses");
    apply(paths, &q, &base, false)
        .into_iter()
        .map(|p| p.strip_prefix(&base).unwrap_or(&p).to_string())
        .collect()
}

#[test]
fn kinds_of_entry() {
    let dir = tree();
    assert_eq!(keep(dir.path(), "dir"), ["sub"]);
    assert_eq!(keep(dir.path(), "link"), ["lnk"]);
    assert_eq!(keep(dir.path(), "empty"), ["c.txt", "sub"]);
}

#[test]
fn size_and_age() {
    let dir = tree();
    assert_eq!(keep(dir.path(), "larger 1k"), ["b.log"]);
    assert_eq!(keep(dir.path(), "older 7d, file"), ["a.log", "lnk"]);
    assert!(!keep(dir.path(), "newer 1h").contains(&"a.log".to_string()));
}

#[test]
fn regex_on_the_name_or_the_whole_path() {
    let dir = tree();
    assert_eq!(
        keep(dir.path(), r"re '^img\d+\.png$'"),
        ["img7.png", "img42.png"]
    );
    assert_eq!(keep(dir.path(), "path re 'x/y'"), ["deep/x/y.log"]);
    assert_eq!(keep(dir.path(), "re '^[ab]', not b*"), ["a.log"]);
}

#[test]
fn a_number_in_the_name() {
    let dir = tree();
    assert_eq!(keep(dir.path(), "num 10-100"), ["img42.png"]);
}

#[test]
fn order_and_slice() {
    let dir = tree();
    assert_eq!(keep(dir.path(), "file, by size, rev, first 1"), ["b.log"]);
    assert_eq!(keep(dir.path(), "largest 2, re log"), ["b.log", "a.log"]);
    assert_eq!(keep(dir.path(), "re png, last 1"), ["img42.png"]);
    assert_eq!(keep(dir.path(), "depth 2-9"), ["deep/x/y.log"]);
}

#[test]
fn a_mistake_is_named() {
    assert!(
        parse("oldr 7d")
            .unwrap_err()
            .contains("`oldr` is not a qualifier")
    );
    assert!(parse("older").unwrap_err().contains("needs a value"));
    assert!(parse("larger 3Q").unwrap_err().contains("units"));
    assert!(parse("re '('").is_err(), "a regex that does not compile");
    assert!(parse("file extra").unwrap_err().contains("unexpected"));
}

#[test]
fn the_base_is_everything_before_the_first_globbing_component() {
    assert_eq!(base_of("src/**/*.rs"), "src/");
    assert_eq!(base_of("/a/b/c*/d"), "/a/b/");
    assert_eq!(base_of("*.log"), "");
    assert_eq!(base_of("dir/sub/"), "dir/sub/");
}

#[test]
fn words_split_at_commas_and_keep_their_quotes() {
    let q = parse("re 'a, b', by name").expect("parses");
    assert!(matches!(&q.tests[..], [Test::Re { .. }]));
    assert_eq!(q.by, Some(By::Name));
}
