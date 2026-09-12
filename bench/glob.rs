//! What pathname expansion costs on a large tree, measured rather than reasoned about.
//!
//! The walker reads each directory once per expansion (a cache shared by `**` and the component
//! after it), and bash reads it twice; this is where that shows. Run with
//! `cargo bench --bench glob` (release, so the numbers mean something).

use oslo_base::glob::qualify;
use oslo_base::glob::walk::{self, Options};
use std::path::Path;
use std::time::Instant;

/// `width` directories per level, `depth` levels, `files` files in each directory.
fn tree(root: &Path, width: usize, depth: usize, files: usize) -> usize {
    let mut made = 0;
    let mut level = vec![root.to_path_buf()];
    for _ in 0..depth {
        let mut next = Vec::new();
        for dir in &level {
            for f in 0..files {
                let ext = ["rs", "d", "log", "txt"][f % 4];
                std::fs::write(dir.join(format!("f{f}.{ext}")), "").expect("file");
                made += 1;
            }
            for d in 0..width {
                let sub = dir.join(format!("d{d}"));
                std::fs::create_dir(&sub).expect("dir");
                next.push(sub);
                made += 1;
            }
        }
        level = next;
    }
    made
}

fn time(label: &str, rounds: u32, mut run: impl FnMut() -> usize) {
    let mut found = 0;
    let started = Instant::now();
    for _ in 0..rounds {
        found = run();
    }
    let each = started.elapsed() / rounds;
    println!("{label:<34} {found:>7} matches  {each:>10.2?} per expansion");
}

fn main() {
    let dir = tempfile::tempdir().expect("tempdir");
    let entries = tree(dir.path(), 6, 4, 12);
    println!("tree: {entries} entries\n");

    let base = format!("{}/", dir.path().display());
    let options = Options {
        globstar: true,
        ..Options::default()
    };
    let expand = |pattern: &str| {
        let chars: Vec<(char, bool)> = base
            .chars()
            .map(|c| (c, false))
            .chain(pattern.chars().map(|c| (c, true)))
            .collect();
        walk::expand(&chars, &options).unwrap_or_default()
    };

    time("**/*", 5, || expand("**/*").len());
    time("**/*.d", 5, || expand("**/*.d").len());
    time("*/*/*", 5, || expand("*/*/*").len());
    time("**/", 5, || expand("**/").len());

    let q = qualify::parse("file, re '^f[0-9]+\\.rs$', by size, rev, first 100").expect("parses");
    time("**/*(file, re …, first 100)", 5, || {
        qualify::apply(expand("**/*"), &q, &base, false).len()
    });
}
