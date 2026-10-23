//! Linux's memory calls (M6, D53): `brk`, `mmap`, `munmap` and `mprotect`, on the process's
//! regions ([`crate::regions`]).
//!
//! Anonymous mappings only until there are files (M7): a mapping is a region whose pages are
//! mapped, zeroed, as the program first touches them. `MAP_SHARED` anonymous memory behaves
//! as private while a process cannot share it with another (`fork`). Placement is top-down
//! from below the stack's guard gap, as Linux places mappings; the break starts at the end of
//! the program's last segment ([`super::exec`]). Writable and executable at once is refused
//! with `EACCES`, as Linux's `PR_SET_MDWE` and SELinux's `execmem` do (decided by research,
//! D52); turning a writable mapping executable is allowed, so JITs and loaders work.
//!
//! Every call checks its arguments as Linux's man pages and observed behaviour say, with
//! Linux's error numbers, so a program sees the same error for the same mistake.

use super::Errno;
use super::exec::{MINIMUM_ADDRESS, STACK_GUARD_GAP, STACK_RESERVE};
use crate::process::{Placement, Process};
use crate::regions::RegionError;
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

/// Where `mmap` places what it is free to place: from below the stack's reserve and the guard
/// gap under it, down to [`MINIMUM_ADDRESS`].
fn mmap_top<P: Platform>() -> u64 {
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

/// `brk(address)`: moves the program break to `address` and returns where it is then, as the
/// raw system call does: the new break, or the current one if the move is refused (`brk(0)`
/// asks for it). Never an error number.
pub fn brk<P: Platform>(process: &Process<P>, address: u64) -> u64 {
    process.move_program_break(address, mmap_top::<P>())
}

/// `mmap(address, length, prot, flags, fd, offset)`, anonymous mappings only: returns the
/// mapping's start.
pub fn mmap<P: Platform>(
    process: &Process<P>,
    address: u64,
    length: u64,
    prot: u64,
    flags: u64,
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
        // No files yet (M7): no descriptor names one.
        return Err(Errno::EBADF);
    }
    if flags & map::HUGETLB != 0 {
        // No pool of huge pages, as on a Linux system that reserved none.
        return Err(Errno::ENOMEM);
    }
    let attributes = attributes(prot)?;
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
        // A hint is moved up to a page boundary and used if the range there is free.
        let hint = (address != 0)
            .then(|| address.checked_next_multiple_of(FRAME_SIZE))
            .flatten();
        Placement::Anywhere {
            hint,
            floor: MINIMUM_ADDRESS,
            top: mmap_top::<P>(),
        }
    };
    process
        .map_region(placement, length, attributes)
        .map_err(|error| match error {
            RegionError::Overlaps => Errno::EEXIST,
            RegionError::Invalid | RegionError::NotCovered | RegionError::NoRoom => Errno::ENOMEM,
        })
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
    let attributes = attributes(prot)?;
    if length == 0 {
        return Ok(0);
    }
    let length = page_aligned(length).ok_or(Errno::ENOMEM)?;
    let end = address.checked_add(length).ok_or(Errno::ENOMEM)?;
    if end > <P::PageTableFormat as Format>::USER_END {
        return Err(Errno::ENOMEM);
    }
    process
        .protect_range(address, end, attributes)
        .map_err(|error| match error {
            RegionError::NotCovered => Errno::ENOMEM,
            _ => Errno::EINVAL,
        })?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
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
            handle(process, &mut context, UserExit::Syscall),
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
        // Nothing mapped until touched; then a zeroed page.
        assert!(process.address_space().translate(first).is_none());
        assert_eq!(word(&process, first + 8), Some(0));
        assert!(store(&process, first + 3 * PAGE, 7));
        // The next goes below; read-only and executable ones refuse writes.
        let second = mmap(&process, 0, PAGE, prot::READ, map::SHARED | map::ANONYMOUS);
        assert_eq!(second, first - PAGE);
        assert!(!store(&process, second, 1));
        let code = mmap(&process, 0, PAGE, prot::EXEC, ANONYMOUS);
        assert_eq!(
            process.region(code).unwrap().attributes,
            Some(Attributes::USER_TEXT)
        );
        // A free hint is used, rounded up to a page; a taken one is not.
        let hint = 0x1000_0000;
        assert_eq!(mmap(&process, hint - 5, PAGE, READ_WRITE, ANONYMOUS), hint);
        assert_eq!(
            mmap(&process, hint, PAGE, READ_WRITE, ANONYMOUS),
            code - PAGE
        );
        // `MAP_FIXED_NOREPLACE`: only where nothing is.
        let fixed_new = ANONYMOUS | map::FIXED_NOREPLACE;
        assert_eq!(
            mmap(&process, hint, PAGE, READ_WRITE, fixed_new),
            Errno::EEXIST.result()
        );
        assert_eq!(
            mmap(&process, hint + PAGE, PAGE, READ_WRITE, fixed_new),
            hint + PAGE
        );
        // `MAP_FIXED` replaces what was there, pages and all: the frame goes back.
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
        // `PROT_NONE`: a region the program may not touch.
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
        // Unknown flags are ignored with the other types, and the descriptor and offset of
        // an anonymous mapping are too.
        let address = call(
            &process,
            number::MMAP,
            [0, PAGE, READ_WRITE, ANONYMOUS | 0x80_0000, 3, PAGE],
        );
        assert!(process.region(address).is_some());
        // Nothing was mapped by the refused calls.
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
        // The middle two, with the address space active here: its translations go too.
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
        // Not loaded here: nothing of it is cached on this CPU to drop.
        assert_eq!(munmap(start, PAGE), 0);
        assert!(dropped_translations().is_empty());
        // Nothing mapped there: fine.
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
            // No access: nothing translates, no copy reaches it, and no frame goes.
            assert_eq!(mprotect(start, 2 * PAGE, 0), 0);
            assert!(process.address_space().translate(start).is_none());
            assert_eq!(word(&process, start), None);
            assert!(!store(&process, start + PAGE, 1));
            assert_eq!(process.region(start).unwrap().attributes, None);
            assert_eq!(free(), mapped);
            // Back: the same frames, contents intact.
            dropped_translations();
            let result = process.with_active(|| mprotect(start, 2 * PAGE, READ_WRITE));
            assert_eq!(result, 0);
            assert_eq!(dropped_translations(), [(start, 2 * PAGE)]);
            assert_eq!(word(&process, start), Some(0x1234));
            assert_eq!(word(&process, start + PAGE), Some(0x5678));
            assert_eq!(free(), mapped);
            // Read-only, then executable (from writable: allowed), never both writable and
            // executable.
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
            // A hole: what lies before it changes, then `ENOMEM`.
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
        }
        // The inaccessible pages' frames are freed with the process.
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
        // One region with the data below, up to the page the break is in.
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
        // Shrinking gives the pages back.
        let before = free();
        assert_eq!(brk(BREAK + 1), BREAK + 1);
        assert_eq!(free(), before + 1);
        assert_eq!(word(&process, BREAK + PAGE), None);
        // Below its start, past where mappings go, or into a mapping: refused, unchanged.
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
        // Shrinking back to the start never touches the data below it.
        assert!(store(&process, BREAK - 8, 5));
        assert_eq!(brk(BREAK), BREAK);
        assert_eq!(word(&process, BREAK - 8), Some(5));
        assert_eq!(process.region(BREAK), None);
        // A process without a break (not a Linux program): 0.
        // SAFETY: as in `process`.
        let bare = Arc::new(unsafe { Process::<P>::new(&kernel) }.unwrap());
        assert_eq!(call(&bare, number::BRK, [BREAK, 0, 0, 0, 0, 0]), 0);
    }
}
