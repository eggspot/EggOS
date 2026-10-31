//! Android's system properties (D188): the property area that bionic's
//! `__system_property_get` reads, the lines of a `build.prop` file, and the request `setprop`
//! sends the kernel's property service.
//!
//! The area is bionic's single "pre-split" area, the one bionic maps read-only when
//! `/dev/__properties__` is a file: a [`HEADER_SIZE`]-byte header (the bytes used after it, a
//! serial that changes with every change, [`MAGIC`] and [`VERSION`]), then a trie. Each level
//! of the trie is one dot-separated part of a name, kept as a binary tree of nodes ordered by
//! length, then by bytes; a node may point to the record of the property its path names. A
//! record holds a serial (the value's length in its top byte), the value with its NUL in
//! [`VALUE_MAX`] bytes, and the whole name. Offsets are from the end of the header, in the
//! machine's byte order. The kernel writes the area; programs and `getprop` only read it.
//! Nothing here allocates or panics.

#![cfg_attr(not(test), no_std)]

use core::cmp::Ordering;
use core::ffi::CStr;

/// Where programs find the area.
pub const AREA_PATH: &CStr = c"/dev/__properties__";
/// The area's name in `/dev`.
pub const AREA_NAME: &[u8] = b"__properties__";
/// The file of properties the property service sets at boot, Android's own.
pub const BUILD_PROPERTIES_PATH: &[u8] = b"/system/build.prop";
/// The header's size: the trie starts after it.
pub const HEADER_SIZE: usize = 128;
/// The header's magic number.
pub const MAGIC: u32 = 0x504f_5250;
/// The area format's version.
pub const VERSION: u32 = 0xfc6e_d0ab;
/// A value's room, its NUL included: values are at most `VALUE_MAX - 1` bytes.
pub const VALUE_MAX: usize = 92;
/// The longest name.
pub const NAME_MAX: usize = 255;
/// The size of the request [`encode_request`] makes.
pub const REQUEST_SIZE: usize = NAME_AT + NAME_MAX + 1 + VALUE_MAX;
/// The `ioctl` request on the area that sets a property: `_IOW('P', 1, [u8; REQUEST_SIZE])`.
pub const SET_REQUEST: u32 = 0x4000_0000 | ((REQUEST_SIZE as u32) << 16) | (0x50 << 8) | 1;

const BYTES_USED: usize = 0;
const AREA_SERIAL: usize = 4;
const MAGIC_AT: usize = 8;
const VERSION_AT: usize = 12;
const NODE_SIZE: usize = 20;
const NAME_LENGTH: usize = 0;
const PROPERTY: usize = 4;
const LEFT: usize = 8;
const RIGHT: usize = 12;
const CHILDREN: usize = 16;
const RECORD_SIZE: usize = 4 + VALUE_MAX;
const FIRST_FREE: usize = NODE_SIZE + VALUE_MAX;
const LENGTH_SHIFT: u32 = 24;
const LONG_VALUE: u32 = 1 << 16;
const COUNTER: u32 = 0x00ff_ffff;
const PAGE: usize = 4096;
const WALK_DEPTH: usize = 512;
const EMIT: u32 = 1 << 31;
const NAME_AT: usize = 8;
const VALUE_AT: usize = NAME_AT + NAME_MAX + 1;
const READ_ONLY_PREFIX: &[u8] = b"ro.";

/// The smallest area: its header and the trie's root.
pub const SMALLEST_AREA: usize = HEADER_SIZE + FIRST_FREE;

/// Why a property could not be set or the area read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropertyError {
    /// Not a property name: 1 to [`NAME_MAX`] bytes of parts separated by single dots, each
    /// of letters, digits, `_`, `-`, `:` and `@`.
    InvalidName,
    /// The value is longer than `VALUE_MAX - 1` bytes or holds a NUL.
    InvalidValue,
    /// The name starts with `ro.` and the property has a value already.
    ReadOnly,
    /// The area has no room: an area of `needed` bytes would have.
    Full {
        /// The area's size that would hold the change.
        needed: usize,
    },
    /// The bytes are not a property area, or it is damaged.
    NotAnArea,
    /// The trie is deeper than a walk follows.
    TooDeep,
}

/// Whether `name` is a property name (see [`PropertyError::InvalidName`]).
pub fn is_valid_name(name: &[u8]) -> bool {
    !name.is_empty()
        && name.len() <= NAME_MAX
        && name.split(|&byte| byte == b'.').all(|part| {
            !part.is_empty()
                && part
                    .iter()
                    .all(|&byte| byte.is_ascii_alphanumeric() || b"_-:@".contains(&byte))
        })
}

/// Whether `value` can be a property's value.
pub fn is_valid_value(value: &[u8]) -> bool {
    value.len() < VALUE_MAX && !value.contains(&0)
}

/// Whether `name` is a read-only property's, which keeps its first value.
pub fn is_read_only(name: &[u8]) -> bool {
    name.starts_with(READ_ONLY_PREFIX)
}

/// Makes `area` an empty property area: [`PropertyError::Full`] when it is shorter than
/// [`SMALLEST_AREA`].
pub fn initialize(area: &mut [u8]) -> Result<(), PropertyError> {
    if area.len() < SMALLEST_AREA {
        return Err(PropertyError::Full {
            needed: SMALLEST_AREA,
        });
    }
    area.fill(0);
    store(area, BYTES_USED, FIRST_FREE as u32)?;
    store(area, MAGIC_AT, MAGIC)?;
    store(area, VERSION_AT, VERSION)
}

/// Whether `area` starts with a property area's header.
pub fn is_area(area: &[u8]) -> bool {
    area.len() >= SMALLEST_AREA
        && word(area, MAGIC_AT) == Some(MAGIC)
        && word(area, VERSION_AT) == Some(VERSION)
}

/// The area's serial, which changes with every change to it.
pub fn serial(area: &[u8]) -> Option<u32> {
    is_area(area).then(|| word(area, AREA_SERIAL)).flatten()
}

/// The bytes of `area` in use: the header and the trie.
pub fn used(area: &[u8]) -> Option<usize> {
    let used = usize::try_from(word(area, BYTES_USED)?).ok()?;
    is_area(area).then_some(HEADER_SIZE.checked_add(used)?)
}

/// The value of property `name` in `area`, if it has one.
pub fn get<'a>(area: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
    if !is_area(area) || !is_valid_name(name) {
        return None;
    }
    let mut node = 0;
    for part in name.split(|&byte| byte == b'.') {
        node = child(area, node, part)?;
    }
    match word(area, field(node, PROPERTY)?)? {
        0 => None,
        record => value_of(area, record),
    }
}

/// Calls `visit` with each property's name and value, in the trie's order: within a level,
/// shorter parts first, then by bytes, each node's own property before those below it.
pub fn for_each(area: &[u8], mut visit: impl FnMut(&[u8], &[u8])) -> Result<(), PropertyError> {
    if !is_area(area) {
        return Err(PropertyError::NotAnArea);
    }
    let mut stack = [0u32; WALK_DEPTH];
    let mut depth = 0usize;
    let first = load(area, field(0, CHILDREN).ok_or(PropertyError::NotAnArea)?)?;
    if first != 0 {
        stack[0] = first;
        depth = 1;
    }
    let mut steps = 0;
    while let Some(top) = depth.checked_sub(1) {
        depth = top;
        let task = stack.get(top).copied().ok_or(PropertyError::NotAnArea)?;
        steps += 1;
        if steps > area.len() {
            return Err(PropertyError::NotAnArea);
        }
        if task & EMIT != 0 {
            let record = task & !EMIT;
            if let (Some(name), Some(value)) = (name_of(area, record), value_of(area, record)) {
                visit(name, value);
            }
            continue;
        }
        let at = |offset| field(task, offset).ok_or(PropertyError::NotAnArea);
        let property = load(area, at(PROPERTY)?)?;
        let emit = if property == 0 { 0 } else { property | EMIT };
        let next = [
            load(area, at(RIGHT)?)?,
            load(area, at(CHILDREN)?)?,
            emit,
            load(area, at(LEFT)?)?,
        ];
        for task in next.into_iter().filter(|&task| task != 0) {
            *stack.get_mut(depth).ok_or(PropertyError::TooDeep)? = task;
            depth += 1;
        }
    }
    Ok(())
}

/// Sets property `name` to `value` in `area`. A read-only property (`ro.`) keeps its first
/// value. On [`PropertyError::Full`] the area may hold new empty nodes of the name's path,
/// which a retry in a larger copy of the area finds again.
pub fn set(area: &mut [u8], name: &[u8], value: &[u8]) -> Result<(), PropertyError> {
    if !is_valid_name(name) {
        return Err(PropertyError::InvalidName);
    }
    if !is_valid_value(value) {
        return Err(PropertyError::InvalidValue);
    }
    if !is_area(area) {
        return Err(PropertyError::NotAnArea);
    }
    let mut node = 0;
    for part in name.split(|&byte| byte == b'.') {
        node = child_or_new(area, node, part)?;
    }
    let link = field(node, PROPERTY).ok_or(PropertyError::NotAnArea)?;
    let length = value.len() as u32;
    match load(area, link)? {
        0 => {
            let record = allocate(area, RECORD_SIZE + name.len() + 1)?;
            let at = field(record, 0).ok_or(PropertyError::NotAnArea)?;
            write_value(area, at, value)?;
            bytes_mut(area, at + RECORD_SIZE, name.len())?.copy_from_slice(name);
            store(area, at, length << LENGTH_SHIFT)?;
            store(area, link, record)?;
        }
        _ if is_read_only(name) => return Err(PropertyError::ReadOnly),
        record => {
            let at = field(record, 0).ok_or(PropertyError::NotAnArea)?;
            let counter = (load(area, at)? & COUNTER).wrapping_add(2) & COUNTER & !1;
            write_value(area, at, value)?;
            store(area, at, (length << LENGTH_SHIFT) | counter)?;
        }
    }
    let serial = load(area, AREA_SERIAL)?.wrapping_add(1);
    store(area, AREA_SERIAL, serial)
}

/// The property a `build.prop` line sets, `name=value` with spaces around either trimmed;
/// `None` for a blank line, a `#` comment or a line that sets nothing.
pub fn assignment(line: &[u8]) -> Option<(&[u8], &[u8])> {
    let line = line.trim_ascii();
    if line.first() == Some(&b'#') {
        return None;
    }
    let equals = line.iter().position(|&byte| byte == b'=')?;
    let name = line.get(..equals)?.trim_ascii();
    let value = line.get(equals + 1..)?.trim_ascii();
    (is_valid_name(name) && is_valid_value(value)).then_some((name, value))
}

/// The [`SET_REQUEST`] argument that sets `name` to `value`: both lengths as native `u32`s,
/// then the name and the value, each in a fixed room. `None` when either is too long.
pub fn encode_request(name: &[u8], value: &[u8]) -> Option<[u8; REQUEST_SIZE]> {
    if name.len() > NAME_MAX || value.len() >= VALUE_MAX {
        return None;
    }
    let mut request = [0; REQUEST_SIZE];
    request
        .get_mut(..4)?
        .copy_from_slice(&(name.len() as u32).to_ne_bytes());
    request
        .get_mut(4..NAME_AT)?
        .copy_from_slice(&(value.len() as u32).to_ne_bytes());
    request
        .get_mut(NAME_AT..NAME_AT + name.len())?
        .copy_from_slice(name);
    request
        .get_mut(VALUE_AT..VALUE_AT + value.len())?
        .copy_from_slice(value);
    Some(request)
}

/// The name and value in a [`SET_REQUEST`] argument; `None` when its lengths do not fit.
pub fn decode_request(request: &[u8; REQUEST_SIZE]) -> Option<(&[u8], &[u8])> {
    let name_length = usize::try_from(word(request, 0)?).ok()?;
    let value_length = usize::try_from(word(request, 4)?).ok()?;
    if name_length > NAME_MAX || value_length >= VALUE_MAX {
        return None;
    }
    Some((
        request.get(NAME_AT..NAME_AT + name_length)?,
        request.get(VALUE_AT..VALUE_AT + value_length)?,
    ))
}

fn word(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_ne_bytes(
        bytes.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn load(area: &[u8], at: usize) -> Result<u32, PropertyError> {
    word(area, at).ok_or(PropertyError::NotAnArea)
}

fn store(area: &mut [u8], at: usize, value: u32) -> Result<(), PropertyError> {
    bytes_mut(area, at, 4)?.copy_from_slice(&value.to_ne_bytes());
    Ok(())
}

fn bytes_mut(area: &mut [u8], at: usize, length: usize) -> Result<&mut [u8], PropertyError> {
    at.checked_add(length)
        .and_then(|end| area.get_mut(at..end))
        .ok_or(PropertyError::NotAnArea)
}

fn field(object: u32, offset: usize) -> Option<usize> {
    HEADER_SIZE
        .checked_add(usize::try_from(object).ok()?)?
        .checked_add(offset)
}

fn compare(part: &[u8], other: &[u8]) -> Ordering {
    part.len().cmp(&other.len()).then_with(|| part.cmp(other))
}

fn node_name(area: &[u8], node: u32) -> Option<&[u8]> {
    let length = usize::try_from(word(area, field(node, NAME_LENGTH)?)?).ok()?;
    let start = field(node, NODE_SIZE)?;
    area.get(start..start.checked_add(length)?)
}

fn child(area: &[u8], parent: u32, part: &[u8]) -> Option<u32> {
    let mut current = word(area, field(parent, CHILDREN)?)?;
    for _ in 0..area.len() / NODE_SIZE {
        if current == 0 {
            return None;
        }
        let side = match compare(part, node_name(area, current)?) {
            Ordering::Equal => return Some(current),
            Ordering::Less => LEFT,
            Ordering::Greater => RIGHT,
        };
        current = word(area, field(current, side)?)?;
    }
    None
}

fn child_or_new(area: &mut [u8], parent: u32, part: &[u8]) -> Result<u32, PropertyError> {
    let mut link = field(parent, CHILDREN).ok_or(PropertyError::NotAnArea)?;
    for _ in 0..area.len() / NODE_SIZE {
        let current = load(area, link)?;
        if current == 0 {
            let node = allocate(area, NODE_SIZE + part.len() + 1)?;
            let at = field(node, 0).ok_or(PropertyError::NotAnArea)?;
            store(area, at + NAME_LENGTH, part.len() as u32)?;
            bytes_mut(area, at + NODE_SIZE, part.len())?.copy_from_slice(part);
            store(area, link, node)?;
            return Ok(node);
        }
        let name = node_name(area, current).ok_or(PropertyError::NotAnArea)?;
        let side = match compare(part, name) {
            Ordering::Equal => return Ok(current),
            Ordering::Less => LEFT,
            Ordering::Greater => RIGHT,
        };
        link = field(current, side).ok_or(PropertyError::NotAnArea)?;
    }
    Err(PropertyError::NotAnArea)
}

fn allocate(area: &mut [u8], size: usize) -> Result<u32, PropertyError> {
    let size = size.next_multiple_of(4);
    let used = usize::try_from(load(area, BYTES_USED)?).map_err(|_| PropertyError::NotAnArea)?;
    let mut start = HEADER_SIZE + used;
    if size <= PAGE && start / PAGE != (start + size - 1) / PAGE {
        start = (start / PAGE + 1) * PAGE;
    }
    let end = start + size;
    if end > area.len() {
        return Err(PropertyError::Full { needed: end });
    }
    let used = u32::try_from(end - HEADER_SIZE).map_err(|_| PropertyError::Full { needed: end })?;
    store(area, BYTES_USED, used)?;
    u32::try_from(start - HEADER_SIZE).map_err(|_| PropertyError::NotAnArea)
}

fn write_value(area: &mut [u8], record_at: usize, value: &[u8]) -> Result<(), PropertyError> {
    let room = bytes_mut(area, record_at + 4, VALUE_MAX)?;
    room.fill(0);
    room.get_mut(..value.len())
        .ok_or(PropertyError::InvalidValue)?
        .copy_from_slice(value);
    Ok(())
}

fn value_of(area: &[u8], record: u32) -> Option<&[u8]> {
    let serial = word(area, field(record, 0)?)?;
    if serial & LONG_VALUE != 0 {
        return None;
    }
    let length = usize::try_from(serial >> LENGTH_SHIFT).ok()?;
    if length >= VALUE_MAX {
        return None;
    }
    let start = field(record, 4)?;
    area.get(start..start + length)
}

fn name_of(area: &[u8], record: u32) -> Option<&[u8]> {
    let rest = area.get(field(record, RECORD_SIZE)?..)?;
    let end = rest.iter().position(|&byte| byte == 0)?;
    rest.get(..end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(size: usize) -> Vec<u8> {
        let mut area = vec![0; size];
        initialize(&mut area).unwrap();
        area
    }

    fn listed(area: &[u8]) -> Vec<(String, String)> {
        let mut found = Vec::new();
        for_each(area, |name, value| {
            found.push((
                String::from_utf8_lossy(name).into_owned(),
                String::from_utf8_lossy(value).into_owned(),
            ));
        })
        .unwrap();
        found
    }

    #[test]
    fn an_empty_area_has_bionic_s_header_and_no_properties() {
        let area = area(4096);
        assert_eq!(&area[8..12], &MAGIC.to_ne_bytes());
        assert_eq!(&area[12..16], &VERSION.to_ne_bytes());
        assert_eq!(word(&area, 0), Some(112));
        assert_eq!(used(&area), Some(240));
        assert_eq!(serial(&area), Some(0));
        assert_eq!(get(&area, b"ro.build.version.sdk"), None);
        assert!(listed(&area).is_empty());
        assert!(!is_area(&area[..100]));
        assert_eq!(
            initialize(&mut [0; 200]),
            Err(PropertyError::Full { needed: 240 })
        );
    }

    #[test]
    fn a_set_property_is_found_by_its_name_and_its_record_is_bionic_s() {
        let mut area = area(4096);
        set(&mut area, b"ro.build.version.sdk", b"29").unwrap();
        assert_eq!(get(&area, b"ro.build.version.sdk"), Some(&b"29"[..]));
        assert_eq!(get(&area, b"ro.build.version"), None);
        assert_eq!(get(&area, b"ro.build.version.sdkx"), None);
        let ro = word(&area, HEADER_SIZE + CHILDREN).unwrap();
        assert_eq!(ro, 112);
        assert_eq!(node_name(&area, ro), Some(&b"ro"[..]));
        let mut node = ro;
        for _ in 0..3 {
            node = word(&area, field(node, CHILDREN).unwrap()).unwrap();
        }
        assert_eq!(node_name(&area, node), Some(&b"sdk"[..]));
        let record = word(&area, field(node, PROPERTY).unwrap()).unwrap();
        let at = field(record, 0).unwrap();
        assert_eq!(word(&area, at), Some(2 << 24));
        assert_eq!(&area[at + 4..at + 7], b"29\0");
        assert_eq!(&area[at + 96..at + 117], b"ro.build.version.sdk\0");
        assert_eq!(serial(&area), Some(1));
    }

    #[test]
    fn siblings_are_ordered_by_length_then_bytes_and_listed_in_that_order() {
        let mut area = area(4096);
        for (name, value) in [
            ("persist.sys.locale", "en-US"),
            ("ro.hardware", "eggos"),
            ("ro.product.cpu.abi", "arm64-v8a"),
            ("ro.debuggable", "0"),
            ("ro", "root"),
            ("eggos.test", "x"),
        ] {
            set(&mut area, name.as_bytes(), value.as_bytes()).unwrap();
        }
        let first = word(&area, HEADER_SIZE + CHILDREN).unwrap();
        assert_eq!(node_name(&area, first), Some(&b"persist"[..]));
        let left = word(&area, field(first, LEFT).unwrap()).unwrap();
        assert_eq!(node_name(&area, left), Some(&b"ro"[..]));
        let names: Vec<String> = listed(&area).into_iter().map(|(name, _)| name).collect();
        assert_eq!(
            names,
            [
                "ro",
                "ro.product.cpu.abi",
                "ro.hardware",
                "ro.debuggable",
                "eggos.test",
                "persist.sys.locale",
            ]
        );
    }

    #[test]
    fn changing_a_value_bumps_its_serial_and_read_only_ones_keep_their_first() {
        let mut area = area(4096);
        set(&mut area, b"debug.level", b"verbose").unwrap();
        set(&mut area, b"debug.level", b"1").unwrap();
        assert_eq!(get(&area, b"debug.level"), Some(&b"1"[..]));
        let used_before = used(&area);
        set(&mut area, b"debug.level", b"").unwrap();
        assert_eq!(get(&area, b"debug.level"), Some(&b""[..]));
        assert_eq!(used(&area), used_before);
        assert_eq!(serial(&area), Some(3));
        set(&mut area, b"ro.secure", b"1").unwrap();
        assert_eq!(
            set(&mut area, b"ro.secure", b"0"),
            Err(PropertyError::ReadOnly)
        );
        assert_eq!(get(&area, b"ro.secure"), Some(&b"1"[..]));
    }

    #[test]
    fn names_and_values_are_checked() {
        let mut area = area(4096);
        for name in ["", ".a", "a.", "a..b", "a b", "a/b", "é"] {
            assert_eq!(
                set(&mut area, name.as_bytes(), b"1"),
                Err(PropertyError::InvalidName),
                "{name}"
            );
        }
        assert!(is_valid_name(b"vendor.wifi_hal-1:x@2"));
        assert!(!is_valid_name(&[b'a'; NAME_MAX + 1]));
        assert_eq!(
            set(&mut area, b"a", &[b'v'; VALUE_MAX]),
            Err(PropertyError::InvalidValue)
        );
        assert_eq!(
            set(&mut area, b"a", b"x\0y"),
            Err(PropertyError::InvalidValue)
        );
        set(&mut area, b"a", &[b'v'; VALUE_MAX - 1]).unwrap();
        assert_eq!(get(&area, b"a").map(<[u8]>::len), Some(VALUE_MAX - 1));
        assert_eq!(
            set(&mut [0; 4096], b"a", b"1"),
            Err(PropertyError::NotAnArea)
        );
    }

    #[test]
    fn a_full_area_says_how_large_one_must_be_and_a_larger_copy_takes_the_change() {
        let mut area = area(SMALLEST_AREA + 40);
        let mut grown = Vec::new();
        loop {
            match set(&mut area, b"ro.hardware", b"eggos") {
                Ok(()) => break,
                Err(PropertyError::Full { needed }) => {
                    grown.push(needed);
                    area.resize(needed, 0);
                }
                Err(other) => panic!("{other:?}"),
            }
        }
        assert_eq!(grown, [296, 404]);
        assert_eq!(get(&area, b"ro.hardware"), Some(&b"eggos"[..]));
        assert_eq!(listed(&area).len(), 1);
    }

    #[test]
    fn no_record_or_node_crosses_a_page() {
        let mut area = area(64 * 1024);
        for index in 0..300 {
            let name = format!("eggos.test.property{index}");
            set(&mut area, name.as_bytes(), b"a value of some length").unwrap();
        }
        let mut found = 0;
        for_each(&area, |name, _| {
            let record = name.as_ptr() as usize - area.as_ptr() as usize - RECORD_SIZE;
            assert_eq!(record / PAGE, (record + RECORD_SIZE + name.len()) / PAGE);
            found += 1;
        })
        .unwrap();
        assert_eq!(found, 300);
        assert_eq!(
            get(&area, b"eggos.test.property299"),
            Some(&b"a value of some length"[..])
        );
    }

    #[test]
    fn a_damaged_area_is_refused_without_a_panic() {
        let mut area = area(4096);
        set(&mut area, b"a.b", b"1").unwrap();
        store(&mut area, HEADER_SIZE + CHILDREN, 4000).unwrap();
        assert_eq!(get(&area, b"a.b"), None);
        assert!(for_each(&area, |_, _| {}).is_err());
        let first = word(&area, HEADER_SIZE + CHILDREN).unwrap();
        assert_eq!(first, 4000);
        store(&mut area, HEADER_SIZE + CHILDREN, 112).unwrap();
        store(&mut area, HEADER_SIZE + 112 + LEFT, 112).unwrap();
        assert_eq!(get(&area, b"a.b"), Some(&b"1"[..]));
        assert_eq!(get(&area, b"0"), None);
        assert!(for_each(&area, |_, _| {}).is_err());
    }

    #[test]
    fn build_prop_lines_set_properties_and_comments_and_blanks_set_nothing() {
        assert_eq!(
            assignment(b"  ro.build.version.sdk = 29 \r"),
            Some((&b"ro.build.version.sdk"[..], &b"29"[..]))
        );
        assert_eq!(
            assignment(b"ro.product.model="),
            Some((&b"ro.product.model"[..], &b""[..]))
        );
        assert_eq!(assignment(b"a=b=c"), Some((&b"a"[..], &b"b=c"[..])));
        for line in ["", "   ", "# ro.a=1", "import /vendor/build.prop", "a b=1"] {
            assert_eq!(assignment(line.as_bytes()), None, "{line}");
        }
    }

    #[test]
    fn a_request_carries_the_name_and_value_and_refuses_lengths_that_do_not_fit() {
        let request = encode_request(b"eggos.greeting", b"hello").unwrap();
        assert_eq!(
            decode_request(&request),
            Some((&b"eggos.greeting"[..], &b"hello"[..]))
        );
        assert_eq!(encode_request(&[b'a'; NAME_MAX + 1], b""), None);
        assert_eq!(encode_request(b"a", &[b'v'; VALUE_MAX]), None);
        let mut damaged = request;
        damaged[..4].copy_from_slice(&1000u32.to_ne_bytes());
        assert_eq!(decode_request(&damaged), None);
        assert_eq!(REQUEST_SIZE, 356);
        assert_eq!(SET_REQUEST, 0x4164_5001);
    }
}
