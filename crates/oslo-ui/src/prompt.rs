//! Prompt rendering for the left and right sides of the edited line.
//!
//! A prompt is a Lua function (see `crate::lua::api::prompt`); everything here is either what a
//! prompt function calls, or the built-in prompt used when no Lua one is set.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::theme;

// The row-tracking half now lives in `super::row`; re-exported so callers keep one name for
// "the prompt" rather than having to know which half a function is in.
pub use super::row::{language, note_row, toggle_language};

/// The branch the working directory is on, or a short hash when detached.
///
/// Asks [`crate::git::dir`] where `HEAD` is rather than joining `.git/HEAD`: in a linked worktree
/// and in a submodule `.git` is a *file* holding `gitdir:`, so the join read a path under a regular
/// file and this answered `None` for a repository plainly on a branch.
pub fn git_branch() -> Option<String> {
    let content = fs::read_to_string(crate::git::dir()?.join("HEAD")).ok()?;
    let trimmed = content.trim();
    match trimmed.strip_prefix("ref: refs/heads/") {
        Some(branch) => Some(branch.to_string()),
        // Detached: `HEAD` holds the commit itself, and seven characters is what everyone shows.
        None if trimmed.len() >= 7 => Some(trimmed[..7].to_string()),
        None => None,
    }
}

/// The top of the working tree the current directory is in.
///
/// **Remembered for a moment, because one prompt asks seven times.** The branch, the dirty marker,
/// the ahead/behind counts and the rest each want the root, and each was walking every directory up
/// to `/` to find it — seven identical walks per frame, restatting the same ancestors.
///
/// The window is deliberately short rather than keyed on the directory alone. A cache that lived
/// until the next `cd` would go on insisting there is no repository after a `git init` in the
/// directory you are standing in, which is a wrong prompt for as long as you stay there. Sixty
/// milliseconds is longer than a frame and shorter than anything a person would notice, so the
/// seven walks collapse to one and the answer is never meaningfully stale.
pub fn git_root() -> Option<PathBuf> {
    use std::time::{Duration, Instant};
    const REMEMBERED_FOR: Duration = Duration::from_millis(60);

    let here = env::current_dir().ok()?;
    thread_local! {
        static CACHED: std::cell::RefCell<Option<(PathBuf, Instant, Option<PathBuf>)>> =
            const { std::cell::RefCell::new(None) };
    }
    if let Some(hit) = CACHED.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|(at, when, _)| *at == here && when.elapsed() < REMEMBERED_FOR)
            .map(|(_, _, root)| root.clone())
    }) {
        return hit;
    }
    let root = git_root_of(&here);
    CACHED.with(|slot| {
        *slot.borrow_mut() = Some((here, Instant::now(), root.clone()));
    });
    root
}

/// The top of the working tree `dir` belongs to.
///
/// Split out from [`git_root`] because the tracker has to ask about a directory the shell is not
/// standing in any more: a command is attributed to where it *started*, which a `cd` has already
/// left by the time the loop comes to write it down.
///
/// A `.git` counts only when it is a repository — see [`oslo_base::repo::is_repository`], which
/// also accepts the file a linked worktree has. An empty `~/.git` made all of `$HOME` one
/// workspace, so Up opened on a "workspace" history anywhere under it.
pub fn git_root_of(dir: &Path) -> Option<PathBuf> {
    let mut dir = dir;
    loop {
        if oslo_base::repo::is_repository(&dir.join(".git")) {
            return Some(dir.to_path_buf());
        }
        dir = dir.parent()?;
    }
}

/// A path with `$HOME` written as `~`.
pub fn tilde(path: &str) -> String {
    let home = env::var("HOME").unwrap_or_default();
    if home.is_empty() || !path.starts_with(&home) {
        return path.to_string();
    }
    // Only at a component boundary: `/home/bo` must not become `~` when `$HOME` is `/home/b`.
    let rest = &path[home.len()..];
    if rest.is_empty() || rest.starts_with('/') {
        format!("~{rest}")
    } else {
        path.to_string()
    }
}

/// `~/d/o/t/rush` — every component but the last `keep` cut to its first character.
pub fn shorten(path: &str, keep: usize) -> String {
    let path = tilde(path);
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() <= keep {
        return path;
    }
    let cut = parts.len() - keep;
    parts
        .iter()
        .enumerate()
        .map(|(i, part)| {
            if i >= cut || part.is_empty() {
                return (*part).to_string();
            }
            // A leading dot is kept with the letter after it, or `.config` becomes `.` and every
            // dotted directory looks the same.
            let mut chars = part.chars();
            match chars.next() {
                Some('.') => match chars.next() {
                    Some(second) => format!(".{second}"),
                    None => ".".to_string(),
                },
                Some(first) => first.to_string(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Every language the prompt segment can show.
///
/// The list exists so the prompt's width can be *measured* rather than assumed. Add a language
/// here and the prompt keeps its width automatically; hard-coding a number instead would put the
/// shifting bug back the first time a name of a different length appeared.
pub const LANGUAGES: [&str; 2] = ["sh", "lua"];

/// The width the built-in left prompt is held to, in cells.
///
/// **Measured, not guessed.** Every language is rendered and the widest wins, so switching between
/// them cannot move the line. This matters more than it looks: the line editor is told the
/// prompt's width once, when the line starts, and every piece of arithmetic it does afterwards —
/// where the cursor is, where the hint goes, where a wrapped row breaks — comes off that number.
/// It has no way to be told the prompt changed size. So the prompt must not change size, and the
/// only honest way to promise that is to measure.
pub fn measured_width(last_status: i32) -> usize {
    LANGUAGES
        .iter()
        .map(|language| printed_width(&render_default_left_prompt_unpadded(last_status, language)))
        .max()
        .unwrap_or(0)
}

/// The built-in left prompt, used when no Lua one is set.
/// `user@host | N | sh >`
///
/// Three segments, each answering a question the others cannot: **who and where** you are logged
/// in, **which editing mode** the keyboard is in, and **which language** the line will be read as.
/// The last matters more in oslo than in any other shell — the same characters mean different
/// things in shell and in Lua, and mistaking one for the other is the mistake this prompt exists
/// to prevent.
///
/// The directory and the branch are on the *right*, because both change constantly and would
/// otherwise push the command you are typing further and further across the screen.
pub fn render_default_left_prompt(last_status: i32, language: &str) -> String {
    // Held to one width across every language, and the reason is not cosmetic.
    //
    // The editor measures the prompt string it is handed *once*, when the line starts, and lays
    // out the row, the cursor and the ghost hint against that number for as long as the line
    // lives. It cannot be told the prompt changed size. So a language whose name is a cell wider
    // can be *drawn* — see `highlight_prompt` — only if it occupies the same cells; otherwise
    // every redraw puts the text a cell away from where the editor believes it is.
    //
    // The width is measured across the languages rather than hard-coded, so adding one cannot
    // quietly reintroduce the shifting.
    let mut out = render_default_left_prompt_unpadded(last_status, language);
    for _ in printed_width(&out)..measured_width(last_status) {
        out.push(' ');
    }
    out
}

fn render_default_left_prompt_unpadded(last_status: i32, language: &str) -> String {
    let theme = theme::current();
    let depth = theme::depth();
    let bar = theme.prompt.aside.paint(" | ", depth);

    let mut out = theme.prompt.user.paint(&username(), depth);
    out.push_str(&theme.prompt.aside.paint("@", depth));
    out.push_str(&theme.prompt.host.paint(&hostname(), depth));

    // The mode letter follows the editor's current vi state.
    if let Some(mode) = super::vi::mode() {
        out.push_str(&bar);
        let style = match mode {
            super::vi::Mode::Insert => theme.prompt.ok,
            super::vi::Mode::Normal => theme.prompt.host,
            super::vi::Mode::Replace => theme.prompt.failed,
        };
        out.push_str(&style.paint(mode.name(), depth));
    }

    out.push_str(&bar);
    out.push_str(&theme.prompt.git.paint(language, depth));

    let arrow = if last_status == 0 {
        theme.prompt.ok
    } else {
        theme.prompt.failed
    };
    out.push(' ');
    out.push_str(&arrow.paint(">", depth));
    out.push(' ');
    out
}

/// This machine's name, short — everything before the first dot.
///
/// A fully qualified name is most of the prompt's width on a machine that has one, and the part
/// that identifies it to a person is the first label.
fn hostname() -> String {
    nix::unistd::gethostname()
        .ok()
        .and_then(|h| h.into_string().ok())
        .map(|h| h.split('.').next().unwrap_or(&h).to_string())
        .unwrap_or_else(|| "?".to_string())
}

/// Who you are. `$USER` first, then the password database, because `$USER` is what `su` updates
/// and the uid is what it does not.
fn username() -> String {
    env::var("USER")
        .ok()
        .filter(|u| !u.is_empty())
        .or_else(|| {
            nix::unistd::User::from_uid(nix::unistd::getuid())
                .ok()
                .flatten()
                .map(|u| u.name)
        })
        .unwrap_or_else(|| "?".to_string())
}

/// A duration worth mentioning, or `None`.
///
/// Short commands are the overwhelming majority and saying `3ms` after each of them is noise, so
/// nothing is shown below the threshold. The number that *is* shown is the one you would have
/// wanted before you knew you wanted it — which is the whole argument for a duration in a prompt
/// rather than a `time` you have to remember to type.
pub fn notable_duration(elapsed: Duration) -> Option<String> {
    const WORTH_SAYING: Duration = Duration::from_millis(500);
    if elapsed < WORTH_SAYING {
        return None;
    }
    let secs = elapsed.as_secs_f64();
    Some(if secs < 60.0 {
        format!("{secs:.1}s")
    } else {
        let whole = elapsed.as_secs();
        format!("{}m{:02}s", whole / 60, whole % 60)
    })
}

/// The right prompt oslo draws when a config has not asked for its own.
///
/// It shows what the left prompt cannot: the *number* behind a failing status — the left arrow only
/// goes red — how long the last command took when that is worth saying, and the time. A successful,
/// quick command leaves only the clock, so the line stays quiet until it has something to report.
pub fn render_default_right_prompt(last_status: i32, elapsed: Option<Duration>) -> String {
    let theme = theme::current();
    let depth = theme::depth();
    // The mirror of the left prompt's `>`, opening the right side the way that one closes the
    // left. It takes the same colour, so the pair reads as one frame around the line you type.
    let arrow = if last_status == 0 {
        theme.prompt.ok
    } else {
        theme.prompt.failed
    };
    let mut parts = vec![arrow.paint("❮", depth)];

    // The status number, which the left arrow's colour cannot carry.
    if last_status != 0 {
        parts.push(
            theme
                .prompt
                .failed
                .paint(&format!("({last_status})"), depth),
        );
    }
    if let Some(took) = elapsed.and_then(notable_duration) {
        parts.push(theme.prompt.aside.paint(&took, depth));
    }
    // The branch sits with the directory, because they answer the same question — *where* you are
    // — and separating them meant reading both ends of the line to know it.
    if let Some(branch) = git_branch() {
        parts.push(theme.prompt.git.paint(&format!("({branch})"), depth));
    }
    // The directory lives here rather than on the left: it is what changes on every `cd`, and on
    // the right it does not push the command you are typing further and further across.
    let pwd = tilde(
        &env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "/".to_string()),
    );
    parts.push(theme.prompt.cwd.paint(&pwd, depth));
    parts.join(&theme.prompt.aside.paint("  ", depth))
}

/// Return relative-motion escapes for a right prompt, or empty when it does not fit.
pub fn right_prompt_escape(right: &str, used: usize, cols: usize) -> String {
    if right.is_empty() || cols == 0 {
        return String::new();
    }
    let right_w = printed_width(right);
    // Keep two cells between input and the right prompt plus one unused final cell.
    if used + right_w + 3 > cols {
        return String::new();
    }
    // Relative motion avoids the terminal's shared save/restore cursor slot.
    let gap = cols - right_w - used - 1;
    let home = used;
    if home == 0 {
        format!("\x1b[{gap}C{right}\r")
    } else {
        format!("\x1b[{gap}C{right}\r\x1b[{home}C")
    }
}

/// Count displayed cells while excluding CSI and two-byte escape sequences.
pub fn printed_width(text: &str) -> usize {
    super::dropdown::display_width(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_under_home_is_written_with_a_tilde() {
        // SAFETY: a test-local variable, read only by the functions under test.
        unsafe { env::set_var("HOME", "/home/someone") };
        assert_eq!(tilde("/home/someone/src"), "~/src");
        assert_eq!(tilde("/home/someone"), "~");
        // Only at a component boundary: a different user whose name starts the same is not home.
        assert_eq!(tilde("/home/someoneelse/src"), "/home/someoneelse/src");
        assert_eq!(tilde("/etc"), "/etc");
    }

    #[test]
    fn shortening_keeps_the_tail_and_abbreviates_the_rest() {
        unsafe { env::set_var("HOME", "/home/someone") };
        assert_eq!(shorten("/home/someone/data/code/rush", 1), "~/d/c/rush");
        assert_eq!(shorten("/home/someone/data/code/rush", 2), "~/d/code/rush");
        // A dotted directory keeps the letter after the dot, or every one of them looks alike.
        assert_eq!(shorten("/home/someone/.config/oslo", 1), "~/.c/oslo");
        // Nothing to cut.
        assert_eq!(shorten("/etc", 1), "/etc");
    }

    /// Drawn flush with the right edge, and returning the cursor to where it was so that whatever
    /// is written next — the ghost hint — still lands at the cursor.
    /// `measured_width` really is the widest the prompt gets, whichever language it shows.
    #[test]
    fn the_measured_width_covers_every_language() {
        for status in [0, 1] {
            let widths: Vec<usize> = LANGUAGES
                .iter()
                .map(|l| printed_width(&render_default_left_prompt(status, l)))
                .collect();
            // The prompt is its natural width in each language — `lua` really is a cell wider
            // than `sh`. What keeps the line from being left behind is that the row is redrawn,
            // not that the prompt is padded.
            assert_eq!(*widths.iter().max().unwrap(), measured_width(status));
        }
        // And the padding is on the end: the prompt still reads the same up to the arrow.
        assert!(render_default_left_prompt(0, "sh").contains("sh"));
        assert!(render_default_left_prompt(0, "lua").contains("lua"));
    }

    #[test]
    fn a_right_prompt_is_drawn_flush_right_and_restores_the_cursor() {
        let escape = right_prompt_escape("12:34", 10, 80);
        // From column 11, 64 cells forward, and `12:34` then ends on column 79 — one short of the
        // edge. The final column is deliberately never written: see the gap calculation.
        assert!(escape.starts_with("\x1b[64C"), "{escape:?}");
        assert!(escape.contains("12:34"), "{escape:?}");
        // Back to where it started, via column 1.
        assert!(escape.ends_with("\r\x1b[10C"), "{escape:?}");

        // The last cell written is never the last column, whatever the width.
        for cols in [40usize, 60, 80, 100, 200] {
            let escape = right_prompt_escape("12:34", 10, cols);
            let Some(rest) = escape.strip_prefix("\x1b[") else {
                continue;
            };
            let gap: usize = rest[..rest.find('C').unwrap()].parse().unwrap();
            assert!(
                10 + gap + 5 < cols,
                "right prompt reaches the final column at {cols}: gap {gap}"
            );
        }

        // **No save/restore.** There is one DECSC slot per terminal and it is shared with the
        // dropdown and with any multiplexer hosting the session; a restore could land wherever
        // somebody else's save left it, which is what made the right prompt jump and duplicate.
        assert!(!escape.contains("\x1b7"), "{escape:?}");
        assert!(!escape.contains("\x1b8"), "{escape:?}");

        // A prompt at column 1 needs no move back, only the `\r`.
        let at_start = right_prompt_escape("12:34", 0, 80);
        assert!(at_start.ends_with("\r"), "{at_start:?}");
    }

    /// A prompt is mostly colour escapes, so measuring it as plain text would push the right
    /// prompt left by however many bytes of colour the left one carried.
    #[test]
    fn width_counts_cells_and_not_escapes() {
        assert_eq!(printed_width("abc"), 3);
        assert_eq!(printed_width("\x1b[1;32mabc\x1b[0m"), 3);
        assert_eq!(printed_width("\x1b7\x1b[70Gx\x1b8abc"), 4);
        // And a coloured right prompt is positioned by its cells, not its bytes.
        let plain = right_prompt_escape("12:34", 0, 80);
        let painted = right_prompt_escape("\x1b[90m12:34\x1b[0m", 0, 80);
        // The leading forward move, which is what carries the position.
        let moved = |s: &str| s.split('C').next().map(str::to_string);
        assert_eq!(moved(&plain), moved(&painted));
    }

    /// A right prompt that would collide with what is being typed is worse than none — that
    /// collision is what made the previous attempt look broken.
    #[test]
    fn a_right_prompt_is_dropped_when_it_will_not_fit() {
        // The line has grown to within two columns of it.
        assert_eq!(right_prompt_escape("12:34", 74, 80), "");
        assert_eq!(right_prompt_escape("12:34", 0, 0), "");
        assert_eq!(right_prompt_escape("", 0, 80), "");
        // With room, it is drawn.
        assert!(!right_prompt_escape("12:34", 10, 80).is_empty());
    }
}

#[cfg(test)]
mod right_prompt_tests {
    use super::{notable_duration, render_default_right_prompt};
    use std::time::Duration;

    /// Short commands are the overwhelming majority; saying `3ms` after each is noise.
    #[test]
    fn only_a_duration_worth_saying_is_said() {
        assert_eq!(notable_duration(Duration::from_millis(3)), None);
        assert_eq!(notable_duration(Duration::from_millis(499)), None);
        assert_eq!(
            notable_duration(Duration::from_millis(1300)).as_deref(),
            Some("1.3s")
        );
        assert_eq!(
            notable_duration(Duration::from_secs(75)).as_deref(),
            Some("1m15s")
        );
    }

    /// A quick success leaves only the clock — the line stays quiet until it has something to
    /// report. A failure shows the *number*, which the left prompt's arrow cannot.
    #[test]
    fn the_right_prompt_reports_only_what_is_worth_reporting() {
        let quiet = plain(&render_default_right_prompt(
            0,
            Some(Duration::from_millis(3)),
        ));
        assert!(
            quiet.starts_with('❮'),
            "the mirror of the left arrow: {quiet:?}"
        );
        // No *status* on a success. Checked as "a paren followed by a digit" rather than as any
        // paren, because the git branch lives here too and `(develop)` is not an exit code.
        assert!(
            !quiet
                .match_indices('(')
                .any(|(i, _)| quiet[i + 1..].starts_with(|c: char| c.is_ascii_digit())),
            "no status on a success: {quiet:?}"
        );
        assert!(
            !quiet.contains("ms") && !quiet.contains("0.0s"),
            "no duration for a quick command: {quiet:?}"
        );

        let failed = plain(&render_default_right_prompt(7, None));
        assert!(failed.contains("(7)"), "{failed:?}");

        let slow = plain(&render_default_right_prompt(
            0,
            Some(Duration::from_secs(3)),
        ));
        assert!(slow.contains("3.0s"), "{slow:?}");
    }

    /// Strip the styling, leaving what is on screen.
    fn plain(text: &str) -> String {
        let mut out = String::new();
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }
}

/// How many times something a prompt is built from has changed.
///
/// The prompt used to be a `&str` computed once per line, which meant a vi-mode indicator could
/// never be right: pressing Esc changes the mode but nothing re-renders, so an external prompt
/// stayed frozen at whatever mode the line began in — always insert.
///
/// A counter rather than a callback per input, because the editor needs one cheap question in its
/// redraw loop ("is what I drew still true?") and the answer has to cost nothing on the ordinary
/// path. Typing does not bump it, so a prompt built by running another program is run **once per
/// line**, exactly as before; Esc bumps it, so that one costs a single re-render at human speed.
static PROMPT_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Say that a prompt built now would differ from one built a moment ago.
///
/// Called from the places a prompt's inputs actually change: the vi mode, the language, the working
/// directory, a resize, and a timer or spawn callback that ran while the prompt sat idle. A
/// keystroke and a plain redraw leave it alone.
///
/// **A resize belongs here**, and the note that used to say otherwise was describing a resize that
/// never arrived: `term::input::parse` was discarding the marker as an undecodable byte. A prompt
/// that measures the terminal is wrong at the new width, and redrawing the old string would only
/// re-place a stale one.
pub fn invalidate() {
    PROMPT_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // And that the segments behind it are stale, which a tick never says. See `animation`.
    animation::content_changed();
    // And tell the editor the frame is stale. Two counters rather than one because they answer
    // different questions: this one decides whether the *prompt* is rebuilt, and `pending`'s
    // decides whether the editor redraws at all — a suggestion landing moves the second and must
    // not move the first, or every provider answer would rebuild the prompt as well.
    crate::pending::landed();
}

/// The current generation. Equal to a previous reading means nothing has changed since.
pub fn generation() -> u64 {
    PROMPT_GENERATION.load(std::sync::atomic::Ordering::Relaxed)
}

/// Say that a prompt is being rebuilt somewhere off this thread.
///
/// **What makes an asynchronous prompt visible at all.** The editor waits for a keystroke, and a
/// blocking wait cannot notice a generation bump — so the answer a background run produced sat in
/// the cache until the next key was pressed, which for the last prompt of a session is never. This
/// tells the input wait to come up for air while an answer may still be coming.
///
/// The counter itself is [`crate::pending`], shared with everything else that can answer late.
pub fn refresh_started() {
    crate::pending::started();
}

/// Say that one finished, however it ended.
pub fn refresh_finished() {
    crate::pending::finished();
}

/// Whether an answer may still arrive for anything already on screen.
pub fn refreshing() -> bool {
    crate::pending::outstanding()
}

#[path = "prompt/hold.rs"]
pub mod hold;

#[path = "prompt/animation.rs"]
mod animation;
pub use animation::{animate_in, content_generation, settle as settle_animation, tick_due};

#[path = "prompt/continuation.rs"]
mod continuation;
pub use continuation::{block_depth, continuation_marker};
