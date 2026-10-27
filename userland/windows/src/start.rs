//! Starting a Windows program: the kernel runs the layer as `windows <program> <arguments>`
//! for a file that starts with `MZ` (D118). The layer reads the file, checks it, maps the image
//! at its preferred base (or relocates it), binds its imports to the layer's own functions,
//! protects each section, builds the TEB and PEB, points GS at the TEB and calls the entry
//! point, whose return value is the exit code.

use crate::image::{self, BindError};
use crate::linux::{self, PROT_EXEC, PROT_READ, PROT_WRITE};
use crate::pe::{InvalidPe, Pe};
use crate::win32::{self, thread_environment};
use eggos_runtime::errno::Errno;
use eggos_runtime::startup::Startup;

const STANDARD_ERROR: usize = 2;
const CANNOT_EXECUTE: i32 = 126;
const STACK_RESERVE: usize = 8 << 20;
const DLL_PROCESS_ATTACH: u32 = 1;

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
        image::Import::Ordinal(_) => None,
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
        let slots = thread_local_block(image, &tls).ok_or(Failure::Memory)?;
        image[tls.index..tls.index + 4].copy_from_slice(&0u32.to_le_bytes());
        // SAFETY: `teb` is the TEB `create` returned.
        unsafe { thread_environment::set_thread_local_storage(teb, slots) };
    }
    let callbacks = tls.and_then(|tls| tls.callbacks);
    let entry = base + pe.entry as usize;
    protect(&pe, base)?;
    // SAFETY: the file's copy is not used after this: the image holds everything it needs.
    let _ = unsafe { linux::unmap(file.as_ptr() as usize, file.len().max(1)) };
    // SAFETY: GS gets the TEB this process's Win32 functions read through it.
    unsafe { linux::set_gs_base(teb) }.map_err(|_| Failure::Memory)?;
    if let Some(callbacks) = callbacks {
        run_callbacks(base, callbacks, size);
    }
    // SAFETY: `entry` is the checked entry point inside an executable section of the image,
    // laid out, relocated and bound, and Windows calls it as a function without arguments
    // that returns the exit code.
    let entry: extern "win64" fn() -> u32 = unsafe { core::mem::transmute(entry) };
    linux::exit(entry())
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

fn stack_top() -> usize {
    let marker = 0u8;
    (&raw const marker as usize).next_multiple_of(linux::PAGE_SIZE)
}

fn thread_local_block(image: &[u8], tls: &image::ThreadLocalStorage) -> Option<usize> {
    let template = &image[tls.template_start..tls.template_end];
    let size = template.len().checked_add(tls.zero_fill)?;
    let block = win32::heap_allocate(size.max(1));
    let slots = win32::heap_allocate(8).cast::<u64>();
    if block.is_null() || slots.is_null() {
        return None;
    }
    // SAFETY: `block` is a fresh heap block of `size` bytes, the template's length first.
    unsafe { block.copy_from_nonoverlapping(template.as_ptr(), template.len()) };
    // SAFETY: `slots` is a fresh heap block of one pointer.
    unsafe { slots.write(block as u64) };
    Some(slots as usize)
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

fn run_callbacks(base: usize, callbacks: usize, size: usize) {
    let mut at = base + callbacks;
    while at + 8 <= base + size {
        // SAFETY: the TLS directory's callback array, checked to start inside the image, is a
        // null-terminated array of function addresses the image's relocations fixed.
        let callback = unsafe { (at as *const u64).read_unaligned() };
        if callback == 0 {
            return;
        }
        // SAFETY: as above; Windows calls each callback as `PIMAGE_TLS_CALLBACK`.
        let callback: extern "win64" fn(usize, u32, usize) =
            unsafe { core::mem::transmute(callback as usize) };
        callback(base, DLL_PROCESS_ATTACH, 0);
        at += 8;
    }
}
