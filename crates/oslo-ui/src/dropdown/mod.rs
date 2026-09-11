//! The completion dropdown: candidates, layout, rendering, and the raw-mode selection loop.
//!
//! Everything here is measured in *terminal cells*, not bytes and not `char`s. A box drawn from
//! byte lengths overflows as soon as a label carries an emoji icon, and a box drawn without
//! asking how wide the terminal is overflows as soon as the cwd is deep. An overflowing row
//! wraps; a wrapped row makes the "move the cursor back up" sequence count too few rows; and the
//! redraw then paints over the prompt and the scrollback above it (R9.4).
//!
//! The three pieces below keep that from happening:
//!
//! * `width` — cells per string, wraps per row, and the terminal's own size via `TIOCGWINSZ`;
//! * `layout` — column widths clamped so no row can exceed the screen;
//! * `render` — the escapes, plus the physical row count to walk back up.
//!
//! [`render_vertical_dropdown_at_width`] takes the column count as an argument rather than
//! querying it, so the whole layout is testable at 80 columns with no terminal attached.

mod columns;
mod layout;
mod render;
pub mod width;

pub use columns::{
    Facts, Provider, builtin_columns, columns_for, facts_for, human_age, human_mode, human_size,
    set_provider,
};
pub use layout::{DropdownLayout, compute_layout};
pub use render::{
    CEILING_ROWS, DEFAULT_ROWS, render_vertical_dropdown, render_vertical_dropdown_at_width,
};
pub use width::{
    FALLBACK_COLS, display_width, pad_to_width, physical_rows, terminal_cols, truncate_to_width,
    visible_len,
};

use crate::term::{InputEvent, Key, Keys};

#[derive(Debug, Clone)]
pub struct CompletionCandidate {
    pub display: String,
    pub replacement: String,
    pub description: Option<String>,
    pub kind: Option<String>,
    /// The file this candidate names, when it names one.
    ///
    /// Carried rather than reconstructed: `replacement` is *quoted*, so working back to a path
    /// would mean unquoting it — and getting that wrong turns a `stat` of `My File.txt` into a
    /// `stat` of `My\ File.txt`, which simply is not there. The completer already knows the path
    /// it built, so it hands it over.
    pub path: Option<String>,
    /// A fact the completer already knew and the renderer would have to guess.
    ///
    /// What an alias expands to, mainly. It is captured here rather than looked up at render time
    /// because the renderer has no environment: it is handed a list and a width, and reaching back
    /// into the shell from inside a draw would put a lock on the frame path.
    pub detail: Option<String>,
}

impl CompletionCandidate {
    pub fn new(display: String, replacement: String, description: Option<String>) -> Self {
        Self {
            display,
            replacement,
            description,
            kind: None,
            path: None,
            detail: None,
        }
    }

    /// The word the kind badge spells, or `None` for a candidate with no kind.
    ///
    /// Replaces an emoji icon per kind. Two reasons it went: an emoji is two cells wide in some
    /// terminals and one in others, so a column of them cannot be laid out reliably; and a glyph
    /// has to be learned, where ` builtin ` is already the word. The badge is drawn as a coloured
    /// pill, which is what carries the "this is a kind" reading that the icon was there for.
    pub fn badge(&self) -> Option<&str> {
        match self.kind.as_deref() {
            Some("dir") | Some("directory") => Some("dir"),
            Some("file") => Some("file"),
            Some("builtin") => Some("builtin"),
            Some("command") => Some("command"),
            Some("variable") => Some("variable"),
            Some("history") => Some("history"),
            Some("alias") => Some("alias"),
            Some("flag") | Some("option") => Some("option"),
            Some("subcommand") => Some("subcmd"),
            Some("function") => Some("func"),
            // A kind nothing has claimed is still a kind; showing it is how the next one gets
            // noticed rather than silently drawn as blank.
            Some(other) if !other.is_empty() => Some(other),
            _ => None,
        }
    }
}

pub struct DropdownMenu {
    pub candidates: Vec<CompletionCandidate>,
    pub selected_index: usize,
    pub max_visible: usize,
    pub indent_cols: usize,
}

impl DropdownMenu {
    pub fn new(candidates: Vec<CompletionCandidate>, indent_cols: usize) -> Self {
        Self {
            candidates,
            selected_index: 0,
            max_visible: DEFAULT_ROWS,
            indent_cols,
        }
    }

    pub fn select_interactive(
        candidates: Vec<CompletionCandidate>,
        indent_cols: usize,
        typed: &str,
        keys: &mut Keys,
    ) -> Option<CompletionCandidate> {
        if candidates.is_empty() {
            return None;
        }
        if candidates.len() == 1 {
            return Some(candidates[0].clone());
        }

        let mut menu = Self::new(candidates, indent_cols);
        // `oslo.completion.max_rows` *sets* the height rather than capping it. Taking the minimum
        // with the built-in default meant the setting could only ever make the menu smaller:
        // asking for twenty rows silently got you eight.
        //
        // Bounded by the terminal as well, with rows left for the prompt and the line being typed
        // — a menu that filled the screen would have nowhere to appear below.
        let wanted = crate::settings::current().completion.max_rows;
        menu.max_visible = wanted.min(width::terminal_rows().saturating_sub(3)).max(1);

        // Rows currently reserved below the prompt. See the comment in the loop.
        let mut reserved = 0usize;

        let selected = loop {
            // The width is re-queried every frame: the terminal can be resized while the menu is
            // open, and a stale width is exactly the state that overwrites the prompt.
            let (rendered, num_lines) = render_vertical_dropdown_at_width(
                &menu.candidates,
                menu.selected_index,
                menu.max_visible,
                menu.indent_cols,
                terminal_cols(),
                typed,
            );
            // **Reserve the rows before drawing into them.** Drawing first and walking back up
            // afterwards is what ate the prompt: near the bottom of the screen the newlines make
            // the terminal *scroll*, so the cursor ends up fewer rows down than were printed, and
            // moving up by the full count lands above the prompt — where the erase then removes
            // it. Pressing the arrow keys made it worse each frame.
            //
            // Printing the newlines first makes any scroll happen while the cursor is still ours
            // to account for: after moving back up the same number, the prompt is wherever the
            // scroll left it, and every frame after this one is a redraw in place.
            // Where the cursor is: past the prompt and whatever of the word has been typed. Coming
            // back is a *move* rather than a terminal restore, so the column has to be known.
            let column = indent_cols + display_width(typed);
            write_fd(keys.fd(), reserve_rows(num_lines, &mut reserved).as_bytes());
            write_fd(
                keys.fd(),
                draw_below(&rendered, num_lines, column).as_bytes(),
            );

            let Some(event) = keys.read_event() else {
                break None;
            };
            match event {
                InputEvent::Key(Key::Accept | Key::Char(' ')) => {
                    break Some(menu.candidates[menu.selected_index].clone());
                }
                InputEvent::Key(Key::ToggleScope | Key::Down) => {
                    menu.selected_index = (menu.selected_index + 1) % menu.candidates.len();
                }
                InputEvent::Key(Key::BackTab | Key::Up) => {
                    menu.selected_index = menu
                        .selected_index
                        .checked_sub(1)
                        .unwrap_or(menu.candidates.len() - 1);
                }
                InputEvent::Key(Key::Cancel) => break None,
                // **A resize closes the menu rather than re-rendering it.**
                //
                // Re-rendering was only half the frame: the menu came back at the new width, but
                // the prompt and the line being typed sit *above* it and are the editor's to draw
                // — this loop never reaches them. At any width that reflows, the stale line rewrapped
                // off-screen and left a screen with a menu on it and nowhere to type, until the next
                // keypress happened to repaint.
                //
                // So the event is handed back and the menu gives up its rows. The editor's own
                // resize path invalidates the prompt and repaints the line, which is the half that
                // was missing; Tab reopens the menu at the new width. Losing an open menu to a
                // resize is a smaller surprise than losing the prompt.
                resize @ (InputEvent::Resized | InputEvent::Key(Key::Resized)) => {
                    keys.unread_event(resize);
                    break None;
                }
                other => {
                    keys.unread_event(other);
                    break None;
                }
            }
        };

        // Erase what was drawn, from one row below the prompt to the end of the screen. `\x1b[B`
        // rather than a newline: the reserved rows already exist, and a newline at the bottom of
        // the screen would scroll again.
        write_fd(
            keys.fd(),
            erase_below(reserved, indent_cols + display_width(typed)).as_bytes(),
        );
        selected
    }
}

/// One dim line under the word, instead of a menu, until the next key — which is then handled as
/// it would have been.
///
/// For a Tab that found nothing *and knows why*: a remote listing that failed. Without it the menu
/// stayed shut and a refused key, a wrong name and a slow link all looked the same.
pub fn notice(text: &str, indent_cols: usize, typed: &str, keys: &mut Keys) {
    let room = terminal_cols().saturating_sub(indent_cols + 1).max(1);
    let shown: String = text.chars().take(room).collect();
    let row = format!("\r\n{}\x1b[2m{shown}\x1b[0m\x1b[K", " ".repeat(indent_cols));
    let column = indent_cols + display_width(typed);
    let mut reserved = 0usize;
    write_fd(keys.fd(), reserve_rows(1, &mut reserved).as_bytes());
    write_fd(keys.fd(), draw_below(&row, 1, column).as_bytes());
    if let Some(event) = keys.read_event() {
        keys.unread_event(event);
    }
    write_fd(keys.fd(), erase_below(reserved, column).as_bytes());
}

fn write_fd(fd: i32, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        // SAFETY: `bytes` is live and the editor owns a writable terminal descriptor.
        let written = unsafe { nix::libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if written > 0 {
            bytes = &bytes[written as usize..];
        } else if written < 0
            && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
        {
            continue;
        } else {
            break;
        }
    }
}

/// Make room below the prompt for `wanted` rows, given how many are already reserved.
///
/// **This is the fix for the menu eating the prompt.** The old loop drew first and walked back up
/// afterwards; near the bottom of the screen the newlines make the terminal *scroll*, so the
/// cursor ends up fewer rows down than were printed and moving up by the full count lands above
/// the prompt — where the erase then removes it. Holding an arrow key made it worse every frame.
///
/// Printing the newlines *first* makes any scroll happen while the cursor is still ours to
/// account for: moving back up the same number returns to the prompt wherever the scroll left it,
/// and every frame after that is a redraw into rows that already exist.
fn reserve_rows(wanted: usize, reserved: &mut usize) -> String {
    if wanted <= *reserved {
        return String::new();
    }
    let extra = wanted - *reserved;
    *reserved = wanted;
    format!("{}\x1b[{extra}A", "\n".repeat(extra))
}

/// Draw `rendered` below the cursor and come back to it. No arithmetic to get wrong.
///
/// The `\x1b[J` at the end is not decoration. Each row clears its own tail with `\x1b[K`, which
/// handles a row getting *shorter*, but nothing clears the rows below the last one drawn — and the
/// reserved height only ever grows, so a frame is never given fewer rows than the tallest one so
/// far. Page through a listing whose last page is shorter than the one before it and the previous
/// page's leftover rows stay on screen underneath, looking like candidates that are still there.
///
/// Erasing to the end of the screen from the last drawn row is safe because everything below it
/// belongs to this menu: the rows were reserved by `reserve_rows`, and `erase_below` clears the
/// same region when the menu closes.
/// `rows` is how many physical rows `rendered` occupies, and `column` the cursor's column when the
/// menu opened — both needed because coming back is done by *moving*, not by restoring.
///
/// **No `\x1b7`/`\x1b8`.** There is one save slot per terminal and it is shared with everything
/// drawing on it, including whatever multiplexer is hosting the session. A restore then lands
/// wherever somebody else's save left the cursor, which is why opening this menu could throw the
/// prompt back to column 1. Relative motion has no shared state to lose.
fn draw_below(rendered: &str, rows: usize, column: usize) -> String {
    // Every row begins with `\r\n`, so after drawing the cursor sits at column 1, `rows` below
    // where it started.
    let mut out = format!("{rendered}\x1b[J");
    if rows > 0 {
        out.push_str(&format!("\x1b[{rows}A"));
    }
    out.push('\r');
    if column > 0 {
        out.push_str(&format!("\x1b[{column}C"));
    }
    out
}

/// Erase everything from one row below the prompt to the end of the screen.
///
/// `\x1b[B` rather than a newline: the reserved rows already exist, and a newline at the bottom of
/// the screen would scroll again — which is the whole class of bug this avoids.
fn erase_below(reserved: usize, column: usize) -> String {
    if reserved == 0 {
        return String::new();
    }
    // Down one, clear to the end of the screen, back up, and back along to where the cursor was.
    // Moving rather than restoring, for the reason given on `draw_below`.
    let mut out = String::from("\x1b[B\r\x1b[J\x1b[A\r");
    if column > 0 {
        out.push_str(&format!("\x1b[{column}C"));
    }
    out
}

#[cfg(test)]
mod frame_tests {
    use super::*;
    use std::os::fd::AsRawFd;

    fn candidate(name: &str) -> CompletionCandidate {
        CompletionCandidate::new(name.to_string(), name.to_string(), None)
    }

    /// The rows are reserved before anything is drawn into them, and only the shortfall is added.
    #[test]
    fn rows_are_reserved_once_and_only_grown() {
        let mut reserved = 0;
        // Four rows: print four newlines, then come back up four.
        assert_eq!(reserve_rows(4, &mut reserved), "\n\n\n\n\x1b[4A");
        assert_eq!(reserved, 4);
        // The same height again asks for nothing: the rows are already there.
        assert_eq!(reserve_rows(4, &mut reserved), "");
        // A taller menu adds only the difference.
        assert_eq!(reserve_rows(6, &mut reserved), "\n\n\x1b[2A");
        assert_eq!(reserved, 6);
        // A shorter one keeps what it has rather than giving rows back, which would scroll again.
        assert_eq!(reserve_rows(2, &mut reserved), "");
        assert_eq!(reserved, 6);
    }

    /// A frame comes back by *moving*, never by restoring.
    ///
    /// `\x1b7`/`\x1b8` is one slot per terminal, shared with the right prompt and with any
    /// multiplexer hosting the session. Opening this menu used to throw the prompt back to column
    /// 1, because the restore landed wherever somebody else's save had left the cursor.
    #[test]
    fn a_frame_moves_back_rather_than_restoring() {
        let frame = draw_below("\r\nROWS", 1, 19);
        assert!(frame.contains("ROWS"));
        assert!(!frame.contains("\x1b7"), "a save survived: {frame:?}");
        assert!(!frame.contains("\x1b8"), "a restore survived: {frame:?}");
        // Up over the rows it drew, then back along to the cursor's column.
        assert!(frame.contains("\x1b[1A"), "{frame:?}");
        assert!(frame.ends_with("\r\x1b[19C"), "{frame:?}");

        // At column 1 there is nothing to move along.
        assert!(draw_below("\r\nROWS", 1, 0).ends_with('\r'));
    }

    /// A frame erases below its last row, or a shorter page leaves the previous one's tail on
    /// screen. The reserved height only grows, so this is the *only* thing that clears them.
    #[test]
    fn a_shorter_frame_erases_what_the_taller_one_left() {
        let tall = draw_below("r1\r\nr2\r\nr3\r\nr4", 4, 19);
        let short = draw_below("r1\r\nr2", 2, 19);
        for frame in [&tall, &short] {
            assert!(frame.contains("\x1b[J"), "no erase-below: {frame:?}");
            // After the content, not before it, or it would wipe the rows just drawn.
            let erase = frame.find("\x1b[J").expect("present");
            let last_row = frame.rfind("r2").expect("present");
            assert!(erase > last_row, "erase came before the content: {frame:?}");
        }
        // Each comes back up by the rows it actually drew.
        assert!(tall.contains("\x1b[4A"), "{tall:?}");
        assert!(short.contains("\x1b[2A"), "{short:?}");
    }

    #[test]
    fn erasing_moves_down_a_row_rather_than_printing_a_newline() {
        let erase = erase_below(3, 19);
        assert!(erase.contains("\x1b[B"), "{erase:?}");
        assert!(!erase.contains('\n'), "a newline would scroll: {erase:?}");
        assert!(erase.contains("\x1b[J"), "{erase:?}");
        // Back up to the prompt's row and along to where the cursor was — no restore.
        assert!(erase.ends_with("\x1b[A\r\x1b[19C"), "{erase:?}");
        assert!(!erase.contains("\x1b7"), "{erase:?}");
        // Nothing was drawn, so there is nothing to erase.
        assert_eq!(erase_below(0, 19), "");
    }

    #[test]
    fn selection_reads_complete_events_from_the_shared_reader() {
        for pending in [b"\x1b[B\r".as_slice(), b"\x1b[57353u\r".as_slice()] {
            let (reader, _writer) = nix::unistd::pipe().expect("pipe");
            let mut keys = Keys::editor(reader.as_raw_fd(), pending.to_vec(), false);
            let selected = DropdownMenu::select_interactive(
                vec![candidate("one"), candidate("two")],
                0,
                "",
                &mut keys,
            )
            .expect("selection");
            assert_eq!(selected.replacement, "two");
        }
    }

    #[test]
    fn dismissing_events_are_replayed_exactly_once() {
        for (pending, expected) in [
            (b"x".to_vec(), InputEvent::Key(Key::Char('x'))),
            (b"\x03".to_vec(), InputEvent::Key(Key::Abort)),
            (
                b"\x1b[200~echo hi\x1b[201~".to_vec(),
                InputEvent::Paste("echo hi".to_string()),
            ),
            (b"\x1b[I".to_vec(), InputEvent::Focus(true)),
        ] {
            let (reader, _writer) = nix::unistd::pipe().expect("pipe");
            let mut keys = Keys::editor(reader.as_raw_fd(), pending, false);
            assert!(
                DropdownMenu::select_interactive(
                    vec![candidate("one"), candidate("two")],
                    0,
                    "",
                    &mut keys,
                )
                .is_none()
            );
            assert_eq!(keys.read_event(), Some(expected));
        }
    }

    /// A resize closes the menu and hands the event on, so the editor can repaint the line the
    /// menu cannot reach. The mouse click queued behind it must still survive.
    #[test]
    fn resize_closes_the_menu_and_is_handed_on() {
        let (reader, _writer) = nix::unistd::pipe().expect("pipe");
        let mut keys = Keys::editor(reader.as_raw_fd(), Vec::new(), false);
        let click = crate::term::mouse::Event {
            button: crate::term::mouse::Button::Left,
            column: 4,
            row: 2,
            pressed: true,
            shift: false,
            alt: false,
            ctrl: false,
        };
        keys.unread_event(InputEvent::Mouse(click));
        keys.unread_event(InputEvent::Resized);
        assert!(
            DropdownMenu::select_interactive(
                vec![candidate("one"), candidate("two")],
                0,
                "",
                &mut keys,
            )
            .is_none()
        );
        assert_eq!(
            keys.read_event(),
            Some(InputEvent::Resized),
            "the editor has to see the resize, or the line is never repainted"
        );
        assert_eq!(keys.read_event(), Some(InputEvent::Mouse(click)));
    }

    #[test]
    fn escape_does_not_consume_the_following_event() {
        let (reader, _writer) = nix::unistd::pipe().expect("pipe");
        let mut keys = Keys::editor(reader.as_raw_fd(), Vec::new(), false);
        keys.unread_event(InputEvent::Key(Key::Cancel));
        keys.unread_event(InputEvent::Key(Key::Char('x')));
        assert!(
            DropdownMenu::select_interactive(
                vec![candidate("one"), candidate("two")],
                0,
                "",
                &mut keys,
            )
            .is_none()
        );
        assert_eq!(keys.read_event(), Some(InputEvent::Key(Key::Char('x'))));
    }
}
