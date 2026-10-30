//! A `String`'s `Character`s in the runtime (D180): its count, moving an index by characters,
//! reading the character or the substring at indices, iterating, `Character`'s hashing and
//! printing, `lowercased()`, `String(substring)` and copying the UTF-8 out. Characters are
//! grapheme clusters (`grapheme.rs`) of the string's UTF-8, and the indices the runtime makes
//! are character-aligned UTF-8 indices (`string_index.rs`); a `Character` is a `String` of one
//! cluster, small when it fits.

use super::case_mapping;
use super::grapheme;
use super::hasher::HasherState;
use super::metadata::{Metadata, copy_bytes};
use super::protocols::write_to_stream;
use super::standard_protocols::string_hash_into;
use super::string::{bytes_of, make, retained};
use super::string_index;
use super::string_object::RawString;
use crate::objc::buffer::Buffer;

fn raw(count_and_flags: u64, object: u64) -> RawString {
    RawString {
        count_and_flags,
        object,
    }
}

fn out_of_bounds() -> ! {
    super::fatal_error(&[b"String index is out of bounds"])
}

fn utf8_offset(index: u64, bytes: &[u8]) -> usize {
    if !string_index::is_utf8(index) {
        super::unimplemented(&[b"UTF-16 indices of strings bridged from NSString"]);
    }
    let offset = string_index::offset(index);
    if offset > bytes.len() {
        out_of_bounds();
    }
    if string_index::is_character_aligned(index) {
        offset
    } else {
        grapheme::cluster_start(bytes, offset)
    }
}

/// An `Optional` of a one-word payload as Swift returns it: the payload, then the tag byte
/// (1 for `nil`).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct OptionalWord {
    value: u64,
    none: u8,
}

const NONE: OptionalWord = OptionalWord { value: 0, none: 1 };

fn some(value: u64) -> OptionalWord {
    OptionalWord { value, none: 0 }
}

extern "C" fn count(count_and_flags: u64, object: u64) -> isize {
    let mut scratch = [0u8; 16];
    grapheme::count(bytes_of(raw(count_and_flags, object), &mut scratch)) as isize
}

extern "C" fn index_offset_by(
    index: u64,
    distance: isize,
    limit: u64,
    count_and_flags: u64,
    object: u64,
) -> OptionalWord {
    let mut scratch = [0u8; 16];
    let bytes = bytes_of(raw(count_and_flags, object), &mut scratch);
    let mut at = utf8_offset(index, bytes);
    let limit = utf8_offset(limit, bytes);
    for _ in 0..distance.unsigned_abs() {
        if at == limit {
            return NONE;
        }
        at = if distance > 0 {
            if at >= bytes.len() {
                out_of_bounds();
            }
            grapheme::next_break(bytes, at)
        } else {
            if at == 0 {
                out_of_bounds();
            }
            grapheme::cluster_start(bytes, at - 1)
        };
    }
    some(string_index::character(at))
}

extern "C" fn character_at(index: u64, count_and_flags: u64, object: u64) -> RawString {
    let mut scratch = [0u8; 16];
    let bytes = bytes_of(raw(count_and_flags, object), &mut scratch);
    let at = utf8_offset(index, bytes);
    if at >= bytes.len() {
        out_of_bounds();
    }
    make(&bytes[at..grapheme::next_break(bytes, at)])
}

/// `String.subscript(Range<String.Index>) -> Substring`: the bounds, then the string retained,
/// into `out` (the entry returns the four words in registers).
pub(super) extern "C" fn substring(
    out: *mut [u64; 4],
    lower: u64,
    upper: u64,
    count_and_flags: u64,
    object: u64,
) {
    let value = raw(count_and_flags, object);
    let mut scratch = [0u8; 16];
    let bytes = bytes_of(value, &mut scratch);
    let start = utf8_offset(lower, bytes);
    let end = utf8_offset(upper, bytes);
    if start > end {
        out_of_bounds();
    }
    let kept = retained(value);
    let words = [
        string_index::character(start),
        string_index::character(end),
        kept.count_and_flags,
        kept.object,
    ];
    // SAFETY: the entry's four words of stack for the result.
    unsafe { out.write(words) };
}

fn bounded_offset(index: u64, length: usize) -> usize {
    let offset = string_index::offset(index);
    if offset > length {
        out_of_bounds();
    }
    offset
}

extern "C" fn from_substring(
    lower: u64,
    upper: u64,
    count_and_flags: u64,
    object: u64,
) -> RawString {
    let value = raw(count_and_flags, object);
    let mut scratch = [0u8; 16];
    let bytes = bytes_of(value, &mut scratch);
    let start = bounded_offset(lower, bytes.len());
    let end = bounded_offset(upper, bytes.len());
    if start > end {
        out_of_bounds();
    }
    if start == 0 && end == bytes.len() {
        return retained(value);
    }
    make(&bytes[start..end])
}

/// `String.Iterator`: the string, the UTF-8 offset of the next character and the end.
#[repr(C)]
#[derive(Debug)]
#[allow(dead_code)]
pub struct StringIterator {
    string: RawString,
    position: usize,
    end: usize,
}

/// `String.Iterator.next()`: the next character, or `nil` (both words zero) at the end.
pub(super) extern "C" fn iterator_next(iterator: *mut StringIterator) -> RawString {
    // SAFETY: Swift hands over its inout iterator for the call.
    let state = unsafe { &mut *iterator };
    if state.position >= state.end {
        return raw(0, 0);
    }
    let string = state.string;
    let mut scratch = [0u8; 16];
    let bytes = bytes_of(string, &mut scratch);
    let bytes = &bytes[..state.end.min(bytes.len())];
    if state.position >= bytes.len() {
        return raw(0, 0);
    }
    let end = grapheme::next_break(bytes, state.position);
    let character = make(&bytes[state.position..end]);
    state.position = end;
    character
}

extern "C" fn character_hash(hasher: *mut HasherState, count_and_flags: u64, object: u64) {
    let value = raw(count_and_flags, object);
    string_hash_into(hasher, &raw const value);
}

extern "C" fn character_write(
    target: *mut u8,
    count_and_flags: u64,
    object: u64,
    stream: Metadata,
    table: usize,
) {
    let mut scratch = [0u8; 16];
    let bytes = bytes_of(raw(count_and_flags, object), &mut scratch);
    write_to_stream(target, bytes, stream, table);
}

extern "C" fn lowercased(count_and_flags: u64, object: u64) -> RawString {
    let value = raw(count_and_flags, object);
    let mut scratch = [0u8; 16];
    let bytes = bytes_of(value, &mut scratch);
    if !bytes
        .iter()
        .any(|&byte| byte.is_ascii_uppercase() || !byte.is_ascii())
    {
        return retained(value);
    }
    let mut lower = Buffer::<u8>::new();
    let mut at = 0;
    let mut room = true;
    while at < bytes.len() {
        let (scalar, length) = grapheme::decode(bytes, at);
        at += length;
        let mut mapped = [0u32; 2];
        let scalars = case_mapping::lowercase(scalar, &mut mapped);
        for &scalar in &mapped[..scalars] {
            let mut encoded = [0u8; 4];
            let size = grapheme::encode(scalar, &mut encoded);
            for &byte in &encoded[..size] {
                room &= lower.push(byte);
            }
        }
    }
    if !room {
        super::fatal_error(&[b"out of memory for a lowercased String"]);
    }
    let result = make(lower.as_slice());
    lower.free();
    result
}

extern "C" fn foreign_count(count_and_flags: u64, object: u64) -> isize {
    let mut scratch = [0u8; 16];
    bytes_of(raw(count_and_flags, object), &mut scratch).len() as isize
}

extern "C" fn copy_utf8(
    start: *mut u8,
    capacity: isize,
    count_and_flags: u64,
    object: u64,
) -> OptionalWord {
    let mut scratch = [0u8; 16];
    let bytes = bytes_of(raw(count_and_flags, object), &mut scratch);
    if bytes.len() > capacity.max(0) as usize {
        return NONE;
    }
    copy_bytes(start, bytes.as_ptr(), bytes.len());
    some(bytes.len() as u64)
}

pub fn resolve(name: &[u8]) -> Option<u64> {
    use super::entry::{self, address};
    let found = match name {
        b"_$sSS5countSivg" => count as *const () as u64,
        b"_$sSS5index_8offsetBy07limitedC0SS5IndexVSgAE_SiAEtF" => {
            index_offset_by as *const () as u64
        }
        b"_$sSSySJSS5IndexVcig" => character_at as *const () as u64,
        b"_$sSSySsSnySS5IndexVGcig" => address(entry::eggos_swift_substring),
        b"_$sSS14_fromSubstringySSSshFZ" => from_substring as *const () as u64,
        b"_$sSS8IteratorV4nextSJSgyF" => address(entry::eggos_swift_iterator_next),
        b"_$sSJ4hash4intoys6HasherVz_tF" => character_hash as *const () as u64,
        b"_$sSJ5write2toyxz_ts16TextOutputStreamRzlF" => character_write as *const () as u64,
        b"_$sSS10lowercasedSSyF" => lowercased as *const () as u64,
        b"_$sSS8UTF8ViewV13_foreignCountSiyF" => foreign_count as *const () as u64,
        b"_$ss11_StringGutsV8copyUTF84intoSiSgSrys5UInt8VG_tF" => copy_utf8 as *const () as u64,
        _ => return None,
    };
    Some(found)
}
