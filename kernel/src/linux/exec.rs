//! Starting a Linux program from its ELF image (M6, D51): what `execve` does once it has the
//! file's bytes.
//!
//! [`load`] first checks everything (the image, each `LOAD` segment, where the program
//! headers are, the size of the arguments), then maps the segments into a new process, each
//! page a frame of its own holding the file's bytes and zero after them (`.bss` included),
//! records the stack's region at the top of user space (its pages are mapped as the program
//! touches them, D52) and lays out on it what a Linux program expects at its entry: `argc`,
//! the `argv` and `envp` pointer arrays, each ended by a null pointer, and the auxiliary vector, with the strings they point to above them. It returns the registers
//! the program starts with: its entry point and that stack, every other register zero (C
//! libraries take a non-zero `rdx` / `x0` / `a0` as a function to run at exit).
//!
//! Static programs only: one that names an interpreter (a dynamic loader) is refused with
//! `ENOENT`, as Linux refuses one whose interpreter is missing, until there is a file system
//! to load it from (M7). Position-independent ones (static PIE, the default of Rust's musl
//! targets) are loaded at [`pie_base`], where the program relocates itself.

use super::Errno;
use super::credentials;
use crate::elf::{self, Elf, InvalidElf, Kind, ProgramHeader, segment};
use crate::process::Process;
use crate::random;
use alloc::vec;
use alloc::vec::Vec;
use eggos_hal::Platform;
use eggos_hal::memory::FRAME_SIZE;
use eggos_hal::paging::{Attributes, Format};
use eggos_hal::user::UserContext;

/// The address space kept for the stack, below the end of user space: Linux's default stack
/// limit. It is one region (D52): the pages holding the arguments are mapped up front, the rest
/// as the program first touches them. No segment may reach into it, nor into the
/// [`STACK_GUARD_GAP`] below it.
pub const STACK_RESERVE: u64 = 8 << 20;
/// Address space below [`STACK_RESERVE`] that stays unmapped, so a stack that overruns
/// faults instead of running into the program's data (Linux keeps the same gap).
pub const STACK_GUARD_GAP: u64 = 1 << 20;
/// The most bytes the arguments, the environment, their pointers and the auxiliary vector
/// may take on the stack: a quarter of [`STACK_RESERVE`], as Linux allows a quarter of its
/// stack limit. More is `E2BIG`.
pub const MAX_ARGUMENTS_SIZE: u64 = STACK_RESERVE / 4;
/// The longest single argument or environment string, with its NUL (Linux's `MAX_ARG_STRLEN`).
pub const MAX_STRING_SIZE: usize = 128 << 10;
/// No segment may start below this address, so a program can never map the page at 0 (a
/// null pointer must fault, and D49's SWAPGS fence is needed before it can map pages there).
/// Linux's default `mmap_min_addr`, and the lowest address RISC-V linkers place programs at.
pub const MINIMUM_ADDRESS: u64 = 0x1_0000;

/// The largest segment alignment accepted (`p_align`).
const MAX_ALIGN: u64 = 1 << 30;
/// [`pie_base`]'s smallest alignment.
const PIE_ALIGN: u64 = 2 << 20;

/// Auxiliary vector types (`AT_*`, from `getauxval(3)` and the psABIs).
pub mod auxv {
    pub const NULL: u64 = 0;
    pub const PHDR: u64 = 3;
    pub const PHENT: u64 = 4;
    pub const PHNUM: u64 = 5;
    pub const PAGESZ: u64 = 6;
    pub const BASE: u64 = 7;
    pub const FLAGS: u64 = 8;
    pub const ENTRY: u64 = 9;
    pub const UID: u64 = 11;
    pub const EUID: u64 = 12;
    pub const GID: u64 = 13;
    pub const EGID: u64 = 14;
    pub const HWCAP: u64 = 16;
    pub const CLKTCK: u64 = 17;
    pub const SECURE: u64 = 23;
    pub const RANDOM: u64 = 25;
    pub const HWCAP2: u64 = 26;
    pub const EXECFN: u64 = 31;
}

/// Why a program could not be started, as `execve` reports it ([`Self::errno`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecError {
    /// The image is not a program the kernel can run: `ENOEXEC`. The text says why.
    InvalidProgram(&'static str),
    /// The program names an interpreter, and there is none to load: `ENOENT`.
    NoInterpreter,
    /// A segment or the entry point is not within user space, or a segment has more file
    /// bytes than memory: `EINVAL`.
    InvalidSegment(&'static str),
    /// A segment starts below [`MINIMUM_ADDRESS`]: `EPERM`.
    BelowMinimumAddress,
    /// The arguments and environment do not fit in [`MAX_ARGUMENTS_SIZE`], or one string is
    /// longer than [`MAX_STRING_SIZE`]: `E2BIG`.
    ArgumentsTooLong,
    /// There was no memory for the program's pages or tables: `ENOMEM`.
    OutOfMemory(&'static str),
}

impl ExecError {
    pub fn errno(self) -> Errno {
        match self {
            Self::InvalidProgram(_) => Errno::ENOEXEC,
            Self::NoInterpreter => Errno::ENOENT,
            Self::InvalidSegment(_) => Errno::EINVAL,
            Self::BelowMinimumAddress => Errno::EPERM,
            Self::ArgumentsTooLong => Errno::E2BIG,
            Self::OutOfMemory(_) => Errno::ENOMEM,
        }
    }
}

impl From<InvalidElf> for ExecError {
    fn from(InvalidElf(reason): InvalidElf) -> Self {
        Self::InvalidProgram(reason)
    }
}

/// Where a position-independent program goes in a user space that ends at `user_end`: two
/// thirds of the way up, as Linux places one, leaving room below for `mmap` and above for the
/// stack, aligned to 2 MiB or to the program's largest segment alignment `align` if larger.
/// Fixed until address-space layout randomization draws from the random number generator (D56).
pub fn pie_base(user_end: u64, align: u64) -> u64 {
    (user_end / 3 * 2) & !(align.max(PIE_ALIGN) - 1)
}

/// Maps the program in `image` into `process` and lays out its stack with `path` (the name it
/// was started by, `AT_EXECFN`), `arguments` and `environment` (strings without their
/// terminating NUL; one inside a string ends it, as the program sees it). Returns the
/// registers it starts with. Nothing is mapped unless every check passed; if mapping then
/// runs out of memory, part of the program may be mapped, and the caller drops `process`,
/// which frees it.
pub fn load<P: Platform>(
    process: &mut Process<P>,
    image: &[u8],
    path: &[u8],
    arguments: &[&[u8]],
    environment: &[&[u8]],
) -> Result<UserContext, ExecError> {
    let user_end = <P::PageTableFormat as Format>::USER_END;
    let elf = Elf::parse(image, elf::machine::NATIVE)?;
    let bias = load_bias(&elf, user_end)?;
    // The program header table: its offset in the image and its size.
    let table = (
        elf.program_header_offset(),
        u64::from(elf.program_header_count()) * elf::PROGRAM_HEADER_SIZE as u64,
    );
    // Where `PHDR` says the table is, and where the `LOAD` segment that holds it maps it
    // (both before the bias).
    let mut named = None;
    let mut found = None;
    let mut mapped_end = 0;
    for header in elf.program_headers() {
        match header.kind {
            segment::INTERP => return Err(ExecError::NoInterpreter),
            segment::PHDR => named = Some(header.virt),
            segment::GNU_STACK if header.flags & segment::EXECUTE != 0 => {
                return Err(ExecError::InvalidProgram(
                    "a program that asks for an executable stack",
                ));
            }
            segment::LOAD if header.memory_size > 0 => {
                let (start, end) = check_segment(image, &header, bias, user_end)?;
                if start & !(FRAME_SIZE - 1) < mapped_end {
                    return Err(ExecError::InvalidProgram(
                        "segments that share a page or are out of order",
                    ));
                }
                mapped_end = end.next_multiple_of(FRAME_SIZE);
                let within = table
                    .0
                    .checked_sub(header.offset)
                    .filter(|within| within.saturating_add(table.1) <= header.file_size);
                if let (None, Some(within)) = (found, within) {
                    found = Some(header.virt.wrapping_add(within));
                }
            }
            _ => {}
        }
    }
    if mapped_end == 0 {
        return Err(ExecError::InvalidProgram("a program with nothing to load"));
    }
    // C libraries read the table at `AT_PHDR`, and static PIEs find their own base from
    // `PHDR`: it must be mapped, and where `PHDR` says.
    let program_headers = found.ok_or(ExecError::InvalidProgram(
        "the program headers are not in a segment",
    ))?;
    if named.is_some_and(|named| named != program_headers) {
        return Err(ExecError::InvalidProgram(
            "the program headers are not where PHDR says",
        ));
    }

    let entry = elf.entry().wrapping_add(bias);
    if elf.entry() >= user_end || entry >= user_end {
        return Err(ExecError::InvalidSegment(
            "an entry point outside user space",
        ));
    }
    let [hwcap, hwcap2] = P::linux_hardware_capabilities();
    let (user, group) = process
        .credentials_and_limits_mut()
        .credentials
        .after_exec()
        .map_err(|_| ExecError::OutOfMemory("the credentials"))?;
    let secure = u64::from(credentials::is_secure_execution(user, group));
    // Linux's order, so a dump of a program's vector compares line by line with Linux's.
    let auxiliary = [
        (auxv::HWCAP, hwcap),
        (auxv::PAGESZ, FRAME_SIZE),
        // `times(2)`'s unit, as on Linux.
        (auxv::CLKTCK, 100),
        (auxv::PHDR, program_headers.wrapping_add(bias)),
        (auxv::PHENT, elf::PROGRAM_HEADER_SIZE as u64),
        (auxv::PHNUM, u64::from(elf.program_header_count())),
        // No interpreter, so no interpreter's base.
        (auxv::BASE, 0),
        (auxv::FLAGS, 0),
        (auxv::ENTRY, entry),
        (auxv::UID, u64::from(user.real)),
        (auxv::EUID, u64::from(user.effective)),
        (auxv::GID, u64::from(group.real)),
        (auxv::EGID, u64::from(group.effective)),
        (auxv::SECURE, secure),
        // Filled in with the stack's layout: both point into it.
        (auxv::RANDOM, 0),
        (auxv::HWCAP2, hwcap2),
        (auxv::EXECFN, 0),
    ];
    let stack = InitialStack::new(
        user_end,
        path,
        arguments,
        environment,
        &auxiliary,
        &random_bytes::<P>(),
    )?;

    for header in elf.program_headers() {
        if header.kind == segment::LOAD && header.memory_size > 0 {
            map_segment(process, image, &header, bias)?;
        }
    }
    // Linux starts the break at the page-aligned end of the last segment.
    process.set_program_break(mapped_end);
    process
        .add_region(
            user_end - STACK_RESERVE,
            user_end,
            Attributes::USER_READ_WRITE,
        )
        .map_err(|_| ExecError::InvalidProgram("a segment in the stack's reserve"))?;
    // Maps the pages it writes, zeroed first, as the program's own stores would.
    process
        .write_user_memory(stack.start, &stack.bytes)
        .map_err(|_| ExecError::OutOfMemory("no frame for the stack"))?;
    Ok(UserContext::new(entry, stack.start))
}

/// What is added to every address in a program's headers: 0 for an executable, which runs
/// where it was linked; for a position-independent program, what moves it to [`pie_base`]
/// (its lowest segment there, or as far above as that segment is above its alignment).
fn load_bias(elf: &Elf<'_>, user_end: u64) -> Result<u64, ExecError> {
    let mut align = FRAME_SIZE;
    let mut lowest = u64::MAX;
    for header in elf.program_headers() {
        if header.kind != segment::LOAD || header.memory_size == 0 {
            continue;
        }
        // 0 and 1 ask for no alignment.
        if header.align > 1 {
            if !header.align.is_power_of_two() || header.align > MAX_ALIGN {
                return Err(ExecError::InvalidProgram(
                    "a segment alignment that is not a power of two up to 1 GiB",
                ));
            }
            align = align.max(header.align);
        }
        lowest = lowest.min(header.virt);
    }
    if elf.kind() == Kind::Executable || lowest == u64::MAX {
        return Ok(0);
    }
    Ok(pie_base(user_end, align).wrapping_sub(lowest & !(align - 1)))
}

/// Checks a `LOAD` segment with memory: its access; if it has file bytes, that they are within
/// `image` and that its file offset and address are equal within a page (each page maps one
/// page of the file); and its memory within user space, before and after the move by `bias`,
/// at or above [`MINIMUM_ADDRESS`] and below the stack's reserve. Returns its (start, end) in
/// memory.
fn check_segment(
    image: &[u8],
    header: &ProgramHeader,
    bias: u64,
    user_end: u64,
) -> Result<(u64, u64), ExecError> {
    segment_attributes(header.flags)?;
    if header.file_size > header.memory_size {
        return Err(ExecError::InvalidSegment(
            "a segment with more file bytes than memory",
        ));
    }
    // A segment without file bytes is zero pages alone: its offset means nothing.
    if header.file_size > 0 {
        if header.file_bytes(image).is_none() {
            return Err(ExecError::InvalidProgram(
                "a segment is not within the image",
            ));
        }
        if header.offset % FRAME_SIZE != header.virt % FRAME_SIZE {
            return Err(ExecError::InvalidProgram(
                "a segment whose file offset and address differ within a page",
            ));
        }
    }
    // Within user space before the bias too: a position-independent program's segments are
    // then less than `user_end` above its lowest, so moving them to the PIE base (below
    // `user_end`) cannot wrap around.
    let start = header.virt.wrapping_add(bias);
    let limit = user_end - STACK_RESERVE - STACK_GUARD_GAP;
    let end = start
        .checked_add(header.memory_size)
        .filter(|&end| end <= limit && start < limit && header.virt < user_end)
        .ok_or(ExecError::InvalidSegment(
            "a segment past the end of user space or in the stack's reserve",
        ))?;
    if start < MINIMUM_ADDRESS {
        return Err(ExecError::BelowMinimumAddress);
    }
    Ok((start, end))
}

/// Maps a checked `LOAD` segment ([`check_segment`]), moved by `bias`: every page it touches
/// gets a frame of its own with the file's bytes for that page (from the page's start, as a
/// file mapping has them) and zero after the segment's file bytes; one without file bytes is
/// all zero.
fn map_segment<P: Platform>(
    process: &mut Process<P>,
    image: &[u8],
    header: &ProgramHeader,
    bias: u64,
) -> Result<(), ExecError> {
    let attributes = segment_attributes(header.flags)?;
    let start = header.virt.wrapping_add(bias);
    let first_page = start & !(FRAME_SIZE - 1);
    // The segment's file bytes, from the start of its first page: the image holds the bytes
    // of that page before the segment just before them (checked: the offset and the address
    // are equal within a page, and the bias is page-aligned).
    let bytes = if header.file_size == 0 {
        &[][..]
    } else {
        let head = (start - first_page) as usize;
        let from = header.offset as usize - head;
        &image[from..header.offset as usize + header.file_size as usize]
    };
    let end = start + header.memory_size;
    for (index, page) in (first_page..end).step_by(FRAME_SIZE as usize).enumerate() {
        let offset = (index * FRAME_SIZE as usize).min(bytes.len());
        let contents = &bytes[offset..(offset + FRAME_SIZE as usize).min(bytes.len())];
        process
            .map_new_page(page, attributes, contents)
            .map_err(ExecError::OutOfMemory)?;
    }
    // Every page is mapped already; the region records what the program may do there.
    process
        .add_region(first_page, end.next_multiple_of(FRAME_SIZE), attributes)
        .map_err(|_| ExecError::InvalidProgram("segments that share a page"))
}

/// The page attributes for a segment's `p_flags`. Code is readable, as on every architecture
/// brought up. Writable code is refused: user pages are never writable and executable (D51),
/// as kernel pages never are.
fn segment_attributes(flags: u32) -> Result<Attributes, ExecError> {
    let write = flags & segment::WRITE != 0;
    let execute = flags & segment::EXECUTE != 0;
    match (write, execute) {
        (true, true) => Err(ExecError::InvalidProgram(
            "a segment that is both writable and executable",
        )),
        (false, true) => Ok(Attributes::USER_TEXT),
        (true, false) => Ok(Attributes::USER_READ_WRITE),
        (false, false) if flags & segment::READ != 0 => Ok(Attributes::USER_READ_ONLY),
        (false, false) => Err(ExecError::InvalidProgram("a segment with no access")),
    }
}

/// 16 bytes for `AT_RANDOM`, which C libraries take their stack-protector canary and pointer
/// guard from: from the kernel's random number generator (D56), one draw. Linux does not wait
/// for the generator to be seeded here either.
fn random_bytes<P: Platform>() -> [u8; 16] {
    let mut bytes = [0; 16];
    random::fill::<P>(&mut bytes);
    bytes
}

/// A program's stack at its entry, as Linux lays it out: from [`Self::start`], where the stack
/// pointer points (16-byte aligned), `argc`; the `argv` pointers and a null one; the `envp`
/// pointers and a null one; the auxiliary vector's (type, value) pairs ended by `AT_NULL`;
/// then, higher up, the 16 `AT_RANDOM` bytes, the argument strings, the environment strings
/// and the program's path, each NUL-terminated, and 8 zero bytes at the very top.
#[derive(Debug)]
struct InitialStack {
    /// The user address of the stack's lowest byte: the stack pointer.
    start: u64,
    /// Everything from `start` up to the top of the stack.
    bytes: Vec<u8>,
}

impl InitialStack {
    /// The stack for a program whose stack ends at `top` (page-aligned). `auxiliary`'s
    /// `AT_RANDOM` and `AT_EXECFN` entries get their values here, since they point into the
    /// stack, and `AT_NULL` ends it.
    fn new(
        top: u64,
        path: &[u8],
        arguments: &[&[u8]],
        environment: &[&[u8]],
        auxiliary: &[(u64, u64)],
        random: &[u8; 16],
    ) -> Result<Self, ExecError> {
        let too_long = |string: &&[u8]| string.len() >= MAX_STRING_SIZE;
        if too_long(&path) || arguments.iter().chain(environment).any(too_long) {
            return Err(ExecError::ArgumentsTooLong);
        }
        // Each list is bounded by the size check below before it could overflow anything: a
        // string takes at least its NUL.
        let strings = |list: &[&[u8]]| {
            list.iter()
                .fold(0u64, |sum, s| sum.saturating_add(s.len() as u64 + 1))
        };
        let argument_bytes = strings(arguments);
        let environment_bytes = strings(environment);
        // argc, argv and its null, envp and its null, the vector and `AT_NULL`.
        let words = (arguments.len() as u64 + environment.len() as u64)
            .saturating_add(3 + 2 * (auxiliary.len() as u64 + 1));
        // The 8-byte end marker, the strings, then (each 16-byte aligned) the random bytes
        // and the pointers: at most 15 bytes of padding each.
        let most = [
            8,
            path.len() as u64 + 1,
            environment_bytes,
            argument_bytes,
            16 + 15,
            words.saturating_mul(8).saturating_add(15),
        ]
        .iter()
        .fold(0u64, |sum, &size| sum.saturating_add(size));
        if most > MAX_ARGUMENTS_SIZE {
            return Err(ExecError::ArgumentsTooLong);
        }
        let path_at = top - 8 - (path.len() as u64 + 1);
        let environment_at = path_at - environment_bytes;
        let arguments_at = environment_at - argument_bytes;
        let random_at = (arguments_at - 16) & !15;
        let start = (random_at - words * 8) & !15;
        let mut bytes = vec![0u8; (top - start) as usize];
        let mut put = |address: u64, data: &[u8]| {
            let at = (address - start) as usize;
            bytes[at..at + data.len()].copy_from_slice(data);
        };
        put(path_at, path);
        put(random_at, random);
        let mut pointers = Vec::with_capacity(words as usize);
        pointers.push(arguments.len() as u64);
        for (list, mut address) in [(arguments, arguments_at), (environment, environment_at)] {
            for string in list {
                put(address, string);
                pointers.push(address);
                address += string.len() as u64 + 1;
            }
            pointers.push(0);
        }
        for &(kind, value) in auxiliary.iter().chain(&[(auxv::NULL, 0)]) {
            let value = match kind {
                auxv::RANDOM => random_at,
                auxv::EXECFN => path_at,
                _ => value,
            };
            pointers.extend([kind, value]);
        }
        for (index, word) in pointers.iter().enumerate() {
            put(start + index as u64 * 8, &word.to_le_bytes());
        }
        Ok(Self { start, bytes })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory;
    use crate::paging::PageTables;
    use crate::process::TEST_PROGRAM_ADDRESS;
    use crate::regions::Region;
    use crate::selftest::{
        LINUX_START_ARGUMENTS, LINUX_START_DATA, LINUX_START_DATA_BYTES, LINUX_START_DATA_SIZE,
        LINUX_START_ENVIRONMENT, linux_start_image,
    };
    use crate::tests::TestPlatform as P;
    use crate::thread::host::frames_for_test;
    use alloc::vec::Vec;

    const USER_END: u64 = <<P as Platform>::PageTableFormat as Format>::USER_END;
    const PATH: &[u8] = b"/bin/test";

    fn kernel_tables() -> PageTables<<P as Platform>::PageTableFormat> {
        let mut frames = memory::FRAMES.lock();
        let mut tables = PageTables::new(frames.as_mut().unwrap()).unwrap();
        tables.mark_active();
        tables
    }

    fn free() -> u64 {
        memory::FRAMES.lock().as_ref().unwrap().free_frames()
    }

    /// Loads `image` into a new process with the self-test's arguments and environment.
    fn load_image(
        kernel: &PageTables<<P as Platform>::PageTableFormat>,
        image: &[u8],
    ) -> (Process<P>, Result<UserContext, ExecError>) {
        // SAFETY: test tables, never active; the test platform's switch only records roots.
        let mut process = unsafe { Process::<P>::new(kernel) }.unwrap();
        let arguments = LINUX_START_ARGUMENTS.map(str::as_bytes);
        let environment = LINUX_START_ENVIRONMENT.map(str::as_bytes);
        let result = load(&mut process, image, PATH, &arguments, &environment);
        (process, result)
    }

    fn word(process: &Process<P>, virt: u64) -> u64 {
        let mut bytes = [0; 8];
        process.read_user_memory(virt, &mut bytes).unwrap();
        u64::from_le_bytes(bytes)
    }

    /// The NUL-terminated string at `virt`.
    fn string(process: &Process<P>, virt: u64) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut byte = [0];
        for at in virt.. {
            process.read_user_memory(at, &mut byte).unwrap();
            if byte[0] == 0 {
                return bytes;
            }
            bytes.push(byte[0]);
        }
        unreachable!()
    }

    /// A stack's argc, argument strings, environment strings and auxiliary vector.
    type Stack = (u64, Vec<Vec<u8>>, Vec<Vec<u8>>, Vec<(u64, u64)>);

    /// The stack at `sp`, read as a program reads it.
    fn read_stack(process: &Process<P>, sp: u64) -> Stack {
        let argc = word(process, sp);
        let mut at = sp + 8;
        let mut lists = [Vec::new(), Vec::new()];
        for list in &mut lists {
            loop {
                let pointer = word(process, at);
                at += 8;
                if pointer == 0 {
                    break;
                }
                list.push(string(process, pointer));
            }
        }
        let mut auxiliary = Vec::new();
        loop {
            let pair = (word(process, at), word(process, at + 8));
            at += 16;
            if pair.0 == auxv::NULL {
                break;
            }
            auxiliary.push(pair);
        }
        let [arguments, environment] = lists;
        (argc, arguments, environment, auxiliary)
    }

    fn value(auxiliary: &[(u64, u64)], kind: u64) -> u64 {
        let found: Vec<_> = auxiliary.iter().filter(|(k, _)| *k == kind).collect();
        assert_eq!(found.len(), 1, "AT_{kind} given once");
        found[0].1
    }

    #[test]
    fn an_executable_is_mapped_with_its_stack_laid_out_as_linux_does() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let before = free();
        {
            let image = linux_start_image::<P>();
            let (process, context) = load_image(&kernel, &image);
            let context = context.unwrap();
            let program = P::user_test_program();
            assert_eq!(
                context.pc(),
                TEST_PROGRAM_ADDRESS + program.linux_start as u64
            );
            let space = process.address_space();
            // Code with the headers before it, data and .bss across three pages.
            let text = TEST_PROGRAM_ADDRESS - FRAME_SIZE;
            for page in [text, TEST_PROGRAM_ADDRESS] {
                assert_eq!(space.translate(page).unwrap().1, Attributes::USER_TEXT);
            }
            let data_page = LINUX_START_DATA & !(FRAME_SIZE - 1);
            for page in 0..3 {
                let at = data_page + page * FRAME_SIZE;
                assert_eq!(space.translate(at).unwrap().1, Attributes::USER_READ_WRITE);
            }
            assert_eq!(space.translate(data_page + 3 * FRAME_SIZE), None);
            let mut code = alloc::vec![0; program.code.len()];
            process
                .read_user_memory(TEST_PROGRAM_ADDRESS, &mut code)
                .unwrap();
            assert_eq!(code, program.code);
            let mut data = alloc::vec![0xff; LINUX_START_DATA_SIZE as usize];
            process
                .read_user_memory(LINUX_START_DATA, &mut data)
                .unwrap();
            assert_eq!(&data[..16], LINUX_START_DATA_BYTES);
            assert!(data[16..].iter().all(|&byte| byte == 0));
            // The file header is mapped where the image has it, as a file mapping would.
            let mut magic = [0; 4];
            process.read_user_memory(text, &mut magic).unwrap();
            assert_eq!(&magic, b"\x7fELF");

            // The stack: 16-byte aligned, at the top of user space. Only the arguments' pages
            // are mapped; the whole reserve is one region, mapped as it is touched, and
            // nothing is below it.
            let sp = context.stack();
            assert_eq!(sp % 16, 0);
            assert!(USER_END - sp < FRAME_SIZE);
            let arguments_page = sp & !(FRAME_SIZE - 1);
            for page in (arguments_page..USER_END).step_by(FRAME_SIZE as usize) {
                assert_eq!(
                    space.translate(page).unwrap().1,
                    Attributes::USER_READ_WRITE
                );
            }
            assert_eq!(space.translate(arguments_page - FRAME_SIZE), None);
            let reserve = USER_END - STACK_RESERVE;
            assert_eq!(
                process.region(reserve),
                Some(Region {
                    start: reserve,
                    end: USER_END,
                    attributes: Some(Attributes::USER_READ_WRITE)
                })
            );
            assert_eq!(process.region(reserve - 1), None);
            assert_eq!(word(&process, USER_END - 8), 0, "the end marker");
            // A page the program has not touched reads as zero, mapped by the read.
            assert_eq!(word(&process, reserve), 0);
            assert!(space.translate(reserve).is_some());
            // The segments are regions too, as mapped.
            assert_eq!(
                process.region(text).and_then(|region| region.attributes),
                Some(Attributes::USER_TEXT)
            );

            let (argc, arguments, environment, auxiliary) = read_stack(&process, sp);
            assert_eq!(argc, 2);
            assert_eq!(
                arguments,
                LINUX_START_ARGUMENTS.map(|s| s.as_bytes().to_vec())
            );
            assert_eq!(
                environment,
                LINUX_START_ENVIRONMENT.map(|s| s.as_bytes().to_vec())
            );
            // Linux's order.
            let kinds: Vec<_> = auxiliary.iter().map(|(kind, _)| *kind).collect();
            assert_eq!(
                kinds,
                [
                    auxv::HWCAP,
                    auxv::PAGESZ,
                    auxv::CLKTCK,
                    auxv::PHDR,
                    auxv::PHENT,
                    auxv::PHNUM,
                    auxv::BASE,
                    auxv::FLAGS,
                    auxv::ENTRY,
                    auxv::UID,
                    auxv::EUID,
                    auxv::GID,
                    auxv::EGID,
                    auxv::SECURE,
                    auxv::RANDOM,
                    auxv::HWCAP2,
                    auxv::EXECFN,
                ]
            );
            assert_eq!(
                value(&auxiliary, auxv::PHDR),
                text + elf::HEADER_SIZE as u64
            );
            assert_eq!(value(&auxiliary, auxv::PHENT), 56);
            assert_eq!(value(&auxiliary, auxv::PHNUM), 4);
            assert_eq!(value(&auxiliary, auxv::PAGESZ), 4096);
            assert_eq!(value(&auxiliary, auxv::ENTRY), context.pc());
            assert_eq!(value(&auxiliary, auxv::BASE), 0);
            assert_eq!(value(&auxiliary, auxv::SECURE), 0);
            assert_eq!(value(&auxiliary, auxv::CLKTCK), 100);
            assert_eq!(string(&process, value(&auxiliary, auxv::EXECFN)), PATH);
            // The random bytes are on the stack, above the vector, and 16-byte aligned.
            let random = value(&auxiliary, auxv::RANDOM);
            assert!(random > sp && random + 16 <= USER_END && random.is_multiple_of(16));
            // AT_PHDR's table is the image's.
            let table = 4 * elf::PROGRAM_HEADER_SIZE;
            let mut headers = alloc::vec![0; table];
            process
                .read_user_memory(value(&auxiliary, auxv::PHDR), &mut headers)
                .unwrap();
            assert_eq!(headers, image[elf::HEADER_SIZE..elf::HEADER_SIZE + table]);
        }
        assert_eq!(free(), before);
    }

    #[test]
    fn a_position_independent_program_is_moved_to_the_pie_base() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        // The self-test's image, linked at 0 instead, with every address moved down.
        let mut image = linux_start_image::<P>();
        let text = TEST_PROGRAM_ADDRESS - FRAME_SIZE;
        image[16..18].copy_from_slice(&3u16.to_le_bytes());
        let entry = u64::from_le_bytes(image[24..32].try_into().unwrap()) - text;
        image[24..32].copy_from_slice(&entry.to_le_bytes());
        for index in 0..4 {
            let at = elf::HEADER_SIZE + index * elf::PROGRAM_HEADER_SIZE + 16;
            let virt = u64::from_le_bytes(image[at..at + 8].try_into().unwrap());
            image[at..at + 8].copy_from_slice(&virt.saturating_sub(text).to_le_bytes());
        }
        let (process, context) = load_image(&kernel, &image);
        let context = context.unwrap();
        let base = pie_base(USER_END, FRAME_SIZE);
        assert_eq!(base % (2 << 20), 0);
        assert_eq!(context.pc(), base + entry);
        let (_, _, _, auxiliary) = read_stack(&process, context.stack());
        assert_eq!(value(&auxiliary, auxv::ENTRY), base + entry);
        assert_eq!(
            value(&auxiliary, auxv::PHDR),
            base + elf::HEADER_SIZE as u64
        );
        assert_eq!(value(&auxiliary, auxv::BASE), 0);
        let mut data = [0; 16];
        process
            .read_user_memory(base + LINUX_START_DATA - text, &mut data)
            .unwrap();
        assert_eq!(&data, LINUX_START_DATA_BYTES);
    }

    #[test]
    fn a_segment_without_file_bytes_is_zero_pages_wherever_its_offset_points() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let good = linux_start_image::<P>();
        let image = with_header(&good, 2, |h| {
            h.offset = 1 << 40;
            h.file_size = 0;
        });
        let (process, context) = load_image(&kernel, &image);
        context.unwrap();
        let mut data = alloc::vec![0xff; LINUX_START_DATA_SIZE as usize];
        process
            .read_user_memory(LINUX_START_DATA & !(FRAME_SIZE - 1), &mut data)
            .unwrap();
        assert!(data.iter().all(|&byte| byte == 0));
    }

    #[test]
    fn the_pie_base_is_two_thirds_up_and_aligned() {
        use eggos_hal::paging::{aarch64::Aarch64, riscv64::Riscv64Sv39, x86_64::X86_64};
        assert_eq!(pie_base(X86_64::USER_END, FRAME_SIZE), 0x5555_5540_0000);
        assert_eq!(pie_base(Aarch64::USER_END, 64 << 10), 0xaaaa_aaa0_0000);
        assert_eq!(pie_base(Riscv64Sv39::USER_END, FRAME_SIZE), 0x2a_aaa0_0000);
        assert_eq!(pie_base(Riscv64Sv39::USER_END, 1 << 30), 0x2a_8000_0000);
    }

    /// Changes `image`'s program header `index` with `change`.
    fn with_header(image: &[u8], index: usize, change: impl FnOnce(&mut ProgramHeader)) -> Vec<u8> {
        let elf = Elf::parse(image, elf::machine::NATIVE).unwrap();
        let mut headers: Vec<_> = elf.program_headers().collect();
        change(&mut headers[index]);
        let mut image = image.to_vec();
        let start = elf::HEADER_SIZE;
        let rebuilt = elf::build::image(2, elf::machine::NATIVE, elf.entry(), &headers, &[]);
        image[start..start + headers.len() * elf::PROGRAM_HEADER_SIZE]
            .copy_from_slice(&rebuilt[start..start + headers.len() * elf::PROGRAM_HEADER_SIZE]);
        image
    }

    #[test]
    fn bad_programs_are_refused_with_linux_errors_before_anything_is_mapped() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let good = linux_start_image::<P>();
        // Headers: 0 PHDR, 1 text, 2 data, 3 GNU_STACK.
        let rw = segment::READ | segment::WRITE;
        let cases: [(Vec<u8>, Errno); 13] = [
            (good[..10].to_vec(), Errno::ENOEXEC),
            (
                with_header(&good, 2, |h| h.flags = rw | segment::EXECUTE),
                Errno::ENOEXEC,
            ),
            (with_header(&good, 2, |h| h.flags = 0), Errno::ENOEXEC),
            (
                with_header(&good, 3, |h| h.kind = segment::INTERP),
                Errno::ENOENT,
            ),
            (
                with_header(&good, 3, |h| h.flags = rw | segment::EXECUTE),
                Errno::ENOEXEC,
            ),
            (
                with_header(&good, 2, |h| h.file_size = h.memory_size + 1),
                Errno::EINVAL,
            ),
            (
                with_header(&good, 2, |h| {
                    h.virt = USER_END - STACK_RESERVE - FRAME_SIZE + 0x123
                }),
                Errno::EINVAL,
            ),
            (
                with_header(&good, 2, |h| h.virt = !0xfff | 0x123),
                Errno::EINVAL,
            ),
            (with_header(&good, 1, |h| h.virt = 0), Errno::EPERM),
            (with_header(&good, 2, |h| h.virt += 8), Errno::ENOEXEC),
            // The data segment moved onto the code's last page.
            (
                with_header(&good, 2, |h| h.virt = TEST_PROGRAM_ADDRESS + 0x123),
                Errno::ENOEXEC,
            ),
            (with_header(&good, 0, |h| h.virt += 8), Errno::ENOEXEC),
            (
                with_header(&good, 2, |h| {
                    h.file_size = good.len() as u64;
                    h.memory_size = good.len() as u64;
                }),
                Errno::ENOEXEC,
            ),
        ];
        for (index, (image, errno)) in cases.iter().enumerate() {
            let before = free();
            let (process, result) = load_image(&kernel, image);
            assert_eq!(
                result.map(|_| ()).map_err(ExecError::errno),
                Err(*errno),
                "case {index}"
            );
            // Nothing was mapped: only the process's root table is taken.
            assert_eq!(before - free(), 1, "case {index}");
            drop(process);
        }
        // A position-independent program whose second segment is so far above its first that
        // the move would wrap around to a valid-looking address.
        let mut pie = with_header(&good, 2, |h| h.virt = 0u64.wrapping_sub(0x10_0000) | 0x123);
        pie[16..18].copy_from_slice(&3u16.to_le_bytes());
        assert_eq!(
            load_image(&kernel, &pie)
                .1
                .map_err(ExecError::errno)
                .map(|_| ()),
            Err(Errno::EINVAL)
        );
        // An entry point past the end of user space.
        let mut far = good.clone();
        far[24..32].copy_from_slice(&USER_END.to_le_bytes());
        assert_eq!(
            load_image(&kernel, &far)
                .1
                .map_err(ExecError::errno)
                .map(|_| ()),
            Err(Errno::EINVAL)
        );
        // Program headers outside every segment.
        let outside = with_header(&good, 1, |h| {
            h.offset = FRAME_SIZE;
            h.virt = TEST_PROGRAM_ADDRESS;
            h.file_size -= FRAME_SIZE;
            h.memory_size -= FRAME_SIZE;
        });
        let outside = with_header(&outside, 0, |h| h.kind = 0);
        assert_eq!(
            load_image(&kernel, &outside).1.map(|_| ()),
            Err(ExecError::InvalidProgram(
                "the program headers are not in a segment"
            ))
        );
    }

    #[test]
    fn arguments_that_are_too_long_are_e2big() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let image = linux_start_image::<P>();
        let long = alloc::vec![b'x'; MAX_STRING_SIZE];
        let fits = alloc::vec![b'x'; MAX_STRING_SIZE - 1];
        let many: Vec<&[u8]> = (0..17).map(|_| &fits[..]).collect();
        for (arguments, environment) in [
            (&[&long[..]][..], &[][..]),
            (&[][..], &[&long[..]][..]),
            (&many[..], &[][..]),
        ] {
            // SAFETY: as in `load_image`.
            let mut process = unsafe { Process::<P>::new(&kernel) }.unwrap();
            assert_eq!(
                load(&mut process, &image, PATH, arguments, environment).map(|_| ()),
                Err(ExecError::ArgumentsTooLong)
            );
        }
        assert_eq!(ExecError::ArgumentsTooLong.errno(), Errno::E2BIG);
        assert_eq!(ExecError::OutOfMemory("").errno(), Errno::ENOMEM);
    }

    #[test]
    fn programs_do_not_get_the_same_random_bytes() {
        assert_ne!(random_bytes::<P>(), random_bytes::<P>());
    }
}
