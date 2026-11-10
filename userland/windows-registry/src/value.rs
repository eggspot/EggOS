//! Value types and how data is shown and typed, one set of words for advapi32's callers, the
//! command, MCP and Settings (docs/ux/windows-registry-v0.3.md section 3).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::Error;
use crate::path::{utf8, utf16};

/// No type.
pub const REG_NONE: u32 = 0;
/// A string.
pub const REG_SZ: u32 = 1;
/// A string with `%VARIABLE%`s a program expands.
pub const REG_EXPAND_SZ: u32 = 2;
/// Bytes.
pub const REG_BINARY: u32 = 3;
/// A 32-bit number, little-endian.
pub const REG_DWORD: u32 = 4;
/// A 32-bit number, big-endian.
pub const REG_DWORD_BIG_ENDIAN: u32 = 5;
/// A list of strings, each ending with a NUL, and a NUL after the last.
pub const REG_MULTI_SZ: u32 = 7;
/// A 64-bit number, little-endian.
pub const REG_QWORD: u32 = 11;

const NAMED: [(u32, &str); 8] = [
    (REG_NONE, "REG_NONE"),
    (REG_SZ, "REG_SZ"),
    (REG_EXPAND_SZ, "REG_EXPAND_SZ"),
    (REG_BINARY, "REG_BINARY"),
    (REG_DWORD, "REG_DWORD"),
    (REG_DWORD_BIG_ENDIAN, "REG_DWORD_BIG_ENDIAN"),
    (REG_MULTI_SZ, "REG_MULTI_SZ"),
    (REG_QWORD, "REG_QWORD"),
];

/// The types a person can make and edit, in Settings' order.
pub const EDITABLE: [u32; 6] = [
    REG_SZ,
    REG_EXPAND_SZ,
    REG_MULTI_SZ,
    REG_DWORD,
    REG_QWORD,
    REG_BINARY,
];

/// Its name: `REG_SZ`, or `REG_<n>` for a type without one.
pub fn type_name(kind: u32) -> String {
    match NAMED.iter().find(|(named, _)| *named == kind) {
        Some((_, name)) => String::from(*name),
        None => format!("REG_{kind}"),
    }
}

/// The editable type `name` names (`REG_DWORD`, or `dword`), ignoring case.
pub fn type_from_name(name: &str) -> Option<u32> {
    let bare = name
        .get(..4)
        .filter(|prefix| prefix.eq_ignore_ascii_case("REG_"))
        .map_or(name, |_| &name[4..]);
    EDITABLE
        .into_iter()
        .find(|kind| type_name(*kind)[4..].eq_ignore_ascii_case(bare))
}

/// The strings of string data: the UTF-16 units up to the first NUL, or all of them.
pub fn string_of(data: &[u8]) -> String {
    let units: Vec<u16> = units_of(data);
    let end = units
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(units.len());
    utf8(&units[..end])
}

/// The strings of `REG_MULTI_SZ` data, up to the empty one that ends them.
pub fn strings_of(data: &[u8]) -> Vec<String> {
    let units = units_of(data);
    let mut strings = Vec::new();
    for part in units.split(|unit| *unit == 0) {
        if part.is_empty() {
            break;
        }
        strings.push(utf8(part));
    }
    strings
}

fn units_of(data: &[u8]) -> Vec<u16> {
    data.chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect()
}

/// The number of `REG_DWORD`, `REG_DWORD_BIG_ENDIAN` or `REG_QWORD` data of the right size.
pub fn number_of(kind: u32, data: &[u8]) -> Option<u64> {
    match (kind, data.len()) {
        (REG_DWORD, 4) => Some(u64::from(u32::from_le_bytes([
            data[0], data[1], data[2], data[3],
        ]))),
        (REG_DWORD_BIG_ENDIAN, 4) => Some(u64::from(u32::from_be_bytes([
            data[0], data[1], data[2], data[3],
        ]))),
        (REG_QWORD, 8) => {
            let mut word = [0u8; 8];
            word.copy_from_slice(data);
            Some(u64::from_le_bytes(word))
        }
        _ => None,
    }
}

/// The data as the value table, the command and MCP's text show it.
pub fn data_text(kind: u32, data: &[u8]) -> String {
    if data.is_empty() {
        return String::from("(zero-length value)");
    }
    match kind {
        REG_SZ | REG_EXPAND_SZ => string_of(data),
        REG_MULTI_SZ => strings_of(data).join(" \u{b7} "),
        REG_DWORD | REG_DWORD_BIG_ENDIAN | REG_QWORD => match number_of(kind, data) {
            Some(number) if kind == REG_QWORD => format!("0x{number:016x} ({number})"),
            Some(number) => format!("0x{number:08x} ({number})"),
            None => hex_text(data, " "),
        },
        _ => hex_text(data, " "),
    }
}

/// The bytes in lowercase hex, `separator` between them.
pub fn hex_text(data: &[u8], separator: &str) -> String {
    let mut text = String::with_capacity(data.len() * (2 + separator.len()));
    for (index, byte) in data.iter().enumerate() {
        if index > 0 {
            text.push_str(separator);
        }
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

/// A string as `REG_SZ` data: UTF-16 with its NUL.
pub fn string_data(text: &str) -> Vec<u8> {
    let mut data: Vec<u8> = utf16(text).into_iter().flat_map(u16::to_le_bytes).collect();
    data.extend_from_slice(&[0, 0]);
    data
}

/// Strings as `REG_MULTI_SZ` data.
pub fn strings_data(strings: &[&str]) -> Vec<u8> {
    let mut data = Vec::new();
    for string in strings {
        data.extend(string_data(string));
    }
    data.extend_from_slice(&[0, 0]);
    data
}

/// Typed text as data of `kind`: one string for the string types, one string a line of
/// `texts` for `REG_MULTI_SZ`, a number (decimal, or hex after `0x`) for the number types and
/// pairs of hex digits (spaces allowed) for binary.
pub fn parse_data(kind: u32, texts: &[&str]) -> Result<Vec<u8>, Error> {
    let data = match kind {
        REG_SZ | REG_EXPAND_SZ => match texts {
            [] => string_data(""),
            [text] => string_data(text),
            _ => return Err(Error::BadData),
        },
        REG_MULTI_SZ => {
            if texts.iter().any(|text| text.is_empty()) {
                return Err(Error::BadData);
            }
            strings_data(texts)
        }
        REG_DWORD | REG_QWORD => {
            let [text] = texts else {
                return Err(Error::BadData);
            };
            let number = parse_number(text).ok_or(Error::BadData)?;
            if kind == REG_DWORD {
                u32::try_from(number)
                    .map_err(|_| Error::BadData)?
                    .to_le_bytes()
                    .to_vec()
            } else {
                number.to_le_bytes().to_vec()
            }
        }
        REG_BINARY => {
            let joined: String = texts.concat();
            parse_hex(&joined).ok_or(Error::BadData)?
        }
        _ => return Err(Error::BadData),
    };
    if data.len() > crate::MAX_DATA_BYTES {
        return Err(Error::ValueTooLarge);
    }
    Ok(data)
}

/// A whole number in decimal, or in hex after `0x`.
pub fn parse_number(text: &str) -> Option<u64> {
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) if !hex.is_empty() => u64::from_str_radix(hex, 16).ok(),
        Some(_) => None,
        None if !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()) => {
            text.parse().ok()
        }
        None => None,
    }
}

/// Pairs of hex digits, spaces between pairs allowed.
pub fn parse_hex(text: &str) -> Option<Vec<u8>> {
    let digits: Vec<u8> = text.bytes().filter(|byte| *byte != b' ').collect();
    if digits.len() % 2 != 0 {
        return None;
    }
    digits
        .chunks_exact(2)
        .map(|pair| {
            let high = char::from(pair[0]).to_digit(16)?;
            let low = char::from(pair[1]).to_digit(16)?;
            Some((high * 16 + low) as u8)
        })
        .collect()
}
