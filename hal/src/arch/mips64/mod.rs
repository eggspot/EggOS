//! MIPS64 (little-endian, Release 2) on QEMU `loongson3-virt` (D313).
//!
//! Boot contract: QEMU loads the ELF image at its physical addresses and enters `_start` at its
//! `CKSEG0` address in kernel mode, with `a0` and `a1` the argument count and a table of 32-bit
//! pointers to the arguments (the second is the command line) and `a2` the Loongson boot
//! parameters ([`lefi`]), which give the RAM, the CPUs and the clock. The machine gives no
//! device tree, so the HAL writes one ([`describe_machine`]): the RAM, the CPUs, the UART and
//! the generic ECAM PCI host at the addresses QEMU's board puts them, which the kernel's
//! discovery reads as any other.
//!
//! Translation: the kernel runs in the unmapped segments, its image in `CKSEG0` and the direct
//! map in the cached `XKPHYS` window, so nothing but programs (`XUSEG`) and the device window
//! (`XKSEG`, [`DEVICE_WINDOW`]) goes through the TLB, which the refill handler fills from page
//! tables in the [`paging::mips64`] format. Address-space identifiers change on every switch, so
//! a program never sees another's translations without the TLB being flushed each time. The
//! per-CPU block is in `$gp`, which a static, non-`abicalls` kernel never uses otherwise; the
//! entry from user mode puts the kernel's back.

mod exception;
mod fp;
mod lefi;
mod liointc;
mod thread;
mod timer;

use crate::arch::boot_memory;
use crate::cpu::{CpuStart, StartCpuError};
use crate::fdt::writer::Writer;
use crate::memory::PhysicalRange;
use crate::paging::{self, KernelImage};
use crate::{ExitStatus, MemoryMap, Platform};
use core::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};
use eggos_drivers::io::Mmio;
use eggos_drivers::serial::{Ns16550, Uart};

core::arch::global_asm!(
    r#"
    .set        noat
    .section .text.boot, "ax"
    .global _start
_start:
    li          $12, {status_kernel}
    mtc0        $12, $12
    ehb
    move        $16, $5
    move        $17, $6
    dla         $12, __bss_start
    dla         $13, __bss_end
1:  sltu        $14, $12, $13
    beqz        $14, 2f
    sd          $zero, 0($12)
    daddiu      $12, $12, 8
    b           1b
2:  dla         $12, {boot_arguments}
    sd          $16, 0($12)
    dla         $12, {boot_parameters}
    sd          $17, 0($12)
    dla         $29, __stack_top
    move        $28, $zero
    jal         mips64_cpu_setup
    jal         {init_timer}
    jal         kernel_main
3:  wait
    b           3b

    // Kernel mode with interrupts masked: the exception base, user access to `UserLocal` and
    // the counter through `rdhwr`, 4 KiB pages, read and execute inhibit, an empty TLB. Clobbers
    // $12 to $15.
mips64_cpu_setup:
    dla         $12, mips64_vectors
    mtc0        $12, $15, 1
    li          $12, {hwrena}
    mtc0        $12, $7
    mtc0        $zero, $5
    li          $12, {page_grain}
    mtc0        $12, $5, 1
    mtc0        $zero, $6
    ehb
    daddiu      $29, $29, -16
    sd          $31, 0($29)
    jal         {flush_tlb}
    ld          $31, 0($29)
    daddiu      $29, $29, 16
    jr          $31

    // A secondary CPU, sent here by QEMU's boot code with `a1` its `CpuStart` and `sp` the
    // stack mailbox 1 held.
    .global eggos_secondary_start
eggos_secondary_start:
    li          $12, {status_kernel}
    mtc0        $12, $12
    ehb
    move        $16, $5
    ld          $29, {start_stack_top}($16)
    move        $28, $zero
    jal         mips64_cpu_setup
    move        $4, $16
    jal         {secondary_main}
4:  wait
    b           4b
"#,
    status_kernel = const STATUS_KERNEL,
    hwrena = const HWRENA,
    page_grain = const PAGE_GRAIN,
    boot_arguments = sym BOOT_ARGUMENTS,
    boot_parameters = sym BOOT_PARAMETERS,
    init_timer = sym timer::init,
    flush_tlb = sym exception::flush_tlb,
    start_stack_top = const core::mem::offset_of!(CpuStart, stack_top),
    secondary_main = sym secondary_main,
);

/// The image's virtual address minus its physical address: `CKSEG0` (D33). `linker.ld` uses the
/// same value and checks it against this one.
pub(crate) const KERNEL_VIRTUAL_OFFSET: u64 = 0xffff_ffff_8000_0000;

/// `Status` for the kernel: 64-bit kernel, supervisor and user segments (`KX`, `SX`, `UX`),
/// 64-bit FP registers (`FR`), kernel mode, interrupts masked, the exception base from `EBase`
/// (`BEV` clear), the timer, IPI and device interrupt lines unmasked for when `IE` is set.
const STATUS_KERNEL: u64 = (1 << 26) | (1 << 7) | (1 << 6) | (1 << 5) | INTERRUPT_LINES;
/// `Status.IM` bits: the PCI and UART interrupts on `IP2`, IPIs on `IP6` and the timer on `IP7`.
pub(super) const INTERRUPT_LINES: u64 = (1 << 10) | (1 << 14) | (1 << 15);
/// `HWREna`: programs may read the CPU number, the SYNCI step, the counter, its resolution and
/// `UserLocal`.
const HWRENA: u64 = 0xf | (1 << 29);
/// `PageGrain`: `RI` and `XI` in `EntryLo` take effect.
const PAGE_GRAIN: u64 = (1 << 31) | (1 << 30);

/// The cached `XKPHYS` window, the direct map (D29): all physical memory, untranslated.
pub(super) const CACHED_WINDOW: u64 = paging::mips64::CACHED_WINDOW;
/// The uncached `XKPHYS` window, for the board's registers the HAL reaches itself.
const UNCACHED_WINDOW: u64 = 0x9000_0000_0000_0000;
const DIRECT_MAP_OFFSET: usize = CACHED_WINDOW as usize;
/// The physical memory the direct map covers: `XKPHYS` reaches 48 bits.
const DIRECT_MAP_END: u64 = 1 << 48;
/// The device window (D110): one root entry of `XKSEG`, below the one the image's addresses
/// index (the low 48 bits of `CKSEG0` addresses select root entry 511).
const DEVICE_WINDOW: core::ops::Range<u64> = 0xc000_ff00_0000_0000..0xc000_ff80_0000_0000;
/// The kernel's load address.
const KERNEL_LOAD_ADDRESS: u64 = 0x20_0000;

/// QEMU `loongson3-virt`'s board (`hw/mips/loongson3_virt.c`): the UART, the power-management
/// block, the inter-processor interrupt block, PCI Express configuration space, I/O and memory
/// windows.
const UART_BASE: u64 = 0x1fe0_01e0;
const UART_REG_SHIFT: u32 = 0;
const UART_CLOCK: u32 = 100_000_000;
const POWER_MANAGEMENT: u64 = 0x1008_0000;
const POWER_OFF: u8 = 0xff;
const RESET: u8 = 0x00;
pub(super) const IPI_BLOCK: u64 = 0x3ff0_1000;
pub(super) const IPI_CORE_STRIDE: u64 = 0x100;
const PCI_ECAM: (u64, u64) = (0x1a00_0000, 0x0200_0000);
const PCI_IO: (u64, u64) = (0x1800_0000, 0x1_0000);
const PCI_MEMORY: (u64, u64) = (0x4000_0000, 0x4000_0000);
const PCI_LAST_BUS: u32 = 0x1f;
/// RAM without boot parameters: the low 256 MiB.
const DEFAULT_RAM: (u64, u64) = (0, 256 << 20);
/// The boot information QEMU keeps at the top of the low 256 MiB, which its memory map leaves
/// out: kept out here too when the map is the default.
const DEFAULT_RAM_KEPT: u64 = 16 << 20;

/// Each CPU's boot mailboxes in its IPI block: the entry address, the stack, `gp` and `a1`.
mod mailbox {
    pub const ENTRY: u64 = 0x20;
    pub const STACK: u64 = 0x28;
    pub const GLOBAL_POINTER: u64 = 0x30;
    pub const ARGUMENT: u64 = 0x38;
}

/// What `_start` found in `a1` and `a2`.
static BOOT_ARGUMENTS: AtomicU64 = AtomicU64::new(0);
static BOOT_PARAMETERS: AtomicU64 = AtomicU64::new(0);

const DEVICE_TREE_BYTES: usize = 4096;

#[repr(C, align(8))]
struct DeviceTreeBuffer([u8; DEVICE_TREE_BYTES]);

/// The device tree [`describe_machine`] writes, once.
static mut DEVICE_TREE_BLOB: DeviceTreeBuffer = DeviceTreeBuffer([0; DEVICE_TREE_BYTES]);
static DEVICE_TREE_STATE: AtomicU8 = AtomicU8::new(UNWRITTEN);
static DEVICE_TREE_LENGTH: AtomicUsize = AtomicUsize::new(0);
const UNWRITTEN: u8 = 0;
const WRITTEN: u8 = 1;
const FAILED: u8 = 2;

const COMMAND_LINE_BYTES: usize = 512;

/// The command line, copied from the argument table when the device tree is written.
static mut COMMAND_LINE: [u8; COMMAND_LINE_BYTES] = [0; COMMAND_LINE_BYTES];
static COMMAND_LINE_LENGTH: AtomicUsize = AtomicUsize::new(0);

/// A kernel address the firmware passed, as a physical address in the low 4 GiB, if it is one.
fn firmware_physical(address: u64) -> Option<u64> {
    match address >> 32 {
        0xffff_ffff => Some(address & 0x1fff_ffff),
        _ if address >> 62 == 2 => Some(address & (DIRECT_MAP_END - 1)),
        0 => Some(address),
        _ => None,
    }
    .filter(|&physical| physical < 1 << 32)
}

/// Copies the command line, the second argument (`argv[1]`, a 32-bit pointer), into
/// [`COMMAND_LINE`].
fn copy_command_line() {
    let arguments = BOOT_ARGUMENTS.load(Ordering::Relaxed);
    let Some(table) = firmware_physical(arguments) else {
        return;
    };
    // SAFETY: the argument table and the strings are in the low memory QEMU wrote them to,
    // reached through the cached window, which maps all physical memory; nothing writes them.
    let pointer = unsafe { core::ptr::read_volatile((CACHED_WINDOW + table + 4) as *const i32) };
    let Some(line) = firmware_physical(pointer as i64 as u64) else {
        return;
    };
    // SAFETY: only `describe_machine` writes the copy, once, on the boot CPU before any reader.
    let copy = unsafe { &mut *(&raw mut COMMAND_LINE) };
    let mut length = 0;
    while length < COMMAND_LINE_BYTES - 1 {
        // SAFETY: as for the table above.
        let byte = unsafe { core::ptr::read_volatile((CACHED_WINDOW + line + length as u64) as *const u8) };
        if byte == 0 {
            break;
        }
        copy[length] = byte;
        length += 1;
    }
    COMMAND_LINE_LENGTH.store(length, Ordering::Relaxed);
}

/// Writes the machine's device tree from the boot parameters and QEMU's board layout, once, on
/// the boot CPU; its physical address, or 0 when it did not fit.
fn describe_machine() -> usize {
    let address = || ((&raw const DEVICE_TREE_BLOB) as u64 - KERNEL_VIRTUAL_OFFSET) as usize;
    match DEVICE_TREE_STATE.load(Ordering::Acquire) {
        WRITTEN => return address(),
        FAILED => return 0,
        _ => {}
    }
    copy_command_line();
    let parameters = lefi::read(BOOT_PARAMETERS.load(Ordering::Relaxed));
    let mut ram = [0u64; 16];
    let mut ram_words = 0;
    match &parameters {
        Some(found) if found.ram_count > 0 => {
            for &(start, length) in &found.ram[..found.ram_count] {
                ram[ram_words] = start;
                ram[ram_words + 1] = length;
                ram_words += 2;
            }
        }
        _ => {
            ram[0] = DEFAULT_RAM.0;
            ram[1] = DEFAULT_RAM.1 - DEFAULT_RAM_KEPT;
            ram_words = 2;
        }
    }
    let cpus = parameters.as_ref().map_or(1, |found| found.cpus);
    if let Some(found) = &parameters
        && found.cpu_clock != 0
    {
        timer::set_frequency(found.cpu_clock / 2);
    }
    // SAFETY: only this function writes the buffer, once, on the boot CPU before any CPU
    // starts and before anything reads it (the state says when it is written).
    let buffer = unsafe { &mut (*(&raw mut DEVICE_TREE_BLOB)).0 };
    // SAFETY: as for the command line in `copy_command_line`.
    let command_line = unsafe { &(*(&raw const COMMAND_LINE))[..COMMAND_LINE_LENGTH.load(Ordering::Relaxed)] };
    let mut writer = Writer::new(buffer);
    writer
        .begin(b"")
        .cells(b"#address-cells", &[2])
        .cells(b"#size-cells", &[2])
        .string(b"compatible", b"eggos,loongson3-virt")
        .string(b"model", b"QEMU loongson3-virt");
    writer.begin(b"chosen").string(b"bootargs", command_line).end();
    writer.begin(b"cpus").cells(b"#address-cells", &[1]).cells(b"#size-cells", &[0]);
    for cpu in 0..cpus {
        let mut name = *b"cpu@00";
        let length = match cpu {
            0..16 => 5,
            _ => 6,
        };
        let digits = b"0123456789abcdef";
        match length {
            5 => name[4] = digits[cpu as usize],
            _ => {
                name[4] = digits[(cpu >> 4) as usize & 15];
                name[5] = digits[cpu as usize & 15];
            }
        }
        writer
            .begin(&name[..length])
            .string(b"device_type", b"cpu")
            .string(b"compatible", b"loongson,gs464v")
            .cells(b"reg", &[cpu])
            .end();
    }
    writer.end();
    writer
        .begin(b"memory@0")
        .string(b"device_type", b"memory")
        .wide_cells(b"reg", &ram[..ram_words])
        .end();
    writer
        .begin(b"serial@1fe001e0")
        .string(b"compatible", b"ns16550a")
        .wide_cells(b"reg", &[UART_BASE, 8])
        .cells(b"clock-frequency", &[UART_CLOCK])
        .end();
    writer
        .begin(b"pcie@1a000000")
        .string(b"compatible", b"pci-host-ecam-generic")
        .string(b"device_type", b"pci")
        .cells(b"#address-cells", &[3])
        .cells(b"#size-cells", &[2])
        .wide_cells(b"reg", &[PCI_ECAM.0, PCI_ECAM.1])
        .cells(b"bus-range", &[0, PCI_LAST_BUS])
        .cells(
            b"ranges",
            &[
                0x0100_0000,
                0,
                0,
                (PCI_IO.0 >> 32) as u32,
                PCI_IO.0 as u32,
                0,
                PCI_IO.1 as u32,
                0x0200_0000,
                0,
                PCI_MEMORY.0 as u32,
                (PCI_MEMORY.0 >> 32) as u32,
                PCI_MEMORY.0 as u32,
                0,
                PCI_MEMORY.1 as u32,
            ],
        )
        .property(b"dma-coherent", &[])
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

/// Called by `eggos_secondary_start` on the `CpuStart`'s stack: switches to the kernel's
/// tables, sets this CPU's timer up (D41), then calls the entry.
extern "C" fn secondary_main(start: &CpuStart) -> ! {
    // SAFETY: `start_cpu`'s caller guarantees the tables are the kernel's own; interrupts are
    // masked (`Status.IE` is clear since the entry).
    unsafe { Mips64::activate_page_tables(&start.page_table_roots[..2]) };
    timer::init();
    (start.entry)(start.argument)
}

/// This CPU's number, `EBase.CPUNum`.
pub(super) fn cpu_number() -> u64 {
    let value: u64;
    // SAFETY: reading `EBase` has no side effects.
    unsafe { core::arch::asm!("mfc0 {0}, $15, 1", out(reg) value, options(nomem, nostack)) };
    value & 0x3ff
}

/// The IPI block register `offset` of CPU `cpu`, through the uncached window.
pub(super) fn ipi_register(cpu: u64, offset: u64) -> u64 {
    UNCACHED_WINDOW + IPI_BLOCK + cpu * IPI_CORE_STRIDE + offset
}

/// The early console, the UART through the uncached window.
#[inline]
fn uart() -> Ns16550<Mmio> {
    // SAFETY: on QEMU `loongson3-virt` the UART's register block is at `UART_BASE`, which the
    // uncached window reaches from the first instruction on; only the console's transmit and
    // receive paths use it.
    let regs = unsafe {
        Mmio::new(
            (UNCACHED_WINDOW + UART_BASE) as usize,
            Ns16550::<Mmio>::register_block_size(UART_REG_SHIFT),
        )
    };
    Ns16550::new(regs, UART_REG_SHIFT)
}

fn power_management(value: u8) {
    // SAFETY: the board's power-management register, through the uncached window; writing it
    // powers the machine off or resets it, which is what the callers want.
    unsafe { core::ptr::write_volatile((UNCACHED_WINDOW + POWER_MANAGEMENT) as *mut u8, value) };
}

#[derive(Debug)]
pub struct Mips64;

impl Platform for Mips64 {
    const ARCH: &'static str = "mips64";

    #[inline]
    fn console_write(bytes: &[u8]) {
        uart().write_bytes(bytes);
    }

    fn console_read(buffer: &mut [u8]) -> usize {
        uart().read_bytes(buffer)
    }

    fn enable_console_input() -> Result<(), &'static str> {
        liointc::enable_console()?;
        uart().enable_receive_interrupt();
        Ok(())
    }

    fn route_device_interrupt(
        specifier: &crate::fdt::InterruptSpecifier,
    ) -> Result<crate::DeviceLine, &'static str> {
        liointc::route_device(specifier.cells())
    }

    fn message_interrupt() -> Result<(crate::DeviceLine, crate::interrupt::InterruptMessage), &'static str> {
        Err("loongson3-virt's PCI Express has no MSI controller: its devices use INTx (D325)")
    }

    fn exit(status: ExitStatus) -> ! {
        let _ = status;
        power_management(POWER_OFF);
        Self::halt()
    }

    fn restart() -> ! {
        power_management(RESET);
        Self::halt()
    }

    fn halt() -> ! {
        loop {
            // SAFETY: `wait` only waits for an interrupt; with interrupts masked it waits on.
            unsafe { core::arch::asm!("wait", options(nomem, nostack, preserves_flags)) };
        }
    }

    fn breakpoint() {
        // SAFETY: `break` traps to the general exception entry, which saves and restores every
        // register and resumes after it. No `nomem`/`nostack`: the kernel's handler may read and
        // write any memory, and its frame is pushed below the stack pointer.
        unsafe { core::arch::asm!("break") };
    }

    fn execute_illegal_instruction() -> ! {
        // SAFETY: the word is a reserved `SPECIAL` function (5) on Release 2; it traps to the general exception entry, and
        // the kernel never resumes from that exception.
        unsafe { core::arch::asm!(".word 0x00000005", options(noreturn)) }
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
        // SAFETY: `wait` stops until an interrupt is pending, masked or not; `ei` then takes it
        // through the general entry, which restores every register, and `di` masks interrupts
        // again. No `nomem` or `nostack`, as in `breakpoint`.
        unsafe { core::arch::asm!("wait", "ei", "ehb", "di", "ehb") };
    }

    #[inline]
    fn interrupt_window() {
        // SAFETY: as in `wait_for_interrupt`, without the `wait`.
        unsafe { core::arch::asm!("ei", "ehb", "di", "ehb") };
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
                    DEFAULT_RAM.1 - DEFAULT_RAM_KEPT,
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

    type PageTableFormat = paging::mips64::Mips64;

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
                map.add_ram(PhysicalRange::from_start_length(DEFAULT_RAM.0, DEFAULT_RAM.1));
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
        describe_machine();
        // SAFETY: written once by `describe_machine` before any reader, read-only after.
        let bytes = unsafe { &(*(&raw const COMMAND_LINE))[..COMMAND_LINE_LENGTH.load(Ordering::Relaxed)] };
        line(bytes);
    }

    fn make_kernel_mappings_visible(_start: usize, _length: usize) {
        exception::flush_tlb();
    }

    unsafe fn activate_page_tables(roots: &[u64]) {
        let [low, high] = roots else {
            panic!("MIPS64 page tables have two roots");
        };
        exception::set_roots(*low, Some(*high));
        exception::flush_tlb();
    }

    unsafe fn switch_address_space(roots: &[u64]) {
        let Some(low) = roots.first() else {
            panic!("MIPS64 page tables have two roots");
        };
        // SAFETY: `sync` orders this CPU's earlier table writes before the refills that follow.
        unsafe { core::arch::asm!("sync", options(nostack)) };
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
        let step: usize;
        // SAFETY: `rdhwr $1` reads the SYNCI step, which kernel mode may always read.
        unsafe { core::arch::asm!(".set push", ".set noat", "rdhwr $1, $1", "move {0}, $1", ".set pop", out(reg) step, out("$1") _, options(nomem, nostack)) };
        if step != 0 {
            let mut line = start & !(step - 1);
            while line < start + length {
                // SAFETY: `synci` writes the data cache line back and drops the instruction
                // cache's copy; it changes no memory contents.
                unsafe { core::arch::asm!("synci 0({0})", in(reg) line, options(nostack)) };
                line += step;
            }
        }
        // SAFETY: `sync` completes the `synci`s; the hazard barrier makes later fetches see them.
        unsafe { core::arch::asm!("sync", "ehb", options(nostack)) };
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
        // SAFETY: the frame, with the rounding down to 16 bytes, fits in `INITIAL_FRAME_MAX`
        // bytes, which the caller guarantees are writable and unused below `stack_top`.
        unsafe {
            crate::thread::write_initial_frame(stack_top, &thread::initial_frame(entry, argument))
        }
    }

    #[inline]
    unsafe fn switch_thread(save: *mut usize, resume: usize) {
        // SAFETY: the caller guarantees `save` is writable and `resume` a saved context whose
        // stack is allocated and not running; the switch saves and restores every register the
        // calling convention requires a call to preserve.
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
        match describe_machine() {
            0 => cpu(Self::boot_cpu()),
            // SAFETY: as in `memory_map`.
            address => unsafe { boot_memory::device_tree_cpus(address, cpu) },
        }
    }

    fn boot_cpu() -> u64 {
        cpu_number()
    }

    unsafe fn start_cpu(cpu: u64, start: &CpuStart) -> Result<(), StartCpuError> {
        unsafe extern "C" {
            /// The secondary entry point in the boot code above.
            fn eggos_secondary_start();
        }
        if cpu >= 64 {
            return Err(StartCpuError::InvalidCpu);
        }
        let write = |offset: u64, value: u64| {
            // SAFETY: CPU `cpu`'s boot mailboxes in its IPI block, through the uncached window;
            // QEMU's boot code on that CPU waits for the entry mailbox to be set, then jumps.
            unsafe { core::ptr::write_volatile(ipi_register(cpu, offset) as *mut u64, value) };
        };
        core::sync::atomic::fence(Ordering::SeqCst);
        write(mailbox::STACK, start.stack_top as u64);
        write(mailbox::GLOBAL_POINTER, 0);
        write(mailbox::ARGUMENT, core::ptr::from_ref(start) as u64);
        core::sync::atomic::fence(Ordering::SeqCst);
        write(mailbox::ENTRY, eggos_secondary_start as *const () as u64);
        Ok(())
    }

    unsafe fn set_cpu_local(block: usize) {
        // SAFETY: `$gp` is unused by the static, non-`abicalls` kernel; the entry from user mode
        // puts the kernel's value back (D313).
        unsafe { core::arch::asm!("move $28, {}", in(reg) block, options(nomem, nostack)) };
    }

    #[inline]
    fn cpu_local() -> usize {
        let block: usize;
        // SAFETY: reading `$gp` has no side effects.
        unsafe { core::arch::asm!("move {}, $28", out(reg) block, options(nomem, nostack)) };
        block
    }

    #[inline]
    unsafe fn cpu_local_u64<const OFFSET: usize>() -> u64 {
        let value: u64;
        // SAFETY: the caller guarantees `$gp` holds this CPU's block, with an aligned word at
        // `OFFSET`: one aligned 8-byte load, which never tears.
        unsafe {
            core::arch::asm!(
                "ld {}, {offset}($28)",
                out(reg) value,
                offset = const OFFSET,
                options(nostack, readonly, preserves_flags),
            );
        }
        value
    }

    fn wake_cpu(cpu: u64) {
        timer::wake(cpu);
    }
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
    unsafe { core::arch::asm!("sync", options(nostack, preserves_flags)) };
}
