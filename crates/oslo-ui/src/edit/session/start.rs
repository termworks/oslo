//! How an editing session starts.

use super::{Session, Step};
use crate::edit::buffer::Buffer;
use crate::edit::vi::{Mode, Vi};

impl Session {
    pub fn new(text: &str, cursor: usize) -> Session {
        let mut buffer = Buffer::new();
        buffer.set(text, cursor);
        Session {
            buffer,
            vi: crate::vi::enabled().then(Vi::default),
            ..Session::default()
        }
    }

    /// Return the current vi mode stored by this session.
    pub fn mode(&self) -> Option<Mode> {
        self.vi.as_ref().map(|vi| vi.mode)
    }

    pub fn paste(&mut self, text: &str) -> Step {
        self.buffer.insert_str(text);
        Step::Continue {
            redraw: !text.is_empty(),
        }
    }
}
