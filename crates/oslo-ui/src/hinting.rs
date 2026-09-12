//! The ghost suggestion shown past the cursor.
//!
//! Three sources, in fish's order: **history, then completions, then file paths.** History is a
//! line the user has actually run, which is a better guess than anything that can be ranked;
//! completions answer for a command name being typed; and paths answer for the argument after it,
//! which is the case the other two cannot see at all.
//!
//! Two things were wrong with it before. It only fired at column zero, so `true && ec` suggested
//! nothing; and it sorted alphabetically, so typing `exit` suggested `exitsnoop-bpfcc` — a
//! command the user has never run, ahead of the one they were plainly typing.

use super::OsloHelper;
use super::command_index::CommandIndex;
use super::words::{Quote, current_word};
use std::cell::RefCell;
use std::rc::Rc;

/// How a hint candidate was found, best first. Ties on frecency are broken by this.
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Debug)]
enum Origin {
    /// An external program on `$PATH`.
    External,
    /// A builtin, alias or function: part of this shell, so nearer to hand.
    Shell,
}

/// A directory's names, and whether each is itself a directory.
type Listing = Rc<Vec<(String, bool)>>;

/// That listing, with the directory it came from and the mtime it was read at.
type Remembered = (String, std::time::SystemTime, Listing);

/// One directory's entries, remembered between keystrokes.
///
/// The ghost re-listed the whole directory on **every** keystroke and threw the result away: in a
/// directory of twenty thousand names that is a `getdents64` sweep per character, about 11 ms, and
/// a sixteen-character paste paid it sixteen times. Nothing about the listing changes between two
/// keystrokes of the same word, so it is read once and kept.
///
/// **Keyed on the directory's own mtime**, which is what changes when an entry is created or
/// removed — so a file appearing while you type is picked up on the next keystroke, at the cost of
/// one `stat` instead of a full walk. One entry, because a word is being typed in one directory;
/// `Tracker::worktree` caches for the same reason and to the same depth.
fn entries_of(base: &str) -> Option<Listing> {
    thread_local! {
        static CACHED: RefCell<Option<Remembered>> = const { RefCell::new(None) };
    }
    let stamp = std::fs::metadata(base).ok()?.modified().ok()?;
    if let Some(hit) = CACHED.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|(path, at, _)| path == base && *at == stamp)
            .map(|(_, _, entries)| Rc::clone(entries))
    }) {
        return Some(hit);
    }
    let entries: Vec<(String, bool)> = std::fs::read_dir(base)
        .ok()?
        .flatten()
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            // `file_type` comes back with the directory entry on Linux, so this is not a syscall.
            (name, entry.file_type().is_ok_and(|t| t.is_dir()))
        })
        .collect();
    let entries = Rc::new(entries);
    CACHED.with(|slot| {
        *slot.borrow_mut() = Some((base.to_string(), stamp, Rc::clone(&entries)));
    });
    Some(entries)
}

/// The grey suffix to draw after a half-typed **Lua** name.
///
/// # Why this is not "the one candidate"
///
/// It was, and the effect was a ghost that almost never appeared. A shell prompt has history behind
/// it, so a few characters usually pin a whole line; a Lua prompt has a namespace, where several
/// names share a prefix nearly all the time — `os.t` is `time` and `tmpname`, `str` is a dozen
/// things. Requiring a unique match meant the feature was silent exactly when a namespace is
/// hardest to remember.
///
/// So what is offered is the **longest prefix every candidate agrees on**. That can never be wrong:
/// accepting it types characters you would have had to type anyway, whichever name you meant. With
/// one candidate it is the whole name, which is the old behaviour unchanged.
///
/// Only after two characters. One letter agrees with most of a namespace, so hinting from it means
/// grey text that changes on every keystroke and is right by luck.
pub(crate) fn lua_hint(line: &str, pos: usize) -> Option<String> {
    let typed = super::completion::lua::typed_at(line, pos)?;
    if typed.stem.chars().count() < 2 {
        return None;
    }
    let (_, candidates) = super::completion::lua::candidates(line, pos)?;
    let mut names = candidates.iter().map(|c| c.display.as_str());
    let first = names.next()?;
    let agreed = names.fold(first.to_string(), |shared, name| {
        common_prefix(&shared, name)
    });
    // A candidate that does not extend what was typed has nothing to offer — and a *fuzzy* match
    // may not even start with it, in which case there is no suffix to draw at all.
    agreed
        .strip_prefix(&typed.stem)
        .filter(|rest| !rest.is_empty())
        .map(str::to_string)
}

/// The longest prefix `a` and `b` share, cut on a character boundary.
fn common_prefix(a: &str, b: &str) -> String {
    a.chars()
        .zip(b.chars())
        .take_while(|(x, y)| x == y)
        .map(|(x, _)| x)
        .collect()
}

impl OsloHelper {
    /// Return the command-name suffix to draw after the cursor.
    pub fn command_hint(&self, line: &str, pos: usize) -> Option<String> {
        // **A Lua prompt suggests Lua names**, and it asks before anything below reads `word`:
        // `current_word` is a *shell* reading of the line, and `command_position` is a shell idea
        // that a Lua expression has no version of.
        //
        // Everything below offers a *command* — a builtin, an alias, a shell function, something
        // on `$PATH`. None of those are Lua, so at a Lua prompt `l` was answered with `ls`: a
        // suggestion that cannot run in the language being typed, which is worse than none. That
        // left history as the only Lua suggestion; now the names that exist suggest too.
        // **Nothing here at a Lua prompt.** Everything below offers a *command* — a builtin, an
        // alias, a shell function, something on `$PATH` — and none of those can be written in Lua.
        // The Lua answer is `Source::Names`, which the suggestion list reaches directly; this
        // source is the shell one and says so by declining.
        if super::prompt::language().is_some_and(|language| language == "lua") {
            return None;
        }

        let word = current_word(line, pos);
        // A half-typed quoted argument is a filename, not a command, and guessing at it in grey
        // text is worse than saying nothing.
        if !word.command_position || word.quote != Quote::None || word.stem.is_empty() {
            return None;
        }
        let stem = word.stem.as_str();

        let env = self.env.lock().unwrap_or_else(|held| held.into_inner());
        let path = env.var("PATH").unwrap_or_default().to_string();
        let is_shell_name =
            |n: &str| env.is_builtin(n) || env.alias(n).is_some() || env.is_function(n);

        // If what has been typed already names a command, it is not a prefix of the answer — it
        // *is* the answer. This is the `exit` case: bash shows nothing, and so should we.
        //
        // **A reserved word is finished for the same reason, and more urgently.** `current_word`
        // starts a fresh command after `;`, so the *closing* keyword of a compound lands in command
        // position: `if true; then echo a; fi` was being extended into `final`, and accepting it
        // left an unterminated `if` and a continuation prompt. Nothing can complete `fi` — the
        // shell's grammar is not a namespace to draw candidates from.
        if is_shell_name(stem)
            || CommandIndex::contains(&path, stem)
            || oslo_base::vocab::contains(stem)
            || crate::highlight::lex::is_keyword(stem)
        {
            return None;
        }

        let mut best: Option<Ranked> = None;
        // The score is looked up *inside*, after the prefix test, because it takes a lock. Passed
        // in as an argument it was evaluated for every name the caller enumerated — a lock per
        // builtin, alias and function on every keystroke — whatever the prefix said.
        let frecency = &self.frecency;
        let mut consider = |name: &str, origin: Origin| {
            if !name.starts_with(stem) || name.len() == stem.len() {
                return;
            }
            let candidate = Ranked {
                score: frecency.score(name),
                origin,
                name: name.to_string(),
            };
            if best.as_ref().is_none_or(|current| candidate.beats(current)) {
                best = Some(candidate);
            }
        };

        for name in env.builtin_names() {
            consider(&name, Origin::Shell);
        }
        for name in env.aliases().keys() {
            consider(name, Origin::Shell);
        }
        for name in env.functions().keys() {
            consider(name, Origin::Shell);
        }
        drop(env);

        // The structured verbs and registered tools. They run, so the ghost should reach them —
        // and nothing else here can, since `$PATH` has never heard of any of them.
        for (name, _) in oslo_base::vocab::all() {
            consider(&name, Origin::Shell);
        }

        // A binary search rather than a walk: `$PATH` holds a few thousand names here and only the
        // ones sharing the typed prefix can win.
        let sorted = CommandIndex::sorted(&path);
        let range = CommandIndex::starting_with(&sorted, stem);
        for name in &sorted[range] {
            consider(name, Origin::External);
        }

        best.map(|b| b.name[stem.len()..].to_string())
    }
}

impl OsloHelper {
    /// The completion of a *path* being typed, or `None`.
    ///
    /// fish's third source, and the one that covers the argument rather than the command. Only
    /// for a word that is not in command position: a bare name at the start of a line is a command
    /// to look up, not a file in the current directory, and suggesting `./notes.txt` when someone
    /// typed `no` would be nonsense.
    ///
    /// **Unless it has a `/` in it**, which is how a command *is* named as a path. `./bui` and
    /// `/usr/bin/gre` are commands nothing on `$PATH` can answer for, so refusing them here left
    /// them with no ghost from any source at all.
    pub fn path_hint(&self, line: &str, pos: usize) -> Option<String> {
        let word = current_word(line, pos);
        if word.stem.is_empty() || (word.command_position && !word.stem.contains('/')) {
            return None;
        }

        // Split what was typed into the directory to look in and the stem to match. A trailing
        // `/` means the directory itself is complete and every entry in it is a candidate.
        let typed = word.stem.as_str();
        let (dir, stem) = match typed.rfind('/') {
            Some(cut) => (&typed[..=cut], &typed[cut + 1..]),
            None => ("", typed),
        };
        // Only once there is something to match on. Listing a directory on the keystroke that
        // begins a word would fire for every argument of every command.
        if stem.is_empty() && dir.is_empty() {
            return None;
        }

        // **The same rule Tab follows.** `cd` refuses a file, and the ghost had no notion of that —
        // so `cd a` was offered `azzz/` by Tab and suggested `aa` by the ghost, which `cd` then
        // refused. It looked right only while the directory happened to sort first.
        let only_dirs = word
            .prior_words
            .first()
            .map(|w| crate::words::unquote(w))
            .is_some_and(|c| crate::completion::takes_only_directories(&c));

        let expanded = expand_tilde(dir);
        let base = if expanded.is_empty() { "." } else { &expanded };
        // **Gathered first, then ranked, and only then asked whether it runs.**
        //
        // The executable test is a `statx`, and asking it inside the loop asked it of every name
        // that merely shared a prefix — 43,763 of them per keystroke in a directory of 40,000, to
        // choose one. The ranking does not depend on the answer, so it goes first and the syscall is
        // paid for the winner alone. `file_type` stays in the loop because Linux hands it back with
        // the directory entry; it is not a syscall.
        let mut matches: Vec<(String, String, bool)> = Vec::new();
        for (name, is_dir) in entries_of(base)?.iter() {
            let (name, is_dir) = (name.clone(), *is_dir);
            if !name.starts_with(stem) || name.len() == stem.len() {
                continue;
            }
            // A dotfile is only offered once the user has typed the dot, the same rule globbing
            // follows — otherwise every argument suggests `.git`.
            if name.starts_with('.') && !stem.starts_with('.') {
                continue;
            }
            if only_dirs && !is_dir {
                continue;
            }
            let candidate = if is_dir {
                format!("{name}/")
            } else {
                name.clone()
            };
            matches.push((candidate, name, is_dir));
        }
        // Shortest wins: it is the least presumptuous completion, and the one the user is most
        // likely already heading for. Sorted on the candidate itself — the very value the old loop
        // compared — so the answer cannot drift from what it used to be.
        matches.sort_by(|a, b| (a.0.len(), &a.0).cmp(&(b.0.len(), &b.0)));

        let mut best: Option<String> = None;
        // How many names may be tested for executability before the ghost gives up.
        //
        // The test is a `statx`, and "stop at the first one that runs" is only cheap when one of
        // them does: typing `./` in a directory of twenty thousand data files matches every entry
        // and finds nothing runnable, so the walk paid twenty thousand syscalls to answer nothing.
        // A ghost is a suggestion, and a suggestion that is not among the shortest few dozen names
        // was never going to be the one — so past that, silence is the right answer and the cheap
        // one. `completion::paths` bounds its own walk the same way.
        const MAX_EXECUTABLE_TESTS: usize = 64;
        let mut tested = 0;
        for (candidate, name, is_dir) in matches {
            // A command named as a path can only be one that runs, so a plain data file is not a
            // suggestion for it — the rule bash follows, and the reason `{dir}/not` in command
            // position stays silent while `./bui` reaches an executable `build.sh`.
            if word.command_position && !is_dir {
                tested += 1;
                if tested > MAX_EXECUTABLE_TESTS {
                    break;
                }
                if !crate::completion::executable(&std::path::Path::new(base).join(&name)) {
                    continue;
                }
            }
            if best
                .as_ref()
                .is_none_or(|b| (candidate.len(), &candidate) < (b.len(), b))
            {
                best = Some(candidate);
            }
        }
        best.map(|name| name[stem.len()..].to_string())
    }
}

/// A leading `~`, so a path typed with one can still be suggested.
///
/// **All four forms**, through the same expander the shell uses. Knowing only `~` and `~/…` left
/// `~root/bi` and `~+/sr` with no ghost at all, though the shell expands both exactly as bash does.
fn expand_tilde(dir: &str) -> String {
    // `@name` as well as `~`, because the ghost was the only one of the four that did not know it.
    // Tab completes `ls @proj/zeb`, the highlighter colours it and the expander resolves it — but
    // the hint stayed blank, and `cd @proj/` is the case marks exist for.
    // `highlight::names_an_existing_file` has the same two branches, for the same reason.
    if let Some(rest) = dir.strip_prefix('@') {
        return match oslo_base::dirs::expand_at(rest) {
            Some(path) => path,
            // A name that stands for nothing keeps its own text, so the caller reads it as a
            // literal directory rather than as the filesystem root.
            None => dir.to_string(),
        };
    }
    oslo_base::tilde::expand_prefix(dir, &oslo_base::tilde::from_process)
}

/// One candidate hint, with everything the ordering looks at.
struct Ranked {
    score: f64,
    origin: Origin,
    name: String,
}

impl Ranked {
    /// Most used first; then shell-provided; then shortest; then alphabetical.
    ///
    /// Length before the alphabet because with nothing else to go on the shortest completion is
    /// the least presumptuous — it is the one the user is most likely already heading for.
    fn beats(&self, other: &Self) -> bool {
        match self.score.partial_cmp(&other.score) {
            Some(std::cmp::Ordering::Greater) => return true,
            Some(std::cmp::Ordering::Less) | None => return false,
            Some(std::cmp::Ordering::Equal) => {}
        }
        if self.origin != other.origin {
            return self.origin > other.origin;
        }
        if self.name.len() != other.name.len() {
            return self.name.len() < other.name.len();
        }
        self.name < other.name
    }
}

#[cfg(test)]
mod lua_hint_tests {
    use super::lua_hint;
    use crate::completion::lua::set_name_source;

    fn offering(names: &'static [(&'static str, bool)]) {
        set_name_source(Some(std::rc::Rc::new(move |_: &[String]| {
            names
                .iter()
                .map(|(n, callable)| ((*n).to_string(), *callable))
                .collect()
        })));
    }

    /// **The ghost is the prefix every candidate agrees on**, so it appears even when the name is
    /// not yet pinned down — and accepting it can never type the wrong thing.
    #[test]
    fn the_ghost_offers_what_every_candidate_agrees_on() {
        offering(&[("difftime", true), ("date", true), ("remove", true)]);
        // `da` pins one name, so the whole of it is offered.
        assert_eq!(lua_hint("da", 2).as_deref(), Some("te"));

        // Two names share `d`, and they agree on nothing past it: no ghost rather than a guess.
        offering(&[("difftime", true), ("dofile", true)]);
        assert_eq!(lua_hint("di", 2).as_deref(), Some("fftime"));

        // Several names agreeing on more than what was typed offer the shared part.
        offering(&[("tostring", true), ("tonumber", true)]);
        assert_eq!(
            lua_hint("to", 2),
            None,
            "when the candidates agree on nothing past what was typed, there is no ghost"
        );
        offering(&[("setmetatable", true), ("setlocale", true)]);
        assert_eq!(lua_hint("se", 2).as_deref(), Some("t"));
    }

    /// One letter agrees with most of a namespace, so nothing is offered from it.
    #[test]
    fn a_single_character_offers_nothing() {
        offering(&[("print", true), ("pairs", true)]);
        assert_eq!(lua_hint("p", 1), None);
    }
}
