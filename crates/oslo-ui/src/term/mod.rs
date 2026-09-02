//! Terminal ownership, restoration, and decoded input.

use nix::sys::termios::{InputFlags, LocalFlags, SetArg, Termios, tcgetattr, tcsetattr};
use std::cell::Cell;
use std::fs::File;
use std::io::{self, Write};
use std::os::fd::{AsRawFd, OwnedFd};

/// What the editor writes before it draws, to undo what the last program left behind.
///
/// **A program that exits badly leaves the terminal mid-state, and the shell inherits it.** `\e(0`
/// puts the G0 charset into line drawing, so every letter after it comes out as a box character;
/// `\e[?7l` turns autowrap off, so long lines overwrite their own last column instead of wrapping;
/// a stray SGR leaves everything bold or inverted. None of those are the shell's doing and all of
/// them make its output look broken.
///
/// `reset` fixes them, and having to run it is the complaint: the shell knows it is about to draw a
/// line and can simply not draw it through somebody else's leftovers. Three sequences, all
/// idempotent, all understood by every terminal worth the name:
///
/// * `\e[0m` — attributes off, so no colour or bold leaks into the prompt.
/// * `\e(B` — ASCII into G0, which is what undoes the line-drawing charset.
/// * `\e[?7h` — autowrap on, which is what makes a long line wrap rather than pile up.
///
/// Deliberately *not* a full `RIS`: that clears the screen and drops the scrollback, which is a
/// thing to do when asked and never a thing to do before every prompt.
pub const SANITISE: &[u8] = b"\x1b[0m\x1b(B\x1b[?7h";

pub const BRACKETED_PASTE_ENABLE: &[u8] = b"\x1b[?2004h";
pub const BRACKETED_PASTE_DISABLE: &[u8] = b"\x1b[?2004l";

thread_local! {
    static EDITOR_KITTY_ACTIVE: Cell<bool> = const { Cell::new(false) };
    static EDITOR_LEGACY_MOUSE_ACTIVE: Cell<bool> = const { Cell::new(false) };
}

pub struct Tty {
    _owned: Option<OwnedFd>,
    fd: i32,
}

impl Tty {
    pub fn open() -> Option<Tty> {
        if io::IsTerminal::is_terminal(&io::stdin()) {
            return Some(Tty {
                _owned: None,
                fd: nix::libc::STDIN_FILENO,
            });
        }
        let file = File::options()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .ok()?;
        let fd = file.as_raw_fd();
        Some(Tty {
            fd,
            _owned: Some(file.into()),
        })
    }

    pub fn fd(&self) -> i32 {
        self.fd
    }
}

pub struct Restore {
    tty: Tty,
    original: Termios,
    alternate: bool,
    bracketed_paste: bool,
    kitty_keyboard: bool,
    resume_kitty_keyboard: bool,
    resume_legacy_mouse: bool,
    mouse_events: bool,
    legacy_mouse: bool,
    pending: Vec<u8>,
}

impl Restore {
    pub fn fd(&self) -> i32 {
        self.tty.fd()
    }

    pub fn mouse_events(&self) -> bool {
        self.mouse_events
    }

    pub fn take_pending(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pending)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Inline,
    Alternate,
    Line,
}

impl Restore {
    pub fn enter(screen: Screen) -> Option<Restore> {
        let alternate = screen == Screen::Alternate;
        let bracketed_paste = screen == Screen::Line;
        let tty = Tty::open()?;
        // SAFETY: `tty` owns this descriptor for the guard's lifetime.
        let handle = unsafe { std::os::fd::BorrowedFd::borrow_raw(tty.fd()) };
        let original = tcgetattr(handle).ok()?;
        let raw = editor_termios(&original);
        tcsetattr(handle, SetArg::TCSANOW, &raw).ok()?;

        let pending = if screen == Screen::Line {
            query::take_startup_input()
        } else {
            Vec::new()
        };
        let kitty_keyboard = screen == Screen::Line && keyboard::supported();
        let capabilities = capability::snapshot();
        let mouse_events =
            screen == Screen::Line && (capabilities.semantic_clicks || capabilities.legacy_clicks);
        let legacy_mouse = screen == Screen::Line && capabilities.legacy_clicks;
        let resume_kitty_keyboard =
            screen != Screen::Line && EDITOR_KITTY_ACTIVE.with(|active| active.replace(false));
        let resume_legacy_mouse = screen != Screen::Line
            && EDITOR_LEGACY_MOUSE_ACTIVE.with(|active| active.replace(false));

        let mut out = io::stderr();
        // **Before anything else this writes.** The line editor is about to draw, and whatever the
        // last program left behind is what it would draw through. See [`SANITISE`].
        let _ = out.write_all(SANITISE);
        if resume_kitty_keyboard {
            let _ = out.write_all(keyboard::POP.as_bytes());
        }
        if resume_legacy_mouse {
            let _ = out.write_all(mouse::DISABLE);
        }
        let _ = out.write_all(match screen {
            Screen::Alternate => b"\x1b[?1049h\x1b[?25l".as_slice(),
            Screen::Inline => b"\x1b[?25l".as_slice(),
            Screen::Line => BRACKETED_PASTE_ENABLE,
        });
        if kitty_keyboard {
            let _ = out.write_all(keyboard::PUSH_ENHANCEMENTS.as_bytes());
            EDITOR_KITTY_ACTIVE.with(|active| active.set(true));
        }
        if legacy_mouse {
            let _ = out.write_all(mouse::ENABLE);
            EDITOR_LEGACY_MOUSE_ACTIVE.with(|active| active.set(true));
        }
        let _ = out.flush();
        // What it takes to undo all of that, stashed where a panic hook can reach it: `Drop` below
        // is the ordinary way home and `panic = "abort"` does not run it. See [`rescue`].
        rescue::remember(
            tty.fd(),
            &original,
            alternate,
            bracketed_paste,
            kitty_keyboard,
            legacy_mouse,
        );
        Some(Restore {
            tty,
            original,
            alternate,
            bracketed_paste,
            kitty_keyboard,
            resume_kitty_keyboard,
            resume_legacy_mouse,
            mouse_events,
            legacy_mouse,
            pending,
        })
    }
}

fn editor_termios(original: &Termios) -> Termios {
    let mut raw = original.clone();
    raw.local_flags.remove(LocalFlags::ICANON);
    raw.local_flags.remove(LocalFlags::ECHO);
    raw.local_flags.remove(LocalFlags::ISIG);
    // Ctrl-S belongs to the editor, not to the line discipline. Left on, IXON eats the byte as
    // XOFF and the terminal stops painting — so the documented `oslo.keys["ctrl-s"]` binding could
    // never fire, and the only way out was Ctrl-Q, unbindable for the same reason.
    raw.input_flags.remove(InputFlags::IXON);
    raw.control_chars[nix::libc::VMIN] = 1;
    raw.control_chars[nix::libc::VTIME] = 0;
    raw
}

impl Drop for Restore {
    fn drop(&mut self) {
        // The terminal is going back the ordinary way, so the rescue has nothing left to do.
        rescue::forget();
        let mut out = io::stderr();
        if self.legacy_mouse {
            let _ = out.write_all(mouse::DISABLE);
            EDITOR_LEGACY_MOUSE_ACTIVE.with(|active| active.set(false));
        }
        if self.kitty_keyboard {
            let _ = out.write_all(keyboard::POP.as_bytes());
            EDITOR_KITTY_ACTIVE.with(|active| active.set(false));
        }
        if self.bracketed_paste {
            let _ = out.write_all(BRACKETED_PASTE_DISABLE);
        }
        let _ = out.write_all(if self.alternate {
            b"\x1b[?25h\x1b[?1049l".as_slice()
        } else {
            b"\x1b[?25h".as_slice()
        });
        let _ = out.flush();
        // SAFETY: `self.tty` still owns the descriptor.
        let handle = unsafe { std::os::fd::BorrowedFd::borrow_raw(self.tty.fd()) };
        let _ = tcsetattr(handle, SetArg::TCSANOW, &self.original);
        if self.resume_kitty_keyboard {
            let mut out = io::stderr();
            let _ = out.write_all(keyboard::PUSH_ENHANCEMENTS.as_bytes());
            let _ = out.flush();
            EDITOR_KITTY_ACTIVE.with(|active| active.set(true));
        }
        if self.resume_legacy_mouse {
            let mut out = io::stderr();
            let _ = out.write_all(mouse::ENABLE);
            let _ = out.flush();
            EDITOR_LEGACY_MOUSE_ACTIVE.with(|active| active.set(true));
        }
    }
}

/// Whether this terminal draws anything at all.
///
/// `TERM=dumb` is the long-standing way of saying it does not — readline turns itself off on it,
/// and oslo already reads it in three places: no colour ([`crate::theme`]), no terminal queries
/// (`startup::terminal`), no OSC 133 marks ([`crate::marks`]). An empty `TERM` says the same thing
/// less loudly.
///
/// **The point of asking is to not spend anything.** Everything gated on this is work whose only
/// product is something on a screen — a colour, a mark, a frame of an animation. Where there is no
/// screen the work has no product, and the most expensive of it spawns processes: see
/// [`crate::prompt::animation::animate_in`].
///
/// An *unset* `TERM` is deliberately not the same answer as `dumb`. It is the ordinary state of a
/// perfectly capable terminal that nobody has told, and treating it as no terminal at all would
/// turn oslo monochrome in every environment that forgot to export one.
pub fn draws() -> bool {
    drawn_by(std::env::var("TERM").ok().as_deref())
}

/// The rule itself, given the value rather than reading it.
///
/// Split so the test can state every case without setting a process-wide variable every other test
/// in the binary shares — the trap `startup::editor`'s own tests call out by refusing to set it.
fn drawn_by(term: Option<&str>) -> bool {
    !matches!(term, Some("dumb") | Some(""))
}

pub mod anchor;
pub mod capability;
mod child;
pub mod freshline;
mod input;
pub mod keyboard;
pub mod metadata;
pub mod mouse;
pub mod negotiate;
pub mod osc133;
mod paste;
pub mod query;
pub mod rescue;
mod resize;
pub mod semantic;
pub mod vscode;
pub use child::watch_for_children;
pub use input::{EventPressed, InputEvent, Key, Keys, PasteError, Pressed, key};
pub use resize::watch_for_resize;

#[cfg(test)]
use input::{Parsed, parse};

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
