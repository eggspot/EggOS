//! SPARC64 (big-endian, V9) on QEMU `sun4u`, an UltraSPARC IIi with OpenBIOS (D326).
//!
//! Boot contract: QEMU loads the ELF image at its physical address, 4 MiB, and OpenBIOS enters
//! `_start` there with its own trap table and identity mapping and `%o4` its client interface
//! (IEEE 1275). `_start` locks two 4 MiB TLB entries mapping the image at
//! [`KERNEL_VIRTUAL_OFFSET`] above it and jumps there; `sparc64_boot` asks Open Firmware for the
//! RAM, the CPU's clock and the command line ([`firmware`]); then the HAL takes the traps over
//! and enters the kernel. The machine's tree is not flattened by the firmware, so the HAL writes
//! one ([`describe_machine`]): the RAM, the CPU and the UART, which the kernel's discovery reads
//! as any other.
//!
//! Translation: the kernel half is privileged, global pages; programs run in a context of
//! their own, given afresh at every switch. The MMU's TLBs are loaded by software
//! (`exception.rs`) from page tables in the [`paging::sparc64`] format, and the direct map in
//! 4 MiB entries. The per-CPU block's address is a word of the HAL's own: QEMU's `sun4u` has one
//! CPU.

mod exception;
mod firmware;
mod fp;
mod thread;
mod timer;

use crate::arch::boot_memory;
use crate::cpu::{CpuStart, StartCpuError};
use crate::fdt::writer::Writer;
use crate::memory::PhysicalRange;
use crate::paging::{self, KernelImage};
use crate::{ExitStatus, MemoryMap, Platform};
use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use eggos_drivers::io::RegisterIo;
use eggos_drivers::serial::{Ns16550, Uart};
use exception::pstate;

core::arch::global_asm!(
    r#"
    .macro SETX symbol, scratch, register
    sethi       %hh(\symbol), \scratch
    or          \scratch, %hm(\symbol), \scratch
    sllx        \scratch, 32, \scratch
    sethi       %hi(\symbol), \register
    or          \register, %lo(\symbol), \register
    or          \register, \scratch, \register
    .endm

    .section .text.boot, "ax"
    .global _start
_start:
    mov         %o4, %l7
    rd          %pc, %l0
    srlx        %l0, 22, %l0
    sllx        %l0, 22, %l0
    mov         -1, %l1
    sllx        %l1, 31, %l1
    add         %l0, %l1, %l2
    mov         -1, %l3
    sllx        %l3, 61, %l3
    or          %l3, %l0, %l3
    or          %l3, {locked_bits}, %l3
    mov         {tag_access}, %l4
    sethi       %hi(0x400000), %l5
    .rept 2
    stxa        %l2, [%l4] {asi_dmmu}
    stxa        %l3, [%g0] {asi_dtlb_in}
    stxa        %l2, [%l4] {asi_immu}
    stxa        %l3, [%g0] {asi_itlb_in}
    membar      #Sync
    add         %l2, %l5, %l2
    add         %l3, %l5, %l3
    .endr
1:  rd          %pc, %l4
    sethi       %hi(sparc64_start_high - 1b), %l5
    or          %l5, %lo(sparc64_start_high - 1b), %l5
    add         %l4, %l5, %l4
    add         %l4, %l1, %l4
    jmpl        %l4, %g0
     nop

sparc64_start_high:
    SETX        __bss_start, %l4, %l5
    SETX        __bss_end, %l4, %l6
2:  cmp         %l5, %l6
    bgeu        %xcc, 3f
     nop
    stx         %g0, [%l5]
    ba          2b
     add        %l5, 8, %l5
3:  SETX        __stack_top, %l4, %sp
    sub         %sp, {bias} + {frame}, %sp
    call        {boot}
     mov        %l7, %o0
    SETX        sparc64_trap_table, %l4, %l5
    wrpr        %l5, 0, %tba
    wrpr        %g0, {pstate_kernel}, %pstate
    wrpr        %g0, 0, %pil
    wrpr        %g0, 0, %wstate
    wrpr        %g0, 0, %canrestore
    wrpr        %g0, 0, %otherwin
    wrpr        %g0, {windows} - 2, %cansave
    wrpr        %g0, {windows} - 1, %cleanwin
    wrpr        %g0, {pstate_kernel_mmu}, %pstate
    mov         0, %g6
    mov         0, %g7
    wrpr        %g0, {pstate_kernel}, %pstate
    SETX        __stack_top, %l4, %sp
    sub         %sp, {bias} + {frame}, %sp
    mov         0, %fp
    call        {init_timer}
     nop
    call        kernel_main
     nop
4:  ba          4b
     nop
"#,
    locked_bits = const 0x77,
    tag_access = const 0x30,
    asi_immu = const 0x50,
    asi_itlb_in = const 0x54,
    asi_dmmu = const 0x58,
    asi_dtlb_in = const 0x5c,
    bias = const thread::STACK_BIAS,
    frame = const 192,
    windows = const exception::WINDOWS,
    pstate_kernel = const pstate::KERNEL,
    pstate_kernel_mmu = const pstate::KERNEL | pstate::MMU_GLOBALS,
    boot = sym sparc64_boot,
    init_timer = sym timer::init,
);

/// The image's virtual address minus its physical address: the top 2 GiB (D33). `linker.ld`
/// uses the same value and checks it against this one.
pub(crate) const KERNEL_VIRTUAL_OFFSET: u64 = 0xffff_ffff_8000_0000;
const DIRECT_MAP_OFFSET: usize = paging::sparc64::HIGH_HALF as usize;
const DIRECT_MAP_END: u64 = 1 << 41;
const DEVICE_WINDOW: core::ops::Range<u64> = 0xffff_fc00_0000_0000..0xffff_fc80_0000_0000;
const KERNEL_LOAD_ADDRESS: u64 = 0x40_0000;
const UART_BASE: u64 = 0x1fe_0200_03f8;
const UART_CLOCK: u32 = 1_843_200;
const DEFAULT_RAM: (u64, u64) = (0, 128 << 20);

const DEVICE_TREE_BYTES: usize = 4096;

#[repr(C, align(8))]
struct DeviceTreeBuffer([u8; DEVICE_TREE_BYTES]);

static mut DEVICE_TREE_BLOB: DeviceTreeBuffer = DeviceTreeBuffer([0; DEVICE_TREE_BYTES]);
static DEVICE_TREE_STATE: AtomicU8 = AtomicU8::new(UNWRITTEN);
static DEVICE_TREE_LENGTH: AtomicUsize = AtomicUsize::new(0);
const UNWRITTEN: u8 = 0;
const WRITTEN: u8 = 1;
const FAILED: u8 = 2;

static CPU_LOCAL: AtomicUsize = AtomicUsize::new(0);

extern "C" fn sparc64_boot(client_interface: u64) {
    firmware::read(client_interface);
    if let Some(frequency) = firmware::cpu_frequency() {
        timer::set_frequency(frequency);
    }
}

fn describe_machine() -> usize {
    let address = || ((&raw const DEVICE_TREE_BLOB) as u64 - KERNEL_VIRTUAL_OFFSET) as usize;
    match DEVICE_TREE_STATE.load(Ordering::Acquire) {
        WRITTEN => return address(),
        FAILED => return 0,
        _ => {}
    }
    let mut ram = [0u64; 2 * firmware::MAX_RAM_RANGES];
    let mut ram_words = 0;
    for (start, length) in firmware::ram() {
        ram[ram_words] = start;
        ram[ram_words + 1] = length;
        ram_words += 2;
    }
    if ram_words == 0 {
        ram[0] = DEFAULT_RAM.0;
        ram[1] = DEFAULT_RAM.1;
        ram_words = 2;
    }
    // SAFETY: only this function writes the buffer, once, before anything reads it (the state
    // says when it is written).
    let buffer = unsafe { &mut (*(&raw mut DEVICE_TREE_BLOB)).0 };
    let mut writer = Writer::new(buffer);
    writer
        .begin(b"")
        .cells(b"#address-cells", &[2])
        .cells(b"#size-cells", &[2])
        .string(b"compatible", b"eggos,sun4u")
        .string(b"model", b"QEMU sun4u");
    writer
        .begin(b"chosen")
        .string(b"bootargs", firmware::command_line())
        .end();
    writer
        .begin(b"cpus")
        .cells(b"#address-cells", &[1])
        .cells(b"#size-cells", &[0]);
    writer
        .begin(b"cpu@0")
        .string(b"device_type", b"cpu")
        .string(b"compatible", b"SUNW,UltraSPARC-IIi")
        .cells(b"reg", &[0])
        .end();
    writer.end();
    writer
        .begin(b"memory@0")
        .string(b"device_type", b"memory")
        .wide_cells(b"reg", &ram[..ram_words])
        .end();
    writer
        .begin(b"serial@1fe020003f8")
        .string(b"compatible", b"ns16550a")
        .wide_cells(b"reg", &[UART_BASE, 8])
        .cells(b"clock-frequency", &[UART_CLOCK])
        .end();
    writer.end();
    match writer.finish() {
        Some(length) => {
            DEVICE_TREE_LENGTH.store(length, Ordering::Relaxed);
            DEVICE_TREE_STATE.store(WRITTEN, Ordering::Release);
            address()
        }
        None => {
            DEVICE_TREE_STATE.store(FAILED, Ordering::Release);
            0
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct PhysicalRegisters(u64);

impl RegisterIo for PhysicalRegisters {
    fn read8(&self, offset: usize) -> u8 {
        let value: u8;
        // SAFETY: a byte load from the board's UART register, which only the console reaches.
        unsafe {
            core::arch::asm!("lduba [{0}] 0x15, {1}", in(reg) self.0 + offset as u64, out(reg) value, options(nostack))
        };
        value
    }
    fn write8(&self, offset: usize, value: u8) {
        // SAFETY: as in `read8`, a store.
        unsafe {
            core::arch::asm!("stba {1}, [{0}] 0x15", in(reg) self.0 + offset as u64, in(reg) value, options(nostack))
        };
    }
    fn read16(&self, offset: usize) -> u16 {
        u16::from_be_bytes([self.read8(offset), self.read8(offset + 1)])
    }
    fn write16(&self, offset: usize, value: u16) {
        let [high, low] = value.to_be_bytes();
        self.write8(offset, high);
        self.write8(offset + 1, low);
    }
    fn read32(&self, offset: usize) -> u32 {
        u32::from(self.read16(offset)) << 16 | u32::from(self.read16(offset + 2))
    }
    fn write32(&self, offset: usize, value: u32) {
        self.write16(offset, (value >> 16) as u16);
        self.write16(offset + 2, value as u16);
    }
}

#[inline]
fn uart() -> Ns16550<PhysicalRegisters> {
    Ns16550::new(PhysicalRegisters(UART_BASE), 0)
}

#[derive(Debug)]
pub struct Sparc64;

impl Platform for Sparc64 {
    const ARCH: &'static str = "sparc64";

    #[inline]
    fn console_write(bytes: &[u8]) {
        uart().write_bytes(bytes);
    }

    fn console_read(buffer: &mut [u8]) -> usize {
        uart().read_bytes(buffer)
    }

    fn enable_console_input() -> Result<(), &'static str> {
        Err(
            "the UART's interrupt goes through the PCI bridge's interrupt mapping, not wired yet (D326)",
        )
    }

    fn route_device_interrupt(
        _specifier: &crate::fdt::InterruptSpecifier,
    ) -> Result<crate::DeviceLine, &'static str> {
        Err("device interrupts need the PCI bridge's interrupt mapping (D326)")
    }

    fn message_interrupt()
    -> Result<(crate::DeviceLine, crate::interrupt::InterruptMessage), &'static str> {
        Err("sun4u's PCI bridge has no MSI (D326)")
    }

    fn exit(status: ExitStatus) -> ! {
        let _ = status;
        Self::halt()
    }

    fn restart() -> ! {
        // SAFETY: `sir` resets the machine; nothing runs after it.
        unsafe { core::arch::asm!("sir 0", options(noreturn)) }
    }

    fn halt() -> ! {
        loop {
            // SAFETY: with interrupts off, this waits on, touching nothing.
            unsafe { core::arch::asm!("nop", options(nomem, nostack)) };
        }
    }

    fn breakpoint() {
        // SAFETY: `ta 1` traps to the kernel entry, which saves and restores every register
        // and resumes after it. No `nomem`/`nostack`: the handler may touch any memory.
        unsafe { core::arch::asm!("ta 1") };
    }

    fn execute_illegal_instruction() -> ! {
        // SAFETY: `illtrap` raises the illegal-instruction trap, which the kernel never
        // resumes from.
        unsafe { core::arch::asm!("illtrap 0", options(noreturn)) }
    }

    #[inline]
    fn clock_ticks() -> u64 {
        timer::now()
    }

    #[inline]
    fn clock_frequency() -> u64 {
        timer::frequency()
    }

    fn set_timer_deadline(deadline: u64) {
        timer::set_deadline(deadline);
    }

    fn wait_for_interrupt() {
        Self::interrupt_window();
    }

    #[inline]
    fn interrupt_window() {
        // SAFETY: `PSTATE.IE` on lets a pending interrupt in, through the kernel entry, which
        // restores every register; off again after. No `nomem` or `nostack`, as in
        // `breakpoint`.
        unsafe {
            core::arch::asm!(
                "wrpr %g0, {on}, %pstate",
                "nop",
                "wrpr %g0, {off}, %pstate",
                on = const pstate::KERNEL | pstate::INTERRUPTS,
                off = const pstate::KERNEL,
            )
        };
    }

    fn memory_map() -> MemoryMap {
        let mut map = match describe_machine() {
            // SAFETY: the tree is the HAL's own, written once in the kernel image, which stays
            // mapped and unwritten from then on.
            address @ 1.. => unsafe { boot_memory::device_tree_memory_map(address) },
            0 => {
                let mut map = MemoryMap::new();
                map.add_ram(PhysicalRange::from_start_length(
                    DEFAULT_RAM.0,
                    DEFAULT_RAM.1,
                ));
                map.reserve(boot_memory::kernel_image());
                map
            }
        };
        map.reserve(PhysicalRange {
            start: 0,
            end: KERNEL_LOAD_ADDRESS,
        });
        map.retain_within(PhysicalRange {
            start: 0,
            end: DIRECT_MAP_END,
        });
        map
    }

    #[inline]
    fn physical_memory_offset() -> usize {
        DIRECT_MAP_OFFSET
    }

    type PageTableFormat = paging::sparc64::Sparc64;

    fn direct_map_offset() -> usize {
        DIRECT_MAP_OFFSET
    }

    fn device_window() -> Option<core::ops::Range<u64>> {
        Some(DEVICE_WINDOW)
    }

    fn kernel_image() -> KernelImage {
        boot_memory::kernel_image_parts()
    }

    fn physical_memory() -> MemoryMap {
        let mut map = match describe_machine() {
            // SAFETY: as in `memory_map`.
            address @ 1.. => unsafe { boot_memory::device_tree_ram(address) },
            0 => {
                let mut map = MemoryMap::new();
                map.add_ram(PhysicalRange::from_start_length(
                    DEFAULT_RAM.0,
                    DEFAULT_RAM.1,
                ));
                map
            }
        };
        map.retain_within(PhysicalRange {
            start: 0,
            end: DIRECT_MAP_END,
        });
        map
    }

    fn early_device_regions(_region: &mut dyn FnMut(PhysicalRange)) {}

    fn device_tree() -> Option<&'static [u8]> {
        // SAFETY: as in `memory_map`.
        unsafe { boot_memory::device_tree_blob(describe_machine()) }
    }

    fn command_line(line: &mut dyn FnMut(&[u8])) {
        line(firmware::command_line());
    }

    fn make_kernel_mappings_visible(_start: usize, _length: usize) {
        exception::flush_tlb();
    }

    unsafe fn activate_page_tables(roots: &[u64]) {
        let [low, high] = roots else {
            panic!("SPARC64 page tables have two roots");
        };
        exception::set_roots(*low, Some(*high));
        exception::flush_tlb();
    }

    unsafe fn switch_address_space(roots: &[u64]) {
        let Some(low) = roots.first() else {
            panic!("SPARC64 page tables have two roots");
        };
        // SAFETY: orders this CPU's earlier table writes before the TLB loads that follow.
        unsafe { core::arch::asm!("membar #StoreLoad | #StoreStore", options(nostack)) };
        exception::set_roots(*low, None);
        exception::next_address_space();
    }

    fn with_user_access<R>(access: impl FnOnce() -> R) -> R {
        access()
    }

    unsafe fn run_user(context: &mut crate::user::UserContext) -> crate::user::UserExit {
        // SAFETY: the caller's guarantees are `run_user`'s.
        unsafe { exception::run_user(context) }
    }

    fn user_test_program() -> crate::user::TestProgram {
        exception::user_test_program()
    }

    fn linux_hardware_capabilities() -> [u64; 2] {
        [0, 0]
    }

    fn make_instructions_visible(start: usize, length: usize) {
        let mut line = start & !7;
        while line < start + length {
            // SAFETY: `flush` makes later fetches of the doubleword see earlier stores; it
            // changes no memory.
            unsafe { core::arch::asm!("flush {0}", in(reg) line, options(nostack)) };
            line += 8;
        }
    }

    fn page_fault(kind: crate::ExceptionKind, code: u64) -> Option<paging::PageFault> {
        exception::page_fault(kind, code)
    }

    fn drop_user_translations(start: u64, length: u64) {
        exception::drop_translations(start, length);
    }

    unsafe fn prepare_thread(
        stack_top: usize,
        entry: extern "C" fn(usize) -> !,
        argument: usize,
    ) -> usize {
        // SAFETY: the two frames fit in `INITIAL_FRAME_MAX` bytes, which the caller guarantees
        // are writable and unused below `stack_top`.
        let frame = unsafe {
            crate::thread::write_initial_frame(stack_top, &thread::initial_frame(entry, argument))
        };
        frame - thread::STACK_BIAS
    }

    #[inline]
    unsafe fn switch_thread(save: *mut usize, resume: usize) {
        // SAFETY: the caller guarantees `save` is writable and `resume` a saved context whose
        // stack is allocated and not running; the switch flushes and restores every window.
        unsafe { thread::eggos_switch_thread(save, resume) }
    }

    fn random_seeds(_seed: &mut dyn FnMut(crate::random::SeedSource, &[u8])) {}

    unsafe fn set_user_fp(on: bool) {
        // SAFETY: the caller's contract is `set_user`'s.
        unsafe { fp::set_user(on) }
    }

    unsafe fn save_fp(state: *mut crate::fp::FpState) {
        // SAFETY: the caller's contract is `save`'s.
        unsafe { fp::save(state) }
    }

    unsafe fn restore_fp(state: *const crate::fp::FpState) {
        // SAFETY: the caller's contract is `restore`'s.
        unsafe { fp::restore(state) }
    }

    fn cpus(cpu: &mut dyn FnMut(u64)) {
        cpu(Self::boot_cpu());
    }

    fn boot_cpu() -> u64 {
        0
    }

    unsafe fn start_cpu(_cpu: u64, _start: &CpuStart) -> Result<(), StartCpuError> {
        Err(StartCpuError::InvalidCpu)
    }

    unsafe fn set_cpu_local(block: usize) {
        CPU_LOCAL.store(block, Ordering::Relaxed);
    }

    #[inline]
    fn cpu_local() -> usize {
        CPU_LOCAL.load(Ordering::Relaxed)
    }

    #[inline]
    unsafe fn cpu_local_u64<const OFFSET: usize>() -> u64 {
        let block = Self::cpu_local();
        // SAFETY: the caller guarantees this CPU's block holds an aligned word at `OFFSET`.
        unsafe { ((block + OFFSET) as *const u64).read_volatile() }
    }

    fn wake_cpu(_cpu: u64) {}
}

/// Zeroes the `length` bytes at `start` ([`crate::memory::zero`]) in 64-byte lines.
///
/// # Safety
/// As for [`crate::memory::zero`].
#[inline]
pub(crate) unsafe fn zero(start: *mut u8, length: usize) {
    // SAFETY: guaranteed by the caller.
    unsafe { crate::memory::zero_by_lines(start, length) }
}

#[inline]
pub(crate) fn doorbell_barrier() {
    // SAFETY: a barrier: it touches no memory or register, only orders stores.
    unsafe {
        core::arch::asm!(
            "membar #StoreStore | #StoreLoad",
            options(nostack, preserves_flags)
        )
    };
}
