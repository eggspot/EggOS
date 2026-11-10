//! The component store's format (D235): a 512-byte header, the index as text, then each
//! file's bytes at a sector-aligned offset. Everything in a store is untrusted input: the
//! header and every index line are checked against hard limits before anything is used, and
//! each file's SHA-256 is checked while it is copied.

use alloc::string::String;
use alloc::vec::Vec;
use eggos_crypto::sha2::Sha256;

/// The first bytes of a store.
pub const MAGIC: &[u8; 16] = b"EGGOS-COMPONENTS";
/// The store format this reader knows.
pub const FORMAT: u32 = 1;
/// The header's size, and the alignment of every file's bytes.
pub const SECTOR: u64 = 512;
/// The largest index a store may carry.
pub const MAX_INDEX_BYTES: u32 = 64 * 1024;
/// The most components one store offers.
pub const MAX_COMPONENTS: usize = 6;
/// The most files one component holds.
pub const MAX_FILES: usize = 64;
/// The largest file a component holds.
pub const MAX_FILE_BYTES: u64 = 256 << 20;
/// The longest path of a component's file, in bytes.
pub const MAX_PATH_BYTES: usize = 255;
/// The longest name of a component.
pub const MAX_NAME_BYTES: usize = 32;
/// The longest title, summary or remove note.
pub const MAX_TEXT_BYTES: usize = 96;
/// The only folders a component's files may be in: the base links into them.
pub const ALLOWED_PREFIXES: [&str; 2] = ["usr/lib/eggos/", "usr/lib/firmware/"];
/// The block size a store image is padded to, so the kernel can attach it as a disk.
pub const STORE_BLOCK: usize = 4096;
/// The first line of an index.
pub const INDEX_TAG: &str = "eggos-components";

const HEADER_FORMAT: usize = 16;
const HEADER_LENGTH: usize = 20;
const HEADER_DIGEST: usize = 24;
const DIGEST_BYTES: usize = 32;
const NEEDED_FIELDS: u8 = 0b111;

/// Why a store or an installed record cannot be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexError {
    /// The first bytes are not [`MAGIC`].
    NotAStore,
    /// The store is of a format this reader does not know.
    UnknownFormat,
    /// The index is empty or larger than [`MAX_INDEX_BYTES`].
    IndexSize,
    /// The index does not match the header's SHA-256.
    IndexDamaged,
    /// Line `n` (from 1) breaks the format.
    Line(usize),
    /// More than [`MAX_COMPONENTS`] components or [`MAX_FILES`] files.
    TooMany,
    /// A component is named twice, or a file's path twice.
    Duplicate,
    /// A component needs one that does not come before it.
    UnknownNeed,
    /// A file's bytes lie outside the store or before its data.
    OutsideStore,
    /// The index ends inside a component.
    Truncated,
}

/// The store's header, its first [`SECTOR`] bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// The index's length in bytes; it starts at [`SECTOR`].
    pub index_bytes: u32,
    /// The index's SHA-256.
    pub index_digest: [u8; DIGEST_BYTES],
}

impl Header {
    /// Reads a header from the store's first bytes.
    pub fn parse(bytes: &[u8]) -> Result<Self, IndexError> {
        if bytes.len() < SECTOR as usize || &bytes[..MAGIC.len()] != MAGIC {
            return Err(IndexError::NotAStore);
        }
        let word = |at: usize| {
            u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };
        if word(HEADER_FORMAT) != FORMAT {
            return Err(IndexError::UnknownFormat);
        }
        let index_bytes = word(HEADER_LENGTH);
        if index_bytes == 0 || index_bytes > MAX_INDEX_BYTES {
            return Err(IndexError::IndexSize);
        }
        let mut index_digest = [0u8; DIGEST_BYTES];
        index_digest.copy_from_slice(&bytes[HEADER_DIGEST..HEADER_DIGEST + DIGEST_BYTES]);
        Ok(Self {
            index_bytes,
            index_digest,
        })
    }

    /// The header of a store whose index is `index`.
    pub fn of(index: &[u8]) -> [u8; SECTOR as usize] {
        let mut bytes = [0u8; SECTOR as usize];
        bytes[..MAGIC.len()].copy_from_slice(MAGIC);
        bytes[HEADER_FORMAT..HEADER_FORMAT + 4].copy_from_slice(&FORMAT.to_le_bytes());
        bytes[HEADER_LENGTH..HEADER_LENGTH + 4]
            .copy_from_slice(&(index.len() as u32).to_le_bytes());
        bytes[HEADER_DIGEST..HEADER_DIGEST + DIGEST_BYTES].copy_from_slice(&sha256(index));
        bytes
    }

    /// Where the files' bytes may start: the first sector after the index.
    pub fn data_start(&self) -> u64 {
        (SECTOR + u64::from(self.index_bytes)).div_ceil(SECTOR) * SECTOR
    }
}

/// The SHA-256 of `bytes`.
pub fn sha256(bytes: &[u8]) -> [u8; DIGEST_BYTES] {
    let mut hash = Sha256::new();
    hash.update(bytes);
    hash.finish()
}

/// One component's record and its files' bytes, in the record's order, for [`store_image`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packed {
    /// The record; [`store_image`] sets each file's offset, length and SHA-256.
    pub component: Component,
    /// The files' bytes.
    pub contents: Vec<Vec<u8>>,
}

fn placed(packed: &[Packed], version: &str, arch: &str, data_start: u64) -> Index {
    let mut offset = data_start;
    let components = packed
        .iter()
        .map(|part| {
            let mut component = part.component.clone();
            for (file, bytes) in component.files.iter_mut().zip(&part.contents) {
                file.offset = offset;
                file.length = bytes.len() as u64;
                file.sha256 = sha256(bytes);
                offset += (bytes.len() as u64).div_ceil(SECTOR).max(1) * SECTOR;
            }
            component
        })
        .collect();
    Index {
        version: version.into(),
        arch: arch.into(),
        components,
    }
}

/// The store image of `version` for `arch` holding `packed`: the header, the index, then each
/// file's bytes at a sector-aligned offset, padded to [`STORE_BLOCK`].
pub fn store_image(packed: &[Packed], version: &str, arch: &str) -> Vec<u8> {
    let mut data_start = SECTOR * 2;
    let index = loop {
        let index = placed(packed, version, arch, data_start);
        let text = index.write();
        let needed = Header::parse(&Header::of(text.as_bytes()))
            .map(|header| header.data_start())
            .unwrap_or(data_start);
        if needed == data_start {
            break index;
        }
        data_start = needed;
    };
    let text = index.write();
    let mut image = Header::of(text.as_bytes()).to_vec();
    image.extend_from_slice(text.as_bytes());
    for (part, component) in packed.iter().zip(&index.components) {
        for (bytes, file) in part.contents.iter().zip(&component.files) {
            image.resize(file.offset as usize, 0);
            image.extend_from_slice(bytes);
        }
    }
    image.resize(image.len().next_multiple_of(STORE_BLOCK), 0);
    image
}

/// One file of a component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    /// Its path below the component's root and below `/`, relative, under an allowed prefix.
    pub path: String,
    /// `0o644` or `0o755`.
    pub mode: u32,
    /// Where its bytes start in the store.
    pub offset: u64,
    /// How many bytes it has.
    pub length: u64,
    /// Its bytes' SHA-256.
    pub sha256: [u8; DIGEST_BYTES],
}

/// One component, as the store's index or an installed record says it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component {
    /// Its name, `windows-apps`.
    pub name: String,
    /// Its title, `Windows apps`.
    pub title: String,
    /// One line about it.
    pub summary: String,
    /// The sentence the remove confirmation shows under its question.
    pub on_remove: String,
    /// The components it needs, each earlier in the index.
    pub needs: Vec<String>,
    /// Its files.
    pub files: Vec<File>,
}

impl Component {
    /// Its installed size: its files' bytes.
    pub fn size_bytes(&self) -> u64 {
        self.files.iter().map(|file| file.length).sum()
    }

    /// Whether it needs `name`.
    pub fn needs(&self, name: &str) -> bool {
        self.needs.iter().any(|need| need == name)
    }

    /// Writes it as a record block, as the index holds it and as an installed record keeps it.
    pub fn write_block(&self, out: &mut String) {
        use core::fmt::Write;
        let _ = writeln!(out, "component {}", self.name);
        let _ = writeln!(out, "title {}", self.title);
        let _ = writeln!(out, "summary {}", self.summary);
        let _ = writeln!(out, "on-remove {}", self.on_remove);
        for need in &self.needs {
            let _ = writeln!(out, "needs {need}");
        }
        for file in &self.files {
            let _ = write!(out, "file {:o} {} {} ", file.mode, file.offset, file.length);
            for byte in file.sha256 {
                let _ = write!(out, "{byte:02x}");
            }
            let _ = writeln!(out, " {}", file.path);
        }
        out.push_str("end\n");
    }
}

/// A store's index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Index {
    /// The EggOS version the store was made for.
    pub version: String,
    /// The architecture its programs run on.
    pub arch: String,
    /// Its components, each after those it needs.
    pub components: Vec<Component>,
}

impl Index {
    /// The component named `name`.
    pub fn component(&self, name: &str) -> Option<&Component> {
        self.components
            .iter()
            .find(|component| component.name == name)
    }

    /// Writes the index's text.
    pub fn write(&self) -> String {
        let mut out = String::new();
        out.push_str(INDEX_TAG);
        out.push_str(" 1 ");
        out.push_str(&self.version);
        out.push(' ');
        out.push_str(&self.arch);
        out.push('\n');
        for component in &self.components {
            component.write_block(&mut out);
        }
        out
    }
}

/// Where a file's bytes may lie: in a store, between its data start and its end; in a record,
/// anywhere, since a record's offsets are only what the store said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bounds {
    /// In a store of `length` bytes whose data starts at `data_start`.
    Store { data_start: u64, length: u64 },
    /// In an installed record.
    Record,
}

/// Reads and checks an index whose header is `header`, from `text`, for a store of
/// `store_bytes` bytes.
pub fn parse_index(header: &Header, text: &[u8], store_bytes: u64) -> Result<Index, IndexError> {
    if text.len() != header.index_bytes as usize {
        return Err(IndexError::IndexSize);
    }
    if sha256(text) != header.index_digest {
        return Err(IndexError::IndexDamaged);
    }
    let bounds = Bounds::Store {
        data_start: header.data_start(),
        length: store_bytes,
    };
    let text = core::str::from_utf8(text).map_err(|_| IndexError::Line(1))?;
    let mut lines = text.lines().enumerate().map(|(at, line)| (at + 1, line));
    let (number, first) = lines.next().ok_or(IndexError::Truncated)?;
    let mut words = first.split(' ');
    let (Some(INDEX_TAG), Some("1"), Some(version), Some(arch), None) = (
        words.next(),
        words.next(),
        words.next(),
        words.next(),
        words.next(),
    ) else {
        return Err(IndexError::Line(number));
    };
    if !is_version(version) || !is_arch(arch) {
        return Err(IndexError::Line(number));
    }
    let mut components: Vec<Component> = Vec::new();
    while let Some(component) = parse_block(&mut lines, bounds)? {
        if components.len() == MAX_COMPONENTS {
            return Err(IndexError::TooMany);
        }
        if components.iter().any(|known| known.name == component.name) {
            return Err(IndexError::Duplicate);
        }
        let known = |need: &String| components.iter().any(|earlier| &earlier.name == need);
        if !component.needs.iter().all(known) {
            return Err(IndexError::UnknownNeed);
        }
        components.push(component);
    }
    Ok(Index {
        version: version.into(),
        arch: arch.into(),
        components,
    })
}

/// Reads and checks one installed record: one component block and nothing else.
pub fn parse_record(text: &[u8]) -> Result<Component, IndexError> {
    if text.len() > MAX_INDEX_BYTES as usize {
        return Err(IndexError::IndexSize);
    }
    let text = core::str::from_utf8(text).map_err(|_| IndexError::Line(1))?;
    let mut lines = text.lines().enumerate().map(|(at, line)| (at + 1, line));
    let component = parse_block(&mut lines, Bounds::Record)?.ok_or(IndexError::Truncated)?;
    match lines.next() {
        Some((number, _)) => Err(IndexError::Line(number)),
        None => Ok(component),
    }
}

fn parse_block<'a>(
    lines: &mut impl Iterator<Item = (usize, &'a str)>,
    bounds: Bounds,
) -> Result<Option<Component>, IndexError> {
    let Some((number, line)) = lines.next() else {
        return Ok(None);
    };
    let name = match line.split_once(' ') {
        Some(("component", name)) if is_name(name) => name,
        _ => return Err(IndexError::Line(number)),
    };
    let mut component = Component {
        name: name.into(),
        title: String::new(),
        summary: String::new(),
        on_remove: String::new(),
        needs: Vec::new(),
        files: Vec::new(),
    };
    let mut seen = 0u8;
    for (number, line) in lines.by_ref() {
        if line == "end" {
            if seen != NEEDED_FIELDS || component.files.is_empty() {
                return Err(IndexError::Line(number));
            }
            return Ok(Some(component));
        }
        let (key, value) = line.split_once(' ').ok_or(IndexError::Line(number))?;
        let field = match key {
            "title" => Some((0, &mut component.title)),
            "summary" => Some((1, &mut component.summary)),
            "on-remove" => Some((2, &mut component.on_remove)),
            _ => None,
        };
        if let Some((bit, slot)) = field {
            if seen & (1 << bit) != 0 || !is_text(value) {
                return Err(IndexError::Line(number));
            }
            seen |= 1 << bit;
            slot.push_str(value);
            continue;
        }
        match key {
            "needs" if is_name(value) && value != component.name => {
                if component.needs.iter().any(|need| need == value) {
                    return Err(IndexError::Duplicate);
                }
                if component.needs.len() == MAX_COMPONENTS {
                    return Err(IndexError::TooMany);
                }
                component.needs.push(value.into());
            }
            "file" => {
                let file = parse_file(value, bounds).ok_or(IndexError::Line(number))?;
                if component.files.len() == MAX_FILES {
                    return Err(IndexError::TooMany);
                }
                if bounds != Bounds::Record && file.length > 0 {
                    check_inside(&file, bounds)?;
                }
                if component
                    .files
                    .iter()
                    .any(|known| paths_clash(&known.path, &file.path))
                {
                    return Err(IndexError::Duplicate);
                }
                component.files.push(file);
            }
            _ => return Err(IndexError::Line(number)),
        }
    }
    Err(IndexError::Truncated)
}

fn parse_file(value: &str, bounds: Bounds) -> Option<File> {
    let mut words = value.splitn(5, ' ');
    let mode = match words.next()? {
        "644" => 0o644,
        "755" => 0o755,
        _ => return None,
    };
    let offset = parse_number(words.next()?)?;
    let length = parse_number(words.next()?)?;
    let digest = parse_digest(words.next()?)?;
    let path = words.next()?;
    if length > MAX_FILE_BYTES || !is_component_path(path) {
        return None;
    }
    if matches!(bounds, Bounds::Store { .. }) && offset % SECTOR != 0 {
        return None;
    }
    Some(File {
        path: path.into(),
        mode,
        offset,
        length,
        sha256: digest,
    })
}

fn check_inside(file: &File, bounds: Bounds) -> Result<(), IndexError> {
    let Bounds::Store { data_start, length } = bounds else {
        return Ok(());
    };
    let end = file
        .offset
        .checked_add(file.length)
        .ok_or(IndexError::OutsideStore)?;
    match file.offset >= data_start && end <= length {
        true => Ok(()),
        false => Err(IndexError::OutsideStore),
    }
}

fn paths_clash(first: &str, second: &str) -> bool {
    let inside = |outer: &str, inner: &str| {
        inner.len() > outer.len()
            && inner.starts_with(outer)
            && inner.as_bytes()[outer.len()] == b'/'
    };
    first == second || inside(first, second) || inside(second, first)
}

fn parse_number(word: &str) -> Option<u64> {
    if word.is_empty() || word.len() > 20 || !word.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    if word.len() > 1 && word.starts_with('0') {
        return None;
    }
    word.parse().ok()
}

fn parse_digest(word: &str) -> Option<[u8; DIGEST_BYTES]> {
    let bytes = word.as_bytes();
    if bytes.len() != DIGEST_BYTES * 2 {
        return None;
    }
    let digit = |byte: u8| match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    };
    let mut digest = [0u8; DIGEST_BYTES];
    for (at, [high, low]) in bytes.as_chunks::<2>().0.iter().enumerate() {
        digest[at] = digit(*high)? << 4 | digit(*low)?;
    }
    Some(digest)
}

/// Whether `name` may name a component: 1 to [`MAX_NAME_BYTES`] lowercase letters, digits and
/// `-`, starting with a letter.
pub fn is_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_NAME_BYTES
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|&byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// Whether `path` may be a component's file: relative, under an allowed prefix, every part a
/// plain name of letters, digits, `.`, `_`, `+` and `-` that is neither `.` nor `..`.
pub fn is_component_path(path: &str) -> bool {
    path.len() <= MAX_PATH_BYTES
        && ALLOWED_PREFIXES
            .iter()
            .any(|prefix| path.len() > prefix.len() && path.starts_with(prefix))
        && path.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-')
                })
        })
}

fn is_text(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAX_TEXT_BYTES
        && !text.starts_with(' ')
        && !text.chars().any(char::is_control)
}

fn is_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= MAX_NAME_BYTES
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'+' | b'-'))
}

fn is_arch(arch: &str) -> bool {
    !arch.is_empty()
        && arch.len() <= 16
        && arch
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}
