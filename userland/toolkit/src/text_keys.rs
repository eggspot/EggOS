//! The word and line keys of text fields and text areas (docs/ux/toolkit-v0.6.md section 5.2):
//! which key does what in each desktop profile, and where a word starts and ends.

use crate::input::key;

/// What a word or line key does to the caret, or deletes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextKey {
    WordStart,
    WordEnd,
    DeleteToWordStart,
    DeleteToWordEnd,
    LineStart,
    LineEnd,
    TextStart,
    TextEnd,
    DeleteToLineStart,
}

impl TextKey {
    /// Whether it deletes rather than moves.
    pub fn deletes(self) -> bool {
        matches!(
            self,
            Self::DeleteToWordStart | Self::DeleteToWordEnd | Self::DeleteToLineStart
        )
    }
}

/// The modifiers held with a key, Ctrl and Super told apart (Super is Cmd in the macOS profile).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HeldKeys {
    pub control: bool,
    pub alt: bool,
    pub command: bool,
}

/// The word or line key `code` is with `held`, in the macOS profile's column when `mac`.
pub fn text_key(code: u16, held: HeldKeys, mac: bool) -> Option<TextKey> {
    let word = match mac {
        true => held.alt || held.control,
        false => held.control || held.command,
    };
    let line = mac && held.command;
    let modifier = held.control || held.command;
    Some(match code {
        key::LEFT if line => TextKey::LineStart,
        key::RIGHT if line => TextKey::LineEnd,
        key::UP if line => TextKey::TextStart,
        key::DOWN if line => TextKey::TextEnd,
        key::BACKSPACE if line => TextKey::DeleteToLineStart,
        key::LEFT if word => TextKey::WordStart,
        key::RIGHT if word => TextKey::WordEnd,
        key::BACKSPACE if word => TextKey::DeleteToWordStart,
        key::DELETE if word => TextKey::DeleteToWordEnd,
        key::HOME if modifier => TextKey::TextStart,
        key::END if modifier => TextKey::TextEnd,
        _ => return None,
    })
}

/// Whether key `code` with `held` belongs to the program rather than the text: Alt with an
/// arrow outside the macOS profile (Back and Forward), and Mod with Up or Down there.
pub fn left_to_program(code: u16, held: HeldKeys, mac: bool) -> bool {
    if mac {
        return false;
    }
    let arrow = matches!(code, key::LEFT | key::RIGHT | key::UP | key::DOWN);
    let vertical = matches!(code, key::UP | key::DOWN);
    (arrow && held.alt && !held.control && !held.command)
        || (vertical && (held.control || held.command))
}

/// The three kinds of character a word move and a double-click tell apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordClass {
    Space,
    Word,
    Sign,
}

/// Which kind `character` is: space, tab and line breaks are space; ASCII letters and digits,
/// `_` and every character beyond ASCII are word characters; the rest are signs.
pub fn word_class(character: char) -> WordClass {
    match character {
        ' ' | '\t' | '\n' | '\r' => WordClass::Space,
        character if character.is_ascii_alphanumeric() || character == '_' => WordClass::Word,
        character if !character.is_ascii() => WordClass::Word,
        _ => WordClass::Sign,
    }
}

/// Where a move to the right from byte `at` of `words` stops: past any space, at the end of
/// the run of one kind that follows.
pub fn word_end(words: &str, at: usize) -> usize {
    let at = at.min(words.len());
    let mut rest = words[at..]
        .char_indices()
        .skip_while(|&(_, character)| word_class(character) == WordClass::Space)
        .peekable();
    let Some(&(_, first)) = rest.peek() else {
        return words.len();
    };
    let kind = word_class(first);
    rest.take_while(|&(_, character)| word_class(character) == kind)
        .last()
        .map_or(at, |(index, character)| at + index + character.len_utf8())
}

/// Where a move to the left from byte `at` of `words` stops: before any space, at the start of
/// the run of one kind that precedes it.
pub fn word_start(words: &str, at: usize) -> usize {
    let at = at.min(words.len());
    let mut before = words[..at]
        .char_indices()
        .rev()
        .skip_while(|&(_, character)| word_class(character) == WordClass::Space)
        .peekable();
    let Some(&(_, first)) = before.peek() else {
        return 0;
    };
    let kind = word_class(first);
    before
        .take_while(|&(_, character)| word_class(character) == kind)
        .last()
        .map_or(at, |(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_moves_stop_at_the_ends_of_words_and_runs_of_signs() {
        let words = "let a = b->c...  done";
        assert_eq!(word_end(words, 0), 3);
        assert_eq!(word_end(words, 3), 5);
        assert_eq!(word_end(words, 9), 11);
        assert_eq!(word_end(words, 11), 12);
        assert_eq!(word_end(words, 12), 15);
        assert_eq!(word_end(words, 15), words.len());
        assert_eq!(word_start(words, words.len()), 17);
        assert_eq!(word_start(words, 17), 12);
        assert_eq!(word_start(words, 3), 0);
        assert_eq!(word_start(words, 0), 0);
        assert_eq!(word_end("café au", 0), "café".len());
    }

    #[test]
    fn each_profile_has_its_own_column() {
        let control = HeldKeys {
            control: true,
            ..HeldKeys::default()
        };
        let alt = HeldKeys {
            alt: true,
            ..HeldKeys::default()
        };
        let command = HeldKeys {
            command: true,
            ..HeldKeys::default()
        };
        assert_eq!(text_key(key::RIGHT, control, false), Some(TextKey::WordEnd));
        assert_eq!(text_key(key::RIGHT, alt, false), None);
        assert!(left_to_program(key::LEFT, alt, false));
        assert_eq!(text_key(key::LEFT, alt, true), Some(TextKey::WordStart));
        assert_eq!(text_key(key::LEFT, command, true), Some(TextKey::LineStart));
        assert_eq!(text_key(key::DOWN, command, true), Some(TextKey::TextEnd));
        assert_eq!(
            text_key(key::BACKSPACE, command, true),
            Some(TextKey::DeleteToLineStart)
        );
        assert_eq!(
            text_key(key::BACKSPACE, command, false),
            Some(TextKey::DeleteToWordStart)
        );
        assert!(left_to_program(key::UP, control, false));
        assert!(!left_to_program(key::UP, control, true));
    }
}
