//! Completing a word that names a file or a directory.
//!
//! Split from `completion.rs` when it crossed the 600-line limit. It is a subject of its own: every
//! other builder answers from something the shell already knows — its builtins, its specs, its
//! history — and this one is the only one that goes to the filesystem.

use super::{CompletionCandidate, matches_prefix};
use crate::OsloHelper;
use crate::words::{Word, quote_replacement, unquote};
use oslo_base::glob::ShellPattern;
use std::fs;

/// Whether `text` holds a character that makes it a pattern rather than a name.
fn has_glob(text: &str) -> bool {
    text.contains(['*', '?', '['])
}

/// The same question asked of a word *as typed*, where a quote decides.
///
/// `rm "tw*"` is a file whose name ends in a star and the shell will not expand it, so completion
/// must not offer what it would have matched — the two would be describing different commands.
fn globs_unquoted(text: &str) -> bool {
    let mut quote = None;
    let mut escaped = false;
    for ch in text.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match (quote, ch) {
            (None, '\\') => escaped = true,
            (Some(q), _) if ch == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(ch),
            (None, '*' | '?' | '[') => return true,
            _ => {}
        }
    }
    false
}

/// The entries of a directory, read once so the offer loop can be handed a slice.
fn entries_of(entries: fs::ReadDir) -> Vec<fs::DirEntry> {
    entries.flatten().collect()
}

/// The one directory an unglobbed path names: as typed, and as read.
///
/// They differ for exactly one reason — a `~` reads from `$HOME` but has to be written back as the
/// `~` that was typed, or a completion would replace the tilde with the whole home path.
fn plain_directory(dir_part: &str) -> (String, String) {
    (
        dir_part.to_string(),
        oslo_base::tilde::expand_prefix(dir_part, &oslo_base::tilde::from_process),
    )
}

/// The directories a path's leading part names, as `(text as typed, directory to read)`.
///
/// Ordinarily one pair. Two things make it several or make the halves differ:
///
/// * a `~`, which reads from `$HOME` but must be *written back* as the `~` the user typed;
/// * a `*` in the directory part — `ls /x/*/fo` names every directory one level down, and each is
///   a different place to look with a different path to write back.
///
/// Without this, `read_dir("/x/*/")` simply failed and Tab offered nothing at all.
fn directories_named(dir_part: &str) -> Vec<(String, String)> {
    // The tilde is resolved whole — `~root`, `~+` and `~-` as well as `~/` — and kept as typed on
    // the writing-back side, so a completion never replaces the tilde with the path it stood for.
    let (typed_root, read_root, rest) = match dir_part.strip_prefix('~') {
        Some(after) => {
            let cut = after.find('/').unwrap_or(after.len());
            let (user, tail) = after.split_at(cut);
            let home = oslo_base::tilde::expand(user, &oslo_base::tilde::from_process);
            (format!("~{user}"), home, tail)
        }
        _ if dir_part.starts_with('/') => ("/".to_string(), "/".to_string(), &dir_part[1..]),
        _ => (String::new(), String::new(), dir_part),
    };
    let mut open = vec![(typed_root, read_root)];

    for part in rest.split('/') {
        if part.is_empty() {
            continue;
        }
        if !has_glob(part) {
            open = open
                .into_iter()
                .map(|(typed, read)| (join(&typed, part), join(&read, part)))
                .collect();
            continue;
        }
        let pattern = ShellPattern::from_unquoted(part);
        let mut next = Vec::new();
        for (typed, read) in &open {
            let Ok(entries) = fs::read_dir(if read.is_empty() { "." } else { read }) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                // A pattern never reaches a dotfile unless the dot was typed, as in bash.
                if name.starts_with('.') && !part.starts_with('.') {
                    continue;
                }
                if !pattern.matches(&name) || !entry.metadata().is_ok_and(|m| m.is_dir()) {
                    continue;
                }
                next.push((join(typed, &name), join(read, &name)));
            }
        }
        // Sorted, because `read_dir` has no order and two identical Tabs must not disagree.
        next.sort();
        open = next;
    }
    // Every one names a directory, so each ends in the separator the caller will write after it.
    open.into_iter()
        .map(|(typed, read)| (with_slash(typed), read))
        .collect()
}

fn join(base: &str, part: &str) -> String {
    match base.is_empty() || base.ends_with('/') {
        true => format!("{base}{part}"),
        false => format!("{base}/{part}"),
    }
}

fn with_slash(text: String) -> String {
    match text.is_empty() || text.ends_with('/') {
        true => text,
        false => format!("{text}/"),
    }
}

/// Whether `command` refuses anything that is not a directory.
///
/// **One list, because two layers ask.** Completion had it and the ghost did not, so `cd a` was
/// offered `azzz/` by Tab and suggested `aa` — a file — by the ghost, which `cd` then refused.
/// `nav` was in neither, though it answers `not a directory` for a file like the rest of them.
pub(crate) fn takes_only_directories(command: &str) -> bool {
    matches!(command, "cd" | "pushd" | "rmdir" | "nav")
}

/// Whether a directory entry has an execute bit anybody could use.
pub(crate) fn runnable(entry: &fs::DirEntry) -> bool {
    executable(&entry.path())
}

/// The same question of a path rather than an entry.
///
/// For a caller that has already decided which candidate it wants and does not want to have paid a
/// `statx` for the ones it discarded — see the ghost's `path_hint`, which was asking this of every
/// prefix match in the directory before it knew which one could win.
pub(crate) fn executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

/// What a path completion will accept, beyond what the word itself says.
///
/// Two of these come from the line — `cd` takes only directories, a command word must be runnable —
/// and the rest come from a spec: `$files([.go])`, `$directories`, `$chdir(/tmp)`. One struct so
/// that a position declaring a filter and a command implying one arrive by the same door.
#[derive(Debug, Clone, Default)]
pub(crate) struct Wanted {
    pub only_dirs: bool,
    pub only_runnable: bool,
    /// Names ending in any of these. Empty means every name.
    pub suffixes: Vec<String>,
    /// Read relative to this rather than to the working directory. `$chdir`.
    ///
    /// **The read side only.** A completion is written back as the text that was typed, so a word
    /// completed under a `$chdir` still inserts the relative path the command will resolve itself.
    pub root: String,
}

impl Wanted {
    pub(crate) fn accepts(&self, name: &str, is_dir: bool) -> bool {
        if self.only_dirs && !is_dir {
            return false;
        }
        // A suffix filter is about files. Directories stay, or there would be no way to reach the
        // ones further down.
        if is_dir || self.suffixes.is_empty() {
            return true;
        }
        self.suffixes.iter().any(|suffix| name.ends_with(suffix))
    }

    /// `dir` as it must be read, which is under `root` when there is one and `.` when there is not.
    fn read(&self, dir: &str) -> String {
        match (self.root.as_str(), dir) {
            ("", "") => ".".to_string(),
            ("", dir) => dir.to_string(),
            (root, "") => root.to_string(),
            (root, dir) if dir.starts_with('/') => {
                let _ = root;
                dir.to_string()
            }
            (root, dir) => format!("{}/{}", root.trim_end_matches('/'), dir),
        }
    }
}

impl OsloHelper {
    pub(super) fn path_candidates(&self, word: &Word<'_>, out: &mut Vec<CompletionCandidate>) {
        let wanted = Wanted {
            only_dirs: word
                .prior_words
                .first()
                .map(|w| unquote(w))
                .is_some_and(|c| takes_only_directories(&c)),
            // **A command named as a path can only be one that runs.** `./bui` reaches this builder
            // because no `$PATH` name can answer for it, and a plain data file is not an answer
            // either: bash offers directories and executables there, and nothing else.
            only_runnable: word.command_position,
            ..Wanted::default()
        };
        self.path_candidates_for(word, &wanted, out);
    }

    pub(crate) fn path_candidates_for(
        &self,
        word: &Word<'_>,
        wanted: &Wanted,
        out: &mut Vec<CompletionCandidate>,
    ) {
        let stem = word.stem.as_str();
        // **`~name` before the first slash is a user, not a filename.** With no `/` yet the word was
        // handed to the ordinary prefix walk and matched against the working directory, so `~ro`
        // offered nothing at all — while the highlighter coloured `~root` as a real path and the
        // expander resolved it. The three only had to agree.
        if let Some(typed) = stem.strip_prefix('~')
            && !stem.contains('/')
            && word.quote == crate::words::Quote::None
        {
            for name in oslo_base::tilde::user_names() {
                if !name.starts_with(typed) {
                    continue;
                }
                // Written back as `~name/`, not as the home directory it stands for — the same rule
                // `directories_named` follows, so accepting a completion never replaces the tilde
                // with the path behind it.
                out.push(CompletionCandidate::new(
                    format!("~{name}/"),
                    format!("~{name}/"),
                    Some("user".to_string()),
                ));
            }
            return;
        }
        // Split on the *unquoted* value: `"My Dir/fi` has to look inside `My Dir`.
        let (dir_part, prefix) = match stem.rfind('/') {
            Some(i) => (&stem[..=i], &stem[i + 1..]),
            None => ("", stem),
        };

        // Asked of the word *as typed*: `rm "tw*"` names one file and expands to nothing, so it
        // completes by prefix like any other name.
        if !globs_unquoted(word.text) {
            let (head, read_from) = plain_directory(dir_part);
            if let Ok(entries) = fs::read_dir(wanted.read(&read_from)) {
                let entries = entries_of(entries);
                self.offer_from(&entries, &head, prefix, None, word, wanted, out);
            }
            return;
        }

        // **A glob asks the shell's own engine**, so Tab and the command agree on what it names:
        // `**` recursive, closed braces expanded, `~` written back as typed. See `super::glob`.
        if crate::settings::current().completion.glob != crate::settings::GlobTab::Literal {
            if self.glob_candidates(word, wanted, out) {
                return;
            }
            // Nothing matched even with a `*` added, so the characters are part of a real name —
            // `weird[1` — and it completes by prefix, escaped on the way back.
            let (head, read_from) = plain_directory(dir_part);
            if let Ok(entries) = fs::read_dir(wanted.read(&read_from)) {
                let entries = entries_of(entries);
                self.offer_from(&entries, &head, prefix, None, word, wanted, out);
            }
            return;
        }

        // `oslo.completion.glob = "literal"`: the matches one level at a time.
        let pattern = has_glob(prefix).then(|| ShellPattern::from_unquoted(prefix));

        // One entry for a plain directory; several when the *directory* part globs too, as in
        // `ls /x/*/fo`. Each carries the text as typed and the directory it really reads, because
        // a `~` and a `*` both differ between the two.
        for (head, read_from) in directories_named(dir_part) {
            let Ok(entries) = fs::read_dir(wanted.read(&read_from)) else {
                continue;
            };
            self.offer_from(
                &entries_of(entries),
                &head,
                prefix,
                pattern.as_ref(),
                word,
                wanted,
                out,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn offer_from(
        &self,
        entries: &[fs::DirEntry],
        dir_part: &str,
        prefix: &str,
        pattern: Option<&ShellPattern>,
        word: &Word<'_>,
        wanted: &Wanted,
        out: &mut Vec<CompletionCandidate>,
    ) {
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            let matched = match pattern {
                Some(pattern) => pattern.matches(&name),
                None => matches_prefix(&name, prefix, self.case_sensitive()),
            };
            if !matched {
                continue;
            }
            // A dotfile only shows up when it was asked for, as in bash.
            if name.starts_with('.') && !prefix.starts_with('.') {
                continue;
            }
            // `metadata` follows symlinks: a link to a directory is a directory as far as `cd`
            // and the trailing slash are concerned.
            let is_dir = entry.metadata().map(|m| m.is_dir()).unwrap_or(false);
            if !wanted.accepts(&name, is_dir) {
                continue;
            }
            if wanted.only_runnable && !is_dir && !runnable(entry) {
                continue;
            }

            let display = if is_dir {
                format!("{}/", name)
            } else {
                name.clone()
            };
            // The replacement is the whole word, not just the tail: quoting a fragment would
            // leave the directory part unquoted and the two halves would not agree.
            //
            // **Minus whatever is already on the line.** A word retargeted at a piece of itself —
            // the item inside `{a,b}` — keeps the directory in its stem so the right directory is
            // read, while `start` points past it. Writing the whole path there gave
            // `rm /dir/{alpha,/dir/beta`: a different, longer path than the one typed, on the
            // command whose documented example is `rm`. See [`Word::carried`].
            let head = dir_part.get(word.carried..).unwrap_or("");
            let value = format!("{head}{display}");
            out.push(CompletionCandidate {
                display,
                replacement: quote_replacement(&value, word.quote),
                // No description. The badge already says `dir` or `file`, and "Directory"
                // beside a ` dir ` badge is the same fact written twice — it also forces the
                // description column to exist for a listing that has nothing to put in it,
                // taking width from the names. This is IRIS's rule: where the *kind* is the
                // whole story the tag carries it alone, and only a kind that leaves something
                // unsaid (an alias, and what it expands to) gets both.
                description: None,
                kind: Some(if is_dir { "dir" } else { "file" }.to_string()),
                // The path the entry was read from, not one rebuilt from the typed text: a
                // `~/` stem reads from `$HOME` and would `stat` a directory literally named
                // `~` if the display were re-joined instead.
                path: Some(entry.path().to_string_lossy().into_owned()),
                detail: None,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A word with no directory part names the working directory, and reading `""` is not reading
    /// `.` — a bare `rm tw*` offered nothing at all until the caller substituted one for the other.
    #[test]
    fn a_word_with_no_directory_part_names_here() {
        assert_eq!(
            directories_named(""),
            vec![(String::new(), String::new())],
            "one place to look, and the caller reads `.` for it"
        );
    }

    /// An ordinary directory is one pair, and the two halves are the same unless a `~` is involved.
    #[test]
    fn a_plain_directory_is_itself() {
        assert_eq!(
            directories_named("/etc/"),
            vec![("/etc/".to_string(), "/etc".to_string())]
        );
        let (typed, read) = plain_directory("/etc/");
        assert_eq!((typed.as_str(), read.as_str()), ("/etc/", "/etc/"));
    }

    /// **A `~` reads from `$HOME` and is written back as a `~`.** Writing the expansion back would
    /// replace the tilde the user typed with the whole home path.
    #[test]
    fn a_tilde_reads_home_and_writes_back_a_tilde() {
        let Ok(home) = std::env::var("HOME") else {
            return;
        };
        if home.is_empty() {
            return;
        }
        let (typed, read) = plain_directory("~/bin/");
        assert_eq!(typed, "~/bin/");
        assert_eq!(read, format!("{home}/bin/"));
    }

    /// Whether a word globs is asked of the text *as typed*, where a quote decides.
    #[test]
    fn a_quote_decides_whether_a_star_is_a_star() {
        assert!(globs_unquoted("tw*"));
        assert!(globs_unquoted("a?b"));
        assert!(globs_unquoted("[ab]c"));
        assert!(globs_unquoted("dir/*"));
        // Quoted, escaped, or both — the shell will not expand any of these.
        assert!(!globs_unquoted("\"tw*\""));
        assert!(!globs_unquoted("'tw*'"));
        assert!(!globs_unquoted("tw\\*"));
        assert!(!globs_unquoted("plain"));
        // A star outside the quotes still globs, which is why this is per character.
        assert!(globs_unquoted("\"a\"*"));
    }
}
