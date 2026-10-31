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
//! Position-independent programs (static PIE, the default of Rust's musl targets, and the PIEs
//! dynamic linkers make) are loaded at [`pie_base`], where the program relocates itself. A
//! dynamically linked program names an interpreter (`PT_INTERP`, its dynamic loader: glibc's or
//! musl's `ld.so`), which [`load_program`] loads beside it at a base of its own, top-down below
//! the stack's guard gap as Linux places it, and starts instead (D167): `AT_BASE` tells the
//! loader where it is, `AT_ENTRY` and `AT_PHDR` where the program is, and the loader maps the
//! libraries itself with `mmap` of their files. There is no vDSO, so no `AT_SYSINFO_EHDR`: C
//! libraries make the system calls instead. [`load`] alone refuses a program that names an
//! interpreter with `ENOENT`, as Linux refuses one whose interpreter is missing; an
//! interpreter that is not an ELF program for this machine is `ELIBBAD`. A program loaded from
//! a file keeps it as its segments' backing ([`Program::file`]): a page `MADV_DONTNEED`
//! discards is read from the file again.
//!
//! [`execve`] and [`execveat`] start a program from a file (D103): they find it, check that it
//! is a regular file the caller may execute, copy the arguments and environment, and load it
//! into a new process with the caller's id ([`Process::new_image`]), all before the point of
//! no return, so a failure leaves the caller as it was. Then the caller's other threads end,
//! the signal actions go back to their defaults (ignored ones stay ignored), the alternate
//! stack goes, the close-on-exec descriptors close, and the calling thread carries on in the
//! new process as its only thread, with the process's id.

use super::credentials::{self, Credentials, Kind as IdKind};
use super::descriptors::Descriptors;
use super::files::{self, PathBuffer};
use super::stat::at;
use super::thread::LinuxThread;
use super::{Errno, futex, process_table, signals};
use crate::elf::{self, Elf, InvalidElf, Kind, ProgramHeader, segment};
use crate::process::{ImageLayout, Process};
use crate::regions::{FilePages, FileRange};
use crate::vfs::{self, Node, NodeKind};
use crate::{random, thread};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::ops::ControlFlow;
use core::sync::atomic::{AtomicBool, Ordering};
use eggos_hal::Platform;
use eggos_hal::fp::FpState;
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
    pub const PLATFORM: u64 = 15;
    pub const HWCAP: u64 = 16;
    pub const CLKTCK: u64 = 17;
    pub const SECURE: u64 = 23;
    pub const RANDOM: u64 = 25;
    pub const HWCAP2: u64 = 26;
    pub const EXECFN: u64 = 31;
    pub const MINSIGSTKSZ: u64 = 51;
}

/// The string `AT_PLATFORM` points to, as Linux names this machine: `x86_64` and `aarch64`;
/// RISC-V Linux gives none, so neither does EggOS there (D188).
pub const PLATFORM: Option<&[u8]> = match elf::machine::NATIVE {
    elf::machine::X86_64 => Some(b"x86_64"),
    elf::machine::AARCH64 => Some(b"aarch64"),
    _ => None,
};

/// Why a program could not be started, as `execve` reports it ([`Self::errno`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecError {
    /// The image is not a program the kernel can run: `ENOEXEC`. The text says why.
    InvalidProgram(&'static str),
    /// The program names an interpreter, and there is none to load: `ENOENT`.
    NoInterpreter,
    /// The program's interpreter is not one the kernel can run: not an ELF program for this
    /// machine, or one that names an interpreter itself: `ELIBBAD`. The text says why.
    BadInterpreter(&'static str),
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
            Self::BadInterpreter(_) => Errno::ELIBBAD,
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
/// Once [`randomize_layouts`] has run, each program goes up to [`PIE_SLOTS`] alignments lower,
/// and its stack up to [`STACK_SLOTS`] pages lower, at random (D201).
pub fn pie_base(user_end: u64, align: u64) -> u64 {
    (user_end / 3 * 2) & !(align.max(PIE_ALIGN) - 1)
}

/// How many alignments below [`pie_base`] a program may be placed at random.
pub const PIE_SLOTS: u64 = 256;
/// How many pages below the top of its reserve a program's stack may start at random.
pub const STACK_SLOTS: u64 = 256;

static RANDOMIZE_LAYOUTS: AtomicBool = AtomicBool::new(false);

/// Places the programs started from now on at random (D201): position-independent programs
/// below [`pie_base`] and every program's stack below the top of its reserve, so a program
/// cannot know where its code or its stack is. The kernel's own tests and benchmarks run
/// before this, on the fixed layout.
pub fn randomize_layouts() {
    RANDOMIZE_LAYOUTS.store(true, Ordering::Relaxed);
}

fn random_below<P: Platform>(limit: u64) -> u64 {
    if limit == 0 || !RANDOMIZE_LAYOUTS.load(Ordering::Relaxed) {
        return 0;
    }
    let mut bytes = [0u8; 8];
    random::fill::<P>(&mut bytes);
    u64::from_ne_bytes(bytes) % limit
}

/// A program image to load ([`load_program`]) and the file it was read from, if it was: the
/// segments' pages are read from that file again after `MADV_DONTNEED` discards them (D167).
#[derive(Clone)]
pub struct Program<'a> {
    pub image: &'a [u8],
    pub file: Option<Arc<dyn FilePages>>,
}

impl<'a> Program<'a> {
    /// `image`, with no file behind it.
    pub fn bytes(image: &'a [u8]) -> Self {
        Self { image, file: None }
    }
}

impl core::fmt::Debug for Program<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Program")
            .field("bytes", &self.image.len())
            .field("from_a_file", &self.file.is_some())
            .finish()
    }
}

/// Maps the program in `image` into `process` and lays out its stack with `path` (the name it
/// was started by, `AT_EXECFN`), `arguments` and `environment` (strings without their
/// terminating NUL; one inside a string ends it, as the program sees it). Returns the
/// registers it starts with. Nothing is mapped unless every check passed; if mapping then
/// runs out of memory, part of the program may be mapped, and the caller drops `process`,
/// which frees it. A program that names an interpreter is [`ExecError::NoInterpreter`]:
/// [`load_program`] loads one.
pub fn load<P: Platform>(
    process: &mut Process<P>,
    image: &[u8],
    path: &[u8],
    arguments: &[&[u8]],
    environment: &[&[u8]],
) -> Result<UserContext, ExecError> {
    load_program(
        process,
        Program::bytes(image),
        None,
        path,
        arguments,
        environment,
    )
}

/// [`load`] for `program` and, if it names an interpreter (a dynamically linked program),
/// `interpreter`, the program in the file it names, which is loaded beside it at a base of its
/// own, as high as it fits below the stack's guard gap, and started instead, with `AT_BASE`
/// its base (D167). Without
/// `interpreter`, a program that names one is [`ExecError::NoInterpreter`]; an interpreter
/// that is not an ELF program for this machine, or names an interpreter itself, is
/// [`ExecError::BadInterpreter`]. Both are checked in full before anything is mapped.
pub fn load_program<P: Platform>(
    process: &mut Process<P>,
    program: Program<'_>,
    interpreter: Option<Program<'_>>,
    path: &[u8],
    arguments: &[&[u8]],
    environment: &[&[u8]],
) -> Result<UserContext, ExecError> {
    let user_end = <P::PageTableFormat as Format>::USER_END;
    let elf = Elf::parse(program.image, elf::machine::NATIVE)?;
    let span = Span::of(&elf)?;
    let slide = match elf.kind() {
        Kind::Executable => 0,
        _ => random_below::<P>(PIE_SLOTS) * span.align.max(PIE_ALIGN),
    };
    let bias = load_bias(&elf, &span, user_end).wrapping_sub(slide);
    let layout = check_image(&elf, bias, user_end)?;
    let program_headers = layout.program_headers.ok_or(ExecError::InvalidProgram(
        "the program headers are not in a segment",
    ))?;
    if layout
        .named_program_headers
        .is_some_and(|named| named != program_headers)
    {
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
    let loader = match (layout.names_interpreter, interpreter) {
        (false, _) => None,
        (true, None) => return Err(ExecError::NoInterpreter),
        (true, Some(interpreter)) => Some(check_loader(interpreter, &layout, user_end)?),
    };
    let [hwcap, hwcap2] = P::linux_hardware_capabilities();
    let (user, group) = process
        .credentials_and_limits_mut()
        .credentials
        .after_exec()
        .map_err(|_| ExecError::OutOfMemory("the credentials"))?;
    let secure = u64::from(credentials::is_secure_execution(user, group));
    let auxiliary = [
        (auxv::MINSIGSTKSZ, signals::MIN_STACK_SIZE),
        (auxv::HWCAP, hwcap),
        (auxv::PAGESZ, FRAME_SIZE),
        (auxv::CLKTCK, CLOCK_TICKS_PER_SECOND),
        (auxv::PHDR, program_headers.wrapping_add(bias)),
        (auxv::PHENT, elf::PROGRAM_HEADER_SIZE as u64),
        (auxv::PHNUM, u64::from(elf.program_header_count())),
        (auxv::BASE, loader.as_ref().map_or(0, |loader| loader.bias)),
        (auxv::FLAGS, 0),
        (auxv::ENTRY, entry),
        (auxv::UID, u64::from(user.real)),
        (auxv::EUID, u64::from(user.effective)),
        (auxv::GID, u64::from(group.real)),
        (auxv::EGID, u64::from(group.effective)),
        (auxv::SECURE, secure),
        (auxv::RANDOM, 0),
        (auxv::HWCAP2, hwcap2),
        (auxv::EXECFN, 0),
    ];
    let stack_top = user_end - random_below::<P>(STACK_SLOTS) * FRAME_SIZE;
    let stack = InitialStack::new(
        stack_top,
        path,
        PLATFORM,
        arguments,
        environment,
        &auxiliary,
        &random_bytes::<P>(),
    )?;

    map_segments(process, &elf, program.file.as_ref(), bias)?;
    if let Some(loader) = &loader {
        map_segments(process, &loader.elf, loader.file.as_ref(), loader.bias)?;
    }
    process.set_program_break(layout.end);
    process
        .add_region(
            user_end - STACK_RESERVE,
            user_end,
            Attributes::USER_READ_WRITE,
        )
        .map_err(|_| ExecError::InvalidProgram("a segment in the stack's reserve"))?;
    process
        .write_user_memory(stack.start, &stack.bytes)
        .map_err(|_| ExecError::OutOfMemory("no frame for the stack"))?;
    process.set_image_layout(ImageLayout {
        executable: program.file.clone(),
        arguments: stack.arguments,
        environment: stack.environment,
        stack: (user_end - STACK_RESERVE, user_end),
        stack_pointer: stack.start,
    });
    let start = loader.as_ref().map_or(entry, |loader| loader.entry);
    Ok(UserContext::new(start, stack.start))
}

const CLOCK_TICKS_PER_SECOND: u64 = 100;

struct Span {
    lowest: u64,
    highest: u64,
    align: u64,
}

impl Span {
    fn of(elf: &Elf<'_>) -> Result<Self, ExecError> {
        let mut span = Self {
            lowest: u64::MAX,
            highest: 0,
            align: FRAME_SIZE,
        };
        for header in elf.program_headers() {
            if header.kind != segment::LOAD || header.memory_size == 0 {
                continue;
            }
            if header.align > 1 {
                if !header.align.is_power_of_two() || header.align > MAX_ALIGN {
                    return Err(ExecError::InvalidProgram(
                        "a segment alignment that is not a power of two up to 1 GiB",
                    ));
                }
                span.align = span.align.max(header.align);
            }
            span.lowest = span.lowest.min(header.virt);
            span.highest = span
                .highest
                .max(header.virt.saturating_add(header.memory_size));
        }
        Ok(span)
    }

    fn is_empty(&self) -> bool {
        self.lowest == u64::MAX
    }

    fn floor(&self) -> u64 {
        self.lowest & !(self.align - 1)
    }
}

/// What is added to every address in a program's headers: 0 for an executable, which runs
/// where it was linked; for a position-independent program, what moves it to [`pie_base`]
/// (its lowest segment there, or as far above as that segment is above its alignment).
fn load_bias(elf: &Elf<'_>, span: &Span, user_end: u64) -> u64 {
    if elf.kind() == Kind::Executable || span.is_empty() {
        return 0;
    }
    pie_base(user_end, span.align).wrapping_sub(span.floor())
}

fn interpreter_bias(
    elf: &Elf<'_>,
    span: &Span,
    user_end: u64,
    program: &Layout,
) -> Result<u64, ExecError> {
    if elf.kind() == Kind::Executable || span.is_empty() {
        return Ok(0);
    }
    let top = user_end - STACK_RESERVE - STACK_GUARD_GAP;
    let length = span
        .highest
        .checked_sub(span.floor())
        .and_then(|length| length.checked_next_multiple_of(FRAME_SIZE))
        .filter(|&length| length <= top)
        .ok_or(ExecError::BadInterpreter(
            "an interpreter larger than user space",
        ))?;
    let below = |end: u64| {
        end.checked_sub(length)
            .map(|start| start & !(span.align - 1))
            .filter(|&start| start >= MINIMUM_ADDRESS)
    };
    let no_room = ExecError::OutOfMemory("no room for the interpreter");
    let mut base = below(top).ok_or(no_room)?;
    if base < program.end && program.lowest < base + length {
        base = below(program.lowest).ok_or(no_room)?;
    }
    Ok(base.wrapping_sub(span.floor()))
}

struct Layout {
    lowest: u64,
    end: u64,
    program_headers: Option<u64>,
    named_program_headers: Option<u64>,
    names_interpreter: bool,
}

fn check_image(elf: &Elf<'_>, bias: u64, user_end: u64) -> Result<Layout, ExecError> {
    let image = elf.image();
    let table_offset = elf.program_header_offset();
    let table_size = u64::from(elf.program_header_count()) * elf::PROGRAM_HEADER_SIZE as u64;
    let mut layout = Layout {
        lowest: u64::MAX,
        end: 0,
        program_headers: None,
        named_program_headers: None,
        names_interpreter: false,
    };
    for header in elf.program_headers() {
        match header.kind {
            segment::INTERP => layout.names_interpreter = true,
            segment::PHDR => layout.named_program_headers = Some(header.virt),
            segment::GNU_STACK if header.flags & segment::EXECUTE != 0 => {
                return Err(ExecError::InvalidProgram(
                    "a program that asks for an executable stack",
                ));
            }
            segment::LOAD if header.memory_size > 0 => {
                let (start, end) = check_segment(image, &header, bias, user_end)?;
                let first_page = start & !(FRAME_SIZE - 1);
                if first_page < layout.end {
                    return Err(ExecError::InvalidProgram(
                        "segments that share a page or are out of order",
                    ));
                }
                layout.lowest = layout.lowest.min(first_page);
                layout.end = end.next_multiple_of(FRAME_SIZE);
                let within = table_offset
                    .checked_sub(header.offset)
                    .filter(|within| within.saturating_add(table_size) <= header.file_size);
                if let (None, Some(within)) = (layout.program_headers, within) {
                    layout.program_headers = Some(header.virt.wrapping_add(within));
                }
            }
            _ => {}
        }
    }
    if layout.end == 0 {
        return Err(ExecError::InvalidProgram("a program with nothing to load"));
    }
    Ok(layout)
}

struct Loader<'a> {
    elf: Elf<'a>,
    file: Option<Arc<dyn FilePages>>,
    bias: u64,
    entry: u64,
}

fn check_loader<'a>(
    interpreter: Program<'a>,
    program: &Layout,
    user_end: u64,
) -> Result<Loader<'a>, ExecError> {
    let bad = |error: ExecError| match error {
        ExecError::InvalidProgram(reason) => ExecError::BadInterpreter(reason),
        other => other,
    };
    let elf = Elf::parse(interpreter.image, elf::machine::NATIVE)
        .map_err(|InvalidElf(reason)| ExecError::BadInterpreter(reason))?;
    let span = Span::of(&elf).map_err(bad)?;
    let bias = interpreter_bias(&elf, &span, user_end, program)?;
    let layout = check_image(&elf, bias, user_end).map_err(bad)?;
    if layout.names_interpreter {
        return Err(ExecError::BadInterpreter(
            "an interpreter that names an interpreter",
        ));
    }
    if layout.lowest < program.end && program.lowest < layout.end {
        return Err(ExecError::BadInterpreter(
            "an interpreter where the program is",
        ));
    }
    let entry = elf.entry().wrapping_add(bias);
    if elf.entry() >= user_end || entry >= user_end {
        return Err(ExecError::InvalidSegment(
            "an interpreter's entry point outside user space",
        ));
    }
    Ok(Loader {
        elf,
        file: interpreter.file,
        bias,
        entry,
    })
}

fn map_segments<P: Platform>(
    process: &mut Process<P>,
    elf: &Elf<'_>,
    file: Option<&Arc<dyn FilePages>>,
    bias: u64,
) -> Result<(), ExecError> {
    for header in elf.program_headers() {
        if header.kind != segment::LOAD || header.memory_size == 0 {
            continue;
        }
        map_segment(process, elf.image(), &header, bias)?;
        if let Some(file) = file
            && header.file_size > 0
        {
            let start = header.virt.wrapping_add(bias);
            let first_page = start & !(FRAME_SIZE - 1);
            process
                .add_file_range(FileRange {
                    start: first_page,
                    end: (start + header.memory_size).next_multiple_of(FRAME_SIZE),
                    offset: header.offset - (start - first_page),
                    file_bytes_end: header.offset + header.file_size,
                    writable: true,
                    file: Arc::clone(file),
                })
                .map_err(|_| ExecError::OutOfMemory("no memory for a segment's file"))?;
        }
    }
    Ok(())
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
#[derive(Debug)]
struct InitialStack {
    /// The user address of the stack's lowest byte: the stack pointer.
    start: u64,
    /// Everything from `start` up to the top of the stack.
    bytes: Vec<u8>,
    arguments: (u64, u64),
    environment: (u64, u64),
}

impl InitialStack {
    /// The stack for a program whose stack ends at `top` (page-aligned). `auxiliary`'s
    /// `AT_RANDOM` and `AT_EXECFN` entries get their values here, since they point into the
    fn new(
        top: u64,
        path: &[u8],
        platform: Option<&[u8]>,
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
        let platform_entries = u64::from(platform.is_some());
        let platform_bytes = platform.map_or(0, |name| name.len() as u64 + 1);
        // argc, argv and its null, envp and its null, the vector and `AT_NULL`.
        let words = (arguments.len() as u64 + environment.len() as u64)
            .saturating_add(3 + 2 * (auxiliary.len() as u64 + platform_entries + 1));
        // The 8-byte end marker, the strings, then (each 16-byte aligned) the random bytes
        // and the pointers: at most 15 bytes of padding each.
        let most = [
            8,
            path.len() as u64 + 1,
            environment_bytes,
            argument_bytes,
            platform_bytes,
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
        let platform_at = arguments_at - platform_bytes;
        let random_at = (platform_at - 16) & !15;
        let start = (random_at - words * 8) & !15;
        let mut bytes = vec![0u8; (top - start) as usize];
        let mut put = |address: u64, data: &[u8]| {
            let at = (address - start) as usize;
            bytes[at..at + data.len()].copy_from_slice(data);
        };
        put(path_at, path);
        if let Some(name) = platform {
            put(platform_at, name);
        }
        put(random_at, random);
        let lists = [(arguments, arguments_at), (environment, environment_at)];
        for (list, mut address) in lists {
            for string in list {
                put(address, string);
                address += string.len() as u64 + 1;
            }
        }
        let mut word_at = start;
        let mut put_word = |word: u64| {
            put(word_at, &word.to_le_bytes());
            word_at += 8;
        };
        put_word(arguments.len() as u64);
        for (list, mut address) in lists {
            for string in list {
                put_word(address);
                address += string.len() as u64 + 1;
            }
            put_word(0);
        }
        let platform_entry = platform.map(|_| (auxv::PLATFORM, platform_at));
        for &(kind, value) in auxiliary
            .iter()
            .chain(&platform_entry)
            .chain(&[(auxv::NULL, 0)])
        {
            put_word(kind);
            put_word(match kind {
                auxv::RANDOM => random_at,
                auxv::EXECFN => path_at,
                _ => value,
            });
        }
        Ok(Self {
            start,
            bytes,
            arguments: (arguments_at, environment_at),
            environment: (environment_at, path_at),
        })
    }
}

/// `execve(path, argv, envp)`: [`execveat`] from the working directory with no flags.
pub fn execve<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
) -> ControlFlow<()> {
    let [path, arguments, environment, ..] = context.syscall_arguments();
    let request = Request {
        dirfd: at::FDCWD as u64,
        path,
        arguments,
        environment,
        flags: 0,
    };
    execute(process, thread, context, request)
}

/// `execveat(dirfd, path, argv, envp, flags)`: starts the program in the file `dirfd` and
/// `path` name, as `openat` finds it, in the calling process's place (see the module
/// documentation), with the arguments and environment the null-terminated pointer arrays
/// `argv` and `envp` list (a null array is an empty one; with no arguments the program gets
/// one empty argument, as Linux gives it). `AT_EMPTY_PATH` with an empty path starts the file
/// `dirfd` is open on. Errors, in Linux's order: `EINVAL` for another flag than
/// `AT_EMPTY_PATH` and `AT_SYMLINK_NOFOLLOW`; the path's (`EFAULT`, `ENAMETOOLONG`, `ENOENT`,
/// `ENOTDIR`, `EBADF`); `EACCES` for anything but a regular file and for a file without the
/// execute permission the caller's file system ids need (root needs one execute bit);
/// `EFAULT` and `E2BIG` for the arguments and environment; `ENOEXEC` for a file that is not
/// an ELF program for this machine. A file that starts with `#!` is a script (D170): its line
/// ([`script_line`]) names an interpreter, found as `open` finds it, which runs instead with
/// the arguments: the interpreter's path, the line's argument if it has one, the script's path
/// as `execve` was given it, then the script's own arguments after the first; an interpreter
/// may be a script itself, [`MAX_SCRIPT_DEPTH`] deep (`ELOOP` past it); a line without an
/// interpreter is `ENOEXEC`, and the interpreter's walk errors and `EACCES` are the call's;
/// for a dynamically linked program, its interpreter's walk errors (`ENOENT` for one that is not
/// there), `EACCES` for one the caller may not execute and `ELIBBAD` for one the kernel cannot
/// run (D167); `ENOMEM`. On success it does not return: the
/// new program starts.
pub fn execveat<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
) -> ControlFlow<()> {
    let [dirfd, path, arguments, environment, flags, _] = context.syscall_arguments();
    let request = Request {
        dirfd,
        path,
        arguments,
        environment,
        flags,
    };
    execute(process, thread, context, request)
}

const EXECVEAT_FLAGS: u64 = at::EMPTY_PATH | at::SYMLINK_NOFOLLOW;

#[derive(Clone, Copy)]
struct Request {
    dirfd: u64,
    path: u64,
    arguments: u64,
    environment: u64,
    flags: u64,
}

struct Prepared<P: Platform> {
    image: Arc<Process<P>>,
    context: UserContext,
    files: Option<Descriptors>,
    name: process_table::Name,
}

#[inline(never)]
fn execute<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    request: Request,
) -> ControlFlow<()> {
    match prepare(process, thread, request) {
        Ok(prepared) => {
            if !process.end_for_exec() {
                return ControlFlow::Break(());
            }
            let next = start(process, thread, context, prepared);
            super::fork::release_vfork_parent::<P>(process);
            let _ = signals::deliver(&next, thread, context);
            process.hand_over(next, context);
            ControlFlow::Break(())
        }
        Err(errno) => {
            context.set_syscall_result(errno.result());
            ControlFlow::Continue(())
        }
    }
}

fn prepare<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    request: Request,
) -> Result<Prepared<P>, Errno> {
    let mut buffer = PathBuffer::uninit();
    let path = files::read_path(process, request.path, &mut buffer)?;
    if path.is_empty() && request.flags & at::EMPTY_PATH == 0 {
        return Err(Errno::ENOENT);
    }
    if request.flags & !EXECVEAT_FLAGS != 0 {
        return Err(Errno::EINVAL);
    }
    let caller = process.id().get();
    let mut node = find(caller, thread, request.dirfd, path)?;
    let process_name = process_table::name_of(node.file_system().name(node.id()).as_bytes());
    let mut attributes = executable_attributes(process, node)?;
    let mut head = ProgramBytes::read(node, attributes.size, SIGNATURE_BYTES)?;
    let mut strings = Strings::new();
    let argument_count = strings.read_list(process, request.arguments)?;
    if argument_count == 0 {
        strings.push(&[])?;
    }
    let argument_count = argument_count.max(1);
    strings.read_list(process, request.environment)?;
    let mut name = ExecName::new();
    let name = name.of(request.dirfd, path)?;
    let list = strings.list()?;
    let (arguments, environment) = list.split_at(argument_count);
    let mut lead: Vec<Vec<u8>> = Vec::new();
    let mut scripts = 0;
    while head.bytes().starts_with(b"#!") {
        scripts += 1;
        if scripts > MAX_SCRIPT_DEPTH {
            return Err(Errno::ELOOP);
        }
        let line = ProgramBytes::read(node, attributes.size, SCRIPT_LINE_BYTES as u64)?;
        let script = script_line(line.bytes()).ok_or(Errno::ENOEXEC)?;
        let current = owned(lead.first().map_or(name, Vec::as_slice))?;
        let mut next = Vec::new();
        next.try_reserve_exact(lead.len() + 3)
            .map_err(|_| Errno::ENOMEM)?;
        next.push(owned(script.interpreter)?);
        if let Some(argument) = script.argument {
            next.push(owned(argument)?);
        }
        next.push(current);
        next.extend(lead.drain(..).skip(1));
        lead = next;
        node = find(caller, thread, at::FDCWD as u64, script.interpreter)?;
        attributes = executable_attributes(process, node)?;
        head = ProgramBytes::read(node, attributes.size, SIGNATURE_BYTES)?;
    }
    let mut scripted: Vec<&[u8]> = Vec::new();
    let arguments = match lead.is_empty() {
        true => arguments,
        false => {
            scripted
                .try_reserve_exact(lead.len() + arguments.len())
                .map_err(|_| Errno::ENOMEM)?;
            scripted.extend(lead.iter().map(Vec::as_slice));
            scripted.extend_from_slice(arguments.get(1..).unwrap_or(&[]));
            &scripted[..]
        }
    };
    let (image, image_node, interpreter) = match interpreter_for(head.bytes()) {
        Some(interpreter) => {
            let (interpreter, layer, image) = read_interpreter(process, thread, interpreter)?;
            (image, layer, Some(interpreter))
        }
        None => (
            ProgramBytes::read(node, attributes.size, u64::MAX)?,
            node,
            None,
        ),
    };
    let dynamic_loader = read_dynamic_loader(process, thread, image.bytes())?;
    let mut interpreted = Vec::new();
    let (name, arguments) = match interpreter {
        Some(interpreter) => {
            interpreted
                .try_reserve_exact(arguments.len() + 2)
                .map_err(|_| Errno::ENOMEM)?;
            interpreted.extend([interpreter.path, name]);
            interpreted.extend_from_slice(arguments);
            (interpreter.path, &interpreted[..])
        }
        None => (name, arguments),
    };
    let program = Program {
        image: image.bytes(),
        file: Some(Arc::new(vfs::HeldNode::new(image_node))),
    };
    let dynamic_loader = dynamic_loader.as_ref().map(|(loader, bytes)| Program {
        image: bytes.bytes(),
        file: Some(Arc::new(vfs::HeldNode::new(*loader)) as Arc<dyn FilePages>),
    });
    let files = match thread.files().is_shared() {
        true => Some(thread.files().copy()?),
        false => None,
    };
    let mut new = process.new_image().map_err(|_| Errno::ENOMEM)?;
    let context = load_program(
        &mut new,
        program,
        dynamic_loader,
        name,
        arguments,
        environment,
    )
    .map_err(ExecError::errno)?;
    Ok(Prepared {
        image: Arc::new(new),
        context,
        files,
        name: process_name,
    })
}

/// A program format the kernel does not run itself but starts through an interpreter program
/// of an optional component, as Linux's `binfmt_misc` does (D118): a file whose first bytes
/// match `signature` runs as the program at `path` with the arguments `path`, the file's name
/// as `execve` was given it, then the file's own arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interpreter {
    pub signature: Signature,
    pub path: &'static [u8],
}

/// How an [`Interpreter`]'s files start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signature {
    /// These bytes.
    Prefix(&'static [u8]),
    /// A Mach-O fat (universal) header: its magic, then an architecture count from 1 to
    /// [`MAX_FAT_ARCHITECTURES`]. A Java class file starts with the same magic, followed by a
    /// version of at least 45, so it does not match (D120).
    MachOFat,
}

/// The most architectures a fat file may list for the macOS layer to start it.
pub const MAX_FAT_ARCHITECTURES: u32 = 32;

impl Signature {
    fn matches(self, image: &[u8]) -> bool {
        match self {
            Self::Prefix(prefix) => image.starts_with(prefix),
            Self::MachOFat => {
                let word = |at: usize| {
                    image
                        .get(at..at + 4)
                        .map(|bytes| u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
                };
                matches!(word(0), Some(0xcafe_babe | 0xcafe_babf))
                    && word(4).is_some_and(|count| (1..=MAX_FAT_ARCHITECTURES).contains(&count))
            }
        }
    }
}

/// The Windows layer (D118): PE executables (`MZ`), on x86_64 (x64 programs) and aarch64
/// (ARM64 programs, D124).
pub const WINDOWS_LAYER: Interpreter = Interpreter {
    signature: Signature::Prefix(b"MZ"),
    path: b"/usr/lib/eggos/windows",
};

/// The macOS layer (D120): 64-bit Mach-O executables (`0xfeedfacf`, little-endian), on
/// aarch64 and x86_64.
pub const MACOS_LAYER: Interpreter = Interpreter {
    signature: Signature::Prefix(&[0xcf, 0xfa, 0xed, 0xfe]),
    path: MACOS_LAYER_PATH,
};

/// The macOS layer for fat (universal) files, which hold one executable per CPU.
pub const MACOS_FAT_LAYER: Interpreter = Interpreter {
    signature: Signature::MachOFat,
    path: MACOS_LAYER_PATH,
};

const MACOS_LAYER_PATH: &[u8] = b"/usr/lib/eggos/macos";

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
const INTERPRETERS: &[Interpreter] = &[WINDOWS_LAYER, MACOS_LAYER, MACOS_FAT_LAYER];
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
const INTERPRETERS: &[Interpreter] = &[];
const ANY_INTERPRETER: bool = !INTERPRETERS.is_empty();

/// The interpreter that starts `image` on this architecture, if any.
pub fn interpreter_for(image: &[u8]) -> Option<Interpreter> {
    INTERPRETERS
        .iter()
        .find(|interpreter| interpreter.signature.matches(image))
        .copied()
}

fn read_interpreter<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    interpreter: Interpreter,
) -> Result<(Interpreter, Node, ProgramBytes), Errno> {
    if !ANY_INTERPRETER {
        return Err(Errno::ENOEXEC);
    }
    let not_installed = |_| Errno::ENOEXEC;
    let node = find(
        process.id().get(),
        thread,
        at::FDCWD as u64,
        interpreter.path,
    )
    .map_err(not_installed)?;
    let attributes = node.attributes();
    let may_execute = {
        let credentials = process.credentials_and_limits().lock();
        may_execute(&attributes, &credentials.credentials)
    };
    if attributes.kind != NodeKind::Regular || !may_execute {
        return Err(Errno::ENOEXEC);
    }
    Ok((
        interpreter,
        node,
        ProgramBytes::read(node, attributes.size, u64::MAX)?,
    ))
}

fn read_dynamic_loader<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    image: &[u8],
) -> Result<Option<(Node, ProgramBytes)>, Errno> {
    let Ok(elf) = Elf::parse(image, elf::machine::NATIVE) else {
        return Ok(None);
    };
    let path = match elf.interpreter() {
        Ok(Some(path)) => path,
        Ok(None) => return Ok(None),
        Err(_) => return Err(Errno::ENOEXEC),
    };
    let node = find(process.id().get(), thread, at::FDCWD as u64, path)?;
    let attributes = node.attributes();
    let may_execute = {
        let credentials = process.credentials_and_limits().lock();
        may_execute(&attributes, &credentials.credentials)
    };
    if attributes.kind != NodeKind::Regular || !may_execute {
        return Err(Errno::EACCES);
    }
    Ok(Some((
        node,
        ProgramBytes::read(node, attributes.size, u64::MAX)?,
    )))
}

const SIGNATURE_BYTES: u64 = 64;

/// How many bytes of a script its `#!` line is read from (Linux's `BINPRM_BUF_SIZE`).
pub const SCRIPT_LINE_BYTES: usize = 256;

/// How many scripts deep `#!` lines may lead before the program (Linux's
/// `BINPRM_MAX_RECURSION`); one more is `ELOOP`.
pub const MAX_SCRIPT_DEPTH: u32 = 4;

/// A script's `#!` line: the interpreter's path and its one optional argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScriptLine<'a> {
    /// The interpreter's path, found as `open` finds it.
    pub interpreter: &'a [u8],
    /// The rest of the line, blanks around it trimmed, if any is left.
    pub argument: Option<&'a [u8]>,
}

/// The `#!` line `head`, a script's first [`SCRIPT_LINE_BYTES`] bytes (or all of a shorter
/// file), starts with, as Linux reads it (D170): after `#!` and any spaces and tabs, the
/// interpreter runs to the next space, tab, newline or NUL; the rest of the line, spaces and
/// tabs trimmed at both ends, is one argument. `None` for no `#!`, no interpreter, or a line
/// whose interpreter runs past the bytes read.
pub fn script_line(head: &[u8]) -> Option<ScriptLine<'_>> {
    let rest = head.strip_prefix(b"#!")?;
    let end = rest.iter().position(|&byte| byte == b'\n' || byte == 0);
    let line = &rest[..end.unwrap_or(rest.len())];
    let blank = |byte: &u8| *byte == b' ' || *byte == b'\t';
    let line = &line[line
        .iter()
        .position(|byte| !blank(byte))
        .unwrap_or(line.len())..];
    let interpreter_end = line.iter().position(blank).unwrap_or(line.len());
    let cut = end.is_none() && head.len() >= SCRIPT_LINE_BYTES;
    if interpreter_end == 0 || (cut && interpreter_end == line.len()) {
        return None;
    }
    let (interpreter, rest) = line.split_at(interpreter_end);
    let start = rest
        .iter()
        .position(|byte| !blank(byte))
        .unwrap_or(rest.len());
    let stop = rest
        .iter()
        .rposition(|byte| !blank(byte))
        .map_or(0, |at| at + 1);
    Some(ScriptLine {
        interpreter,
        argument: (start < stop).then(|| &rest[start..stop]),
    })
}

fn owned(bytes: &[u8]) -> Result<Vec<u8>, Errno> {
    let mut copy = Vec::new();
    copy.try_reserve_exact(bytes.len())
        .map_err(|_| Errno::ENOMEM)?;
    copy.extend_from_slice(bytes);
    Ok(copy)
}

fn executable_attributes<P: Platform>(
    process: &Process<P>,
    node: Node,
) -> Result<vfs::Attributes, Errno> {
    let attributes = node.attributes();
    let may_execute = {
        let credentials = process.credentials_and_limits().lock();
        may_execute(&attributes, &credentials.credentials)
    };
    if attributes.kind != NodeKind::Regular || !may_execute {
        return Err(Errno::EACCES);
    }
    Ok(attributes)
}

/// A program's bytes: lent by a file system that holds them in memory, else copied.
pub(crate) enum ProgramBytes {
    Lent(&'static [u8]),
    Copied(Vec<u8>),
}

impl ProgramBytes {
    /// The first `at_most` of the `size` bytes of `node`.
    pub(crate) fn read(node: Node, size: u64, at_most: u64) -> Result<Self, Errno> {
        let file_system = node.file_system();
        if !file_system.copies_contents() {
            return Ok(Self::Lent(node.read(0, size as usize)));
        }
        let length = usize::try_from(size.min(at_most)).map_err(|_| Errno::ENOMEM)?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(length).map_err(|_| Errno::ENOMEM)?;
        bytes.resize(length, 0);
        let mut filled = 0;
        while filled < length {
            match file_system.read_into(node.id(), filled as u64, &mut bytes[filled..]) {
                Ok(0) => break,
                Ok(read) => filled += read,
                Err(_) => return Err(Errno::EIO),
            }
        }
        bytes.truncate(filled);
        Ok(Self::Copied(bytes))
    }

    /// The bytes.
    pub(crate) fn bytes(&self) -> &[u8] {
        match self {
            Self::Lent(bytes) => bytes,
            Self::Copied(bytes) => bytes,
        }
    }
}

fn find(caller: u32, thread: &LinuxThread, dirfd: u64, path: &[u8]) -> Result<Node, Errno> {
    if path.is_empty() {
        let file = thread.files().file(dirfd)?;
        return file.kind().node().ok_or(Errno::EACCES);
    }
    let (root, start) = files::starting_points(thread, dirfd, path)?;
    vfs::resolve_as(root, start, path, caller, true).map_err(files::walk_error)
}

fn may_execute(attributes: &vfs::Attributes, credentials: &Credentials) -> bool {
    const EXECUTE: u32 = 0o1;
    let user = credentials.ids(IdKind::User).filesystem;
    let group = credentials.ids(IdKind::Group).filesystem;
    let bits = attributes.permissions;
    let class = if user == 0 {
        bits | bits >> 3 | bits >> 6
    } else if user == attributes.owner {
        bits >> 6
    } else if group == attributes.group || credentials.groups().contains(&attributes.group) {
        bits >> 3
    } else {
        bits
    };
    class & EXECUTE != 0
}

fn start<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    prepared: Prepared<P>,
) -> Arc<Process<P>> {
    let Prepared {
        image,
        context: started,
        files,
        name,
    } = prepared;
    let id = process.id().get();
    let signals = process.signals().lock().take_for_exec(thread.id(), id);
    let sigchld_ignored = signals
        .action(super::signal::SIGCHLD)
        .is_some_and(|action| action.handler == signals::IGNORE);
    *image.signals().lock() = signals;
    if process.threads() > 1 {
        futex::wake_space::<P>(process.futex_space());
        crate::console::wake_input_waiters::<P>();
        super::wait::wake_child_waiters::<P>(id);
    }
    let robust_list = thread.robust_list();
    if robust_list != 0 {
        futex::release_robust_list(process, thread.id(), robust_list);
    }
    super::clear_child_tid_if_memory_shared::<P>(process, thread);
    thread.start_image(id);
    if let Some(files) = files {
        thread.set_files(files);
    }
    let mut from = 0;
    loop {
        let closed = thread.files().lock().remove_first_closed_on_exec(from);
        let Some((fd, descriptor)) = closed else {
            break;
        };
        super::file_locks::descriptor_closed(id, &descriptor.file);
        drop(descriptor);
        from = fd + 1;
    }
    thread::with_running_fp::<P, _>(|state| *state = FpState::new());
    image.add_thread();
    super::wait::record_running(&image, thread, |table, id| {
        table.mark_executed(id);
        table.set_name(id, name);
        table.set_no_zombie_children(id, sigchld_ignored);
        table.set_no_stop_notices(id, false);
    });
    *context = started;
    image
}

struct Strings {
    bytes: Vec<u8>,
    ends: Vec<usize>,
    size: u64,
}

impl Strings {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            ends: Vec::new(),
            size: 0,
        }
    }

    fn read_list<P: Platform>(&mut self, process: &Process<P>, list: u64) -> Result<usize, Errno> {
        if list == 0 {
            return Ok(0);
        }
        let mut count = 0;
        loop {
            let at = (count as u64)
                .checked_mul(8)
                .and_then(|offset| list.checked_add(offset))
                .ok_or(Errno::EFAULT)?;
            let mut pointer = [0; 8];
            process
                .read_user_memory(at, &mut pointer)
                .map_err(|_| Errno::EFAULT)?;
            let pointer = u64::from_ne_bytes(pointer);
            if pointer == 0 {
                return Ok(count);
            }
            self.read_string(process, pointer)?;
            count += 1;
        }
    }

    fn read_string<P: Platform>(
        &mut self,
        process: &Process<P>,
        address: u64,
    ) -> Result<(), Errno> {
        const CHUNK: usize = 64;
        let start = self.bytes.len();
        let mut chunk = [0u8; CHUNK];
        let mut at = address;
        loop {
            let length = CHUNK.min((FRAME_SIZE - at % FRAME_SIZE) as usize);
            let chunk = &mut chunk[..length];
            process
                .read_user_memory(at, chunk)
                .map_err(|_| Errno::EFAULT)?;
            let end = chunk.iter().position(|&byte| byte == 0);
            let taken = &chunk[..end.unwrap_or(length)];
            if self.bytes.len() - start + taken.len() >= MAX_STRING_SIZE {
                return Err(Errno::E2BIG);
            }
            self.charge(taken.len() as u64)?;
            self.bytes
                .try_reserve(taken.len())
                .map_err(|_| Errno::ENOMEM)?;
            self.bytes.extend_from_slice(taken);
            if end.is_some() {
                break;
            }
            at = at.checked_add(length as u64).ok_or(Errno::EFAULT)?;
        }
        self.finish_string()
    }

    fn push(&mut self, string: &[u8]) -> Result<(), Errno> {
        self.charge(string.len() as u64)?;
        self.bytes
            .try_reserve(string.len())
            .map_err(|_| Errno::ENOMEM)?;
        self.bytes.extend_from_slice(string);
        self.finish_string()
    }

    fn finish_string(&mut self) -> Result<(), Errno> {
        self.charge(1 + 8)?;
        self.ends.try_reserve(1).map_err(|_| Errno::ENOMEM)?;
        self.ends.push(self.bytes.len());
        Ok(())
    }

    fn charge(&mut self, bytes: u64) -> Result<(), Errno> {
        self.size += bytes;
        match self.size > MAX_ARGUMENTS_SIZE {
            true => Err(Errno::E2BIG),
            false => Ok(()),
        }
    }

    fn list(&self) -> Result<Vec<&[u8]>, Errno> {
        let mut list = Vec::new();
        list.try_reserve_exact(self.ends.len())
            .map_err(|_| Errno::ENOMEM)?;
        let mut start = 0;
        for &end in &self.ends {
            list.push(&self.bytes[start..end]);
            start = end;
        }
        Ok(list)
    }
}

struct ExecName {
    bytes: Vec<u8>,
}

impl ExecName {
    fn new() -> Self {
        Self { bytes: Vec::new() }
    }

    fn of<'a>(&'a mut self, dirfd: u64, path: &'a [u8]) -> Result<&'a [u8], Errno> {
        if path.first() == Some(&b'/') || dirfd as u32 as i32 == at::FDCWD {
            return Ok(path);
        }
        let mut digits = [0u8; 10];
        let mut fd = dirfd as u32;
        let mut first = digits.len();
        loop {
            first -= 1;
            digits[first] = b'0' + (fd % 10) as u8;
            fd /= 10;
            if fd == 0 {
                break;
            }
        }
        let parts: [&[u8]; 4] = match path.is_empty() {
            true => [b"/dev/fd/", &digits[first..], b"", b""],
            false => [b"/dev/fd/", &digits[first..], b"/", path],
        };
        let length = parts.iter().map(|part| part.len()).sum();
        self.bytes
            .try_reserve_exact(length)
            .map_err(|_| Errno::ENOMEM)?;
        for part in parts {
            self.bytes.extend_from_slice(part);
        }
        Ok(&self.bytes)
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

    #[test]
    fn a_program_on_a_file_system_that_copies_is_read_whole_or_by_its_head() {
        use crate::vfs::tmpfs::{Budget, Tmpfs};
        use crate::vfs::{FileSystem, WriteAt};
        let budget = Box::leak(Box::new(Budget::new(1 << 20)));
        let tmpfs = Box::leak(Box::new(Tmpfs::new(budget, (0, 30), 0o755).unwrap()));
        let contents: Vec<u8> = (0..10_000u32).map(|index| index as u8).collect();
        let file = tmpfs
            .create(tmpfs.root(), b"program", NodeKind::Regular, 0o755, 0)
            .unwrap();
        tmpfs.write(file, WriteAt::Offset(0), &contents, 0).unwrap();
        let node = Node::root_of(tmpfs).lookup(b"program").unwrap();
        let size = contents.len() as u64;
        let head = ProgramBytes::read(node, size, SIGNATURE_BYTES).unwrap();
        assert_eq!(head.bytes(), &contents[..SIGNATURE_BYTES as usize]);
        let whole = ProgramBytes::read(node, size, u64::MAX).unwrap();
        assert_eq!(whole.bytes(), &contents[..]);
        let short = ProgramBytes::read(node, 10, u64::MAX).unwrap();
        assert_eq!(short.bytes(), &contents[..10]);
    }

    #[test]
    fn arguments_and_environment_are_held_to_a_quarter_of_the_stack_with_their_pointers() {
        let mut strings = Strings::new();
        let string = [b'a'; 1000];
        let each = string.len() as u64 + 1 + 8;
        for _ in 0..MAX_ARGUMENTS_SIZE / each {
            strings.push(&string).unwrap();
        }
        assert_eq!(strings.push(&string), Err(Errno::E2BIG));
        let mut strings = Strings::new();
        strings.push(b"first").unwrap();
        strings.push(b"").unwrap();
        strings.push(b"third").unwrap();
        assert_eq!(strings.list().unwrap(), [&b"first"[..], b"", b"third"]);
    }

    #[test]
    fn a_program_started_relative_to_a_descriptor_is_named_through_dev_fd() {
        let mut name = ExecName::new();
        assert_eq!(name.of(7, b"/bin/sh").unwrap(), b"/bin/sh");
        let mut name = ExecName::new();
        assert_eq!(name.of(at::FDCWD as u64, b"sh").unwrap(), b"sh");
        let mut name = ExecName::new();
        assert_eq!(name.of(12, b"sh").unwrap(), b"/dev/fd/12/sh");
        let mut name = ExecName::new();
        assert_eq!(name.of(0, b"").unwrap(), b"/dev/fd/0");
    }

    #[test]
    fn execute_permission_follows_the_file_system_ids_class_and_root_needs_one_bit() {
        let file = |permissions| vfs::Attributes {
            kind: NodeKind::Regular,
            permissions,
            file_id: 1,
            links: 1,
            owner: 1000,
            group: 100,
            size: 0,
            modified: 0,
        };
        let root = Credentials::new();
        assert!(may_execute(&file(0o001), &root));
        assert!(!may_execute(&file(0o644), &root));
        let mut user = Credentials::new();
        user.set_ids(IdKind::User, credentials::Ids::all(1000))
            .unwrap();
        user.set_ids(IdKind::Group, credentials::Ids::all(5))
            .unwrap();
        assert!(may_execute(&file(0o700), &user));
        assert!(!may_execute(&file(0o077), &user));
        let mut other = Credentials::new();
        other
            .set_ids(IdKind::User, credentials::Ids::all(2000))
            .unwrap();
        other
            .set_ids(IdKind::Group, credentials::Ids::all(100))
            .unwrap();
        assert!(may_execute(&file(0o010), &other));
        assert!(!may_execute(&file(0o701), &other));
        other
            .set_ids(IdKind::Group, credentials::Ids::all(5))
            .unwrap();
        assert!(may_execute(&file(0o001), &other));
    }

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
            let mut linux_order = alloc::vec![
                auxv::MINSIGSTKSZ,
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
            ];
            if let Some(platform) = PLATFORM {
                linux_order.push(auxv::PLATFORM);
                assert_eq!(
                    string(&process, value(&auxiliary, auxv::PLATFORM)),
                    platform
                );
            }
            assert_eq!(kinds, linux_order);
            assert_eq!(
                value(&auxiliary, auxv::MINSIGSTKSZ),
                signals::MIN_STACK_SIZE
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
    fn the_platform_s_name_lies_between_the_random_bytes_and_the_arguments() {
        let arguments: [&[u8]; 1] = [b"x"];
        let auxiliary = [(auxv::RANDOM, 0)];
        for platform in [None, Some(&b"aarch64"[..])] {
            let stack = InitialStack::new(
                1 << 20,
                b"/bin/x",
                platform,
                &arguments,
                &[],
                &auxiliary,
                &[7; 16],
            )
            .unwrap();
            let word = |at: u64| {
                let at = (at - stack.start) as usize;
                u64::from_le_bytes(stack.bytes[at..at + 8].try_into().unwrap())
            };
            let vector = stack.start + 4 * 8;
            let pairs: Vec<(u64, u64)> = (0u64..)
                .map(|index| (word(vector + 16 * index), word(vector + 16 * index + 8)))
                .take_while(|&(kind, _)| kind != auxv::NULL)
                .collect();
            assert_eq!(pairs[0].0, auxv::RANDOM);
            let random_at = pairs[0].1;
            match platform {
                None => assert_eq!(pairs.len(), 1),
                Some(name) => {
                    assert_eq!(pairs.len(), 2);
                    let (kind, platform_at) = pairs[1];
                    assert_eq!(kind, auxv::PLATFORM);
                    let at = (platform_at - stack.start) as usize;
                    assert_eq!(&stack.bytes[at..at + name.len() + 1], b"aarch64\0");
                    assert!(platform_at >= random_at + 16);
                    assert_eq!(platform_at + name.len() as u64 + 1, stack.arguments.0);
                }
            }
        }
    }

    fn position_independent() -> (Vec<u8>, u64) {
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
        (image, entry)
    }

    #[test]
    fn a_position_independent_program_is_moved_to_the_pie_base() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let text = TEST_PROGRAM_ADDRESS - FRAME_SIZE;
        let (image, entry) = position_independent();
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

    fn load_dynamic(
        kernel: &PageTables<<P as Platform>::PageTableFormat>,
        program: Program<'_>,
        interpreter: Option<Program<'_>>,
    ) -> (Process<P>, Result<UserContext, ExecError>) {
        // SAFETY: test tables, never active; the test platform's switch only records roots.
        let mut process = unsafe { Process::<P>::new(kernel) }.unwrap();
        let arguments = LINUX_START_ARGUMENTS.map(str::as_bytes);
        let environment = LINUX_START_ENVIRONMENT.map(str::as_bytes);
        let result = load_program(
            &mut process,
            program,
            interpreter,
            PATH,
            &arguments,
            &environment,
        );
        (process, result)
    }

    fn naming_an_interpreter(image: &[u8]) -> Vec<u8> {
        with_header(image, 3, |h| h.kind = segment::INTERP)
    }

    #[test]
    fn a_dynamically_linked_program_starts_in_its_interpreter_at_its_own_base() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let before = free();
        {
            let program = naming_an_interpreter(&linux_start_image::<P>());
            let (loader, loader_entry) = position_independent();
            let (process, context) = load_dynamic(
                &kernel,
                Program::bytes(&program),
                Some(Program::bytes(&loader)),
            );
            let context = context.unwrap();
            let elf = Elf::parse(&loader, elf::machine::NATIVE).unwrap();
            let span = Span::of(&elf).unwrap();
            let length = (span.highest - span.floor()).next_multiple_of(FRAME_SIZE);
            let top = USER_END - STACK_RESERVE - STACK_GUARD_GAP;
            let base = (top - length) & !(span.align - 1);
            assert_eq!(context.pc(), base + loader_entry);
            let (argc, _, _, auxiliary) = read_stack(&process, context.stack());
            assert_eq!(argc, 2);
            assert_eq!(value(&auxiliary, auxv::BASE), base);
            let text = TEST_PROGRAM_ADDRESS - FRAME_SIZE;
            assert_eq!(
                value(&auxiliary, auxv::ENTRY),
                TEST_PROGRAM_ADDRESS + P::user_test_program().linux_start as u64
            );
            assert_eq!(
                value(&auxiliary, auxv::PHDR),
                text + elf::HEADER_SIZE as u64
            );
            let mut data = [0; 16];
            process
                .read_user_memory(base + LINUX_START_DATA - text, &mut data)
                .unwrap();
            assert_eq!(&data, LINUX_START_DATA_BYTES);
            process
                .read_user_memory(LINUX_START_DATA, &mut data)
                .unwrap();
            assert_eq!(&data, LINUX_START_DATA_BYTES);
            assert_eq!(
                process.region(base).and_then(|region| region.attributes),
                Some(Attributes::USER_TEXT)
            );
        }
        assert_eq!(free(), before);
    }

    #[test]
    fn interpreters_the_kernel_cannot_run_are_elibbad_before_anything_is_mapped() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let program = naming_an_interpreter(&linux_start_image::<P>());
        let (loader, _) = position_independent();
        let naming = naming_an_interpreter(&loader);
        let mut other_machine = loader.clone();
        let other = if elf::machine::NATIVE == elf::machine::X86_64 {
            elf::machine::AARCH64
        } else {
            elf::machine::X86_64
        };
        other_machine[18..20].copy_from_slice(&other.to_le_bytes());
        for (bad, why) in [
            (&loader[..10], "not an ELF image"),
            (&naming[..], "an interpreter that names an interpreter"),
            (&other_machine[..], "an ELF image for another machine"),
        ] {
            let before = free();
            let (process, result) =
                load_dynamic(&kernel, Program::bytes(&program), Some(Program::bytes(bad)));
            assert_eq!(result.map(|_| ()), Err(ExecError::BadInterpreter(why)));
            assert_eq!(before - free(), 1, "{why}");
            drop(process);
        }
        assert_eq!(ExecError::BadInterpreter("").errno(), Errno::ELIBBAD);
        let (_, result) = load_dynamic(&kernel, Program::bytes(&program), None);
        assert_eq!(result.map(|_| ()), Err(ExecError::NoInterpreter));
    }

    #[test]
    fn a_discarded_page_of_a_segment_is_read_from_the_program_s_file_again() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let image = linux_start_image::<P>();
        let file: Arc<dyn FilePages> = Arc::new(crate::regions::BytesFile(image.clone()));
        let program = Program {
            image: &image,
            file: Some(file),
        };
        let (process, context) = load_dynamic(&kernel, program, None);
        context.unwrap();
        process
            .write_user_memory(LINUX_START_DATA, b"overwritten data")
            .unwrap();
        let page = LINUX_START_DATA & !(FRAME_SIZE - 1);
        assert!(process.maps_file_in(page, page + FRAME_SIZE));
        process.discard_pages(page, page + FRAME_SIZE).unwrap();
        assert_eq!(process.address_space().translate(page), None);
        let mut data = [0; 16];
        process
            .read_user_memory(LINUX_START_DATA, &mut data)
            .unwrap();
        assert_eq!(&data, LINUX_START_DATA_BYTES);
        let bss = page + FRAME_SIZE;
        process.write_user_memory(bss, b"x").unwrap();
        process.discard_pages(bss, bss + FRAME_SIZE).unwrap();
        let mut byte = [0xff];
        process.read_user_memory(bss, &mut byte).unwrap();
        assert_eq!(byte, [0], "the segment's .bss reads zeros, not the file");
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
    fn windows_programs_start_through_the_windows_layer_on_x86_64_and_aarch64() {
        let windows = interpreter_for(b"MZ\x90\0rest of the file");
        match cfg!(any(target_arch = "x86_64", target_arch = "aarch64")) {
            true => assert_eq!(windows, Some(WINDOWS_LAYER)),
            false => assert_eq!(windows, None),
        }
        assert_eq!(WINDOWS_LAYER.path, b"/usr/lib/eggos/windows");
        for image in [&b"\x7fELF"[..], b"#!/bin/eggsh", b"M", b"", b"ZM"] {
            assert_eq!(interpreter_for(image), None);
        }
    }

    #[test]
    fn mach_o_and_fat_files_start_through_the_macos_layer_on_aarch64_and_x86_64() {
        let runs = cfg!(any(target_arch = "aarch64", target_arch = "x86_64"));
        let thin = interpreter_for(b"\xcf\xfa\xed\xfe\x0c\0\0\x01");
        let fat = interpreter_for(b"\xca\xfe\xba\xbe\0\0\0\x02rest");
        assert_eq!(thin, runs.then_some(MACOS_LAYER));
        assert_eq!(fat, runs.then_some(MACOS_FAT_LAYER));
        assert_eq!(MACOS_LAYER.path, b"/usr/lib/eggos/macos");
        assert_eq!(MACOS_FAT_LAYER.path, b"/usr/lib/eggos/macos");
        for image in [
            &b"\xca\xfe\xba\xbe\0\0\0\x41java"[..],
            b"\xca\xfe\xba\xbe\0\0\0\0",
            b"\xca\xfe\xba\xbe\0\0",
            b"\xfe\xed\xfa\xcf",
            b"\xce\xfa\xed\xfe",
        ] {
            assert_eq!(interpreter_for(image), None);
        }
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

    #[test]
    fn a_script_line_names_its_interpreter_and_one_argument_as_linux_reads_it() {
        fn line(head: &[u8]) -> Option<(&[u8], Option<&[u8]>)> {
            script_line(head).map(|line| (line.interpreter, line.argument))
        }
        assert_eq!(
            line(b"#!/bin/eggsh\necho"),
            Some((&b"/bin/eggsh"[..], None))
        );
        assert_eq!(
            line(b"#! \t/usr/bin/env  python3 -u \t\nrest"),
            Some((&b"/usr/bin/env"[..], Some(&b"python3 -u"[..])))
        );
        assert_eq!(line(b"#!/bin/sh"), Some((&b"/bin/sh"[..], None)));
        assert_eq!(line(b"#!/bin/sh\r\n"), Some((&b"/bin/sh\r"[..], None)));
        assert_eq!(line(b"#!\n/bin/sh"), None);
        assert_eq!(line(b"#!   \n"), None);
        assert_eq!(line(b"\x7fELF"), None);
        let mut long = b"#!/bin/sh ".to_vec();
        long.resize(SCRIPT_LINE_BYTES, b'a');
        assert_eq!(line(&long).map(|(path, _)| path), Some(&b"/bin/sh"[..]));
        let mut cut = b"#!/".to_vec();
        cut.resize(SCRIPT_LINE_BYTES, b'a');
        assert_eq!(line(&cut), None);
    }
}
