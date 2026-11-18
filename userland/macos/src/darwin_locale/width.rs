//! The columns a character takes in a terminal, as `wcwidth` answers in a UTF-8 locale: 0 for
//! NUL, combining marks and zero-width characters, 2 for East Asian wide and full-width
//! characters and emoji, -1 for control characters, 1 for the rest. The ranges follow
//! Unicode's East Asian Width (W and F) and the main blocks of nonspacing marks (Mn, Me).

const ZERO_WIDTH: [(u32, u32); 24] = [
    (0x0300, 0x036f),
    (0x0483, 0x0489),
    (0x0591, 0x05bd),
    (0x05bf, 0x05bf),
    (0x05c1, 0x05c2),
    (0x05c4, 0x05c5),
    (0x05c7, 0x05c7),
    (0x0610, 0x061a),
    (0x064b, 0x065f),
    (0x0670, 0x0670),
    (0x06d6, 0x06dc),
    (0x06df, 0x06e4),
    (0x0e31, 0x0e31),
    (0x0e34, 0x0e3a),
    (0x0e47, 0x0e4e),
    (0x1ab0, 0x1aff),
    (0x1dc0, 0x1dff),
    (0x200b, 0x200f),
    (0x202a, 0x202e),
    (0x2060, 0x2064),
    (0x20d0, 0x20ff),
    (0xfe00, 0xfe0f),
    (0xfe20, 0xfe2f),
    (0xfeff, 0xfeff),
];

const WIDE: [(u32, u32); 17] = [
    (0x1100, 0x115f),
    (0x231a, 0x231b),
    (0x2329, 0x232a),
    (0x2e80, 0x303e),
    (0x3041, 0x33ff),
    (0x3400, 0x4dbf),
    (0x4e00, 0x9fff),
    (0xa000, 0xa4cf),
    (0xa960, 0xa97f),
    (0xac00, 0xd7a3),
    (0xf900, 0xfaff),
    (0xfe30, 0xfe4f),
    (0xff00, 0xff60),
    (0xffe0, 0xffe6),
    (0x1f300, 0x1f64f),
    (0x1f900, 0x1f9ff),
    (0x20000, 0x3fffd),
];

fn within(ranges: &[(u32, u32)], rune: u32) -> bool {
    ranges
        .binary_search_by(|&(first, last)| match () {
            () if last < rune => core::cmp::Ordering::Less,
            () if first > rune => core::cmp::Ordering::Greater,
            () => core::cmp::Ordering::Equal,
        })
        .is_ok()
}

/// The columns `rune` takes, or -1 for a control character or a value that is no character.
pub fn columns(rune: i32) -> i32 {
    let Ok(rune) = u32::try_from(rune) else {
        return -1;
    };
    match rune {
        0 => 0,
        0x01..=0x1f | 0x7f..=0x9f => -1,
        0xd800..=0xdfff => -1,
        0x11_0000.. => -1,
        _ if within(&ZERO_WIDTH, rune) => 0,
        _ if within(&WIDE, rune) => 2,
        _ => 1,
    }
}

/// The columns of `runes` together, or -1 when one of them has none.
pub fn columns_of(runes: impl IntoIterator<Item = i32>) -> i32 {
    let mut total: i32 = 0;
    for rune in runes {
        match columns(rune) {
            -1 => return -1,
            width => total = total.saturating_add(width),
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_marks_wide_characters_and_controls_have_their_widths() {
        assert!(ZERO_WIDTH.windows(2).all(|pair| pair[0].1 < pair[1].0));
        assert!(WIDE.windows(2).all(|pair| pair[0].1 < pair[1].0));
        assert_eq!(columns('a' as i32), 1);
        assert_eq!(columns(0), 0);
        assert_eq!(columns(0x0301), 0);
        assert_eq!(columns('漢' as i32), 2);
        assert_eq!(columns('가' as i32), 2);
        assert_eq!(columns(0x1f600), 2);
        assert_eq!(columns('\n' as i32), -1);
        assert_eq!(columns(-5), -1);
        assert_eq!(columns_of("e\u{301}漢".chars().map(|c| c as i32)), 3);
        assert_eq!(columns_of(['a' as i32, 0x07]), -1);
    }
}
