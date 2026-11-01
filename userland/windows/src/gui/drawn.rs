//! The text a program draws with `TextOutW` and `DrawTextW`, remembered per window (D162) so
//! an agent's element tree shows it (`wm show`): the last [`MAX_DRAWN`] pieces, each where it
//! was drawn in its window's client area and cut to [`DRAWN_TEXT_BYTES`] of UTF-8. A piece
//! drawn where another started replaces it, and painting a region forgets what was drawn in
//! it, so the store follows what the window shows.

use eggos_windows_gui::surface::Rect;

/// The most pieces of drawn text a program's windows keep.
pub const MAX_DRAWN: usize = 32;
/// The most bytes of one piece.
pub const DRAWN_TEXT_BYTES: usize = 64;

/// One piece of drawn text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrawnText {
    /// The window it was drawn in.
    pub window: usize,
    /// Where, in the window's client coordinates.
    pub rect: Rect,
    length: usize,
    text: [u8; DRAWN_TEXT_BYTES],
    age: u32,
}

impl DrawnText {
    const EMPTY: DrawnText = DrawnText {
        window: 0,
        rect: Rect::new(0, 0, 0, 0),
        length: 0,
        text: [0; DRAWN_TEXT_BYTES],
        age: 0,
    };

    /// Its text.
    pub fn text(&self) -> &[u8] {
        &self.text[..self.length]
    }
}

/// The pieces of every window of the program.
#[derive(Debug, Clone)]
pub struct DrawnTexts {
    pieces: [DrawnText; MAX_DRAWN],
    clock: u32,
}

impl Default for DrawnTexts {
    fn default() -> Self {
        Self::new()
    }
}

impl DrawnTexts {
    /// No pieces.
    pub const fn new() -> Self {
        Self {
            pieces: [DrawnText::EMPTY; MAX_DRAWN],
            clock: 0,
        }
    }

    /// Remembers `units` drawn in `window` at `rect`; empty text and text of only spaces is
    /// not kept. It replaces a piece of the window that started at the same point, else a free
    /// slot, else the oldest piece.
    pub fn record(&mut self, window: usize, rect: Rect, units: &[u16]) {
        let mut text = [0u8; DRAWN_TEXT_BYTES];
        let mut length = 0;
        for character in char::decode_utf16(units.iter().copied()) {
            let character = character.unwrap_or(char::REPLACEMENT_CHARACTER);
            let mut encoded = [0u8; 4];
            let encoded = character.encode_utf8(&mut encoded).as_bytes();
            if length + encoded.len() > DRAWN_TEXT_BYTES {
                break;
            }
            text[length..length + encoded.len()].copy_from_slice(encoded);
            length += encoded.len();
        }
        if text[..length].iter().all(|&byte| byte == b' ') {
            return;
        }
        self.clock = self.clock.wrapping_add(1);
        let same_place = |piece: &DrawnText| {
            piece.length > 0
                && piece.window == window
                && (piece.rect.left, piece.rect.top) == (rect.left, rect.top)
        };
        let slot = match self.pieces.iter().position(same_place) {
            Some(slot) => slot,
            None => match self.pieces.iter().position(|piece| piece.length == 0) {
                Some(slot) => slot,
                None => self.oldest(),
            },
        };
        self.pieces[slot] = DrawnText {
            window,
            rect,
            length,
            text,
            age: self.clock,
        };
    }

    fn oldest(&self) -> usize {
        let clock = self.clock;
        (0..MAX_DRAWN)
            .max_by_key(|&slot| clock.wrapping_sub(self.pieces[slot].age))
            .unwrap_or(0)
    }

    /// Forgets the pieces of `window` that overlap `region`, which is being painted again.
    pub fn forget_in(&mut self, window: usize, region: Rect) {
        for piece in &mut self.pieces {
            if piece.length > 0
                && piece.window == window
                && !piece.rect.intersection(&region).is_empty()
            {
                *piece = DrawnText::EMPTY;
            }
        }
    }

    /// Forgets every piece of `window`, which is gone.
    pub fn forget_window(&mut self, window: usize) {
        for piece in &mut self.pieces {
            if piece.window == window {
                *piece = DrawnText::EMPTY;
            }
        }
    }

    /// The pieces of `window`, oldest first.
    pub fn of(&self, window: usize) -> impl Iterator<Item = (usize, &DrawnText)> + '_ {
        let mut order = [0usize; MAX_DRAWN];
        for (slot, entry) in order.iter_mut().enumerate() {
            *entry = slot;
        }
        let clock = self.clock;
        order.sort_unstable_by_key(|&slot| {
            core::cmp::Reverse(clock.wrapping_sub(self.pieces[slot].age))
        });
        order
            .into_iter()
            .map(move |slot| (slot, &self.pieces[slot]))
            .filter(move |(_, piece)| piece.length > 0 && piece.window == window)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    fn texts(drawn: &DrawnTexts, window: usize) -> Vec<String> {
        drawn
            .of(window)
            .map(|(_, piece)| String::from_utf8(piece.text().to_vec()).unwrap())
            .collect()
    }

    #[test]
    fn pieces_are_kept_per_window_replaced_in_place_and_forgotten_when_painted_over() {
        let mut drawn = DrawnTexts::new();
        drawn.record(1, Rect::sized(10, 10, 40, 16), &wide("Hello"));
        drawn.record(2, Rect::sized(0, 0, 8, 16), &wide("x"));
        drawn.record(1, Rect::sized(10, 30, 8, 16), &wide("é"));
        drawn.record(1, Rect::sized(10, 50, 24, 16), &wide("   "));
        assert_eq!(texts(&drawn, 1), ["Hello", "é"]);
        drawn.record(1, Rect::sized(10, 10, 48, 16), &wide("Hello!"));
        assert_eq!(texts(&drawn, 1), ["é", "Hello!"]);
        drawn.forget_in(1, Rect::sized(0, 0, 100, 20));
        assert_eq!(texts(&drawn, 1), ["é"]);
        drawn.forget_window(1);
        assert!(texts(&drawn, 1).is_empty());
        assert_eq!(texts(&drawn, 2), ["x"]);
    }

    #[test]
    fn long_text_is_cut_at_a_character_and_the_oldest_piece_makes_room() {
        let mut drawn = DrawnTexts::new();
        drawn.record(1, Rect::sized(0, 0, 8, 16), &wide(&"ü".repeat(40)));
        assert_eq!(texts(&drawn, 1)[0].len(), DRAWN_TEXT_BYTES);
        for index in 0..MAX_DRAWN as i32 {
            drawn.record(1, Rect::sized(0, 20 + index, 8, 16), &wide("a"));
        }
        let kept = texts(&drawn, 1);
        assert_eq!(kept.len(), MAX_DRAWN);
        assert!(kept.iter().all(|text| text == "a"), "the first piece went");
    }
}
