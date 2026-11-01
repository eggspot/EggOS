//! Text in Egg Sans: measuring it, cutting it with `...` when it does not fit
//! (docs/ux/toolkit-v0.3.md section 2), and the fixed-size strings widgets keep their text in.

use eggos_font::egg_sans;

/// The ellipsis drawn after text cut to fit.
pub const ELLIPSIS: &str = "...";

/// The width of `text` at scale 1 and size 1.
pub fn width(text: &str) -> i32 {
    egg_sans::width(text) as i32
}

/// How much of `text` is drawn in `space` pixels: the bytes to draw and whether `...` follows
/// them. Whole text when it fits; else glyphs are dropped from the end until the rest and the
/// ellipsis fit; when not even one glyph and the ellipsis fit, as many whole glyphs as fit.
pub fn fit(text: &str, space: i32) -> (usize, bool) {
    if width(text) <= space {
        return (text.len(), false);
    }
    let room = space - width(ELLIPSIS);
    let mut used = 0;
    let mut end = 0;
    for (at, character) in text.char_indices() {
        let advance = i32::from(egg_sans::glyph(character).advance);
        if used + advance > room {
            break;
        }
        used += advance;
        end = at + character.len_utf8();
    }
    if end > 0 {
        return (end, true);
    }
    let mut used = 0;
    for (at, character) in text.char_indices() {
        let advance = i32::from(egg_sans::glyph(character).advance);
        if used + advance > space {
            return (at, false);
        }
        used += advance;
    }
    (text.len(), false)
}

/// A string of at most `N` bytes of UTF-8 kept inside the value, so widgets hold their text
/// without a heap.
#[derive(Clone, Copy)]
pub struct Text<const N: usize> {
    bytes: [u8; N],
    length: usize,
}

impl<const N: usize> core::fmt::Debug for Text<N> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self.as_str(), formatter)
    }
}

impl<const N: usize> Default for Text<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> From<&str> for Text<N> {
    fn from(text: &str) -> Self {
        let mut made = Self::new();
        made.set(text);
        made
    }
}

impl<const N: usize> PartialEq for Text<N> {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl<const N: usize> Eq for Text<N> {}

impl<const N: usize> Text<N> {
    /// The empty string.
    pub const fn new() -> Self {
        Self {
            bytes: [0; N],
            length: 0,
        }
    }

    /// The text.
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.length]).unwrap_or("")
    }

    /// Its length in bytes.
    pub fn len(&self) -> usize {
        self.length
    }

    /// Whether it is empty.
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// Replaces it with `text`, cut at the last whole character that fits.
    pub fn set(&mut self, text: &str) {
        self.length = 0;
        self.push_str(text);
    }

    /// Appends as much of `text` as fits, in whole characters; whether all of it did.
    pub fn push_str(&mut self, text: &str) -> bool {
        for character in text.chars() {
            if !self.insert(self.length, character) {
                return false;
            }
        }
        true
    }

    /// Inserts `character` at byte `at`, a character boundary; `false` when it does not fit
    /// or `at` is not a boundary.
    pub fn insert(&mut self, at: usize, character: char) -> bool {
        let mut encoded = [0u8; 4];
        let encoded = character.encode_utf8(&mut encoded).as_bytes();
        if self.length + encoded.len() > N || !self.as_str().is_char_boundary(at) {
            return false;
        }
        self.bytes.copy_within(at..self.length, at + encoded.len());
        self.bytes[at..at + encoded.len()].copy_from_slice(encoded);
        self.length += encoded.len();
        true
    }

    /// Removes the character that starts at byte `at`; how many bytes it had, 0 when there is
    /// none.
    pub fn remove(&mut self, at: usize) -> usize {
        let Some(character) = self.as_str().get(at..).and_then(|rest| rest.chars().next()) else {
            return 0;
        };
        let removed = character.len_utf8();
        self.bytes.copy_within(at + removed..self.length, at);
        self.length -= removed;
        removed
    }

    /// The boundary before byte `at`, or 0.
    pub fn previous_boundary(&self, at: usize) -> usize {
        self.as_str()[..at.min(self.length)]
            .char_indices()
            .last()
            .map_or(0, |(index, _)| index)
    }

    /// The boundary after byte `at`, or the end.
    pub fn next_boundary(&self, at: usize) -> usize {
        let at = at.min(self.length);
        self.as_str()[at..]
            .chars()
            .next()
            .map_or(self.length, |character| at + character.len_utf8())
    }
}

impl<const N: usize> core::fmt::Write for Text<N> {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        match self.push_str(text) {
            true => Ok(()),
            false => Err(core::fmt::Error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_that_does_not_fit_ends_in_an_ellipsis_never_cutting_a_glyph() {
        assert_eq!(fit("Settings", 100), (8, false));
        let whole = width("Settings");
        assert_eq!(fit("Settings", whole), (8, false));
        let (end, ellipsis) = fit("Settings", whole - 1);
        assert!(ellipsis);
        assert!(width(&"Settings"[..end]) + width(ELLIPSIS) < whole);
        assert!(width(&"Settings"[..end + 1]) + width(ELLIPSIS) > whole - 1);
        assert_eq!(fit("Settings", width("S") + 2), (1, false));
        assert_eq!(fit("Settings", 0), (0, false));
        assert_eq!(fit("Éé", width("É")), (2, false));
    }

    #[test]
    fn a_text_keeps_whole_characters_within_its_capacity() {
        let mut text = Text::<5>::from("aéb");
        assert_eq!(text.as_str(), "aéb");
        assert!(!text.insert(1, 'é'));
        assert!(text.insert(1, 'x'));
        assert_eq!(text.as_str(), "axéb");
        assert!(!text.insert(3, 'y'));
        assert_eq!(text.remove(2), 2);
        assert_eq!(text.as_str(), "axb");
        assert_eq!(text.remove(3), 0);
        assert_eq!(Text::<3>::from("aéé").as_str(), "aé");
        let text = Text::<8>::from("aéb");
        assert_eq!(text.next_boundary(1), 3);
        assert_eq!(text.previous_boundary(3), 1);
        assert_eq!(text.previous_boundary(0), 0);
        assert_eq!(text.next_boundary(4), 4);
    }
}
