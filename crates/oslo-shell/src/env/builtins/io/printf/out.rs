//! Where `printf`'s bytes go: standard output in chunks, or a buffer for `-v`.

/// Collected output, flushed to descriptor 1 whenever it grows past [`CHUNK`] unless it is kept
/// for a variable.
pub struct Out {
    buf: Vec<u8>,
    keep: bool,
    /// Bytes produced in the current pass over the format, for `%n`.
    pub pass: usize,
    /// The status of the first failed write.
    pub failed: Option<i32>,
}

const CHUNK: usize = 1 << 16;

impl Out {
    pub fn new(keep: bool) -> Self {
        Out {
            buf: Vec::new(),
            keep,
            pass: 0,
            failed: None,
        }
    }

    pub fn bytes(&mut self, bytes: &[u8]) {
        self.pass += bytes.len();
        if self.failed.is_some() {
            return;
        }
        self.buf.extend_from_slice(bytes);
        if !self.keep && self.buf.len() >= CHUNK {
            self.flush();
        }
    }

    pub fn byte(&mut self, byte: u8) {
        self.bytes(&[byte]);
    }

    /// `count` copies of `byte`, in pieces so a wide field is never one allocation.
    pub fn fill(&mut self, byte: u8, count: usize) {
        let piece = [byte; 4096];
        let mut left = count;
        while left > 0 && self.failed.is_none() {
            let n = left.min(piece.len());
            self.bytes(&piece[..n]);
            left -= n;
        }
        self.pass += left;
    }

    /// Write what is buffered, unless the output is kept for a variable.
    pub fn flush(&mut self) {
        if self.keep || self.buf.is_empty() || self.failed.is_some() {
            return;
        }
        match super::super::write_raw("printf", &self.buf) {
            0 => self.buf.clear(),
            status => self.failed = Some(status),
        }
    }

    /// The kept bytes, for `-v`.
    pub fn into_kept(self) -> Vec<u8> {
        self.buf
    }
}
