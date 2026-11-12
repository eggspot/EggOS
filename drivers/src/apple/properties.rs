//! The binary property lists Apple's coprocessor firmware sends (D545): a `0xd3` header, then
//! 32-bit tags, each 4-byte aligned, giving a value's size or count in bits 23:0, its type in
//! bits 28:24 and whether it is the last of its container in bit 31: dictionaries of key and
//! value pairs, arrays, 64-bit integers, strings, blobs and booleans.

const HEADER: u32 = 0xd3;
const SIZE_MASK: u32 = (1 << 24) - 1;
const TYPE_SHIFT: u32 = 24;
const TYPE_MASK: u32 = 0x1f;
const PADDING_MASK: u32 = 0x3 << 29;
const DICTIONARY: u32 = 1;
const ARRAY: u32 = 2;
const INTEGER: u32 = 4;
const STRING: u32 = 9;
const BLOB: u32 = 10;
const BOOLEAN: u32 = 11;
const MAXIMUM_NESTING: u32 = 16;

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let taken = self.bytes.get(self.at..self.at.checked_add(count)?)?;
        self.at += count;
        Some(taken)
    }

    fn word(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn tag(&mut self) -> Option<(u32, u32)> {
        self.at = self.at.next_multiple_of(4);
        let tag = self.word()?;
        if tag & PADDING_MASK != 0 {
            return None;
        }
        Some((tag >> TYPE_SHIFT & TYPE_MASK, tag & SIZE_MASK))
    }

    fn string(&mut self) -> Option<&'a [u8]> {
        match self.tag()? {
            (STRING, size) => self.take(size as usize),
            _ => None,
        }
    }

    fn skip(&mut self, nesting: u32) -> Option<()> {
        if nesting > MAXIMUM_NESTING {
            return None;
        }
        let (kind, size) = self.tag()?;
        match kind {
            DICTIONARY => {
                for _ in 0..size {
                    self.skip(nesting + 1)?;
                    self.skip(nesting + 1)?;
                }
            }
            ARRAY => {
                for _ in 0..size {
                    self.skip(nesting + 1)?;
                }
            }
            INTEGER => {
                self.take(8)?;
            }
            STRING | BLOB => {
                self.take(size as usize)?;
            }
            BOOLEAN => {}
            _ => return None,
        }
        Some(())
    }
}

/// The string the top-level dictionary of `list` holds under `key`; `None` when `list` is not
/// a property list, has no such key, or holds something else there.
pub fn string_value<'a>(list: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    let mut reader = Reader { bytes: list, at: 0 };
    if reader.word()? != HEADER {
        return None;
    }
    let (DICTIONARY, pairs) = reader.tag()? else {
        return None;
    };
    for _ in 0..pairs {
        let name = reader.string()?;
        if name == key {
            return reader.string();
        }
        reader.skip(1)?;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    fn tag(kind: u32, size: u32) -> [u8; 4] {
        (kind << TYPE_SHIFT | size).to_le_bytes()
    }

    fn string(out: &mut Vec<u8>, text: &[u8]) {
        out.resize(out.len().next_multiple_of(4), 0);
        out.extend_from_slice(&tag(STRING, text.len() as u32));
        out.extend_from_slice(text);
    }

    #[test]
    fn a_string_is_found_past_other_values() {
        let mut list = HEADER.to_le_bytes().to_vec();
        list.extend_from_slice(&tag(DICTIONARY, 3));
        string(&mut list, b"EPICUnit");
        list.resize(list.len().next_multiple_of(4), 0);
        list.extend_from_slice(&tag(INTEGER, 0));
        list.extend_from_slice(&0i64.to_le_bytes());
        string(&mut list, b"EPICName");
        string(&mut list, b"dcp");
        string(&mut list, b"EPICProviderClass");
        string(&mut list, b"disp0-service");
        assert_eq!(string_value(&list, b"EPICProviderClass"), Some(&b"disp0-service"[..]));
        assert_eq!(string_value(&list, b"EPICName"), Some(&b"dcp"[..]));
        assert_eq!(string_value(&list, b"Missing"), None);
        assert_eq!(string_value(&list[4..], b"EPICName"), None);
    }
}
