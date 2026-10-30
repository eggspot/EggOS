//! Command-line option parsing as macOS's `getopt`, `getopt_long` and `getopt_long_only` do
//! it, over the arguments as byte strings: short options grouped (`-vn3`), with an argument
//! attached or in the next word, optional arguments (`x::`, attached only), long options with
//! `=value` or the next word, and unique abbreviations of long names (D190). `getopt` stops at
//! the first word that is not an option; the long forms move such words after the options as
//! they go, unless the option string starts with `+` or `POSIXLY_CORRECT` is set, and hand
//! each back in turn as option 1 when the option string starts with `-`.

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

/// A long option: its name, its argument and what it gives back (`struct option`'s `flag`
/// address and `val`), which tells options that differ only in name apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Long<'a> {
    pub name: &'a [u8],
    pub takes: Takes,
    pub gives: (usize, i32),
}

/// Which function parses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Short,
    Long,
    LongOnly,
}

/// What happens to a word that is not an option.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    /// Parsing stops there (`getopt`, a `+` option string, `POSIXLY_CORRECT`).
    Stop,
    /// It moves after the options (the long forms).
    Permute,
    /// It comes back as option 1 with the word as its argument (a `-` option string).
    InOrder,
}

/// How a call parses: its function, the order of words, whether a missing argument gives `:`
/// without a message, and the short options without the option string's prefixes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rules<'s> {
    pub kind: Kind,
    pub order: Order,
    pub quiet: bool,
    pub shorts: &'s [u8],
}

impl<'s> Rules<'s> {
    pub fn new(kind: Kind, spec: &'s [u8], posixly_correct: bool) -> Self {
        let (order, rest) = match spec.first() {
            Some(b'-') if kind != Kind::Short => (Order::InOrder, &spec[1..]),
            Some(b'+' | b'-') => (Order::Stop, &spec[1..]),
            _ if kind == Kind::Short || posixly_correct => (Order::Stop, spec),
            _ => (Order::Permute, spec),
        };
        Self {
            kind,
            order,
            quiet: rest.starts_with(b":"),
            shorts: rest.strip_prefix(b":").unwrap_or(rest),
        }
    }
}

/// The program's words (word 0 is its name), which the long forms reorder.
pub trait Words {
    fn count(&self) -> usize;
    /// Word `index`, below [`Words::count`].
    fn word(&self, index: usize) -> &[u8];
    /// Moves the words `middle..end` before the words `start..middle`.
    fn rotate(&mut self, start: usize, middle: usize, end: usize);
}

impl Words for [&[u8]] {
    fn count(&self) -> usize {
        self.len()
    }

    fn word(&self, index: usize) -> &[u8] {
        self[index]
    }

    fn rotate(&mut self, start: usize, middle: usize, end: usize) {
        self[start..end].rotate_left(middle - start);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Skipped {
    start: usize,
    end: Option<usize>,
}

/// Where parsing is: `index` is `optind`, `offset` the next byte of a group of short options
/// (0 between words); words skipped by the long forms wait to move after the options found
/// after them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub index: usize,
    pub offset: usize,
    skipped: Option<Skipped>,
}

impl Cursor {
    pub const fn at(index: usize) -> Self {
        Self {
            index,
            offset: 0,
            skipped: None,
        }
    }
}

/// An option's argument: a word, from a byte offset.
pub type Value = Option<(usize, usize)>;

/// A long option as it was typed, for messages: `dashes` dashes, then the bytes `start..end`
/// of word `index`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Typed {
    pub index: usize,
    pub dashes: usize,
    pub start: usize,
    pub end: usize,
}

/// What the next call finds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Found {
    Done,
    Operand(usize),
    Short(u8, Value),
    Long(usize, Value),
    UnknownShort(u8),
    MissingShort(u8),
    UnknownLong(Typed),
    AmbiguousLong(Typed),
    MissingLong(usize, Typed),
    UnexpectedLong(usize, Typed),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Match {
    One(usize, Takes),
    Ambiguous,
    Missing,
}

fn find_long<'a>(
    longs: impl Iterator<Item = Long<'a>> + Clone,
    name: &[u8],
    exact_only: bool,
) -> Match {
    if let Some((index, option)) = longs
        .clone()
        .enumerate()
        .find(|(_, option)| option.name == name)
    {
        return Match::One(index, option.takes);
    }
    if exact_only {
        return Match::Missing;
    }
    let mut prefixed = longs
        .enumerate()
        .filter(|(_, option)| option.name.starts_with(name));
    let Some((index, first)) = prefixed.next() else {
        return Match::Missing;
    };
    if prefixed.any(|(_, other)| other.takes != first.takes || other.gives != first.gives) {
        return Match::Ambiguous;
    }
    Match::One(index, first.takes)
}

/// The next option in `words` for `rules` and the long options `longs` (in the program's
/// order), moving `cursor` past it and, for the long forms, the words that are not options
/// after the options found so far.
pub fn next<'a, W: Words + ?Sized>(
    words: &mut W,
    rules: Rules<'_>,
    longs: impl Iterator<Item = Long<'a>> + Clone,
    cursor: &mut Cursor,
) -> Found {
    if cursor.index >= words.count() {
        cursor.offset = 0;
    }
    if cursor.offset == 0 {
        if let Some(found) = start_word(words, rules, cursor) {
            return found;
        }
        if rules.kind != Kind::Short && words.word(cursor.index).starts_with(b"--") {
            cursor.offset = 2;
            return long(&*words, longs, false, 2, cursor).unwrap_or(Found::Done);
        }
        cursor.offset = 1;
    }
    let Some(&option) = words.word(cursor.index).get(cursor.offset) else {
        *cursor = Cursor {
            index: cursor.index + 1,
            offset: 0,
            ..*cursor
        };
        return Found::Done;
    };
    if rules.kind == Kind::LongOnly {
        let short_too = option != b':' && rules.shorts.contains(&option);
        if let Some(found) = long(&*words, longs, short_too, 1, cursor) {
            return found;
        }
    }
    short(&*words, rules, cursor)
}

fn start_word<W: Words + ?Sized>(
    words: &mut W,
    rules: Rules<'_>,
    cursor: &mut Cursor,
) -> Option<Found> {
    if let Some(Skipped {
        start,
        end: Some(end),
    }) = cursor.skipped
    {
        words.rotate(start, end, cursor.index);
        cursor.skipped = Some(Skipped {
            start: start + (cursor.index - end),
            end: None,
        });
    }
    loop {
        if cursor.index >= words.count() {
            if let Some(skipped) = cursor.skipped.take() {
                cursor.index = skipped.start;
            }
            return Some(Found::Done);
        }
        let word = words.word(cursor.index);
        if word == b"--" {
            cursor.index += 1;
            if let Some(skipped) = cursor.skipped.take() {
                words.rotate(skipped.start, cursor.index - 1, cursor.index);
                cursor.index = skipped.start + 1;
            }
            return Some(Found::Done);
        }
        if word.len() >= 2 && word.starts_with(b"-") {
            if let Some(skipped) = &mut cursor.skipped {
                skipped.end = Some(cursor.index);
            }
            return None;
        }
        match rules.order {
            Order::Stop => return Some(Found::Done),
            Order::InOrder => {
                cursor.index += 1;
                return Some(Found::Operand(cursor.index - 1));
            }
            Order::Permute => {
                if cursor.skipped.is_none() {
                    cursor.skipped = Some(Skipped {
                        start: cursor.index,
                        end: None,
                    });
                }
                cursor.index += 1;
            }
        }
    }
}

fn long<'a, W: Words + ?Sized>(
    words: &W,
    longs: impl Iterator<Item = Long<'a>> + Clone,
    short_too: bool,
    dashes: usize,
    cursor: &mut Cursor,
) -> Option<Found> {
    let at = cursor.index;
    let start = cursor.offset;
    let word = words.word(at);
    let body = word.get(start..).unwrap_or_default();
    let (end, attached) = match body.iter().position(|&b| b == b'=') {
        Some(equals) => (start + equals, Some(start + equals + 1)),
        None => (start + body.len(), None),
    };
    let name = word.get(start..end).unwrap_or_default();
    let typed = Typed {
        index: at,
        dashes,
        start,
        end,
    };
    let found = find_long(longs, name, short_too && name.len() == 1);
    if found == Match::Missing && short_too {
        return None;
    }
    cursor.index = at + 1;
    cursor.offset = 0;
    Some(match found {
        Match::Missing => Found::UnknownLong(typed),
        Match::Ambiguous => Found::AmbiguousLong(typed),
        Match::One(index, takes) => match (takes, attached) {
            (Takes::Nothing, Some(_)) => Found::UnexpectedLong(index, typed),
            (Takes::Nothing | Takes::Optional, None) => Found::Long(index, None),
            (_, Some(offset)) => Found::Long(index, Some((at, offset))),
            (Takes::Required, None) => {
                if cursor.index < words.count() {
                    cursor.index += 1;
                    Found::Long(index, Some((at + 1, 0)))
                } else {
                    Found::MissingLong(index, typed)
                }
            }
        },
    })
}

fn short<W: Words + ?Sized>(words: &W, rules: Rules<'_>, cursor: &mut Cursor) -> Found {
    let word = words.word(cursor.index);
    let option = word[cursor.offset];
    cursor.offset += 1;
    let at_end = cursor.offset >= word.len();
    let finish_word = |cursor: &mut Cursor| {
        cursor.index += 1;
        cursor.offset = 0;
    };
    let Some(position) = rules
        .shorts
        .iter()
        .position(|&b| b == option)
        .filter(|_| option != b':')
    else {
        if at_end {
            finish_word(cursor);
        }
        return Found::UnknownShort(option);
    };
    let takes = match (
        rules.shorts.get(position + 1),
        rules.shorts.get(position + 2),
    ) {
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
            if cursor.index < words.count() {
                cursor.index += 1;
                Found::Short(option, Some((cursor.index - 1, 0)))
            } else {
                Found::MissingShort(option)
            }
        }
    }
}
