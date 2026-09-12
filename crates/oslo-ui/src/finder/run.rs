//! Opening the finder: raw mode, the key loop, and putting the terminal back.
//!
//! # The one rule
//!
//! **Whatever happens, the terminal is restored.** The alternate screen, the cursor, the original
//! termios — all three are undone on every path out, including the ones nobody plans for. A finder
//! that leaves the screen in its own mode does not merely look wrong; it leaves a shell you cannot
//! type into, and the only way out is to close the window. So the restore is a guard that runs on
//! drop rather than a line at the end of the loop, and the loop below cannot forget it.

use super::Scope;
use super::rank::{Ranked, rank};
use super::render::{Frame, frame, visible_rows};
use crate::matching::Fuzzy;
use crate::scanner::Scanner;
use crate::term::{Key, Keys, Pressed, Restore, Screen};
use oslo_base::track::history::Command;
use std::io::{self, Write};

/// What the finder was left with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// A line to put back on the prompt, and the language it was typed in.
    Chosen { line: String, mode: String },
    /// Esc, or Ctrl-C: the prompt comes back exactly as it was.
    Cancelled,
}

/// Open the finder over `commands` and run it until it is dismissed.
///
/// `None` when there is no terminal to draw on or nothing to show — a finder that opened onto an
/// empty screen would be a keystroke that appears to hang.
pub fn open(
    commands: &[Command],
    cwd: &str,
    now: i64,
    fuzzy: Fuzzy,
    seed: &str,
    scope: Option<Scope>,
) -> Option<Outcome> {
    if commands.is_empty() {
        return None;
    }
    // Raw mode and the alternate screen, both undone whichever way this returns. The prompt
    // underneath is untouched and comes back exactly as it was, which is the whole reason a
    // full-screen finder is affordable here.
    let restore = Restore::enter(Screen::Alternate)?;

    // When the finder opened, so the scanner in the search bar knows how far through its sweep it
    // is. Taken once: the animation is a function of elapsed time, not of a counter to keep.
    let opened = std::time::Instant::now();
    // Only the *step* is needed here — the bar owns how wide the sweep is drawn. Sharing the
    // default keeps the loop's tick and the animation's frame rate the same number.
    let scanner = Scanner::default();

    let mut stdout = io::stdout();
    let mut state = State::new(commands, cwd, fuzzy, seed);
    state.start_in(scope);
    let mut keys = Keys::on(restore.fd());

    // The last frame written, so an unchanged one is not written again.
    //
    // The scanner holds still for nine frames at each end of its sweep, and nothing else moves
    // between keystrokes — so without this the finder would rewrite the whole screen many times a
    // second to produce a picture identical to the one already on it.
    let mut last = String::new();

    loop {
        let (cols, rows) = terminal_size();
        state.fit(rows);
        // One bool per visible row, resolved here because the frame is drawn against a list that
        // cannot change underneath it — the state keeps marks by name, not by position.
        let marked: Vec<bool> = state
            .matches
            .iter()
            .map(|row| {
                state
                    .marked
                    .contains(&(row.command.line.clone(), row.command.mode.clone()))
            })
            .collect();
        let painted = frame(&Frame {
            matches: &state.matches,
            selected: state.selected,
            offset: state.offset,
            query: &state.query,
            elapsed_ms: opened.elapsed().as_millis() as u64,
            confirm: state.confirm,
            marked: &marked,
            profile: &state.profile,
            scope: state.scope,
            total: state.total(),
            cols,
            rows,
            now,
        });
        if painted != last {
            let _ = stdout.write_all(painted.as_bytes());
            let _ = stdout.flush();
            last = painted;
        }

        // **Waited for with a deadline, not blocked on.** A blocking read would leave the
        // scanner frozen between keystrokes, which is the opposite of what an animation is for.
        // On a timeout the loop simply comes back round and draws the next frame.
        let pressed = match keys.read_within(scanner.step_ms as i32) {
            Pressed::Key(key) => key,
            Pressed::Timeout => continue,
            Pressed::Ended => return Some(Outcome::Cancelled),
        };

        // While the question is up it owns the keyboard: anything else typed would filter a list
        // you cannot see the search bar for, and Enter would mean two different things at once.
        if let Some(yes) = state.confirm {
            match pressed {
                Key::Left | Key::Right | Key::ToggleScope | Key::BackTab => {
                    state.confirm = Some(!yes)
                }
                Key::Accept => {
                    state.confirm = None;
                    if yes {
                        state.forget_doomed();
                    }
                }
                // **Delete again means yes.** The key that asked the question is the one already
                // under the finger, and pressing it twice is how somebody who knows what they are
                // doing gets through the guard without reaching for Enter. It answers whichever
                // button is highlighted, because the second press is the answer.
                Key::Delete => {
                    state.confirm = None;
                    state.forget_doomed();
                }
                // Esc and Ctrl-C answer *no* rather than leaving the finder: you asked to delete
                // something and changed your mind, which is not the same as wanting to close.
                Key::Cancel | Key::Abort => state.confirm = None,
                _ => {}
            }
            continue;
        }

        match pressed {
            // The history finder has one way out: both leave the line as it was.
            Key::Cancel | Key::Abort => return Some(Outcome::Cancelled),
            Key::Accept => {
                return Some(match state.matches.get(state.selected) {
                    Some(chosen) => Outcome::Chosen {
                        line: chosen.command.line.clone(),
                        mode: chosen.command.mode.clone(),
                    },
                    // Enter on an empty result list dismisses rather than choosing nothing:
                    // there is no line to put back, and clearing what the prompt already had
                    // would lose work.
                    None => Outcome::Cancelled,
                });
            }
            Key::Up => state.up(),
            Key::Down => state.down(),
            Key::PageUp => state.page_up(),
            Key::PageDown => state.page_down(),
            // **The arrows move the scope.** There is nothing to move a cursor over — the search
            // box only ever appends — so the keys are free, and the scopes are a line from widest
            // to narrowest, which is what an arrow means.
            Key::Right => state.narrow_scope(),
            Key::Left => state.widen_scope(),
            Key::Backspace => {
                state.query.pop();
                state.refilter();
            }
            Key::Clear => {
                state.query.clear();
                state.refilter();
            }
            // Delete forgets the highlighted command — every run of it, in every directory. The
            // selection stays where it is so a run of unwanted lines can be cleared without
            // moving the cursor back each time.
            //
            // Asked about first unless the config turned that off: the rows are gone from the
            // store afterwards and the only way back is to run the command again.
            Key::Delete => {
                if state.doomed().is_empty() {
                    // Nothing marked and nothing under the cursor: nothing to ask about.
                } else if crate::settings::current().finder.confirm_delete {
                    // *No* is selected first, so a stray Enter answers the safe way.
                    state.confirm = Some(false);
                } else {
                    state.forget_doomed();
                }
            }
            // Ctrl-Space marks a row, so a scattered handful can go in one Delete rather than one
            // question each.
            Key::Ctrl(' ') => state.toggle_mark(),
            Key::Char(c) => {
                state.query.push(c);
                state.refilter();
            }
            // Keys the shared reader knows but the finder has no use for: a full-screen list
            // has no cursor to move within a line.
            // Tab moves to the next profile — a different pair of stores, so the whole list is
            // replaced rather than filtered.
            Key::ToggleScope => state.next_profile(),
            // A resize redraws on the next loop pass, which reads the size afresh.
            Key::Ctrl(_)
            | Key::Alt(_)
            | Key::Function(_)
            | Key::Ignored
            | Key::Resized
            | Key::Home
            | Key::End
            | Key::BackTab
            | Key::Submit
            | Key::CtrlTab => {}
        }
    }
}

/// Every command another profile's store knows.
///
/// Opened for the read and dropped again: the finder holds one profile's rows at a time, and
/// keeping every visited profile's store open would be a file handle per Tab press.
fn load_profile(name: &str) -> Vec<Command> {
    let limit = crate::settings::current().finder.limit;
    let Some(directory) = oslo_base::track::profile::profile_dir(
        std::env::var("XDG_DATA_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
        name,
    ) else {
        return Vec::new();
    };
    oslo_base::track::Track::open(&directory.join("hist.db"))
        .map(|track| track.commands(limit))
        .unwrap_or_default()
}

/// The finder's state between keystrokes.
struct State {
    /// Owned rather than borrowed: Delete removes a row, and a slice cannot lose one.
    commands: Vec<Command>,
    cwd: String,
    fuzzy: Fuzzy,
    query: String,
    scope: Scope,
    matches: Vec<Ranked>,
    /// Index into `matches`.
    selected: usize,
    /// First visible row, so a list longer than the screen scrolls under a fixed window.
    offset: usize,
    /// How many rows the list currently has. Set from the terminal each frame, because it can be
    /// resized while the finder is open.
    window: usize,
    /// `Some(true)` while Delete is waiting on an answer, with *yes* selected.
    confirm: Option<bool>,
    /// Rows Ctrl-Space has marked, by the pair that identifies a command in the store.
    ///
    /// **Not indices.** Every keystroke re-ranks and re-filters the list, so a row's position is
    /// meaningless a moment later — but `(line, mode)` is what `forget` takes and what a row *is*,
    /// so a mark survives typing, a scope change and a profile change.
    marked: std::collections::HashSet<(String, String)>,
    /// Which profile's history is on screen. Tab moves to the next one.
    profile: String,
    /// The git worktree the shell is standing in, resolved once when the finder opens.
    worktree: Option<String>,
    /// This shell's session id and this machine's name, to compare rows against.
    session: String,
    host: String,
}

impl State {
    /// `seed` is whatever was already on the prompt line.
    ///
    /// Carried in rather than starting empty, because pressing Up after typing `ls` means "find
    /// the `ls` I ran before" — throwing the word away and asking for it again is a keystroke tax
    /// on the one case the key exists for.
    fn new(commands: &[Command], cwd: &str, fuzzy: Fuzzy, seed: &str) -> State {
        let query = seed.trim().to_string();
        let matches = rank(commands, &query, cwd, fuzzy);
        State {
            commands: commands.to_vec(),
            cwd: cwd.to_string(),
            fuzzy,
            query,
            scope: Scope::Global,
            matches,
            // The list grows upward from the search bar, so the row *nearest* the bar is the one
            // selected when it opens: the best match, under the cursor, one keystroke away.
            selected: 0,
            offset: 0,
            window: 1,
            confirm: None,
            marked: std::collections::HashSet::new(),
            profile: oslo_base::track::profile::current(),
            worktree: crate::prompt::git_root_of(std::path::Path::new(cwd))
                .map(|root| root.to_string_lossy().into_owned()),
            session: oslo_base::track::session::id(),
            host: oslo_base::track::session::host(),
        }
    }

    /// Open in `wanted`, or — for `None`, the `auto` setting — in the workspace.
    ///
    /// Whichever it is, a scope with nothing in it falls back to global: outside a repository, or
    /// in one nothing has been run in yet, an empty finder would read as lost history.
    fn start_in(&mut self, wanted: Option<Scope>) {
        self.scope = wanted.unwrap_or(Scope::Workspace);
        if self.total() == 0 {
            self.scope = Scope::Global;
        }
        self.refilter();
    }

    fn refilter(&mut self) {
        let mut found = rank(&self.commands, &self.query, &self.cwd, self.fuzzy);
        found.retain(|row| self.in_scope(&row.command));
        self.matches = found;
        // Back to the top: the old selection referred to a list that no longer exists, and
        // keeping the index would land the cursor on an unrelated command.
        self.selected = 0;
        self.offset = 0;
    }

    /// What Delete would remove: everything marked, or the highlighted row when nothing is.
    ///
    /// The same rule `ui choose` uses for Enter, and for the same reason — marking first and then
    /// pressing the key is one way to work, and pressing the key on a row is the other, and neither
    /// should need the mode to be announced.
    fn doomed(&self) -> Vec<(String, String)> {
        if !self.marked.is_empty() {
            let mut wanted: Vec<(String, String)> = self.marked.iter().cloned().collect();
            wanted.sort();
            return wanted;
        }
        self.matches
            .get(self.selected)
            .map(|row| vec![(row.command.line.clone(), row.command.mode.clone())])
            .unwrap_or_default()
    }

    /// Forget everything Delete was aimed at, in the store and in the list on screen.
    fn forget_doomed(&mut self) {
        let doomed = self.doomed();
        if doomed.is_empty() {
            return;
        }
        if let Some(track) = oslo_base::track::store() {
            for (line, mode) in &doomed {
                track.forget(line, mode);
            }
        }
        // Dropped from the in-memory copy too, so the rows go now rather than the next time the
        // finder is opened — the key has to look like it did something.
        self.commands.retain(|command| {
            !doomed
                .iter()
                .any(|(line, mode)| command.line == *line && command.mode == *mode)
        });
        self.marked.clear();
        let was = self.selected;
        self.refilter();
        // Back to where the eye was. `refilter` homes the selection because a *query* change
        // makes the old index meaningless; a deletion does not — the rows around it are the same.
        self.selected = was.min(self.matches.len().saturating_sub(1));
    }

    /// Mark or unmark the row under the cursor, and step to the next one.
    ///
    /// **Moving is the point.** Marking a run of unwanted lines is the case this key exists for,
    /// and stopping after each one would mean two keystrokes per row. Unmarking is the rarer half
    /// and costs one step back.
    ///
    /// "Next" is [`up`](Self::up), because the list grows *upward* from the search bar: the row
    /// after the one under the cursor is the one above it on screen.
    fn toggle_mark(&mut self) {
        let Some(row) = self.matches.get(self.selected) else {
            return;
        };
        let it = (row.command.line.clone(), row.command.mode.clone());
        if !self.marked.remove(&it) {
            self.marked.insert(it);
        }
        self.up();
    }

    /// Whether `command` belongs to the scope being shown.
    ///
    /// Every answer comes from what the store recorded at the time the command ran. None of it is
    /// re-derived here: which shell ran a line, and which machine, are facts about that moment and
    /// cannot be recovered from the line afterwards.
    fn in_scope(&self, command: &Command) -> bool {
        match self.scope {
            Scope::Global => true,
            // Every row in a local store was run here, so this shows everything today. It filters
            // for real the moment a store is shared between machines, and rows written now already
            // carry the name.
            Scope::Host => command.host.is_empty() || command.host == self.host,
            Scope::Session => command.session == self.session,
            Scope::Directory => command.dir == self.cwd,
            // The worktree the *command* ran in, against the one the shell is standing in. Both
            // were resolved by the store when the directory was first seen.
            Scope::Workspace => match (&command.root, &self.worktree) {
                (Some(ran_in), Some(here)) => ran_in == here,
                // Not in a repository at all, so nothing is in this one.
                _ => false,
            },
        }
    }

    /// Show the next profile's history: a different store, so the commands are re-read.
    ///
    /// The query and the scope survive the switch. You are asking the same question of a different
    /// history, and making you retype it would be the wrong answer to "what did the agent run".
    fn next_profile(&mut self) {
        let Some(next) = oslo_base::track::profile::after(&self.profile) else {
            return;
        };
        self.commands = load_profile(&next);
        self.profile = next;
        self.selected = 0;
        self.offset = 0;
        self.refilter();
    }

    fn narrow_scope(&mut self) {
        self.scope = self.scope.next();
        self.refilter();
    }

    fn widen_scope(&mut self) {
        self.scope = self.scope.previous();
        self.refilter();
    }

    fn total(&self) -> usize {
        match self.scope {
            Scope::Global => self.commands.len(),
            // Counted over what this scope *could* show, not over the store: `3/5` is only
            // meaningful if the denominator is the pool the query narrowed.
            _ => self
                .commands
                .iter()
                .filter(|command| self.in_scope(command))
                .count(),
        }
    }

    /// Results are stored best-first but painted bottom-up. Moving visually upward therefore
    /// advances through the vector; moving down goes back toward index zero.
    fn up(&mut self) {
        self.move_by(1);
    }

    fn down(&mut self) {
        self.move_by(-1);
    }

    fn page_up(&mut self) {
        self.move_by(self.window as isize);
    }

    fn page_down(&mut self) {
        self.move_by(-(self.window as isize));
    }

    /// Move the selection, clamped, and bring it back into view.
    fn move_by(&mut self, delta: isize) {
        if self.matches.is_empty() {
            return;
        }
        let last = self.matches.len() - 1;
        let next = (self.selected as isize + delta).clamp(0, last as isize) as usize;
        self.selected = next;
        self.scroll_into_view();
    }

    /// Note this frame's window size and keep the selection visible in it.
    fn fit(&mut self, rows: usize) {
        self.window = visible_rows(rows);
        self.scroll_into_view();
    }

    fn scroll_into_view(&mut self) {
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + self.window {
            self.offset = self.selected + 1 - self.window;
        }
        // A window that grew past the end of the list would leave blank rows below the last match
        // and above the bar, which reads as the list having ended early.
        let max_offset = self.matches.len().saturating_sub(self.window);
        self.offset = self.offset.min(max_offset);
    }
}

/// The terminal's size, re-asked every frame because it can be resized while the finder is open.
fn terminal_size() -> (usize, usize) {
    (
        crate::dropdown::width::terminal_cols(),
        crate::dropdown::width::terminal_rows(),
    )
}

#[cfg(test)]
#[path = "run/tests.rs"]
mod tests;
