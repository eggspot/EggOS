//! The program's resources (D178): the resource directory of its image (the PE format's
//! `.rsrc`, three levels: type, name and language, each with its named entries before its
//! numbered ones), found from the headers of the mapped image, and kernel32's
//! `FindResourceW`, `FindResourceExW`, `LoadResource`, `LockResource`, `SizeofResource` and
//! `EnumResourceNamesW`. A resource handle is the address of its data entry, as on Windows, and
//! a resource's data is the image's own bytes, never copied. The layer's own libraries have no
//! resources.

use super::dib::{u16_at, u32_at};
use super::process::process;
use super::{Bool, FALSE, TRUE, call_program, error, fail, heap};
use crate::library::Export;
use crate::text;

const DIRECTORY_HEADER: usize = 16;
const ENTRY_SIZE: usize = 8;
const DATA_ENTRY_SIZE: usize = 16;
const SUBDIRECTORY: u32 = 0x8000_0000;
const NAMED: u32 = 0x8000_0000;
const RESOURCE_DIRECTORY: usize = 2;
const HEADERS_READ: usize = 4096;
const PE_HEADER_OFFSET: usize = 0x3c;
const PE32_PLUS: u16 = 0x20b;
const IDS: usize = 0x1_0000;
const LANGUAGE_NEUTRAL: u16 = 0;
const LANGUAGE_USER_DEFAULT: u16 = 0x400;
const LANGUAGE_ENGLISH_US: u16 = 0x409;

/// The resource types (`RT_*`) the layer looks up.
pub(super) mod kind {
    pub const CURSOR: u16 = 1;
    pub const BITMAP: u16 = 2;
    pub const ICON: u16 = 3;
    pub const DIALOG: u16 = 5;
    pub const ACCELERATOR: u16 = 9;
    pub const GROUP_CURSOR: u16 = 12;
    pub const GROUP_ICON: u16 = 14;
}

/// A resource type's or name's identity: a number, or a name (compared ignoring case).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Name<'a> {
    Id(u16),
    Text(&'a [u16]),
}

fn decimal(digits: &[u16]) -> Option<u16> {
    if digits.is_empty() {
        return None;
    }
    digits.iter().try_fold(0u16, |value, &unit| {
        let digit = unit
            .checked_sub(u16::from(b'0'))
            .filter(|digit| *digit < 10)?;
        value.checked_mul(10)?.checked_add(digit)
    })
}

fn name_of(text: &[u16]) -> Name<'_> {
    match text.split_first() {
        Some((&hash, digits)) if hash == u16::from(b'#') => match decimal(digits) {
            Some(id) => Name::Id(id),
            None => Name::Text(text),
        },
        _ => Name::Text(text),
    }
}

/// The resource type or name a program passes: below `0x10000` a number (`MAKEINTRESOURCE`),
/// else a NUL-terminated UTF-16 name, where `#` and decimal digits is a number too.
///
/// # Safety
/// `pointer` must be below `0x10000` or point to a NUL-terminated UTF-16 string that lives as
/// long as the result is used.
pub(super) unsafe fn name_from(pointer: usize) -> Name<'static> {
    if pointer < IDS {
        return Name::Id(pointer as u16);
    }
    // SAFETY: the caller's duty: a NUL-terminated string.
    name_of(unsafe { super::file::wide_string(pointer as *const u16) })
}

fn entries(section: &[u8], directory: usize) -> Option<impl Iterator<Item = (u32, u32)> + '_> {
    let named = usize::from(u16_at(section, directory + 12)?);
    let numbered = usize::from(u16_at(section, directory + 14)?);
    let first = directory + DIRECTORY_HEADER;
    Some((0..named + numbered).map_while(move |index| {
        let at = first + index * ENTRY_SIZE;
        Some((u32_at(section, at)?, u32_at(section, at + 4)?))
    }))
}

fn entry_name(section: &[u8], raw: u32) -> Option<&[u8]> {
    let at = (raw & !NAMED) as usize;
    let length = usize::from(u16_at(section, at)?);
    section.get(at + 2..at + 2 + 2 * length)
}

fn matches(section: &[u8], raw: u32, name: Name<'_>) -> bool {
    match (raw & NAMED != 0, name) {
        (false, Name::Id(id)) => raw == u32::from(id),
        (true, Name::Text(wanted)) => entry_name(section, raw).is_some_and(|stored| {
            stored.len() == 2 * wanted.len()
                && stored
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .zip(wanted)
                    .all(|(unit, &other)| {
                        text::upper(u16::from_le_bytes(*unit)) == text::upper(other)
                    })
        }),
        _ => false,
    }
}

fn lookup(section: &[u8], directory: usize, name: Name<'_>) -> Option<u32> {
    entries(section, directory)?
        .find(|&(raw, _)| matches(section, raw, name))
        .map(|(_, target)| target)
}

fn subdirectory(target: u32) -> Option<usize> {
    (target & SUBDIRECTORY != 0).then_some((target & !SUBDIRECTORY) as usize)
}

fn find_in(
    section: &[u8],
    kind: Name<'_>,
    name: Name<'_>,
    language: Option<u16>,
) -> Result<usize, u32> {
    let names = lookup(section, 0, kind)
        .and_then(subdirectory)
        .ok_or(error::RESOURCE_TYPE_NOT_FOUND)?;
    let languages = lookup(section, names, name)
        .and_then(subdirectory)
        .ok_or(error::RESOURCE_NAME_NOT_FOUND)?;
    let chosen = match language {
        Some(language) => {
            lookup(section, languages, Name::Id(language)).ok_or(error::RESOURCE_LANG_NOT_FOUND)?
        }
        None => [LANGUAGE_NEUTRAL, LANGUAGE_USER_DEFAULT, LANGUAGE_ENGLISH_US]
            .into_iter()
            .find_map(|language| lookup(section, languages, Name::Id(language)))
            .or_else(|| {
                entries(section, languages)?
                    .next()
                    .map(|(_, target)| target)
            })
            .ok_or(error::RESOURCE_LANG_NOT_FOUND)?,
    };
    match chosen & SUBDIRECTORY {
        0 if section.len() >= chosen as usize + DATA_ENTRY_SIZE => Ok(chosen as usize),
        _ => Err(error::RESOURCE_DATA_NOT_FOUND),
    }
}

#[derive(Debug, Clone, Copy)]
struct Resources {
    base: usize,
    image_size: usize,
    section: &'static [u8],
}

impl Resources {
    fn program() -> Option<Resources> {
        let base = process().image_base as usize;
        // SAFETY: the loader maps the program's headers at its base, a whole page at least,
        // readable for the process's life.
        let headers = unsafe { core::slice::from_raw_parts(base as *const u8, HEADERS_READ) };
        let optional = u32_at(headers, PE_HEADER_OFFSET)? as usize + 24;
        if u16_at(headers, optional)? != PE32_PLUS
            || u32_at(headers, optional + 108)? as usize <= RESOURCE_DIRECTORY
        {
            return None;
        }
        let image_size = u32_at(headers, optional + 56)? as usize;
        let directory = optional + 112 + 8 * RESOURCE_DIRECTORY;
        let address = u32_at(headers, directory)? as usize;
        let size = u32_at(headers, directory + 4)? as usize;
        if size == 0 || address.checked_add(size)? > image_size {
            return None;
        }
        // SAFETY: the directory lies inside the image (checked against its size), which stays
        // mapped and readable for the process's life.
        let section = unsafe { core::slice::from_raw_parts((base + address) as *const u8, size) };
        Some(Resources {
            base,
            image_size,
            section,
        })
    }

    fn of(module: usize) -> Result<Resources, u32> {
        let program = process().image_base as usize;
        if module != 0 && module != program {
            return Err(error::RESOURCE_DATA_NOT_FOUND);
        }
        Resources::program().ok_or(error::RESOURCE_DATA_NOT_FOUND)
    }

    fn entry_offset(&self, handle: usize) -> Option<usize> {
        let start = self.section.as_ptr() as usize;
        let offset = handle.checked_sub(start)?;
        (offset + DATA_ENTRY_SIZE <= self.section.len()).then_some(offset)
    }

    fn data(&self, entry: usize) -> Option<(usize, u32)> {
        let address = u32_at(self.section, entry)? as usize;
        let size = u32_at(self.section, entry + 4)?;
        (address.checked_add(size as usize)? <= self.image_size)
            .then_some((self.base + address, size))
    }
}

/// The address and size of resource `kind`/`name` of `module` (0 or the image base: the
/// program), in the language Windows would pick; `None` when there is none.
pub(super) fn find(module: usize, kind: Name<'_>, name: Name<'_>) -> Option<(usize, u32)> {
    let resources = Resources::of(module).ok()?;
    let entry = find_in(resources.section, kind, name, None).ok()?;
    resources.data(entry)
}

fn find_handle(module: usize, kind: usize, name: usize, language: Option<u16>) -> usize {
    let resources = match Resources::of(module) {
        Ok(resources) => resources,
        Err(code) => return fail(code, 0),
    };
    // SAFETY: `FindResource`'s contract: an id or a NUL-terminated name.
    let kind = unsafe { name_from(kind) };
    // SAFETY: as above.
    let name = unsafe { name_from(name) };
    match find_in(resources.section, kind, name, language) {
        Ok(entry) => resources.section.as_ptr() as usize + entry,
        Err(code) => fail(code, 0),
    }
}

extern "efiapi" fn find_resource_w(module: usize, name: usize, kind: usize) -> usize {
    find_handle(module, kind, name, None)
}

extern "efiapi" fn find_resource_ex_w(
    module: usize,
    kind: usize,
    name: usize,
    language: u16,
) -> usize {
    let language = (language != LANGUAGE_NEUTRAL).then_some(language);
    find_handle(module, kind, name, language)
}

fn resource_data(handle: usize) -> Result<(usize, u32), u32> {
    let resources = Resources::program().ok_or(error::RESOURCE_DATA_NOT_FOUND)?;
    resources
        .entry_offset(handle)
        .and_then(|entry| resources.data(entry))
        .ok_or(error::INVALID_HANDLE)
}

extern "efiapi" fn load_resource(_module: usize, handle: usize) -> usize {
    match resource_data(handle) {
        Ok((address, _)) => address,
        Err(code) => fail(code, 0),
    }
}

extern "efiapi" fn lock_resource(data: usize) -> usize {
    data
}

extern "efiapi" fn sizeof_resource(_module: usize, handle: usize) -> u32 {
    match resource_data(handle) {
        Ok((_, size)) => size,
        Err(code) => fail(code, 0),
    }
}

fn call_with_name(callback: usize, arguments: [usize; 4], name: &[u8]) -> Option<usize> {
    let units = name.len() / 2;
    let copy = heap::allocate((units + 1) * 2).cast::<u16>();
    if copy.is_null() {
        return None;
    }
    for (index, unit) in name.as_chunks::<2>().0.iter().enumerate() {
        let slot = copy.wrapping_add(index);
        // SAFETY: `copy` holds `units + 1` units and `index < units`.
        unsafe { slot.write(u16::from_le_bytes(*unit)) };
    }
    let [module, kind, _, parameter] = arguments;
    // SAFETY: `EnumResourceNamesW`'s contract: `callback` is the program's
    // `EnumResNameProcW`, which takes these four arguments; the name is NUL-terminated (the
    // block is zeroed) and lives through the call.
    let result = unsafe { call_program(callback, [module, kind, copy as usize, parameter]) };
    // SAFETY: the block this function allocated, which the program no longer uses.
    unsafe { heap::free(copy.cast()) };
    Some(result)
}

extern "efiapi" fn enum_resource_names_w(
    module: usize,
    kind: usize,
    callback: usize,
    parameter: usize,
) -> Bool {
    let resources = match Resources::of(module) {
        Ok(resources) => resources,
        Err(code) => return fail(code, FALSE),
    };
    let section = resources.section;
    // SAFETY: `EnumResourceNamesW`'s contract: an id or a NUL-terminated type name.
    let wanted = unsafe { name_from(kind) };
    let names = lookup(section, 0, wanted).and_then(subdirectory);
    let Some(list) = names.and_then(|names| entries(section, names)) else {
        return fail(error::RESOURCE_TYPE_NOT_FOUND, FALSE);
    };
    let arguments = [resources.base, kind, 0, parameter];
    for (raw, _) in list {
        let result = match raw & NAMED {
            0 => {
                let [module, kind, _, parameter] = arguments;
                // SAFETY: as in `call_with_name`, with a numbered name.
                Some(unsafe { call_program(callback, [module, kind, raw as usize, parameter]) })
            }
            _ => {
                entry_name(section, raw).and_then(|name| call_with_name(callback, arguments, name))
            }
        };
        match result {
            None => return fail(error::NOT_ENOUGH_MEMORY, FALSE),
            Some(going_on) if going_on as u32 == 0 => {
                return fail(error::RESOURCE_ENUM_USER_STOP, FALSE);
            }
            Some(_) => {}
        }
    }
    TRUE
}

pub(super) const EXPORTS: &[Export] = exports![
    b"FindResourceW" => find_resource_w,
    b"FindResourceExW" => find_resource_ex_w,
    b"LoadResource" => load_resource,
    b"LockResource" => lock_resource,
    b"SizeofResource" => sizeof_resource,
    b"EnumResourceNamesW" => enum_resource_names_w,
];

#[cfg(test)]
mod tests {
    use super::*;

    fn directory(named: u16, numbered: u16) -> Vec<u8> {
        let mut bytes = vec![0u8; 12];
        bytes.extend_from_slice(&named.to_le_bytes());
        bytes.extend_from_slice(&numbered.to_le_bytes());
        bytes
    }

    fn entry(bytes: &mut Vec<u8>, name: u32, target: u32) {
        bytes.extend_from_slice(&name.to_le_bytes());
        bytes.extend_from_slice(&target.to_le_bytes());
    }

    fn section() -> Vec<u8> {
        let mut bytes = directory(0, 1);
        entry(&mut bytes, u32::from(kind::GROUP_ICON), SUBDIRECTORY | 0x18);
        bytes.extend(directory(1, 1));
        entry(&mut bytes, NAMED | 0x80, SUBDIRECTORY | 0x38);
        entry(&mut bytes, 7, SUBDIRECTORY | 0x50);
        bytes.extend(directory(0, 1));
        entry(&mut bytes, 0x409, 0x90);
        bytes.extend(directory(0, 2));
        entry(&mut bytes, 0x407, 0xa0);
        entry(&mut bytes, 0x409, 0xb0);
        bytes.resize(0x80, 0);
        bytes.extend_from_slice(&3u16.to_le_bytes());
        for unit in "APP".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes.resize(0x90, 0);
        for (address, size) in [(0x1000u32, 20u32), (0x2000, 30), (0x3000, 40)] {
            bytes.extend_from_slice(&address.to_le_bytes());
            bytes.extend_from_slice(&size.to_le_bytes());
            bytes.extend_from_slice(&[0; 8]);
        }
        bytes
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    #[test]
    fn the_directory_is_walked_by_type_name_and_language() {
        let section = section();
        let group = Name::Id(kind::GROUP_ICON);
        let app = wide("app");
        assert_eq!(find_in(&section, group, Name::Text(&app), None), Ok(0x90));
        assert_eq!(find_in(&section, group, Name::Id(7), None), Ok(0xb0));
        assert_eq!(find_in(&section, group, Name::Id(7), Some(0x407)), Ok(0xa0));
        assert_eq!(
            find_in(&section, group, Name::Id(7), Some(0x40c)),
            Err(error::RESOURCE_LANG_NOT_FOUND)
        );
        assert_eq!(
            find_in(&section, group, Name::Id(8), None),
            Err(error::RESOURCE_NAME_NOT_FOUND)
        );
        assert_eq!(
            find_in(&section, Name::Id(kind::ICON), Name::Id(7), None),
            Err(error::RESOURCE_TYPE_NOT_FOUND)
        );
        let other = wide("APPS");
        assert!(find_in(&section, group, Name::Text(&other), None).is_err());
        assert_eq!(u32_at(&section, 0xb0 + 4), Some(40));
        let names: Vec<u32> = entries(&section, 0x18)
            .unwrap()
            .map(|(raw, _)| raw)
            .collect();
        assert_eq!(names, [NAMED | 0x80, 7]);
        assert_eq!(entry_name(&section, NAMED | 0x80).map(<[u8]>::len), Some(6));
    }

    #[test]
    fn numbered_names_are_read_from_hash_and_digits() {
        let numbered = wide("#101");
        assert_eq!(name_of(&numbered), Name::Id(101));
        let named = wide("#1x");
        assert_eq!(name_of(&named), Name::Text(&named));
        let bare = wide("#");
        assert_eq!(name_of(&bare), Name::Text(&bare));
        assert_eq!(decimal(&wide("70000")), None);
    }
}
