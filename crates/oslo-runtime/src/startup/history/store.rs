//! Session history, and the `$HISTFILE` it is exported to.
//!
//! # The database is the history; the file is an export
//!
//! History is read from the profile store — see `History::seed` — and `$HISTFILE` is written so
//! that *other* programs can see what this shell ran, the way they read bash's or zsh's. There is no
//! default file: a shell nobody has configured keeps everything and exports nothing.
//!
//! The file is never read back. It used to be the source, which meant one fact had two records free
//! to disagree — and the poorer of the two was winning, since a plain list of lines knows nothing
//! about the directory, the language, the status or the duration that the store keeps.
//!
//! # Appended, never rewritten
//!
//! `save_history` writes the whole file, so two shells open at once each ended with only their own
//! commands (PLAN R9.11). Every line is appended as it is typed instead, which is also what makes a
//! command that hangs or kills the shell still turn up in the file afterwards.
//!
//! # The size limit is applied with slack
//!
//! Trimming on every append would mean rewriting the file per command, so it is allowed to grow past
//! the limit and rewritten once per `max / 4` commands beyond it.
//!
//! **The trim reads the file** — the one place that does, and the exception to the paragraph above.
//! It has to: the lines it must keep are the file's own, and the in-memory list is not them. Writing
//! that list over the file instead is what once replaced a 20,000-line history with a single line on
//! the first command of the first session. See [`History::trim_file`].

use std::io::Write;
use std::path::{Path, PathBuf};

/// The in-memory history, oldest first.
#[derive(Debug, Default)]
pub struct History {
    entries: Vec<String>,
    file: Option<PathBuf>,
    max: usize,
    /// How many lines the file holds, so it can be rewritten when it outgrows the limit without
    /// being read back to find out.
    file_lines: usize,
}

impl History {
    /// Open the history that will be written to `file`, keeping at most `max` entries.
    ///
    /// **The file is written, not read.** History comes from the profile database — see
    /// [`Self::seed`] — and `$HISTFILE` exists so that *other* programs can read what this shell
    /// ran, the way they read bash's or zsh's. Reading it back would give oslo two records of the
    /// same thing, free to disagree: the file is a plain list of lines, and the database knows the
    /// directory, the language, the status and the duration.
    ///
    /// A missing or unreadable file is not an error. Its existing lines are counted rather than
    /// parsed, which is all the trimming below needs to know.
    pub fn open(file: Option<PathBuf>, max: usize) -> History {
        let file_lines = file
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .map(|text| text.lines().filter(|line| !line.is_empty()).count())
            .unwrap_or(0);
        History {
            file_lines,
            entries: Vec::new(),
            file,
            max,
        }
    }

    /// Fill this history from the store, oldest first.
    ///
    /// **The database is where history comes from, whether or not there is a file.** `$HISTFILE` is
    /// an export for other programs — bash, zsh, anything that reads one — not oslo's own record,
    /// and a shell that read its history back out of it would have two sources that drift apart.
    /// The `history` builtin reads this list, and it was the last reader still looking at the file.
    pub fn seed(&mut self, lines: impl IntoIterator<Item = String>) {
        self.entries
            .extend(lines.into_iter().filter(|line| !line.is_empty()));
        if self.max > 0 && self.entries.len() > self.max {
            self.entries.drain(..self.entries.len() - self.max);
        }
    }

    /// Every entry, oldest first.
    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    /// Remember a line, and append it to the file.
    ///
    /// A line identical to the one before it is not stored twice — the editor's Up walk would
    /// otherwise show it twice in a row, which reads as the key having failed. The *file* still
    /// receives it, because `$HISTFILE` is a record of what ran.
    pub fn add(&mut self, line: &str) {
        if line.is_empty() {
            return;
        }
        if self.entries.last().map(String::as_str) != Some(line) {
            self.entries.push(line.to_string());
            if self.max > 0 && self.entries.len() > self.max {
                self.entries.remove(0);
            }
        }
        self.append_to_file(line);
    }

    fn append_to_file(&mut self, line: &str) {
        let Some(path) = self.file.clone() else {
            return;
        };
        if let Err(e) = append_line(&path, &escape(line)) {
            self.report(&path, e);
            return;
        }
        self.file_lines += 1;

        // The file is trimmed only when it has outgrown the limit by a margin, so the rewrite it
        // costs is paid once per `max / 4` commands rather than on every one past the cap. The
        // alternative — rewriting the whole file per command — is what makes an append-only
        // history worth having in the first place.
        let slack = self.max / 4;
        if self.max > 0 && self.file_lines > self.max + slack {
            self.trim_file(&path);
        }
    }

    /// Cut `$HISTFILE` back to its last `max` lines.
    ///
    /// **The file's own lines, which is the whole correction here.** This used to write
    /// `self.entries` over the file — oslo's in-memory list, seeded from the profile database and
    /// not from the file, because [`Self::open`] counts the file's lines without reading them. On a
    /// fresh profile that list holds only what this session has typed, so the first command of the
    /// first session replaced an existing history with one line:
    ///
    /// ```text
    ///   before: 20000 lines          after: 1 line
    /// ```
    ///
    /// Silent and irreversible, and pointed straight at years of accumulated history by the
    /// ordinary `HISTFILE=~/.bash_history` — which on a machine where oslo is `bash` is the default
    /// rather than an unusual setting. bash trims the same file to `HISTSIZE` lines and keeps their
    /// content; so does this now.
    ///
    /// Lines are carried across as they are: they were escaped on the way in, and escaping them a
    /// second time would turn every `\n` in a stored line into a literal backslash.
    fn trim_file(&mut self, path: &Path) {
        let Ok(text) = std::fs::read_to_string(path) else {
            // Unreadable, so there is nothing to preserve and nothing to say: the append above
            // already succeeded, and refusing to trim only means trying again next time.
            return;
        };
        let lines: Vec<&str> = text.lines().filter(|line| !line.is_empty()).collect();
        let kept = &lines[lines.len().saturating_sub(self.max)..];
        match std::fs::write(path, kept.join("\n") + "\n") {
            Ok(()) => self.file_lines = kept.len(),
            Err(e) => self.report(path, e),
        }
    }

    fn report(&self, path: &Path, e: std::io::Error) {
        eprintln!(
            "oslo: {}: {}",
            oslo_ui::marks::path(&path.display().to_string()),
            e
        );
    }

    /// Forget everything in memory. The file is left alone, which is what `history -c` means here:
    /// see `recall`'s note on why the directories are kept too.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

/// One entry, on one line.
///
/// A newline inside a command would be an entry boundary to whatever reads the file, splitting one
/// command into several — so it is written as `\n`. The backslash has to be escaped too, or a
/// command ending in one would swallow the next line's boundary.
///
/// There is no `unescape` any more. It existed because the file was read back at startup; the
/// database is the record now, and this direction is all a file that only other programs read needs.
fn escape(line: &str) -> String {
    line.replace('\\', "\\\\").replace('\n', "\\n")
}

fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{line}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_remembered_and_appended() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("hist");
        let mut history = History::open(Some(path.clone()), 100);
        history.add("echo one");
        history.add("echo two");
        assert_eq!(history.entries(), ["echo one", "echo two"]);

        // The file has both, for whatever else reads it.
        let text = std::fs::read_to_string(&path).expect("file");
        assert_eq!(text.lines().collect::<Vec<_>>(), ["echo one", "echo two"]);
    }

    /// **The file is written and never read.** It exists so that other programs can see what this
    /// shell ran; oslo's own history comes from the database, and reading the file back would give
    /// one fact two records free to disagree.
    #[test]
    fn opening_does_not_read_the_file_back() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("hist");
        std::fs::write(&path, "echo from an earlier shell\n").expect("write");

        let history = History::open(Some(path.clone()), 100);
        assert!(
            history.entries().is_empty(),
            "the file is not where history comes from"
        );

        // And what it holds is still counted, so the trim below knows how long it is.
        let mut history = History::open(Some(path.clone()), 2);
        history.add(": b");
        history.add(": c");
        let text = std::fs::read_to_string(&path).expect("file");
        assert!(
            text.lines().count() <= 3,
            "the existing line counted towards the cap: {text:?}"
        );
    }

    /// Where history actually comes from: the database, whether or not a file was asked for.
    #[test]
    fn seeding_fills_the_history_from_the_store() {
        let mut history = History::open(None, 100);
        history.seed(["one".to_string(), String::new(), "two".to_string()]);
        assert_eq!(
            history.entries(),
            ["one", "two"],
            "empty lines are not entries"
        );
        history.add("three");
        assert_eq!(history.entries(), ["one", "two", "three"]);
    }

    /// A file being present changes nothing about where history comes from.
    #[test]
    fn seeding_happens_with_a_file_too() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("hist");
        std::fs::write(&path, "ignored\n").expect("write");
        let mut history = History::open(Some(path), 100);
        history.seed(["from the database".to_string()]);
        assert_eq!(history.entries(), ["from the database"]);
    }

    #[test]
    fn seeding_keeps_the_newest_when_there_are_more_than_the_limit() {
        let mut history = History::open(None, 3);
        history.seed((0..10).map(|i| format!("command {i}")));
        assert_eq!(history.entries(), ["command 7", "command 8", "command 9"]);
    }

    /// The same line twice running is one entry, or the Up walk shows it twice and looks broken.
    #[test]
    fn an_immediate_repeat_is_not_stored_twice() {
        let mut history = History::open(None, 100);
        history.add("ls");
        history.add("ls");
        history.add("pwd");
        history.add("ls");
        assert_eq!(
            history.entries(),
            ["ls", "pwd", "ls"],
            "but a later repeat is"
        );
    }

    /// A multi-line command is one line in the file.
    ///
    /// A raw newline would be an entry boundary to whatever reads it, splitting one command into
    /// three — and the pieces would not parse.
    #[test]
    fn a_multi_line_command_stays_one_entry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("hist");
        let command = "for i in a b\ndo echo $i\ndone";
        let mut history = History::open(Some(path.clone()), 100);
        history.add(command);

        let text = std::fs::read_to_string(&path).expect("file");
        assert_eq!(text.lines().count(), 1, "one entry is one line: {text:?}");
        assert_eq!(text.trim_end(), "for i in a b\\ndo echo $i\\ndone");
        assert_eq!(history.entries(), [command], "and in memory it is whole");
    }

    /// A backslash at the end of a command must not swallow the entry boundary.
    #[test]
    fn a_trailing_backslash_is_escaped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("hist");
        let mut history = History::open(Some(path.clone()), 100);
        history.add("echo ending in a backslash \\");
        history.add("echo after");

        let text = std::fs::read_to_string(&path).expect("file");
        assert_eq!(
            text.lines().collect::<Vec<_>>(),
            ["echo ending in a backslash \\\\", "echo after"],
            "two lines, the first ending in an escaped backslash: {text:?}"
        );
    }

    /// The **file** is capped too, not only the memory — trimmed with slack so the rewrite it
    /// costs is not paid on every command past the limit.
    #[test]
    fn the_file_is_trimmed_when_it_outgrows_the_limit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("hist");
        let mut history = History::open(Some(path.clone()), 2);
        for line in [": a", ": b", ": c", ": d"] {
            history.add(line);
        }
        let text = std::fs::read_to_string(&path).expect("file");
        assert_eq!(
            text.lines().collect::<Vec<_>>(),
            [": c", ": d"],
            "the file kept the newest: {text:?}"
        );
    }

    /// No file at all is a working history that simply is not exported — the default.
    #[test]
    fn a_session_without_a_file_still_remembers() {
        let mut history = History::open(None, 10);
        history.add("secret work");
        assert_eq!(history.entries(), ["secret work"]);
    }

    #[test]
    fn clearing_forgets_the_session_and_keeps_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("hist");
        let mut history = History::open(Some(path.clone()), 100);
        history.add("kept in the file");
        history.clear();
        assert!(history.entries().is_empty());
        assert!(
            std::fs::read_to_string(&path)
                .expect("file")
                .contains("kept"),
            "the file is the record of what ran"
        );
    }

    /// **Trimming keeps the file's own lines.** It used to write oslo's in-memory list over the
    /// file — a list seeded from the profile database, not from the file — so on a fresh profile
    /// the first command of the first session replaced an existing history with one line. Silent,
    /// irreversible, and aimed straight at `HISTFILE=~/.bash_history`.
    #[test]
    fn trimming_keeps_what_the_file_already_held() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("hist");
        let old: Vec<String> = (1..=200).map(|n| format!("old_command_{n}")).collect();
        std::fs::write(&path, old.join("\n") + "\n").expect("write");

        // A fresh profile: the database is empty, so nothing seeds `entries`.
        let mut history = History::open(Some(path.clone()), 100);
        assert!(history.entries().is_empty(), "nothing is read back in");
        history.add("echo NEWCMD");

        let text = std::fs::read_to_string(&path).expect("file");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines.len(),
            100,
            "trimmed to the limit, not to this session"
        );
        assert_eq!(
            lines.last(),
            Some(&"echo NEWCMD"),
            "the new line is the newest"
        );
        assert!(
            lines.iter().any(|line| line.starts_with("old_command_")),
            "the earlier history survived: {lines:?}"
        );
        assert_eq!(
            lines[0], "old_command_102",
            "the oldest lines are the ones cut"
        );
    }

    /// A file smaller than the limit is left alone entirely.
    #[test]
    fn a_short_file_is_not_rewritten() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("hist");
        std::fs::write(&path, "one\ntwo\n").expect("write");
        let mut history = History::open(Some(path.clone()), 100);
        history.add("three");
        let text = std::fs::read_to_string(&path).expect("file");
        assert_eq!(text.lines().collect::<Vec<_>>(), ["one", "two", "three"]);
    }
}
