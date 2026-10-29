//! Words (docs/ux/text-editor-v0.3.md section 5), for Ctrl+arrows and double-click: a run of
//! ASCII letters, digits and `_`, or of characters beyond ASCII, is a word; every other
//! character that is not a space or a tab is a word of its own; a run of spaces and tabs is not
//! a word, but is what a double-click on it selects.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Word,
    Space,
    Break,
    Mark,
}

fn class(byte: u8) -> Class {
    match byte {
        b' ' | b'\t' => Class::Space,
        b'\n' => Class::Break,
        b'_' | b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z' | 0x80.. => Class::Word,
        _ => Class::Mark,
    }
}

fn is_gap(byte: u8) -> bool {
    matches!(class(byte), Class::Space | Class::Break)
}

/// Where the next word starts after byte `at` of `text`, as Ctrl+Right moves: past the word
/// at `at`, then past spaces and line breaks; the end at the last word.
pub fn next_word_start(text: &[u8], at: usize) -> usize {
    let mut at = at.min(text.len());
    match text.get(at).map(|&byte| class(byte)) {
        Some(Class::Word) => {
            while at < text.len() && class(text[at]) == Class::Word {
                at += 1;
            }
        }
        Some(Class::Mark) => at += 1,
        _ => {}
    }
    while at < text.len() && is_gap(text[at]) {
        at += 1;
    }
    at
}

/// Where the word before byte `at` of `text` starts, as Ctrl+Left moves: back over spaces and
/// line breaks, then to the start of the word there.
pub fn previous_word_start(text: &[u8], at: usize) -> usize {
    let mut at = at.min(text.len());
    while at > 0 && is_gap(text[at - 1]) {
        at -= 1;
    }
    match at.checked_sub(1).map(|before| class(text[before])) {
        Some(Class::Word) => {
            while at > 0 && class(text[at - 1]) == Class::Word {
                at -= 1;
            }
        }
        Some(_) => at -= 1,
        None => {}
    }
    at
}

/// What a double-click on the character at byte `at` of `text` selects: its word, the mark
/// itself, or its run of spaces and tabs; nothing at a line break or the end.
pub fn run_at(text: &[u8], at: usize) -> (usize, usize) {
    let Some(&byte) = text.get(at) else {
        return (at.min(text.len()), at.min(text.len()));
    };
    let kind = class(byte);
    match kind {
        Class::Break => (at, at),
        Class::Mark => (at, at + 1),
        Class::Word | Class::Space => {
            let mut start = at;
            while start > 0 && class(text[start - 1]) == kind {
                start -= 1;
            }
            let mut end = at;
            while end < text.len() && class(text[end]) == kind {
                end += 1;
            }
            (start, end)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_right_goes_to_the_next_word_s_start() {
        let text = b"let x_1 = a.b;\n  next";
        assert_eq!(next_word_start(text, 0), 4);
        assert_eq!(next_word_start(text, 4), 8);
        assert_eq!(next_word_start(text, 8), 10);
        assert_eq!(next_word_start(text, 10), 11);
        assert_eq!(next_word_start(text, 11), 12);
        assert_eq!(next_word_start(text, 13), 17);
        assert_eq!(next_word_start(text, 17), text.len());
        assert_eq!(next_word_start(text, text.len()), text.len());
    }

    #[test]
    fn ctrl_left_goes_to_the_word_before() {
        let text = b"let x_1 = a.b;\n  next";
        assert_eq!(previous_word_start(text, text.len()), 17);
        assert_eq!(previous_word_start(text, 17), 13);
        assert_eq!(previous_word_start(text, 13), 12);
        assert_eq!(previous_word_start(text, 11), 10);
        assert_eq!(previous_word_start(text, 8), 4);
        assert_eq!(previous_word_start(text, 2), 0);
        assert_eq!(previous_word_start(text, 0), 0);
    }

    #[test]
    fn a_double_click_selects_the_run_under_it() {
        let text = "café  ok.\n".as_bytes();
        assert_eq!(run_at(text, 1), (0, 5));
        assert_eq!(run_at(text, 5), (5, 7));
        assert_eq!(run_at(text, 7), (7, 9));
        assert_eq!(run_at(text, 9), (9, 10));
        assert_eq!(run_at(text, 10), (10, 10));
        assert_eq!(run_at(text, 11), (11, 11));
        assert_eq!(run_at(text, 40), (11, 11));
    }
}
