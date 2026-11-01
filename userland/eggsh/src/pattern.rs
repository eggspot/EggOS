//! Shell patterns (D182), as POSIX's pattern matching notation has them: `*` matches any text,
//! `?` one character, `[...]` one character of a set (`[a-z]`, `[!x]` or `[^x]` for any but
//! those, `[[:alpha:]]` and the other classes, in ASCII), and `\` makes the next character
//! plain. Quoted characters reach the matcher escaped with `\` (`Words::pattern`). A character
//! is a whole UTF-8 sequence. Used by pathname expansion, `case` and `${NAME#pattern}`.

/// Which end of a value `${NAME#pattern}` and `${NAME%pattern}` take the match from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// `#` and `##`: the start.
    Prefix,
    /// `%` and `%%`: the end.
    Suffix,
}

/// Whether `text` matches `pattern` whole.
pub fn matches(pattern: &[u8], text: &[u8]) -> bool {
    let mut p = 0;
    let mut t = 0;
    let mut star: Option<(usize, usize)> = None;
    loop {
        if p < pattern.len() {
            match pattern[p] {
                b'*' => {
                    p += 1;
                    star = Some((p, t));
                    continue;
                }
                _ if t < text.len() => {
                    if let Some((after, taken)) = single(pattern, p, text, t) {
                        p = after;
                        t += taken;
                        continue;
                    }
                }
                _ => {}
            }
        } else if t == text.len() {
            return true;
        }
        match star {
            Some((after, start)) if start < text.len() => {
                let next = start + character_length(text[start]).min(text.len() - start);
                star = Some((after, next));
                p = after;
                t = next;
            }
            _ => return false,
        }
    }
}

/// Whether `pattern` holds a `*`, `?` or `[` that is not escaped: whether it is more than its
/// own text.
pub fn has_special(pattern: &[u8]) -> bool {
    let mut at = 0;
    while at < pattern.len() {
        match pattern[at] {
            b'\\' => at += 2,
            b'*' | b'?' | b'[' => return true,
            _ => at += 1,
        }
    }
    false
}

/// `pattern` without its escapes: the text a pattern with no special character matches.
pub fn unescape<'b>(pattern: &[u8], buffer: &'b mut [u8]) -> Option<&'b [u8]> {
    let mut length = 0;
    let mut at = 0;
    while at < pattern.len() {
        let byte = match pattern[at] {
            b'\\' if at + 1 < pattern.len() => {
                at += 1;
                pattern[at]
            }
            byte => byte,
        };
        *buffer.get_mut(length)? = byte;
        length += 1;
        at += 1;
    }
    Some(&buffer[..length])
}

/// What is left of `value` once the shortest (or `longest`) part at `side` that `pattern`
/// matches is removed: `value` itself when nothing matches.
pub fn remove<'v>(value: &'v [u8], pattern: &[u8], side: Side, longest: bool) -> &'v [u8] {
    let length = value.len();
    let mut cut = |size: usize| match side {
        Side::Prefix => matches(pattern, &value[..size]).then(|| &value[size..]),
        Side::Suffix => matches(pattern, &value[length - size..]).then(|| &value[..length - size]),
    };
    let found = if longest {
        (0..=length).rev().find_map(&mut cut)
    } else {
        (0..=length).find_map(&mut cut)
    };
    found.unwrap_or(value)
}

fn character_length(first: u8) -> usize {
    match first {
        0xf0.. => 4,
        0xe0.. => 3,
        0xc0.. => 2,
        _ => 1,
    }
}

fn character_at(text: &[u8], at: usize) -> (u32, usize) {
    let length = character_length(text[at]).min(text.len() - at);
    let value = text[at..at + length]
        .iter()
        .fold(0u32, |value, &byte| (value << 8) | u32::from(byte));
    (value, length)
}

fn single(pattern: &[u8], p: usize, text: &[u8], t: usize) -> Option<(usize, usize)> {
    let (character, taken) = character_at(text, t);
    match pattern[p] {
        b'?' => Some((p + 1, taken)),
        b'[' => match bracket(pattern, p, character) {
            Some((after, true)) => Some((after, taken)),
            Some((_, false)) => None,
            None => (text[t] == b'[').then_some((p + 1, 1)),
        },
        b'\\' if p + 1 < pattern.len() => {
            let (wanted, length) = character_at(pattern, p + 1);
            (wanted == character).then_some((p + 1 + length, taken))
        }
        _ => {
            let (wanted, length) = character_at(pattern, p);
            (wanted == character).then_some((p + length, taken))
        }
    }
}

fn bracket(pattern: &[u8], open: usize, character: u32) -> Option<(usize, bool)> {
    let mut at = open + 1;
    let negated = matches!(pattern.get(at), Some(b'!' | b'^'));
    if negated {
        at += 1;
    }
    let first = at;
    let mut found = false;
    loop {
        let &byte = pattern.get(at)?;
        if byte == b']' && at > first {
            return Some((at + 1, found != negated));
        }
        if byte == b'[' && pattern.get(at + 1) == Some(&b':') {
            let name_start = at + 2;
            let length = pattern[name_start..]
                .windows(2)
                .position(|pair| pair == b":]")?;
            found |= in_class(&pattern[name_start..name_start + length], character);
            at = name_start + length + 2;
            continue;
        }
        let (low, after) = bracket_character(pattern, at)?;
        if pattern.get(after) == Some(&b'-') && pattern.get(after + 1).is_some_and(|&b| b != b']') {
            let (high, end) = bracket_character(pattern, after + 1)?;
            found |= (low..=high).contains(&character);
            at = end;
        } else {
            found |= low == character;
            at = after;
        }
    }
}

fn bracket_character(pattern: &[u8], at: usize) -> Option<(u32, usize)> {
    let at = match pattern[at] {
        b'\\' if at + 1 < pattern.len() => at + 1,
        _ => at,
    };
    pattern.get(at)?;
    let (value, length) = character_at(pattern, at);
    Some((value, at + length))
}

fn in_class(name: &[u8], character: u32) -> bool {
    let Ok(byte) = u8::try_from(character) else {
        return false;
    };
    match name {
        b"alpha" => byte.is_ascii_alphabetic(),
        b"digit" => byte.is_ascii_digit(),
        b"alnum" => byte.is_ascii_alphanumeric(),
        b"upper" => byte.is_ascii_uppercase(),
        b"lower" => byte.is_ascii_lowercase(),
        b"space" => byte.is_ascii_whitespace() || byte == 0x0b,
        b"blank" => byte == b' ' || byte == b'\t',
        b"punct" => byte.is_ascii_punctuation(),
        b"print" => byte.is_ascii_graphic() || byte == b' ',
        b"graph" => byte.is_ascii_graphic(),
        b"cntrl" => byte.is_ascii_control(),
        b"xdigit" => byte.is_ascii_hexdigit(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stars_and_question_marks_match_any_text_and_one_character() {
        for (pattern, text, expected) in [
            ("*", "", true),
            ("*", "anything", true),
            ("*.rs", "main.rs", true),
            ("*.rs", "main.rc", false),
            ("a*b*c", "aXXbYYc", true),
            ("a*b*c", "aXXbYY", false),
            ("?", "é", true),
            ("??", "é", false),
            ("a?c", "abc", true),
            ("a?c", "ac", false),
            ("*a", "baaa", true),
            ("", "", true),
            ("", "a", false),
        ] {
            assert_eq!(
                matches(pattern.as_bytes(), text.as_bytes()),
                expected,
                "{pattern} {text}"
            );
        }
    }

    #[test]
    fn brackets_match_sets_ranges_classes_and_their_complements() {
        for (pattern, text, expected) in [
            ("[abc]", "b", true),
            ("[abc]", "d", false),
            ("[a-c]x", "bx", true),
            ("[!a-c]", "d", true),
            ("[^a-c]", "a", false),
            ("[]]", "]", true),
            ("[a-]", "-", true),
            ("[[:digit:]][[:alpha:]]", "1x", true),
            ("[[:upper:]]", "a", false),
            ("[", "[", true),
            ("[a", "[a", true),
            ("[é]", "é", true),
        ] {
            assert_eq!(
                matches(pattern.as_bytes(), text.as_bytes()),
                expected,
                "{pattern} {text}"
            );
        }
    }

    #[test]
    fn escapes_make_special_characters_plain() {
        assert!(matches(br"\*", b"*"));
        assert!(!matches(br"\*", b"a"));
        assert!(matches(br"a\?", b"a?"));
        assert!(matches(br"[\]]", b"]"));
        assert!(has_special(b"a*"));
        assert!(!has_special(br"a\*\?\["));
        let mut buffer = [0; 8];
        assert_eq!(unescape(br"a\*b", &mut buffer), Some(&b"a*b"[..]));
        assert_eq!(unescape(b"too long here", &mut buffer), None);
    }

    #[test]
    fn removal_takes_the_shortest_or_longest_match_from_either_end() {
        let path = b"/usr/lib/file.tar.gz";
        assert_eq!(
            remove(path, b"*/", Side::Prefix, false),
            b"usr/lib/file.tar.gz"
        );
        assert_eq!(remove(path, b"*/", Side::Prefix, true), b"file.tar.gz");
        assert_eq!(
            remove(path, b".*", Side::Suffix, false),
            b"/usr/lib/file.tar"
        );
        assert_eq!(remove(path, b".*", Side::Suffix, true), b"/usr/lib/file");
        assert_eq!(remove(path, b"x*", Side::Prefix, false), path);
        assert_eq!(remove(b"abc", b"", Side::Suffix, true), b"abc");
    }
}
