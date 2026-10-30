//! Initialization files (`GetPrivateProfileStringW` and its family, D178): real files read and
//! rewritten whole on each call through `crate::ini`, in the encoding they have (UTF-16 with
//! its byte order mark, or UTF-8, the layer's ANSI code page); a new file is UTF-8. A file
//! named without a folder is in the Windows folder, as on Windows.

use super::{Bool, FALSE, TRUE, error, error_for, fail, heap};
use crate::ini::{self, Change, Line};
use crate::library::Export;
use crate::linux::{self, O_CREAT, O_RDONLY, O_TRUNC, O_WRONLY};
use crate::text::{self, PATH_CAPACITY};

const ENOENT: u16 = 2;
const MAX_FILE_SIZE: u64 = 16 << 20;
const UTF16_MARK: [u8; 2] = [0xff, 0xfe];
const UTF8_MARK: [u8; 3] = [0xef, 0xbb, 0xbf];
const EQUALS: &[u16] = &[b'=' as u16];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Utf16,
    Utf8 { marked: bool },
}

/// A file's text as UTF-16 units in a heap block, and how it was stored.
struct Loaded {
    units: *mut u16,
    length: usize,
    encoding: Encoding,
    existed: bool,
}

impl Loaded {
    fn text(&self) -> &[u16] {
        match self.units.is_null() {
            true => &[],
            // SAFETY: the block holds `length` units, filled when the file was read.
            false => unsafe { core::slice::from_raw_parts(self.units, self.length) },
        }
    }
}

impl Drop for Loaded {
    fn drop(&mut self) {
        // SAFETY: the block is this file's alone, or null.
        unsafe { heap::free(self.units.cast()) };
    }
}

struct Block(*mut u8);

impl Drop for Block {
    fn drop(&mut self) {
        // SAFETY: the block is this value's alone, or null.
        unsafe { heap::free(self.0) };
    }
}

unsafe fn file_path(name: *const u16, out: &mut [u8; PATH_CAPACITY]) -> Option<usize> {
    if name.is_null() {
        return None;
    }
    // SAFETY: the callers' contract: a NUL-terminated name.
    let units = unsafe { super::file::wide_string(name) };
    let bare = !units.iter().any(|unit| {
        *unit == u16::from(b'\\') || *unit == u16::from(b'/') || *unit == u16::from(b':')
    });
    if !bare {
        return text::eggos_path(text::utf16_chars(units), out);
    }
    let folder = super::system::WINDOWS_FOLDER;
    let chars = folder
        .iter()
        .map(|byte| char::from(*byte))
        .chain(['/'])
        .chain(text::utf16_chars(units));
    text::eggos_path(chars, out)
}

fn read_bytes(path: &[u8]) -> Result<Option<(Block, usize)>, u32> {
    let fd = match linux::open(path, O_RDONLY) {
        Ok(fd) => fd,
        Err(errno) if errno.0 == ENOENT => return Ok(None),
        Err(errno) => return Err(super::directory::path_error(path, errno)),
    };
    let read = read_open(fd);
    let _ = linux::close(fd);
    read.map(Some)
}

fn read_open(fd: usize) -> Result<(Block, usize), u32> {
    let size = linux::fstat(fd).map_err(error_for)?[linux::STAT_SIZE_WORD];
    if size > MAX_FILE_SIZE {
        return Err(error::NOT_ENOUGH_MEMORY);
    }
    let block = Block(heap::allocate(size as usize + 1));
    if block.0.is_null() {
        return Err(error::NOT_ENOUGH_MEMORY);
    }
    // SAFETY: the block holds `size + 1` bytes.
    let buffer = unsafe { core::slice::from_raw_parts_mut(block.0, size as usize) };
    let mut filled = 0;
    while filled < buffer.len() {
        match linux::read(fd, &mut buffer[filled..]) {
            Ok(0) => break,
            Ok(count) => filled += count,
            Err(errno) => return Err(error_for(errno)),
        }
    }
    Ok((block, filled))
}

fn load(path: &[u8]) -> Result<Loaded, u32> {
    let Some((block, length)) = read_bytes(path)? else {
        return Ok(Loaded {
            units: core::ptr::null_mut(),
            length: 0,
            encoding: Encoding::Utf8 { marked: false },
            existed: false,
        });
    };
    // SAFETY: `read_bytes` filled `length` bytes of the block.
    let bytes = unsafe { core::slice::from_raw_parts(block.0, length) };
    let (encoding, body) = if let Some(body) = bytes.strip_prefix(&UTF16_MARK) {
        (Encoding::Utf16, body)
    } else if let Some(body) = bytes.strip_prefix(&UTF8_MARK) {
        (Encoding::Utf8 { marked: true }, body)
    } else {
        (Encoding::Utf8 { marked: false }, bytes)
    };
    let count = match encoding {
        Encoding::Utf16 => body.len() / 2,
        Encoding::Utf8 { .. } => text::utf16_length(body),
    };
    let units = heap::allocate(count.max(1) * 2).cast::<u16>();
    if units.is_null() {
        return Err(error::NOT_ENOUGH_MEMORY);
    }
    // SAFETY: a fresh block of at least `count` units.
    let out = unsafe { core::slice::from_raw_parts_mut(units, count) };
    match encoding {
        Encoding::Utf16 => {
            for (unit, pair) in out.iter_mut().zip(body.chunks_exact(2)) {
                *unit = u16::from_le_bytes([pair[0], pair[1]]);
            }
        }
        Encoding::Utf8 { .. } => {
            text::write_utf16(body, out);
        }
    }
    Ok(Loaded {
        units,
        length: count,
        encoding,
        existed: true,
    })
}

fn save(path: &[u8], units: &[u16], encoding: Encoding) -> Result<(), u32> {
    let (mark, body_length): (&[u8], usize) = match encoding {
        Encoding::Utf16 => (&UTF16_MARK[..], units.len() * 2),
        Encoding::Utf8 { marked } => (
            if marked { &UTF8_MARK[..] } else { &[] },
            text::utf8_length(units),
        ),
    };
    let length = mark.len() + body_length;
    let block = Block(heap::allocate(length.max(1)));
    if block.0.is_null() {
        return Err(error::NOT_ENOUGH_MEMORY);
    }
    // SAFETY: a fresh block of at least `length` bytes.
    let bytes = unsafe { core::slice::from_raw_parts_mut(block.0, length) };
    bytes[..mark.len()].copy_from_slice(mark);
    let body = &mut bytes[mark.len()..];
    match encoding {
        Encoding::Utf16 => {
            for (pair, unit) in body.chunks_exact_mut(2).zip(units) {
                pair.copy_from_slice(&unit.to_le_bytes());
            }
        }
        Encoding::Utf8 { .. } => {
            text::write_utf8(units, body);
        }
    }
    let fd = linux::open(path, O_WRONLY | O_CREAT | O_TRUNC)
        .map_err(|errno| super::directory::path_error(path, errno))?;
    let written = linux::write_all(fd, bytes).map_err(error_for);
    let _ = linux::close(fd);
    written
}

fn apply(path: &[u8], section: &[u16], change: Change<'_>) -> Result<(), u32> {
    let loaded = load(path)?;
    let removal = matches!(change, Change::RemoveKey(_) | Change::RemoveSection);
    if !loaded.existed && removal {
        return Ok(());
    }
    let mut count = 0;
    ini::change(loaded.text(), section, change, &mut |piece| {
        count += piece.len()
    });
    let block = Block(heap::allocate(count.max(1) * 2));
    if block.0.is_null() {
        return Err(error::NOT_ENOUGH_MEMORY);
    }
    // SAFETY: a fresh block of at least `count` units.
    let out = unsafe { core::slice::from_raw_parts_mut(block.0.cast::<u16>(), count) };
    let mut at = 0;
    ini::change(loaded.text(), section, change, &mut |piece| {
        out[at..at + piece.len()].copy_from_slice(piece);
        at += piece.len();
    });
    save(path, out, loaded.encoding)
}

/// Fills a caller's buffer with strings each ended by a NUL and the list by another, as the
/// profile functions do: a list that does not fit is cut, ended by two NULs, and its length is
/// the buffer's less two.
struct ListWriter<'a> {
    out: &'a mut [u16],
    at: usize,
    cut: bool,
}

impl ListWriter<'_> {
    fn push(&mut self, pieces: &[&[u16]]) {
        if self.cut {
            return;
        }
        let length: usize = pieces.iter().map(|piece| piece.len()).sum();
        if self.at + length + 2 <= self.out.len() {
            for piece in pieces {
                self.out[self.at..self.at + piece.len()].copy_from_slice(piece);
                self.at += piece.len();
            }
            self.out[self.at] = 0;
            self.at += 1;
            return;
        }
        self.cut = true;
        let room = self.out.len().saturating_sub(2);
        for unit in pieces.iter().flat_map(|piece| piece.iter()) {
            if self.at >= room {
                break;
            }
            self.out[self.at] = *unit;
            self.at += 1;
        }
    }

    fn finish(self) -> u32 {
        let size = self.out.len();
        if self.cut {
            if size >= 2 {
                self.out[size - 2] = 0;
                self.out[size - 1] = 0;
            }
            return size.saturating_sub(2) as u32;
        }
        if self.at < size {
            self.out[self.at] = 0;
        }
        if self.at == 0 && size > 1 {
            self.out[1] = 0;
        }
        self.at as u32
    }
}

unsafe fn optional_string<'a>(text: *const u16) -> Option<&'a [u16]> {
    match text.is_null() {
        true => None,
        // SAFETY: the callers' contract: a NUL-terminated string, or null.
        false => Some(unsafe { super::file::wide_string(text) }),
    }
}

fn found_or_missing(loaded: &Loaded) {
    let code = if loaded.existed {
        error::SUCCESS
    } else {
        error::FILE_NOT_FOUND
    };
    super::thread_environment::set_last_error(code);
}

unsafe extern "efiapi" fn get_private_profile_string_w(
    section: *const u16,
    key: *const u16,
    default: *const u16,
    buffer: *mut u16,
    size: u32,
    file: *const u16,
) -> u32 {
    let mut path = [0u8; PATH_CAPACITY];
    // SAFETY: `GetPrivateProfileStringW`'s contract: a NUL-terminated file name.
    let Some(length) = (unsafe { file_path(file, &mut path) }) else {
        return fail(error::INVALID_PARAMETER, 0);
    };
    if buffer.is_null() || size == 0 {
        return fail(error::INVALID_PARAMETER, 0);
    }
    let loaded = match load(&path[..length]) {
        Ok(loaded) => loaded,
        Err(code) => return fail(code, 0),
    };
    // SAFETY: the caller passes `size` writable units.
    let out = unsafe { super::file::wide_buffer(buffer, size as usize) };
    let text = loaded.text();
    // SAFETY: as above: a NUL-terminated section name, or null.
    let section = unsafe { optional_string(section) };
    // SAFETY: as above: a NUL-terminated key, or null.
    let key = unsafe { optional_string(key) };
    let result = match (section, key) {
        (None, _) => {
            let mut list = ListWriter {
                out,
                at: 0,
                cut: false,
            };
            ini::section_names(text, &mut |name| list.push(&[name]));
            list.finish()
        }
        (Some(section), None) => {
            let mut list = ListWriter {
                out,
                at: 0,
                cut: false,
            };
            ini::section_lines(text, section, &mut |line| {
                if let Line::Entry { key, .. } = line {
                    list.push(&[key]);
                }
            });
            list.finish()
        }
        (Some(section), Some(key)) => {
            // SAFETY: as above.
            let default = unsafe { optional_string(default) }.unwrap_or(&[]);
            let default_end = default
                .iter()
                .rposition(|unit| *unit != u16::from(b' '))
                .map_or(0, |last| last + 1);
            let value = ini::value(text, section, key).unwrap_or(&default[..default_end]);
            let copied = value.len().min(out.len() - 1);
            out[..copied].copy_from_slice(&value[..copied]);
            out[copied] = 0;
            copied as u32
        }
    };
    found_or_missing(&loaded);
    result
}

unsafe extern "efiapi" fn get_private_profile_section_w(
    section: *const u16,
    buffer: *mut u16,
    size: u32,
    file: *const u16,
) -> u32 {
    let mut path = [0u8; PATH_CAPACITY];
    // SAFETY: `GetPrivateProfileSectionW`'s contract: a NUL-terminated file name.
    let Some(length) = (unsafe { file_path(file, &mut path) }) else {
        return fail(error::INVALID_PARAMETER, 0);
    };
    // SAFETY: as above: a NUL-terminated section name.
    let Some(section) = (unsafe { optional_string(section) }) else {
        return fail(error::INVALID_PARAMETER, 0);
    };
    let loaded = match load(&path[..length]) {
        Ok(loaded) => loaded,
        Err(code) => return fail(code, 0),
    };
    // SAFETY: the caller passes `size` writable units, or null.
    let out = unsafe { super::file::wide_buffer(buffer, size as usize) };
    let mut list = ListWriter {
        out,
        at: 0,
        cut: false,
    };
    ini::section_lines(loaded.text(), section, &mut |line| match line {
        Line::Entry { key, value } => list.push(&[key, EQUALS, value]),
        Line::Other(other) => list.push(&[other]),
        _ => {}
    });
    found_or_missing(&loaded);
    list.finish()
}

unsafe extern "efiapi" fn get_private_profile_section_names_w(
    buffer: *mut u16,
    size: u32,
    file: *const u16,
) -> u32 {
    let mut path = [0u8; PATH_CAPACITY];
    // SAFETY: `GetPrivateProfileSectionNamesW`'s contract: a NUL-terminated file name.
    let Some(length) = (unsafe { file_path(file, &mut path) }) else {
        return fail(error::INVALID_PARAMETER, 0);
    };
    let loaded = match load(&path[..length]) {
        Ok(loaded) => loaded,
        Err(code) => return fail(code, 0),
    };
    // SAFETY: the caller passes `size` writable units, or null.
    let out = unsafe { super::file::wide_buffer(buffer, size as usize) };
    let mut list = ListWriter {
        out,
        at: 0,
        cut: false,
    };
    ini::section_names(loaded.text(), &mut |name| list.push(&[name]));
    found_or_missing(&loaded);
    list.finish()
}

fn write_result(result: Result<(), u32>) -> Bool {
    match result {
        Ok(()) => TRUE,
        Err(code) => fail(code, FALSE),
    }
}

unsafe extern "efiapi" fn write_private_profile_string_w(
    section: *const u16,
    key: *const u16,
    value: *const u16,
    file: *const u16,
) -> Bool {
    // SAFETY: `WritePrivateProfileStringW`'s contract: NUL-terminated strings, or nulls.
    let section = unsafe { optional_string(section) };
    // SAFETY: as above.
    let key = unsafe { optional_string(key) };
    // SAFETY: as above.
    let value = unsafe { optional_string(value) };
    let mut path = [0u8; PATH_CAPACITY];
    // SAFETY: as above: a NUL-terminated file name, or null.
    let length = unsafe { file_path(file, &mut path) };
    let (Some(section), Some(length)) = (section, length) else {
        return match (section, key, value) {
            (None, None, None) => TRUE,
            _ => fail(error::INVALID_PARAMETER, FALSE),
        };
    };
    let change = match (key, value) {
        (None, _) => Change::RemoveSection,
        (Some(key), None) => Change::RemoveKey(key),
        (Some(key), Some(value)) => Change::Set { key, value },
    };
    write_result(apply(&path[..length], section, change))
}

unsafe extern "efiapi" fn write_private_profile_section_w(
    section: *const u16,
    entries: *const u16,
    file: *const u16,
) -> Bool {
    // SAFETY: `WritePrivateProfileSectionW`'s contract: a NUL-terminated section name.
    let Some(section) = (unsafe { optional_string(section) }) else {
        return fail(error::INVALID_PARAMETER, FALSE);
    };
    let mut path = [0u8; PATH_CAPACITY];
    // SAFETY: as above: a NUL-terminated file name.
    let Some(length) = (unsafe { file_path(file, &mut path) }) else {
        return fail(error::INVALID_PARAMETER, FALSE);
    };
    let change = match entries.is_null() {
        true => Change::RemoveSection,
        // SAFETY: the entries are strings each ended by a NUL, the list by another.
        false => Change::ReplaceSection(unsafe { super::processes::listed(entries) }),
    };
    write_result(apply(&path[..length], section, change))
}

pub(super) const EXPORTS: &[Export] = exports![
    b"GetPrivateProfileStringW" => get_private_profile_string_w,
    b"GetPrivateProfileSectionW" => get_private_profile_section_w,
    b"GetPrivateProfileSectionNamesW" => get_private_profile_section_names_w,
    b"WritePrivateProfileStringW" => write_private_profile_string_w,
    b"WritePrivateProfileSectionW" => write_private_profile_section_w,
];
