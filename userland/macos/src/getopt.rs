//! Command-line option parsing as `getopt` and `getopt_long` do it, over the arguments as byte
//! strings: short options grouped (`-vn3`), with an argument attached or in the next word,
//! optional arguments (`x::`, attached only), long options with `=value` or the next word, and
//! unique abbreviations of long names. Parsing stops at the first word that is not an option
//! and after `--`, as BSD's `getopt` does; words are not reordered.

/// Whether a long option takes an argument (`struct option`'s `has_arg`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Takes {
    Nothing,
    Required,
    Optional,
}

impl Takes {
    pub fn from_c(has_arg: i32) -> Self {
        match has_arg {
            1 => Self::Required,
            2 => Self::Optional,
            _ => Self::Nothing,
        }
    }
}

/// Where parsing is: `index` is `optind`, `offset` the next byte of a group of short options
/// (0 between words).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cursor {
    pub index: usize,
    pub offset: usize,
}

/// An option's argument: a word, from a byte offset.
pub type Value = Option<(usize, usize)>;

/// What the next call finds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Found {
    Done,
    Short(u8, Value),
    Long(usize, Value),
    UnknownShort(u8),
    MissingShort(u8),
    UnknownLong(usize),
    AmbiguousLong(usize),
    MissingLong(usize),
    UnexpectedLong(usize),
}

fn short_spec(spec: &[u8]) -> &[u8] {
    let spec = spec
        .strip_prefix(b"+")
        .or(spec.strip_prefix(b"-"))
        .unwrap_or(spec);
    spec.strip_prefix(b":").unwrap_or(spec)
}

/// Whether `spec` asks for `:` rather than `?` for a missing argument (and no messages).
pub fn quiet(spec: &[u8]) -> bool {
    let spec = spec
        .strip_prefix(b"+")
        .or(spec.strip_prefix(b"-"))
        .unwrap_or(spec);
    spec.starts_with(b":")
}

fn find_long<'a>(
    names: impl Iterator<Item = (&'a [u8], Takes)> + Clone,
    name: &[u8],
) -> Result<(usize, Takes), bool> {
    if let Some((index, (_, takes))) = names
        .clone()
        .enumerate()
        .find(|(_, (long, _))| *long == name)
    {
        return Ok((index, takes));
    }
    let mut matches = names
        .enumerate()
        .filter(|(_, (long, _))| long.starts_with(name));
    match (matches.next(), matches.next()) {
        (Some((index, (_, takes))), None) => Ok((index, takes)),
        (Some(_), Some(_)) => Err(true),
        _ => Err(false),
    }
}

/// The next option in the words `arguments` returns (word 0 is the program's name) for the short options in
/// `spec` and the long ones in `long` (name and argument, in the program's order), moving
/// `cursor` past it.
pub fn next<'a, 'b>(
    arguments: impl Fn(usize) -> Option<&'b [u8]>,
    spec: &[u8],
    long: impl Iterator<Item = (&'a [u8], Takes)> + Clone,
    has_long: bool,
    cursor: &mut Cursor,
) -> Found {
    if cursor.offset == 0 {
        let Some(word) = arguments(cursor.index) else {
            return Found::Done;
        };
        if word == b"--" {
            cursor.index += 1;
            return Found::Done;
        }
        if word.len() < 2 || word[0] != b'-' {
            return Found::Done;
        }
        if has_long && word.starts_with(b"--") {
            let at = cursor.index;
            cursor.index += 1;
            let body = &word[2..];
            let (name, attached) = match body.iter().position(|&b| b == b'=') {
                Some(equals) => (&body[..equals], Some(2 + equals + 1)),
                None => (body, None),
            };
            return match find_long(long, name) {
                Err(true) => Found::AmbiguousLong(at),
                Err(false) => Found::UnknownLong(at),
                Ok((index, takes)) => match (takes, attached) {
                    (Takes::Nothing, Some(_)) => Found::UnexpectedLong(index),
                    (Takes::Nothing, None) | (Takes::Optional, None) => Found::Long(index, None),
                    (_, Some(offset)) => Found::Long(index, Some((at, offset))),
                    (Takes::Required, None) => {
                        if arguments(cursor.index).is_some() {
                            cursor.index += 1;
                            Found::Long(index, Some((cursor.index - 1, 0)))
                        } else {
                            Found::MissingLong(index)
                        }
                    }
                },
            };
        }
        cursor.offset = 1;
    }
    let word = arguments(cursor.index).unwrap_or_default();
    let Some(&option) = word.get(cursor.offset) else {
        *cursor = Cursor {
            index: cursor.index + 1,
            offset: 0,
        };
        return Found::Done;
    };
    cursor.offset += 1;
    let at_end = cursor.offset >= word.len();
    let finish_word = |cursor: &mut Cursor| {
        cursor.index += 1;
        cursor.offset = 0;
    };
    let spec = short_spec(spec);
    let Some(position) = spec
        .iter()
        .position(|&b| b == option)
        .filter(|_| option != b':')
    else {
        if at_end {
            finish_word(cursor);
        }
        return Found::UnknownShort(option);
    };
    let takes = match (spec.get(position + 1), spec.get(position + 2)) {
        (Some(b':'), Some(b':')) => Takes::Optional,
        (Some(b':'), _) => Takes::Required,
        _ => Takes::Nothing,
    };
    match takes {
        Takes::Nothing => {
            if at_end {
                finish_word(cursor);
            }
            Found::Short(option, None)
        }
        Takes::Optional => {
            let value = (!at_end).then_some((cursor.index, cursor.offset));
            finish_word(cursor);
            Found::Short(option, value)
        }
        Takes::Required if !at_end => {
            let value = (cursor.index, cursor.offset);
            finish_word(cursor);
            Found::Short(option, Some(value))
        }
        Takes::Required => {
            finish_word(cursor);
            if arguments(cursor.index).is_some() {
                cursor.index += 1;
                Found::Short(option, Some((cursor.index - 1, 0)))
            } else {
                Found::MissingShort(option)
            }
        }
    }
}
