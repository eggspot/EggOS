//! Starting a macOS program, as dyld does: the kernel runs the layer as
//! `macos <program> <arguments>` for a Mach-O or fat file (D120). The layer reads the file,
//! picks the slice for this CPU, checks it, maps its segments (anywhere for a position-
//! independent program, at its own addresses otherwise), binds its imports to the layer's
//! libSystem and rebases its pointers by the slide through its chained fixups, protects each
//! segment and calls `main(argc, argv, envp, apple)`, whose return value goes to `exit`.

use crate::appkit;
use crate::fixups::{ChainedFixups, Source};
use crate::foundation;
use crate::libsystem;
use crate::linux::{self, PROT_EXEC, PROT_READ, PROT_WRITE};
use crate::macho::{
    self, Cpu, InvalidMachO, MachO, NoSlice, SG_READ_ONLY, VM_PROT_EXECUTE, VM_PROT_READ,
    VM_PROT_WRITE,
};
use crate::objc;
use core::ffi::{c_char, c_int};
use eggos_runtime::errno::Errno;
use eggos_runtime::startup::Startup;

const STANDARD_ERROR: usize = 2;
const CANNOT_EXECUTE: i32 = 126;
const EXECUTABLE_PATH: &[u8] = b"executable_path=";
const LAYER_ARGUMENTS: usize = 2;

enum Failure {
    Usage,
    Open(Errno),
    Read(Errno),
    Invalid(InvalidMachO),
    OtherCpus([Option<Cpu>; 4]),
    Library(&'static [u8]),
    TooManyMissing,
    Memory,
}

pub fn run(startup: &Startup) -> i32 {
    let program = startup
        .arguments()
        .nth(1)
        .map_or(&b"macos"[..], |path| path.to_bytes());
    let failure = match start(*startup) {
        Ok(never) => match never {},
        Err(failure) => failure,
    };
    report(program, failure);
    CANNOT_EXECUTE
}

fn report(program: &[u8], failure: Failure) {
    let mut message = Message::default();
    message.push(b"macos: ");
    if !matches!(failure, Failure::Usage) {
        message.push(program);
        message.push(b": ");
    }
    match failure {
        Failure::Usage => message.push(b"macOS programs start by their own name, e.g. `hello`"),
        Failure::Open(errno) | Failure::Read(errno) => message.push(errno.message()),
        Failure::Invalid(InvalidMachO(reason)) => message.push(reason.as_bytes()),
        Failure::OtherCpus(found) => {
            message.push(b"has code only for ");
            for (index, cpu) in found.iter().flatten().enumerate() {
                if index > 0 {
                    message.push(b", ");
                }
                message.push(cpu.name().as_bytes());
            }
            message.push(b", not for this ");
            message.push(macho::host_cpu().map_or("", Cpu::name).as_bytes());
            message.push(b" machine");
        }
        Failure::Library(path) => {
            message.push(b"needs ");
            message.push(path);
            message.push(b", which EggOS does not provide yet");
        }
        Failure::TooManyMissing => {
            message.push(b"imports more functions EggOS does not provide yet than it can stub")
        }
        Failure::Memory => message.push(b"out of memory"),
    }
    message.push(b"\n");
    message.write();
}

struct Message {
    bytes: [u8; 512],
    length: usize,
}

impl Default for Message {
    fn default() -> Self {
        Self {
            bytes: [0; 512],
            length: 0,
        }
    }
}

impl Message {
    fn push(&mut self, text: &[u8]) {
        let room = self.bytes.len() - self.length;
        let taken = text.len().min(room);
        self.bytes[self.length..self.length + taken].copy_from_slice(&text[..taken]);
        self.length += taken;
    }

    fn write(&self) {
        let _ = linux::write_all(STANDARD_ERROR, &self.bytes[..self.length]);
    }
}

type Main =
    extern "C" fn(c_int, *const *mut c_char, *const *mut c_char, *const *mut c_char) -> c_int;

fn start(startup: Startup) -> Result<core::convert::Infallible, Failure> {
    let stack_marker = 0u8;
    // SAFETY: the layer's first step, on its only thread.
    unsafe { libsystem::set_up_main_thread(&raw const stack_marker as usize) };
    let path = startup.arguments().nth(1).ok_or(Failure::Usage)?;
    let file = read_file(path.to_bytes_with_nul())?;
    let cpu = macho::host_cpu().ok_or(Failure::OtherCpus([None; 4]))?;
    let slice = macho::select_slice(file, cpu).map_err(|error| match error {
        NoSlice::Invalid(invalid) => Failure::Invalid(invalid),
        NoSlice::OtherCpus { found } => Failure::OtherCpus(found),
    })?;
    let program = MachO::parse(slice).map_err(Failure::Invalid)?;
    if program.cpu != cpu {
        return Err(Failure::OtherCpus([Some(program.cpu), None, None, None]));
    }
    for library in program.libraries() {
        if Library::provided(library.path).is_none() && !library.weak {
            return Err(Failure::Library(library.path));
        }
    }
    let uses_objc = program.objc.present()
        || program
            .libraries()
            .any(|library| Library::provided(library.path).is_some_and(Library::is_objc));
    if uses_objc && !(objc::set_up() && foundation::set_up()) {
        return Err(Failure::Memory);
    }
    let uses_appkit = program.libraries().any(|library| {
        matches!(
            Library::provided(library.path),
            Some(Library::AppKit | Library::Cocoa)
        )
    });
    if uses_appkit && !appkit::set_up() {
        return Err(Failure::Memory);
    }
    let fixups = match program.chained_fixups {
        Some(range) => Some(ChainedFixups::parse(&program, range).map_err(Failure::Invalid)?),
        None if program.dyld_info => {
            return Err(Failure::Invalid(InvalidMachO(
                "uses LC_DYLD_INFO fixups instead of chained ones (built for an older macOS: \
                 before 12 on arm64, before 13 on x86_64), which EggOS does not read yet",
            )));
        }
        None => None,
    };
    let size = program.mapped_size() as usize;
    let read_write = PROT_READ | PROT_WRITE;
    let base = if program.pie {
        linux::map_anonymous(0, size, read_write, false)
    } else {
        linux::map_anonymous(program.base_address() as usize, size, read_write, true)
    }
    .map_err(|_| Failure::Memory)?;
    // SAFETY: `base` is a new private mapping of `size` zeroed bytes that only this slice uses
    // until the program runs.
    let image = unsafe { core::slice::from_raw_parts_mut(base as *mut u8, size) };
    for segment in program.segments() {
        let at = (segment.vmaddr - program.base_address()) as usize;
        let from = segment.fileoff as usize;
        let length = segment.filesize as usize;
        image[at..at + length].copy_from_slice(&slice[from..from + length]);
    }
    if let Some(fixups) = fixups {
        let imports = bind_imports(&program, &fixups)?;
        fixups
            .apply(&program, image, base as u64, imports)
            .map_err(Failure::Invalid)?;
    }
    let slide = (base as isize).wrapping_sub(program.base_address() as isize);
    if uses_objc {
        // SAFETY: the image is mapped, bound and rebased, still writable everywhere, and no
        // program code has run.
        unsafe { objc::register_image(slide, &program.objc) }.map_err(Failure::Invalid)?;
    }
    protect(&program, base)?;
    if uses_objc {
        let unwind_info = program.unwind_info.map(|section| {
            (
                base + (section.address - program.base_address()) as usize,
                section.size as usize,
            )
        });
        objc::exception::set_image(base, size, unwind_info);
    }
    if let Some(template) = program.thread_locals {
        let address = base + (template.address - program.base_address()) as usize;
        // SAFETY: the template lies inside a readable segment of the image, which stays
        // mapped and which nothing writes: each thread uses its own copy.
        let made = unsafe {
            libsystem::set_thread_local_template(
                address,
                template.size as usize,
                template.alignment as usize,
            )
        };
        if !made {
            return Err(Failure::Memory);
        }
    }
    let arguments = argument_vector(&startup)?;
    let environment = environment_vector(&startup)?;
    let (apple, executable_path) = apple_vector(path.to_bytes())?;
    // SAFETY: the program has not started; these record its image, arguments and streams.
    unsafe { libsystem::set_image(base, slide, executable_path) };
    // SAFETY: as above; the arrays are never freed.
    unsafe { libsystem::set_process(argument_count(&startup), arguments, environment) };
    // SAFETY: as above.
    unsafe { libsystem::set_up_streams() };
    if uses_objc {
        objc::run_load_methods();
    }
    let entry = base + program.entry as usize;
    // SAFETY: `entry` is LC_MAIN's checked entry point inside an executable segment of the
    // image, mapped, bound, rebased and protected; dyld calls it as
    // `int main(int, char **, char **, char **)`.
    let main: Main = unsafe { core::mem::transmute(entry) };
    let status = main(
        argument_count(&startup) as c_int,
        arguments,
        environment,
        apple,
    );
    libsystem::exit(status)
}

fn bind_imports<'a>(
    program: &MachO<'a>,
    fixups: &ChainedFixups<'a>,
) -> Result<&'static [u64], Failure> {
    let count = fixups.import_count();
    let table = allocate_pages(count.max(1) * 8)?.cast::<u64>();
    let mut missing = 0;
    for index in 0..count {
        let import = fixups.import(index).map_err(Failure::Invalid)?;
        let library = match import.source {
            Source::Library(ordinal) => Some(
                program
                    .library(ordinal)
                    .ok_or(Failure::Invalid(InvalidMachO(
                        "an import from a library it does not link",
                    )))?
                    .path,
            ),
            Source::Flat | Source::Weak => Some(macho::LIBSYSTEM),
            Source::Executable => None,
        };
        let library = library.and_then(Library::provided);
        let found = library.and_then(|_| resolve(import.name));
        // SAFETY: the file's mapping stays for as long as the program runs, so the name does
        // too.
        let name: &'static [u8] =
            unsafe { core::slice::from_raw_parts(import.name.as_ptr(), import.name.len()) };
        let address = match (found, library) {
            (Some(address), _) => address,
            (None, _) if import.weak => 0,
            (None, Some(library)) if objc::is_class_symbol(name) => {
                objc::bind_missing_class(library.name(), name).ok_or(Failure::TooManyMissing)?
            }
            (None, library) => {
                if missing == libsystem::MISSING_STUBS {
                    return Err(Failure::TooManyMissing);
                }
                missing += 1;
                let library = library.unwrap_or(Library::System);
                libsystem::bind_missing(missing - 1, library.name(), name)
            }
        };
        // SAFETY: `table` holds `count` entries.
        unsafe {
            table
                .wrapping_add(index)
                .write(address.wrapping_add(import.addend as u64))
        };
    }
    // SAFETY: the `count` entries written above, in a block that is never freed.
    Ok(unsafe { core::slice::from_raw_parts(table, count) })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Library {
    System,
    Objc,
    Foundation,
    CoreFoundation,
    AppKit,
    Cocoa,
}

impl Library {
    fn provided(path: &[u8]) -> Option<Self> {
        Some(match path {
            macho::LIBSYSTEM => Self::System,
            macho::LIBOBJC => Self::Objc,
            macho::FOUNDATION => Self::Foundation,
            macho::CORE_FOUNDATION => Self::CoreFoundation,
            macho::APPKIT => Self::AppKit,
            macho::COCOA => Self::Cocoa,
            _ => return None,
        })
    }

    fn is_objc(self) -> bool {
        self != Self::System
    }

    fn name(self) -> &'static [u8] {
        match self {
            Self::System => b"libSystem",
            Self::Objc => b"libobjc",
            Self::Foundation => b"Foundation",
            Self::CoreFoundation => b"CoreFoundation",
            Self::AppKit => b"AppKit",
            Self::Cocoa => b"Cocoa",
        }
    }
}

fn resolve(name: &[u8]) -> Option<u64> {
    objc::resolve(name)
        .or_else(|| foundation::resolve(name))
        .or_else(|| appkit::resolve(name))
        .or_else(|| libsystem::resolve(name))
}

fn allocate_pages(size: usize) -> Result<*mut u8, Failure> {
    let block = linux::map_anonymous(0, size, PROT_READ | PROT_WRITE, false)
        .map_err(|_| Failure::Memory)?;
    Ok(block as *mut u8)
}

fn argument_count(startup: &Startup) -> usize {
    startup.argument_count().saturating_sub(LAYER_ARGUMENTS)
}

fn argument_vector(startup: &Startup) -> Result<*mut *mut c_char, Failure> {
    let count = argument_count(startup);
    let vector = allocate_pages((count + 1) * 8)?.cast::<*mut c_char>();
    for (index, argument) in startup
        .arguments()
        .skip(LAYER_ARGUMENTS)
        .enumerate()
        .take(count)
    {
        // SAFETY: `vector` holds `count + 1` pointers; the strings stay on the start-up stack.
        unsafe {
            vector
                .wrapping_add(index)
                .write(argument.as_ptr().cast_mut())
        };
    }
    Ok(vector)
}

fn environment_vector(startup: &Startup) -> Result<*mut *mut c_char, Failure> {
    let count = startup.environment().count();
    let vector = allocate_pages((count + 1) * 8)?.cast::<*mut c_char>();
    for (index, entry) in startup.environment().enumerate().take(count) {
        // SAFETY: as in `argument_vector`.
        unsafe { vector.wrapping_add(index).write(entry.as_ptr().cast_mut()) };
    }
    Ok(vector)
}

fn apple_vector(path: &[u8]) -> Result<(*mut *mut c_char, *const c_char), Failure> {
    let length = EXECUTABLE_PATH.len() + path.len() + 1;
    let block = allocate_pages(16 + length)?;
    let text = block.wrapping_add(16);
    // SAFETY: the block holds two pointers, then the `executable_path=` string and its NUL,
    // zeroed.
    let bytes = unsafe { core::slice::from_raw_parts_mut(text, length) };
    bytes[..EXECUTABLE_PATH.len()].copy_from_slice(EXECUTABLE_PATH);
    bytes[EXECUTABLE_PATH.len()..length - 1].copy_from_slice(path);
    // SAFETY: the first of the two pointers; the second stays null.
    unsafe { block.cast::<*mut u8>().write(text) };
    Ok((
        block.cast(),
        text.wrapping_add(EXECUTABLE_PATH.len()).cast_const().cast(),
    ))
}

fn read_file(path: &[u8]) -> Result<&'static [u8], Failure> {
    let fd = linux::open(path, linux::O_RDONLY | linux::O_CLOEXEC, 0).map_err(Failure::Open)?;
    let mut status = [0u8; crate::darwin::LINUX_STAT_SIZE];
    linux::fstat(fd, &mut status).map_err(Failure::Read)?;
    let size = crate::darwin::Status::from_linux(&status, crate::darwin::LinuxLayout::HOST).size;
    if size < 0 || size as u64 > 2 * macho::MAX_IMAGE_SIZE {
        return Err(Failure::Invalid(InvalidMachO("larger than 2 GiB")));
    }
    let size = size as usize;
    let buffer = linux::map_anonymous(0, size.max(1), PROT_READ | PROT_WRITE, false)
        .map_err(|_| Failure::Memory)?;
    // SAFETY: a new private mapping of at least `size` bytes, used only through this slice.
    let bytes = unsafe { core::slice::from_raw_parts_mut(buffer as *mut u8, size) };
    let mut filled = 0;
    while filled < size {
        match linux::read(fd, &mut bytes[filled..]).map_err(Failure::Read)? {
            0 => break,
            read => filled += read,
        }
    }
    let _ = linux::close(fd);
    Ok(&bytes[..filled])
}

fn protect(program: &MachO<'_>, base: usize) -> Result<(), Failure> {
    for segment in program.segments() {
        let start = base + (segment.vmaddr - program.base_address()) as usize;
        let length = (segment.vmsize as usize).next_multiple_of(linux::PAGE_SIZE);
        let writable = segment.initprot & VM_PROT_WRITE != 0 && segment.flags & SG_READ_ONLY == 0;
        let protection = if segment.initprot & VM_PROT_READ != 0 {
            PROT_READ
        } else {
            0
        } | if writable { PROT_WRITE } else { 0 }
            | if segment.initprot & VM_PROT_EXECUTE != 0 {
                PROT_EXEC | PROT_READ
            } else {
                0
            };
        // SAFETY: each segment's pages get the protection its load command asks for; nothing
        // but the program uses them from now on.
        unsafe { linux::protect(start, length, protection) }.map_err(|_| Failure::Memory)?;
    }
    Ok(())
}
