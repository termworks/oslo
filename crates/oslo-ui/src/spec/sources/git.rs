//! The refs of the repository being typed in: branches, tags, remotes.
//!
//! ```text
//!   git checkout ⇥      develop        current   branch
//!                       feat/after-fish          branch
//!                       origin/main              remote branch
//!                       v0.6.2                   tag
//! ```
//!
//! # Why this one matters most
//!
//! `git checkout <Tab>` is the most-typed completion in any shell, and without this it offers the
//! filenames in the current directory — the shipped `git` spec has three hundred kilobytes of flags
//! and not one branch, because a branch is not a fact about `git`.
//!
//! # Read from `.git`, not from `git`
//!
//! zsh and carapace both run `git for-each-ref` here: a fork, a process, and on a cold cache a
//! visible one. The refs are files. `refs/heads/` is a directory of them and `packed-refs` is a
//! text file of the rest, so the whole answer is two reads and a walk of a directory that has one
//! entry per branch.
//!
//! It also works where a fork would not: a repository whose `git` is not on `$PATH`, and the
//! keystroke path in raw mode where [`crate::spec::action::Runner`] has to impose a deadline
//! precisely because a child might never come back.
//!
//! # Never cached
//!
//! A branch you just created is the branch you are about to check out, and the directory being
//! completed in changes with every `cd`. Both make a cache wrong within one command — and the cost
//! is a directory walk of something with a hundred entries at the outside.

use super::Suggestion;
use std::path::{Path, PathBuf};

/// Local branches, the checked-out one marked.
pub fn branches(dir: &str) -> Vec<Suggestion> {
    let Some(git) = repository(dir) else {
        return Vec::new();
    };
    let here = checked_out(&git);
    refs(&git, "refs/heads/")
        .into_iter()
        .map(|name| {
            let note = match Some(&name) == here.as_ref() {
                true => "current",
                false => "",
            };
            Suggestion::new(name, note, "branch")
        })
        .collect()
}

/// Every tag in the repository.
pub fn tags(dir: &str) -> Vec<Suggestion> {
    let Some(git) = repository(dir) else {
        return Vec::new();
    };
    refs(&git, "refs/tags/")
        .into_iter()
        .map(|name| Suggestion::new(name, "", "tag"))
        .collect()
}

/// The remotes this repository has, from `.git/config`.
///
/// The config rather than `refs/remotes/`, because a remote that has never been fetched has a
/// stanza and no refs — and it is still a remote `git fetch` takes.
pub fn remotes(dir: &str) -> Vec<Suggestion> {
    let Some(git) = repository(dir) else {
        return Vec::new();
    };
    let text = super::read(&git.join("config").to_string_lossy());
    let mut found = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        // `[remote "origin"]`
        let Some(rest) = line.strip_prefix("[remote \"") else {
            continue;
        };
        if let Some(name) = rest.strip_suffix("\"]") {
            found.push(Suggestion::new(name, "", "remote"));
        }
    }
    found
}

/// Everything a revision argument accepts: branches, remote-tracking branches, and tags.
///
/// # Tags only once something has been typed
///
/// **A bare Tab offers branches; a prefix searches everything.** This repository has 68 tags and
/// three branches, and the menu ranks by what you have run and breaks ties alphabetically — so
/// including tags in the empty menu buries every branch under `0.1.1`. That is the wrong answer to
/// `git checkout <Tab>`, which is the most-typed completion there is.
///
/// Nothing is lost: `git checkout v0.6<Tab>` still finds the tag, because by then there is a prefix
/// to search with. An empty Tab is a menu of what you might want; a typed prefix is a search.
pub fn revisions(dir: &str, word: &str) -> Vec<Suggestion> {
    let Some(git) = repository(dir) else {
        return Vec::new();
    };
    let mut found = branches(dir);
    found.extend(
        refs(&git, "refs/remotes/")
            .into_iter()
            // `origin/HEAD` is a symbolic ref to the remote's default branch, not a branch of its
            // own, and checking it out detaches.
            .filter(|name| !name.ends_with("/HEAD"))
            .map(|name| Suggestion::new(name, "", "remote branch")),
    );
    if !word.is_empty() {
        found.extend(tags(dir));
    }
    found
}

/// The `.git` of whatever repository `dir` is inside, or `None` if it is inside none.
///
/// Walks up, as git does — a completion typed three directories down is still in the repository.
fn repository(dir: &str) -> Option<PathBuf> {
    let start = match dir.is_empty() {
        true => std::env::current_dir().ok()?,
        false => PathBuf::from(dir),
    };
    let mut at = start.as_path();
    loop {
        let candidate = at.join(".git");
        if candidate.is_dir() {
            return Some(common(candidate));
        }
        // A worktree and a submodule have a `.git` *file* holding `gitdir: <path>`.
        if candidate.is_file() {
            let text = super::read(&candidate.to_string_lossy());
            let pointed = text.trim().strip_prefix("gitdir:")?.trim();
            let resolved = match Path::new(pointed).is_absolute() {
                true => PathBuf::from(pointed),
                false => at.join(pointed),
            };
            return Some(common(resolved));
        }
        at = at.parent()?;
    }
}

/// The directory the refs actually live in.
///
/// A linked worktree's own `.git` holds its `HEAD` and `index` but no `refs/heads` — those belong
/// to the repository all the worktrees share, named by a `commondir` file beside them. Following it
/// is the difference between completing branches in a worktree and completing nothing.
fn common(git: PathBuf) -> PathBuf {
    let marker = git.join("commondir");
    if !marker.is_file() {
        return git;
    }
    let text = super::read(&marker.to_string_lossy());
    let pointed = text.trim();
    match pointed.is_empty() {
        true => git,
        false => match Path::new(pointed).is_absolute() {
            true => PathBuf::from(pointed),
            false => git.join(pointed),
        },
    }
}

/// Every ref under one prefix, loose and packed alike, named without it.
fn refs(git: &Path, prefix: &str) -> Vec<String> {
    let mut found = Vec::new();
    walk(
        &git.join(prefix.trim_end_matches('/')),
        &mut String::new(),
        &mut found,
    );
    // `packed-refs` holds the ones `git gc` has folded into a single file, which on a repository
    // with any history is most of the tags and often every branch.
    for line in super::read(&git.join("packed-refs").to_string_lossy()).lines() {
        if line.starts_with('#') || line.starts_with('^') {
            continue;
        }
        let Some((_sha, full)) = line.split_once(' ') else {
            continue;
        };
        if let Some(name) = full.trim().strip_prefix(prefix) {
            found.push(name.to_string());
        }
    }
    found.sort_unstable();
    found.dedup();
    found
}

/// Loose refs are files in a tree, and a branch named `feat/thing` is a directory and a file.
fn walk(dir: &Path, under: &mut String, found: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let was = under.len();
        under.push_str(&name);
        if kind.is_dir() {
            under.push('/');
            walk(&entry.path(), under, found);
        } else {
            found.push(under.clone());
        }
        under.truncate(was);
    }
}

/// The branch `HEAD` points at, or `None` when the head is detached.
fn checked_out(git: &Path) -> Option<String> {
    let text = super::read(&git.join("HEAD").to_string_lossy());
    let pointed = text.trim().strip_prefix("ref:")?.trim();
    Some(pointed.strip_prefix("refs/heads/")?.to_string())
}

#[cfg(test)]
#[path = "git/tests.rs"]
mod tests;
