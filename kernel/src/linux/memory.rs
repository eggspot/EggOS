//! Linux's memory calls (M6, D53): `brk`, `mmap`, `munmap` and `mprotect`, on the process's
//! regions ([`crate::regions`]).
//!
//! Anonymous mappings, `MAP_SHARED` mappings of a window's pixels on `/dev/window` (D127),
//! whose pages map the window server's frames at once and are never the process's, and
//! mappings of a memfd (D139, [`super::memfd`]), which map the file's frames the same way.
//! A mapping of a regular file (D167) is a region whose pages are read from the file when first
//! touched, each into a frame of its own: private, so writes stay the process's, and read again
//! after `MADV_DONTNEED`; a page wholly past the file's end raises `SIGBUS`. `MAP_SHARED` of a
//! regular file is read-only here: its pages are the file as it was read, and it can never be
//! made writable. An anonymous mapping is a region whose pages are
//! mapped, zeroed, as the program first touches them. `MAP_SHARED` anonymous memory behaves
//! as private within one memory, which `CLONE_VM` children share; it marks the memory, and a
//! `fork` that would copy it is refused with `ENOSYS` until copy-on-write (D104). Placement is top-down
//! from below the stack's guard gap, as Linux places mappings; the break starts at the end of
//! the program's last segment ([`super::exec`]). Writable and executable at once is refused
//! with `EACCES`, as Linux's `PR_SET_MDWE` and SELinux's `execmem` do (decided by research,
//! D52); turning a writable mapping executable is allowed, so JITs and loaders work.
//!
//! Every call checks its arguments as Linux's man pages and observed behaviour say, with
//! Linux's error numbers, so a program sees the same error for the same mistake.

use super::Errno;
use super::descriptors::{Descriptors, FileKind, OpenFile};
use super::exec::{MINIMUM_ADDRESS, STACK_GUARD_GAP, STACK_RESERVE};
use super::memory_devices::MemoryDevice;
use crate::process::{Coverage, Placement, PopulateError, Process, Remap};
use crate::regions::{Access, RegionError};
use crate::vfs::Node;
use alloc::sync::Arc;
use eggos_hal::Platform;
use eggos_hal::memory::FRAME_SIZE;
use eggos_hal::paging::{Attributes, Format};

/// `prot` bits.
pub mod prot {
    pub const READ: u64 = 0x1;
    pub const WRITE: u64 = 0x2;
    pub const EXEC: u64 = 0x4;
    /// AArch64's branch-target pages: accepted and not enforced, as on a CPU without BTI.
    #[cfg(target_arch = "aarch64")]
    pub const BTI: u64 = 0x10;
    #[cfg(target_arch = "aarch64")]
    pub(super) const KNOWN: u64 = READ | WRITE | EXEC | BTI;
    #[cfg(not(target_arch = "aarch64"))]
    pub(super) const KNOWN: u64 = READ | WRITE | EXEC;
}

/// `flags` bits, the same on every architecture EggOS runs but for `MAP_32BIT`.
pub mod map {
    pub const SHARED: u64 = 0x01;
    pub const PRIVATE: u64 = 0x02;
    pub const SHARED_VALIDATE: u64 = 0x03;
    /// The field that holds one of the three above.
    pub const TYPE: u64 = 0x0f;
    pub const FIXED: u64 = 0x10;
    pub const ANONYMOUS: u64 = 0x20;
    /// x86_64 only: somewhere in the second gigabyte.
    #[cfg(target_arch = "x86_64")]
    pub const BIT32: u64 = 0x40;
    pub const GROWSDOWN: u64 = 0x100;
    pub const DENYWRITE: u64 = 0x800;
    pub const EXECUTABLE: u64 = 0x1000;
    pub const LOCKED: u64 = 0x2000;
    pub const NORESERVE: u64 = 0x4000;
    pub const POPULATE: u64 = 0x8000;
    pub const NONBLOCK: u64 = 0x1_0000;
    pub const STACK: u64 = 0x2_0000;
    pub const HUGETLB: u64 = 0x4_0000;
    pub const FIXED_NOREPLACE: u64 = 0x10_0000;
    /// What `MAP_SHARED_VALIDATE` accepts; any other bit is `EOPNOTSUPP` there (and ignored
    /// with the other types, as Linux does).
    pub(super) const KNOWN: u64 = TYPE
        | FIXED
        | ANONYMOUS
        | GROWSDOWN
        | DENYWRITE
        | EXECUTABLE
        | LOCKED
        | NORESERVE
        | POPULATE
        | NONBLOCK
        | STACK
        | HUGETLB
        | FIXED_NOREPLACE
        | ARCH_KNOWN;
    #[cfg(target_arch = "x86_64")]
    const ARCH_KNOWN: u64 = BIT32;
    #[cfg(not(target_arch = "x86_64"))]
    const ARCH_KNOWN: u64 = 0;

    /// Whether `flags` asks for `MAP_32BIT`, which only x86_64 has.
    pub(super) fn wants_32_bit(flags: u64) -> bool {
        #[cfg(target_arch = "x86_64")]
        return flags & BIT32 != 0;
        #[cfg(not(target_arch = "x86_64"))]
        {
            let _ = flags;
            false
        }
    }
}

/// `madvise`'s `advice` values, the same on every architecture.
pub mod advice {
    pub const NORMAL: u64 = 0;
    pub const RANDOM: u64 = 1;
    pub const SEQUENTIAL: u64 = 2;
    pub const WILLNEED: u64 = 3;
    pub const DONTNEED: u64 = 4;
    pub const FREE: u64 = 8;
    pub const REMOVE: u64 = 9;
    pub const DONTFORK: u64 = 10;
    pub const DOFORK: u64 = 11;
    pub const MERGEABLE: u64 = 12;
    pub const UNMERGEABLE: u64 = 13;
    pub const HUGEPAGE: u64 = 14;
    pub const NOHUGEPAGE: u64 = 15;
    pub const DONTDUMP: u64 = 16;
    pub const DODUMP: u64 = 17;
    pub const WIPEONFORK: u64 = 18;
    pub const KEEPONFORK: u64 = 19;
    pub const COLD: u64 = 20;
    pub const PAGEOUT: u64 = 21;
    pub const POPULATE_READ: u64 = 22;
    pub const POPULATE_WRITE: u64 = 23;
    pub const DONTNEED_LOCKED: u64 = 24;
    pub const COLLAPSE: u64 = 25;
    pub const HWPOISON: u64 = 100;
    pub const SOFT_OFFLINE: u64 = 101;

    /// Whether Linux 6.1 knows `advice`.
    pub(super) fn is_known(advice: u64) -> bool {
        matches!(
            advice,
            NORMAL..=DONTNEED | FREE..=COLLAPSE | HWPOISON | SOFT_OFFLINE
        )
    }
}

/// `mremap`'s `flags` bits.
pub mod remap {
    pub const MAYMOVE: u64 = 0x1;
    pub const FIXED: u64 = 0x2;
    pub const DONTUNMAP: u64 = 0x4;
    pub(super) const KNOWN: u64 = MAYMOVE | FIXED | DONTUNMAP;
}

/// Where `mmap` places what it is free to place: from below the stack's reserve and the guard
/// gap under it, down to [`MINIMUM_ADDRESS`].
pub(super) fn mmap_top<P: Platform>() -> u64 {
    <P::PageTableFormat as Format>::USER_END - STACK_RESERVE - STACK_GUARD_GAP
}

/// The attributes for `prot`, or `None` for `PROT_NONE`: what the hardware can express of
/// it. Write implies read, and execute read, on every architecture EggOS runs (as on Linux).
fn attributes(prot: u64) -> Result<Option<Attributes>, Errno> {
    if prot & !prot::KNOWN != 0 {
        return Err(Errno::EINVAL);
    }
    let (write, execute) = (prot & prot::WRITE != 0, prot & prot::EXEC != 0);
    Ok(match (write, execute) {
        (true, true) => return Err(Errno::EACCES),
        (true, false) => Some(Attributes::USER_READ_WRITE),
        (false, true) => Some(Attributes::USER_TEXT),
        (false, false) if prot & prot::READ != 0 => Some(Attributes::USER_READ_ONLY),
        (false, false) => None,
    })
}

/// `length` rounded up to whole pages, or `None` if that wraps.
fn page_aligned(length: u64) -> Option<u64> {
    length.checked_next_multiple_of(FRAME_SIZE)
}

fn mapped_file(file: Option<MappedFile<'_>>) -> Result<Arc<OpenFile>, Errno> {
    let file = file.ok_or(Errno::EBADF)?;
    let open = file.files.file(file.fd)?;
    match open.kind() {
        FileKind::Window(_)
        | FileKind::MemoryFile(_)
        | FileKind::Regular(_)
        | FileKind::MemoryDevice(MemoryDevice::Zero, _) => Ok(open),
        FileKind::Snapshot(_) | FileKind::MemoryDevice(..) | FileKind::Event(_) => {
            Err(Errno::ENODEV)
        }
        _ => Err(Errno::EBADF),
    }
}

/// `brk(address)`: moves the program break to `address` and returns where it is then, as the
/// raw system call does: the new break, or the current one if the move is refused (`brk(0)`
/// asks for it). Never an error number.
pub fn brk<P: Platform>(process: &Process<P>, address: u64) -> u64 {
    process.move_program_break(address, mmap_top::<P>())
}

/// The descriptor a mapping of a file maps, in its process's table.
#[derive(Debug, Clone, Copy)]
pub struct MappedFile<'a> {
    pub files: &'a Descriptors,
    pub fd: u64,
}

/// `mmap(address, length, prot, flags, -1, offset)`, an anonymous mapping: returns the
/// mapping's start (any other is `EBADF`).
pub fn mmap<P: Platform>(
    process: &Process<P>,
    address: u64,
    length: u64,
    prot: u64,
    flags: u64,
    offset: u64,
) -> Result<u64, Errno> {
    map_memory(process, address, length, prot, flags, None, offset)
}

/// `mmap(address, length, prot, flags, fd, offset)`: anonymous mappings, `MAP_SHARED`
/// mappings of a window's pixels on `/dev/window` (D127; `EACCES` with `MAP_PRIVATE`,
/// `EPERM` executable or inaccessible), mappings of a memfd (D139: shared, or private
/// and read-only; `ENODEV` private and writable, `EPERM` executable or inaccessible), and
/// mappings of a regular file (D167: private, or shared and read-only; `EACCES` for a file not
/// open for reading, or shared and writable without write access to it; `ENODEV` shared and
/// writable; `EOVERFLOW` past the largest offset). A mapping of `/dev/zero` is anonymous
/// memory, shared or private, as on Linux (D168); a file made when it is opened (`/proc`) and
/// the other memory devices are `ENODEV`; any other descriptor is `EBADF`. Returns the
/// mapping's start.
pub fn mmap_file<P: Platform>(
    process: &Process<P>,
    address: u64,
    length: u64,
    prot: u64,
    flags: u64,
    file: MappedFile<'_>,
    offset: u64,
) -> Result<u64, Errno> {
    map_memory(process, address, length, prot, flags, Some(file), offset)
}

#[inline(always)]
fn map_memory<P: Platform>(
    process: &Process<P>,
    address: u64,
    length: u64,
    prot: u64,
    flags: u64,
    file: Option<MappedFile<'_>>,
    offset: u64,
) -> Result<u64, Errno> {
    if !offset.is_multiple_of(FRAME_SIZE) {
        return Err(Errno::EINVAL);
    }
    let kind = flags & map::TYPE;
    if !matches!(kind, map::SHARED | map::PRIVATE | map::SHARED_VALIDATE) {
        return Err(Errno::EINVAL);
    }
    if kind == map::SHARED_VALIDATE && flags & !map::KNOWN != 0 {
        return Err(Errno::EOPNOTSUPP);
    }
    if length == 0 {
        return Err(Errno::EINVAL);
    }
    let length = page_aligned(length).ok_or(Errno::ENOMEM)?;
    if flags & map::ANONYMOUS == 0 {
        return map_file(process, address, length, prot, flags, file, offset);
    }
    if flags & map::HUGETLB != 0 {
        return Err(Errno::ENOMEM);
    }
    let attributes = attributes(prot)?;
    if kind != map::PRIVATE {
        process.mark_shared_anonymous();
    }
    let placement = placement::<P>(address, length, flags)?;
    process
        .map_region(placement, length, attributes)
        .map_err(|error| match error {
            RegionError::Overlaps => Errno::EEXIST,
            RegionError::Invalid
            | RegionError::NotCovered
            | RegionError::NoRoom
            | RegionError::OutOfMemory
            | RegionError::NotWritable => Errno::ENOMEM,
        })
}

#[inline(never)]
fn map_file<P: Platform>(
    process: &Process<P>,
    address: u64,
    length: u64,
    prot: u64,
    flags: u64,
    file: Option<MappedFile<'_>>,
    offset: u64,
) -> Result<u64, Errno> {
    let kind = flags & map::TYPE;
    let file = mapped_file(file)?;
    if flags & map::HUGETLB != 0 {
        return Err(Errno::ENOMEM);
    }
    if let FileKind::MemoryDevice(MemoryDevice::Zero, _) = file.kind() {
        return mmap::<P>(process, address, length, prot, flags | map::ANONYMOUS, 0);
    }
    let attributes = attributes(prot)?;
    let shared = kind != map::PRIVATE;
    if let FileKind::Regular(node) = file.kind() {
        let wanted = Wanted {
            address,
            length,
            attributes,
            flags,
            offset,
        };
        return map_regular(process, *node, &file, wanted);
    }
    match file.kind() {
        FileKind::Window(_) if !shared => return Err(Errno::EACCES),
        FileKind::MemoryFile(_) if !shared && attributes.is_some_and(|a| a.writable) => {
            return Err(Errno::ENODEV);
        }
        _ => {}
    }
    let Some(attributes) = attributes.filter(|attributes| !attributes.executable) else {
        return Err(Errno::EPERM);
    };
    let placement = placement::<P>(address, length, flags)?;
    match file.kind() {
        FileKind::Window(connection) => {
            super::window::map(process, connection, placement, length, attributes, offset)
        }
        FileKind::MemoryFile(memory) => super::memfd::map(
            process, memory, placement, length, attributes, offset, shared,
        ),
        _ => Err(Errno::EBADF),
    }
}

/// What an `mmap` of a regular file asks for, its checked arguments.
#[derive(Debug, Clone, Copy)]
struct Wanted {
    address: u64,
    length: u64,
    attributes: Option<Attributes>,
    flags: u64,
    offset: u64,
}

/// `mmap` of `node`, a regular file open as `file`, as `wanted` says (D167): a region whose
/// pages are read from the file when first touched ([`Process::map_file_region`]). As Linux,
/// in this order: `EACCES` for a file not open for reading, or a shared writable mapping of a
/// file not open for writing; `EOVERFLOW` for a range past the largest offset. A shared
/// mapping is read-only here: writable is `ENODEV`, and it can never be made writable
/// (`mprotect`'s `EACCES`). Executable mappings are allowed: they are how a dynamic loader maps
/// a library's code.
fn map_regular<P: Platform>(
    process: &Process<P>,
    node: Node,
    file: &OpenFile,
    wanted: Wanted,
) -> Result<u64, Errno> {
    let shared = wanted.flags & map::TYPE != map::PRIVATE;
    let writes = wanted
        .attributes
        .is_some_and(|attributes| attributes.writable);
    if !file.is_readable() || (shared && writes && !file.is_writable()) {
        return Err(Errno::EACCES);
    }
    if wanted
        .offset
        .checked_add(wanted.length)
        .is_none_or(|end| end > i64::MAX as u64)
    {
        return Err(Errno::EOVERFLOW);
    }
    if shared && writes {
        return Err(Errno::ENODEV);
    }
    let placement = placement::<P>(wanted.address, wanted.length, wanted.flags)?;
    process
        .map_file_region(
            placement,
            wanted.length,
            wanted.attributes,
            Arc::new(node),
            wanted.offset,
            !shared,
        )
        .map_err(|error| match error {
            RegionError::Overlaps => Errno::EEXIST,
            RegionError::Invalid
            | RegionError::NotCovered
            | RegionError::NoRoom
            | RegionError::OutOfMemory
            | RegionError::NotWritable => Errno::ENOMEM,
        })
}

#[inline(always)]
fn placement<P: Platform>(address: u64, length: u64, flags: u64) -> Result<Placement, Errno> {
    let user_end = <P::PageTableFormat as Format>::USER_END;
    let placement = if flags & (map::FIXED | map::FIXED_NOREPLACE) != 0 {
        if !address.is_multiple_of(FRAME_SIZE) {
            return Err(Errno::EINVAL);
        }
        if address.checked_add(length).is_none_or(|end| end > user_end) {
            return Err(Errno::ENOMEM);
        }
        if address < MINIMUM_ADDRESS {
            return Err(Errno::EPERM);
        }
        if flags & map::FIXED_NOREPLACE != 0 {
            Placement::Exactly(address)
        } else {
            Placement::Replace(address)
        }
    } else if map::wants_32_bit(flags) {
        Placement::Anywhere {
            hint: None,
            floor: 0x4000_0000,
            top: 0x8000_0000,
        }
    } else {
        let hint = (address != 0)
            .then(|| address.checked_next_multiple_of(FRAME_SIZE))
            .flatten();
        Placement::Anywhere {
            hint,
            floor: MINIMUM_ADDRESS,
            top: mmap_top::<P>(),
        }
    };
    Ok(placement)
}

/// `munmap(address, length)`: unmaps whatever is mapped in the range, freeing the pages.
/// A range with nothing mapped is fine, as on Linux.
pub fn munmap<P: Platform>(process: &Process<P>, address: u64, length: u64) -> Result<u64, Errno> {
    let user_end = <P::PageTableFormat as Format>::USER_END;
    if !address.is_multiple_of(FRAME_SIZE) || length == 0 {
        return Err(Errno::EINVAL);
    }
    let length = page_aligned(length).ok_or(Errno::EINVAL)?;
    if length > user_end || address > user_end - length {
        return Err(Errno::EINVAL);
    }
    process
        .unmap_range(address, address + length)
        .map_err(|_| Errno::EINVAL)?;
    Ok(0)
}

/// `mprotect(address, length, prot)`: gives the range's mappings `prot`. Part of the range
/// unmapped is `ENOMEM`, after what lies before it has changed, as on Linux.
pub fn mprotect<P: Platform>(
    process: &Process<P>,
    address: u64,
    length: u64,
    prot: u64,
) -> Result<u64, Errno> {
    if !address.is_multiple_of(FRAME_SIZE) {
        return Err(Errno::EINVAL);
    }
    if length == 0 {
        return Ok(0);
    }
    let attributes = attributes(prot)?;
    let length = page_aligned(length).ok_or(Errno::ENOMEM)?;
    let end = address.checked_add(length).ok_or(Errno::ENOMEM)?;
    if end > <P::PageTableFormat as Format>::USER_END {
        return Err(Errno::ENOMEM);
    }
    process
        .protect_range(address, end, attributes)
        .map_err(|error| match error {
            RegionError::NotCovered => Errno::ENOMEM,
            RegionError::NotWritable => Errno::EACCES,
            _ => Errno::EINVAL,
        })?;
    Ok(0)
}

/// `msync`'s `flags` bits.
pub mod sync {
    pub const ASYNC: u64 = 0x1;
    pub const INVALIDATE: u64 = 0x2;
    pub const SYNC: u64 = 0x4;
    pub(super) const KNOWN: u64 = ASYNC | INVALIDATE | SYNC;
}

/// `msync(address, length, flags)` (D167): as Linux, `EINVAL` for an unknown flag,
/// `MS_ASYNC` with `MS_SYNC`, or an unaligned address; `ENOMEM` for a length that wraps when
/// rounded up to pages or a range not all mapped; 0 for an empty range. Nothing is ever written
/// back: every mapping here is anonymous, private, a shared read-only file mapping, or memory
/// a device or a memory file lends, none of which has changes a file waits for.
pub fn msync<P: Platform>(
    process: &Process<P>,
    address: u64,
    length: u64,
    flags: u64,
) -> Result<u64, Errno> {
    if flags & !sync::KNOWN != 0
        || flags & (sync::ASYNC | sync::SYNC) == sync::ASYNC | sync::SYNC
        || !address.is_multiple_of(FRAME_SIZE)
    {
        return Err(Errno::EINVAL);
    }
    let length = page_aligned(length).ok_or(Errno::ENOMEM)?;
    let end = address.checked_add(length).ok_or(Errno::ENOMEM)?;
    if length == 0 {
        return Ok(0);
    }
    match process.coverage(address, end) {
        Coverage::Whole => Ok(0),
        Coverage::Part | Coverage::Nothing => Err(Errno::ENOMEM),
    }
}

/// `madvise(address, length, advice)` on anonymous memory, checked in Linux's order:
/// `EINVAL` for an advice Linux does not know, an unaligned address, or a length that wraps
/// when rounded up to pages or past the end of the address space; then nothing to do for an
/// empty range. Pages are discarded (`DONTNEED`, `DONTNEED_LOCKED`, `FREE`) or made ready
/// (`POPULATE_READ`, `POPULATE_WRITE`) in the regions the range meets, and a part of it in no
/// region is `ENOMEM` once the rest is done. Advice about paging, sharing, merging, huge pages,
/// core dumps and `fork` is accepted and changes nothing: there is no swap, no `fork` and no
/// core dump yet, and anonymous memory is private (D53). `REMOVE` needs shared memory and
/// `COLLAPSE` huge pages: `EINVAL`. `HWPOISON` and `SOFT_OFFLINE` are `EPERM` for an
/// unprivileged process and `EINVAL` for root, as no page is ever taken offline (D77). A page
/// of a file mapping that `DONTNEED` discards is read from the file again when next touched;
/// `FREE` on a file mapping is `EINVAL`, as on Linux (D167).
pub fn madvise<P: Platform>(
    process: &Process<P>,
    address: u64,
    length: u64,
    advice: u64,
) -> Result<u64, Errno> {
    if !advice::is_known(advice) || !address.is_multiple_of(FRAME_SIZE) {
        return Err(Errno::EINVAL);
    }
    let length = page_aligned(length).ok_or(Errno::EINVAL)?;
    let end = address.checked_add(length).ok_or(Errno::EINVAL)?;
    if length == 0 {
        return Ok(0);
    }
    let not_covered = |coverage| match coverage {
        Coverage::Whole => Ok(0),
        Coverage::Part | Coverage::Nothing => Err(Errno::ENOMEM),
    };
    match advice {
        advice::FREE if process.maps_file_in(address, end) => Err(Errno::EINVAL),
        advice::DONTNEED | advice::DONTNEED_LOCKED | advice::FREE => {
            match process.discard_pages(address, end) {
                Ok(()) => Ok(0),
                Err(_) => Err(Errno::ENOMEM),
            }
        }
        advice::POPULATE_READ | advice::POPULATE_WRITE => {
            let access = if advice == advice::POPULATE_WRITE {
                Access::Write
            } else {
                Access::Read
            };
            match process.populate_range(address, end, access) {
                Ok(()) => Ok(0),
                Err(PopulateError::NotAllowed) => Err(Errno::EINVAL),
                Err(PopulateError::OutOfMemory | PopulateError::NotCovered) => Err(Errno::ENOMEM),
                Err(PopulateError::BusError) => Err(Errno::EFAULT),
            }
        }
        advice::REMOVE | advice::COLLAPSE => match process.coverage(address, end) {
            Coverage::Nothing => Err(Errno::ENOMEM),
            Coverage::Part | Coverage::Whole => Err(Errno::EINVAL),
        },
        advice::HWPOISON | advice::SOFT_OFFLINE => {
            let privileged = process
                .credentials_and_limits()
                .lock()
                .credentials
                .is_privileged();
            Err(if privileged {
                Errno::EINVAL
            } else {
                Errno::EPERM
            })
        }
        _ => not_covered(process.coverage(address, end)),
    }
}

/// `mremap(old_address, old_size, new_size, flags, new_address)` on anonymous memory, checked
/// in Linux's order (D77): `EINVAL` for unknown flags, `MREMAP_FIXED` without
/// `MREMAP_MAYMOVE`, `MREMAP_DONTUNMAP` without it or with sizes that differ, an unaligned old
/// address, or a size that wraps or is 0 when rounded up to pages; then `EFAULT` if the old
/// address is in no region. Shrinking unmaps the tail; growing needs the old range in one
/// region (`EFAULT`) and grows it in place if the addresses after it are free and below where
/// `mmap` places mappings, so the stack's guard gap stays (D77), otherwise moves it with
/// `MREMAP_MAYMOVE` (`ENOMEM` without). A moved range takes its pages with it, never a
/// copy. `MREMAP_FIXED` moves to `new_address`, replacing what was there, and
/// `MREMAP_DONTUNMAP` leaves the old range mapped with no pages; the new range is checked
/// before anything is unmapped. Returns the range's new start.
pub fn mremap<P: Platform>(
    process: &Process<P>,
    old_address: u64,
    old_size: u64,
    new_size: u64,
    flags: u64,
    new_address: u64,
) -> Result<u64, Errno> {
    let moves_to = flags & (remap::FIXED | remap::DONTUNMAP) != 0;
    if flags & !remap::KNOWN != 0
        || (moves_to && flags & remap::MAYMOVE == 0)
        || (flags & remap::DONTUNMAP != 0 && old_size != new_size)
        || !old_address.is_multiple_of(FRAME_SIZE)
    {
        return Err(Errno::EINVAL);
    }
    let old_length = page_aligned(old_size).ok_or(Errno::EINVAL)?;
    let new_length = page_aligned(new_size)
        .filter(|&length| length != 0)
        .ok_or(Errno::EINVAL)?;
    if process.region(old_address).is_none() {
        return Err(Errno::EFAULT);
    }
    if moves_to {
        return move_to::<P>(
            process,
            old_address,
            old_length,
            new_length,
            flags,
            new_address,
        );
    }
    if old_length >= new_length {
        if old_length > new_length {
            unmap_tail::<P>(process, old_address, new_length, old_length)?;
        }
        return Ok(old_address);
    }
    if old_length == 0 {
        return Err(Errno::EINVAL);
    }
    let remap = if flags & remap::MAYMOVE != 0 {
        Remap::InPlaceOrAnywhere {
            floor: MINIMUM_ADDRESS,
            top: mmap_top::<P>(),
        }
    } else {
        Remap::InPlace {
            top: mmap_top::<P>(),
        }
    };
    process
        .remap_region(old_address, old_length, new_length, remap, false)
        .map_err(remap_error)
}

/// [`mremap`] with `MREMAP_FIXED` or `MREMAP_DONTUNMAP`: moves the old range's first
/// `new_length` bytes, or all of it with the rest new, to `new_address` (`MREMAP_FIXED`) or
/// where it fits with `new_address` as a hint, and unmaps the rest of the old range.
fn move_to<P: Platform>(
    process: &Process<P>,
    old_address: u64,
    old_length: u64,
    new_length: u64,
    flags: u64,
    new_address: u64,
) -> Result<u64, Errno> {
    let user_end = <P::PageTableFormat as Format>::USER_END;
    if !new_address.is_multiple_of(FRAME_SIZE)
        || new_length > user_end
        || new_address > user_end - new_length
        || (new_address < old_address.saturating_add(old_length)
            && old_address < new_address + new_length)
    {
        return Err(Errno::EINVAL);
    }
    if old_length > new_length && old_address.saturating_add(old_length) > user_end {
        return Err(Errno::EINVAL);
    }
    let fixed = flags & remap::FIXED != 0;
    if fixed && new_address < MINIMUM_ADDRESS {
        return Err(Errno::EPERM);
    }
    let moved = old_length.min(new_length);
    if moved == 0 {
        return Err(Errno::EINVAL);
    }
    let remap = if fixed {
        Remap::Replace(new_address)
    } else {
        Remap::Anywhere {
            hint: (new_address != 0).then_some(new_address),
            floor: MINIMUM_ADDRESS,
            top: mmap_top::<P>(),
        }
    };
    let keep_source = flags & remap::DONTUNMAP != 0;
    let start = process
        .remap_region(old_address, moved, new_length, remap, keep_source)
        .map_err(remap_error)?;
    if old_length > moved {
        unmap_tail::<P>(process, old_address, moved, old_length)?;
    }
    Ok(start)
}

/// Unmaps `old_address + kept..old_address + old_length`, the part of an old range `mremap`
/// gives up, as `munmap` would: `EINVAL` if it reaches past user space.
fn unmap_tail<P: Platform>(
    process: &Process<P>,
    old_address: u64,
    kept: u64,
    old_length: u64,
) -> Result<(), Errno> {
    let end = old_address
        .checked_add(old_length)
        .filter(|&end| end <= <P::PageTableFormat as Format>::USER_END)
        .ok_or(Errno::EINVAL)?;
    process
        .unmap_range(old_address + kept, end)
        .map_err(|_| Errno::EINVAL)
}

/// The error number for [`Process::remap_region`]'s `error`.
fn remap_error(error: RegionError) -> Errno {
    match error {
        RegionError::NotCovered => Errno::EFAULT,
        RegionError::NoRoom | RegionError::OutOfMemory => Errno::ENOMEM,
        RegionError::Invalid | RegionError::Overlaps | RegionError::NotWritable => Errno::EINVAL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::thread::LinuxThread;
    use crate::linux::{handle, number};
    use crate::memory::FRAMES;
    use crate::paging::PageTables;
    use crate::regions::Region;
    use crate::tests::{TestPlatform as P, dropped_translations};
    use crate::thread::host::frames_for_test;
    use alloc::sync::Arc;
    use core::ops::ControlFlow;
    use eggos_hal::user::{UserContext, UserExit};

    const PAGE: u64 = FRAME_SIZE;
    const ANONYMOUS: u64 = map::PRIVATE | map::ANONYMOUS;
    const READ_WRITE: u64 = prot::READ | prot::WRITE;
    /// Where the test's program break starts, above its data region.
    const BREAK: u64 = 0x60_0000;

    fn kernel_tables() -> PageTables<<P as Platform>::PageTableFormat> {
        let mut frames = FRAMES.lock();
        let mut tables = PageTables::new(frames.as_mut().unwrap()).unwrap();
        tables.mark_active();
        tables
    }

    fn free() -> u64 {
        FRAMES.lock().as_ref().unwrap().free_frames()
    }

    /// A process with a data region of two pages below [`BREAK`], where its break starts.
    fn process(kernel: &PageTables<<P as Platform>::PageTableFormat>) -> Arc<Process<P>> {
        // SAFETY: test tables, never active; the test platform's switch only records roots.
        let mut process = unsafe { Process::<P>::new(kernel) }.unwrap();
        process
            .add_region(BREAK - 2 * PAGE, BREAK, Attributes::USER_READ_WRITE)
            .unwrap();
        process.set_program_break(BREAK);
        Arc::new(process)
    }

    /// Makes system call `number` with `arguments` through the Linux layer; its result.
    fn call(process: &Arc<Process<P>>, number: u64, arguments: [u64; 6]) -> u64 {
        let mut context = UserContext::new(0x40_0000, 0);
        context.simulate_syscall(number, arguments, 0x40_0004);
        assert_eq!(
            handle(
                process,
                &mut LinuxThread::first(process),
                &mut context,
                UserExit::Syscall
            ),
            ControlFlow::Continue(())
        );
        context.syscall_result()
    }

    fn mmap(process: &Arc<Process<P>>, address: u64, length: u64, prot: u64, flags: u64) -> u64 {
        call(
            process,
            number::MMAP,
            [address, length, prot, flags, u64::MAX, 0],
        )
    }

    fn word(process: &Process<P>, address: u64) -> Option<u64> {
        let mut word = [0; 8];
        process.read_user_memory(address, &mut word).ok()?;
        Some(u64::from_ne_bytes(word))
    }

    fn store(process: &Process<P>, address: u64, value: u64) -> bool {
        process
            .write_user_memory(address, &value.to_ne_bytes())
            .is_ok()
    }

    #[test]
    fn mmap_places_memory_top_down_where_it_is_free_and_maps_pages_on_first_touch() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process(&kernel);
        let top = mmap_top::<P>();
        let first = mmap(&process, 0, 3 * PAGE + 1, READ_WRITE, ANONYMOUS);
        assert_eq!(first, top - 4 * PAGE);
        assert_eq!(
            process.region(first),
            Some(Region {
                start: first,
                end: top,
                attributes: Some(Attributes::USER_READ_WRITE)
            })
        );
        assert!(process.address_space().translate(first).is_none());
        assert_eq!(word(&process, first + 8), Some(0));
        assert!(store(&process, first + 3 * PAGE, 7));
        let second = mmap(&process, 0, PAGE, prot::READ, map::SHARED | map::ANONYMOUS);
        assert_eq!(second, first - PAGE);
        assert!(!store(&process, second, 1));
        let code = mmap(&process, 0, PAGE, prot::EXEC, ANONYMOUS);
        assert_eq!(
            process.region(code).unwrap().attributes,
            Some(Attributes::USER_TEXT)
        );
        let hint = 0x1000_0000;
        assert_eq!(mmap(&process, hint - 5, PAGE, READ_WRITE, ANONYMOUS), hint);
        assert_eq!(
            mmap(&process, hint, PAGE, READ_WRITE, ANONYMOUS),
            code - PAGE
        );
        let fixed_new = ANONYMOUS | map::FIXED_NOREPLACE;
        assert_eq!(
            mmap(&process, hint, PAGE, READ_WRITE, fixed_new),
            Errno::EEXIST.result()
        );
        assert_eq!(
            mmap(&process, hint + PAGE, PAGE, READ_WRITE, fixed_new),
            hint + PAGE
        );
        let before = free();
        let fixed = ANONYMOUS | map::FIXED;
        assert_eq!(
            mmap(&process, first + 2 * PAGE, 2 * PAGE, prot::READ, fixed),
            first + 2 * PAGE
        );
        assert_eq!(free(), before + 1);
        assert_eq!(word(&process, first + 3 * PAGE), Some(0));
        assert!(!store(&process, first + 3 * PAGE, 7));
        assert!(store(&process, first + PAGE, 7));
        let none = mmap(&process, 0, PAGE, 0, ANONYMOUS);
        assert_eq!(process.region(none).unwrap().attributes, None);
        assert_eq!(word(&process, none), None);
    }

    #[test]
    fn mmap_checks_its_arguments_as_linux_does() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process(&kernel);
        let user_end = <<P as Platform>::PageTableFormat as Format>::USER_END;
        let fixed = ANONYMOUS | map::FIXED;
        for (address, length, prot, flags, offset, errno) in [
            (0, PAGE, READ_WRITE, ANONYMOUS, 1, Errno::EINVAL),
            (0, PAGE, READ_WRITE, map::ANONYMOUS, 0, Errno::EINVAL),
            (0, PAGE, READ_WRITE, map::ANONYMOUS | 0x4, 0, Errno::EINVAL),
            (
                0,
                PAGE,
                READ_WRITE,
                map::SHARED_VALIDATE | map::ANONYMOUS | 0x80_0000,
                0,
                Errno::EOPNOTSUPP,
            ),
            (0, 0, READ_WRITE, ANONYMOUS, 0, Errno::EINVAL),
            (0, u64::MAX, READ_WRITE, ANONYMOUS, 0, Errno::ENOMEM),
            (0, user_end, READ_WRITE, ANONYMOUS, 0, Errno::ENOMEM),
            (0, PAGE, READ_WRITE, map::PRIVATE, 0, Errno::EBADF),
            (
                0,
                PAGE,
                READ_WRITE,
                ANONYMOUS | map::HUGETLB,
                0,
                Errno::ENOMEM,
            ),
            (
                0,
                PAGE,
                prot::WRITE | prot::EXEC,
                ANONYMOUS,
                0,
                Errno::EACCES,
            ),
            (0, PAGE, 0x100, ANONYMOUS, 0, Errno::EINVAL),
            (0x10_0001, PAGE, READ_WRITE, fixed, 0, Errno::EINVAL),
            (
                user_end - PAGE,
                2 * PAGE,
                READ_WRITE,
                fixed,
                0,
                Errno::ENOMEM,
            ),
            (
                MINIMUM_ADDRESS - PAGE,
                PAGE,
                READ_WRITE,
                fixed,
                0,
                Errno::EPERM,
            ),
        ] {
            assert_eq!(
                call(
                    &process,
                    number::MMAP,
                    [address, length, prot, flags, u64::MAX, offset]
                ),
                errno.result(),
                "{address:#x} {length:#x} {prot:#x} {flags:#x} {offset:#x}"
            );
        }
        let address = call(
            &process,
            number::MMAP,
            [0, PAGE, READ_WRITE, ANONYMOUS | 0x80_0000, 3, PAGE],
        );
        assert!(process.region(address).is_some());
        assert_eq!(
            process
                .region(mmap_top::<P>() - PAGE)
                .map(|region| region.start),
            Some(address)
        );
    }

    #[test]
    fn munmap_takes_pages_out_and_frees_them_and_drops_their_translations() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process(&kernel);
        let start = mmap(&process, 0, 4 * PAGE, READ_WRITE, ANONYMOUS);
        for page in 0..4 {
            assert!(store(&process, start + page * PAGE, page));
        }
        let before = free();
        dropped_translations();
        let munmap =
            |address, length| call(&process, number::MUNMAP, [address, length, 0, 0, 0, 0]);
        let result = process.with_active(|| munmap(start + PAGE, 2 * PAGE - 5));
        assert_eq!(result, 0);
        assert_eq!(free(), before + 2);
        assert_eq!(dropped_translations(), [(start + PAGE, 2 * PAGE)]);
        assert_eq!(process.region(start).unwrap().end, start + PAGE);
        assert_eq!(
            process.region(start + 3 * PAGE).unwrap().start,
            start + 3 * PAGE
        );
        assert_eq!(process.region(start + PAGE), None);
        assert_eq!(word(&process, start + PAGE), None);
        assert_eq!(word(&process, start + 3 * PAGE), Some(3));
        assert_eq!(munmap(start, PAGE), 0);
        assert!(dropped_translations().is_empty());
        assert_eq!(munmap(start, 3 * PAGE), 0);
        assert_eq!(free(), before + 3);
        let user_end = <<P as Platform>::PageTableFormat as Format>::USER_END;
        for (address, length) in [
            (start + 1, PAGE),
            (start, 0),
            (start, u64::MAX),
            (user_end - PAGE, 2 * PAGE),
        ] {
            assert_eq!(
                munmap(address, length),
                Errno::EINVAL.result(),
                "{address:#x}"
            );
        }
        assert_eq!(word(&process, start + 3 * PAGE), Some(3));
    }

    #[test]
    fn mprotect_changes_mapped_pages_and_inaccessible_ones_keep_their_contents() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let before = free();
        {
            let process = process(&kernel);
            let start = mmap(&process, 0, 3 * PAGE, READ_WRITE, ANONYMOUS);
            assert!(store(&process, start, 0x1234));
            assert!(store(&process, start + PAGE, 0x5678));
            let mprotect = |address, length, prot| {
                call(&process, number::MPROTECT, [address, length, prot, 0, 0, 0])
            };
            let mapped = free();
            assert_eq!(mprotect(start, 2 * PAGE, 0), 0);
            assert!(process.address_space().translate(start).is_none());
            assert_eq!(word(&process, start), None);
            assert!(!store(&process, start + PAGE, 1));
            assert_eq!(process.region(start).unwrap().attributes, None);
            assert_eq!(free(), mapped);
            dropped_translations();
            let result = process.with_active(|| mprotect(start, 2 * PAGE, READ_WRITE));
            assert_eq!(result, 0);
            assert_eq!(dropped_translations(), [(start, 2 * PAGE)]);
            assert_eq!(word(&process, start), Some(0x1234));
            assert_eq!(word(&process, start + PAGE), Some(0x5678));
            assert_eq!(free(), mapped);
            assert_eq!(mprotect(start, PAGE, prot::READ), 0);
            assert!(!store(&process, start, 1));
            assert_eq!(
                process.address_space().translate(start).unwrap().1,
                Attributes::USER_READ_ONLY
            );
            assert_eq!(mprotect(start + PAGE, PAGE, prot::READ | prot::EXEC), 0);
            assert_eq!(
                process.address_space().translate(start + PAGE).unwrap().1,
                Attributes::USER_TEXT
            );
            assert_eq!(
                mprotect(start, PAGE, prot::WRITE | prot::EXEC),
                Errno::EACCES.result()
            );
            assert_eq!(
                call(
                    &process,
                    number::MUNMAP,
                    [start + 2 * PAGE, PAGE, 0, 0, 0, 0]
                ),
                0
            );
            assert_eq!(mprotect(start, 4 * PAGE, 0), Errno::ENOMEM.result());
            assert_eq!(process.region(start + PAGE).unwrap().attributes, None);
            assert_eq!(mprotect(start + 2 * PAGE, PAGE, 0), Errno::ENOMEM.result());
            for (address, length, prot, errno) in [
                (start + 1, PAGE, 0, Errno::EINVAL),
                (start, PAGE, 0x100, Errno::EINVAL),
                (start, u64::MAX, 0, Errno::ENOMEM),
            ] {
                assert_eq!(mprotect(address, length, prot), errno.result());
            }
            assert_eq!(mprotect(start + 1, 0, 0), Errno::EINVAL.result());
            assert_eq!(mprotect(start, 0, 0), 0);
            assert_eq!(mprotect(start, 0, 0x100), 0);
            assert_eq!(mprotect(start, 0, prot::WRITE | prot::EXEC), 0);
        }
        assert_eq!(free(), before);
    }

    #[test]
    fn brk_grows_and_shrinks_the_heap_from_the_end_of_the_program() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process(&kernel);
        let brk = |address| call(&process, number::BRK, [address, 0, 0, 0, 0, 0]);
        assert_eq!(brk(0), BREAK);
        assert_eq!(brk(BREAK + PAGE + 5), BREAK + PAGE + 5);
        assert_eq!(
            process.region(BREAK),
            Some(Region {
                start: BREAK - 2 * PAGE,
                end: BREAK + 2 * PAGE,
                attributes: Some(Attributes::USER_READ_WRITE)
            })
        );
        assert!(store(&process, BREAK + PAGE, 9));
        assert!(!store(&process, BREAK + 2 * PAGE, 9));
        let before = free();
        assert_eq!(brk(BREAK + 1), BREAK + 1);
        assert_eq!(free(), before + 1);
        assert_eq!(word(&process, BREAK + PAGE), None);
        let user_end = <<P as Platform>::PageTableFormat as Format>::USER_END;
        for refused in [BREAK - 1, mmap_top::<P>() + 1, user_end, u64::MAX] {
            assert_eq!(brk(refused), BREAK + 1, "{refused:#x}");
        }
        let fixed = ANONYMOUS | map::FIXED;
        assert_eq!(
            mmap(&process, BREAK + 4 * PAGE, PAGE, READ_WRITE, fixed),
            BREAK + 4 * PAGE
        );
        assert_eq!(brk(BREAK + 5 * PAGE), BREAK + 1);
        assert_eq!(brk(BREAK + 4 * PAGE), BREAK + 4 * PAGE);
        assert!(store(&process, BREAK - 8, 5));
        assert_eq!(brk(BREAK), BREAK);
        assert_eq!(word(&process, BREAK - 8), Some(5));
        assert_eq!(process.region(BREAK), None);
        // SAFETY: as in `process`.
        let bare = Arc::new(unsafe { Process::<P>::new(&kernel) }.unwrap());
        assert_eq!(call(&bare, number::BRK, [BREAK, 0, 0, 0, 0, 0]), 0);
    }

    fn madvise(process: &Arc<Process<P>>, address: u64, length: u64, advice: u64) -> u64 {
        call(process, number::MADVISE, [address, length, advice, 0, 0, 0])
    }

    fn mremap(
        process: &Arc<Process<P>>,
        old: u64,
        old_size: u64,
        new_size: u64,
        flags: u64,
        new: u64,
    ) -> u64 {
        call(
            process,
            number::MREMAP,
            [old, old_size, new_size, flags, new, 0],
        )
    }

    #[test]
    fn madvise_checks_its_arguments_in_linux_order_and_accepts_what_changes_nothing() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process(&kernel);
        let start = mmap(&process, 0, 2 * PAGE, READ_WRITE, ANONYMOUS);
        for (address, length, advice, errno) in [
            (start, 0, 5, Errno::EINVAL),
            (start, PAGE, 26, Errno::EINVAL),
            (start, PAGE, 99, Errno::EINVAL),
            (start + 1, 0, advice::NORMAL, Errno::EINVAL),
            (start, u64::MAX, advice::NORMAL, Errno::EINVAL),
            (start, u64::MAX - start - 2, advice::NORMAL, Errno::EINVAL),
            (start, 3 * PAGE, advice::WILLNEED, Errno::ENOMEM),
            (start - PAGE, PAGE, advice::NORMAL, Errno::ENOMEM),
            (start, PAGE, advice::REMOVE, Errno::EINVAL),
            (start - PAGE, PAGE, advice::REMOVE, Errno::ENOMEM),
            (start, PAGE, advice::COLLAPSE, Errno::EINVAL),
            (start, PAGE, advice::HWPOISON, Errno::EINVAL),
            (start, PAGE, advice::SOFT_OFFLINE, Errno::EINVAL),
        ] {
            assert_eq!(
                madvise(&process, address, length, advice),
                errno.result(),
                "{address:#x} {length:#x} {advice}"
            );
        }
        assert_eq!(madvise(&process, start, 0, advice::HWPOISON), 0);
        assert_eq!(madvise(&process, start - PAGE, 0, advice::NORMAL), 0);
        for advice in [
            advice::NORMAL,
            advice::RANDOM,
            advice::SEQUENTIAL,
            advice::WILLNEED,
            advice::DONTFORK,
            advice::DOFORK,
            advice::MERGEABLE,
            advice::UNMERGEABLE,
            advice::HUGEPAGE,
            advice::NOHUGEPAGE,
            advice::DONTDUMP,
            advice::DODUMP,
            advice::WIPEONFORK,
            advice::KEEPONFORK,
            advice::COLD,
            advice::PAGEOUT,
        ] {
            assert_eq!(
                madvise(&process, start, 2 * PAGE - 1, advice),
                0,
                "{advice}"
            );
        }
        assert_eq!(call(&process, number::SETUID, [1000, 0, 0, 0, 0, 0]), 0);
        for advice in [advice::HWPOISON, advice::SOFT_OFFLINE] {
            assert_eq!(
                madvise(&process, start, PAGE, advice),
                Errno::EPERM.result()
            );
        }
    }

    #[test]
    fn madvise_dontneed_and_free_give_frames_back_and_the_pages_read_zero_again() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process(&kernel);
        let start = mmap(&process, 0, 4 * PAGE, READ_WRITE, ANONYMOUS);
        for page in 0..4 {
            assert!(store(&process, start + page * PAGE, page + 1));
        }
        let mprotect =
            |address, prot| call(&process, number::MPROTECT, [address, PAGE, prot, 0, 0, 0]);
        assert_eq!(mprotect(start + PAGE, 0), 0);
        assert_eq!(
            call(
                &process,
                number::MUNMAP,
                [start + 2 * PAGE, PAGE, 0, 0, 0, 0]
            ),
            0
        );
        let before = free();
        dropped_translations();
        let result = process.with_active(|| madvise(&process, start, 4 * PAGE, advice::DONTNEED));
        assert_eq!(result, Errno::ENOMEM.result());
        assert_eq!(free(), before + 3);
        assert_eq!(
            dropped_translations(),
            [(start, 2 * PAGE), (start + 3 * PAGE, PAGE)]
        );
        assert_eq!(process.region(start + PAGE).unwrap().attributes, None);
        assert_eq!(mprotect(start + PAGE, READ_WRITE), 0);
        for page in [0, 1, 3] {
            assert_eq!(word(&process, start + page * PAGE), Some(0), "{page}");
        }
        for advice in [advice::FREE, advice::DONTNEED_LOCKED] {
            assert!(store(&process, start, 9));
            let before = free();
            assert_eq!(madvise(&process, start, PAGE, advice), 0);
            assert_eq!(free(), before + 1);
            assert_eq!(word(&process, start), Some(0));
        }
    }

    #[test]
    fn madvise_populate_maps_writable_pages_and_checks_the_access() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process(&kernel);
        let start = mmap(&process, 0, 4 * PAGE, READ_WRITE, ANONYMOUS);
        assert_eq!(
            call(&process, number::MUNMAP, [start + PAGE, PAGE, 0, 0, 0, 0]),
            0
        );
        assert!(store(&process, start + 3 * PAGE, 7));
        let before = free();
        assert_eq!(
            madvise(&process, start, 4 * PAGE, advice::POPULATE_READ),
            Errno::ENOMEM.result()
        );
        assert_eq!(free(), before);
        assert_eq!(
            madvise(&process, start, 4 * PAGE, advice::POPULATE_WRITE),
            Errno::ENOMEM.result()
        );
        assert_eq!(free(), before - 2);
        for page in [0, 2, 3] {
            assert!(
                process
                    .address_space()
                    .translate(start + page * PAGE)
                    .is_some()
            );
        }
        assert_eq!(word(&process, start + 3 * PAGE), Some(7));
        let populated = free();
        assert_eq!(
            madvise(&process, start + 2 * PAGE, 2 * PAGE, advice::POPULATE_WRITE),
            0
        );
        assert_eq!(free(), populated);
        assert_eq!(word(&process, start + 3 * PAGE), Some(7));
        let read_only = mmap(&process, 0, PAGE, prot::READ, ANONYMOUS);
        let none = mmap(&process, 0, PAGE, 0, ANONYMOUS);
        assert_eq!(madvise(&process, read_only, PAGE, advice::POPULATE_READ), 0);
        for (address, advice) in [
            (read_only, advice::POPULATE_WRITE),
            (none, advice::POPULATE_READ),
            (none, advice::POPULATE_WRITE),
        ] {
            assert_eq!(
                madvise(&process, address, PAGE, advice),
                Errno::EINVAL.result()
            );
        }
        assert!(process.address_space().translate(read_only).is_none());
    }

    #[test]
    fn mremap_checks_its_arguments_in_linux_order() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process(&kernel);
        let start = mmap(&process, 0, 2 * PAGE, READ_WRITE, ANONYMOUS);
        assert!(store(&process, start, 7));
        let user_end = <<P as Platform>::PageTableFormat as Format>::USER_END;
        let (may_move, fixed, keep) = (remap::MAYMOVE, remap::FIXED, remap::DONTUNMAP);
        let elsewhere = start - 0x10_0000;
        for (old, old_size, new_size, flags, new, errno) in [
            (start, PAGE, PAGE, 0x8, 0, Errno::EINVAL),
            (start, PAGE, PAGE, fixed, elsewhere, Errno::EINVAL),
            (start, PAGE, PAGE, keep, elsewhere, Errno::EINVAL),
            (start, PAGE, PAGE - 1, may_move | keep, 0, Errno::EINVAL),
            (start + 1, PAGE, PAGE, 0, 0, Errno::EINVAL),
            (start, PAGE, 0, 0, 0, Errno::EINVAL),
            (start, PAGE, u64::MAX, may_move, 0, Errno::EINVAL),
            (start, u64::MAX, PAGE, 0, 0, Errno::EINVAL),
            (start - PAGE, PAGE, 2 * PAGE, may_move, 0, Errno::EFAULT),
            (
                start,
                PAGE,
                PAGE,
                may_move | fixed,
                elsewhere + 1,
                Errno::EINVAL,
            ),
            (
                start,
                2 * PAGE,
                PAGE,
                may_move | fixed,
                start + PAGE,
                Errno::EINVAL,
            ),
            (start, PAGE, PAGE, may_move | fixed, user_end, Errno::EINVAL),
            (
                start,
                PAGE,
                PAGE,
                may_move | fixed,
                user_end - 1,
                Errno::EINVAL,
            ),
            (start, PAGE, PAGE, may_move | fixed, 0, Errno::EPERM),
            (
                start,
                user_end,
                PAGE,
                may_move | fixed,
                elsewhere,
                Errno::EINVAL,
            ),
            (start, user_end, PAGE, may_move | fixed, 0, Errno::EINVAL),
            (start, 0, PAGE, may_move | fixed, elsewhere, Errno::EINVAL),
            (start, 0, PAGE, may_move, 0, Errno::EINVAL),
            (start, 3 * PAGE, 4 * PAGE, may_move, 0, Errno::EFAULT),
        ] {
            assert_eq!(
                mremap(&process, old, old_size, new_size, flags, new),
                errno.result(),
                "{old:#x} {old_size:#x} {new_size:#x} {flags:#x} {new:#x}"
            );
        }
        assert_eq!(process.region(start).unwrap().end, start + 2 * PAGE);
        assert_eq!(word(&process, start), Some(7));
        assert_eq!(process.region(elsewhere), None);
    }

    #[test]
    fn mremap_shrinks_in_place_and_grows_in_place_where_the_addresses_after_are_free() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process(&kernel);
        let hint = 0x1000_0000;
        let start = mmap(&process, hint, 4 * PAGE, READ_WRITE, ANONYMOUS);
        assert_eq!(start, hint);
        for page in 0..4 {
            assert!(store(&process, start + page * PAGE, page + 1));
        }
        let before = free();
        assert_eq!(mremap(&process, start, 4 * PAGE, 2 * PAGE + 1, 0, 0), start);
        assert_eq!(free(), before + 1);
        assert_eq!(process.region(start).unwrap().end, start + 3 * PAGE);
        assert_eq!(mremap(&process, start, PAGE, PAGE, 0, 0), start);
        assert_eq!(mremap(&process, start, 3 * PAGE, 5 * PAGE, 0, 0), start);
        assert_eq!(process.region(start).unwrap().end, start + 5 * PAGE);
        assert_eq!(word(&process, start + 2 * PAGE), Some(3));
        assert_eq!(word(&process, start + 4 * PAGE), Some(0));
        let fixed = ANONYMOUS | map::FIXED;
        let blocker = start + 6 * PAGE;
        assert_eq!(mmap(&process, blocker, PAGE, prot::READ, fixed), blocker);
        assert_eq!(
            mremap(&process, start, 5 * PAGE, 7 * PAGE, 0, 0),
            Errno::ENOMEM.result()
        );
        assert_eq!(
            mremap(&process, start, 2 * PAGE, 3 * PAGE, 0, 0),
            Errno::ENOMEM.result()
        );
        assert_eq!(
            mremap(&process, start, 5 * PAGE, 6 * PAGE, 0, 0),
            start,
            "up to the next region"
        );
        assert_eq!(process.region(start).unwrap().end, blocker);
        let top = mmap(&process, 0, PAGE, READ_WRITE, ANONYMOUS);
        assert_eq!(top, mmap_top::<P>() - PAGE);
        assert_eq!(
            mremap(&process, top, PAGE, 2 * PAGE, 0, 0),
            Errno::ENOMEM.result(),
            "never into the stack's guard gap"
        );
    }

    #[test]
    fn mremap_moves_pages_with_their_contents_and_leaves_nothing_behind() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process(&kernel);
        let hint = 0x1000_0000;
        let start = mmap(&process, hint, 3 * PAGE, READ_WRITE, ANONYMOUS);
        let fixed = ANONYMOUS | map::FIXED;
        let blocker = start + 3 * PAGE;
        assert_eq!(mmap(&process, blocker, PAGE, prot::READ, fixed), blocker);
        assert!(store(&process, start, 0x1234));
        assert!(store(&process, start + PAGE, 0x5678));
        let mprotect = |address, length, prot| {
            call(&process, number::MPROTECT, [address, length, prot, 0, 0, 0])
        };
        assert_eq!(mprotect(start, 3 * PAGE, 0), 0);
        let before = free();
        dropped_translations();
        let moved =
            process.with_active(|| mremap(&process, start, 3 * PAGE, 5 * PAGE, remap::MAYMOVE, 0));
        assert_eq!(moved, mmap_top::<P>() - 5 * PAGE);
        assert_eq!(dropped_translations(), [(start, 3 * PAGE)]);
        assert!(free() <= before, "frames move, never copied");
        let tables = <<P as Platform>::PageTableFormat as Format>::LEVELS as u64 - 1;
        assert!(free() + tables >= before, "at most the new place's tables");
        assert_eq!(process.region(start), None);
        assert_eq!(word(&process, start), None);
        assert_eq!(
            process.region(moved),
            Some(Region {
                start: moved,
                end: moved + 5 * PAGE,
                attributes: None
            })
        );
        assert_eq!(word(&process, moved), None);
        assert_eq!(mprotect(moved, 5 * PAGE, READ_WRITE), 0);
        assert_eq!(word(&process, moved), Some(0x1234));
        assert_eq!(word(&process, moved + PAGE), Some(0x5678));
        assert_eq!(word(&process, moved + 4 * PAGE), Some(0));
        assert_eq!(
            mremap(&process, moved, 2 * PAGE, 3 * PAGE, 0, 0),
            Errno::ENOMEM.result(),
            "only a range that ends its region grows in place"
        );
        assert_eq!(mprotect(moved + 2 * PAGE, PAGE, prot::READ), 0);
        assert_eq!(
            mremap(&process, moved, 3 * PAGE, 6 * PAGE, remap::MAYMOVE, 0),
            Errno::EFAULT.result(),
            "a range in two regions"
        );
    }

    #[test]
    fn mremap_out_of_frames_for_tables_keeps_the_pages_and_drops_the_new_place_s_translations() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process(&kernel);
        let start = mmap(&process, 0x1000_0000, 2 * PAGE, READ_WRITE, ANONYMOUS);
        assert!(store(&process, start, 0x1234));
        assert!(store(&process, start + PAGE, 0x5678));
        let blocker = start + 2 * PAGE;
        assert_eq!(
            mmap(&process, blocker, PAGE, prot::READ, ANONYMOUS | map::FIXED),
            blocker
        );
        let taken: Vec<u64> =
            core::iter::from_fn(|| FRAMES.lock().as_mut().unwrap().allocate()).collect();
        dropped_translations();
        let result =
            process.with_active(|| mremap(&process, start, 2 * PAGE, 3 * PAGE, remap::MAYMOVE, 0));
        let dropped = dropped_translations();
        for frame in taken {
            // SAFETY: taken from `FRAMES` above and used for nothing.
            unsafe { FRAMES.lock().as_mut().unwrap().free(frame) };
        }
        assert_eq!(result, Errno::ENOMEM.result());
        assert_eq!(dropped, [(mmap_top::<P>() - 3 * PAGE, 2 * PAGE)]);
        assert_eq!(process.region(mmap_top::<P>() - 3 * PAGE), None);
        assert_eq!(word(&process, start), Some(0x1234));
        assert_eq!(word(&process, start + PAGE), Some(0x5678));
    }

    #[test]
    fn mremap_fixed_replaces_the_target_and_dontunmap_leaves_an_empty_mapping() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process(&kernel);
        let (first, target) = (0x1000_0000, 0x2000_0000);
        assert_eq!(
            mmap(&process, first, 3 * PAGE, READ_WRITE, ANONYMOUS),
            first
        );
        assert_eq!(
            mmap(&process, target, 2 * PAGE, READ_WRITE, ANONYMOUS),
            target
        );
        for page in 0..3 {
            assert!(store(&process, first + page * PAGE, page + 1));
        }
        assert!(store(&process, target + PAGE, 9));
        let before = free();
        let flags = remap::MAYMOVE | remap::FIXED;
        assert_eq!(
            mremap(&process, first, 3 * PAGE, 2 * PAGE, flags, target),
            target
        );
        assert_eq!(free(), before + 2, "the target's page and the old tail");
        assert_eq!(word(&process, target), Some(1));
        assert_eq!(word(&process, target + PAGE), Some(2));
        assert_eq!(process.region(first), None);
        assert_eq!(process.region(first + 2 * PAGE), None);
        let flags = remap::MAYMOVE | remap::DONTUNMAP;
        let kept = mremap(&process, target, 2 * PAGE, 2 * PAGE, flags, first);
        assert_eq!(kept, first, "the hint, free again");
        assert_eq!(word(&process, first + PAGE), Some(2));
        assert_eq!(
            process.region(target).map(|region| region.end),
            Some(target + 2 * PAGE)
        );
        assert!(process.address_space().translate(target).is_none());
        assert_eq!(word(&process, target), Some(0));
    }

    /// A tmpfs file of `contents`, open with `flags` as descriptor `fd` of the returned
    /// table.
    fn open_file(contents: &[u8], flags: u32) -> (Descriptors, u64) {
        use crate::linux::descriptors::open_flag;
        use crate::vfs::tmpfs::{Budget, Tmpfs};
        use crate::vfs::{FileSystem, NodeKind, WriteAt};
        let budget = alloc::boxed::Box::leak(alloc::boxed::Box::new(Budget::new(1 << 20)));
        let tmpfs = alloc::boxed::Box::leak(alloc::boxed::Box::new(
            Tmpfs::new(budget, (0, 31), 0o755).unwrap(),
        ));
        let file = tmpfs
            .create(tmpfs.root(), b"mapped", NodeKind::Regular, 0o644, 0)
            .unwrap();
        tmpfs.write(file, WriteAt::Offset(0), contents, 0).unwrap();
        let node = Node::root_of(tmpfs).lookup(b"mapped").unwrap();
        let files = Descriptors::with_console_streams();
        let open = Arc::new(OpenFile::new(
            FileKind::Regular(node),
            flags & open_flag::ACCMODE,
        ));
        let fd = files.lock().open(open, false, 64).unwrap();
        (files, u64::from(fd))
    }

    #[test]
    fn regular_files_map_privately_or_shared_read_only_with_linux_errors() {
        use crate::linux::descriptors::open_flag;
        use alloc::vec::Vec;
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let process = process(&kernel);
        let contents: Vec<u8> = (0..PAGE + 10).map(|at| at as u8 | 1).collect();
        let (files, fd) = open_file(&contents, open_flag::RDONLY);
        let file = MappedFile { files: &files, fd };
        let mapping =
            |prot, flags, offset| mmap_file::<P>(&process, 0, 3 * PAGE, prot, flags, file, offset);
        let private = mapping(READ_WRITE, map::PRIVATE, 0).unwrap();
        assert_eq!(
            word(&process, private),
            Some(u64::from_ne_bytes(contents[..8].try_into().unwrap()))
        );
        assert!(
            store(&process, private, 7),
            "a private mapping is the process's own"
        );
        assert_eq!(word(&process, private), Some(7));
        assert_eq!(
            super::madvise::<P>(&process, private, PAGE, advice::DONTNEED),
            Ok(0)
        );
        assert_eq!(
            word(&process, private),
            Some(u64::from_ne_bytes(contents[..8].try_into().unwrap())),
            "read from the file again"
        );
        assert_eq!(word(&process, private + 2 * PAGE), None, "past the end");
        assert_eq!(
            super::madvise::<P>(&process, private, PAGE, advice::FREE),
            Err(Errno::EINVAL)
        );
        let code = mapping(prot::READ | prot::EXEC, map::PRIVATE, PAGE).unwrap();
        assert_eq!(
            process.region(code).and_then(|region| region.attributes),
            Some(Attributes::USER_TEXT)
        );
        let shared = mapping(prot::READ, map::SHARED, 0).unwrap();
        assert_eq!(
            mprotect(&process, shared, PAGE, READ_WRITE),
            Err(Errno::EACCES)
        );
        assert_eq!(mapping(READ_WRITE, map::SHARED, 0), Err(Errno::EACCES));
        assert_eq!(mapping(prot::READ, map::PRIVATE, 1), Err(Errno::EINVAL));
        assert_eq!(
            mapping(prot::READ, map::PRIVATE, u64::MAX & !(PAGE - 1)),
            Err(Errno::EOVERFLOW)
        );
        let (writable, fd) = open_file(&contents, open_flag::RDWR);
        let both = MappedFile {
            files: &writable,
            fd,
        };
        assert_eq!(
            mmap_file::<P>(&process, 0, PAGE, READ_WRITE, map::SHARED, both, 0),
            Err(Errno::ENODEV)
        );
        let (unreadable, fd) = open_file(&contents, open_flag::WRONLY);
        let write_only = MappedFile {
            files: &unreadable,
            fd,
        };
        assert_eq!(
            mmap_file::<P>(&process, 0, PAGE, prot::READ, map::PRIVATE, write_only, 0),
            Err(Errno::EACCES)
        );
        assert_eq!(msync(&process, private, 3 * PAGE, sync::SYNC), Ok(0));
        assert_eq!(msync(&process, private, PAGE, 0), Ok(0));
        assert_eq!(msync(&process, private, 0, sync::ASYNC), Ok(0));
        assert_eq!(
            msync(&process, private, PAGE, sync::ASYNC | sync::SYNC),
            Err(Errno::EINVAL)
        );
        assert_eq!(msync(&process, private + 1, PAGE, 0), Err(Errno::EINVAL));
        assert_eq!(msync(&process, private, PAGE, 8), Err(Errno::EINVAL));
        assert_eq!(
            msync(&process, 0x7000_0000, PAGE, sync::SYNC),
            Err(Errno::ENOMEM)
        );
    }
}
