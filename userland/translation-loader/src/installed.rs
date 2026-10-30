//! A translation mapped into the process ([`install`]): the translated code, a forward bridge
//! per import stub and, for the Windows layer, the procedure trampolines, made read-only and
//! executable once written, with the entries table beside them. [`InstalledCode::call_guest`]
//! calls translated code from native code through the entry trampoline, on a thread whose
//! guest state [`new_guest_thread`] made, and [`InstalledCode::guest_fault`] tells a fault
//! handler where in the guest a trap or fault in translated code happened.

use crate::callback_thunks;
use crate::trap::{self, TrapCause};
use alloc::vec::Vec;
use eggos_runtime::syscall::call;
use eggos_translate::codegen::state;
use eggos_translate::image::bridge::{self, thread};
use eggos_translate::image::runtime::{context, shadow_stack};
use eggos_translate::image::{BackMapEntry, TranslatedImage};

const MMAP: usize = 222;
const MPROTECT: usize = 226;
const PROT_READ: usize = 1;
const PROT_WRITE: usize = 2;
const PROT_EXEC: usize = 4;
const MAP_PRIVATE: usize = 0x02;
const MAP_ANONYMOUS: usize = 0x20;
const PAGE_SIZE: usize = 4096;
const BRIDGE_ALIGNMENT: usize = 16;
const GUEST_STACK_BYTES: usize = 8 << 20;
const GUEST_STACK_HEADROOM: u64 = 256;
const CALL_FRAME_BYTES: u64 = 64;
const THREAD_BLOCK_BYTES: usize = 4096;
const INITIAL_MXCSR: u32 = 0x1F80;
const INITIAL_X87_CONTROL: u16 = 0x037F;
const RSP: u8 = 4;
const RAX: u8 = 0;

/// Why a translation could not be installed: the process is out of memory, or the
/// translation lacks a stub it was made with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstallError;

/// The forward bridge an import stub jumps to: the stub's relative address (a key of the
/// entries table) and the bridge's words, whose last two hold the dispatch's address once
/// installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StubBridge {
    /// The stub's guest relative address.
    pub stub_rva: u32,
    /// The bridge's AArch64 words.
    pub words: Vec<u32>,
}

/// A translation mapped into the process.
#[derive(Debug)]
pub struct InstalledCode {
    /// Where the guest image is loaded.
    pub guest_base: u64,
    /// The guest image's mapped size.
    pub guest_size: u64,
    /// The translated code's start.
    pub code: usize,
    /// The bytes of translated code before the bridges.
    pub code_bytes: usize,
    /// The executable mapping's length (code, bridges and trampolines).
    pub mapped: usize,
    /// The runtime entries table, which `x19` points at.
    pub entries: usize,
    /// The entry trampoline.
    pub enter: usize,
    /// The indirect dispatch.
    pub dispatch: u64,
    /// The writable page of procedure slots after the code (0 without procedures).
    pub procedures: usize,
    back_map: Vec<BackMapEntry>,
}

fn map(length: usize) -> Result<usize, InstallError> {
    // SAFETY: an anonymous private mapping at an address the kernel picks replaces nothing.
    let made = unsafe {
        call(
            MMAP,
            [
                0,
                length,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS,
                usize::MAX,
                0,
            ],
        )
    };
    usize::try_from(made).map_err(|_| InstallError)
}

fn protect(start: usize, length: usize, protection: usize) -> Result<(), InstallError> {
    // SAFETY: the callers change only mappings this module made and has finished writing.
    let made = unsafe { call(MPROTECT, [start, length, protection, 0, 0, 0]) };
    usize::try_from(made).map(|_| ()).map_err(|_| InstallError)
}

fn write_u64(address: usize, value: u64) {
    // SAFETY: every caller passes an 8-byte aligned address in a writable mapping this module
    // made, which stays mapped for the process's life.
    unsafe { (address as *mut u64).write(value) };
}

fn read_u64(address: usize) -> u64 {
    // SAFETY: as for `write_u64`, for reading.
    unsafe { (address as *const u64).read() }
}

fn put_words(host: &mut [u8], at: usize, words: &[u32]) {
    for (byte, word) in (at..).step_by(4).zip(words) {
        host[byte..byte + 4].copy_from_slice(&word.to_le_bytes());
    }
}

/// Cleans the data cache and invalidates the instruction cache over `length` bytes from
/// `start`, so code written there runs as written.
pub fn synchronize_instruction_cache(start: usize, length: usize) {
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

/// Maps `words` on pages of their own, read-only and executable once written; the address
/// of the first word.
pub fn executable_words(words: &[u32]) -> Option<usize> {
    let bytes = (words.len() * 4).next_multiple_of(PAGE_SIZE);
    let page = map(bytes).ok()?;
    // SAFETY: `page` is a new private mapping of `bytes` zeroed bytes, room for every word.
    let host = unsafe { core::slice::from_raw_parts_mut(page as *mut u8, bytes) };
    put_words(host, 0, words);
    synchronize_instruction_cache(page, bytes);
    protect(page, bytes, PROT_READ | PROT_EXEC).ok()?;
    Some(page)
}

/// Maps a page of callback thunks ([`callback_thunks::page_words`]), read-only and executable,
/// followed by its writable data page with `entry` in [`callback_thunks::ENTRY_SLOT`]; the
/// code page's address. The caller writes a thunk's guest address in its slot before handing
/// the thunk out.
pub fn callback_thunk_page(entry: u64) -> Option<usize> {
    let page = map(2 * callback_thunks::PAGE_BYTES).ok()?;
    // SAFETY: `page` is a new private mapping of two zeroed pages that only this function uses
    // until the first is made executable.
    let host =
        unsafe { core::slice::from_raw_parts_mut(page as *mut u8, callback_thunks::PAGE_BYTES) };
    put_words(host, 0, &callback_thunks::page_words());
    write_u64(
        page + callback_thunks::guest_slot_offset(callback_thunks::ENTRY_SLOT),
        entry,
    );
    synchronize_instruction_cache(page, callback_thunks::PAGE_BYTES);
    protect(page, callback_thunks::PAGE_BYTES, PROT_READ | PROT_EXEC).ok()?;
    Some(page)
}

/// Maps `translation`, made for a guest image preferred at `preferred_base` and loaded at
/// `guest_base` (`guest_size` bytes): the code with its guest addresses moved by the load
/// difference, each stub's bridge after it with the stub pointed at it, a procedure
/// trampoline per `procedure_rvas` stub reading its slot in a writable page after the code,
/// and the entries table.
pub fn install(
    mut translation: TranslatedImage,
    preferred_base: u64,
    guest_base: u64,
    guest_size: u64,
    bridges: &mut [StubBridge],
    procedure_rvas: &[u32],
) -> Result<InstalledCode, InstallError> {
    let code_bytes = translation.code.len();
    let bridge_bytes: usize = bridges
        .iter()
        .map(|bridge| (bridge.words.len() * 4).next_multiple_of(BRIDGE_ALIGNMENT))
        .sum();
    let bridges_start = code_bytes.next_multiple_of(BRIDGE_ALIGNMENT);
    let trampolines_start = bridges_start + bridge_bytes;
    let trampoline_bytes =
        (bridge::PROCEDURE_TRAMPOLINE_BYTES as usize).next_multiple_of(BRIDGE_ALIGNMENT);
    let mapped =
        (trampolines_start + procedure_rvas.len() * trampoline_bytes).next_multiple_of(PAGE_SIZE);
    let procedure_page = match procedure_rvas.is_empty() {
        true => 0,
        false => PAGE_SIZE,
    };
    let total = mapped + procedure_page;
    let code = map(total)?;
    // SAFETY: `code` is a new private mapping of `total` zeroed bytes that only this function
    // uses until its first `mapped` bytes are made executable.
    let host = unsafe { core::slice::from_raw_parts_mut(code as *mut u8, total) };
    host[..code_bytes].copy_from_slice(&translation.code);
    let delta = guest_base.wrapping_sub(preferred_base);
    for &offset in &translation.guest_address_literals {
        let at = offset as usize;
        let mut literal = [0u8; 8];
        literal.copy_from_slice(&host[at..at + 8]);
        let moved = u64::from_le_bytes(literal).wrapping_add(delta);
        host[at..at + 8].copy_from_slice(&moved.to_le_bytes());
    }
    let dispatch = (code + translation.dispatch as usize) as u64;
    let mut at = bridges_start;
    for stub_bridge in bridges.iter_mut() {
        let words = &mut stub_bridge.words;
        let length = words.len();
        if length < 2 {
            return Err(InstallError);
        }
        words[length - 2] = dispatch as u32;
        words[length - 1] = (dispatch >> 32) as u32;
        put_words(host, at, words);
        let stub = translation
            .entries
            .lookup(stub_bridge.stub_rva)
            .ok_or(InstallError)? as usize;
        let literal = stub + bridge::IMPORT_STUB_LITERAL as usize;
        host[literal..literal + 8].copy_from_slice(&((code + at) as u64).to_le_bytes());
        at += (length * 4).next_multiple_of(BRIDGE_ALIGNMENT);
    }
    let procedures = match procedure_rvas.is_empty() {
        true => 0,
        false => code + mapped,
    };
    for (index, &rva) in procedure_rvas.iter().enumerate() {
        let at = trampolines_start + index * trampoline_bytes;
        let words = bridge::procedure_trampoline((procedures + 8 * index) as u64);
        put_words(host, at, &words);
        let stub = translation.entries.lookup(rva).ok_or(InstallError)? as usize;
        let literal = stub + bridge::IMPORT_STUB_LITERAL as usize;
        host[literal..literal + 8].copy_from_slice(&((code + at) as u64).to_le_bytes());
    }
    let table = translation.entries.runtime_bytes(guest_base, code as u64);
    let entries_bytes = table.len().next_multiple_of(PAGE_SIZE);
    let entries = map(entries_bytes)?;
    // SAFETY: `entries` is a new private mapping of at least `table.len()` bytes.
    unsafe { core::slice::from_raw_parts_mut(entries as *mut u8, table.len()) }
        .copy_from_slice(&table);
    protect(entries, entries_bytes, PROT_READ)?;
    synchronize_instruction_cache(code, mapped);
    protect(code, mapped, PROT_READ | PROT_EXEC)?;
    Ok(InstalledCode {
        guest_base,
        guest_size,
        code,
        code_bytes,
        mapped,
        entries,
        enter: code + translation.enter as usize,
        dispatch,
        procedures,
        back_map: core::mem::take(&mut translation.back_map),
    })
}

/// A thread's guest state, made on its first call into translated code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuestThread {
    /// The thread block: the state block, then the bridge fields.
    pub block: usize,
    /// The guest stack's lowest address.
    pub stack_limit: u64,
    /// The guest stack's top.
    pub stack_base: u64,
}

/// Makes a thread's guest state: a block (state block with MXCSR `0x1F80`, the x87 control
/// word `0x037F` and `gs_base`, then the bridge fields), a 64 KiB shadow ring aligned to
/// 64 KiB and an 8 MiB guest stack.
pub fn new_guest_thread(gs_base: u64) -> Option<GuestThread> {
    let block = map(THREAD_BLOCK_BYTES).ok()?;
    let ring_bytes = shadow_stack::BYTES as usize;
    let ring = map(2 * ring_bytes).ok()?;
    let ring = shadow_stack::initial(ring.next_multiple_of(ring_bytes) as u64)?;
    let stack = map(GUEST_STACK_BYTES).ok()?;
    let top = (stack + GUEST_STACK_BYTES) as u64;
    write_u64(block + state::GS_BASE as usize, gs_base);
    // SAFETY: the state block's MXCSR field is aligned and inside the new block.
    unsafe { ((block + state::MXCSR as usize) as *mut u32).write(INITIAL_MXCSR) };
    // SAFETY: as above, for the x87 control word.
    unsafe { ((block + state::X87_CONTROL as usize) as *mut u16).write(INITIAL_X87_CONTROL) };
    write_u64(
        block + thread::NATIVE_CALL_GUEST_STACK as usize,
        top - GUEST_STACK_HEADROOM,
    );
    write_u64(block + thread::NATIVE_CALL_SHADOW_STACK as usize, ring);
    Some(GuestThread {
        block,
        stack_limit: stack as u64,
        stack_base: top,
    })
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

fn general(register: u8) -> u32 {
    context::GENERAL + 8 * u32::from(register)
}

impl InstalledCode {
    /// Whether `address` is in the guest image.
    pub fn holds_guest(&self, address: u64) -> bool {
        address.wrapping_sub(self.guest_base) < self.guest_size
    }

    /// Calls the guest function at `address` on the thread whose block is `block` (from
    /// [`new_guest_thread`]), with `registers` set (x86-64 register numbers and values) and
    /// `gs_base` as the GS base, through the entry trampoline; returns `rax`. The guest stack
    /// continues below where the thread last called native code, with the guest base pushed as
    /// the return address under a 64-byte frame, so the call works from any depth of nested
    /// calls between native and translated code.
    ///
    /// # Safety
    /// `address` must be a function of the guest image that accepts the registers given, and
    /// `block` the calling thread's own block.
    pub unsafe fn call_guest(
        &self,
        block: usize,
        gs_base: u64,
        address: u64,
        registers: &[(u8, u64)],
    ) -> u64 {
        let saved_stack = read_u64(block + thread::NATIVE_CALL_GUEST_STACK as usize);
        let saved_shadow = read_u64(block + thread::NATIVE_CALL_SHADOW_STACK as usize);
        let entry_stack = (saved_stack & !15) - CALL_FRAME_BYTES - 8;
        write_u64(entry_stack as usize, self.guest_base);
        let mut frame = Context([0; context::SIZE as usize / 8]);
        for &(register, value) in registers {
            frame.set(general(register), value);
        }
        frame.set(general(RSP), entry_stack);
        frame.set(context::RIP, address);
        frame.set(context::GS_BASE, gs_base);
        frame.set(context::ENTRIES, self.entries as u64);
        frame.set(context::SHADOW_STACK, saved_shadow);
        frame.set(context::STATE, block as u64);
        let control: u64;
        // SAFETY: reading FPCR changes nothing.
        unsafe { core::arch::asm!("mrs {}, fpcr", out(reg) control) };
        // SAFETY: `enter` is the trampoline of the mapped translated code, an AAPCS64
        // function taking a guest context; the guest returns to the guest base pushed above.
        let enter: extern "C" fn(*mut Context) = unsafe { core::mem::transmute(self.enter) };
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
        frame.get(general(RAX))
    }

    /// Where in the guest a fault at host `pc` happened when `pc` is in the translated code,
    /// a bridge or a trampoline: the trap's cause when `trapped` (a `brk`, `SIGTRAP`) and the
    /// guest address, from `x16` for refused sites and undiscovered targets, else from the
    /// back-map (0 in a bridge).
    pub fn guest_fault(
        &self,
        trapped: bool,
        pc: u64,
        x16: u64,
    ) -> Option<(Option<TrapCause>, u64)> {
        let offset = pc.checked_sub(self.code as u64)?;
        if offset >= self.mapped as u64 {
            return None;
        }
        let cause = match trapped && pc.is_multiple_of(4) {
            // SAFETY: `pc` is inside the translated code's mapping, which is readable.
            true => TrapCause::of(unsafe { (pc as *const u32).read() }),
            false => None,
        };
        let guest = match cause {
            Some(cause) if cause.address_in_x16() => x16,
            _ if offset < self.code_bytes as u64 => trap::guest_rva(&self.back_map, offset as u32)
                .map_or(0, |rva| self.guest_base + u64::from(rva)),
            _ => 0,
        };
        Some((cause, guest))
    }
}
