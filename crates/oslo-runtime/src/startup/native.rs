//! Adapts shell completion, history and highlighting to the native editor's [`Assist`].

use crate::lua::api::hooks;

mod keys;
use keys::{hook_key_name, key_name};
use oslo_ui::edit::session::{Assist, Bound, KeyHook, Placed};
use oslo_ui::term::Key;
use oslo_ui::{OsloHelper, abbr, dropdown, editor, settings};

/// How much of a tab's output is replayed on attaching: a screenful, roughly, at any sane width.
/// Enough to land on the screen it was left on, not enough to redraw a session's history.
#[cfg(feature = "scratch")]
const REPLAY: u64 = 8192;

/// What the shell plugs into an editing session.
pub struct ShellAssist<'a> {
    /// The completion and hinting machinery, borrowed rather than rebuilt: it carries the
    /// frecency table and the command index, and a second copy would rank differently.
    helper: Option<&'a OsloHelper>,
    /// Displayed prompt width used to place the completion menu.
    prompt_cols: usize,
    /// History, newest last — the same order the editor's own store keeps.
    history: Vec<String>,
    /// How many steps back into history the walk has gone. `0` is the line being composed.
    back: usize,
    /// What was on the line when the walk started, so coming back out restores it rather than
    /// blanking it. oslo has always promised this; it is the reason a walk is not destructive.
    composing: Option<String>,
    /// The keys that switch language — `crate::startup::mode::TOGGLE_KEYS`, less any the config
    /// unbound with `oslo.keys["shift-tab"] = "none"`.
    ///
    /// A list rather than one name because Shift+Tab needs the terminal to report a modifier and
    /// Ctrl+Space does not. See `TOGGLE_KEYS` for which cost each one carries.
    toggle: Vec<String>,
}

impl<'a> ShellAssist<'a> {
    pub fn new(
        history: Vec<String>,
        helper: Option<&'a OsloHelper>,
        prompt_cols: usize,
        toggle: Vec<String>,
    ) -> ShellAssist<'a> {
        ShellAssist {
            helper,
            prompt_cols,
            history,
            back: 0,
            composing: None,
            toggle,
        }
    }

    /// Start a fresh line: the walk position resets, or Up would resume where the last line left
    /// off and appear to skip entries.
    pub fn begin(&mut self) {
        self.back = 0;
        self.composing = None;
    }
}

/// A character index into the byte index Lua counts in.
///
/// The editor's cursor is a position among *characters*; `line.cursor` is documented as bytes so
/// that `line.text:sub(1, line.cursor)` works, and Lua's own string functions count bytes. The two
/// agree for ASCII and diverge on the first accented letter, which is why this is not skipped.
fn byte_cursor(line: &str, cursor: usize) -> usize {
    oslo_ui::edit::display::char_to_byte(line, cursor)
}

/// The byte index a handler answered with, back into a character index for the buffer.
fn char_cursor(line: &str, at: usize) -> usize {
    oslo_ui::edit::display::byte_to_char(line, at)
}

/// A handler's answer as the editor wants it: text, a character cursor, and whether to run it.
///
/// The cursor a handler gives is a byte offset, because that is what it computed with; the buffer
/// counts characters. Defaulting to the end is what "just set the line" means, and it is by far
/// the common case.
fn placed(answer: editor::Answer) -> Placed {
    let end = answer.text.chars().count();
    let cursor = answer
        .cursor
        .map(|at| char_cursor(&answer.text, at))
        .unwrap_or(end)
        .min(end);
    Placed {
        text: answer.text,
        cursor,
        submit: answer.submit,
        erase: answer.erase,
    }
}

/// Tell a hook something happened, spelled once so the call sites read as one line each.
fn fire(index: usize, fields: &[(&str, &str)]) {
    crate::lua::engine::fire_at_here(index, fields);
}

/// Open the full-screen history finder.
///
/// `None` means it could not open at all — no terminal, no store, nothing remembered — and the
/// caller should carry on as though it had never been asked. That is a different answer from
/// `Some(Cancelled)`, which means the user looked and declined, and where carrying on would
/// scroll their line away as if Esc had done something.
fn open_finder(seed: &str) -> Option<oslo_ui::finder::Outcome> {
    let settings = settings::current();
    if !settings.finder.enabled || !oslo_base::feature::on(oslo_base::feature::at::FINDER) {
        return None;
    }
    let track = oslo_base::track::store()?;
    // **The one read that has to be current.** The command you just ran is written on the writer
    // thread, so for a moment after the prompt returns the store does not have it yet — and asking
    // for your history and not finding the thing you just typed is the kind of wrong that makes a
    // shell feel broken. Worse: with nothing else in the store the finder declines to open at all.
    //
    // Waited for here and nowhere else. This is a key somebody deliberately pressed, so a fraction
    // of a millisecond is invisible; the ghost and the `cd` ranking read the same store on every
    // keystroke and must not, which is the whole point of the writes being off this thread.
    oslo_base::track::writer::settle();
    // Only this language's commands. The editor's history holds both, and offering a Lua line at a
    // shell prompt produces something that cannot run — the same crossing the ghost suggestion and
    // the arrow keys are already filtered for.
    let language = oslo_ui::prompt::language().unwrap_or_else(|| "sh".to_string());
    let commands: Vec<_> = track
        .commands(settings.finder.limit)
        .into_iter()
        .filter(|command| command.mode == language)
        .collect();
    if commands.is_empty() {
        return None;
    }
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // The three history events bracket the search itself. `open` is fired here rather than inside
    // the finder because everything above can decline — no store, nothing remembered, disabled by
    // config — and a hook that fired for a search that never appeared would be lying.
    fire(hooks::at::HISTORY_OPEN, &[("seed", seed)]);
    let outcome = oslo_ui::finder::open(
        &commands,
        &cwd,
        now,
        settings.completion.fuzzy,
        seed,
        settings.finder.scope,
    );
    match &outcome {
        Some(oslo_ui::finder::Outcome::Chosen { line, .. }) => {
            fire(hooks::at::HISTORY_SELECT, &[("line", line)]);
            fire(hooks::at::HISTORY_CLOSE, &[("chosen", "true")]);
        }
        _ => fire(hooks::at::HISTORY_CLOSE, &[("chosen", "false")]),
    }
    outcome
}

impl Assist for ShellAssist<'_> {
    fn highlight(&mut self, line: &str) -> String {
        let Some(helper) = self.helper else {
            return line.to_string();
        };
        if line.is_empty() {
            return String::new();
        }
        helper.paint(line)
    }

    /// The suggestion as plain text, which is what accepting it inserts.
    fn hint_text(&mut self, line: &str, cursor: usize) -> Option<String> {
        let helper = self.helper?;
        if cursor < line.chars().count() {
            return None;
        }
        helper.suggest(line, line.len())
    }

    fn paint_hint(&mut self, text: &str) -> String {
        self.helper
            .map(|helper| helper.paint_hint(text))
            .unwrap_or_else(|| text.to_string())
    }

    // Without the model there is nothing to correct a line *to*, so the editor's default applies:
    // `repair_text` answers `None` and nothing is ever drawn after the line. The keys keep their
    // ordinary meanings, because `take_repair` is only reached when this returns something.
    #[cfg(feature = "vista")]
    fn repair_text(&mut self, line: &str, cursor: usize) -> Option<String> {
        let helper = self.helper?;
        // At the end of the line only, like the suggestion: a correction offered while the cursor
        // is mid-word is about a line the user has not finished saying.
        if cursor < line.chars().count() {
            return None;
        }
        helper.repair(line)
    }

    #[cfg(feature = "vista")]
    fn paint_repair(&mut self, typed: &str, fixed: &str) -> String {
        self.helper
            .map(|helper| helper.paint_repair(typed, fixed))
            .unwrap_or_else(|| fixed.to_string())
    }

    /// Tab. Runs the whole interaction — the dropdown draws itself and takes its own keys — and
    /// answers with the line it produced.
    /// Open the scratch finder, from a prompt in a shell that is not itself inside a scratch's
    /// client.
    ///
    /// Errors are printed rather than returned: this is a key, and a key that fails has to say so
    /// where it was pressed. The prompt comes back either way.
    #[cfg(feature = "scratch")]
    fn open_scratch(&mut self) -> bool {
        use oslo_shell::scratch::enter;
        let key = settings::current().scratch.key.clone();
        match enter::open(&key, REPLAY) {
            Ok(enter::Went::ThereAndBack) => true,
            Ok(enter::Went::Nowhere) => false,
            Err(err) => {
                eprintln!("\r\noslo: scratch: {err}");
                true
            }
        }
    }

    /// Open the macro manager on the key that asked for it.
    ///
    /// The same screen `oslo macros show` opens, and the same code — see [`crate::macros::screen`].
    /// A failure is printed where the key was pressed rather than returned, because there is nobody
    /// above this to report it to: the prompt comes back either way.
    /// Hand the line to `$EDITOR`, and take back what it wrote.
    ///
    /// The same `crate::editor::edit` the macro manager uses, so the choice of editor, the
    /// temporary file's permissions and "unchanged is not a write" are all decided in one place.
    /// The extension is `sh` because that is what the line *is*, and syntax highlighting is most of
    /// the reason to want a real editor for it.
    ///
    /// **A trailing newline is dropped.** Every editor puts one at the end of a file and none of
    /// them means it as part of the command; keeping it would make Enter run a line with a blank
    /// one after it. Newlines *inside* stay, because a shell line may genuinely have them.
    ///
    /// The terminal is not handed over explicitly, on the same terms as the macro manager's own
    /// editing: `$EDITOR` saves the modes it finds and puts them back on exit, and the prompt is
    /// rebuilt afterwards because whatever is on the screen was written by something else.
    fn edit_externally(&mut self, line: &str) -> Option<String> {
        match crate::editor::edit(line, "sh") {
            Ok(Some(edited)) => Some(edited.trim_end_matches(['\n', '\r']).to_string()),
            // Saved with no change, or quit without saving. Either way the line stands.
            Ok(None) => None,
            Err(problem) => {
                eprintln!("\r\noslo: {problem}");
                None
            }
        }
    }

    fn open_macros(&mut self) -> bool {
        match crate::macros::screen() {
            Some(Ok(())) => true,
            Some(Err(problem)) => {
                eprintln!("\r\noslo: macros: {problem}");
                true
            }
            // No terminal to draw on. Nothing happened, so nothing is repainted.
            None => false,
        }
    }

    fn complete(
        &mut self,
        line: &str,
        cursor: usize,
        _back: bool,
        keys: &mut oslo_ui::term::Keys,
    ) -> Option<(String, usize)> {
        let helper = self.helper?;
        // The dropdown works in bytes; the editor's cursor is in characters.
        let pos: usize = line.chars().take(cursor).map(char::len_utf8).sum();
        let (start, candidates) = helper.complete_word(line, pos);
        // Taken whatever happens, so a failure from this Tab is never shown on a later one.
        let failure = oslo_ui::spec::remote::take_failure();
        // `on-completion-start` fires only once there is something to choose from. Tab on a word
        // nothing matches has not started a completion — it has done nothing, and a hook that said
        // otherwise would fire on every stray Tab.
        if candidates.is_empty() {
            // Nothing, but for a reason worth saying: `host:/dir/` that could not be listed.
            if let Some(why) = failure {
                let cells = self.prompt_cols + dropdown::visible_len(&line[..start]);
                let indent = cells % dropdown::terminal_cols().max(1);
                dropdown::notice(&why, indent, &line[start..pos], keys);
            }
            return None;
        }
        fire(
            hooks::at::COMPLETION_START,
            &[
                ("word", &line[start..pos]),
                ("line", line),
                ("count", &candidates.len().to_string()),
            ],
        );

        let chosen = if candidates.len() == 1 {
            // Already recorded by `complete_word`, which is where "one candidate is an
            // acceptance" belongs so that every caller agrees.
            candidates.into_iter().next()?
        } else {
            // **The column the word is on, not how far along the line it is.** Those are the same
            // number only until the line wraps: past that, the absolute offset is wider than the
            // terminal and the menu was clamped to the same place whatever was typed — a word at
            // column 10 of the second row got a dropdown at column 31. The wrap is what the
            // terminal does to the offset, so it is what the indent has to do too.
            let cells = self.prompt_cols + dropdown::visible_len(&line[..start]);
            let indent = cells % dropdown::terminal_cols().max(1);
            let Some(chosen) = dropdown::DropdownMenu::select_interactive(
                candidates,
                indent,
                &line[start..pos],
                keys,
            ) else {
                // Esc, or a key that closed the menu. Distinct from "nothing matched": the choice
                // was offered and declined, which is the moment a hook wants to know about.
                fire(hooks::at::COMPLETION_CANCEL, &[("word", &line[start..pos])]);
                return None;
            };
            helper.record_accepted(&chosen);
            chosen
        };
        fire(
            hooks::at::COMPLETION_SELECT,
            &[("value", &chosen.replacement), ("word", &line[start..pos])],
        );

        let mut out = String::with_capacity(line.len() + chosen.replacement.len());
        out.push_str(&line[..start]);
        out.push_str(&chosen.replacement);
        let at = out.chars().count();
        out.push_str(&line[pos..]);
        Some((out, at))
    }

    fn expand_glob(&mut self, line: &str, cursor: usize) -> Option<(String, usize)> {
        let helper = self.helper?;
        let pos: usize = line.chars().take(cursor).map(char::len_utf8).sum();
        let (start, end, words) = helper.glob_words(line, pos)?;
        let mut out = String::with_capacity(line.len() + 64);
        out.push_str(&line[..start]);
        out.push_str(&words.join(" "));
        let at = out.chars().count();
        out.push_str(&line[end.max(pos)..]);
        Some((out, at))
    }

    fn list_glob(&mut self, line: &str, cursor: usize, keys: &mut oslo_ui::term::Keys) {
        let Some(helper) = self.helper else {
            return;
        };
        let pos: usize = line.chars().take(cursor).map(char::len_utf8).sum();
        let (start, shown) = match helper.glob_words(line, pos) {
            Some((start, _, words)) => (
                start,
                format!("{} matches: {}", words.len(), words.join("  ")),
            ),
            None => (
                oslo_ui::words::current_word(line, pos).start,
                "matches nothing".to_string(),
            ),
        };
        let start = start.min(pos);
        let cells = self.prompt_cols + dropdown::visible_len(&line[..start]);
        let indent = cells % dropdown::terminal_cols().max(1);
        dropdown::notice(&shown, indent, &line[start..pos], keys);
    }

    /// What the config bound `key` to.
    ///
    /// The order is the order of specificity: an `oslo.keys` entry is the most explicit statement
    /// a config makes, then the suggestion-accept keys, then oslo's own defaults. A default that
    /// could shadow a config entry would make the entry look ignored.
    fn binding(&mut self, key: Key) -> Option<Bound> {
        let name = key_name(key)?;
        let settings = settings::current();

        if let Some((_, action)) = settings.keys.iter().find(|(bound, _)| *bound == name) {
            return match oslo_ui::keys::action(action) {
                Some(oslo_ui::keys::Action::ToggleLanguage) => Some(Bound::ToggleLanguage),
                Some(oslo_ui::keys::Action::ClearScreen) => Some(Bound::ClearScreen),
                Some(oslo_ui::keys::Action::HistorySearchBackward) => Some(Bound::SearchHistory),
                Some(oslo_ui::keys::Action::AcceptSuggestion) => Some(Bound::AcceptHint),
                Some(oslo_ui::keys::Action::AcceptSuggestionWord) => Some(Bound::AcceptHintWord),
                Some(oslo_ui::keys::Action::Interrupt) => Some(Bound::Interrupt),
                Some(oslo_ui::keys::Action::Complete) => Some(Bound::Complete),
                Some(oslo_ui::keys::Action::EditExternally) => Some(Bound::EditExternally),
                Some(oslo_ui::keys::Action::ExpandGlob) => Some(Bound::ExpandGlob),
                Some(oslo_ui::keys::Action::ListGlob) => Some(Bound::ListGlob),
                Some(oslo_ui::keys::Action::LuaHandler) => Some(Bound::Lua(name)),
                // Unbound on purpose. Answering `None` here rather than with a do-nothing `Bound`
                // is what makes it reach the *defaults* below and cancel them too — which is the
                // whole point, since Shift-Tab is bound before any config has run.
                Some(oslo_ui::keys::Action::Nothing) => return None,
                // An action name oslo does not know was already reported when the config was
                // read; doing nothing here is better than doing something arbitrary.
                None => None,
            };
        }

        // Ranked below `oslo.keys` so that binding the same chord to something else wins, and
        // above the defaults because `oslo.scratch.key` is a statement the config made on purpose.
        // Only in a build that has tabs: elsewhere the setting is read and means nothing, so the
        // key must fall through to whatever it would otherwise have done.
        #[cfg(feature = "scratch")]
        if settings.scratch.key == name {
            return Some(Bound::OpenScratch);
        }

        // Ranked with it, and for the same reason: `oslo.macros.key` is a statement the config
        // made on purpose, and `oslo.keys` binding the same chord to something else still wins.
        if settings.macros.key == name {
            return Some(Bound::OpenMacros);
        }

        if settings.suggest.accept.as_deref() == Some(name.as_str()) {
            return Some(Bound::AcceptHint);
        }
        if settings.suggest.accept_word.as_deref() == Some(name.as_str()) {
            return Some(Bound::AcceptHintWord);
        }

        // `alt-*` and `alt-g` are bash's `C-x *` and `C-x g`, on single keys because oslo has no
        // chords. Below every config binding, so `oslo.keys` can take either key back.
        match name.as_str() {
            "alt-*" => return Some(Bound::ExpandGlob),
            "alt-g" => return Some(Bound::ListGlob),
            _ => {}
        }

        // oslo's own default, for a key the ordinary keymap does not already answer. Reached only
        // when the config said nothing about this key: `oslo.keys["shift-tab"] = "none"` returns
        // above and so cancels it.
        self.toggle
            .iter()
            .any(|key| key == name.as_str())
            .then_some(Bound::ToggleLanguage)
    }

    fn key_name(&mut self, key: Key) -> Option<String> {
        let name = key_name(key)?;
        // **Asked of the handler registry, not of `oslo.keys` in the settings.** A binding whose
        // value is a *function* never reaches the settings — those hold only the name-to-action
        // strings — so filtering on them meant every Lua binding was silently never consulted.
        //
        // A hash lookup per named key is cheap, and `key_name` has already ruled out every
        // ordinary character, which is what keeps this off the path of simply typing.
        editor::handler(&name).is_some().then_some(name)
    }

    fn lua_key(&mut self, name: &str, line: &str, cursor: usize) -> Option<Placed> {
        let handler = editor::handler(name)?;
        let table = editor::line_table(line, byte_cursor(line, cursor));
        let answer = match crate::lua::engine::call_here(&handler, vec![table]) {
            Ok(values) => values.into_iter().next().unwrap_or_default(),
            // Reported rather than swallowed: a binding that silently does nothing is
            // indistinguishable from one that was never installed.
            Err(e) => {
                eprintln!("oslo: keys['{name}']: {e}");
                return None;
            }
        };
        let answer = editor::answer_from(&answer)?;
        Some(placed(answer))
    }

    fn watches_keys(&mut self) -> bool {
        crate::lua::engine::key_hook_watched()
    }

    fn key_hook(&mut self, key: Key, line: &str, cursor: usize) -> Option<KeyHook> {
        let (name, pressed) = hook_key_name(key);
        let table = editor::key_table(&name, pressed, line, byte_cursor(line, cursor));
        match editor::key_outcome_from(&crate::lua::engine::key_hook_here(vec![table])?) {
            editor::KeyOutcome::Pass => None,
            editor::KeyOutcome::Swallow => Some(KeyHook::Swallow),
            editor::KeyOutcome::Line(answer) => Some(KeyHook::Line(placed(answer))),
        }
    }

    fn abbreviation(
        &mut self,
        line: &str,
        cursor: usize,
        ending: Option<char>,
    ) -> Option<(String, usize)> {
        // **Enter turns `pattern(qualifiers)` into filenames before the line runs**, so what runs
        // and what history records is the files. Not gated on abbreviations: a different feature
        // that happens to share the moment. See `oslo_ui::completion::qualified`.
        if ending.is_none() {
            match oslo_ui::completion::qualified::rewrite(line) {
                Ok(Some(text)) => {
                    let cursor = text.chars().count();
                    return Some((text, cursor));
                }
                Ok(None) => {}
                // Left as typed, and said why: the shell's own complaint about the `(` follows.
                Err(problem) => eprintln!("oslo: {problem}"),
            }
        }
        if !oslo_base::feature::on(oslo_base::feature::at::ABBR) {
            return None;
        }
        // The dropdown and `abbr` both work in bytes; the editor's cursor is in characters.
        let at: usize = line.chars().take(cursor).map(char::len_utf8).sum();
        let (mut text, expanded_to) = abbr::expand(line, at)?;
        // Whatever the keystroke would have typed, supplied here because this consumed it. Enter
        // types nothing, so the expansion is the whole of what it leaves behind.
        let Some(ending) = ending else {
            let cursor = text[..expanded_to].chars().count();
            return Some((text, cursor));
        };
        text.insert(expanded_to, ending);
        let cursor = text[..expanded_to + ending.len_utf8()].chars().count();
        Some((text, cursor))
    }

    fn search_history(&mut self, line: &str) -> Option<String> {
        // Chosen, but **not run**: you may want to edit it first, which is the contract every
        // other recall in the shell has.
        match open_finder(line)? {
            oslo_ui::finder::Outcome::Chosen { line, .. } => Some(line),
            oslo_ui::finder::Outcome::Cancelled => None,
        }
    }

    fn history_prev(&mut self, line: &str) -> Option<String> {
        // Up opens the finder when the config asked for that, which is oslo's default: a
        // full-screen fuzzy search over what you have actually run.
        let settings = settings::current();
        if self.back == 0 && settings.finder.enabled && settings.finder.key == "up" {
            // Whatever is already on the line seeds the search: pressing Up after typing `ls`
            // means "the `ls` I ran before".
            match open_finder(line) {
                Some(oslo_ui::finder::Outcome::Chosen { line, .. }) => return Some(line),
                // Looked and declined: leave the line exactly as it was rather than falling
                // through to a walk, which would scroll it away as if Esc had done something.
                Some(oslo_ui::finder::Outcome::Cancelled) => return None,
                // Could not open — no terminal, or nothing remembered yet. Walk the history the
                // ordinary way, which is what Up meant before the finder existed.
                None => {}
            }
        }
        let entry = self.history.iter().rev().nth(self.back)?.clone();
        if self.back == 0 {
            self.composing = Some(line.to_string());
        }
        self.back += 1;
        Some(entry)
    }

    fn history_next(&mut self) -> Option<String> {
        match self.back {
            0 => None,
            // Out the far end of the walk: the line being composed comes back.
            1 => {
                self.back = 0;
                self.composing.take()
            }
            _ => {
                self.back -= 1;
                self.history.iter().rev().nth(self.back - 1).cloned()
            }
        }
    }
}

#[cfg(test)]
#[path = "native/tests.rs"]
mod tests;
