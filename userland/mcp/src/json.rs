//! JSON (RFC 8259) without an allocator: [`parse`] validates a whole message and hands back
//! [`Value`]s that borrow its bytes, and [`JsonWriter`] writes JSON into a fixed buffer, saying
//! so instead of failing when the buffer is too small. Strings stay escaped as received until a
//! caller decodes them ([`Str::decode_into`]); keys are compared decoded, so `"a"` and
//! `"a"` are the same key.

#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

/// How deeply arrays and objects may nest.
pub const MAX_DEPTH: usize = 32;
/// How many values one message may hold.
pub const MAX_VALUES: usize = 4096;
/// How many members one object may have.
pub const MAX_MEMBERS: usize = 64;
/// Objects at this depth or shallower (the message is 1, its `params` 2) refuse a duplicate
/// key outright; deeper ones only report it ([`Parsed::nested_duplicate`]).
pub const STRICT_DEPTH: usize = 2;

/// Why a message is not acceptable JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonError {
    /// The bytes are not UTF-8.
    InvalidUtf8,
    /// The grammar breaks at this byte.
    Syntax(usize),
    /// Arrays and objects nest deeper than [`MAX_DEPTH`].
    TooDeep,
    /// More than [`MAX_VALUES`] values.
    TooManyValues,
    /// An object with more than [`MAX_MEMBERS`] members.
    TooManyMembers,
    /// An object at [`STRICT_DEPTH`] or shallower names a key twice.
    DuplicateKey,
}

/// A message's top value, and whether a deeper object names a key twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parsed<'a> {
    /// The value.
    pub value: Value<'a>,
    /// An object deeper than [`STRICT_DEPTH`] names a key twice.
    pub nested_duplicate: bool,
}

/// A JSON value, borrowing the validated text it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value<'a> {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// A number, as written.
    Number(Number<'a>),
    /// A string.
    Str(Str<'a>),
    /// An array.
    Array(Array<'a>),
    /// An object.
    Object(Object<'a>),
}

/// A number as written in the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Number<'a>(&'a [u8]);

/// A string's text, quotes and escapes still in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Str<'a>(&'a [u8]);

/// An array's text, brackets included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Array<'a>(&'a [u8]);

/// An object's text, braces included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Object<'a>(&'a [u8]);

/// Validates `text` as one JSON value with nothing but white space around it.
pub fn parse(text: &[u8]) -> Result<Parsed<'_>, JsonError> {
    if core::str::from_utf8(text).is_err() {
        return Err(JsonError::InvalidUtf8);
    }
    let mut parser = Parser {
        text,
        position: 0,
        values: 0,
        check: true,
        nested_duplicate: false,
    };
    parser.skip_space();
    let value = parser.value(1)?;
    parser.skip_space();
    if parser.position != text.len() {
        return Err(JsonError::Syntax(parser.position));
    }
    Ok(Parsed {
        value,
        nested_duplicate: parser.nested_duplicate,
    })
}

struct Parser<'a> {
    text: &'a [u8],
    position: usize,
    values: usize,
    check: bool,
    nested_duplicate: bool,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.text.get(self.position).copied()
    }

    fn error(&self) -> JsonError {
        JsonError::Syntax(self.position)
    }

    fn skip_space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.position += 1;
        }
    }

    fn expect(&mut self, byte: u8) -> Result<(), JsonError> {
        match self.peek() {
            Some(found) if found == byte => {
                self.position += 1;
                Ok(())
            }
            _ => Err(self.error()),
        }
    }

    fn literal(&mut self, word: &[u8]) -> Result<(), JsonError> {
        match self.text.get(self.position..self.position + word.len()) {
            Some(found) if found == word => {
                self.position += word.len();
                Ok(())
            }
            _ => Err(self.error()),
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value<'a>, JsonError> {
        if self.check {
            self.values += 1;
            if self.values > MAX_VALUES {
                return Err(JsonError::TooManyValues);
            }
        }
        match self.peek() {
            Some(b'n') => self.literal(b"null").map(|()| Value::Null),
            Some(b't') => self.literal(b"true").map(|()| Value::Bool(true)),
            Some(b'f') => self.literal(b"false").map(|()| Value::Bool(false)),
            Some(b'"') => self.string().map(Value::Str),
            Some(b'[') => self.array(depth).map(Value::Array),
            Some(b'{') => self.object(depth).map(Value::Object),
            Some(b'-' | b'0'..=b'9') => self.number().map(Value::Number),
            _ => Err(self.error()),
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.position;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.position += 1;
        }
        self.position - start
    }

    fn number(&mut self) -> Result<Number<'a>, JsonError> {
        let start = self.position;
        if self.peek() == Some(b'-') {
            self.position += 1;
        }
        match self.peek() {
            Some(b'0') => self.position += 1,
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => return Err(self.error()),
        }
        if self.peek() == Some(b'.') {
            self.position += 1;
            if self.digits() == 0 {
                return Err(self.error());
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.position += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.position += 1;
            }
            if self.digits() == 0 {
                return Err(self.error());
            }
        }
        Ok(Number(self.text.get(start..self.position).unwrap_or(&[])))
    }

    fn string(&mut self) -> Result<Str<'a>, JsonError> {
        let start = self.position;
        self.expect(b'"')?;
        loop {
            match self.peek() {
                None => return Err(self.error()),
                Some(b'"') => break,
                Some(b'\\') => {
                    self.position += 1;
                    match self.peek() {
                        Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') => {
                            self.position += 1
                        }
                        Some(b'u') => {
                            self.position += 1;
                            for _ in 0..4 {
                                match self.peek() {
                                    Some(byte) if byte.is_ascii_hexdigit() => self.position += 1,
                                    _ => return Err(self.error()),
                                }
                            }
                        }
                        _ => return Err(self.error()),
                    }
                }
                Some(0..0x20) => return Err(self.error()),
                Some(_) => self.position += 1,
            }
        }
        self.position += 1;
        Ok(Str(self.text.get(start..self.position).unwrap_or(&[])))
    }

    fn array(&mut self, depth: usize) -> Result<Array<'a>, JsonError> {
        if depth > MAX_DEPTH {
            return Err(JsonError::TooDeep);
        }
        let start = self.position;
        self.expect(b'[')?;
        self.skip_space();
        if self.peek() == Some(b']') {
            self.position += 1;
            return Ok(Array(self.text.get(start..self.position).unwrap_or(&[])));
        }
        loop {
            self.skip_space();
            self.value(depth + 1)?;
            self.skip_space();
            match self.peek() {
                Some(b',') => self.position += 1,
                Some(b']') => {
                    self.position += 1;
                    return Ok(Array(self.text.get(start..self.position).unwrap_or(&[])));
                }
                _ => return Err(self.error()),
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<Object<'a>, JsonError> {
        if depth > MAX_DEPTH {
            return Err(JsonError::TooDeep);
        }
        let start = self.position;
        self.expect(b'{')?;
        self.skip_space();
        if self.peek() == Some(b'}') {
            self.position += 1;
            return Ok(Object(self.text.get(start..self.position).unwrap_or(&[])));
        }
        let mut keys = [(0usize, 0usize); MAX_MEMBERS];
        let mut count = 0;
        loop {
            self.skip_space();
            let key_start = self.position;
            let key = self.string()?;
            if self.check {
                let seen = keys
                    .iter()
                    .take(count)
                    .any(|&(from, to)| Str(self.text.get(from..to).unwrap_or(&[])).same_text(key));
                if seen {
                    match depth <= STRICT_DEPTH {
                        true => return Err(JsonError::DuplicateKey),
                        false => self.nested_duplicate = true,
                    }
                }
                match keys.get_mut(count) {
                    Some(slot) => *slot = (key_start, self.position),
                    None => return Err(JsonError::TooManyMembers),
                }
                count += 1;
            }
            self.skip_space();
            self.expect(b':')?;
            self.skip_space();
            self.value(depth + 1)?;
            self.skip_space();
            match self.peek() {
                Some(b',') => self.position += 1,
                Some(b'}') => {
                    self.position += 1;
                    return Ok(Object(self.text.get(start..self.position).unwrap_or(&[])));
                }
                _ => return Err(self.error()),
            }
        }
    }
}

/// The members of an object, read from its validated text, in order.
#[derive(Debug, Clone)]
pub struct Members<'a> {
    text: &'a [u8],
    position: usize,
    done: bool,
}

impl<'a> Members<'a> {
    fn over(text: &'a [u8]) -> Self {
        let start = text
            .iter()
            .position(|&byte| byte == b'{')
            .map_or(text.len(), |at| at + 1);
        Self {
            text,
            position: start,
            done: false,
        }
    }

    fn next_member(&mut self) -> Option<(Str<'a>, Value<'a>)> {
        if self.done {
            return None;
        }
        let mut parser = Parser {
            text: self.text,
            position: self.position,
            values: 0,
            check: false,
            nested_duplicate: false,
        };
        parser.skip_space();
        if parser.peek() != Some(b'"') {
            self.done = true;
            return None;
        }
        let member = parser.string().ok().and_then(|key| {
            parser.skip_space();
            parser.expect(b':').ok()?;
            parser.skip_space();
            let value = parser.value(1).ok()?;
            parser.skip_space();
            if parser.peek() == Some(b',') {
                parser.position += 1;
            }
            Some((key, value))
        });
        self.position = parser.position;
        if member.is_none() {
            self.done = true;
        }
        member
    }
}

impl<'a> Iterator for Members<'a> {
    type Item = (Str<'a>, Value<'a>);

    fn next(&mut self) -> Option<Self::Item> {
        self.next_member()
    }
}

/// The elements of an array, read from its validated text, in order.
#[derive(Debug, Clone)]
pub struct Elements<'a> {
    text: &'a [u8],
    position: usize,
    done: bool,
}

impl<'a> Iterator for Elements<'a> {
    type Item = Value<'a>;

    fn next(&mut self) -> Option<Value<'a>> {
        if self.done {
            return None;
        }
        let mut parser = Parser {
            text: self.text,
            position: self.position,
            values: 0,
            check: false,
            nested_duplicate: false,
        };
        parser.skip_space();
        if parser.peek() == Some(b']') {
            self.done = true;
            return None;
        }
        let value = parser.value(1).ok();
        parser.skip_space();
        if parser.peek() == Some(b',') {
            parser.position += 1;
        }
        self.position = parser.position;
        if value.is_none() {
            self.done = true;
        }
        value
    }
}

impl<'a> Object<'a> {
    /// Its members, in order.
    pub fn members(self) -> Members<'a> {
        Members::over(self.0)
    }

    /// The value of the member named `key` (compared decoded).
    pub fn get(self, key: &str) -> Option<Value<'a>> {
        self.members()
            .find(|(name, _)| name.equals(key))
            .map(|(_, value)| value)
    }

    /// Its text as received.
    pub fn text(self) -> &'a [u8] {
        self.0
    }
}

impl<'a> Array<'a> {
    /// Its elements, in order.
    pub fn elements(self) -> Elements<'a> {
        Elements {
            text: self.0,
            position: 1,
            done: false,
        }
    }
}

impl<'a> Value<'a> {
    /// The member named `key`, if this is an object that has one.
    pub fn get(self, key: &str) -> Option<Value<'a>> {
        match self {
            Value::Object(object) => object.get(key),
            _ => None,
        }
    }

    /// The string, if this is one.
    pub fn as_str(self) -> Option<Str<'a>> {
        match self {
            Value::Str(text) => Some(text),
            _ => None,
        }
    }

    /// The object, if this is one.
    pub fn as_object(self) -> Option<Object<'a>> {
        match self {
            Value::Object(object) => Some(object),
            _ => None,
        }
    }

    /// The value's text exactly as received: a number's digits, a string with its quotes.
    pub fn raw(self) -> &'a [u8] {
        match self {
            Value::Null => b"null",
            Value::Bool(true) => b"true",
            Value::Bool(false) => b"false",
            Value::Number(number) => number.0,
            Value::Str(text) => text.0,
            Value::Array(array) => array.0,
            Value::Object(object) => object.0,
        }
    }
}

impl<'a> Number<'a> {
    /// The number as an unsigned integer, if it is one written without a fraction or
    /// exponent and fits.
    pub fn as_u64(self) -> Option<u64> {
        if self.0.is_empty() || !self.0.iter().all(u8::is_ascii_digit) {
            return None;
        }
        self.0.iter().try_fold(0u64, |total, &digit| {
            total.checked_mul(10)?.checked_add(u64::from(digit - b'0'))
        })
    }

    /// The number as a signed integer, if it is one written without a fraction or exponent
    /// and fits.
    pub fn as_i64(self) -> Option<i64> {
        match self.0.strip_prefix(b"-") {
            Some(digits) => {
                let magnitude = Number(digits).as_u64()?;
                0i64.checked_sub_unsigned(magnitude)
            }
            None => i64::try_from(self.as_u64()?).ok(),
        }
    }
}

impl<'a> Str<'a> {
    /// The escaped text between the quotes.
    pub fn raw(self) -> &'a [u8] {
        self.0.get(1..self.0.len().saturating_sub(1)).unwrap_or(&[])
    }

    /// Its characters, decoded: escapes resolved, an unpaired surrogate as U+FFFD.
    pub fn chars(self) -> Chars<'a> {
        Chars {
            text: self.raw(),
            position: 0,
        }
    }

    /// Whether it decodes to `text`.
    pub fn equals(self, text: &str) -> bool {
        self.chars().eq(text.chars())
    }

    fn same_text(self, other: Str<'_>) -> bool {
        self.chars().eq(other.chars())
    }

    /// Decodes it into the start of `buffer`; `None` if it does not fit.
    pub fn decode_into(self, buffer: &mut [u8]) -> Option<&str> {
        let mut length = 0;
        for character in self.chars() {
            let mut bytes = [0; 4];
            let encoded = character.encode_utf8(&mut bytes).as_bytes();
            buffer
                .get_mut(length..length + encoded.len())?
                .copy_from_slice(encoded);
            length += encoded.len();
        }
        core::str::from_utf8(buffer.get(..length)?).ok()
    }

    /// Decodes as much of it as fits into `buffer`, whole characters only; returns the text
    /// and whether all of it fitted.
    pub fn decode_prefix_into(self, buffer: &mut [u8]) -> (&str, bool) {
        let mut length = 0;
        let mut whole = true;
        for character in self.chars() {
            let mut bytes = [0; 4];
            let encoded = character.encode_utf8(&mut bytes).as_bytes();
            match buffer.get_mut(length..length + encoded.len()) {
                Some(place) => place.copy_from_slice(encoded),
                None => {
                    whole = false;
                    break;
                }
            }
            length += encoded.len();
        }
        let text = buffer
            .get(..length)
            .and_then(|bytes| core::str::from_utf8(bytes).ok())
            .unwrap_or("");
        (text, whole)
    }

    /// How many bytes it takes decoded.
    pub fn decoded_len(self) -> usize {
        self.chars().map(char::len_utf8).sum()
    }
}

/// The decoded characters of a [`Str`].
#[derive(Debug, Clone)]
pub struct Chars<'a> {
    text: &'a [u8],
    position: usize,
}

impl Chars<'_> {
    fn hex4(&mut self) -> Option<u32> {
        let digits = self.text.get(self.position..self.position + 4)?;
        let text = core::str::from_utf8(digits).ok()?;
        let value = u32::from_str_radix(text, 16).ok()?;
        self.position += 4;
        Some(value)
    }
}

impl Iterator for Chars<'_> {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        let &byte = self.text.get(self.position)?;
        if byte != b'\\' {
            let rest = self.text.get(self.position..)?;
            let length = match byte {
                0..0x80 => 1,
                0xc0..0xe0 => 2,
                0xe0..0xf0 => 3,
                _ => 4,
            };
            let character = rest
                .get(..length)
                .and_then(|bytes| core::str::from_utf8(bytes).ok())
                .and_then(|text| text.chars().next())
                .unwrap_or(char::REPLACEMENT_CHARACTER);
            self.position += length;
            return Some(character);
        }
        self.position += 1;
        let &escape = self.text.get(self.position)?;
        self.position += 1;
        Some(match escape {
            b'"' => '"',
            b'\\' => '\\',
            b'/' => '/',
            b'b' => '\u{8}',
            b'f' => '\u{c}',
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            _ => {
                let high = self.hex4()?;
                if (0xd800..0xdc00).contains(&high) && self.text.get(self.position) == Some(&b'\\')
                {
                    let saved = self.position;
                    if self.text.get(self.position + 1) == Some(&b'u') {
                        self.position += 2;
                        if let Some(low) = self.hex4()
                            && (0xdc00..0xe000).contains(&low)
                        {
                            let code = 0x10000 + ((high - 0xd800) << 10) + (low - 0xdc00);
                            return Some(char::from_u32(code).unwrap_or('\u{fffd}'));
                        }
                    }
                    self.position = saved;
                }
                char::from_u32(high).unwrap_or(char::REPLACEMENT_CHARACTER)
            }
        })
    }
}

/// The bytes of a base64 string written in pieces ([`JsonWriter::base64_piece`]) that do not
/// yet make a whole group of three.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Base64Carry {
    group: [u8; 3],
    length: usize,
}

/// Where a [`JsonWriter`] was, to go back to with [`JsonWriter::rewind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    length: usize,
    depth: usize,
    first: u64,
    after_key: bool,
    overflowed: bool,
}

/// Writes JSON into a fixed buffer, commas and escapes included; text that does not fit is
/// dropped and [`Self::overflowed`] says so.
#[derive(Debug)]
pub struct JsonWriter<'b> {
    out: &'b mut [u8],
    length: usize,
    depth: usize,
    first: u64,
    after_key: bool,
    overflowed: bool,
}

const HEX: &[u8; 16] = b"0123456789abcdef";
const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn digit(alphabet: &[u8], value: usize) -> u8 {
    alphabet
        .get(value % alphabet.len())
        .copied()
        .unwrap_or(b'0')
}

impl<'b> JsonWriter<'b> {
    /// A writer filling `out` from its start.
    pub fn new(out: &'b mut [u8]) -> Self {
        Self {
            out,
            length: 0,
            depth: 0,
            first: 1,
            after_key: false,
            overflowed: false,
        }
    }

    /// What was written.
    pub fn as_bytes(&self) -> &[u8] {
        self.out.get(..self.length).unwrap_or(&[])
    }

    /// How many bytes were written.
    pub fn len(&self) -> usize {
        self.length
    }

    /// Whether nothing was written.
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// Room left in the buffer.
    pub fn remaining(&self) -> usize {
        self.out.len() - self.length
    }

    /// Whether some text did not fit.
    pub fn overflowed(&self) -> bool {
        self.overflowed
    }

    /// Empties the writer.
    pub fn clear(&mut self) {
        self.rewind(Mark {
            length: 0,
            depth: 0,
            first: 1,
            after_key: false,
            overflowed: false,
        });
    }

    /// Where the writer is now.
    pub fn mark(&self) -> Mark {
        Mark {
            length: self.length,
            depth: self.depth,
            first: self.first,
            after_key: self.after_key,
            overflowed: self.overflowed,
        }
    }

    /// Goes back to `mark`, forgetting what was written since.
    pub fn rewind(&mut self, mark: Mark) {
        self.length = mark.length;
        self.depth = mark.depth;
        self.first = mark.first;
        self.after_key = mark.after_key;
        self.overflowed = mark.overflowed;
    }

    fn put(&mut self, bytes: &[u8]) {
        match self.out.get_mut(self.length..self.length + bytes.len()) {
            Some(place) => {
                place.copy_from_slice(bytes);
                self.length += bytes.len();
            }
            None => self.overflowed = true,
        }
    }

    fn before_value(&mut self) {
        if self.after_key {
            self.after_key = false;
            return;
        }
        let bit = 1u64 << self.depth.min(63);
        if self.depth > 0 && self.first & bit == 0 {
            self.put(b",");
        }
        self.first &= !bit;
    }

    fn open(&mut self, bracket: &[u8]) {
        self.before_value();
        self.put(bracket);
        self.depth += 1;
        self.first |= 1u64 << self.depth.min(63);
    }

    fn close(&mut self, bracket: &[u8]) {
        self.depth = self.depth.saturating_sub(1);
        self.put(bracket);
    }

    /// `{`.
    pub fn begin_object(&mut self) {
        self.open(b"{");
    }

    /// `}`.
    pub fn end_object(&mut self) {
        self.close(b"}");
    }

    /// `[`.
    pub fn begin_array(&mut self) {
        self.open(b"[");
    }

    /// `]`.
    pub fn end_array(&mut self) {
        self.close(b"]");
    }

    /// A member's key and its colon; the member's value comes next.
    #[inline(never)]
    pub fn key(&mut self, key: &str) {
        self.string(key);
        self.put(b":");
        self.after_key = true;
    }

    /// A string.
    #[inline(never)]
    pub fn string(&mut self, text: &str) {
        self.begin_string();
        self.string_piece(text);
        self.end_string();
    }

    /// A string of `bytes`, invalid UTF-8 replaced by U+FFFD.
    pub fn string_lossy(&mut self, bytes: &[u8]) {
        self.begin_string();
        self.string_piece_lossy(bytes);
        self.end_string();
    }

    /// The opening quote of a string written in pieces.
    pub fn begin_string(&mut self) {
        self.before_value();
        self.put(b"\"");
    }

    /// Part of a string begun with [`Self::begin_string`].
    pub fn string_piece(&mut self, text: &str) {
        let bytes = text.as_bytes();
        let mut start = 0;
        for (index, &byte) in bytes.iter().enumerate() {
            let escape: Option<&[u8]> = match byte {
                b'"' => Some(b"\\\""),
                b'\\' => Some(b"\\\\"),
                b'\n' => Some(b"\\n"),
                b'\r' => Some(b"\\r"),
                b'\t' => Some(b"\\t"),
                0x08 => Some(b"\\b"),
                0x0c => Some(b"\\f"),
                0..0x20 => None,
                _ => continue,
            };
            self.put(bytes.get(start..index).unwrap_or(&[]));
            match escape {
                Some(escape) => self.put(escape),
                None => self.put(&[
                    b'\\',
                    b'u',
                    b'0',
                    b'0',
                    digit(HEX, usize::from(byte >> 4)),
                    digit(HEX, usize::from(byte)),
                ]),
            }
            start = index + 1;
        }
        self.put(bytes.get(start..).unwrap_or(&[]));
    }

    /// Part of a string from `bytes`, invalid UTF-8 replaced by U+FFFD.
    pub fn string_piece_lossy(&mut self, bytes: &[u8]) {
        for chunk in bytes.utf8_chunks() {
            self.string_piece(chunk.valid());
            if !chunk.invalid().is_empty() {
                self.string_piece("\u{fffd}");
            }
        }
    }

    /// The closing quote of a string written in pieces.
    pub fn end_string(&mut self) {
        self.put(b"\"");
    }

    /// `bytes` in base64 (RFC 4648) as a string.
    pub fn string_base64(&mut self, bytes: &[u8]) {
        self.begin_string();
        let mut carry = Base64Carry::default();
        self.base64_piece(&mut carry, bytes);
        self.base64_end(carry);
        self.end_string();
    }

    /// Part of a base64 string begun with [`Self::begin_string`]: `bytes` encoded after what
    /// `carry` holds, which keeps the last one or two bytes that do not make a whole group, so
    /// pieces of any length give the same text as one piece.
    pub fn base64_piece(&mut self, carry: &mut Base64Carry, bytes: &[u8]) {
        for &byte in bytes {
            if let Some(slot) = carry.group.get_mut(carry.length) {
                *slot = byte;
            }
            carry.length += 1;
            if carry.length == carry.group.len() {
                self.put_base64_group(&carry.group, carry.length);
                carry.length = 0;
            }
        }
    }

    /// Ends a base64 string written with [`Self::base64_piece`]: the bytes `carry` still
    /// holds, padded with `=`. The closing quote is [`Self::end_string`]'s.
    pub fn base64_end(&mut self, carry: Base64Carry) {
        if carry.length > 0 {
            self.put_base64_group(&carry.group, carry.length);
        }
    }

    fn put_base64_group(&mut self, group: &[u8; 3], length: usize) {
        let used = group.get(..length).unwrap_or(&[]);
        let word = used.iter().enumerate().fold(0u32, |word, (index, &byte)| {
            word | u32::from(byte) << (16 - 8 * index)
        });
        let mut encoded = [b'='; 4];
        for (index, slot) in encoded.iter_mut().enumerate().take(used.len() + 1) {
            *slot = digit(BASE64, (word >> (18 - 6 * index)) as usize);
        }
        self.put(&encoded);
    }

    /// An unsigned number.
    pub fn u64(&mut self, value: u64) {
        self.before_value();
        let mut digits = [0u8; 20];
        let mut start = digits.len();
        let mut rest = value;
        loop {
            start -= 1;
            if let Some(digit) = digits.get_mut(start) {
                *digit = b'0' + (rest % 10) as u8;
            }
            rest /= 10;
            if rest == 0 {
                break;
            }
        }
        self.put(digits.get(start..).unwrap_or(&[]));
    }

    /// A signed number.
    pub fn i64(&mut self, value: i64) {
        if value < 0 {
            self.before_value();
            self.put(b"-");
            self.after_key = true;
        }
        self.u64(value.unsigned_abs());
    }

    /// `true` or `false`.
    pub fn bool(&mut self, value: bool) {
        self.before_value();
        self.put(if value { b"true" } else { b"false" });
    }

    /// `null`.
    pub fn null(&mut self) {
        self.before_value();
        self.put(b"null");
    }

    /// A value's text exactly as given, which must be valid JSON (a value [`parse`] returned).
    pub fn raw(&mut self, json: &[u8]) {
        self.before_value();
        self.put(json);
    }

    /// Ends a message with a newline, as the stdio transport frames messages.
    pub fn end_line(&mut self) {
        self.put(b"\n");
    }
}

#[cfg(test)]
mod tests;
