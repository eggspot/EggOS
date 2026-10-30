//! Starting a Windows program: the kernel runs the layer as `windows <program> <arguments>`
//! for a file that starts with `MZ` (D118), a console or a GUI program (D129). The layer reads the file, checks it, maps the image
//! at its preferred base (or relocates it), binds its imports to the layer's own functions,
//! protects each section, maps the page Windows shares with every process, handles the
//! program's faults (D151), builds the TEB and PEB, points the thread's TEB register at the TEB
//! (GS on x86_64; `tpidr_el0` on aarch64, from which `x18` is loaded for the program's code) and
//! calls the entry point, whose return value is the exit code. Exception handling (D177) learns
//! where the image's exception directory is and where the layer's own code lies.

use crate::fault::{self, Image};
use crate::fault_handler;
use crate::image::{self, BindError};
use crate::linux::{self, PROT_EXEC, PROT_READ, PROT_WRITE};
use crate::pe::{InvalidPe, Machine, Pe, Subsystem, directory};
use crate::user_shared_data;
use crate::win32::{self, thread_environment};
use eggos_runtime::errno::Errno;
use eggos_runtime::startup::{Startup, auxv};

const STANDARD_ERROR: usize = 2;
const CANNOT_EXECUTE: i32 = 126;
const STACK_RESERVE: usize = 8 << 20;
const DLL_PROCESS_ATTACH: u32 = 1;
const PROGRAM_HEADER_SIZE: usize = 56;
#[cfg(target_arch = "x86_64")]
const THIS_COMPUTER: Machine = Machine::X64;
#[cfg(target_arch = "aarch64")]
const THIS_COMPUTER: Machine = Machine::Arm64;

enum Failure {
    Usage,
    Open(Errno),
    Read(Errno),
    Invalid(InvalidPe),
    Unresolved(BindError),
    Memory,
}

pub fn run(startup: &Startup) -> i32 {
    let mut arguments = startup.arguments();
    let _layer = arguments.next();
    let program = arguments
        .next()
        .map_or(&b"windows"[..], |path| path.to_bytes());
    let failure = match start(*startup) {
        Ok(never) => match never {},
        Err(failure) => failure,
    };
    report(program, failure);
    CANNOT_EXECUTE
}

fn report(program: &[u8], failure: Failure) {
    let mut message = Message::default();
    message.push(b"windows: ");
    if !matches!(failure, Failure::Usage) {
        message.push(program);
        message.push(b": ");
    }
    match failure {
        Failure::Usage => {
            message.push(b"Windows programs start by their own name, e.g. `hello.exe`")
        }
        Failure::Open(errno) | Failure::Read(errno) => message.push(errno.message()),
        Failure::Invalid(InvalidPe(reason)) => message.push(reason.as_bytes()),
        Failure::Unresolved(BindError::Invalid(InvalidPe(reason))) => {
            message.push(reason.as_bytes())
        }
        Failure::Unresolved(BindError::Unresolved { library, function }) => {
            message.push(b"needs ");
            message.push(function.as_bytes());
            message.push(b" from ");
            message.push(library.as_bytes());
            message.push(b", which EggOS does not provide yet");
        }
        Failure::Unresolved(BindError::UnresolvedOrdinal { library, ordinal }) => {
            message.push(b"needs function ");
            let mut digits = [0u8; 5];
            message.push(decimal(ordinal, &mut digits));
            message.push(b" (by ordinal) from ");
            message.push(library.as_bytes());
            message.push(b", which EggOS does not provide yet");
        }
        Failure::Memory => message.push(b"out of memory"),
    }
    message.push(b"\n");
    message.write();
}

fn decimal(mut value: u16, digits: &mut [u8; 5]) -> &[u8] {
    let mut first = digits.len();
    loop {
        first -= 1;
        digits[first] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            return &digits[first..];
        }
    }
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

fn start(startup: Startup) -> Result<core::convert::Infallible, Failure> {
    let path = startup.arguments().nth(1).ok_or(Failure::Usage)?;
    let file = read_file(path.to_bytes_with_nul())?;
    let pe = Pe::parse(file).map_err(Failure::Invalid)?;
    if let Some(refusal) = pe.machine.refusal_on(THIS_COMPUTER) {
        return Err(Failure::Invalid(refusal));
    }
    if pe.subsystem == Subsystem::Gui {
        win32::set_gui_program();
    }
    let size = pe.mapped_size() as usize;
    let read_write = PROT_READ | PROT_WRITE;
    let base = match linux::map_anonymous(pe.image_base as usize, size, read_write, true) {
        Ok(base) => base,
        Err(_) => linux::map_anonymous(0, size, read_write, false).map_err(|_| Failure::Memory)?,
    };
    // SAFETY: `base` is a new private mapping of `size` zeroed bytes that only this slice uses
    // until the program runs.
    let image = unsafe { core::slice::from_raw_parts_mut(base as *mut u8, size) };
    image::copy_sections(&pe, image);
    image::relocate(&pe, image, base as u64).map_err(Failure::Invalid)?;
    image::bind_imports(&pe, image, |library, import| match import {
        image::Import::Name(function) => win32::resolve(library, function),
        image::Import::Ordinal(ordinal) => win32::resolve_ordinal(library, ordinal),
    })
    .map_err(Failure::Unresolved)?;
    let tls = image::thread_local_storage(&pe, image, base as u64).map_err(Failure::Invalid)?;
    let heap = win32::process_heap();
    win32::set_process(startup, base as u64).ok_or(Failure::Memory)?;
    let stack = stack_top();
    let teb = thread_environment::create(thread_environment::Blocks {
        image_base: base as u64,
        heap,
        stack_base: stack,
        stack_limit: stack - STACK_RESERVE,
    })
    .map_err(|_| Failure::Memory)?;
    if let Some(tls) = tls {
        image[tls.index..tls.index + 4].copy_from_slice(&0u32.to_le_bytes());
        // SAFETY: the template lies inside the image, which stays mapped and readable for the
        // process's life.
        let template = unsafe {
            core::slice::from_raw_parts(
                (base + tls.template_start) as *const u8,
                tls.template_end - tls.template_start,
            )
        };
        thread_environment::set_static_tls(thread_environment::StaticTls {
            template,
            zero_fill: tls.zero_fill,
            callbacks: tls
                .callbacks
                .map(|callbacks| (base, base + callbacks, base + size)),
        })
        .ok_or(Failure::Memory)?;
        let slots = thread_environment::new_thread_local_storage().ok_or(Failure::Memory)?;
        // SAFETY: `teb` is the TEB `create` returned.
        unsafe { thread_environment::set_thread_local_storage(teb, slots) };
    }
    let entry = base + pe.entry as usize;
    protect(&pe, base)?;
    share_user_data(startup);
    let exceptions = pe.directory(directory::EXCEPTION);
    win32::set_exception_image(
        base as u64,
        size as u64,
        (exceptions.address, exceptions.size),
        layer_code(&startup),
    );
    fault_handler::install(
        path.to_bytes(),
        Image {
            base: base as u64,
            size: size as u64,
        },
    );
    // SAFETY: the file's copy is not used after this: the image holds everything it needs.
    let _ = unsafe { linux::unmap(file.as_ptr() as usize, file.len().max(1)) };
    // SAFETY: the thread's TEB register gets the TEB this process's Win32 functions read
    // through it.
    unsafe { linux::set_thread_environment_block(teb) }.map_err(|_| Failure::Memory)?;
    thread_environment::run_tls_callbacks(DLL_PROCESS_ATTACH);
    // SAFETY: `entry` is the checked entry point inside an executable section of the image,
    // laid out, relocated and bound, and Windows calls it as a function without arguments
    // that returns the exit code.
    let code = unsafe { win32::call_program(entry, [0; 4]) };
    win32::end_thread(code as u32)
}

fn read_file(path: &[u8]) -> Result<&'static [u8], Failure> {
    let fd = linux::open(path, linux::O_RDONLY).map_err(Failure::Open)?;
    let size = linux::fstat(fd).map_err(Failure::Read)?[linux::STAT_SIZE_WORD] as usize;
    if size as u64 > crate::pe::MAX_IMAGE_SIZE {
        return Err(Failure::Invalid(InvalidPe("larger than 1 GiB")));
    }
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

fn share_user_data(startup: Startup) {
    let capabilities = startup.auxiliary_value(auxv::HWCAP).unwrap_or(0);
    let features = user_shared_data::processor_features(capabilities as u64);
    win32::set_processor_features(features);
    let read_write = PROT_READ | PROT_WRITE;
    let Ok(page) = linux::map_anonymous(
        user_shared_data::ADDRESS,
        user_shared_data::SIZE,
        read_write,
        true,
    ) else {
        return;
    };
    // SAFETY: `page` is a new private mapping of one zeroed page that nothing else uses.
    let bytes = unsafe { &mut *(page as *mut [u8; user_shared_data::SIZE]) };
    user_shared_data::write_processor_features(bytes, features);
    // SAFETY: the page is read-only from now on, as on Windows; nothing writes it again.
    let _ = unsafe { linux::protect(page, user_shared_data::SIZE, PROT_READ) };
}

/// Where the layer's own code lies, from its program headers, or nothing if they cannot be
/// read.
fn layer_code(startup: &Startup) -> (u64, u64) {
    let headers = startup.auxiliary_value(auxv::PHDR).unwrap_or(0);
    let count = startup.auxiliary_value(auxv::PHNUM).unwrap_or(0);
    let size = startup.auxiliary_value(auxv::PHENT).unwrap_or(0);
    if headers == 0 || size != PROGRAM_HEADER_SIZE {
        return (0, 0);
    }
    // SAFETY: the kernel maps the layer's program headers where `AT_PHDR` says, for the
    // process's life.
    let bytes = unsafe { core::slice::from_raw_parts(headers as *const u8, count * size) };
    fault::executable_range(bytes, headers as u64).unwrap_or((0, 0))
}

fn stack_top() -> usize {
    let marker = 0u8;
    (&raw const marker as usize).next_multiple_of(linux::PAGE_SIZE)
}

fn protect(pe: &Pe<'_>, base: usize) -> Result<(), Failure> {
    let headers = (pe.headers_size as usize).next_multiple_of(linux::PAGE_SIZE);
    // SAFETY: the headers are read-only from now on, as on Windows.
    unsafe { linux::protect(base, headers, PROT_READ) }.map_err(|_| Failure::Memory)?;
    for (start, end, protection) in image::section_protections(pe) {
        let flags = if protection.read { PROT_READ } else { 0 }
            | if protection.write { PROT_WRITE } else { 0 }
            | if protection.execute {
                PROT_EXEC | PROT_READ
            } else {
                0
            };
        // SAFETY: each section's pages get the protection its flags ask for; nothing but the
        // program uses them from now on.
        unsafe { linux::protect(base + start, end - start, flags) }.map_err(|_| Failure::Memory)?;
    }
    Ok(())
}
