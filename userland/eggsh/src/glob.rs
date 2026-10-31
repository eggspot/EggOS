//! Pathname expansion (D182): a word with an unquoted `*`, `?` or `[` becomes the paths it
//! matches, sorted by their bytes, or stays as it is when nothing matches, as POSIX says. Each
//! `/`-separated part of the pattern is matched against one directory's names; a name that
//! starts with `.` matches only a part that starts with a plain `.`, and `.` and `..` never
//! match. The paths go into fixed room ([`MATCHES_CAPACITY`], [`MAX_MATCHES`]): more is an
//! error, never a list cut short.

use crate::path::PATH_CAPACITY;
use crate::pattern::{has_special, matches, unescape};

/// The most bytes the paths one word expands to take.
pub const MATCHES_CAPACITY: usize = 8192;

/// The most paths one word expands to.
pub const MAX_MATCHES: usize = 256;

const NAMES_CAPACITY: usize = MATCHES_CAPACITY;

const MAX_NAMES: usize = MAX_MATCHES;

/// The directories pathname expansion reads.
pub trait Directories {
    /// Calls `each` with the name of every entry of the directory at `path` (with its NUL),
    /// `.` and `..` included or not; an error if it cannot be read.
    fn read_directory(
        &mut self,
        path: &[u8],
        each: &mut dyn FnMut(&[u8]),
    ) -> Result<(), Unreadable>;
    /// Whether `path` (with its NUL) names something.
    fn exists(&mut self, path: &[u8]) -> bool;
}

/// A directory could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unreadable;

/// The paths do not fit [`MATCHES_CAPACITY`] or [`MAX_MATCHES`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TooMany;

/// The paths a pattern matched.
#[derive(Debug)]
pub struct Matches {
    bytes: [u8; MATCHES_CAPACITY],
    length: usize,
    spans: [(u16, u16); MAX_MATCHES],
    count: usize,
}

impl Default for Matches {
    fn default() -> Self {
        Self {
            bytes: [0; MATCHES_CAPACITY],
            length: 0,
            spans: [(0, 0); MAX_MATCHES],
            count: 0,
        }
    }
}

impl Matches {
    /// How many paths matched.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether nothing matched.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Path `index`, in sorted order once [`expand`] returned.
    pub fn get(&self, index: usize) -> &[u8] {
        let (start, end) = self.spans[index];
        &self.bytes[usize::from(start)..usize::from(end)]
    }

    fn add(&mut self, path: &[u8]) -> Result<(), TooMany> {
        let end = self.length + path.len();
        if self.count == MAX_MATCHES || end > MATCHES_CAPACITY {
            return Err(TooMany);
        }
        self.bytes[self.length..end].copy_from_slice(path);
        self.spans[self.count] = (self.length as u16, end as u16);
        self.length = end;
        self.count += 1;
        Ok(())
    }

    fn sort(&mut self) {
        let bytes = &self.bytes;
        self.spans[..self.count].sort_unstable_by(|&(a, b), &(c, d)| {
            bytes[usize::from(a)..usize::from(b)].cmp(&bytes[usize::from(c)..usize::from(d)])
        });
    }
}

struct Path {
    bytes: [u8; PATH_CAPACITY],
    length: usize,
}

impl Path {
    fn push(&mut self, part: &[u8]) -> Result<(), TooMany> {
        let end = self.length + part.len();
        if end >= PATH_CAPACITY {
            return Err(TooMany);
        }
        self.bytes[self.length..end].copy_from_slice(part);
        self.length = end;
        Ok(())
    }

    fn with_nul(&mut self) -> &[u8] {
        self.bytes[self.length] = 0;
        &self.bytes[..=self.length]
    }
}

/// Expands `pattern` (quoted characters escaped with `\`) into `found`, sorted; nothing is
/// added when no path matches.
pub fn expand(
    pattern: &[u8],
    directories: &mut impl Directories,
    found: &mut Matches,
) -> Result<(), TooMany> {
    let mut path = Path {
        bytes: [0; PATH_CAPACITY],
        length: 0,
    };
    let rest = match pattern.first() {
        Some(b'/') => {
            path.push(b"/")?;
            without_leading_slashes(pattern)
        }
        _ => pattern,
    };
    walk(rest, &mut path, directories, found)?;
    found.sort();
    Ok(())
}

fn without_leading_slashes(bytes: &[u8]) -> &[u8] {
    let start = bytes.iter().take_while(|&&byte| byte == b'/').count();
    &bytes[start..]
}

fn walk(
    pattern: &[u8],
    path: &mut Path,
    directories: &mut impl Directories,
    found: &mut Matches,
) -> Result<(), TooMany> {
    let (part, rest, slash) = match pattern.iter().position(|&byte| byte == b'/') {
        Some(at) => (
            &pattern[..at],
            without_leading_slashes(&pattern[at..]),
            true,
        ),
        None => (pattern, &pattern[pattern.len()..], false),
    };
    let start = path.length;
    if !has_special(part) {
        let mut plain = [0u8; PATH_CAPACITY];
        let Some(plain) = unescape(part, &mut plain) else {
            return Ok(());
        };
        path.push(plain)?;
        let result = finish_part(rest, slash, path, directories, found);
        path.length = start;
        return result;
    }
    let mut names = [0u8; NAMES_CAPACITY];
    let mut spans = [(0u16, 0u16); MAX_NAMES];
    let mut count = 0;
    let mut length = 0;
    let mut overflow = false;
    let hidden = matches!(part, [b'.', ..] | [b'\\', b'.', ..]);
    let directory = match start {
        0 => &b".\0"[..],
        _ => path.with_nul(),
    };
    let read = directories.read_directory(directory, &mut |name| {
        if name == b"." || name == b".." || (name.first() == Some(&b'.') && !hidden) {
            return;
        }
        if !matches(part, name) {
            return;
        }
        let end = length + name.len();
        if count == MAX_NAMES || end > NAMES_CAPACITY {
            overflow = true;
            return;
        }
        names[length..end].copy_from_slice(name);
        spans[count] = (length as u16, end as u16);
        length = end;
        count += 1;
    });
    if read.is_err() {
        return Ok(());
    }
    if overflow {
        return Err(TooMany);
    }
    for &(name_start, name_end) in &spans[..count] {
        path.push(&names[usize::from(name_start)..usize::from(name_end)])?;
        let result = finish_part(rest, slash, path, directories, found);
        path.length = start;
        result?;
    }
    Ok(())
}

fn finish_part(
    rest: &[u8],
    slash: bool,
    path: &mut Path,
    directories: &mut impl Directories,
    found: &mut Matches,
) -> Result<(), TooMany> {
    if !rest.is_empty() {
        path.push(b"/")?;
        return walk(rest, path, directories, found);
    }
    if slash {
        path.push(b"/")?;
    }
    if directories.exists(path.with_nul()) {
        found.add(&path.bytes[..path.length])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tree(Vec<&'static str>);

    impl Directories for Tree {
        fn read_directory(
            &mut self,
            path: &[u8],
            each: &mut dyn FnMut(&[u8]),
        ) -> Result<(), Unreadable> {
            let path = std::str::from_utf8(&path[..path.len() - 1]).unwrap();
            let prefix = match path {
                "." => String::new(),
                "/" => "/".to_owned(),
                _ => format!("{}/", path.trim_end_matches('/')),
            };
            let listed = matches!(path, "." | "/") || self.0.contains(&path.trim_end_matches('/'));
            if !listed {
                return Err(Unreadable);
            }
            each(b".");
            each(b"..");
            for entry in &self.0 {
                if let Some(name) = entry.strip_prefix(&prefix)
                    && !name.contains('/')
                    && !name.is_empty()
                {
                    each(name.as_bytes());
                }
            }
            Ok(())
        }

        fn exists(&mut self, path: &[u8]) -> bool {
            let path = std::str::from_utf8(&path[..path.len() - 1]).unwrap();
            let path = path.trim_end_matches('/');
            path.is_empty() || self.0.contains(&path)
        }
    }

    fn tree() -> Tree {
        Tree(vec![
            "b.rs",
            "a.rs",
            ".hidden.rs",
            "notes",
            "src",
            "src/main.rs",
            "src/lib.rs",
            "src/x",
            "/bin",
            "/bin/ls",
            "/bin/cat",
            "/etc",
        ])
    }

    fn expanded(pattern: &str) -> Vec<String> {
        let mut matches = Matches::default();
        expand(pattern.as_bytes(), &mut tree(), &mut matches).unwrap();
        (0..matches.len())
            .map(|index| String::from_utf8(matches.get(index).to_vec()).unwrap())
            .collect()
    }

    #[test]
    fn a_pattern_becomes_the_sorted_names_it_matches() {
        assert_eq!(expanded("*.rs"), ["a.rs", "b.rs"]);
        assert_eq!(expanded("?.rs"), ["a.rs", "b.rs"]);
        assert_eq!(expanded("[b]*"), ["b.rs"]);
        assert_eq!(expanded("*.c"), Vec::<String>::new());
    }

    #[test]
    fn names_starting_with_a_dot_need_a_plain_dot() {
        assert_eq!(expanded(".*"), [".hidden.rs"]);
        assert_eq!(expanded("*"), ["a.rs", "b.rs", "notes", "src"]);
    }

    #[test]
    fn each_part_of_a_path_is_matched_in_its_directory() {
        assert_eq!(expanded("src/*.rs"), ["src/lib.rs", "src/main.rs"]);
        assert_eq!(expanded("*/main.rs"), ["src/main.rs"]);
        assert_eq!(expanded("/bin/*"), ["/bin/cat", "/bin/ls"]);
        assert_eq!(expanded("/*/ls"), ["/bin/ls"]);
        assert_eq!(expanded("s*/"), ["src/"]);
    }

    #[test]
    fn escaped_characters_match_only_themselves() {
        assert_eq!(expanded(r"\*.rs"), Vec::<String>::new());
        assert_eq!(expanded(r"a\.rs*"), ["a.rs"]);
    }

    #[test]
    fn too_many_paths_are_an_error() {
        let names: Vec<&'static str> = (0..MAX_MATCHES + 1)
            .map(|index| &*Box::leak(format!("f{index}").into_boxed_str()))
            .collect();
        let mut matches = Matches::default();
        let result = expand(b"f*", &mut Tree(names), &mut matches);
        assert_eq!(result, Err(TooMany));
    }
}
