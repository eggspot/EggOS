//! A kernel panic's message as the screen shows it (D583): a fixed buffer that formatting
//! writes into, cut at a character boundary when full, so showing a panic allocates nothing.

/// The most bytes of a panic's text kept.
pub const PANIC_TEXT_BYTES: usize = 1024;

/// See the module documentation.
#[derive(Debug)]
pub struct PanicText {
    bytes: [u8; PANIC_TEXT_BYTES],
    length: usize,
}

impl PanicText {
    /// Empty text.
    pub const fn new() -> Self {
        Self {
            bytes: [0; PANIC_TEXT_BYTES],
            length: 0,
        }
    }

    /// The text written so far.
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.length]).unwrap_or("")
    }
}

impl Default for PanicText {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Write for PanicText {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let room = PANIC_TEXT_BYTES - self.length;
        let mut taken = text.len().min(room);
        while !text.is_char_boundary(taken) {
            taken -= 1;
        }
        self.bytes[self.length..self.length + taken].copy_from_slice(&text.as_bytes()[..taken]);
        self.length += taken;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::fmt::Write;

    #[test]
    fn text_is_kept_and_cut_whole_at_the_end() {
        let mut text = PanicText::new();
        let _ = write!(text, "panic at {}:{}", "kernel/src/x.rs", 4);
        assert_eq!(text.as_str(), "panic at kernel/src/x.rs:4");
        let long = "é".repeat(PANIC_TEXT_BYTES);
        let _ = text.write_str(&long);
        assert!(text.as_str().len() <= PANIC_TEXT_BYTES);
        assert!(text.as_str().ends_with('é'));
    }
}
