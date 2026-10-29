//! The file picker's rules as pure functions (D145, `docs/ux/windows-controls-v0.3.md` section
//! 7.1): which filter applies, which names it shows, and the offsets `OPENFILENAMEW` reports.

use crate::find;

/// The description and pattern of filter `index` (1-based; 0 or past the end is the first) in
/// `filters`, `lpstrFilter`'s pairs of NUL-terminated strings ended by an empty one; as ranges
/// of `filters`.
pub fn filter_pair(
    filters: &[u16],
    index: u32,
) -> Option<(core::ops::Range<usize>, core::ops::Range<usize>)> {
    let mut strings: [core::ops::Range<usize>; 2] = [0..0, 0..0];
    let mut pairs = 0u32;
    let mut first = None;
    let mut start = 0;
    let mut part = 0;
    for (at, &unit) in filters.iter().enumerate() {
        if unit != 0 {
            continue;
        }
        if at == start {
            break;
        }
        strings[part] = start..at;
        start = at + 1;
        part += 1;
        if part == 2 {
            pairs += 1;
            part = 0;
            if pairs == 1 {
                first = Some((strings[0].clone(), strings[1].clone()));
            }
            if pairs == index {
                return Some((strings[0].clone(), strings[1].clone()));
            }
        }
    }
    first
}

/// Whether file `name` matches `pattern`, wildcard patterns separated by `;` (every name for
/// an empty one), compared as `FindFirstFileW` compares (UTF-8 both).
pub fn pattern_matches(pattern: &[u8], name: &[u8]) -> bool {
    pattern.is_empty()
        || pattern
            .split(|byte| *byte == b';')
            .map(|alternative| alternative.trim_ascii())
            .any(|alternative| !alternative.is_empty() && find::matches(alternative, name))
}

/// Whether the picker leaves `name` out: hidden names start with `.`.
pub fn is_hidden(name: &[u8]) -> bool {
    name.first() == Some(&b'.')
}

/// Whether the last component of `path` has an extension.
pub fn has_extension(path: &[u8]) -> bool {
    let name = path.rsplit(|byte| *byte == b'/').next().unwrap_or(path);
    name.iter().skip(1).any(|byte| *byte == b'.')
}

/// `nFileOffset` and `nFileExtension` of the Windows path `path`: where its name starts, and
/// where its extension starts (the terminating NUL's offset when it has none).
pub fn offsets(path: &[u16]) -> (u16, u16) {
    let name = path
        .iter()
        .rposition(|unit| *unit == u16::from(b'\\'))
        .map_or(0, |at| at + 1);
    let extension = path[name..]
        .iter()
        .rposition(|unit| *unit == u16::from(b'.'))
        .filter(|at| *at > 0)
        .map_or(path.len(), |at| name + at + 1);
    (name as u16, extension as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    #[test]
    fn the_filter_index_picks_a_pair_and_falls_back_to_the_first() {
        let filters = units("Text files (*.txt)\0*.txt\0All files\0*.*\0\0");
        let (description, pattern) = filter_pair(&filters, 2).unwrap();
        assert_eq!(&filters[description], &units("All files")[..]);
        assert_eq!(&filters[pattern], &units("*.*")[..]);
        for index in [0, 1, 3] {
            let (description, _) = filter_pair(&filters, index).unwrap();
            assert_eq!(&filters[description], &units("Text files (*.txt)")[..]);
        }
        assert_eq!(filter_pair(&units("\0\0"), 1), None);
    }

    #[test]
    fn patterns_take_alternatives_and_hidden_names_are_left_out() {
        assert!(pattern_matches(b"*.txt;*.md", b"NOTES.MD"));
        assert!(!pattern_matches(b"*.txt;*.md", b"note.rs"));
        assert!(pattern_matches(b"", b"anything"));
        assert!(is_hidden(b".profile") && !is_hidden(b"note.txt"));
    }

    #[test]
    fn extensions_and_the_reported_offsets() {
        assert!(has_extension(b"/tmp/note.txt"));
        assert!(!has_extension(b"/tmp.d/note"));
        assert!(!has_extension(b"/tmp/.hidden"));
        let path = units(r"C:\tmp\win-controls\note.txt");
        assert_eq!(offsets(&path), (20, 25));
        assert_eq!(offsets(&units(r"C:\tmp\note")), (7, 11));
    }
}
