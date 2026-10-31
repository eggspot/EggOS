//! Where a `String`'s `Character`s begin and end (D180): extended grapheme clusters by Unicode's
//! rules (UAX #29) over the string's UTF-8: CR LF stays together, controls stand alone, Hangul
//! syllables join, combining marks, zero-width joiners and variation selectors stay with what
//! they follow, emoji join across a zero-width joiner, and regional indicators pair into flags.
//! The properties come from compact tables of the scripts EggOS shows today (D180 lists what
//! they leave out). Pure logic with host tests.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Property {
    CarriageReturn,
    LineFeed,
    Control,
    Extend,
    ZeroWidthJoiner,
    RegionalIndicator,
    Pictographic,
    LeadingJamo,
    VowelJamo,
    TrailingJamo,
    VowelSyllable,
    TrailingSyllable,
    Other,
}

const EXTEND: &[(u32, u32)] = &[
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
    (0x06e7, 0x06e8),
    (0x06ea, 0x06ed),
    (0x0900, 0x0903),
    (0x093a, 0x093c),
    (0x093e, 0x094f),
    (0x0951, 0x0957),
    (0x0962, 0x0963),
    (0x0981, 0x0983),
    (0x09bc, 0x09bc),
    (0x09be, 0x09cd),
    (0x0e31, 0x0e31),
    (0x0e34, 0x0e3a),
    (0x0e47, 0x0e4e),
    (0x1ab0, 0x1aff),
    (0x1dc0, 0x1dff),
    (0x200c, 0x200c),
    (0x20d0, 0x20f0),
    (0x302a, 0x302f),
    (0x3099, 0x309a),
    (0xfe00, 0xfe0f),
    (0xfe20, 0xfe2f),
    (0xff9e, 0xff9f),
    (0x1f3fb, 0x1f3ff),
    (0xe0020, 0xe007f),
    (0xe0100, 0xe01ef),
];

const PICTOGRAPHIC: &[(u32, u32)] = &[
    (0x00a9, 0x00a9),
    (0x00ae, 0x00ae),
    (0x203c, 0x203c),
    (0x2049, 0x2049),
    (0x2122, 0x2122),
    (0x2139, 0x2139),
    (0x2194, 0x21aa),
    (0x231a, 0x23ff),
    (0x24c2, 0x24c2),
    (0x25aa, 0x25fe),
    (0x2600, 0x27bf),
    (0x2934, 0x2935),
    (0x2b05, 0x2b55),
    (0x3030, 0x3030),
    (0x303d, 0x303d),
    (0x3297, 0x3299),
    (0x1f000, 0x1f1e5),
    (0x1f200, 0x1f3fa),
    (0x1f400, 0x1faff),
];

const FIRST_HANGUL_SYLLABLE: u32 = 0xac00;
const HANGUL_TRAILING_COUNT: u32 = 28;

fn within(table: &[(u32, u32)], scalar: u32) -> bool {
    table
        .binary_search_by(|&(first, last)| {
            if last < scalar {
                core::cmp::Ordering::Less
            } else if first > scalar {
                core::cmp::Ordering::Greater
            } else {
                core::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

fn property(scalar: u32) -> Property {
    match scalar {
        0x0d => Property::CarriageReturn,
        0x0a => Property::LineFeed,
        0x00..=0x1f | 0x7f..=0x9f | 0x200b | 0x200e..=0x200f | 0x2028..=0x202e => Property::Control,
        0x2060..=0x206f | 0xfeff | 0xfff0..=0xfffb => Property::Control,
        0x200d => Property::ZeroWidthJoiner,
        0x1f1e6..=0x1f1ff => Property::RegionalIndicator,
        0x1100..=0x115f | 0xa960..=0xa97c => Property::LeadingJamo,
        0x1160..=0x11a7 | 0xd7b0..=0xd7c6 => Property::VowelJamo,
        0x11a8..=0x11ff | 0xd7cb..=0xd7fb => Property::TrailingJamo,
        FIRST_HANGUL_SYLLABLE..=0xd7a3 => {
            if (scalar - FIRST_HANGUL_SYLLABLE).is_multiple_of(HANGUL_TRAILING_COUNT) {
                Property::VowelSyllable
            } else {
                Property::TrailingSyllable
            }
        }
        _ if within(EXTEND, scalar) => Property::Extend,
        _ if within(PICTOGRAPHIC, scalar) => Property::Pictographic,
        _ => Property::Other,
    }
}

/// The scalar starting at `at` and its length in bytes; a malformed sequence reads as U+FFFD
/// one byte long.
pub fn decode(bytes: &[u8], at: usize) -> (u32, usize) {
    const REPLACEMENT: (u32, usize) = (0xfffd, 1);
    let lead = bytes[at];
    let (length, initial) = match lead {
        0x00..=0x7f => return (u32::from(lead), 1),
        0xc2..=0xdf => (2, u32::from(lead & 0x1f)),
        0xe0..=0xef => (3, u32::from(lead & 0x0f)),
        0xf0..=0xf4 => (4, u32::from(lead & 0x07)),
        _ => return REPLACEMENT,
    };
    let Some(tail) = bytes.get(at + 1..at + length) else {
        return REPLACEMENT;
    };
    let mut scalar = initial;
    for &byte in tail {
        if byte & 0xc0 != 0x80 {
            return REPLACEMENT;
        }
        scalar = (scalar << 6) | u32::from(byte & 0x3f);
    }
    (scalar, length)
}

/// Encodes `scalar` as UTF-8 into `out`, returning its length.
pub fn encode(scalar: u32, out: &mut [u8; 4]) -> usize {
    match scalar {
        0..=0x7f => {
            out[0] = scalar as u8;
            1
        }
        0x80..=0x7ff => {
            out[0] = 0xc0 | (scalar >> 6) as u8;
            out[1] = 0x80 | (scalar & 0x3f) as u8;
            2
        }
        0x800..=0xffff => {
            out[0] = 0xe0 | (scalar >> 12) as u8;
            out[1] = 0x80 | ((scalar >> 6) & 0x3f) as u8;
            out[2] = 0x80 | (scalar & 0x3f) as u8;
            3
        }
        _ => {
            out[0] = 0xf0 | (scalar >> 18) as u8;
            out[1] = 0x80 | ((scalar >> 12) & 0x3f) as u8;
            out[2] = 0x80 | ((scalar >> 6) & 0x3f) as u8;
            out[3] = 0x80 | (scalar & 0x3f) as u8;
            4
        }
    }
}

struct State {
    previous: Property,
    emoji_sequence: bool,
    regional_indicators: usize,
}

impl State {
    fn new(first: Property) -> Self {
        Self {
            previous: first,
            emoji_sequence: first == Property::Pictographic,
            regional_indicators: usize::from(first == Property::RegionalIndicator),
        }
    }

    fn breaks_before(&self, next: Property) -> bool {
        use Property::*;
        match (self.previous, next) {
            (CarriageReturn, LineFeed) => false,
            (CarriageReturn | LineFeed | Control, _) | (_, CarriageReturn | LineFeed | Control) => {
                true
            }
            (LeadingJamo, LeadingJamo | VowelJamo | VowelSyllable | TrailingSyllable) => false,
            (VowelSyllable | VowelJamo, VowelJamo | TrailingJamo) => false,
            (TrailingSyllable | TrailingJamo, TrailingJamo) => false,
            (_, Extend | ZeroWidthJoiner) => false,
            (ZeroWidthJoiner, Pictographic) => !self.emoji_sequence,
            (RegionalIndicator, RegionalIndicator) => self.regional_indicators.is_multiple_of(2),
            _ => true,
        }
    }

    fn advance(&mut self, next: Property) {
        self.emoji_sequence = match next {
            Property::Pictographic => true,
            Property::Extend | Property::ZeroWidthJoiner => self.emoji_sequence,
            _ => false,
        };
        self.regional_indicators = if next == Property::RegionalIndicator {
            self.regional_indicators + 1
        } else {
            0
        };
        self.previous = next;
    }
}

/// Where the grapheme cluster that starts at `start` ends.
pub fn next_break(bytes: &[u8], start: usize) -> usize {
    if start >= bytes.len() {
        return bytes.len();
    }
    let first = bytes[start];
    if first < 0x80 && first != b'\r' && bytes.get(start + 1).is_none_or(|&next| next < 0x80) {
        return start + 1;
    }
    let (scalar, length) = decode(bytes, start);
    let mut state = State::new(property(scalar));
    let mut at = start + length;
    while at < bytes.len() {
        let (scalar, length) = decode(bytes, at);
        let next = property(scalar);
        if state.breaks_before(next) {
            break;
        }
        state.advance(next);
        at += length;
    }
    at
}

/// Where the grapheme cluster that contains the byte at `offset` starts.
pub fn cluster_start(bytes: &[u8], offset: usize) -> usize {
    let mut start = 0;
    while start < bytes.len() {
        let end = next_break(bytes, start);
        if offset < end {
            return start;
        }
        start = end;
    }
    bytes.len()
}

/// The number of grapheme clusters in `bytes`.
pub fn count(bytes: &[u8]) -> usize {
    let mut clusters = 0;
    let mut at = 0;
    while at < bytes.len() {
        at = next_break(bytes, at);
        clusters += 1;
    }
    clusters
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clusters(text: &str) -> Vec<&str> {
        let bytes = text.as_bytes();
        let mut parts = Vec::new();
        let mut at = 0;
        while at < bytes.len() {
            let end = next_break(bytes, at);
            parts.push(&text[at..end]);
            at = end;
        }
        parts
    }

    #[test]
    fn ascii_breaks_between_every_byte_but_carriage_return_line_feed() {
        assert_eq!(clusters("ab\r\nc\n\r"), ["a", "b", "\r\n", "c", "\n", "\r"]);
        assert_eq!(count(b"Hello, World"), 12);
    }

    #[test]
    fn combining_marks_stay_with_their_base() {
        assert_eq!(clusters("He\u{301}llo"), ["H", "e\u{301}", "l", "l", "o"]);
        assert_eq!(clusters("H\u{e9}llo, W\u{f6}rld").len(), 12);
        assert_eq!(clusters("\u{301}a"), ["\u{301}", "a"]);
    }

    #[test]
    fn emoji_join_across_zero_width_joiners_and_take_modifiers() {
        let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}";
        assert_eq!(clusters(family), [family]);
        assert_eq!(clusters("\u{1f44d}\u{1f3fd}x"), ["\u{1f44d}\u{1f3fd}", "x"]);
        assert_eq!(clusters("\u{2764}\u{fe0f}"), ["\u{2764}\u{fe0f}"]);
        assert_eq!(clusters("a\u{200d}\u{1f466}"), ["a\u{200d}", "\u{1f466}"]);
    }

    #[test]
    fn regional_indicators_pair_into_flags() {
        let flags = "\u{1f1eb}\u{1f1f7}\u{1f1e9}\u{1f1ea}\u{1f1ee}";
        assert_eq!(
            clusters(flags),
            ["\u{1f1eb}\u{1f1f7}", "\u{1f1e9}\u{1f1ea}", "\u{1f1ee}"]
        );
    }

    #[test]
    fn hangul_jamo_join_into_syllables() {
        assert_eq!(
            clusters("\u{1100}\u{1161}\u{11a8}"),
            ["\u{1100}\u{1161}\u{11a8}"]
        );
        assert_eq!(
            clusters("\u{ac00}\u{11a8}\u{ac01}"),
            ["\u{ac00}\u{11a8}", "\u{ac01}"]
        );
    }

    #[test]
    fn cluster_starts_round_down() {
        let text = "ae\u{301}b".as_bytes();
        assert_eq!(cluster_start(text, 0), 0);
        assert_eq!(cluster_start(text, 2), 1);
        assert_eq!(cluster_start(text, 3), 1);
        assert_eq!(cluster_start(text, 4), 4);
        assert_eq!(cluster_start(text, 5), 5);
    }

    #[test]
    fn decodes_and_encodes_every_length() {
        for scalar in [0x41, 0xe9, 0x20ac, 0x1f600] {
            let mut bytes = [0u8; 4];
            let length = encode(scalar, &mut bytes);
            assert_eq!(decode(&bytes[..length], 0), (scalar, length));
            let expected = char::from_u32(scalar).unwrap().to_string();
            assert_eq!(&bytes[..length], expected.as_bytes());
        }
        assert_eq!(decode(&[0xc3], 0), (0xfffd, 1));
        assert_eq!(decode(&[0xff, 0x41], 0), (0xfffd, 1));
    }
}
