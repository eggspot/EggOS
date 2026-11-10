//! The XML format: a `<plist>` element holding one value element (`dict`, `array`, `string`,
//! `integer`, `real`, `true`, `false`, `date`, `data`), dictionaries alternating `<key>`s and
//! values. An object is the offset of its element's `<`; elements are found by scanning, never
//! recursively, and comments, processing instructions and the document type are skipped.

use crate::Kind;
use crate::text::{Data, Text};

const COMMENT_OPEN: &[u8] = b"<!--";
const COMMENT_CLOSE: &[u8] = b"-->";
const INSTRUCTION_OPEN: &[u8] = b"<?";
const INSTRUCTION_CLOSE: &[u8] = b"?>";
const DECLARATION_OPEN: &[u8] = b"<!";
const BYTE_ORDER_MARK: &[u8] = b"\xef\xbb\xbf";
const SECONDS_PER_DAY: i64 = 86_400;
const DAYS_1970_TO_2001: i64 = 11_323;

fn find(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    bytes
        .get(from..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|at| from + at)
}

struct Tag<'b> {
    name: &'b [u8],
    empty: bool,
    content: usize,
}

fn tag(bytes: &[u8], at: usize) -> Option<Tag<'_>> {
    if bytes.get(at) != Some(&b'<') {
        return None;
    }
    let name_start = at + 1;
    let name_length = bytes
        .get(name_start..)?
        .iter()
        .position(|&byte| byte.is_ascii_whitespace() || byte == b'/' || byte == b'>')?;
    let close = find(bytes, name_start, b">")?;
    let name = &bytes[name_start..name_start + name_length];
    let valid = !name.is_empty() && name[0] != b'/' && name[0] != b'!' && name[0] != b'?';
    valid.then_some(Tag {
        name,
        empty: bytes[close - 1] == b'/',
        content: close + 1,
    })
}

fn skip_markup(bytes: &[u8], at: usize) -> Option<usize> {
    let rest = bytes.get(at..)?;
    let (close, from) = if rest.starts_with(COMMENT_OPEN) {
        (COMMENT_CLOSE, at + COMMENT_OPEN.len())
    } else if rest.starts_with(INSTRUCTION_OPEN) {
        (INSTRUCTION_CLOSE, at + INSTRUCTION_OPEN.len())
    } else if rest.starts_with(DECLARATION_OPEN) {
        (&b">"[..], at + DECLARATION_OPEN.len())
    } else {
        return None;
    };
    Some(find(bytes, from, close)? + close.len())
}

fn next_element(bytes: &[u8], mut at: usize, end: usize) -> Option<usize> {
    while at < end {
        match bytes[at] {
            byte if byte.is_ascii_whitespace() => at += 1,
            b'<' if bytes.get(at + 1) == Some(&b'/') => return None,
            b'<' => match skip_markup(bytes, at) {
                Some(after) => at = after,
                None => return Some(at),
            },
            _ => return None,
        }
    }
    None
}

fn element_end(bytes: &[u8], at: usize) -> Option<(usize, usize)> {
    let opened = tag(bytes, at)?;
    if opened.empty {
        return Some((opened.content, opened.content));
    }
    let mut depth = 1usize;
    let mut scan = opened.content;
    loop {
        let open = find(bytes, scan, b"<")?;
        if bytes.get(open + 1) == Some(&b'/') {
            let close = find(bytes, open, b">")?;
            depth -= 1;
            if depth == 0 {
                return Some((open, close + 1));
            }
            scan = close + 1;
        } else if let Some(after) = skip_markup(bytes, open) {
            scan = after;
        } else {
            let inner = tag(bytes, open)?;
            depth += usize::from(!inner.empty);
            scan = inner.content;
        }
    }
}

pub(crate) fn tag_name(bytes: &[u8], at: usize) -> Option<&[u8]> {
    tag(bytes, at).map(|tag| tag.name)
}

/// The top value element's offset: `None` when the bytes are not an XML property list,
/// `Some(Err)` when they start as one but have no value.
pub(crate) fn top(bytes: &[u8]) -> Option<Result<usize, ()>> {
    let start = match bytes.starts_with(BYTE_ORDER_MARK) {
        true => BYTE_ORDER_MARK.len(),
        false => 0,
    };
    let root = next_element(bytes, start, bytes.len())?;
    if tag_name(bytes, root)? != b"plist" {
        return None;
    }
    let opened = tag(bytes, root)?;
    let (end, _) = match element_end(bytes, root) {
        Some(found) => found,
        None => return Some(Err(())),
    };
    Some(next_element(bytes, opened.content, end).ok_or(()))
}

fn content(bytes: &[u8], at: usize) -> Option<&[u8]> {
    let opened = tag(bytes, at)?;
    let (end, _) = element_end(bytes, at)?;
    bytes.get(opened.content..end)
}

fn leaf<'b>(bytes: &'b [u8], at: usize, name: &[u8]) -> Option<&'b [u8]> {
    (tag_name(bytes, at)? == name).then(|| content(bytes, at))?
}

fn leaf_text<'b>(bytes: &'b [u8], at: usize, name: &[u8]) -> Option<&'b str> {
    core::str::from_utf8(leaf(bytes, at, name)?)
        .ok()
        .map(str::trim)
}

pub(crate) fn kind(bytes: &[u8], at: usize) -> Option<Kind> {
    Some(match tag_name(bytes, at)? {
        b"dict" => Kind::Dictionary,
        b"array" => Kind::Array,
        b"string" | b"key" => Kind::String,
        b"integer" => Kind::Integer,
        b"real" => Kind::Real,
        b"true" | b"false" => Kind::Boolean,
        b"date" => Kind::Date,
        b"data" => Kind::Data,
        _ => return None,
    })
}

pub(crate) fn boolean(bytes: &[u8], at: usize) -> Option<bool> {
    match tag_name(bytes, at)? {
        b"true" => Some(true),
        b"false" => Some(false),
        _ => None,
    }
}

pub(crate) fn integer(bytes: &[u8], at: usize) -> Option<i128> {
    let text = leaf_text(bytes, at, b"integer")?;
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let (radix, digits) = match digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
    {
        Some(hex) => (16, hex),
        None => (10, digits),
    };
    if !digits.chars().all(|digit| digit.is_digit(radix)) {
        return None;
    }
    let magnitude = u128::from_str_radix(digits, radix).ok()?;
    if negative {
        0i128.checked_sub_unsigned(magnitude)
    } else {
        i128::try_from(magnitude).ok()
    }
}

pub(crate) fn real(bytes: &[u8], at: usize) -> Option<f64> {
    match leaf_text(bytes, at, b"real")? {
        "nan" => Some(f64::NAN),
        "+infinity" | "infinity" => Some(f64::INFINITY),
        "-infinity" => Some(f64::NEG_INFINITY),
        text => text.parse::<f64>().ok(),
    }
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let shifted_month = (month + 9) % 12;
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

pub(crate) fn date(bytes: &[u8], at: usize) -> Option<f64> {
    let text = leaf_text(bytes, at, b"date")?.as_bytes();
    if text.len() != 20 || text[19] != b'Z' || text[10] != b'T' {
        return None;
    }
    let number = |range: core::ops::Range<usize>| -> Option<i64> {
        core::str::from_utf8(&text[range]).ok()?.parse::<i64>().ok()
    };
    let separators = [(4, b'-'), (7, b'-'), (13, b':'), (16, b':')];
    if separators.iter().any(|&(at, byte)| text[at] != byte) {
        return None;
    }
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    let valid = (1..=12).contains(&month)
        && (1..=31).contains(&day)
        && hour < 24
        && minute < 60
        && second < 61;
    if !valid {
        return None;
    }
    let days = days_from_civil(year, month, day) - DAYS_1970_TO_2001;
    Some((days * SECONDS_PER_DAY + hour * 3600 + minute * 60 + second) as f64)
}

pub(crate) fn text(bytes: &[u8], at: usize) -> Option<Text<'_>> {
    let name = tag_name(bytes, at)?;
    (name == b"string" || name == b"key")
        .then(|| content(bytes, at))?
        .map(Text::xml)
}

pub(crate) fn data(bytes: &[u8], at: usize) -> Option<Data<'_>> {
    leaf(bytes, at, b"data").map(Data::base64)
}

/// The child elements of a collection element, in order.
#[derive(Debug, Clone)]
pub(crate) struct Children<'b> {
    bytes: &'b [u8],
    at: usize,
    end: usize,
}

impl<'b> Children<'b> {
    pub fn of(bytes: &'b [u8], at: usize) -> Option<Self> {
        let opened = tag(bytes, at)?;
        let (end, _) = element_end(bytes, at)?;
        Some(Self {
            bytes,
            at: opened.content,
            end,
        })
    }

    pub fn bytes(&self) -> &'b [u8] {
        self.bytes
    }
}

impl Iterator for Children<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        let child = next_element(self.bytes, self.at, self.end)?;
        let (_, after) = element_end(self.bytes, child)?;
        self.at = after;
        Some(child)
    }
}
