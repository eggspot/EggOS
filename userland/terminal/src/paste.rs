//! Pasting into a terminal (docs/ux/terminal-selection-v0.3.md section 1.3): the clipboard's
//! text made safe to type, in bracketed-paste brackets when a program asked for them, and
//! written to the pseudo-terminal a chunk at a time as it has room.

use alloc::vec::Vec;

/// The most bytes of the clipboard one paste takes; a longer clipboard pastes its start.
pub const PASTE_CAPACITY: usize = 64 * 1024;

/// The most bytes written to the pseudo-terminal each time it has room.
pub const PASTE_CHUNK: usize = 1024;

/// What a program asking for bracketed paste (`CSI ? 2004 h`) gets before a paste.
pub const BRACKET_START: &[u8] = b"\x1b[200~";

/// What it gets after one.
pub const BRACKET_END: &[u8] = b"\x1b[201~";

const TAB: u8 = 0x09;
const LF: u8 = 0x0a;
const CR: u8 = 0x0d;
const ESC: u8 = 0x1b;
const DEL: u8 = 0x7f;
const C1_LEAD: u8 = 0xc2;
const C1: core::ops::RangeInclusive<u8> = 0x80..=0x9f;
const C1_CSI: u8 = 0x9b;
const FINAL_BYTES: core::ops::RangeInclusive<u8> = 0x40..=0x7e;

/// The bytes a paste of `text` types: `CR LF` and `LF` become `CR`, `TAB` stays, every other
/// control character is dropped (C0, `DEL` and the C1 controls U+0080 to U+009F), `ESC` and
/// U+009B with the control sequence they start (`ESC [` or U+009B up to its final byte) or
/// alone, other UTF-8 passes through; at most
/// [`PASTE_CAPACITY`] bytes of `text`, cut at a character boundary; between [`BRACKET_START`]
/// and [`BRACKET_END`] when `bracketed`. Empty for an empty text.
pub fn prepare(text: &str, bracketed: bool) -> Vec<u8> {
    let mut end = text.len().min(PASTE_CAPACITY);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let text = &text.as_bytes()[..end];
    let mut bytes = Vec::new();
    if text.is_empty() {
        return bytes;
    }
    let brackets = match bracketed {
        true => BRACKET_START.len() + BRACKET_END.len(),
        false => 0,
    };
    if bytes.try_reserve_exact(text.len() + brackets).is_err() {
        return bytes;
    }
    if bracketed {
        bytes.extend_from_slice(BRACKET_START);
    }
    let mut index = 0;
    while index < text.len() {
        match text[index] {
            CR if text.get(index + 1) == Some(&LF) => {
                bytes.push(CR);
                index += 1;
            }
            CR | LF => bytes.push(CR),
            TAB => bytes.push(TAB),
            ESC if text.get(index + 1) == Some(&b'[') => {
                index = end_of_sequence(text, index + 2);
            }
            C1_LEAD if text.get(index + 1) == Some(&C1_CSI) => {
                index = end_of_sequence(text, index + 2);
            }
            C1_LEAD if text.get(index + 1).is_some_and(|next| C1.contains(next)) => index += 1,
            byte if byte < 0x20 || byte == DEL => {}
            byte => bytes.push(byte),
        }
        index += 1;
    }
    if bracketed {
        bytes.extend_from_slice(BRACKET_END);
    }
    bytes
}

fn end_of_sequence(text: &[u8], mut index: usize) -> usize {
    while index < text.len() && !FINAL_BYTES.contains(&text[index]) {
        index += 1;
    }
    index
}

/// A paste waiting to be written: the bytes [`prepare`] made, and how many are written.
#[derive(Debug, Default)]
pub struct PendingPaste {
    bytes: Vec<u8>,
    written: usize,
}

impl PendingPaste {
    /// Queues `bytes` after what is still waiting.
    pub fn push(&mut self, bytes: &[u8]) {
        if self.written == self.bytes.len() {
            self.bytes.clear();
            self.written = 0;
        }
        if self.bytes.try_reserve(bytes.len()).is_ok() {
            self.bytes.extend_from_slice(bytes);
        }
    }

    /// Whether nothing waits.
    pub fn is_empty(&self) -> bool {
        self.written == self.bytes.len()
    }

    /// The next at most [`PASTE_CHUNK`] bytes to write.
    pub fn next_chunk(&self) -> &[u8] {
        let end = (self.written + PASTE_CHUNK).min(self.bytes.len());
        &self.bytes[self.written..end]
    }

    /// `count` bytes of [`Self::next_chunk`] were written.
    pub fn written(&mut self, count: usize) {
        self.written = (self.written + count).min(self.bytes.len());
        if self.is_empty() {
            self.bytes = Vec::new();
            self.written = 0;
        }
    }
}
