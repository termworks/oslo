//! Opening the explorer: raw mode, the key loop, and putting the terminal back.
//!
//! **Whatever happens, the terminal is restored.** The alternate screen, the cursor and the original
//! termios are undone on every path out, including the ones nobody plans for — the guard runs on
//! drop, so the loop below cannot forget it. The same rule the history finder is written to, and for
//! the same reason: a viewer that leaves the screen in its own mode leaves a shell you cannot type
//! into.

use super::render::{Frame, fitting, frame, widths, window};
use super::{Cell, Sheet};
use crate::dropdown::width::{terminal_cols, terminal_rows};
use crate::term::{Key, Keys, Restore, Screen};

/// How the explorer was left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Read, and closed.
    Closed,
    /// There were no rows. A viewer that opened onto an empty screen would look like a hang.
    Empty,
    /// Nothing to draw on: no terminal, and no `/dev/tty` either.
    NoTerminal,
}

/// One level of the descent: a table, and where you were in it.
///
/// Kept whole on the way down so backing out puts the cursor back where it was rather than at the
/// top of a table you had already scrolled through.
struct Level {
    sheet: Sheet,
    row: usize,
    column: usize,
    top: usize,
    left: usize,
}

impl Level {
    fn new(sheet: Sheet) -> Level {
        Level {
            sheet,
            row: 0,
            column: 0,
            top: 0,
            left: 0,
        }
    }

    /// The table under the cursor, if the cell has one.
    fn under_the_cursor(&self) -> Option<&Sheet> {
        self.sheet
            .rows
            .get(self.row)
            .and_then(|row| row.get(self.column))
            .and_then(Cell::sheet)
    }
}

/// Open `sheet` and read keys until it is dismissed.
pub fn open(sheet: Sheet) -> Outcome {
    if sheet.rows.is_empty() {
        return Outcome::Empty;
    }
    let Some(restore) = Restore::enter(Screen::Alternate) else {
        return Outcome::NoTerminal;
    };

    let mut stack = vec![Level::new(sheet)];
    let mut keys = Keys::on(restore.fd());
    // The last frame written, so a keystroke that changed nothing does not repaint the screen.
    let mut last = String::new();

    loop {
        let (cols, rows) = (terminal_cols(), terminal_rows());
        let trail: Vec<String> = stack
            .iter()
            .take(stack.len() - 1)
            .map(|level| level.sheet.title.clone())
            .collect();
        let here = stack.last_mut().expect("a level is always open");
        let height = window(rows);
        let measured = widths(&here.sheet);

        // **Both viewports are clamped here, once, before drawing.** Doing it at each key would
        // mean every movement key repeating the same four lines, and a terminal resized between
        // frames would leave the cursor outside a window nothing had corrected.
        here.row = here.row.min(here.sheet.rows.len().saturating_sub(1));
        here.top = here.top.min(here.row);
        if here.row >= here.top + height {
            here.top = here.row + 1 - height;
        }
        here.column = here.column.min(here.sheet.columns.len().saturating_sub(1));
        here.left = here.left.min(here.column);
        while here.column >= here.left + fitting(&measured, here.left, cols) {
            here.left += 1;
        }

        let painted = frame(&Frame {
            sheet: &here.sheet,
            row: here.row,
            column: here.column,
            top: here.top,
            left: here.left,
            trail: &trail,
            cols,
            rows,
        });
        if painted != last {
            crate::ask::show(&painted);
            last.clone_from(&painted);
        }

        let Some(pressed) = keys.read() else {
            return Outcome::Closed;
        };
        let last_row = here.sheet.rows.len().saturating_sub(1);
        match pressed {
            Key::Cancel | Key::Abort => return Outcome::Closed,
            Key::Up => here.row = here.row.saturating_sub(1),
            Key::Down => here.row = (here.row + 1).min(last_row),
            Key::Left => here.column = here.column.saturating_sub(1),
            Key::Right => {
                here.column = (here.column + 1).min(here.sheet.columns.len().saturating_sub(1))
            }
            Key::PageUp => here.row = here.row.saturating_sub(height),
            Key::PageDown => here.row = (here.row + height).min(last_row),
            Key::Home => here.row = 0,
            Key::End => here.row = last_row,
            // Down into the cell under the cursor. A cell with nothing under it does nothing rather
            // than opening an empty level.
            Key::Accept => {
                if let Some(sheet) = here.under_the_cursor() {
                    let sheet = sheet.clone();
                    stack.push(Level::new(sheet));
                    last.clear();
                }
            }
            Key::Backspace if stack.len() > 1 => {
                stack.pop();
                last.clear();
            }
            _ => {}
        }
    }
}
