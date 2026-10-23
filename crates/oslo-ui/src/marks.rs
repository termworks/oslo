//! Balanced semantic command marks with one process-stable session identity.
//!
//! The terminal owns scrollback presentation; Oslo emits lifecycle boundaries only.

use crate::term::osc133::percent_encode;
use oslo_base::base64::encode as base64;
use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(false);
static VSCODE_RICH_SENT: AtomicBool = AtomicBool::new(false);
static SEMANTIC: Mutex<Option<crate::term::semantic::SemanticSession>> = Mutex::new(None);
pub use crate::term::semantic::PromptKind;

/// Turn marks on for an interactive session that has a terminal to mark.
///
/// Off for every script, `-c`, and test binary: a program reading oslo's output must never find
/// escape sequences the shell invented in it.
pub fn enable(interactive: bool) {
    let on = interactive
        && nix::unistd::isatty(1).unwrap_or(false)
        && std::env::var("TERM").map(|t| t != "dumb").unwrap_or(true);
    ENABLED.store(on, Ordering::Relaxed);
    VSCODE_RICH_SENT.store(false, Ordering::Relaxed);
    let mut semantic = SEMANTIC.lock().unwrap_or_else(|e| e.into_inner());
    let capabilities = if on {
        detected_capabilities()
    } else {
        crate::term::capability::Capabilities::disabled()
    };
    *semantic = Some(crate::term::semantic::SemanticSession::new(
        aid(),
        capabilities,
    ));
}

/// Whether marks are being written: a terminal that can take them, and the feature still on.
///
/// The feature is the runtime half. `enable` answers "is there anything to mark", which is decided
/// once at startup and cannot change; the mask answers "should we", which a config may change per
/// directory — a mux that gets confused by `OSC 133` in one project is the case.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed) && oslo_base::feature::on(oslo_base::feature::at::MARKS)
}

pub(crate) fn session_aid() -> String {
    aid()
}

fn aid() -> String {
    oslo_base::track::session::id()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect()
}

fn detected_capabilities() -> crate::term::capability::Capabilities {
    *crate::term::capability::snapshot()
}

fn capabilities() -> crate::term::capability::Capabilities {
    SEMANTIC
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()
        .map(crate::term::semantic::SemanticSession::capabilities)
        .unwrap_or_else(detected_capabilities)
}

fn semantic(event: crate::term::semantic::SemanticEvent<'_>) -> String {
    if !enabled() {
        return String::new();
    }
    SEMANTIC
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_mut()
        .map(|session| session.emit(event))
        .unwrap_or_default()
}

/// `OSC 7` — where the shell is now.
///
/// A `file://host/path` URL, which is what makes a terminal open a new tab or split in the
/// directory you were in rather than in `$HOME`. Read by kitty, foot, WezTerm, Ghostty and every
/// multiplexer that cares — it is the highest-value sequence a shell emits and costs one write per
/// `cd`.
///
/// The path is percent-encoded, because a URL is not a path: a directory with a space, a `#` or a
/// `%` in it produces a URL that means something else entirely, and directories like that are
/// exactly the ones nobody tests with.
pub fn working_directory(path: &str) -> String {
    if !enabled() {
        return String::new();
    }
    let host = nix::unistd::gethostname()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_default();
    let osc7 = format!("\x1b]7;file://{host}{}\x1b\\", percent_encode(path));
    format!(
        "{osc7}{}",
        semantic(crate::term::semantic::SemanticEvent::CwdChanged(path))
    )
}

/// `OSC 0` — the window and tab title.
///
/// Set to the command while one is running and to the directory when the shell is idle, which is
/// fish's behaviour: a row of tabs then says what each one is *doing*, not merely where it is.
///
/// A multiplexer that names its own panes will fight this, and the last writer wins. That is why
/// it is a setting rather than unconditional.
pub fn title(text: &str) -> String {
    if !enabled() {
        return String::new();
    }
    // Control characters would end the sequence early and leave the rest as text on screen.
    let clean: String = text.chars().filter(|c| !c.is_control()).collect();
    format!("\x1b]0;{clean}\x1b\\")
}

/// `OSC 52` — put `text` on the system clipboard.
///
/// The one way a shell can reach the clipboard **through the terminal**, which means it works over
/// SSH where `xsel` and `wl-copy` cannot: the bytes travel up the same connection the session
/// does, and the terminal at the far end does the pasting.
///
/// Write-only by design. Reading back is supported almost nowhere, and a terminal that does
/// support it usually asks the user first — so a `paste` built on this would work on one machine
/// in ten and hang on the rest.
///
/// Not gated on [`enabled`]: unlike a title or a prompt mark, this is something the user asked for
/// explicitly, and refusing it because stdout is a pipe would be surprising.
pub fn clipboard(text: &str) -> String {
    format!("\x1b]52;c;{}\x1b\\", base64(text.as_bytes()))
}

/// `OSC 8` — `text`, clickable, pointing at `url`.
///
/// Only ever wrapped around text oslo itself prints: a path in a diagnostic, a file in a listing.
/// It cannot make `ls` or `cargo` output clickable, because oslo never sees those bytes.
///
/// A terminal that does not know `OSC 8` shows `text` and drops the rest, so this is safe to emit
/// unconditionally — which is why it is gated on [`enabled`] only to keep it out of scripts.
pub fn hyperlink(url: &str, text: &str) -> String {
    if !enabled() {
        return text.to_string();
    }
    format!("\x1b]8;;{url}\x1b\\{text}\x1b]8;;\x1b\\")
}

/// A path, printed clickable when the terminal is listening and plain when it is not.
///
/// The shape a diagnostic wants: `eprintln!("oslo: {}: {e}", marks::path(p))` reads the same as
/// before and gains a link for free. A config file named in an error is exactly the thing you want
/// to open next.
pub fn path(path: &str) -> String {
    if !enabled() {
        return path.to_string();
    }
    hyperlink(&file_url(path), path)
}

/// A `file://` URL for a path on this machine, for [`hyperlink`].
pub fn file_url(path: &str) -> String {
    let host = nix::unistd::gethostname()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_default();
    format!("file://{host}{}", percent_encode(path))
}

/// A new primary prompt and interaction begin here.
///
/// Returns only what belongs outside the block — vscode's one-time rich-detection property, which
/// names the session rather than a row. The `A` mark is drawn with the block by [`block_marks`].
pub fn prompt_start() -> String {
    let mut outside = String::new();
    if capabilities().semantic == crate::term::capability::SemanticProtocol::Vscode633
        && !VSCODE_RICH_SENT.swap(true, Ordering::Relaxed)
    {
        outside.push_str(
            &crate::term::vscode::property("HasRichCommandDetection", "True").unwrap_or_default(),
        );
    }
    let _ = semantic(crate::term::semantic::SemanticEvent::PromptStart(
        PromptKind::Primary,
    ));
    outside
}

#[must_use]
pub struct Interaction {
    active: bool,
}

impl Interaction {
    pub fn begin() -> Self {
        let _ = semantic(crate::term::semantic::SemanticEvent::InteractionStart);
        Self { active: true }
    }

    pub fn abort(&mut self) -> String {
        self.active = false;
        cancel_interaction()
    }

    pub fn finish(&mut self, status: i32) -> String {
        self.active = false;
        command_end(status)
    }
}

impl Drop for Interaction {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let mark = cancel_interaction();
        if !mark.is_empty() {
            let mut out = std::io::stdout();
            let _ = out.write_all(mark.as_bytes());
            let _ = out.flush();
        }
    }
}

/// A continuation prompt within the current interaction.
///
/// Advances the lifecycle; the `A ; k=s` mark is drawn with the block, as [`prompt_start`]'s is.
pub fn continuation_prompt_start() -> String {
    let _ = semantic(crate::term::semantic::SemanticEvent::PromptStart(
        PromptKind::Continuation,
    ));
    String::new()
}

/// The prompt ends here and what you type begins.
///
/// Advances the lifecycle; the `B` mark is drawn with the block, as [`prompt_start`]'s is.
pub fn input_start() -> String {
    let _ = semantic(crate::term::semantic::SemanticEvent::InputStart);
    String::new()
}

/// The `A` and `B` marks for the block being edited, to be drawn with it.
///
/// A mark belongs to the row it was written on, and a block is redrawn by erasing its rows and
/// writing them again — so the frame carries these and every repaint puts them back. Reads the
/// lifecycle without advancing it. See `docs/features/terminal-integration.md`.
pub fn block_marks() -> crate::term::semantic::BlockMarks {
    if !enabled() {
        return crate::term::semantic::BlockMarks::default();
    }
    SEMANTIC
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()
        .map(crate::term::semantic::SemanticSession::block_marks)
        .unwrap_or_default()
}

/// `OSC 133 ; C` — everything after this is the command's output.
///
/// Kitty accepts the final command as URL-percent-encoded UTF-8. Other terminals retain the
/// portable classic boundary, and private input passes `None` so it publishes no command text.
pub fn output_start(command: Option<&str>) -> String {
    let mut mark = semantic(crate::term::semantic::SemanticEvent::CommandStart {
        command_line: command.unwrap_or_default(),
        visibility: if command.is_some() {
            crate::term::semantic::CommandVisibility::Publish
        } else {
            crate::term::semantic::CommandVisibility::Omit
        },
    });
    if !mark.is_empty()
        && let Some(progress) = crate::term::metadata::progress(
            &capabilities(),
            crate::term::metadata::Progress::Indeterminate,
        )
    {
        mark.push_str(&progress);
    }
    mark
}

/// `OSC 133 ; D` — the command has finished, with this status.
pub fn command_end(status: i32) -> String {
    let mut mark = semantic(crate::term::semantic::SemanticEvent::CommandEnd { status });
    if !mark.is_empty()
        && let Some(progress) = crate::term::metadata::progress(
            &capabilities(),
            crate::term::metadata::Progress::Complete,
        )
    {
        mark.push_str(&progress);
    }
    mark
}

/// Close input that never became a command, without inventing an execution status or `C` mark.
pub fn cancel_interaction() -> String {
    let mut mark = semantic(crate::term::semantic::SemanticEvent::InteractionAbort);
    if !mark.is_empty()
        && let Some(progress) =
            crate::term::metadata::progress(&capabilities(), crate::term::metadata::Progress::Clear)
    {
        mark.push_str(&progress);
    }
    mark
}

#[cfg(test)]
mod tests {
    /// `ENABLED` is one process-wide flag and these tests set it both ways. Run in parallel they
    /// flip it under each other, which showed up as a mark test failing at random — the same
    /// hazard that has bitten the settings and theme tests. One at a time.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    use super::*;

    fn test_on(capabilities: crate::term::capability::Capabilities) {
        ENABLED.store(true, Ordering::Relaxed);
        VSCODE_RICH_SENT.store(false, Ordering::Relaxed);
        *SEMANTIC.lock().unwrap_or_else(|error| error.into_inner()) = Some(
            crate::term::semantic::SemanticSession::new(aid(), capabilities),
        );
    }

    fn test_off() {
        ENABLED.store(false, Ordering::Relaxed);
        *SEMANTIC.lock().unwrap_or_else(|error| error.into_inner()) = None;
    }

    /// Marks are off unless a person is looking at a terminal. A script's output must never carry
    /// escape sequences the shell invented.
    #[test]
    fn nothing_is_emitted_without_a_terminal() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        enable(false);
        let mut interaction = Interaction::begin();
        assert_eq!(prompt_start(), "");
        assert_eq!(output_start(Some("echo hi")), "");
        assert_eq!(interaction.finish(0), "");
    }

    #[test]
    fn interactions_are_balanced_and_keep_the_session_id() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        test_on(crate::term::capability::Capabilities::portable());

        let mut interaction = Interaction::begin();
        prompt_start();
        // `A` and `B` are drawn with the block, so they are read there rather than returned.
        let block = block_marks();
        input_start();
        let c = output_start(None);
        let d = interaction.finish(3);
        let id = aid();
        assert_eq!(block.prompt, format!("\x1b]133;A;aid={id}\x1b\\"));
        assert_eq!(block.input, format!("\x1b]133;B;aid={id}\x1b\\"));
        assert_eq!(c, format!("\x1b]133;C;aid={id}\x1b\\"));
        assert_eq!(d, format!("\x1b]133;D;3;aid={id}\x1b\\"));

        let mut next_interaction = Interaction::begin();
        prompt_start();
        let next = block_marks().prompt;
        assert!(next.contains(&format!("aid={id}")), "{next:?}");
        let _ = next_interaction.abort();

        test_off();
    }

    #[test]
    fn continuation_prompts_do_not_open_another_interaction() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        test_on(
            crate::term::capability::Capabilities::portable().with_verified(
                crate::term::capability::Verified {
                    semantic_secondary: true,
                    ..crate::term::capability::Verified::default()
                },
            ),
        );

        let mut interaction = Interaction::begin();
        prompt_start();
        assert!(!block_marks().prompt.is_empty());
        input_start();
        assert!(!block_marks().input.is_empty());
        continuation_prompt_start();
        assert!(block_marks().prompt.contains(";A;k=s;"));
        input_start();
        assert!(!output_start(None).is_empty());
        assert!(!interaction.finish(0).is_empty());
        test_off();
    }

    #[test]
    fn cancelled_input_closes_without_an_output_or_status() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        test_on(crate::term::capability::Capabilities::portable());

        let mut interaction = Interaction::begin();
        prompt_start();
        assert!(!block_marks().prompt.is_empty());
        input_start();
        assert!(!block_marks().input.is_empty());
        let closed = interaction.abort();
        assert!(closed.contains(";D;aid="), "{closed:?}");
        assert!(!closed.contains(";C;"), "{closed:?}");
        assert_eq!(command_end(130), "");
        test_off();
    }

    /// An illegal transition changes nothing — the interaction that follows is still well formed.
    ///
    /// Asserted through what the lifecycle produces rather than through the call's return value:
    /// `A` and `B` are drawn with the block now, so a refused `PromptStart` and an accepted one
    /// both hand back an empty string. `SemanticSession::try_emit` is where the refusal itself is
    /// checked — see `duplicates_and_out_of_order_events_are_silent` in `term::semantic`.
    #[test]
    fn illegal_duplicate_transitions_emit_nothing() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        test_on(crate::term::capability::Capabilities::portable());

        let mut interaction = Interaction::begin();
        // Out of order, before any prompt: no block to mark and no output to open.
        assert_eq!(block_marks().prompt, "");
        assert_eq!(output_start(None), "");

        prompt_start();
        prompt_start();
        assert!(block_marks().prompt.contains(";A;"), "{:?}", block_marks());

        input_start();
        input_start();
        assert!(block_marks().input.contains(";B;"), "{:?}", block_marks());

        // Still in `Input` despite the duplicates, so the interaction closes without a command.
        assert!(!interaction.abort().is_empty());
        test_off();
    }

    #[test]
    fn kitty_command_metadata_is_utf8_url_encoded_and_private_input_is_omitted() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let capabilities = crate::term::capability::Capabilities::portable().with_verified(
            crate::term::capability::Verified {
                semantic_cmdline_url: true,
                ..crate::term::capability::Verified::default()
            },
        );
        test_on(capabilities);

        let mut interaction = Interaction::begin();
        let _ = prompt_start();
        let _ = input_start();
        let mark = output_start(Some("echo café; 100%\tnext\nline"));
        assert!(
            mark.contains("cmdline_url=echo%20caf%C3%A9%3B%20100%25%09next%0Aline"),
            "{mark:?}"
        );
        let _ = interaction.finish(0);

        let mut interaction = Interaction::begin();
        let _ = prompt_start();
        let _ = input_start();
        let private = output_start(None);
        assert!(!private.contains("cmdline"), "{private:?}");
        let _ = interaction.finish(0);
        test_off();
    }

    /// A URL is not a path. A directory with a space or a `#` in it makes a URL that means
    /// something else, and those are exactly the directories nobody tests with.
    #[test]
    fn a_working_directory_is_percent_encoded() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        test_on(crate::term::capability::Capabilities::portable());
        let osc = working_directory("/home/u/my dir/a#b");
        assert!(osc.contains("/home/u/my%20dir/a%23b"), "{osc:?}");
        // Slashes stay literal, or the path stops being a path.
        assert!(!osc.contains("%2F"), "{osc:?}");
        assert!(osc.starts_with("\x1b]7;file://"), "{osc:?}");
        assert!(osc.ends_with("\x1b\\"), "{osc:?}");
        test_off();
    }

    #[test]
    fn the_clipboard_sequence_carries_the_encoded_text() {
        assert_eq!(clipboard("foo"), "\x1b]52;c;Zm9v\x1b\\");
        // Not gated on a terminal: copying is something the user asked for by name.
        assert!(!clipboard("x").is_empty());
    }

    /// A terminal that does not know `OSC 8` shows the text and drops the rest, so the text has to
    /// be there in full either way.
    #[test]
    fn a_hyperlink_wraps_its_text_without_altering_it() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        test_on(crate::term::capability::Capabilities::portable());
        let link = hyperlink("file://h/etc/foo", "/etc/foo");
        assert!(
            link.starts_with("\x1b]8;;file://h/etc/foo\x1b\\"),
            "{link:?}"
        );
        assert!(link.contains("/etc/foo"));
        assert!(link.ends_with("\x1b]8;;\x1b\\"), "{link:?}");
        test_off();

        // With marks off — a script — it is the bare text and nothing else.
        assert_eq!(hyperlink("file://h/x", "/x"), "/x");
    }

    /// A control character in a title would end the sequence early and spill the rest onto the
    /// screen as text.
    #[test]
    fn a_title_carries_no_control_characters() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        test_on(crate::term::capability::Capabilities::portable());
        let osc = title("build\x07 done\nnow");
        assert_eq!(osc, "\x1b]0;build donenow\x1b\\");
        test_off();
    }

    /// Every mark is a complete OSC: introducer, payload, terminator. A half-written one would be
    /// swallowed along with whatever text followed it.
    #[test]
    fn every_mark_is_a_terminated_osc() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        test_on(crate::term::capability::Capabilities::portable());
        let mut interaction = Interaction::begin();
        prompt_start();
        let block = block_marks();
        input_start();
        let marks = [
            block.prompt,
            block.input,
            output_start(None),
            interaction.finish(0),
        ];
        for mark in marks {
            assert!(mark.starts_with("\x1b]133;"), "{mark:?}");
            assert!(mark.ends_with("\x1b\\"), "{mark:?}");
            assert!(
                !mark.contains('\n'),
                "a mark must not move the cursor: {mark:?}"
            );
        }
        test_off();
    }
}
