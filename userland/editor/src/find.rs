//! Finding text (docs/ux/text-editor-v0.3.md section 6): the search text's bytes, ASCII
//! letters matching either case unless the case must match; forward from a place or backward
//! from one, wrapping around the document's ends; and which match a hit is among all of them,
//! counted from the top without overlaps.

/// Whether a hit came after wrapping around the document's end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wrap {
    /// It did not wrap.
    No,
    /// Searching forward found nothing after the start and went on from the top.
    FromTop,
    /// Searching backward found nothing before the start and went on from the end.
    FromEnd,
}

/// A hit: where it starts and whether the search wrapped to find it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    pub at: usize,
    pub wrap: Wrap,
}

fn same(a: u8, b: u8, match_case: bool) -> bool {
    match match_case {
        true => a == b,
        false => a.eq_ignore_ascii_case(&b),
    }
}

/// Whether `needle` (not empty) is at byte `at` of `text`.
pub fn is_at(text: &[u8], at: usize, needle: &[u8], match_case: bool) -> bool {
    text.get(at..at + needle.len()).is_some_and(|found| {
        found
            .iter()
            .zip(needle)
            .all(|(&a, &b)| same(a, b, match_case))
    })
}

/// The first match of `needle` starting at or after `from`.
pub fn next(text: &[u8], from: usize, needle: &[u8], match_case: bool) -> Option<usize> {
    if needle.is_empty() || needle.len() > text.len() {
        return None;
    }
    (from..=text.len() - needle.len()).find(|&at| is_at(text, at, needle, match_case))
}

/// The last match of `needle` starting before `before`.
pub fn previous(text: &[u8], before: usize, needle: &[u8], match_case: bool) -> Option<usize> {
    if needle.is_empty() || needle.len() > text.len() {
        return None;
    }
    let last = text.len() - needle.len();
    (0..before.min(last + 1))
        .rev()
        .find(|&at| is_at(text, at, needle, match_case))
}

/// Searches forward from `from` (or backward from it when `!forward`), wrapping around once.
pub fn search(
    text: &[u8],
    from: usize,
    needle: &[u8],
    match_case: bool,
    forward: bool,
) -> Option<Hit> {
    match forward {
        true => next(text, from, needle, match_case)
            .map(|at| Hit { at, wrap: Wrap::No })
            .or_else(|| {
                next(text, 0, needle, match_case).map(|at| Hit {
                    at,
                    wrap: Wrap::FromTop,
                })
            }),
        false => previous(text, from, needle, match_case)
            .map(|at| Hit { at, wrap: Wrap::No })
            .or_else(|| {
                previous(text, text.len() + 1, needle, match_case).map(|at| Hit {
                    at,
                    wrap: Wrap::FromEnd,
                })
            }),
    }
}

/// Which match the one at `at` is, 1-based, among the matches counted from the top without
/// overlaps, and how many there are.
pub fn rank(text: &[u8], at: usize, needle: &[u8], match_case: bool) -> (usize, usize) {
    let mut from = 0;
    let mut index = 0;
    let mut total = 0;
    while let Some(found) = next(text, from, needle, match_case) {
        total += 1;
        if found <= at {
            index = total;
        }
        from = found + needle.len();
    }
    (index, total)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &[u8] = b"Egg eggs EGG";

    #[test]
    fn letters_match_either_case_unless_the_case_must_match() {
        assert_eq!(next(TEXT, 0, b"egg", false), Some(0));
        assert_eq!(next(TEXT, 1, b"egg", false), Some(4));
        assert_eq!(next(TEXT, 0, b"egg", true), Some(4));
        assert_eq!(next(TEXT, 5, b"egg", true), None);
        assert_eq!(next(TEXT, 0, b"", false), None);
        assert_eq!(next(b"eg", 0, b"egg", false), None);
        assert_eq!(previous(TEXT, TEXT.len(), b"egg", false), Some(9));
        assert_eq!(previous(TEXT, 9, b"egg", false), Some(4));
        assert_eq!(previous(TEXT, 0, b"egg", false), None);
    }

    #[test]
    fn a_search_wraps_around_the_ends() {
        assert_eq!(
            search(TEXT, 10, b"egg", false, true),
            Some(Hit {
                at: 0,
                wrap: Wrap::FromTop
            })
        );
        assert_eq!(
            search(TEXT, 0, b"egg", false, false),
            Some(Hit {
                at: 9,
                wrap: Wrap::FromEnd
            })
        );
        assert_eq!(
            search(TEXT, 3, b"egg", false, true),
            Some(Hit {
                at: 4,
                wrap: Wrap::No
            })
        );
        assert_eq!(search(TEXT, 0, b"duck", false, true), None);
        assert_eq!(search(TEXT, 5, b"egg", true, true).unwrap().at, 4);
    }

    #[test]
    fn matches_are_counted_from_the_top_without_overlaps() {
        assert_eq!(rank(TEXT, 4, b"egg", false), (2, 3));
        assert_eq!(rank(TEXT, 9, b"EGG", true), (1, 1));
        assert_eq!(rank(b"aaaa", 0, b"aa", false), (1, 2));
        assert_eq!(rank(b"aaaa", 2, b"aa", false), (2, 2));
        assert_eq!(rank(b"aaaa", 1, b"aa", false), (1, 2));
    }
}
