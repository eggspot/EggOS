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

use crate::linux;
use crate::pe::{Pe, read_u64};
use crate::translated::{self, TrapCause};
use crate::win32::thread_environment::teb;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use eggos_translate::cache::{self, CacheFile};
use eggos_translate::image::bridge;
use eggos_translate::image::{ImageInput, TranslatedImage, translate_image};
use eggos_translation_loader::cache_files;
use eggos_translation_loader::disk::Disk;
use eggos_translation_loader::install_record::{self, Origin};
use eggos_translation_loader::installed::{self, InstalledCode, StubBridge};
use eggos_translation_loader::trap::codegen_mode;

static TRANSLATION: AtomicPtr<InstalledCode> = AtomicPtr::new(core::ptr::null_mut());

fn current() -> Option<&'static InstalledCode> {
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
    translation(file, pe, image, capabilities).map(|(file, _)| file.image)
}

/// The installer's translation ahead of the first launch (`--translate`): [`prepare`]'s,
/// as the record the layer prints.
pub fn install_record(
    file: &[u8],
    pe: &Pe<'_>,
    image: &[u8],
    capabilities: u64,
) -> Result<Vec<u8>, &'static str> {
    let (file, origin) = translation(file, pe, image, capabilities)?;
    Ok(install_record::record(&file, origin))
}

fn translation(
    file: &[u8],
    pe: &Pe<'_>,
    image: &[u8],
    capabilities: u64,
) -> Result<(CacheFile, Origin), &'static str> {
    let mode = codegen_mode(capabilities);
    let digest = cache::input_digest(file);
    let key = cache::cache_key(&digest, mode);
    let mut disk = Disk;
    if let Some(cached) = cache_files::load(&mut disk, &key)
        && cached.image_base == pe.image_base
    {
        return Ok((cached, Origin::Cached));
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
    let stored = cache_files::store(&mut disk, &key, &bytes, u64::from(linux::process_id()));
    let origin = match stored {
        true => Origin::Stored(bytes.len()),
        false => Origin::NotStored(bytes.len()),
    };
    Ok((file, origin))
}

fn write_u64(address: usize, value: u64) {
    // SAFETY: every caller passes an 8-byte aligned address in a writable mapping this module
    // made, or in the TEB, which stay mapped for the process's life.
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
    translation: TranslatedImage,
) -> Result<(), &'static str> {
    const FAILED: &str = "EggOS could not map this x64 program's translated code";
    let slots = translated::import_slots(pe, image).ok_or("the import table is damaged")?;
    let mut bridges = Vec::with_capacity(slots.len());
    for slot in &slots {
        let native = read_u64(image, slot.rva as usize).map_err(|_| FAILED)?;
        let moves = bridge::argument_moves(slot.arguments());
        let words = bridge::forward_bridge(&moves, native, 0).map_err(|_| FAILED)?;
        bridges.push(StubBridge {
            stub_rva: slot.rva,
            words,
        });
    }
    let procedure_rvas =
        translated::procedure_stub_rvas(pe.mapped_size()).ok_or("the program is too large")?;
    let installed = installed::install(
        translation,
        pe.image_base,
        base as u64,
        pe.mapped_size(),
        &mut bridges,
        &procedure_rvas,
    )
    .map_err(|_| FAILED)?;
    for slot in &slots {
        let slot_address = base as u64 + u64::from(slot.rva);
        let slot = slot.rva as usize;
        image[slot..slot + 8].copy_from_slice(&slot_address.to_le_bytes());
    }
    TRANSLATION.store(Box::into_raw(Box::new(installed)), Ordering::Release);
    Ok(())
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
    let thread = installed::new_guest_thread(teb as u64)?;
    write_u64(teb + teb::STACK_BASE, thread.stack_base);
    write_u64(teb + teb::STACK_LIMIT, thread.stack_limit);
    write_u64(slot, thread.block as u64);
    Some(thread.block)
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
    installed::executable_words(&words)
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
    if !translation.holds_guest(address as u64) {
        return None;
    }
    let teb = thread_environment_block();
    let block = thread_block(teb)?;
    let mut registers = [(0u8, 0u64); 4];
    for (register, (&number, &value)) in registers
        .iter_mut()
        .zip(bridge::WIN64_ARGUMENT_REGISTERS.iter().zip(&arguments))
    {
        *register = (number, value as u64);
    }
    // SAFETY: the caller's contract, on this thread's own block.
    let result = unsafe { translation.call_guest(block, teb as u64, address as u64, &registers) };
    Some(result as usize)
}

/// Where in the program a fault at host `pc` happened when `pc` is in translated code or a
/// bridge: the trap's cause (`signal` is `SIGTRAP` for a `brk`) and the guest address, from
/// `x16` for refused sites and undiscovered targets, else from the back-map (0 in a bridge).
pub fn guest_fault(signal: i32, pc: u64, x16: u64) -> Option<(Option<TrapCause>, u64)> {
    current()?.guest_fault(signal == crate::fault::SIGTRAP, pc, x16)
}
