//! A word that globs, answered by the shell's own engine.
//!
//! `ls src/**/*.rs<Tab>` found one file: completion walked `**` as a `*`, took closed braces
//! literally, and offered nothing for `rm *.lo`. Tab and the highlighter now ask
//! `oslo_base::glob::walk` — the walker the executor expands with — so the menu, the colour and the
//! command cannot disagree about what a pattern names.
//!
//! ```text
//!   rm *.log<Tab>        all 3 matches        ← puts every match on the line
//!                        build.log
//!                        run.log
//!                        old/x.log
//! ```

use super::CompletionCandidate;
use super::paths::{Wanted, executable};
use crate::OsloHelper;
use crate::settings::GlobTab;
use crate::words::{Quote, Word, quote_replacement};
use oslo_base::glob::walk::{self, Options};

/// Directory entries one Tab may read. Ten times the highlighter's budget, because a Tab is asked
/// for and a repaint is not.
const TAB_ENTRIES: usize = 20_000;

/// How many directory entries the highlighter reads for one word before giving up.
///
/// **A budget, in the spirit of `MAX_PATH_CHECKS`.** Without one, every keystroke walked every
/// entry: a pattern with a literal tail (`*.dat`) has no match for any of the prefixes on the way
/// to it, so `*`, `*.`, `*.d`, `*.da` each read the lot — measured at 14.9 ms per keystroke in a
/// 62,000-file tree. Giving up answers "no match", the colour an unresolved glob already takes.
pub(crate) const MAX_GLOB_ENTRIES: usize = 2_000;

/// A path a pattern named: as it is written back, and as it was read.
struct Found {
    typed: String,
    read: String,
}

/// What one brace branch names, spending `budget`.
///
/// A `~` is read from `$HOME` and written back as the `~` that was typed; a relative word under a
/// spec's `$chdir` is read under that root and written back relative.
fn expand_branch(branch: &str, root: &str, globstar: bool, budget: &mut usize) -> Vec<Found> {
    let (typed_base, read_base, rest) = bases(branch, root);
    let chars: Vec<(char, bool)> = read_base
        .chars()
        .map(|c| (c, false))
        .chain(rest.chars().map(|c| (c, true)))
        .collect();
    let options = Options {
        globstar,
        budget: Some(*budget),
        ..walk::shell_options()
    };
    let Some(expansion) = walk::expand_counted(&chars, &options) else {
        // Nothing in this branch globs: it names itself, if it exists.
        let whole = format!("{read_base}{rest}");
        let exists = std::fs::symlink_metadata(oslo_base::lossless::to_os(&whole)).is_ok();
        return match exists {
            true => vec![Found {
                typed: format!("{typed_base}{rest}"),
                read: whole,
            }],
            false => Vec::new(),
        };
    };
    *budget = budget.saturating_sub(expansion.read);
    expansion
        .paths
        .into_iter()
        .map(|read| {
            let tail = read.strip_prefix(read_base.as_str()).unwrap_or(&read);
            Found {
                typed: format!("{typed_base}{tail}"),
                read,
            }
        })
        .collect()
}

/// The text written back, the text read, and the rest of the pattern.
fn bases<'a>(stem: &'a str, root: &str) -> (String, String, &'a str) {
    if let Some(after) = stem.strip_prefix('~') {
        let cut = after.find('/').unwrap_or(after.len());
        let (user, tail) = after.split_at(cut);
        let home = oslo_base::tilde::expand(user, &oslo_base::tilde::from_process);
        return (format!("~{user}"), home, tail);
    }
    if stem.starts_with('/') || root.is_empty() {
        return (String::new(), String::new(), stem);
    }
    (
        String::new(),
        format!("{}/", root.trim_end_matches('/')),
        stem,
    )
}

/// Every path `stem` names, braces expanded first as the shell does, one budget for all of it.
fn expand_stem(stem: &str, root: &str, globstar: bool, budget: &mut usize) -> Vec<Found> {
    let mut out = Vec::new();
    for branch in oslo_base::brace::expand_braces_text(stem) {
        if *budget == 0 {
            break;
        }
        out.extend(expand_branch(&branch, root, globstar, budget));
    }
    out
}

/// Whether a path pattern matches anything on disk — what the highlighter needs.
///
/// A globbed word reaches the highlighter in pieces — `tw*` is `tw` then `*` — so it is resolved
/// once, whole. **One budget for the whole word**, brace branches included: `img{001..500}*.jpg`
/// is five hundred branches, and a budget each would read a million entries per keystroke.
pub(crate) fn glob_matches_anything(stem: &str) -> bool {
    let mut budget = MAX_GLOB_ENTRIES;
    matches_within(stem, &mut budget)
}

/// [`glob_matches_anything`], spending a budget the caller owns.
fn matches_within(stem: &str, budget: &mut usize) -> bool {
    let globstar = walk::shell_options().globstar;
    for branch in oslo_base::brace::expand_braces_text(stem) {
        if *budget == 0 {
            return false;
        }
        if !expand_branch(&branch, "", globstar, budget).is_empty() {
            return true;
        }
    }
    false
}

impl OsloHelper {
    /// The glob at `pos` expanded: the byte span it occupies, and every match quoted for the line
    /// — or `None` when there is no glob there or it names nothing. What `expand-glob` and
    /// `list-glob` answer with. A qualified glob, `*.log(older 7d)`, is taken whole.
    pub fn glob_words(&self, line: &str, pos: usize) -> Option<(usize, usize, Vec<String>)> {
        if let Some(q) = super::qualified::at(line, pos) {
            return super::qualified::expand(&q)
                .ok()
                .map(|words| (q.start, q.end, words));
        }
        let word = crate::words::current_word(line, pos);
        if !super::paths::globs_unquoted(word.text) {
            return None;
        }
        let mut budget = TAB_ENTRIES;
        let found = expand_stem(&word.stem, "", true, &mut budget);
        if found.is_empty() {
            return None;
        }
        let words = found
            .iter()
            .map(|found| quote_replacement(&found.typed, Quote::None))
            .collect();
        Some((word.start, pos, words))
    }

    /// Candidates for a word that globs, or `false` when it names nothing at all — so the caller
    /// can complete it as a plain name instead (`weird[1` is a file, not a pattern).
    pub(super) fn glob_candidates(
        &self,
        word: &Word<'_>,
        wanted: &Wanted,
        out: &mut Vec<CompletionCandidate>,
    ) -> bool {
        let stem = word.stem.as_str();
        let mut budget = TAB_ENTRIES;
        let mut found = expand_stem(stem, &wanted.root, true, &mut budget);
        // zsh's `GLOB_COMPLETE`: `rm *.lo` is somebody half way to the logs.
        if found.is_empty() && !stem.ends_with('*') {
            found = expand_stem(&format!("{stem}*"), &wanted.root, true, &mut budget);
        }
        let rows: Vec<(Found, bool)> = found
            .into_iter()
            .filter_map(|found| {
                let real = oslo_base::lossless::to_os(&found.read);
                let is_dir = std::fs::metadata(&real).is_ok_and(|m| m.is_dir());
                let name = found.typed.trim_end_matches('/');
                let name = name.rsplit('/').next().unwrap_or(name);
                let runs = !wanted.only_runnable || is_dir || executable(real.as_ref());
                (wanted.accepts(name, is_dir) && runs).then_some((found, is_dir))
            })
            .collect();
        if rows.is_empty() {
            return false;
        }
        // Minus whatever is already on the line — see `Word::carried`.
        let written = |typed: &str| typed.get(word.carried..).unwrap_or("").to_string();
        let mode = crate::settings::current().completion.glob;
        if rows.len() > 1 || mode == GlobTab::Expand {
            let all: Vec<String> = rows
                .iter()
                .map(|(found, _)| quote_replacement(&written(&found.typed), Quote::None))
                .collect();
            out.push(CompletionCandidate {
                display: format!("all {} matches", rows.len()),
                replacement: all.join(" "),
                description: None,
                kind: Some("glob".to_string()),
                path: None,
                detail: None,
            });
            if mode == GlobTab::Expand {
                return true;
            }
        }
        for (found, is_dir) in rows {
            let display = match is_dir && !found.typed.ends_with('/') {
                true => format!("{}/", found.typed),
                false => found.typed.clone(),
            };
            out.push(CompletionCandidate {
                replacement: quote_replacement(&written(&display), word.quote),
                display,
                description: None,
                kind: Some(if is_dir { "dir" } else { "file" }.to_string()),
                path: Some(found.read),
                detail: None,
            });
        }
        true
    }
}

/// **The entry budget covers the whole word, brace branches included.**
#[cfg(test)]
mod budget_tests {
    use super::{MAX_GLOB_ENTRIES, matches_within};

    /// More entries than one branch may read, so a branch allowed its own budget shows.
    fn crowded() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        for i in 0..(MAX_GLOB_ENTRIES + 50) {
            std::fs::write(dir.path().join(format!("f{i:05}.dat")), "").expect("write");
        }
        dir
    }

    #[test]
    fn every_branch_spends_the_same_budget() {
        let dir = crowded();
        let base = dir.path().display().to_string();
        let stem = format!("{base}/{{a,b,c,d,e}}*.nomatch");
        let mut budget = MAX_GLOB_ENTRIES;
        assert!(!matches_within(&stem, &mut budget), "nothing matches");
        assert_eq!(budget, 0, "the five branches shared one budget");

        let many: Vec<String> = (0..40).map(|i| format!("x{i}")).collect();
        let stem = format!("{base}/{{{}}}*.nomatch", many.join(","));
        let mut budget = MAX_GLOB_ENTRIES;
        assert!(!matches_within(&stem, &mut budget));
        assert_eq!(budget, 0, "still one budget, not forty");
    }

    #[test]
    fn a_branch_that_matches_still_says_so() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("found.dat"), "").expect("write");
        let base = dir.path().display().to_string();
        let mut budget = MAX_GLOB_ENTRIES;
        assert!(matches_within(
            &format!("{base}/{{nope,fou}}*.dat"),
            &mut budget
        ));
        assert!(budget > 0, "it returned on the hit rather than reading on");
    }

    /// A branch that exhausts the budget starves the ones after it: the price of the bound.
    #[test]
    fn a_starved_branch_answers_no() {
        let dir = crowded();
        let base = dir.path().display().to_string();
        let mut budget = MAX_GLOB_ENTRIES;
        assert!(!matches_within(
            &format!("{base}/{{nope,f00001}}*.dat"),
            &mut budget
        ));
    }
}
