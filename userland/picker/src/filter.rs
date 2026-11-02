//! The picker's filters (docs/ux/file-picker-v0.3.md section 3): `<description>|<patterns>`,
//! the patterns `;`-separated with `*` and `?` as the shell's, ASCII case-insensitive.

use alloc::string::String;
use alloc::vec::Vec;

/// What the picker appends when no filter given is `*`.
pub const ALL_FILES: &str = "All files";
const EVERYTHING: &[u8] = b"*";
const MOST_PATTERNS: usize = 32;
const LONGEST_DESCRIPTION: usize = 64;

/// One filter: its words in the drop-down and the patterns a file name must match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    pub description: String,
    pub patterns: Vec<Vec<u8>>,
}

impl Filter {
    /// `text` as `<description>|<patterns>`; `None` without a `|`, with an empty description
    /// or with no pattern.
    pub fn parse(text: &[u8]) -> Option<Self> {
        let bar = text.iter().position(|&byte| byte == b'|')?;
        let description = trimmed(&text[..bar]);
        if description.is_empty() {
            return None;
        }
        let patterns: Vec<Vec<u8>> = text[bar + 1..]
            .split(|&byte| byte == b';')
            .map(trimmed)
            .filter(|pattern| !pattern.is_empty())
            .take(MOST_PATTERNS)
            .map(<[u8]>::to_vec)
            .collect();
        if patterns.is_empty() {
            return None;
        }
        let mut description = String::from_utf8_lossy(description).into_owned();
        cut(&mut description, LONGEST_DESCRIPTION);
        Some(Self {
            description,
            patterns,
        })
    }

    /// The filter every file passes, named [`ALL_FILES`].
    pub fn all_files() -> Self {
        Self {
            description: String::from(ALL_FILES),
            patterns: alloc::vec![EVERYTHING.to_vec()],
        }
    }

    /// Whether it lets every file through: one of its patterns is `*`.
    pub fn is_everything(&self) -> bool {
        self.patterns.iter().any(|pattern| pattern == EVERYTHING)
    }

    /// Whether the file `name` passes it.
    pub fn admits(&self, name: &[u8]) -> bool {
        self.patterns.iter().any(|pattern| matches(pattern, name))
    }
}

fn trimmed(bytes: &[u8]) -> &[u8] {
    let start = bytes.iter().position(|&byte| byte != b' ').unwrap_or(bytes.len());
    let end = bytes.iter().rposition(|&byte| byte != b' ').map_or(start, |end| end + 1);
    &bytes[start..end.max(start)]
}

/// `text` cut to at most `bytes` bytes on a character boundary, ending in `...` when cut.
pub fn cut(text: &mut String, bytes: usize) {
    if text.len() <= bytes {
        return;
    }
    let mut end = bytes.saturating_sub(3);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text.push_str("...");
}

/// Whether `name` matches the shell pattern `pattern` (`*` any run, `?` one byte), ignoring
/// ASCII case. Runs in time proportional to the product of the lengths at worst, without
/// recursion.
pub fn matches(pattern: &[u8], name: &[u8]) -> bool {
    let (mut at, mut from) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while from < name.len() {
        match pattern.get(at) {
            Some(b'*') => {
                star = Some((at, from));
                at += 1;
            }
            Some(&byte) if byte == b'?' || byte.eq_ignore_ascii_case(&name[from]) => {
                at += 1;
                from += 1;
            }
            _ => match star {
                Some((star_at, star_from)) => {
                    at = star_at + 1;
                    from = star_from + 1;
                    star = Some((star_at, star_from + 1));
                }
                None => return false,
            },
        }
    }
    pattern[at.min(pattern.len())..].iter().all(|&byte| byte == b'*')
}
