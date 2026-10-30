//! Running an x64 program on an ARM64 computer (D191, slice 39d2): before the image is
//! relocated, [`prepare`] loads its translation from the cache (`/var/cache/eggos/translate`,
//! keyed by the SHA-256 of the file, the translator's version and the code generation mode)
//! or translates it and stores it there; after its imports are bound, [`install`] maps the
//! translated code with a forward bridge per import, points each import slot at itself so
//! guest calls reach the bridges through the entries table, moves the code's guest
//! addresses by the load difference and writes the entries table. [`call_guest`] is how the
//! layer calls the program's functions (its entry point, TLS callbacks, window procedures,
//! thread start routines): through the entry trampoline, with a per-thread block holding the
//! guest state, a 64 KiB shadow stack ring and a guest stack. [`guest_fault`] tells the fault
//! handler where in the program a trap or fault in translated code happened.

use crate::linux::{self, PROT_EXEC, PROT_READ, PROT_WRITE};
use crate::pe::{Pe, read_u64};
use crate::translated::{self, CacheFiles, TrapCause};
use crate::win32::thread_environment::teb;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use eggos_translate::cache::{self, CacheFile, policy};
use eggos_translate::codegen::state;
use eggos_translate::image::bridge::{self, thread};
use eggos_translate::image::runtime::{context, shadow_stack};
use eggos_translate::image::{BackMapEntry, ImageInput, TranslatedImage, translate_image};

const GUEST_STACK_BYTES: usize = 8 << 20;
const GUEST_STACK_HEADROOM: u64 = 256;
const CALL_FRAME_BYTES: u64 = 64;
const THREAD_BLOCK_BYTES: usize = 4096;
const INITIAL_MXCSR: u32 = 0x1F80;
const INITIAL_X87_CONTROL: u16 = 0x037F;
const BRIDGE_ALIGNMENT: usize = 16;
const DIRECTORY_BUFFER_BYTES: usize = 4096;
const DIRENT_RECORD_LENGTH: usize = 16;
const DIRENT_NAME: usize = 19;
const NANOSECONDS_PER_SECOND: u64 = 1_000_000_000;
const STATFS_BLOCK_SIZE: usize = 1;
const STATFS_BLOCKS: usize = 2;

struct Translation {
    guest_base: u64,
    guest_size: u64,
    code: usize,
    code_bytes: usize,
    mapped: usize,
    entries: usize,
    enter: usize,
    dispatch: u64,
    procedures: usize,
    back_map: Vec<BackMapEntry>,
}

static TRANSLATION: AtomicPtr<Translation> = AtomicPtr::new(core::ptr::null_mut());

fn current() -> Option<&'static Translation> {
    let translation = TRANSLATION.load(Ordering::Acquire);
    // SAFETY: `install` stores a leaked `Box` once, before the program runs, and nothing
    // frees or changes it.
    unsafe { translation.as_ref() }
}

/// The program's translation, from the cache or made now (and then stored), for `file`
/// laid out in `image` and not yet relocated; `capabilities` is the CPU's `AT_HWCAP`.
pub fn prepare(
    file: &[u8],
    pe: &Pe<'_>,
    image: &[u8],
    capabilities: u64,
) -> Result<TranslatedImage, &'static str> {
    let mode = translated::codegen_mode(capabilities);
    let digest = cache::input_digest(file);
    let key = cache::cache_key(&digest, mode);
    let mut disk = Disk;
    if let Some(cached) = translated::load(&mut disk, &key)
        && cached.image_base == pe.image_base
    {
        return Ok(cached.image);
    }
    let sections = translated::sections(pe, image);
    let seeds = translated::seeds(pe, image);
    let input = ImageInput {
        image_base: pe.image_base,
        sections: &sections,
        seeds: &seeds,
    };
    let mut translation = translate_image(&input, mode)
        .map_err(|_| "EggOS could not translate this x64 program's code")?;
    let slots = translated::import_slots(pe, image).ok_or("the import table is damaged")?;
    let mut stub_addresses: Vec<u32> = slots.iter().map(|slot| slot.rva).collect();
    stub_addresses.extend(
        translated::procedure_stub_rvas(pe.mapped_size()).ok_or("the program is too large")?,
    );
    bridge::add_import_stubs(&mut translation, &stub_addresses)
        .map_err(|_| "EggOS could not translate this x64 program's imports")?;
    let file = CacheFile {
        input_digest: digest,
        mode,
        image_base: pe.image_base,
        image: translation,
    };
    let bytes = cache::serialize(&file);
    let _ = translated::store(&mut disk, &key, &bytes, u64::from(linux::process_id()));
    Ok(file.image)
}

fn write_u64(address: usize, value: u64) {
    // SAFETY: every caller passes an 8-byte aligned address in a writable mapping this module
    // made, or in the image or TEB, which stay mapped for the process's life.
    unsafe { (address as *mut u64).write(value) };
}

fn read_u64_at(address: usize) -> u64 {
    // SAFETY: as for `write_u64`, for reading.
    unsafe { (address as *const u64).read() }
}

/// Maps the translated code of the program at `base` (its image `image`, relocated and with
/// its imports bound) with a forward bridge per import, points each import slot at its own
/// guest address, moves the guest addresses in the code by the load difference and writes
/// the entries table.
pub fn install(
    pe: &Pe<'_>,
    image: &mut [u8],
    base: usize,
    mut translation: TranslatedImage,
) -> Result<(), &'static str> {
    const FAILED: &str = "EggOS could not map this x64 program's translated code";
    let slots = translated::import_slots(pe, image).ok_or("the import table is damaged")?;
    let code_bytes = translation.code.len();
    let mut bridges = Vec::with_capacity(slots.len());
    let mut bridge_bytes = 0;
    for slot in &slots {
        let native = read_u64(image, slot.rva as usize).map_err(|_| FAILED)?;
        let moves = bridge::argument_moves(slot.arguments());
        let words = bridge::forward_bridge(&moves, native, 0).map_err(|_| FAILED)?;
        bridge_bytes += (words.len() * 4).next_multiple_of(BRIDGE_ALIGNMENT);
        bridges.push(words);
    }
    let procedure_rvas =
        translated::procedure_stub_rvas(pe.mapped_size()).ok_or("the program is too large")?;
    let bridges_start = code_bytes.next_multiple_of(BRIDGE_ALIGNMENT);
    let trampolines_start = bridges_start + bridge_bytes;
    let trampoline_bytes =
        (bridge::PROCEDURE_TRAMPOLINE_BYTES as usize).next_multiple_of(BRIDGE_ALIGNMENT);
    let mapped = (trampolines_start + procedure_rvas.len() * trampoline_bytes)
        .next_multiple_of(linux::PAGE_SIZE);
    let total = mapped + linux::PAGE_SIZE;
    let code = linux::map_anonymous(0, total, PROT_READ | PROT_WRITE, false).map_err(|_| FAILED)?;
    // SAFETY: `code` is a new private mapping of `total` zeroed bytes that only this slice
    // uses until its first `mapped` bytes are made executable.
    let host = unsafe { core::slice::from_raw_parts_mut(code as *mut u8, total) };
    let procedures = code + mapped;
    host[..code_bytes].copy_from_slice(&translation.code);
    let delta = (base as u64).wrapping_sub(pe.image_base);
    for &offset in &translation.guest_address_literals {
        let at = offset as usize;
        let mut literal = [0u8; 8];
        literal.copy_from_slice(&host[at..at + 8]);
        let moved = u64::from_le_bytes(literal).wrapping_add(delta);
        host[at..at + 8].copy_from_slice(&moved.to_le_bytes());
    }
    let dispatch = (code + translation.dispatch as usize) as u64;
    let mut at = bridges_start;
    for (slot, words) in slots.iter().zip(&mut bridges) {
        let length = words.len();
        words[length - 2] = dispatch as u32;
        words[length - 1] = (dispatch >> 32) as u32;
        for (index, word) in words.iter().enumerate() {
            host[at + 4 * index..at + 4 * index + 4].copy_from_slice(&word.to_le_bytes());
        }
        let stub = translation.entries.lookup(slot.rva).ok_or(FAILED)? as usize;
        let literal = stub + bridge::IMPORT_STUB_LITERAL as usize;
        host[literal..literal + 8].copy_from_slice(&((code + at) as u64).to_le_bytes());
        let slot_address = base as u64 + u64::from(slot.rva);
        let slot = slot.rva as usize;
        image[slot..slot + 8].copy_from_slice(&slot_address.to_le_bytes());
        at += (length * 4).next_multiple_of(BRIDGE_ALIGNMENT);
    }
    for (index, &rva) in procedure_rvas.iter().enumerate() {
        let at = trampolines_start + index * trampoline_bytes;
        let slot = (procedures + 8 * index) as u64;
        let words = bridge::procedure_trampoline(slot);
        for (byte, word) in (at..).step_by(4).zip(words) {
            host[byte..byte + 4].copy_from_slice(&word.to_le_bytes());
        }
        let stub = translation.entries.lookup(rva).ok_or(FAILED)? as usize;
        let literal = stub + bridge::IMPORT_STUB_LITERAL as usize;
        host[literal..literal + 8].copy_from_slice(&((code + at) as u64).to_le_bytes());
    }
    let table = translation.entries.runtime_bytes(base as u64, code as u64);
    let entries_bytes = table.len().next_multiple_of(linux::PAGE_SIZE);
    let entries = linux::map_anonymous(0, entries_bytes, PROT_READ | PROT_WRITE, false)
        .map_err(|_| FAILED)?;
    // SAFETY: `entries` is a new private mapping of at least `table.len()` bytes.
    unsafe { core::slice::from_raw_parts_mut(entries as *mut u8, table.len()) }
        .copy_from_slice(&table);
    // SAFETY: the table is read-only from now on; only the dispatch reads it.
    unsafe { linux::protect(entries, entries_bytes, PROT_READ) }.map_err(|_| FAILED)?;
    synchronize_instruction_cache(code, mapped);
    // SAFETY: the code is complete; from now on it is only executed and read.
    unsafe { linux::protect(code, mapped, PROT_READ | PROT_EXEC) }.map_err(|_| FAILED)?;
    let back_map = core::mem::take(&mut translation.back_map);
    let state = Box::new(Translation {
        guest_base: base as u64,
        guest_size: pe.mapped_size(),
        code,
        code_bytes,
        mapped,
        entries,
        enter: code + translation.enter as usize,
        dispatch,
        procedures,
        back_map,
    });
    TRANSLATION.store(Box::into_raw(state), Ordering::Release);
    Ok(())
}

fn synchronize_instruction_cache(start: usize, length: usize) {
    let cache_type: u64;
    // SAFETY: Linux lets user code read `CTR_EL0`; it changes nothing.
    unsafe { core::arch::asm!("mrs {}, ctr_el0", out(reg) cache_type) };
    let data_line = 4usize << ((cache_type >> 16) & 0xF);
    let instruction_line = 4usize << (cache_type & 0xF);
    let end = start + length;
    let mut at = start & !(data_line - 1);
    while at < end {
        // SAFETY: cleaning a line of memory this process maps changes no data.
        unsafe { core::arch::asm!("dc cvau, {}", in(reg) at) };
        at += data_line;
    }
    // SAFETY: a barrier changes no memory.
    unsafe { core::arch::asm!("dsb ish") };
    at = start & !(instruction_line - 1);
    while at < end {
        // SAFETY: invalidating instruction cache lines of memory this process maps changes no
        // data.
        unsafe { core::arch::asm!("ic ivau, {}", in(reg) at) };
        at += instruction_line;
    }
    // SAFETY: barriers change no memory.
    unsafe { core::arch::asm!("dsb ish", "isb") };
}

fn thread_environment_block() -> usize {
    let teb: usize;
    // SAFETY: reading `tpidr_el0`, where the layer keeps the thread's TEB, changes nothing.
    unsafe { core::arch::asm!("mrs {}, tpidr_el0", out(reg) teb) };
    teb
}

fn thread_block(teb: usize) -> Option<usize> {
    let slot = teb + teb::TRANSLATED_THREAD;
    let existing = read_u64_at(slot) as usize;
    if existing != 0 {
        return Some(existing);
    }
    let read_write = PROT_READ | PROT_WRITE;
    let block = linux::map_anonymous(0, THREAD_BLOCK_BYTES, read_write, false).ok()?;
    let ring_bytes = shadow_stack::BYTES as usize;
    let ring = linux::map_anonymous(0, 2 * ring_bytes, read_write, false).ok()?;
    let ring = shadow_stack::initial(ring.next_multiple_of(ring_bytes) as u64)?;
    let stack = linux::map_anonymous(0, GUEST_STACK_BYTES, read_write, false).ok()?;
    let top = (stack + GUEST_STACK_BYTES) as u64;
    write_u64(block + state::GS_BASE as usize, teb as u64);
    // SAFETY: the state block's MXCSR and x87 control fields are aligned and inside the new
    // block.
    unsafe {
        ((block + state::MXCSR as usize) as *mut u32).write(INITIAL_MXCSR);
    }
    // SAFETY: as above.
    unsafe {
        ((block + state::X87_CONTROL as usize) as *mut u16).write(INITIAL_X87_CONTROL);
    }
    write_u64(
        block + thread::NATIVE_CALL_GUEST_STACK as usize,
        top - GUEST_STACK_HEADROOM,
    );
    write_u64(block + thread::NATIVE_CALL_SHADOW_STACK as usize, ring);
    write_u64(teb + teb::STACK_BASE, top);
    write_u64(teb + teb::STACK_LIMIT, stack as u64);
    write_u64(slot, block as u64);
    Some(block)
}

#[repr(C, align(16))]
struct Context([u64; context::SIZE as usize / 8]);

impl Context {
    fn set(&mut self, offset: u32, value: u64) {
        self.0[offset as usize / 8] = value;
    }

    fn get(&self, offset: u32) -> u64 {
        self.0[offset as usize / 8]
    }
}

fn general(register: u32) -> u32 {
    context::GENERAL + 8 * register
}

static PROCEDURES_LOCK: AtomicBool = AtomicBool::new(false);

struct ProceduresGuard;

impl ProceduresGuard {
    fn take() -> Self {
        while PROCEDURES_LOCK
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        Self
    }
}

impl Drop for ProceduresGuard {
    fn drop(&mut self) {
        PROCEDURES_LOCK.store(false, Ordering::Release);
    }
}

fn procedure_bridge(native: usize, function: &[u8], dispatch: u64) -> Option<usize> {
    let moves = bridge::argument_moves(translated::function_arguments(function));
    let words = bridge::forward_bridge(&moves, native as u64, dispatch).ok()?;
    let bytes = (words.len() * 4).next_multiple_of(linux::PAGE_SIZE);
    let page = linux::map_anonymous(0, bytes, PROT_READ | PROT_WRITE, false).ok()?;
    // SAFETY: `page` is a new private mapping of `bytes` zeroed bytes, room for every word.
    let host = unsafe { core::slice::from_raw_parts_mut(page as *mut u8, bytes) };
    for (at, word) in (0..).step_by(4).zip(&words) {
        host[at..at + 4].copy_from_slice(&word.to_le_bytes());
    }
    synchronize_instruction_cache(page, bytes);
    // SAFETY: the bridge is complete; from now on it is only executed and read.
    unsafe { linux::protect(page, bytes, PROT_READ | PROT_EXEC) }.ok()?;
    Some(page)
}

/// What `GetProcAddress` gives a translated program for the layer's function at `native`
/// named `function`: the guest address of a procedure stub whose bridge calls it (the same
/// one each time for the same function). An ARM64 program, or a translated one past
/// [`translated::PROCEDURE_STUBS`] functions, gets `native` itself.
pub fn guest_procedure(native: usize, function: &[u8]) -> usize {
    let Some(translation) = current() else {
        return native;
    };
    let Some(rvas) = translated::procedure_stub_rvas(translation.guest_size) else {
        return native;
    };
    let _guard = ProceduresGuard::take();
    let natives = translation.procedures + 8 * translated::PROCEDURE_STUBS;
    let mut free = None;
    for (index, &rva) in rvas.iter().enumerate() {
        let recorded = read_u64_at(natives + 8 * index) as usize;
        if recorded == native {
            return (translation.guest_base + u64::from(rva)) as usize;
        }
        if recorded == 0 && free.is_none() {
            free = Some((index, rva));
        }
    }
    let Some((index, rva)) = free else {
        return native;
    };
    let Some(bridge) = procedure_bridge(native, function, translation.dispatch) else {
        return native;
    };
    write_u64(translation.procedures + 8 * index, bridge as u64);
    write_u64(natives + 8 * index, native as u64);
    (translation.guest_base + u64::from(rva)) as usize
}

/// The layer's function behind a guest address `GetProcAddress` handed out, so native code
/// given it as a callback calls the function itself; `None` for any other address.
pub fn native_procedure(address: usize) -> Option<usize> {
    let translation = current()?;
    let rva = (address as u64).checked_sub(translation.guest_base)?;
    let index = translated::procedure_stub_index(translation.guest_size, rva)?;
    let natives = translation.procedures + 8 * translated::PROCEDURE_STUBS;
    let _guard = ProceduresGuard::take();
    let native = read_u64_at(natives + 8 * index) as usize;
    (native != 0).then_some(native)
}

/// Calls the program's function at `address` with `arguments` in `rcx`, `rdx`, `r8` and `r9`
/// through the entry trampoline, and returns `rax`; `None` when the program is not
/// translated or `address` is not in its image (native code is called as it is).
///
/// # Safety
/// `address` must be a function of the program that takes at most four integer or pointer
/// arguments, called with values it accepts, on a thread whose TEB register is set.
pub unsafe fn call_guest(address: usize, arguments: [usize; 4]) -> Option<usize> {
    let translation = current()?;
    let guest = address as u64;
    if guest.wrapping_sub(translation.guest_base) >= translation.guest_size {
        return None;
    }
    let teb = thread_environment_block();
    let block = thread_block(teb)?;
    let saved_stack = read_u64_at(block + thread::NATIVE_CALL_GUEST_STACK as usize);
    let saved_shadow = read_u64_at(block + thread::NATIVE_CALL_SHADOW_STACK as usize);
    let entry_stack = (saved_stack & !15) - CALL_FRAME_BYTES - 8;
    write_u64(entry_stack as usize, translation.guest_base);
    let mut frame = Context([0; context::SIZE as usize / 8]);
    for (register, value) in [1, 2, 8, 9].into_iter().zip(arguments) {
        frame.set(general(register), value as u64);
    }
    frame.set(general(4), entry_stack);
    frame.set(context::RIP, guest);
    frame.set(context::GS_BASE, teb as u64);
    frame.set(context::ENTRIES, translation.entries as u64);
    frame.set(context::SHADOW_STACK, saved_shadow);
    frame.set(context::STATE, block as u64);
    let control: u64;
    // SAFETY: reading FPCR changes nothing.
    unsafe { core::arch::asm!("mrs {}, fpcr", out(reg) control) };
    // SAFETY: `enter` is the trampoline of the mapped translated code, an AAPCS64 function
    // taking a guest context; the guest returns to the guest base pushed above.
    let enter: extern "C" fn(*mut Context) = unsafe { core::mem::transmute(translation.enter) };
    enter(&mut frame);
    // SAFETY: the native code's rounding and flush modes come back.
    unsafe { core::arch::asm!("msr fpcr, {}", in(reg) control) };
    write_u64(
        block + thread::NATIVE_CALL_GUEST_STACK as usize,
        saved_stack,
    );
    write_u64(
        block + thread::NATIVE_CALL_SHADOW_STACK as usize,
        saved_shadow,
    );
    Some(frame.get(general(0)) as usize)
}

/// Where in the program a fault at host `pc` happened when `pc` is in translated code or a
/// bridge: the trap's cause (`signal` is `SIGTRAP` for a `brk`) and the guest address, from
/// `x16` for refused sites and undiscovered targets, else from the back-map (0 in a bridge).
pub fn guest_fault(signal: i32, pc: u64, x16: u64) -> Option<(Option<TrapCause>, u64)> {
    let translation = current()?;
    let offset = pc.checked_sub(translation.code as u64)?;
    if offset >= translation.mapped as u64 {
        return None;
    }
    let cause = match signal == crate::fault::SIGTRAP && pc.is_multiple_of(4) {
        // SAFETY: `pc` is inside the translated code's mapping, which is readable.
        true => TrapCause::of(unsafe { (pc as *const u32).read() }),
        false => None,
    };
    let guest = match cause {
        Some(cause) if cause.address_in_x16() => x16,
        _ if offset < translation.code_bytes as u64 => {
            translated::guest_rva(&translation.back_map, offset as u32)
                .map_or(0, |rva| translation.guest_base + u64::from(rva))
        }
        _ => 0,
    };
    Some((cause, guest))
}

struct Disk;

fn path(name: &str) -> Vec<u8> {
    let mut path = Vec::with_capacity(policy::DIRECTORY.len() + name.len() + 2);
    path.extend_from_slice(policy::DIRECTORY.as_bytes());
    path.push(b'/');
    path.extend_from_slice(name.as_bytes());
    path.push(0);
    path
}

fn directory() -> Vec<u8> {
    let mut directory = Vec::from(policy::DIRECTORY.as_bytes());
    directory.push(0);
    directory
}

fn make_directories() {
    let full = policy::DIRECTORY.as_bytes();
    for (index, &byte) in full.iter().enumerate().skip(1) {
        if byte == b'/' {
            let mut parent = Vec::from(&full[..index]);
            parent.push(0);
            let _ = linux::make_directory(&parent);
        }
    }
    let _ = linux::make_directory(&directory());
}

impl CacheFiles for Disk {
    fn read(&mut self, name: &str) -> Option<Vec<u8>> {
        let fd = linux::open(&path(name), linux::O_RDONLY).ok()?;
        let size = linux::fstat(fd)
            .ok()
            .map(|words| words[linux::STAT_SIZE_WORD] as usize);
        let mut bytes = Vec::new();
        if let Some(size) = size {
            bytes.resize(size, 0);
            let mut filled = 0;
            while filled < size {
                match linux::read(fd, &mut bytes[filled..]) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => filled += read,
                }
            }
            bytes.truncate(filled);
        }
        let _ = linux::close(fd);
        size.map(|_| bytes)
    }

    fn create(&mut self, name: &str, bytes: &[u8]) -> bool {
        make_directories();
        let flags = linux::O_WRONLY | linux::O_CREAT | linux::O_EXCL;
        let Ok(fd) = linux::open(&path(name), flags) else {
            return false;
        };
        let written = linux::write_all(fd, bytes).is_ok();
        linux::close(fd).is_ok() && written
    }

    fn rename(&mut self, from: &str, to: &str) -> bool {
        linux::rename(&path(from), &path(to), false).is_ok()
    }

    fn remove(&mut self, name: &str) {
        let _ = linux::remove(&path(name), 0);
    }

    fn list(&mut self) -> Vec<policy::CachedFile> {
        let mut files = Vec::new();
        let Ok(fd) = linux::open(&directory(), linux::O_RDONLY | linux::O_DIRECTORY) else {
            return files;
        };
        let mut buffer = [0u8; DIRECTORY_BUFFER_BYTES];
        while let Ok(filled) = linux::directory_entries(fd, &mut buffer) {
            if filled == 0 {
                break;
            }
            let mut at = 0;
            while at + DIRENT_NAME < filled {
                let length = usize::from(u16::from_le_bytes([
                    buffer[at + DIRENT_RECORD_LENGTH],
                    buffer[at + DIRENT_RECORD_LENGTH + 1],
                ]));
                if length == 0 {
                    break;
                }
                let record = &buffer[at..(at + length).min(filled)];
                at += length;
                let name = &record[DIRENT_NAME.min(record.len())..];
                let name = &name[..name.iter().position(|&b| b == 0).unwrap_or(name.len())];
                if name == b"." || name == b".." {
                    continue;
                }
                let Ok(text) = core::str::from_utf8(name) else {
                    continue;
                };
                let mut stat_name = Vec::from(name);
                stat_name.push(0);
                let Ok(words) = linux::stat_at(fd, &stat_name) else {
                    continue;
                };
                files.push(policy::CachedFile {
                    name: String::from(text),
                    bytes: words[linux::STAT_SIZE_WORD],
                    last_launch: words[linux::STAT_MODIFY_TIME_WORD],
                    in_use: false,
                });
            }
        }
        let _ = linux::close(fd);
        files
    }

    fn touch(&mut self, name: &str) {
        let _ = linux::touch(&path(name));
    }

    fn cap(&mut self) -> u64 {
        make_directories();
        linux::file_system(&directory()).map_or(0, |words| {
            policy::cap_bytes(words[STATFS_BLOCKS].saturating_mul(words[STATFS_BLOCK_SIZE]))
        })
    }

    fn now(&mut self) -> u64 {
        linux::clock_nanoseconds(linux::CLOCK_REALTIME) / NANOSECONDS_PER_SECOND
    }
}
