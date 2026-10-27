//! Laying a checked [`Pe`] out in memory as Windows does: the headers and each section at its
//! relative virtual address, base relocations applied for the address it was mapped at, and
//! each import's address written into the import address table. Works on the image's bytes
//! alone, so every step is tested on the host; every table read from the image is checked
//! against it.

use crate::pe::{self, InvalidPe, Pe, directory, read_u16, read_u32, read_u64};

const RELOCATION_ABSOLUTE: u16 = 0;
const RELOCATION_HIGHLOW: u16 = 3;
const RELOCATION_DIR64: u16 = 10;
const IMPORT_DESCRIPTOR_SIZE: usize = 20;
const IMPORT_BY_ORDINAL: u64 = 1 << 63;
const MAX_NAME: usize = 256;
const NAME_CAPACITY: usize = 56;
const TLS_DIRECTORY_SIZE: usize = 40;

/// Copies the headers and every section's file bytes from `pe.file` into `image`, which must be
/// [`Pe::mapped_size`] bytes of zeros.
pub fn copy_sections(pe: &Pe<'_>, image: &mut [u8]) {
    let headers = pe.headers_size as usize;
    image[..headers].copy_from_slice(&pe.file[..headers]);
    for section in pe.sections() {
        let start = section.virtual_address as usize;
        let length = section.file_bytes() as usize;
        let raw = section.raw_offset as usize;
        image[start..start + length].copy_from_slice(&pe.file[raw..raw + length]);
    }
}

/// Applies the base relocations for an image mapped at `base` instead of its preferred base.
pub fn relocate(pe: &Pe<'_>, image: &mut [u8], base: u64) -> Result<(), InvalidPe> {
    let delta = base.wrapping_sub(pe.image_base);
    if delta == 0 {
        return Ok(());
    }
    if !pe.relocatable() {
        return Err(InvalidPe(
            "the program has no relocations and its preferred address is taken",
        ));
    }
    let table = pe.directory(directory::BASE_RELOCATION);
    let mut block = table.address as usize;
    let end = block + table.size as usize;
    while block + 8 <= end {
        let page = read_u32(image, block)? as usize;
        let size = read_u32(image, block + 4)? as usize;
        if size < 8 || !size.is_multiple_of(2) || block + size > end {
            return Err(InvalidPe("a relocation block has a bad size"));
        }
        for entry in (block + 8..block + size).step_by(2) {
            let entry = read_u16(image, entry)?;
            let target = page + usize::from(entry & 0xfff);
            match entry >> 12 {
                RELOCATION_ABSOLUTE => {}
                RELOCATION_DIR64 => {
                    let value = read_u64(image, target)
                        .map_err(|_| InvalidPe("a relocation is outside the image"))?;
                    image[target..target + 8]
                        .copy_from_slice(&value.wrapping_add(delta).to_le_bytes());
                }
                RELOCATION_HIGHLOW => {
                    let value = read_u32(image, target)
                        .map_err(|_| InvalidPe("a relocation is outside the image"))?;
                    image[target..target + 4]
                        .copy_from_slice(&value.wrapping_add(delta as u32).to_le_bytes());
                }
                _ => return Err(InvalidPe("a relocation type other than DIR64 or HIGHLOW")),
            }
        }
        block += size;
    }
    Ok(())
}

/// What an import names: a function by name, or by ordinal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Import<'a> {
    Name(&'a [u8]),
    Ordinal(u16),
}

/// A short copy of a name from the image, for messages.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Name {
    bytes: [u8; NAME_CAPACITY],
    length: u8,
}

impl Name {
    pub fn new(name: &[u8]) -> Self {
        let mut bytes = [0; NAME_CAPACITY];
        let length = name.len().min(NAME_CAPACITY);
        bytes[..length].copy_from_slice(&name[..length]);
        Self {
            bytes,
            length: length as u8,
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.length)]
    }
}

impl core::fmt::Debug for Name {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:?}", core::str::from_utf8(self.as_bytes()))
    }
}

/// Why imports could not be bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindError {
    Invalid(InvalidPe),
    Unresolved { library: Name, function: Name },
    UnresolvedOrdinal { library: Name, ordinal: u16 },
}

impl From<InvalidPe> for BindError {
    fn from(error: InvalidPe) -> Self {
        Self::Invalid(error)
    }
}

/// Writes the address `resolve` gives each import into the import address table; the first
/// import it has none for is the error.
pub fn bind_imports(
    pe: &Pe<'_>,
    image: &mut [u8],
    mut resolve: impl FnMut(&[u8], Import<'_>) -> Option<u64>,
) -> Result<usize, BindError> {
    let table = pe.directory(directory::IMPORT);
    if table.size == 0 {
        return Ok(0);
    }
    let mut bound = 0;
    let mut descriptor = table.address as usize;
    loop {
        let lookup = read_u32(image, descriptor)? as usize;
        let name = read_u32(image, descriptor + 12)? as usize;
        let address_table = read_u32(image, descriptor + 16)? as usize;
        if lookup == 0 && name == 0 && address_table == 0 {
            return Ok(bound);
        }
        let library = Name::new(c_string(image, name)?);
        let lookup = match lookup {
            0 => address_table,
            lookup => lookup,
        };
        if address_table == 0 {
            return Err(InvalidPe("an import has no address table").into());
        }
        for index in 0.. {
            let entry = read_u64(image, lookup + index * 8)?;
            if entry == 0 {
                break;
            }
            let slot = address_table + index * 8;
            if slot + 8 > image.len() {
                return Err(InvalidPe("an import address table is outside the image").into());
            }
            let address = if entry & IMPORT_BY_ORDINAL != 0 {
                let ordinal = entry as u16;
                resolve(library.as_bytes(), Import::Ordinal(ordinal))
                    .ok_or(BindError::UnresolvedOrdinal { library, ordinal })?
            } else {
                let hint_name = (entry & 0x7fff_ffff) as usize;
                let function = c_string(image, hint_name.saturating_add(2))?;
                resolve(library.as_bytes(), Import::Name(function)).ok_or(
                    BindError::Unresolved {
                        library,
                        function: Name::new(function),
                    },
                )?
            };
            image[slot..slot + 8].copy_from_slice(&address.to_le_bytes());
            bound += 1;
        }
        descriptor += IMPORT_DESCRIPTOR_SIZE;
    }
}

/// The image's thread-local storage template (`IMAGE_TLS_DIRECTORY64`), with its addresses as
/// offsets into the image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadLocalStorage {
    pub template_start: usize,
    pub template_end: usize,
    pub zero_fill: usize,
    pub index: usize,
    pub callbacks: Option<usize>,
}

/// Reads the TLS directory of an image mapped at `base` (after [`relocate`]), if it has one.
pub fn thread_local_storage(
    pe: &Pe<'_>,
    image: &[u8],
    base: u64,
) -> Result<Option<ThreadLocalStorage>, InvalidPe> {
    let table = pe.directory(directory::TLS);
    if table.size == 0 {
        return Ok(None);
    }
    if (table.size as usize) < TLS_DIRECTORY_SIZE {
        return Err(InvalidPe("the TLS directory is too short"));
    }
    let at = table.address as usize;
    let offset = |address: u64| {
        address
            .checked_sub(base)
            .filter(|offset| *offset < image.len() as u64)
            .map(|offset| offset as usize)
            .ok_or(InvalidPe("a TLS address is outside the image"))
    };
    let template_start = offset(read_u64(image, at)?)?;
    let template_end = match read_u64(image, at + 8)? {
        end if end == base + image.len() as u64 => image.len(),
        end => offset(end)?,
    };
    let index = offset(read_u64(image, at + 16)?)?;
    let callbacks = match read_u64(image, at + 24)? {
        0 => None,
        address => Some(offset(address)?),
    };
    let zero_fill = read_u32(image, at + 32)? as usize;
    if template_end < template_start || index + 4 > image.len() {
        return Err(InvalidPe("the TLS template or index is outside the image"));
    }
    if zero_fill > pe::MAX_IMAGE_SIZE as usize {
        return Err(InvalidPe("the TLS zero fill is larger than 1 GiB"));
    }
    Ok(Some(ThreadLocalStorage {
        template_start,
        template_end,
        zero_fill,
        index,
        callbacks,
    }))
}

/// The protection a section's pages get once the image is laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Protection {
    pub read: bool,
    pub write: bool,
    pub execute: bool,
}

/// Each section's page range in the image and the protection its flags ask for.
pub fn section_protections<'a>(
    pe: &'a Pe<'a>,
) -> impl Iterator<Item = (usize, usize, Protection)> + 'a {
    pe.sections().map(|section| {
        let start = section.virtual_address as usize;
        let end = (start + section.memory_size() as usize).next_multiple_of(pe::PAGE_SIZE as usize);
        let flags = section.flags;
        let protection = Protection {
            read: flags & pe::section_flags::MEMORY_READ != 0,
            write: flags & pe::section_flags::MEMORY_WRITE != 0,
            execute: flags & pe::section_flags::MEMORY_EXECUTE != 0,
        };
        (start, end, protection)
    })
}

fn c_string(image: &[u8], at: usize) -> Result<&[u8], InvalidPe> {
    let rest = image
        .get(at..)
        .ok_or(InvalidPe("an import name is outside the image"))?;
    let rest = &rest[..rest.len().min(MAX_NAME)];
    let length = rest
        .iter()
        .position(|byte| *byte == 0)
        .ok_or(InvalidPe("an import name is not terminated"))?;
    Ok(&rest[..length])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pe::tests::{Builder, DATA, IMAGE_SIZE, RDATA, TEST_IMAGE_BASE, TEXT, put};

    fn laid_out(builder: &Builder) -> (Pe<'_>, Vec<u8>) {
        let pe = Pe::parse(&builder.file).unwrap();
        let mut image = vec![0; pe.mapped_size() as usize];
        copy_sections(&pe, &mut image);
        (pe, image)
    }

    #[test]
    fn sections_are_copied_to_their_addresses_with_zeros_after_their_file_bytes() {
        let builder = Builder::new().section_data(2, 0x10, b"data!");
        let (_, image) = laid_out(&builder);
        assert_eq!(&image[..2], b"MZ");
        assert_eq!(image[TEXT as usize], 0xc3);
        assert_eq!(image[RDATA as usize], 0xc2);
        assert_eq!(&image[DATA as usize + 0x10..DATA as usize + 0x15], b"data!");
        assert!(image[DATA as usize + 0x100..].iter().all(|byte| *byte == 0));
        assert_eq!(image.len(), IMAGE_SIZE as usize);
    }

    fn relocation_block(page: u32, entries: &[u16]) -> Vec<u8> {
        let mut block = page.to_le_bytes().to_vec();
        block.extend((8 + entries.len() as u32 * 2).to_le_bytes());
        for entry in entries {
            block.extend(entry.to_le_bytes());
        }
        block
    }

    #[test]
    fn dir64_and_highlow_relocations_move_with_the_base_and_absolute_ones_are_skipped() {
        let block = relocation_block(DATA, &[0xa000, 0x3008, 0x0000, 0xa010]);
        let builder = Builder::new()
            .directory(directory::BASE_RELOCATION, RDATA, block.len() as u32)
            .section_data(1, 0, &block)
            .section_data(2, 0, &(TEST_IMAGE_BASE + 0x1234).to_le_bytes())
            .section_data(2, 8, &0x4000_1000u32.to_le_bytes())
            .section_data(2, 0x10, &(TEST_IMAGE_BASE + 8).to_le_bytes());
        let (pe, mut image) = laid_out(&builder);
        let unchanged = image.clone();
        relocate(&pe, &mut image, TEST_IMAGE_BASE).unwrap();
        assert_eq!(image, unchanged);
        let base = 0x7000_0000_0000;
        relocate(&pe, &mut image, base).unwrap();
        let data = DATA as usize;
        assert_eq!(read_u64(&image, data).unwrap(), base + 0x1234);
        let delta = base.wrapping_sub(TEST_IMAGE_BASE) as u32;
        assert_eq!(
            read_u32(&image, data + 8).unwrap(),
            0x4000_1000u32.wrapping_add(delta)
        );
        assert_eq!(read_u64(&image, data + 0x10).unwrap(), base + 8);
    }

    #[test]
    fn bad_relocations_are_refused() {
        let attempt = |block: Vec<u8>, size: u32| {
            let builder = Builder::new()
                .directory(directory::BASE_RELOCATION, RDATA, size)
                .section_data(1, 0, &block);
            let (pe, mut image) = laid_out(&builder);
            relocate(&pe, &mut image, 0x10_0000).unwrap_err().0
        };
        let past_image = relocation_block(IMAGE_SIZE - 4, &[0xaffc]);
        assert!(attempt(past_image, 10).contains("outside the image"));
        let unknown_type = relocation_block(DATA, &[0x5000]);
        assert!(attempt(unknown_type, 10).contains("relocation type"));
        let mut bad_size = relocation_block(DATA, &[0xa000]);
        bad_size[4] = 3;
        assert!(attempt(bad_size, 10).contains("bad size"));
        let mut too_long = relocation_block(DATA, &[0xa000]);
        too_long[4] = 0xff;
        assert!(attempt(too_long, 10).contains("bad size"));
        let builder = Builder::new();
        let (pe, mut image) = laid_out(&builder);
        assert!(
            relocate(&pe, &mut image, 0x10_0000)
                .unwrap_err()
                .0
                .contains("no relocations")
        );
    }

    fn import_image(entries: &[u64], library: &[u8]) -> Builder {
        let descriptors = RDATA;
        let lookup = RDATA + 0x40;
        let names = RDATA + 0x80;
        let address_table = DATA;
        let mut descriptor = vec![0u8; 40];
        put(&mut descriptor, 0, &lookup.to_le_bytes());
        put(&mut descriptor, 12, &names.to_le_bytes());
        put(&mut descriptor, 16, &address_table.to_le_bytes());
        let lookup_bytes: Vec<u8> = entries.iter().flat_map(|e| e.to_le_bytes()).collect();
        let mut name_bytes = library.to_vec();
        name_bytes.push(0);
        Builder::new()
            .directory(directory::IMPORT, descriptors, 40)
            .section_data(1, 0, &descriptor)
            .section_data(1, 0x40, &lookup_bytes)
            .section_data(1, 0x80, &name_bytes)
            .section_data(1, 0x90, b"\x00\x00GetStdHandle\0")
            .section_data(1, 0xa0, b"\x05\x00WriteFile\0")
    }

    #[test]
    fn imports_are_bound_by_name_into_the_address_table() {
        let builder = import_image(
            &[u64::from(RDATA + 0x90), u64::from(RDATA + 0xa0), 0],
            b"KERNEL32.dll",
        );
        let (pe, mut image) = laid_out(&builder);
        let mut seen = Vec::new();
        let bound = bind_imports(&pe, &mut image, |library, import| {
            seen.push((library.to_vec(), import.clone_name()));
            Some(0x1000 + seen.len() as u64)
        })
        .unwrap();
        assert_eq!(bound, 2);
        assert_eq!(
            seen,
            [
                (b"KERNEL32.dll".to_vec(), b"GetStdHandle".to_vec()),
                (b"KERNEL32.dll".to_vec(), b"WriteFile".to_vec()),
            ]
        );
        assert_eq!(read_u64(&image, DATA as usize).unwrap(), 0x1001);
        assert_eq!(read_u64(&image, DATA as usize + 8).unwrap(), 0x1002);
    }

    impl Import<'_> {
        fn clone_name(&self) -> Vec<u8> {
            match self {
                Import::Name(name) => name.to_vec(),
                Import::Ordinal(ordinal) => ordinal.to_string().into_bytes(),
            }
        }
    }

    #[test]
    fn an_import_without_an_address_is_reported_by_library_and_name() {
        let builder = import_image(&[u64::from(RDATA + 0xa0), 0], b"kernel32.dll");
        let (pe, mut image) = laid_out(&builder);
        let error = bind_imports(&pe, &mut image, |_, _| None).unwrap_err();
        assert_eq!(
            error,
            BindError::Unresolved {
                library: Name::new(b"kernel32.dll"),
                function: Name::new(b"WriteFile"),
            }
        );
        let builder = import_image(&[1 << 63 | 17, 0], b"ws2_32.dll");
        let (pe, mut image) = laid_out(&builder);
        let error = bind_imports(&pe, &mut image, |_, _| None).unwrap_err();
        assert_eq!(
            error,
            BindError::UnresolvedOrdinal {
                library: Name::new(b"ws2_32.dll"),
                ordinal: 17,
            }
        );
    }

    #[test]
    fn malformed_import_tables_are_refused_without_panicking() {
        let outside = import_image(&[0x7fff_fff0, 0], b"kernel32.dll");
        let (pe, mut image) = laid_out(&outside);
        assert!(matches!(
            bind_imports(&pe, &mut image, |_, _| Some(1)),
            Err(BindError::Invalid(_))
        ));
        assert!(c_string(&[b'A'; 300], 0).is_err());
        assert!(c_string(b"abc", 0).is_err());
        assert!(c_string(b"ab\0", 5).is_err());
        assert_eq!(c_string(b"ab\0", 1), Ok(&b"b"[..]));
        let builder = import_image(&[u64::from(RDATA + 0x90), 0], b"kernel32.dll");
        for at in 0..40 {
            for value in [0x00, 0x30, 0xff] {
                let mut file = builder.file.clone();
                file[0x600 + at] = value;
                let Ok(pe) = Pe::parse(&file) else { continue };
                let mut image = vec![0; pe.mapped_size() as usize];
                copy_sections(&pe, &mut image);
                let _ = bind_imports(&pe, &mut image, |_, _| Some(1));
            }
        }
    }

    #[test]
    fn the_tls_directory_is_read_as_offsets_into_the_image() {
        let mut directory = Vec::new();
        for value in [
            TEST_IMAGE_BASE + u64::from(DATA),
            TEST_IMAGE_BASE + u64::from(DATA) + 0x20,
            TEST_IMAGE_BASE + u64::from(DATA) + 0x40,
            TEST_IMAGE_BASE + u64::from(RDATA) + 0x100,
        ] {
            directory.extend(value.to_le_bytes());
        }
        directory.extend(0x10u32.to_le_bytes());
        directory.extend(0u32.to_le_bytes());
        let builder = Builder::new()
            .directory(directory::TLS, RDATA, 40)
            .section_data(1, 0, &directory);
        let (pe, image) = laid_out(&builder);
        let tls = thread_local_storage(&pe, &image, TEST_IMAGE_BASE)
            .unwrap()
            .unwrap();
        assert_eq!(
            tls,
            ThreadLocalStorage {
                template_start: DATA as usize,
                template_end: DATA as usize + 0x20,
                zero_fill: 0x10,
                index: DATA as usize + 0x40,
                callbacks: Some(RDATA as usize + 0x100),
            }
        );
        assert!(thread_local_storage(&pe, &image, 0x10_0000).is_err());
        let short = Builder::new().directory(directory::TLS, RDATA, 8);
        let (pe, image) = laid_out(&short);
        assert!(thread_local_storage(&pe, &image, TEST_IMAGE_BASE).is_err());
    }

    #[test]
    fn sections_are_protected_as_their_flags_say_in_whole_pages() {
        let builder = Builder::new();
        let pe = Pe::parse(&builder.file).unwrap();
        let protections: Vec<_> = section_protections(&pe).collect();
        let rx = Protection {
            read: true,
            write: false,
            execute: true,
        };
        let r = Protection {
            read: true,
            write: false,
            execute: false,
        };
        let rw = Protection {
            read: true,
            write: true,
            execute: false,
        };
        assert_eq!(
            protections,
            [
                (0x1000, 0x2000, rx),
                (0x2000, 0x3000, r),
                (0x3000, 0x4000, rw)
            ]
        );
    }
}
