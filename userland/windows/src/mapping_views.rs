//! The rules of file mappings and their views (D189): which `PAGE_*` protections a mapping
//! takes, how big it is, and how `MapViewOfFile`'s `FILE_MAP_*` access becomes the protection
//! and sharing of the memory the kernel maps, refused where the mapping's protection forbids it.

use crate::errors::code as error;

/// The `PAGE_*` protections `CreateFileMappingW` takes.
pub mod page {
    pub const READONLY: u32 = 0x02;
    pub const READWRITE: u32 = 0x04;
    pub const WRITECOPY: u32 = 0x08;
    pub const EXECUTE_READ: u32 = 0x20;
    pub const EXECUTE_READWRITE: u32 = 0x40;
    pub const EXECUTE_WRITECOPY: u32 = 0x80;
}

/// The `FILE_MAP_*` access rights of a view.
pub mod access {
    pub const COPY: u32 = 0x01;
    pub const WRITE: u32 = 0x02;
    #[cfg(test)]
    pub const READ: u32 = 0x04;
    pub const EXECUTE: u32 = 0x20;
}

/// `SEC_IMAGE`: a mapping of an executable image, laid out as its loader would.
pub const SECTION_IMAGE: u32 = 0x0100_0000;
/// The alignment of a view's offset in its mapping.
pub const ALLOCATION_GRANULARITY: u64 = 0x1_0000;

const PROTECTION_READ: usize = 1;
const PROTECTION_WRITE: usize = 2;
const PROTECTION_EXECUTE: usize = 4;

/// What a mapping's `PAGE_*` protection lets its views do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Protection {
    pub writable: bool,
    pub copy_on_write: bool,
    pub executable: bool,
}

/// The protection `CreateFileMappingW`'s `flProtect` asks for (its `SEC_*` bits aside), or
/// `ERROR_INVALID_PARAMETER`; `SEC_IMAGE` is `ERROR_NOT_SUPPORTED`.
pub fn protection(flags: u32) -> Result<Protection, u32> {
    if flags & SECTION_IMAGE != 0 {
        return Err(error::NOT_SUPPORTED);
    }
    let (writable, copy_on_write, executable) = match flags & 0xff {
        page::READONLY => (false, false, false),
        page::READWRITE => (true, false, false),
        page::WRITECOPY => (false, true, false),
        page::EXECUTE_READ => (false, false, true),
        page::EXECUTE_READWRITE => (true, false, true),
        page::EXECUTE_WRITECOPY => (false, true, true),
        _ => return Err(error::INVALID_PARAMETER),
    };
    Ok(Protection {
        writable,
        copy_on_write,
        executable,
    })
}

/// The size of a new mapping that asks for `requested` bytes (0: the whole file) of a file of
/// `file_size` bytes (`None` for one backed by the page file), and whether the file must grow to
/// it: an empty mapping is `ERROR_FILE_INVALID` for a file and `ERROR_INVALID_PARAMETER` for
/// the page file, and only a writable mapping may be larger than its file.
pub fn mapping_size(
    requested: u64,
    file_size: Option<u64>,
    protection: Protection,
) -> Result<(u64, bool), u32> {
    match file_size {
        None if requested == 0 => Err(error::INVALID_PARAMETER),
        None => Ok((requested, false)),
        Some(0) if requested == 0 => Err(error::FILE_INVALID),
        Some(file) if requested == 0 => Ok((file, false)),
        Some(file) if requested <= file => Ok((requested, false)),
        Some(_) if protection.writable => Ok((requested, true)),
        Some(_) => Err(error::NOT_ENOUGH_MEMORY),
    }
}

/// How the kernel maps a view: its `PROT_*` bits and whether writes reach the mapping
/// (`MAP_SHARED`) or stay the view's own (`MAP_PRIVATE`, for `FILE_MAP_COPY`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewMapping {
    pub protection: usize,
    pub shared: bool,
}

/// The mapping of a view with `FILE_MAP_*` access `desired` of a mapping with `protection`:
/// `ERROR_ACCESS_DENIED` when it asks to write or run what the mapping does not allow.
/// `FILE_MAP_WRITE` wins over `FILE_MAP_COPY` (`FILE_MAP_ALL_ACCESS` holds both).
pub fn view_mapping(protection: Protection, desired: u32) -> Result<ViewMapping, u32> {
    let write = desired & access::WRITE != 0;
    let copy = !write && desired & access::COPY != 0;
    let execute = desired & access::EXECUTE != 0;
    let refused = (write && !protection.writable)
        || (copy && !(protection.writable || protection.copy_on_write))
        || (execute && !protection.executable);
    if refused {
        return Err(error::ACCESS_DENIED);
    }
    let mut bits = PROTECTION_READ;
    if write || copy {
        bits |= PROTECTION_WRITE;
    }
    if execute {
        bits |= PROTECTION_EXECUTE;
    }
    Ok(ViewMapping {
        protection: bits,
        shared: !copy,
    })
}

/// The length of a view of `bytes` (0: to the mapping's end) at `offset` in a mapping of `size`
/// bytes: `ERROR_MAPPED_ALIGNMENT` for an offset off the allocation granularity, and
/// `ERROR_ACCESS_DENIED` for a view past the mapping's end.
pub fn view_length(size: u64, offset: u64, bytes: u64) -> Result<u64, u32> {
    if !offset.is_multiple_of(ALLOCATION_GRANULARITY) {
        return Err(error::MAPPED_ALIGNMENT);
    }
    match bytes {
        0 if offset < size => Ok(size - offset),
        0 => Err(error::ACCESS_DENIED),
        _ if offset.checked_add(bytes).is_some_and(|end| end <= size) => Ok(bytes),
        _ => Err(error::ACCESS_DENIED),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const READ_WRITE: Protection = Protection {
        writable: true,
        copy_on_write: false,
        executable: false,
    };

    #[test]
    fn protections_are_read_from_the_low_byte() {
        assert_eq!(protection(page::READWRITE | 0x0800_0000), Ok(READ_WRITE));
        let copy = protection(page::WRITECOPY).unwrap();
        assert!(copy.copy_on_write && !copy.writable);
        assert!(protection(page::EXECUTE_READ).unwrap().executable);
        assert_eq!(protection(0x01), Err(error::INVALID_PARAMETER));
        assert_eq!(
            protection(page::READONLY | SECTION_IMAGE),
            Err(error::NOT_SUPPORTED)
        );
    }

    #[test]
    fn mappings_take_the_file_size_or_grow_writable_files() {
        let read_only = protection(page::READONLY).unwrap();
        assert_eq!(
            mapping_size(0, None, READ_WRITE),
            Err(error::INVALID_PARAMETER)
        );
        assert_eq!(mapping_size(8192, None, READ_WRITE), Ok((8192, false)));
        assert_eq!(
            mapping_size(0, Some(0), READ_WRITE),
            Err(error::FILE_INVALID)
        );
        assert_eq!(mapping_size(0, Some(5), read_only), Ok((5, false)));
        assert_eq!(mapping_size(3, Some(5), read_only), Ok((3, false)));
        assert_eq!(mapping_size(9, Some(5), READ_WRITE), Ok((9, true)));
        assert_eq!(
            mapping_size(9, Some(5), read_only),
            Err(error::NOT_ENOUGH_MEMORY)
        );
    }

    #[test]
    fn views_get_what_their_mapping_allows() {
        let read_only = protection(page::READONLY).unwrap();
        let write = view_mapping(READ_WRITE, access::WRITE).unwrap();
        assert_eq!((write.protection, write.shared), (3, true));
        let all = view_mapping(READ_WRITE, 0xF001F).unwrap();
        assert_eq!((all.protection, all.shared), (3, true));
        let copy = view_mapping(read_only, access::READ).unwrap();
        assert_eq!((copy.protection, copy.shared), (1, true));
        assert_eq!(
            view_mapping(read_only, access::WRITE),
            Err(error::ACCESS_DENIED)
        );
        assert_eq!(
            view_mapping(read_only, access::COPY),
            Err(error::ACCESS_DENIED)
        );
        let private = view_mapping(protection(page::WRITECOPY).unwrap(), access::COPY).unwrap();
        assert_eq!((private.protection, private.shared), (3, false));
        assert_eq!(
            view_mapping(READ_WRITE, access::READ | access::EXECUTE),
            Err(error::ACCESS_DENIED)
        );
        let run = protection(page::EXECUTE_READ).unwrap();
        let run = view_mapping(run, access::READ | access::EXECUTE).unwrap();
        assert_eq!(run.protection, 5);
    }

    #[test]
    fn views_are_aligned_and_inside_their_mapping() {
        assert_eq!(view_length(8192, 0, 0), Ok(8192));
        assert_eq!(view_length(0x2_0000, 0x1_0000, 0), Ok(0x1_0000));
        assert_eq!(view_length(8192, 4096, 0), Err(error::MAPPED_ALIGNMENT));
        assert_eq!(view_length(8192, 0, 9000), Err(error::ACCESS_DENIED));
        assert_eq!(
            view_length(0x1_0000, 0x1_0000, 0),
            Err(error::ACCESS_DENIED)
        );
        assert_eq!(view_length(8192, 0, 100), Ok(100));
    }
}
