//! Running an x86_64 program on an arm64 computer (D191, slice 39e2b): before the program's
//! fixups are applied, [`prepare`] loads its translation from the cache
//! (`/var/cache/eggos/translate`) or translates it, with an import stub for every bound pointer
//! that may be a function's call slot, and stores it; after binding, [`install`] maps the
//! translated code with a System V bridge per function import (a variadic one for the layer's
//! variadic functions, which then read a tagged System V `va_list`), points those slots at
//! their own guest addresses so guest calls reach the bridges through the entries table, and
//! leaves data imports at their native addresses. [`call_guest`] enters translated code
//! through the entry trampoline (the program's `main`) with the calling thread's guest state,
//! and [`install_fault_handler`] ends a program whose translated code traps or faults, or
//! whose function the layer called natively, with a line naming the guest address. The
//! layer calls the program's other functions (handlers, comparators, thread starts, blocks,
//! Objective-C methods) through [`native_callable`]'s thunks (slice 39e2c), and its signal
//! handlers through [`call_signal_handler`].

use crate::libsystem::{self, Exclusive};
use crate::linux;
use crate::macho::{MachO, read_u64};
use crate::translated::{self, PointerFixups};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::ffi::c_int;
use core::sync::atomic::{AtomicPtr, AtomicU64, Ordering};
use eggos_runtime::startup::{Startup, auxv};
use eggos_translate::cache::{self, CacheFile};
use eggos_translate::image::bridge;
use eggos_translate::image::{ImageInput, TranslatedImage, translate_image};
use eggos_translation_loader::cache_files;
use eggos_translation_loader::callback_thunks;
use eggos_translation_loader::disk::Disk;
use eggos_translation_loader::install_record::{self, Origin};
use eggos_translation_loader::installed::{self, InstalledCode, StubBridge};
use eggos_translation_loader::trap::codegen_mode;

const PROGRAM_HEADER_SIZE: usize = 56;
const ENTRY_WORDS_BYTES: u64 = 8;
const SIGILL: usize = 4;
const SIGTRAP: usize = 5;
const SIGBUS: usize = 7;
const SIGFPE: usize = 8;
const SIGSEGV: usize = 11;
const SIGNAL_EXIT_BASE: i32 = 128;
const SA_SIGINFO: usize = 4;
const SIGSET_SIZE: usize = 8;
const SIG_DFL: usize = 0;
const SIGINFO_CODE: usize = 8;
const SIGINFO_ADDRESS: usize = 16;
const UCONTEXT_REGISTERS: usize = 184;
const UCONTEXT_PC: usize = 440;
const STANDARD_ERROR: usize = 2;
const GUEST_STACK_REGISTER: usize = 4;
const SHADOW_STACK_REGISTER: usize = 20;
const GUEST_STACK_REACH: u64 = 8 << 20;
const GUEST_RED_ZONE: u64 = 128;

struct Translation {
    code: InstalledCode,
    program: &'static [u8],
}

static TRANSLATION: AtomicPtr<Translation> = AtomicPtr::new(core::ptr::null_mut());

fn current() -> Option<&'static Translation> {
    let translation = TRANSLATION.load(Ordering::Acquire);
    // SAFETY: `install` stores a leaked `Box` once, before the program runs, and nothing
    // frees or changes it.
    unsafe { translation.as_ref() }
}

/// The program's translation, from the cache or made now (and then stored), for `file` (the
/// x86_64 slice) laid out in `image` before its fixups, whose bound pointers and rebase
/// targets are `fixups`; `capabilities` is the CPU's `AT_HWCAP`.
pub fn prepare(
    file: &[u8],
    program: &MachO<'_>,
    image: &[u8],
    fixups: &PointerFixups<'_>,
    capabilities: u64,
) -> Result<TranslatedImage, &'static str> {
    translation(file, program, image, fixups, capabilities).map(|(file, _)| file.image)
}

/// The installer's translation ahead of the first launch (`--translate`): [`prepare`]'s,
/// as the record the layer prints.
pub fn install_record(
    file: &[u8],
    program: &MachO<'_>,
    image: &[u8],
    fixups: &PointerFixups<'_>,
    capabilities: u64,
) -> Result<Vec<u8>, &'static str> {
    let (file, origin) = translation(file, program, image, fixups, capabilities)?;
    Ok(install_record::record(&file, origin))
}

fn translation(
    file: &[u8],
    program: &MachO<'_>,
    image: &[u8],
    fixups: &PointerFixups<'_>,
    capabilities: u64,
) -> Result<(CacheFile, Origin), &'static str> {
    let mode = codegen_mode(capabilities);
    let digest = cache::input_digest(file);
    let key = cache::cache_key(&digest, mode);
    let base = program.base_address();
    let mut disk = Disk;
    if let Some(cached) = cache_files::load(&mut disk, &key)
        && cached.image_base == base
    {
        return Ok((cached, Origin::Cached));
    }
    let sections = translated::sections(program, image);
    let seeds = translated::seeds(program, &sections, &fixups.rebase_targets);
    let input = ImageInput {
        image_base: base,
        sections: &sections,
        seeds: &seeds,
    };
    let mut translation = translate_image(&input, mode)
        .map_err(|_| "EggOS could not translate this x86_64 program's code")?;
    let stubs: Vec<u32> = translated::call_slots(&fixups.binds)
        .iter()
        .map(|slot| slot.rva)
        .collect();
    bridge::add_import_stubs(&mut translation, &stubs)
        .map_err(|_| "EggOS could not translate this x86_64 program's imports")?;
    let file = CacheFile {
        input_digest: digest,
        mode,
        image_base: base,
        image: translation,
    };
    let bytes = cache::serialize(&file);
    let stored = cache_files::store(&mut disk, &key, &bytes, linux::process_id() as u64);
    let origin = match stored {
        true => Origin::Stored(bytes.len()),
        false => Origin::NotStored(bytes.len()),
    };
    Ok((file, origin))
}

/// Where the layer's own code lies, from its program headers; nothing if they cannot be read.
pub fn layer_code(startup: &Startup) -> (u64, u64) {
    let headers = startup.auxiliary_value(auxv::PHDR).unwrap_or(0);
    let count = startup.auxiliary_value(auxv::PHNUM).unwrap_or(0);
    let size = startup.auxiliary_value(auxv::PHENT).unwrap_or(0);
    if headers == 0 || size != PROGRAM_HEADER_SIZE {
        return (0, 0);
    }
    // SAFETY: the kernel maps the layer's program headers where `AT_PHDR` says, for the
    // process's life.
    let bytes = unsafe { core::slice::from_raw_parts(headers as *const u8, count * size) };
    eggos_translation_loader::executable_range(bytes, headers as u64).unwrap_or((0, 0))
}

fn bridge_words(native: u64, name: &[u8]) -> Option<Vec<u32>> {
    if translated::is_stack_probe(name) {
        return bridge::return_bridge(0).ok();
    }
    let first = native as *const u32;
    // SAFETY: `native` lies in the layer's executable code, which is mapped and readable, at
    // least two words before its end.
    let opening = unsafe { first.read() };
    // SAFETY: as above, for the second word.
    let following = unsafe { first.wrapping_add(1).read() };
    match translated::variadic_entry([opening, following]) {
        Some((named, offset)) => {
            bridge::sysv_variadic_bridge(named, native.wrapping_add_signed(offset), 0).ok()
        }
        None => {
            let moves = bridge::sysv_argument_moves(translated::bridge_arguments(name));
            bridge::sysv_forward_bridge(&moves, native, 0).ok()
        }
    }
}

/// Maps the translated code of `program` (named `name`), loaded at `base` and laid out in
/// `image` with its fixups applied, with a System V bridge per bound pointer of `fixups` that
/// holds the address of a function in the layer's code (`layer_code`), and points each of
/// those slots at its own guest address; the others keep their native addresses.
pub fn install(
    name: &'static [u8],
    program: &MachO<'_>,
    image: &mut [u8],
    base: usize,
    fixups: &PointerFixups<'_>,
    translation: TranslatedImage,
    layer_code: (u64, u64),
) -> Result<(), &'static str> {
    const FAILED: &str = "EggOS could not map this x86_64 program's translated code";
    let (code_start, code_end) = layer_code;
    let mut bridges = Vec::new();
    for slot in translated::call_slots(&fixups.binds) {
        let native = read_u64(image, slot.rva as usize).ok_or(FAILED)?;
        if native < code_start || native.saturating_add(ENTRY_WORDS_BYTES) > code_end {
            continue;
        }
        let words = bridge_words(native, slot.name).ok_or(FAILED)?;
        bridges.push(StubBridge {
            stub_rva: slot.rva,
            words,
        });
    }
    let code = installed::install(
        translation,
        program.base_address(),
        base as u64,
        program.mapped_size(),
        &mut bridges,
        &[],
    )
    .map_err(|_| FAILED)?;
    for bridged in &bridges {
        let slot = bridged.stub_rva as usize;
        let own = base as u64 + u64::from(bridged.stub_rva);
        image[slot..slot + 8].copy_from_slice(&own.to_le_bytes());
    }
    let translation = Box::new(Translation {
        code,
        program: name,
    });
    TRANSLATION.store(Box::into_raw(translation), Ordering::Release);
    Ok(())
}

fn thread_block() -> Option<usize> {
    let thread = libsystem::thread::current();
    // SAFETY: the calling thread's control block lives as long as the thread, and only the
    // thread itself reads or writes this field.
    let slot = unsafe { &mut (*thread).translated_thread };
    if *slot == 0 {
        let gs_base = libsystem::thread::specific_values() as u64;
        *slot = installed::new_guest_thread(gs_base)?.block;
    }
    Some(*slot)
}

/// Calls the program's translated function at `address` with `arguments` in `rdi`, `rsi`,
/// `rdx`, `rcx`, `r8` and `r9`, and returns `rax`; `None` when the program is not translated,
/// `address` is not in its image or the thread's guest state cannot be made.
///
/// # Safety
/// `address` must be a function of the program that takes the integer or pointer
/// `arguments` given (at most six), with values it accepts.
pub unsafe fn call_guest(address: u64, arguments: &[u64]) -> Option<u64> {
    let translation = current()?;
    if !translation.code.holds_guest(address) {
        return None;
    }
    let block = thread_block()?;
    let mut registers = [(0u8, 0u64); bridge::SYSV_ARGUMENT_REGISTERS.len()];
    let count = arguments.len().min(registers.len());
    for (register, (&number, &value)) in registers
        .iter_mut()
        .zip(bridge::SYSV_ARGUMENT_REGISTERS.iter().zip(arguments))
    {
        *register = (number, value);
    }
    let gs_base = libsystem::thread::specific_values() as u64;
    // SAFETY: the caller's contract, on this thread's own block.
    Some(unsafe {
        translation
            .code
            .call_guest(block, gs_base, address, &registers[..count])
    })
}

struct Thunks {
    page: usize,
    used: usize,
    by_guest: BTreeMap<u64, usize>,
}

static THUNKS: Exclusive<Thunks> = Exclusive::new(Thunks {
    page: 0,
    used: callback_thunks::THUNKS_PER_PAGE,
    by_guest: BTreeMap::new(),
});

impl Thunks {
    fn thunk(&mut self, guest: u64) -> Option<usize> {
        if let Some(&thunk) = self.by_guest.get(&guest) {
            return Some(thunk);
        }
        if self.used == callback_thunks::THUNKS_PER_PAGE {
            self.page =
                installed::callback_thunk_page(callback_entry as *const () as usize as u64)?;
            self.used = 0;
        }
        let index = self.used;
        let slot = self.page + callback_thunks::guest_slot_offset(index);
        // SAFETY: the slot is an aligned word of the thunk page's writable data page, which
        // stays mapped for the process's life; no thunk reads it before it is handed out.
        unsafe { AtomicU64::from_ptr(slot as *mut u64) }.store(guest, Ordering::Release);
        self.used += 1;
        let thunk = self.page + index * callback_thunks::THUNK_BYTES;
        self.by_guest.insert(guest, thunk);
        Some(thunk)
    }
}

extern "C" fn callback_entry(
    guest: u64,
    first: u64,
    second: u64,
    third: u64,
    fourth: u64,
    fifth: u64,
    sixth: u64,
) -> u64 {
    let arguments = [first, second, third, fourth, fifth, sixth];
    // SAFETY: only the thunk `native_callable` made for `guest` branches here, and the layer
    // calls that thunk where the program expects its function to be called, with the
    // arguments the function takes.
    match unsafe { call_guest(guest, &arguments) } {
        Some(result) => result,
        None => {
            libsystem::write_error(b"macos: no memory to call the program's translated code\n");
            linux::exit(SIGNAL_EXIT_BASE + SIGSEGV as i32)
        }
    }
}

/// A native function that calls the program's translated function at `address` (a thunk made
/// once per address), or `address` itself when it is not in the translated program: how every
/// place in the layer that is given one of the program's functions (a handler, comparator,
/// thread start, block, Objective-C method) calls it. Up to six integer or pointer arguments
/// and an integer result pass through.
pub fn native_callable(address: usize) -> usize {
    let Some(translation) = current() else {
        return address;
    };
    if !translation.code.holds_guest(address as u64) {
        return address;
    }
    THUNKS
        .with(|thunks| thunks.thunk(address as u64))
        .unwrap_or(address)
}

/// Calls the program's signal handler `handler` with `arguments` when it is translated code
/// (false otherwise, and the caller calls it natively). When the signal interrupted translated
/// code (`context` is the kernel's `ucontext_t`), the handler's guest frames go below the
/// interrupted guest stack and its red zone, not below where the thread last called the layer.
/// Takes no lock, so it is safe in a signal handler.
pub fn call_signal_handler(handler: usize, arguments: [u64; 3], context: *mut u8) -> bool {
    let Some(translation) = current() else {
        return false;
    };
    if !translation.code.holds_guest(handler as u64) {
        return false;
    }
    let Some(block) = thread_block() else {
        return false;
    };
    let stack_field = block + bridge::thread::NATIVE_CALL_GUEST_STACK as usize;
    let shadow_field = block + bridge::thread::NATIVE_CALL_SHADOW_STACK as usize;
    let field = |at: usize| {
        // SAFETY: the fields are aligned words of this thread's own block.
        unsafe { AtomicU64::from_ptr(at as *mut u64) }
    };
    let saved = (
        field(stack_field).load(Ordering::Relaxed),
        field(shadow_field).load(Ordering::Relaxed),
    );
    if !context.is_null() {
        let pc = word(context, UCONTEXT_PC);
        let code = translation.code.code as u64;
        let guest_stack = word(context, UCONTEXT_REGISTERS + 8 * GUEST_STACK_REGISTER);
        if pc.wrapping_sub(code) < translation.code.mapped as u64
            && guest_stack < saved.0
            && saved.0 - guest_stack < GUEST_STACK_REACH
        {
            field(stack_field).store(guest_stack - GUEST_RED_ZONE, Ordering::Relaxed);
            let shadow = word(context, UCONTEXT_REGISTERS + 8 * SHADOW_STACK_REGISTER);
            field(shadow_field).store(shadow, Ordering::Relaxed);
        }
    }
    // SAFETY: the program installed `handler` as a signal handler, which takes the signal
    // number, a `siginfo_t` and a context.
    let _ = unsafe { call_guest(handler as u64, &arguments) };
    field(stack_field).store(saved.0, Ordering::Relaxed);
    field(shadow_field).store(saved.1, Ordering::Relaxed);
    true
}

fn word(context: *mut u8, offset: usize) -> u64 {
    // SAFETY: the kernel passes the handler a `ucontext_t`, which holds the interrupted
    // registers at these offsets.
    unsafe { context.wrapping_add(offset).cast::<u64>().read() }
}

fn completed_misaligned_access(code: &InstalledCode, pc: u64, context: *mut u8) -> bool {
    let registers = context.wrapping_add(UCONTEXT_REGISTERS).cast::<[u64; 32]>();
    // SAFETY: the kernel's `ucontext_t` for this signal holds `x0` to `x30` and then `sp`,
    // 8-byte aligned, which `rt_sigreturn` loads back; nothing else uses them meanwhile.
    let registers = unsafe { &mut *registers };
    // SAFETY: they are the faulting thread's registers at `pc`.
    if !unsafe { code.complete_misaligned_access(pc, registers) } {
        return false;
    }
    let resume = context.wrapping_add(UCONTEXT_PC).cast::<u64>();
    // SAFETY: as above, for the `pc` it holds: the program resumes at the next instruction.
    unsafe { resume.write(pc + 4) };
    true
}

extern "C" fn on_fault(signal: c_int, info: *const u8, context: *mut u8) {
    // SAFETY: the kernel calls this handler (`SA_SIGINFO`) with its `siginfo_t`, whose
    // `si_code` is the third 32-bit word.
    let code = unsafe { info.wrapping_add(SIGINFO_CODE).cast::<i32>().read() };
    // SAFETY: as above; a fault's `si_addr` is the 64-bit word at 16.
    let address = unsafe { info.wrapping_add(SIGINFO_ADDRESS).cast::<u64>().read() };
    let pc = word(context, UCONTEXT_PC);
    let x16 = word(context, UCONTEXT_REGISTERS + 16 * 8);
    if let Some(translation) = current()
        && code > 0
    {
        if signal == SIGBUS as c_int && completed_misaligned_access(&translation.code, pc, context)
        {
            return;
        }
        let image = (translation.code.guest_base, translation.code.guest_size);
        let trapped = signal == SIGTRAP as c_int;
        let ended = match translation.code.guest_fault(trapped, pc, x16) {
            Some((cause, guest)) => Some((
                translated::fault_report(translation.program, image, cause, guest),
                translated::fault_signal(cause, signal),
            )),
            None if translation.code.holds_guest(pc) && address == pc => Some((
                translated::callback_report(translation.program, image, pc),
                signal,
            )),
            None => None,
        };
        if let Some((line, ended_by)) = ended {
            libsystem::stdio::flush_all();
            let _ = linux::write_all(STANDARD_ERROR, line.as_bytes());
            linux::exit(SIGNAL_EXIT_BASE + ended_by);
        }
    }
    set_action(signal as usize, SIG_DFL);
}

fn set_action(signal: usize, handler: usize) {
    let words = [handler, SA_SIGINFO, 0, 0];
    // SAFETY: `rt_sigaction` reads one `struct sigaction` from the borrowed words.
    let _ = unsafe {
        linux::syscall(
            linux::number::RT_SIGACTION,
            [signal, words.as_ptr() as usize, 0, SIGSET_SIZE, 0, 0],
        )
    };
}

/// Makes the faults of translated code end the program with a line naming the guest address;
/// any other fault is delivered again with its default action. A handler the program installs
/// for one of these signals replaces this one.
pub fn install_fault_handler() {
    for signal in [SIGILL, SIGTRAP, SIGBUS, SIGFPE, SIGSEGV] {
        set_action(signal, on_fault as *const () as usize);
    }
}
