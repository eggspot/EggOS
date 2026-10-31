//! File mappings (D189): `CreateFileMappingW` over a file, or over the page file
//! (`INVALID_HANDLE_VALUE`), which is a file in `/dev/shm`: removed at once for an unnamed
//! mapping, named after its slot for a named one (`named.rs`), so other processes open it
//! with `OpenFileMappingW`. A mapping is an object handle (`sync.rs`) that keeps its file's
//! descriptor; `MapViewOfFile(Ex)` maps it with the kernel's `mmap` as `mapping_views.rs` says
//! (`MAP_SHARED` writable file mappings, D169), and each view is recorded until
//! `UnmapViewOfFile`, holding a reference to a named mapping so that it stays open while
//! viewed.

use super::named::{self, Reference, SectionPath};
use super::sync::{self, Kind};
use super::{Bool, FALSE, INVALID_HANDLE_VALUE, SpinLock, TRUE, error, error_for, fail};
use super::{file, thread_environment};
use crate::library::Export;
use crate::linux;
use crate::mapping_views::{self, Protection};
use crate::named_objects::{self, State};
use core::sync::atomic::{AtomicU32, Ordering};

const MAX_VIEWS: usize = 256;

/// A file mapping: a descriptor of the file behind it, its size and protection, and its slot
/// in the shared table when it has a name.
#[derive(Debug)]
pub struct Section {
    fd: usize,
    size: u64,
    protection: Protection,
    named: Option<Reference>,
}

impl Drop for Section {
    fn drop(&mut self) {
        let _ = linux::close(self.fd);
    }
}

#[derive(Debug)]
struct View {
    base: usize,
    length: usize,
    held: Option<Reference>,
}

static VIEWS: SpinLock<[Option<View>; MAX_VIEWS]> = SpinLock::new([const { None }; MAX_VIEWS]);
static UNNAMED_SERIAL: AtomicU32 = AtomicU32::new(0);

fn open_backing(path: &[u8], size: u64) -> Result<usize, u32> {
    let fd = linux::open(path, linux::O_RDWR | linux::O_CREAT).map_err(error_for)?;
    if !named::grow(fd, size) {
        let _ = linux::close(fd);
        return Err(error::NOT_ENOUGH_MEMORY);
    }
    Ok(fd)
}

fn unnamed_page_file(size: u64, protection: Protection) -> Result<Section, u32> {
    let serial = UNNAMED_SERIAL.fetch_add(1, Ordering::Relaxed);
    let path = SectionPath::unnamed(serial);
    let fd = open_backing(path.with_nul(), size);
    let _ = linux::remove(path.with_nul(), 0);
    Ok(Section {
        fd: fd?,
        size,
        protection,
        named: None,
    })
}

fn named_section(found: named::Named) -> Result<Section, u32> {
    if found.state.file_backed != 0 {
        return Err(error::NOT_SUPPORTED);
    }
    let protection = mapping_views::protection(found.state.protection)?;
    let fd = open_backing(found.reference.section_path().with_nul(), found.state.size)?;
    Ok(Section {
        fd,
        size: found.state.size,
        protection,
        named: Some(found.reference),
    })
}

fn page_file_section(
    flags: u32,
    protection: Protection,
    requested: u64,
    name: Option<&named_objects::Name<'_>>,
) -> Result<(Section, bool), u32> {
    let (size, _) = mapping_views::mapping_size(requested, None, protection)?;
    let Some(name) = name else {
        return Ok((unnamed_page_file(size, protection)?, false));
    };
    let state = State::section(size, flags & 0xff, false);
    let found = named::create(name, named_objects::kind::SECTION, state)?;
    let existed = found.existed;
    Ok((named_section(found)?, existed))
}

fn file_section(
    file: usize,
    flags: u32,
    protection: Protection,
    requested: u64,
    name: Option<&named_objects::Name<'_>>,
) -> Result<(Section, bool), u32> {
    let fd = file::fd_of(file).ok_or(error::INVALID_HANDLE)?;
    let file_size = linux::fstat(fd).map_err(error_for)?[linux::STAT_SIZE_WORD];
    let (size, grows) = mapping_views::mapping_size(requested, Some(file_size), protection)?;
    let (named, existed) = match name {
        Some(name) => {
            let state = State::section(size, flags & 0xff, true);
            let found = named::create(name, named_objects::kind::SECTION, state)?;
            if found.existed {
                return Ok((named_section(found)?, true));
            }
            (Some(found.reference), false)
        }
        None => (None, false),
    };
    if grows {
        linux::truncate(fd, size).map_err(error_for)?;
    }
    let copy = linux::control(fd, linux::F_DUPFD_CLOEXEC, 0).map_err(error_for)?;
    let section = Section {
        fd: copy,
        size,
        protection,
        named,
    };
    Ok((section, existed))
}

fn create_mapping(file: usize, flags: u32, requested: u64, name: Option<&[u16]>) -> usize {
    let protection = match mapping_views::protection(flags) {
        Ok(protection) => protection,
        Err(code) => return fail(code, 0),
    };
    let name = match name.map(named_objects::parse).transpose() {
        Ok(name) => name,
        Err(code) => return fail(code, 0),
    };
    let made = match file {
        INVALID_HANDLE_VALUE => page_file_section(flags, protection, requested, name.as_ref()),
        _ => file_section(file, flags, protection, requested, name.as_ref()),
    };
    match made {
        Ok((section, existed)) => {
            let (handle, _) = sync::insert(Kind::Section(section), 0);
            if handle != 0 {
                let code = match existed {
                    true => error::ALREADY_EXISTS,
                    false => error::SUCCESS,
                };
                thread_environment::set_last_error(code);
            }
            handle
        }
        Err(code) => fail(code, 0),
    }
}

fn joined(high: u32, low: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}

unsafe extern "efiapi" fn create_file_mapping_w(
    file: usize,
    _attributes: usize,
    flags: u32,
    size_high: u32,
    size_low: u32,
    name: *const u16,
) -> usize {
    // SAFETY: `CreateFileMappingW`'s contract: `name` is null or a NUL-terminated string.
    let name = unsafe { sync::object_name(name) };
    create_mapping(file, flags, joined(size_high, size_low), name)
}

unsafe extern "efiapi" fn create_file_mapping_a(
    file: usize,
    _attributes: usize,
    flags: u32,
    size_high: u32,
    size_low: u32,
    name: *const u8,
) -> usize {
    let create =
        |name: Option<&[u16]>| create_mapping(file, flags, joined(size_high, size_low), name);
    // SAFETY: `CreateFileMappingA`'s contract: `name` is null or a NUL-terminated string.
    unsafe { sync::with_ansi_name(name, create) }
}

fn open_mapping(name: Option<&[u16]>) -> usize {
    let Some(name) = name else {
        return fail(error::INVALID_PARAMETER, 0);
    };
    let opened = named_objects::parse(name)
        .and_then(|name| named::open(&name, named_objects::kind::SECTION))
        .and_then(named_section);
    match opened {
        Ok(section) => {
            let (handle, _) = sync::insert(Kind::Section(section), 0);
            if handle != 0 {
                thread_environment::set_last_error(error::SUCCESS);
            }
            handle
        }
        Err(code) => fail(code, 0),
    }
}

unsafe extern "efiapi" fn open_file_mapping_w(
    _access: u32,
    _inherit: Bool,
    name: *const u16,
) -> usize {
    // SAFETY: `OpenFileMappingW`'s contract: `name` is a NUL-terminated string.
    open_mapping(unsafe { sync::object_name(name) })
}

unsafe extern "efiapi" fn open_file_mapping_a(
    _access: u32,
    _inherit: Bool,
    name: *const u8,
) -> usize {
    // SAFETY: `OpenFileMappingA`'s contract: `name` is a NUL-terminated string.
    unsafe { sync::with_ansi_name(name, open_mapping) }
}

impl Section {
    fn map(&self, desired: u32, offset: u64, bytes: usize, base: usize) -> Result<View, u32> {
        let mapping = mapping_views::view_mapping(self.protection, desired)?;
        let length = mapping_views::view_length(self.size, offset, bytes as u64)?;
        let length = usize::try_from(length).map_err(|_| error::NOT_ENOUGH_MEMORY)?;
        let named = match &self.named {
            Some(reference) => Some(reference.another().ok_or(error::NOT_ENOUGH_MEMORY)?),
            None => None,
        };
        let sharing = match mapping.shared {
            true => linux::MAP_SHARED,
            false => linux::MAP_PRIVATE,
        };
        let placement = match base {
            0 => 0,
            _ => linux::MAP_FIXED_NOREPLACE,
        };
        let address = linux::map_file(
            base,
            length,
            mapping.protection,
            sharing | placement,
            self.fd,
            offset,
        )
        .map_err(|errno| match base {
            0 => error_for(errno),
            _ => error::INVALID_ADDRESS,
        })?;
        if base != 0 && address != base {
            // SAFETY: the mapping was just made and nothing uses it yet.
            let _ = unsafe { linux::unmap(address, length) };
            return Err(error::INVALID_ADDRESS);
        }
        Ok(View {
            base: address,
            length,
            held: named,
        })
    }
}

fn record(view: View) -> Result<usize, u32> {
    let refused = VIEWS.with(|views| match views.iter_mut().find(|slot| slot.is_none()) {
        Some(slot) => {
            let base = view.base;
            *slot = Some(view);
            Ok(base)
        }
        None => Err(view),
    });
    refused.map_err(|view| {
        // SAFETY: the view was never handed out, so nothing uses its memory.
        let _ = unsafe { linux::unmap(view.base, view.length) };
        error::NOT_ENOUGH_MEMORY
    })
}

fn map_view(handle: usize, desired: u32, offset: u64, bytes: usize, base: usize) -> usize {
    let mapped = sync::change(handle, |kind| match kind {
        Kind::Section(section) => (section.map(desired, offset, bytes, base), false),
        _ => (Err(error::INVALID_HANDLE), false),
    });
    match mapped
        .unwrap_or(Err(error::INVALID_HANDLE))
        .and_then(record)
    {
        Ok(address) => address,
        Err(code) => fail(code, 0),
    }
}

extern "efiapi" fn map_view_of_file(
    handle: usize,
    desired: u32,
    offset_high: u32,
    offset_low: u32,
    bytes: usize,
) -> usize {
    map_view(handle, desired, joined(offset_high, offset_low), bytes, 0)
}

extern "efiapi" fn map_view_of_file_ex(
    handle: usize,
    desired: u32,
    offset_high: u32,
    offset_low: u32,
    bytes: usize,
    base: usize,
) -> usize {
    map_view(
        handle,
        desired,
        joined(offset_high, offset_low),
        bytes,
        base,
    )
}

fn holds(view: &Option<View>, address: usize) -> bool {
    view.as_ref()
        .is_some_and(|view| (view.base..view.base + view.length).contains(&address))
}

extern "efiapi" fn unmap_view_of_file(address: usize) -> Bool {
    let taken = VIEWS.with(|views| {
        views
            .iter_mut()
            .find(|view| holds(view, address))
            .and_then(Option::take)
    });
    let Some(View { base, length, held }) = taken else {
        return fail(error::INVALID_ADDRESS, FALSE);
    };
    // SAFETY: `UnmapViewOfFile`'s contract: the program no longer uses the view's memory.
    let _ = unsafe { linux::unmap(base, length) };
    drop(held);
    TRUE
}

extern "efiapi" fn flush_view_of_file(address: usize, bytes: usize) -> Bool {
    let range = VIEWS.with(|views| {
        let view = views.iter().find(|view| holds(view, address))?.as_ref()?;
        let start = address & !(linux::PAGE_SIZE - 1);
        let end = match bytes {
            0 => view.base + view.length,
            _ => (address + bytes).min(view.base + view.length),
        };
        Some((start, end - start))
    });
    let Some((start, length)) = range else {
        return fail(error::INVALID_ADDRESS, FALSE);
    };
    match linux::sync_mapping(start, length) {
        Ok(_) => TRUE,
        Err(errno) => fail(error_for(errno), FALSE),
    }
}

pub(super) const EXPORTS: &[Export] = exports![
    b"CreateFileMappingW" => create_file_mapping_w,
    b"CreateFileMappingA" => create_file_mapping_a,
    b"OpenFileMappingW" => open_file_mapping_w,
    b"OpenFileMappingA" => open_file_mapping_a,
    b"MapViewOfFile" => map_view_of_file,
    b"MapViewOfFileEx" => map_view_of_file_ex,
    b"UnmapViewOfFile" => unmap_view_of_file,
    b"FlushViewOfFile" => flush_view_of_file,
];
