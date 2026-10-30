//! `String.lowercased()`'s mapping of a scalar to its lowercase (D180): Unicode's lowercase
//! mappings for Latin (ASCII, Latin-1, Extended-A and Additional), Greek, Cyrillic, Armenian,
//! Georgian and fullwidth Latin, including `İ`'s two scalars; other scalars stay as they are.
//! Pure logic with host tests.

fn alternating(scalar: u32, first: u32, last: u32, upper_parity: u32) -> Option<u32> {
    ((first..=last).contains(&scalar) && scalar % 2 == upper_parity).then_some(scalar + 1)
}

fn single(scalar: u32) -> u32 {
    let shifted = match scalar {
        0x41..=0x5a | 0xc0..=0xd6 | 0xd8..=0xde => return scalar + 0x20,
        0x0391..=0x03a1 | 0x03a3..=0x03ab => return scalar + 0x20,
        0x0410..=0x042f => return scalar + 0x20,
        0x0400..=0x040f => return scalar + 0x50,
        0x0531..=0x0556 => return scalar + 0x30,
        0x10a0..=0x10c5 => return scalar + 0x1c60,
        0xff21..=0xff3a => return scalar + 0x20,
        0x0178 => return 0xff,
        0x0386 => return 0x03ac,
        0x0388..=0x038a => return scalar + 0x25,
        0x038c => return 0x03cc,
        0x038e..=0x038f => return scalar + 0x3f,
        _ => alternating(scalar, 0x0100, 0x012f, 0)
            .or_else(|| alternating(scalar, 0x0132, 0x0137, 0))
            .or_else(|| alternating(scalar, 0x0139, 0x0148, 1))
            .or_else(|| alternating(scalar, 0x014a, 0x0177, 0))
            .or_else(|| alternating(scalar, 0x0179, 0x017e, 1))
            .or_else(|| alternating(scalar, 0x0460, 0x0481, 0))
            .or_else(|| alternating(scalar, 0x048a, 0x04bf, 0))
            .or_else(|| alternating(scalar, 0x1e00, 0x1e95, 0))
            .or_else(|| alternating(scalar, 0x1ea0, 0x1eff, 0)),
    };
    shifted.unwrap_or(scalar)
}

/// The lowercase of `scalar` into `out`, returning how many scalars it is.
pub fn lowercase(scalar: u32, out: &mut [u32; 2]) -> usize {
    if scalar == 0x0130 {
        *out = [0x69, 0x0307];
        return 2;
    }
    out[0] = single(scalar);
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lower(text: &str) -> String {
        let mut result = String::new();
        for scalar in text.chars() {
            let mut out = [0; 2];
            let count = lowercase(scalar as u32, &mut out);
            for &mapped in &out[..count] {
                result.push(char::from_u32(mapped).unwrap());
            }
        }
        result
    }

    #[test]
    fn matches_unicodes_lowercase_for_the_scripts_it_covers() {
        let samples = [
            "Hello, WORLD 123",
            "\u{c9}COLE \u{c0}\u{df}\u{de}",
            "\u{100}\u{139}\u{14a}\u{178}\u{179}\u{17d}",
            "\u{391}\u{3a3}\u{3a9}\u{386}\u{388}\u{38c}\u{38e}\u{38f}",
            "\u{410}\u{42f}\u{400}\u{40f}\u{460}\u{48a}",
            "\u{531}\u{556}\u{10a0}\u{1e00}\u{1ea0}\u{ff21}",
            "\u{130}stanbul",
        ];
        for sample in samples {
            assert_eq!(lower(sample), sample.to_lowercase(), "{sample}");
        }
    }

    #[test]
    fn leaves_other_scripts_and_lowercase_alone() {
        for sample in ["abc", "\u{65e5}\u{672c}", "\u{1f600}", "\u{3c3}\u{3c2}"] {
            assert_eq!(lower(sample), sample);
        }
    }
}
