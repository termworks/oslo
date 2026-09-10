//! Reading one command from the line editor.
//!
//! Split from [`super::repl`] because reading and running are separate concerns and only one of
//! them is hard. Running a command is a call; *reading* one means a continuation prompt when the
//! line is unfinished, a here-document body that must go in exactly as typed, two languages to
//! choose between, and a key that hands control back by accepting the line.

use crate::expand_history;
use crate::lua::LuaEngine;
use crate::startup::mode::{self, Line, Mode};
use crate::startup::{history, prompt, rc};
use oslo_shell::Environment;
use oslo_ui::InputStatus;
use std::sync::{Arc, Mutex};

/// One trip round the prompt.
pub(super) enum Input {
    /// A complete command, the language to run it in, and whether the user asked for it not to be
    /// remembered.
    Command {
        text: String,
        mode: Mode,
        secret: bool,
    },
    /// Nothing to run: a blank line, or a history reference that did not resolve.
    Nothing,
    /// Ctrl-C. The partial command is dropped and the prompt comes back.
    Interrupted,
    /// End of input. `$IGNOREEOF` may ask for this to be ignored.
    Eof,
}

/// Put a `prompt.transient` where the prompt that read `line` was.
///
/// **The prompt above a finished line has done its job**, and a config that asked for a shorter one
/// to stand in its place gets it here — before the command runs, so what scrolls past is one row of
/// prompt per command rather than three.
///
/// Finished, not accepted: a line abandoned with Ctrl-C is as over as one that ran, and this used
/// to be inline on the accepting path only. `oslo.transcript.rule`, which does the same job a
/// different way, is applied by the editor and had the same gap — see `edit::session`.
///
/// Only to a terminal, and only from the row the editor actually drew: `rewind_after_readline`
/// accounts for the wrap, which is why this is not `ESC [ 1 A`.
fn stand_down(lua: &LuaEngine, last_status: i32, reading: Mode, line: &str) {
    if !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        return;
    }
    let Some(short) = lua.render_with(
        "prompt.transient",
        &prompt::segment_context(last_status, reading, None),
    ) else {
        return;
    };
    print!(
        "{}{short}{line}\r\n",
        oslo_ui::row::rewind_after_readline(line)
    );
    let _ = std::io::Write::flush(&mut std::io::stdout());
}

/// Read one complete command, continuing onto further lines while the parser wants more.
///
/// This is where `PS2` earns its keep: a command that is not finished gets the continuation
/// prompt, instead of the hard syntax error `for i in 1 2 3` used to produce the moment you
/// pressed Enter.
pub(super) fn read_command(
    helper: &oslo_ui::OsloHelper,
    history: &super::history::store::History,
    env_struct: &Arc<Mutex<Environment>>,
    lua: &LuaEngine,
    last_status: i32,
    current: &mut Mode,
) -> Input {
    // A Ctrl-C that arrived while the last command was finishing belongs to that command, not to
    // the one about to be typed. Nothing else drains the flag, so leaving it set made the next
    // command abort at its first boundary with 130 and no output.
    oslo_shell::exec::job::forget_interrupt();
    // Read once per command: a line that changed it mid-block would read its own continuation
    // under a different rule from its first line. `oslo.lua.enter`, from the config — never
    // inferred from the terminal, see `settings::Lua::enter`.
    let lua_enter = oslo_ui::settings::current().lua.enter;
    let mut buffer = String::new();
    let mut secret = false;
    let mut heredoc = HeredocTracker::default();
    // The language *this* command is being read in. It follows `current` until a `!` prefix at a
    // shell prompt sends one line to Lua; a continuation line never re-decides.
    let mut reading = *current;
    // Carried across a toggle, so switching language mid-command does not lose what was typed.
    let mut typed = String::new();
    // Where the cursor goes when `typed` is put back. Only a `bind -x` command moves it away from
    // the end — `$READLINE_POINT` is how a plugin says "leave the cursor here", and a picker that
    // inserts a word mid-line needs it.
    let mut typed_point = 0usize;
    // Language and vi redraws reuse the current prompt region. Only accepting an incomplete
    // physical line opens a continuation region inside the same interaction.
    let mut announce_prompt = Some(oslo_ui::marks::PromptKind::Primary);

    loop {
        // Every line starts in insert mode as far as the editor is concerned, so the mode this
        // module remembers has to start there too. Before the prompt is *rendered*, not after:
        // leaving a line in normal mode otherwise drew the next prompt saying `N` while the editor
        // was already back in insert, and it stayed wrong until the first keystroke.
        oslo_ui::vi::reset();
        // The shape too: the terminal is still drawing whatever the last line ended in, and a
        // block cursor over a line you are typing into says normal mode when it is insert.
        // Only to a terminal: a cursor-shape escape written down a pipe is not a cursor shape,
        // it is two stray bytes in somebody's output.
        let settings = oslo_ui::settings::current();
        // `vi::enabled` rather than the setting, so the `vi` feature is asked about here too — the
        // cursor and the key bindings must not disagree about which mode the editor is in.
        if settings.vi.enabled
            && oslo_ui::vi::enabled()
            && std::io::IsTerminal::is_terminal(&std::io::stdout())
        {
            print!("{}", settings.vi.cursors.insert.escape());
        }

        let prompt = if buffer.is_empty() {
            prompt::primary_prompt(env_struct, lua, last_status, *current)
        } else {
            // A `prompt.continuation` written in Lua wins, then `$PS2`, then oslo's own marker.
            // Both of the first two are things somebody asked for by name; the third is only a
            // default, and a default that overrode either would be a setting that does nothing.
            lua.render_with("prompt.continuation", &{
                let mut ctx = prompt::segment_context(last_status, *current, None);
                // The one fact that is different here, and the reason the field exists.
                ctx.continuation = true;
                ctx
            })
            .or_else(|| {
                rc::ps2_if_set(&mut env_struct.lock().unwrap_or_else(|held| held.into_inner()))
            })
            .unwrap_or_else(|| {
                oslo_ui::prompt::continuation_marker(
                    reading.name(),
                    oslo_ui::prompt::block_depth(&buffer),
                )
            })
        };

        // The right prompt is no longer computed here. It is built by the `render` closure
        // below, together with the left one, so that both are rebuilt when the vi mode changes
        // rather than being fixed for the life of the line — see `session::read_line`.
        let _ = helper;

        // What this row *is*, recorded before the editor is entered rather than from inside the
        // highlighter. The highlighter only reaches its own `note_row` on some paths, so anything
        // that redraws the prompt — the vi mode letter, the language toggle — found nothing
        // recorded and silently did nothing at all. Here it is unconditional: a prompt is about to
        // be drawn, and this is what it says.
        {
            oslo_ui::prompt::note_row(reading.name(), oslo_ui::prompt::printed_width(&prompt));
            // For the reader who goes looking: this also used to render the prompt once per vi
            // mode and stash the three results, so a mode change mid-line could redraw without
            // asking whoever owns the prompt to produce it again. Nothing ever read them, so a
            // `prompt.left = { command = … }` was spawning the user's prompt program three extra
            // times per line — 91 ms each here — for a table with no reader. A mode change is
            // redrawn by the generation counter instead, which re-renders once, when it happens.
        }

        // Written before the prompt rather than inside it, for the same reason the right prompt is
        // not concatenated: the line editor measures the prompt string to know where the line
        // starts, and an OSC in there is counted as visible width.
        // The title goes back to the directory now that nothing is running, and the working
        // directory is (re)announced — the first prompt of a session is the only chance to tell
        // the terminal where it started.
        let semantic_prompt = match announce_prompt.take() {
            Some(oslo_ui::marks::PromptKind::Primary) => oslo_ui::marks::prompt_start(),
            Some(oslo_ui::marks::PromptKind::Continuation) => {
                oslo_ui::marks::continuation_prompt_start()
            }
            Some(oslo_ui::marks::PromptKind::Right) => String::new(),
            None => String::new(),
        };
        print!(
            "{}{}{}",
            oslo_ui::marks::working_directory(&crate::startup::repl::cwd()),
            oslo_ui::marks::title(
                &lua.render_with(
                    "prompt.title",
                    &prompt::segment_context(last_status, reading, None)
                )
                .unwrap_or_else(|| { oslo_ui::prompt::tilde(&crate::startup::repl::cwd()) })
            ),
            semantic_prompt
        );
        let _ = std::io::Write::flush(&mut std::io::stdout());

        let split = typed_point.min(typed.len());

        // The editor produces a `raw` line, and everything below is common to however it was
        // read — the mode prefix, history expansion, here-document tracking, and the completeness
        // check that decides whether to ask for another line under `PS2`.
        let raw = {
            let cursor = typed[..split].chars().count();
            let history = history.entries().to_vec();
            let mut assist = super::native::ShellAssist::new(
                history,
                Some(helper),
                // Position the completion menu from the prompt's displayed width.
                oslo_ui::prompt::printed_width(&prompt),
                mode::TOGGLE_KEYS.iter().map(|k| k.to_string()).collect(),
            );
            assist.begin();
            // Handed as a *function* so the editor can rebuild it when the vi mode changes —
            // see `read_line`. It is not called per keystroke; the generation counter decides.
            let mut render = {
                let at_start = buffer.is_empty();
                let nesting = oslo_ui::prompt::block_depth(&buffer);
                let language = *current;
                let reading_now = reading;
                move || -> (String, String) {
                    let _timed = crate::startup::timing::open("prompt-left");
                    let left = if at_start {
                        prompt::primary_prompt(env_struct, lua, last_status, language)
                    } else {
                        // The same three, in the same order, as the measuring copy above: a
                        // `prompt.continuation` written in Lua, then `$PS2`, then oslo's own
                        // marker. They have to agree — this closure draws the row and the other
                        // one is what the width was measured from.
                        lua.render_with("prompt.continuation", &{
                            let mut ctx = prompt::segment_context(last_status, language, None);
                            ctx.continuation = true;
                            ctx
                        })
                        .or_else(|| {
                            rc::ps2_if_set(
                                &mut env_struct.lock().unwrap_or_else(|held| held.into_inner()),
                            )
                        })
                        .unwrap_or_else(|| {
                            oslo_ui::prompt::continuation_marker(reading_now.name(), nesting)
                        })
                    };
                    drop(_timed);
                    let _timed = crate::startup::timing::open("prompt-right");
                    let facts = prompt::segment_context(last_status, reading_now, None);
                    let right = lua
                        .render_with("prompt.right", &facts)
                        .or_else(|| {
                            rc::rps1(
                                &mut env_struct.lock().unwrap_or_else(|held| held.into_inner()),
                            )
                        })
                        .unwrap_or_else(|| {
                            oslo_ui::prompt::render_default_right_prompt(
                                last_status,
                                super::repl::last_command_duration(),
                            )
                        });
                    (left, right)
                }
            };
            // What a command kept detached rebuilds the prompt from — see
            // `oslo_ui::prompt::hold`. Registered per prompt because the status and the language
            // change, and a prompt rebuilt from last cycle's would be wrong about both.
            prompt::keep_alive(env_struct, lua);
            oslo_ui::prompt::hold::showing(true);
            match oslo_ui::edit::session::read_line(&mut render, (&typed, cursor), &mut assist) {
                oslo_ui::edit::session::Outcome::Line(line) => line,
                // Switch and reopen with the same text and cursor, so the line survives the
                // switch — which is what makes a toggle usable mid-command.
                oslo_ui::edit::session::Outcome::ToggleLanguage { text, cursor } => {
                    // **Hidden across the switch.** The editor puts the cursor back at the top of
                    // the block so the next draw can count from there, and then *returns* — which
                    // restores the terminal and makes the cursor visible again. It then sits at
                    // column one, in plain sight, for as long as rendering the other language's
                    // prompt takes. With a prompt built by running another program that is
                    // milliseconds, and it reads as the cursor jumping to the start of the line
                    // and back. The next `read_line` hides it again on entry and shows it on the
                    // way out, so this only covers the gap between the two.
                    print!("\x1b[?25l");
                    let _ = std::io::Write::flush(&mut std::io::stdout());
                    let switched = current.other();
                    // The other half of `pre`/`post-mode-change`: `kind = "language"` here, and
                    // `kind = "vi"` from the editor. One hook for both, because "the mode changed"
                    // is the same question — a handler that cares about only one reads `kind`.
                    let fields = [
                        ("kind", "language"),
                        ("from", current.name()),
                        ("to", switched.name()),
                    ];
                    crate::lua::engine::fire_at_here(
                        crate::lua::api::hooks::at::PRE_MODE_CHANGE,
                        &fields,
                    );
                    *current = switched;
                    crate::lua::engine::fire_at_here(
                        crate::lua::api::hooks::at::POST_MODE_CHANGE,
                        &fields,
                    );
                    reading = switched;
                    typed = text;
                    typed_point = typed
                        .char_indices()
                        .nth(cursor)
                        .map(|(at, _)| at)
                        .unwrap_or(typed.len());
                    continue;
                }
                // A partial multi-line command is abandoned whole, which is what Ctrl-C means
                // when you are three lines into a `for` loop you no longer want.
                //
                // **The prompt above it stands down all the same.** An abandoned line is as
                // finished as one that ran, and leaving the tall prompt there — where every other
                // way out of the editor replaces it — is what made Ctrl-C look like the shell was
                // still waiting for the line you had just cancelled.
                oslo_ui::edit::session::Outcome::Interrupted(abandoned) => {
                    stand_down(lua, last_status, reading, &abandoned);
                    return Input::Interrupted;
                }
                oslo_ui::edit::session::Outcome::Eof => return Input::Eof,
            }
        };

        typed.clear();
        typed_point = 0;

        stand_down(lua, last_status, reading, &raw);

        if buffer.is_empty() {
            if raw.trim().is_empty() {
                return Input::Nothing;
            }
            // The leading space that asks for a line not to be remembered is a property of the
            // line *as typed*, so it has to be read before anything trims or rewrites it.
            secret = history::is_secret(&raw);
        }

        // Only the first line is trimmed. A continuation line goes in exactly as typed, because
        // the body of a here-document is data: `cat <<EOF` followed by an indented line must keep
        // its indentation.
        let line = if buffer.is_empty() {
            raw.trim()
        } else {
            raw.as_str()
        };

        // The prefix is read off the first line of a *shell* command: it runs that one line as Lua
        // without touching the mode the prompt goes back to. A Lua line is never examined — that
        // prompt is a REPL. See `startup::mode`.
        let line = if buffer.is_empty() {
            match mode::classify(*current, line) {
                Line::Normal(text) => text,
                Line::OneOff { mode, text } => {
                    reading = mode;
                    text
                }
            }
        } else {
            line
        };

        // History expansion belongs to shell syntax. In Lua a `!` is `~=`'s other half and a
        // string may hold anything; rewriting a Lua line against the history would corrupt it.
        let expanded = if reading == Mode::Shell && heredoc.expands_history() {
            // **Only this language's lines.** The editor's history holds both, and `!!` expanding
            // to a Lua line at a shell prompt produces something that cannot run — the same
            // crossing the ghost suggestion and the arrow keys were fixed for.
            let previous: Vec<String> = oslo_ui::recall::for_language(reading.name());
            match expand_history(line, &previous) {
                Some(expanded) => expanded,
                None => return Input::Nothing,
            }
        } else {
            line.to_string()
        };
        // Observed on the expanded text, which is what actually goes into the buffer.
        heredoc.observe(&expanded);

        // **A Lua block, once it has started, ends on a blank line.** Python's rule, and it is
        // there for the same reason: after `local function f()` the next complete-looking thing is
        // `end`, and running the moment the parser is satisfied means a block can never be extended
        // — no line after `end` could ever be typed. So a block that has already asked for more
        // keeps asking until an empty line says it is done.
        //
        // Only after the first line. A one-liner runs on Enter exactly as it always has, which is
        // what `1 + 1` at a REPL must do.
        //
        // With `lua_enter = "newline"` that applies from the very first line: Enter always starts
        // another, and only an empty one runs the block. Without it, a one-liner runs on Enter and
        // the rule takes effect once a block has begun.
        let blank_ends_it = reading == Mode::Lua
            && (!buffer.is_empty() || lua_enter == oslo_ui::settings::Enter::Newline);
        if blank_ends_it && expanded.trim().is_empty() {
            return finish(buffer, reading, secret, helper);
        }
        buffer.push_str(&expanded);
        if !blank_ends_it && is_complete(&buffer, reading) {
            // The frecency table is fed from here rather than from the editor's `validate`,
            // because with editor multi-line off (which is what `PS2` costs) `validate` never
            // sees a multi-line command whole — see `OsloHelper::record_command_use`.
            return finish(buffer, reading, secret, helper);
        }
        buffer.push('\n');
        announce_prompt = Some(oslo_ui::marks::PromptKind::Continuation);
    }
}

/// Whether the line about to be read is the body of a here-document.
///
/// The body of a here-document is **data**, and history expansion rewrites a line before it is
/// parsed — so `cat > note <<EOF` followed by a line containing `!` would silently write some
/// earlier command into the file instead of what was typed. bash does not expand there, and
/// [`oslo_ui::syntax::opens_here_document`] exists precisely to tell "unfinished
/// because a document is open" from "unfinished because a quote is open". It had no callers at
/// all, so every heredoc body typed at oslo's prompt was being rewritten (PLAN C8).
///
/// One bit of state, given a name because it is the bit that decides whether a typed line is a
/// command or data, and because the loop that owns it needs a terminal to exercise.
#[derive(Default)]
pub(super) struct HeredocTracker(bool);

impl HeredocTracker {
    /// Whether history expansion may rewrite the next line.
    pub(super) fn expands_history(&self) -> bool {
        !self.0
    }

    /// Take account of a line that has been accepted into the buffer.
    ///
    /// Only ever turns the bit on. A document whose delimiter arrives part-way through an
    /// unfinished command leaves the rest of that command unexpanded too — the safe direction:
    /// the cost is a `!!` that has to be typed out in full, against a `!` inside a heredoc
    /// quietly becoming somebody else's command.
    pub(super) fn observe(&mut self, line: &str) {
        self.0 |= oslo_ui::syntax::opens_here_document(line);
    }
}

/// Whether the parser is satisfied with what has been typed so far.
///
/// A *syntax error* counts as complete: it is the executor's job to report it, and asking for
/// another line would leave the user unable to get the prompt back with no way to see why. The
/// shell's three-way answer comes from the same classifier the editor's validator uses, so the
/// prompt and the loop can never disagree about whether a line is finished.
///
/// Lua answers through its own parser rather than through Lua's `<eof>`-in-the-message trick,
/// which is all the reference implementation's C API can expose. Having our own parser means
/// asking it directly.
/// Hand the accumulated block back as a command.
///
/// The frecency table is fed from here rather than from the editor's `validate`, because the
/// editor never sees a multi-line command whole — it edits one line at a time and this is what
/// joins them.
fn finish(text: String, mode: Mode, secret: bool, helper: &oslo_ui::OsloHelper) -> Input {
    helper.record_command_use(&text);
    Input::Command { text, mode, secret }
}

pub(super) fn is_complete(source: &str, mode: Mode) -> bool {
    match mode {
        Mode::Lua => oslo_luavm::is_complete(source),
        Mode::Shell => !matches!(oslo_ui::syntax::classify(source), InputStatus::Incomplete),
    }
}
