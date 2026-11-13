//! AArch64 (Armv8-A and later).
//!
//! Boot contract: entered at `_start` in EL1 with the MMU off, as QEMU's `virt` machine does
//! for an ELF kernel. The kernel is built for `aarch64-unknown-none-softfloat` (decision D10):
//! no FP/SIMD in the kernel, and `strict-align`, which was required while the MMU was off. Since
//! D33 only the entry code below runs with the MMU off: it builds the boot page tables, turns
//! the MMU on and jumps to the kernel's virtual address before any Rust code runs.
//!
//! Board addresses are QEMU `virt`'s for now; device-tree discovery replaces them in M5.

mod exception;
mod fp;
mod thread;
mod timer;

use crate::arch::boot_memory;
use crate::atomic::AtomicU64;
use crate::cpu::{CpuStart, StartCpuError};
use crate::memory::PhysicalRange;
use crate::paging::{self, Attributes, KernelImage, MemoryType};
use crate::{ExitStatus, MemoryMap, Platform};
use core::sync::atomic::{AtomicUsize, Ordering};
use eggos_drivers::io::Mmio;
use eggos_drivers::serial::{Pl011, Uart, pl011};

core::arch::global_asm!(
    r#"
    .section .text.boot, "ax"
    .global _start
_start:
    mov     x15, x0
    cbnz    x15, 0f
    mrs     x0, mpidr_el1
    and     x1, x0, #0xffffff
    ubfx    x2, x0, #32, #8
    orr     x0, x1, x2
    cbnz    x0, 6f

0:
    msr     spsel, #1

    adrp    x0, __bss_start
    add     x0, x0, :lo12:__bss_start
    adrp    x1, __bss_end
    add     x1, x1, :lo12:__bss_end
1:  cmp     x0, x1
    b.hs    2f
    str     xzr, [x0], #8
    b       1b

2:  adrp    x0, {l0_low}
    adrp    x1, {l1_low}
    adrp    x2, {l0_high}
    adrp    x3, {l1_direct}
    adrp    x4, {l1_kernel}
    adrp    x9, __kernel_start      // physical
    ldr     x13, =__kernel_start    // virtual
    sub     x16, x13, x9
    adrp    x17, {kernel_offset}
    add     x17, x17, :lo12:{kernel_offset}
    str     x16, [x17]
    adrp    x17, {boot_handoff}
    add     x17, x17, :lo12:{boot_handoff}
    str     x15, [x17]
    lsr     x16, x9, #30
    ubfx    x10, x9, #39, #9
    orr     x6, x1, #{table}
    str     x6, [x0, x10, lsl #3]
    lsr     x10, x9, #30
    lsl     x11, x10, #30
    ldr     x12, ={kernel_block}
    orr     x11, x11, x12
    and     x10, x10, #511
    str     x11, [x1, x10, lsl #3]  // identity
    ubfx    x10, x13, #39, #9
    orr     x6, x4, #{table}
    str     x6, [x2, x10, lsl #3]
    ubfx    x10, x13, #30, #9
    str     x11, [x4, x10, lsl #3]  // the kernel's virtual address
    mov     x10, #{direct_map_l0_index}
    orr     x6, x3, #{table}
    str     x6, [x2, x10, lsl #3]
    ldr     x12, ={device_block}
    ldr     x14, ={normal_block}
    mov     x10, #0
3:  cmp     x10, x16
    csel    x6, x12, x14, lo
    orr     x6, x6, x10, lsl #30
    str     x6, [x3, x10, lsl #3]   // the direct map
    add     x10, x10, #1
    cmp     x10, #{direct_map_gigabytes}
    b.lo    3b

    bl      boot_translation_on
    ldr     x0, =4f
    br      x0

4:  adrp    x0, __stack_top
    add     x0, x0, :lo12:__stack_top
    mov     sp, x0

    mov     x0, #{cpacr_fp_el1_only}
    msr     cpacr_el1, x0
    mov     x0, #{cntkctl_user_virtual_counter}
    msr     cntkctl_el1, x0
    msr     tpidrro_el0, xzr

    adrp    x0, exception_vectors
    add     x0, x0, :lo12:exception_vectors
    msr     vbar_el1, x0
    isb
    bl      {init_timer}
    bl      kernel_main
6:  wfe
    b       6b

boot_translation_on:
    ldr     x0, ={mair}
    msr     mair_el1, x0
    mrs     x1, id_aa64mmfr0_el1
    and     x1, x1, #0xf
    mov     x2, #{ips_max}
    cmp     x1, x2
    csel    x1, x1, x2, lo
    ldr     x0, ={tcr}
    orr     x0, x0, x1, lsl #{ips_shift}
    msr     tcr_el1, x0
    adrp    x0, {l0_low}
    msr     ttbr0_el1, x0
    adrp    x0, {l0_high}
    msr     ttbr1_el1, x0
    dsb     ish
    tlbi    vmalle1
    dsb     ish
    isb
    ldr     x0, ={sctlr}
    msr     sctlr_el1, x0
    isb
    ret

    .global eggos_secondary_start
eggos_secondary_start:
    mov     x19, x0
    msr     spsel, #1
    bl      boot_translation_on
    ldr     x0, =7f
    br      x0
7:  ldr     x0, [x19, #{start_stack_top}]
    mov     sp, x0
    mov     x0, #{cpacr_fp_el1_only}
    msr     cpacr_el1, x0
    mov     x0, #{cntkctl_user_virtual_counter}
    msr     cntkctl_el1, x0
    msr     tpidrro_el0, xzr
    adrp    x0, exception_vectors
    add     x0, x0, :lo12:exception_vectors
    msr     vbar_el1, x0
    isb
    mov     x0, x19
    bl      {secondary_main}
8:  wfe
    b       8b
    .ltorg
"#,
    secondary_main = sym secondary_main,
    start_stack_top = const core::mem::offset_of!(CpuStart, stack_top),
    cpacr_fp_el1_only = const fp::CPACR_FP_EL1_ONLY,
    cntkctl_user_virtual_counter = const timer::CNTKCTL_USER_VIRTUAL_COUNTER,
    init_timer = sym timer::init,
    l0_low = sym BOOT_L0_LOW,
    l1_low = sym BOOT_L1_LOW,
    l0_high = sym BOOT_L0_HIGH,
    l1_direct = sym BOOT_L1_DIRECT,
    l1_kernel = sym BOOT_L1_KERNEL,
    table = const paging::aarch64::table(0),
    kernel_block = const paging::aarch64::leaf(0, 2, BOOT_KERNEL),
    device_block = const paging::aarch64::leaf(0, 2, Attributes::DEVICE),
    normal_block = const paging::aarch64::leaf(0, 2, Attributes::READ_WRITE),
    direct_map_l0_index = const paging::index(DIRECT_MAP_OFFSET as u64, 3),
    kernel_offset = sym KERNEL_OFFSET,
    boot_handoff = sym crate::handoff::HANDOFF_ADDRESS,
    direct_map_gigabytes = const BOOT_DIRECT_MAP_END >> 30,
    mair = const paging::aarch64::MAIR,
    tcr = const TCR,
    ips_max = const TCR_IPS_MAX,
    ips_shift = const TCR_IPS_SHIFT,
    sctlr = const SCTLR_EL1_VALUE,
);

/// The image's virtual address minus its physical address: the kernel links at
/// `0xffff_ffff_8020_0000`, in the top 2 GiB (D33). `linker.ld` uses the same value and checks
/// it against this one.
pub(crate) const KERNEL_VIRTUAL_OFFSET: u64 = 0xffff_ffff_4000_0000;

static KERNEL_OFFSET: AtomicU64 = AtomicU64::new(0);

/// The image's virtual address minus its physical address.
pub(crate) fn kernel_virtual_offset() -> u64 {
    match KERNEL_OFFSET.load(Ordering::Relaxed) {
        0 => KERNEL_VIRTUAL_OFFSET,
        offset => offset,
    }
}

/// Zeroes the `length` bytes at `start` ([`crate::memory::zero`]): in 64-byte lines of word
/// stores, faster here than the compiler's `memset`, a loop of single word stores (D56).
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
    unsafe { core::arch::asm!("dmb oshst", options(nostack, preserves_flags)) };
}

/// The smallest data cache line, from `CTR_EL0.DminLine` (D563).
pub(crate) fn data_cache_line() -> usize {
    let ctr: u64;
    // SAFETY: reading the cache type register has no side effects.
    unsafe { core::arch::asm!("mrs {}, ctr_el0", out(reg) ctr, options(nomem, nostack)) };
    4 << (ctr >> 16 & 0xf)
}

/// Writes the data cache line at `address` back to memory, to the point of coherency (D563).
///
/// # Safety
/// `address` must be mapped.
pub(crate) unsafe fn clean_data_line(address: usize) {
    // SAFETY: cleaning a line writes back what the CPU wrote and changes no memory contents;
    // the caller vouches that `address` is mapped.
    unsafe { core::arch::asm!("dc cvac, {}", in(reg) address, options(nostack)) };
}

/// Writes the data cache line at `address` back and drops it, so the next read comes from
/// memory (D563).
///
/// # Safety
/// `address` must be mapped.
pub(crate) unsafe fn flush_data_line(address: usize) {
    // SAFETY: as `clean_data_line`; dropping a clean line changes no memory contents.
    unsafe { core::arch::asm!("dc civac, {}", in(reg) address, options(nostack)) };
}

/// Waits for the cache maintenance before it to finish, as devices see memory.
pub(crate) fn cache_maintenance_barrier() {
    // SAFETY: a barrier: it touches no memory or register.
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
}

const BOOT_DIRECT_MAP_END: u64 = 256 << 30;

const BOOT_KERNEL: Attributes = Attributes {
    writable: true,
    executable: true,
    memory_type: MemoryType::Normal,
    user: false,
};

#[repr(C, align(4096))]
struct BootTable([u64; paging::ENTRIES]);

static mut BOOT_L0_LOW: BootTable = BootTable([0; paging::ENTRIES]);
static mut BOOT_L1_LOW: BootTable = BootTable([0; paging::ENTRIES]);
static mut BOOT_L0_HIGH: BootTable = BootTable([0; paging::ENTRIES]);
static mut BOOT_L1_DIRECT: BootTable = BootTable([0; paging::ENTRIES]);
static mut BOOT_L1_KERNEL: BootTable = BootTable([0; paging::ENTRIES]);

const RAM_BASE: usize = 0x4000_0000;

const UART0_BASE: usize = 0x0900_0000;

const DIRECT_MAP_OFFSET: usize = 0xffff_0000_0000_0000;

const TCR: u64 = {
    let t0sz = 64 - 48;
    let t1sz = (64 - 48) << 16;
    let walks0 = (0b01 << 8) | (0b01 << 10) | (0b11 << 12);
    let walks1 = (0b01 << 24) | (0b01 << 26) | (0b11 << 28);
    let granule1 = 0b10 << 30;
    t0sz | t1sz | walks0 | walks1 | granule1 | TCR_TBI0_USER_TOP_BYTE_IGNORED
};
const TCR_TBI0_USER_TOP_BYTE_IGNORED: u64 = 1 << 37;
const TCR_IPS_SHIFT: u32 = 32;
const TCR_IPS_MAX: u64 = 0b101;
const SCTLR_MMU_AND_CACHES: u64 = (1 << 0) | (1 << 2) | (1 << 12);
const SCTLR_USER_CACHE_TYPE_AND_MAINTENANCE: u64 = (1 << 15) | (1 << 26);
const SCTLR_RES1: u64 = 0x30d0_0800;
const SCTLR_USER_ZERO_AND_WAIT: u64 = (1 << 14) | (1 << 16) | (1 << 18);
const SCTLR_EL1_VALUE: u64 = SCTLR_RES1
    | SCTLR_MMU_AND_CACHES
    | SCTLR_USER_CACHE_TYPE_AND_MAINTENANCE
    | SCTLR_USER_ZERO_AND_WAIT;

const PSCI_SYSTEM_OFF: u64 = 0x8400_0008;
const PSCI_SYSTEM_RESET: u64 = 0x8400_0009;
const PSCI_CPU_ON: u64 = 0xc400_0003;
const PSCI_SUCCESS: i64 = 0;
const PSCI_INVALID_PARAMETERS: i64 = -2;
const PSCI_ALREADY_ON: i64 = -4;
const PSCI_ON_PENDING: i64 = -5;

extern "C" fn secondary_main(start: &CpuStart) -> ! {
    // SAFETY: `start_cpu`'s caller guarantees the tables are the kernel's own, which map
    // everything this CPU uses at the addresses it uses now, as they did for the boot CPU;
    // interrupts are masked (PSCI CPU_ON starts the CPU with them masked).
    unsafe { Aarch64::activate_page_tables(&start.page_table_roots) };
    timer::init_this_cpu();
    (start.entry)(start.argument)
}

#[inline]
fn atomics_capability() -> u64 {
    const HWCAP_ATOMICS: u64 = 1 << 8;
    const ATOMIC_SHIFT: u32 = 20;
    const ATOMICS_IMPLEMENTED: u64 = 0b0010;
    let isar0: u64;
    // SAFETY: reading an ID register has no side effects.
    unsafe {
        core::arch::asm!("mrs {}, id_aa64isar0_el1", out(reg) isar0, options(nomem, nostack))
    };
    if isar0 >> ATOMIC_SHIFT & 0xf >= ATOMICS_IMPLEMENTED {
        HWCAP_ATOMICS
    } else {
        0
    }
}

static UART_ADDRESS: AtomicUsize = AtomicUsize::new(UNKNOWN_UART);
const UNKNOWN_UART: usize = usize::MAX;
const NO_UART: usize = 0;

fn uart_address() -> usize {
    let known = UART_ADDRESS.load(Ordering::Relaxed);
    if known != UNKNOWN_UART {
        return known;
    }
    let found = match crate::handoff::handoff::<Aarch64>() {
        None => device_tree_bytes()
            .and_then(crate::fdt::pl011)
            .and_then(|address| usize::try_from(address).ok())
            .filter(|&address| address != 0 && (address as u64) < BOOT_DIRECT_MAP_END)
            .unwrap_or(UART0_BASE),
        Some(handoff) => acpi_table_from(handoff.rsdp, b"SPCR")
            .and_then(crate::acpi::parse_spcr)
            .filter(|port| port.kind == crate::acpi::SerialKind::Pl011)
            .and_then(|port| usize::try_from(port.address).ok())
            .filter(|&address| address != 0 && (address as u64) < BOOT_DIRECT_MAP_END)
            .unwrap_or(NO_UART),
    };
    UART_ADDRESS.store(found, Ordering::Relaxed);
    found
}

fn uart0() -> Option<Pl011<Mmio>> {
    let address = uart_address();
    if address == NO_UART {
        return None;
    }
    // SAFETY: the console UART's register block is at `address` (QEMU `virt`'s UART0, or the
    // firmware's SPCR), inside the boot direct map, which the kernel's own tables map as device
    // memory too ([`Aarch64::early_device_regions`]); only the early console's transmit and
    // receive paths use it. Turning receive interrupts on changes registers transmit relies on,
    // which is sound because the boot CPU does it once, before any other CPU starts.
    Some(Pl011::new(unsafe {
        Mmio::new(DIRECT_MAP_OFFSET + address, pl011::REGISTER_BLOCK_SIZE)
    }))
}

fn acpi_table_from(rsdp: u64, signature: &[u8; 4]) -> Option<&'static [u8]> {
    if rsdp == 0 {
        return None;
    }
    crate::acpi::find_table(rsdp, signature, &read_acpi_memory)
}

fn acpi_tables_from(rsdp: u64, signature: &[u8; 4], each: &mut dyn FnMut(&'static [u8])) {
    if rsdp != 0 {
        crate::acpi::find_tables(rsdp, signature, &read_acpi_memory, each);
    }
}

fn read_acpi_memory(address: u64, length: usize) -> Option<&'static [u8]> {
    let end = address.checked_add(length as u64)?;
    if address == 0 || end > BOOT_DIRECT_MAP_END {
        return None;
    }
    // SAFETY: the firmware's ACPI tables are RAM the boot direct map covers (checked), which
    // nothing writes.
    Some(unsafe {
        core::slice::from_raw_parts(
            (address as usize).wrapping_add(DIRECT_MAP_OFFSET) as *const u8,
            length,
        )
    })
}

/// The firmware's device tree, whole, through the boot direct map.
pub(super) fn device_tree_bytes() -> Option<&'static [u8]> {
    // SAFETY: the address is 0, the loader's tree or QEMU `virt`'s at the start of RAM, which
    // nothing writes and the boot direct map covers.
    unsafe { boot_memory::device_tree_blob(device_tree_address()) }
}

fn device_tree_address() -> usize {
    match crate::handoff::handoff::<Aarch64>() {
        Some(handoff) => usize::try_from(handoff.device_tree).unwrap_or(0),
        None => RAM_BASE,
    }
}

fn psci_system_call(function: u64) {
    // SAFETY: SYSTEM_OFF and SYSTEM_RESET take no arguments, touch none of our memory and do
    // not return on success.
    unsafe { psci_call(function, 0, 0, 0) };
}

fn psci_uses_smc() -> bool {
    const FADT_ARM_BOOT_ARCH: usize = 129;
    const PSCI_USE_HVC: u16 = 1 << 1;
    let Some(handoff) = crate::handoff::handoff::<Aarch64>() else {
        return false;
    };
    acpi_table_from(handoff.rsdp, b"FACP")
        .and_then(|fadt| fadt.get(FADT_ARM_BOOT_ARCH..FADT_ARM_BOOT_ARCH + 2))
        .map(|flags| u16::from_le_bytes([flags[0], flags[1]]))
        .is_some_and(|flags| flags & PSCI_USE_HVC == 0)
}

unsafe fn psci_call(function: u64, first: u64, second: u64, third: u64) -> i64 {
    let result: i64;
    if psci_uses_smc() {
        // SAFETY: guaranteed by the caller.
        unsafe {
            core::arch::asm!(
                "dsb ish",
                "smc #0",
                inout("x0") function => result,
                inout("x1") first => _,
                inout("x2") second => _,
                inout("x3") third => _,
                out("x4") _, out("x5") _, out("x6") _, out("x7") _, out("x8") _, out("x9") _,
                out("x10") _, out("x11") _, out("x12") _, out("x13") _, out("x14") _,
                out("x15") _, out("x16") _, out("x17") _,
                options(nostack),
            );
        }
    } else {
        // SAFETY: guaranteed by the caller.
        unsafe {
            core::arch::asm!(
                "dsb ish",
                "hvc #0",
                inout("x0") function => result,
                inout("x1") first => _,
                inout("x2") second => _,
                inout("x3") third => _,
                out("x4") _, out("x5") _, out("x6") _, out("x7") _, out("x8") _, out("x9") _,
                out("x10") _, out("x11") _, out("x12") _, out("x13") _, out("x14") _,
                out("x15") _, out("x16") _, out("x17") _,
                options(nostack),
            );
        }
    }
    result
}

/// The ACPI `MADT` when the UEFI loader started the kernel.
pub(super) fn madt() -> Option<&'static [u8]> {
    acpi_table_from(crate::handoff::handoff::<Aarch64>()?.rsdp, b"APIC")
}

#[derive(Debug)]
pub struct Aarch64;

/// Times `RNDR` is asked for one value before the CPU counts as having none to give.
const RNDR_TRIES: usize = 10;

/// `TTBR0_EL1.ASID`, bits 63:48.
const TTBR_ASID_SHIFT: u32 = 48;

/// The ASID in `TTBR0_EL1`, in place (bits 63:48): what the active user space's translations
/// carry (D552).
fn current_asid() -> u64 {
    let ttbr: u64;
    // SAFETY: reading `TTBR0_EL1` has no side effects.
    unsafe { core::arch::asm!("mrs {}, ttbr0_el1", out(reg) ttbr, options(nomem, nostack, preserves_flags)) };
    ttbr & (0xffff << TTBR_ASID_SHIFT)
}

impl Platform for Aarch64 {
    const ARCH: &'static str = "aarch64";

    #[inline]
    fn console_write(bytes: &[u8]) {
        if let Some(uart) = uart0() {
            uart.write_bytes(bytes);
        }
    }

    fn console_read(buffer: &mut [u8]) -> usize {
        uart0().map_or(0, |uart| uart.read_bytes(buffer))
    }

    fn enable_console_input() -> Result<(), &'static str> {
        if uart_address() != UART0_BASE {
            return Err("the console UART's interrupt is QEMU virt's only so far");
        }
        let uart = uart0().ok_or("no console UART")?;
        uart.enable_receive_interrupt();
        timer::route_console_input();
        Ok(())
    }

    fn route_device_interrupt(
        specifier: &crate::fdt::InterruptSpecifier,
    ) -> Result<crate::DeviceLine, &'static str> {
        let (id, trigger) =
            eggos_drivers::interrupt::gicv2::shared_interrupt_of_specifier(specifier.cells())?;
        timer::route_device_interrupt(id, trigger)
    }

    fn release_device_interrupt(line: crate::DeviceLine) {
        timer::release_device_interrupt(line);
    }

    fn steer_device_interrupt(line: crate::DeviceLine, cpu: u64) -> Result<(), &'static str> {
        timer::steer_device_interrupt(line, cpu)
    }

    fn exit(_status: ExitStatus) -> ! {
        psci_system_call(PSCI_SYSTEM_OFF);
        Self::halt()
    }

    fn restart() -> ! {
        psci_system_call(PSCI_SYSTEM_RESET);
        Self::halt()
    }

    fn halt() -> ! {
        loop {
            // SAFETY: `wfe` only pauses the CPU until an event or interrupt.
            unsafe { core::arch::asm!("wfe", options(nomem, nostack, preserves_flags)) };
        }
    }

    fn breakpoint() {
        // SAFETY: `brk` traps to the exception vectors installed at boot, whose handler saves
        // and restores every general-purpose register and resumes after the `brk`.
        // No `nomem`/`nostack`: the kernel's handler may read and write any memory, and the
        // exception frame is pushed below the stack pointer.
        unsafe { core::arch::asm!("brk #0") };
    }

    fn execute_illegal_instruction() -> ! {
        // SAFETY: `udf` is permanently undefined; it traps to the exception vectors installed
        // at boot, and the kernel never resumes from that exception.
        unsafe { core::arch::asm!("udf #0", options(noreturn)) }
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
        // SAFETY: `wfi` wakes once an interrupt is pending, even with `PSTATE.I` set; clearing
        // I then takes it through the vectors installed at boot, which restore every register,
        // and setting I masks interrupts again. No `nomem`/`nostack`: the kernel's handler may
        // read and write any memory, and the exception frame is pushed below the stack pointer.
        unsafe { core::arch::asm!("wfi", "msr daifclr, #2", "isb", "msr daifset, #2") };
    }

    #[inline]
    fn interrupt_window() {
        // SAFETY: as in `wait_for_interrupt`, without the `wfi`: the `isb` makes the unmasking
        // take effect, so an interrupt already pending is taken before I is set again.
        unsafe { core::arch::asm!("msr daifclr, #2", "isb", "msr daifset, #2") };
    }

    fn memory_map() -> MemoryMap {
        if let Some(handoff) = crate::handoff::handoff::<Self>() {
            let mut map = handoff_memory_map(&handoff, false);
            map.reserve(boot_memory::kernel_image());
            map.reserve(crate::handoff::boot_data(&handoff));
            // SAFETY: the loader's device tree address is 0 or the firmware's tree, which
            // nothing writes and the direct map covers.
            if let Some(tree) = unsafe { boot_memory::device_tree_blob(device_tree_address()) } {
                map.reserve(PhysicalRange::from_start_length(
                    device_tree_address() as u64,
                    tree.len() as u64,
                ));
            }
            map.retain_within(PhysicalRange {
                start: 0,
                end: BOOT_DIRECT_MAP_END,
            });
            return map;
        }
        // SAFETY: on QEMU `virt` the start of RAM holds the device tree QEMU loaded there,
        // which nothing writes; the direct map covers it. If QEMU loaded none, the header check
        // fails and the map is empty.
        let mut map = unsafe { boot_memory::device_tree_memory_map(RAM_BASE) };
        map.retain_within(PhysicalRange {
            start: 0,
            end: BOOT_DIRECT_MAP_END,
        });
        map
    }

    #[inline]
    fn physical_memory_offset() -> usize {
        DIRECT_MAP_OFFSET
    }

    type PageTableFormat = paging::aarch64::Aarch64;

    fn direct_map_offset() -> usize {
        DIRECT_MAP_OFFSET
    }

    fn kernel_image() -> KernelImage {
        boot_memory::kernel_image_parts()
    }

    fn physical_memory() -> MemoryMap {
        let mut map = match crate::handoff::handoff::<Self>() {
            Some(handoff) => {
                let mut map = handoff_memory_map(&handoff, true);
                map.add_ram(crate::handoff::boot_data(&handoff).covering_frames());
                map
            }
            // SAFETY: as in `memory_map`.
            None => unsafe { boot_memory::device_tree_ram(RAM_BASE) },
        };
        map.retain_within(PhysicalRange {
            start: 0,
            end: BOOT_DIRECT_MAP_END,
        });
        map
    }

    fn early_device_regions(region: &mut dyn FnMut(PhysicalRange)) {
        let uart = uart_address();
        if uart != NO_UART {
            region(PhysicalRange::from_start_length(
                uart as u64,
                pl011::REGISTER_BLOCK_SIZE as u64,
            ));
        }
        timer::device_regions(region);
    }

    fn device_tree() -> Option<&'static [u8]> {
        // SAFETY: as in `memory_map`.
        unsafe { boot_memory::device_tree_blob(device_tree_address()) }
    }

    fn acpi_table(signature: &[u8; 4]) -> Option<&'static [u8]> {
        acpi_table_from(crate::handoff::handoff::<Self>()?.rsdp, signature)
    }

    fn acpi_tables(signature: &[u8; 4], table: &mut dyn FnMut(&'static [u8])) {
        if let Some(handoff) = crate::handoff::handoff::<Self>() {
            acpi_tables_from(handoff.rsdp, signature, table);
        }
    }

    fn acpi_dsdt() -> Option<&'static [u8]> {
        let fadt = Self::acpi_table(b"FACP").and_then(crate::acpi::parse_fadt)?;
        crate::acpi::table_at(fadt.dsdt, b"DSDT", &|address, length| {
            let end = address.checked_add(length as u64)?;
            if end > BOOT_DIRECT_MAP_END {
                return None;
            }
            // SAFETY: the firmware's DSDT is in RAM the direct map covers (checked) and nothing
            // writes it.
            Some(unsafe {
                core::slice::from_raw_parts(
                    (address as usize).wrapping_add(DIRECT_MAP_OFFSET) as *const u8,
                    length,
                )
            })
        })
    }

    fn device_apertures(aperture: &mut dyn FnMut(PhysicalRange)) {
        let Some(mcfg) = Self::acpi_table(b"MCFG") else {
            return;
        };
        for region in eggos_drivers::pci::mcfg::regions(mcfg) {
            aperture(PhysicalRange::from_start_length(
                region
                    .base
                    .saturating_add(u64::from(region.start_bus) << 20),
                eggos_drivers::pci::ecam::ecam_length(region.start_bus, region.end_bus),
            ));
        }
    }

    fn command_line(line: &mut dyn FnMut(&[u8])) {
        if let Some(handoff) = crate::handoff::handoff::<Self>() {
            let text = crate::handoff::command_line::<Self>(&handoff);
            if !text.is_empty() {
                line(text);
                return;
            }
        }
        if let Some(arguments) = Self::device_tree().and_then(crate::fdt::boot_arguments) {
            line(arguments);
        }
    }

    const DEVICE_TREE_DMA_COHERENT_BY_DEFAULT: bool = false;

    fn make_kernel_mappings_visible(_start: usize, _length: usize) {
        // SAFETY: barriers change no memory contents: the table writes complete before any walk,
        // and the next instructions see them. Arm caches no invalid entries, so nothing is
        // invalidated.
        unsafe { core::arch::asm!("dsb ishst", "isb", options(nostack, preserves_flags)) };
    }

    unsafe fn activate_page_tables(roots: &[u64]) {
        let [low, high] = roots else {
            panic!("AArch64 page tables have two roots");
        };
        // SAFETY: the caller guarantees the tables map everything this CPU uses from here on
        // at the addresses it uses now: the kernel image at its virtual address (TTBR1), the
        // direct map and the devices in it; the low half (`low`) is empty. `MAIR` and `TCR`
        // stay as the boot code set them. The tables replace the boot tables without two live
        // translations of one address ever meeting in the TLB (the Arm ARM's break-before-make
        // rule, D8.16): the code branches to the kernel's identity alias in the boot TTBR0,
        // points TTBR1 at the empty `low` root and invalidates the TLB, then at the new tables,
        // branches back to the kernel's virtual address, and finally replaces the boot TTBR0
        // with `low` and invalidates again. Nothing in between touches memory, and interrupts
        // are masked. The first barrier completes the table writes before any walk. No
        // `nomem`: memory accesses must not move across the switch.
        unsafe {
            core::arch::asm!(
                "dsb ish",
                "adr {tmp}, 2f",
                "sub {tmp}, {tmp}, {offset}",
                "br {tmp}",
                "2:",
                "msr ttbr1_el1, {low}",
                "isb",
                "tlbi vmalle1",
                "dsb ish",
                "isb",
                "msr ttbr1_el1, {high}",
                "isb",
                "adr {tmp}, 3f",
                "add {tmp}, {tmp}, {offset}",
                "br {tmp}",
                "3:",
                "msr ttbr0_el1, {low}",
                "isb",
                "tlbi vmalle1",
                "dsb ish",
                "isb",
                low = in(reg) *low,
                high = in(reg) *high,
                offset = in(reg) kernel_virtual_offset(),
                tmp = out(reg) _,
                options(nostack, preserves_flags),
            );
        }
    }

    unsafe fn switch_address_space(roots: &[u64]) {
        let [low, _] = roots else {
            panic!("AArch64 page tables have two roots");
        };
        // SAFETY: the caller guarantees `low` translates the low half alone, in the active
        // format, with the kernel half (`TTBR1_EL1`) unchanged, and that nothing uses the low
        // half meanwhile. The first barrier completes the table writes before any walk; every
        // translation runs with ASID 0 (`TCR_EL1.A1` = 0, ASID 0 in `TTBR0_EL1`), user ones
        // tagged with it as not global, so `TLBI ASIDE1` with ASID 0 drops the old low half's
        // and keeps the kernel's global ones. Local: each CPU drops what its own TLB cached when
        // it switches. No `nomem`: memory accesses must not move across the switch.
        unsafe {
            core::arch::asm!(
                "dsb ish",
                "msr ttbr0_el1, {low}",
                "isb",
                "tlbi aside1, xzr",
                "dsb nsh",
                "isb",
                low = in(reg) *low,
                options(nostack, preserves_flags),
            );
        }
    }

    unsafe fn switch_tagged_address_space(roots: &[u64], tag: crate::asid::AddressSpaceTag) {
        let [low, _] = roots else {
            panic!("AArch64 page tables have two roots");
        };
        let block = Self::cpu_local();
        // SAFETY: the per-CPU register holds this CPU's block, with a `HalCpuLocal` at its
        // offset; only this CPU uses its tags, with interrupts masked (the caller's duty).
        let choice = unsafe {
            let hal = &*((block + crate::cpu::HAL_CPU_LOCAL_OFFSET) as *const crate::cpu::HalCpuLocal);
            hal.address_space_tags.choose(tag)
        };
        let asid = u64::from(choice.hardware_tag) << TTBR_ASID_SHIFT;
        // SAFETY: as in `switch_address_space`, with the space's own ASID in `TTBR0_EL1`
        // (`TCR_EL1.A1` = 0, every CPU has at least 8 ASID bits): user translations are not
        // global and carry it, so `TLBI ASIDE1` drops only that space's, and only when the tags
        // say they are stale.
        unsafe {
            core::arch::asm!(
                "dsb ish",
                "msr ttbr0_el1, {ttbr}",
                "isb",
                ttbr = in(reg) *low | asid,
                options(nostack, preserves_flags),
            );
            if choice.drop_old {
                core::arch::asm!(
                    "tlbi aside1, {asid}",
                    "dsb nsh",
                    "isb",
                    asid = in(reg) asid,
                    options(nostack, preserves_flags),
                );
            }
        }
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
        const HWCAP_FP: u64 = 1 << 0;
        const HWCAP_ASIMD: u64 = 1 << 1;
        let features: u64;
        // SAFETY: reading an ID register has no side effects.
        unsafe {
            core::arch::asm!("mrs {}, id_aa64pfr0_el1", out(reg) features, options(nomem, nostack))
        };
        let fp = if features >> 16 & 0xf != 0xf {
            HWCAP_FP
        } else {
            0
        };
        let asimd = if features >> 20 & 0xf != 0xf {
            HWCAP_ASIMD
        } else {
            0
        };
        [fp | asimd | atomics_capability(), 0]
    }

    fn make_instructions_visible(start: usize, length: usize) {
        let ctr: u64;
        // SAFETY: reading the cache type register has no side effects.
        unsafe { core::arch::asm!("mrs {}, ctr_el0", out(reg) ctr, options(nomem, nostack)) };
        let line = 4usize << (ctr >> 16 & 0xf);
        let mut address = start & !(line - 1);
        while address < start + length {
            // SAFETY: cleaning a line to the point of unification writes back what the caller
            // wrote and changes no memory contents; `address` is in the caller's mapped range.
            unsafe { core::arch::asm!("dc cvau, {}", in(reg) address, options(nostack)) };
            address += line;
        }
        // SAFETY: barriers and instruction cache invalidation change no memory contents. The
        // whole instruction cache is invalidated, not by address, so that no alias of the
        // lines (another virtual address of the same frames, as user space has) stays stale;
        // on every CPU in the inner shareable domain (`ialluis`), as the cleaning by address
        // reaches them too, so a program loaded here runs on any CPU (D48).
        unsafe { core::arch::asm!("dsb ish", "ic ialluis", "dsb ish", "isb", options(nostack)) };
    }

    fn page_fault(_kind: crate::ExceptionKind, code: u64) -> Option<paging::PageFault> {
        exception::page_fault(code)
    }

    fn drop_kernel_translations(start: u64, length: u64) {
        let _ = Self::drop_kernel_translations_everywhere(start, length);
    }

    fn drop_kernel_translations_everywhere(start: u64, length: u64) -> bool {
        // SAFETY: as in `drop_user_translations_everywhere`: `VAAE1IS` drops each page's
        // translations, global ones included, on every CPU of the inner shareable domain; the
        // operand's top bits are ignored (the address's bits 55:12 only).
        unsafe { core::arch::asm!("dsb ishst", options(nostack, preserves_flags)) };
        for page in 0..length / crate::memory::FRAME_SIZE {
            let operand = ((start + page * crate::memory::FRAME_SIZE) >> 12) & ((1 << 44) - 1);
            // SAFETY: as above.
            unsafe {
                core::arch::asm!("tlbi vaae1is, {}", in(reg) operand, options(nostack, preserves_flags))
            };
        }
        // SAFETY: as above.
        unsafe { core::arch::asm!("dsb ish", "isb", options(nostack, preserves_flags)) };
        true
    }

    fn drop_user_translations_everywhere(start: u64, length: u64) -> bool {
        let pages = length / crate::memory::FRAME_SIZE;
        if pages > crate::paging::USER_PAGES_DROPPED_ONE_BY_ONE {
            // SAFETY: barriers and TLB invalidation change no memory contents. `VMALLE1IS` drops
            // every stage-1 translation of every CPU in the inner shareable domain, the kernel's
            // too (they are refilled), and the last `DSB ISH` waits until every CPU has.
            unsafe {
                core::arch::asm!(
                    "dsb ishst",
                    "tlbi vmalle1is",
                    "dsb ish",
                    "isb",
                    options(nostack, preserves_flags)
                )
            };
            return true;
        }
        // SAFETY: as above: `VAAE1IS` drops one page's translations in every ASID on every CPU
        // of the inner shareable domain; the barriers order the table writes before and wait for
        // the invalidations after.
        unsafe { core::arch::asm!("dsb ishst", options(nostack, preserves_flags)) };
        for page in 0..pages {
            let operand = (start + page * crate::memory::FRAME_SIZE) >> 12;
            // SAFETY: as above.
            unsafe {
                core::arch::asm!("tlbi vaae1is, {}", in(reg) operand, options(nostack, preserves_flags))
            };
        }
        // SAFETY: as above.
        unsafe { core::arch::asm!("dsb ish", "isb", options(nostack, preserves_flags)) };
        true
    }

    fn drop_user_translations(start: u64, length: u64) {
        let pages = length / crate::memory::FRAME_SIZE;
        let asid = current_asid();
        if pages > crate::paging::USER_PAGES_DROPPED_ONE_BY_ONE {
            // SAFETY: barriers and TLB invalidation change no memory contents. User
            // translations carry the active space's ASID and are not global (see
            // `switch_tagged_address_space`), so `TLBI ASIDE1` with it drops them all and keeps
            // the kernel's. Local: the kernel sees to other CPUs.
            unsafe {
                core::arch::asm!(
                    "dsb ishst",
                    "tlbi aside1, {asid}",
                    "dsb nsh",
                    "isb",
                    asid = in(reg) asid,
                    options(nostack, preserves_flags)
                )
            };
            return;
        }
        // SAFETY: as above: the first barrier completes the caller's table writes before any
        // walk, then each page's translations (any level, the active ASID in the operand's top
        // bits, which user addresses below 2^48 leave free) are dropped on this CPU.
        unsafe { core::arch::asm!("dsb ishst", options(nostack, preserves_flags)) };
        for page in 0..pages {
            let operand = (start + page * crate::memory::FRAME_SIZE) >> 12 | asid;
            // SAFETY: as above.
            unsafe {
                core::arch::asm!("tlbi vae1, {}", in(reg) operand, options(nostack, preserves_flags))
            };
        }
        // SAFETY: as above: completes the invalidations before the next instruction.
        unsafe { core::arch::asm!("dsb nsh", "isb", options(nostack, preserves_flags)) };
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
        // calling convention requires a call to preserve, so to each thread the call is an
        // ordinary function call.
        unsafe { thread::eggos_switch_thread(save, resume) }
    }

    fn random_seeds(seed: &mut dyn FnMut(crate::random::SeedSource, &[u8])) {
        // SAFETY: as in `memory_map`; the kernel calls this once at boot, on the boot CPU
        // before any other starts and between the HAL's other reads of the tree, which the
        // direct map keeps writable (RAM is mapped read-write in it, reserved or not).
        unsafe { boot_memory::device_tree_take_seeds(device_tree_address(), seed) };
        let mut bytes = [0u8; 32];
        if Self::cpu_random(&mut bytes) {
            seed(crate::random::SeedSource::CpuInstruction, &bytes);
        }
        crate::random::wipe(&mut bytes);
    }

    fn user_counter_readable() -> bool {
        true
    }

    fn cpu_random(bytes: &mut [u8; 32]) -> bool {
        let features: u64;
        // SAFETY: reading `ID_AA64ISAR0_EL1` has no side effects.
        unsafe {
            core::arch::asm!("mrs {}, id_aa64isar0_el1", out(reg) features, options(nomem, nostack, preserves_flags))
        };
        if features >> 60 == 0 {
            return false;
        }
        for chunk in bytes.chunks_exact_mut(8) {
            let mut sample = None;
            for _ in 0..RNDR_TRIES {
                let value: u64;
                let failed: u64;
                // SAFETY: `RNDR` (`S3_3_C2_C4_0`) exists, as `ID_AA64ISAR0_EL1.RNDR` says; it
                // writes a random value and sets `PSTATE.Z` when it could not give one.
                unsafe {
                    core::arch::asm!(
                        "mrs {value}, s3_3_c2_c4_0",
                        "cset {failed}, eq",
                        value = out(reg) value,
                        failed = out(reg) failed,
                        options(nomem, nostack),
                    )
                };
                if failed == 0 {
                    sample = Some(value);
                    break;
                }
            }
            let Some(value) = sample else {
                return false;
            };
            chunk.copy_from_slice(&value.to_ne_bytes());
        }
        true
    }

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
        if device_tree_address() == 0 {
            if let Some(madt) = madt() {
                crate::acpi::madt_gic_cpus(madt, cpu);
            }
            return;
        }
        // SAFETY: as in `memory_map`.
        unsafe { boot_memory::device_tree_cpus(device_tree_address(), cpu) };
    }

    fn boot_cpu() -> u64 {
        timer::BOOT_AFFINITY.load(Ordering::Relaxed)
    }

    unsafe fn start_cpu(cpu: u64, start: &CpuStart) -> Result<(), StartCpuError> {
        unsafe extern "C" {
            fn eggos_secondary_start();
        }
        let entry =
            (eggos_secondary_start as *const () as u64).wrapping_sub(kernel_virtual_offset());
        let context = core::ptr::from_ref(start) as u64;
        // SAFETY: CPU_ON starts CPU `cpu` at the physical address `entry`, the secondary entry
        // code, with `context` in x0; the caller guarantees what that code relies on (the
        // kernel's tables, `start` and its stack), and `psci_call` completes the writes to
        // `*start` before the call.
        let result = unsafe { psci_call(PSCI_CPU_ON, cpu, entry, context) };
        match result {
            PSCI_SUCCESS => Ok(()),
            PSCI_INVALID_PARAMETERS => Err(StartCpuError::InvalidCpu),
            PSCI_ALREADY_ON | PSCI_ON_PENDING => Err(StartCpuError::AlreadyRunning),
            error => Err(StartCpuError::Refused(error)),
        }
    }

    unsafe fn set_cpu_local(block: usize) {
        // SAFETY: TPIDR_EL1 is a scratch register for the kernel's own use; nothing else
        // reads it.
        unsafe { core::arch::asm!("msr tpidr_el1, {}", in(reg) block, options(nomem, nostack)) };
    }

    #[inline]
    fn cpu_local() -> usize {
        let block: usize;
        // SAFETY: reading TPIDR_EL1 has no side effects.
        unsafe { core::arch::asm!("mrs {}, tpidr_el1", out(reg) block, options(nomem, nostack)) };
        block
    }

    fn wake_cpu(cpu: u64) {
        timer::wake(cpu);
    }
}

fn handoff_memory_map(handoff: &crate::handoff::Handoff, whole: bool) -> MemoryMap {
    let mut map = MemoryMap::new();
    for range in crate::handoff::memory_ranges::<Aarch64>(handoff) {
        let physical = PhysicalRange::from_start_length(range.start, range.length);
        if whole {
            map.add_ram(physical.covering_frames());
        } else if range.kind == crate::handoff::memory_kind::RAM {
            map.add_ram(physical);
        }
    }
    if !whole {
        for range in crate::handoff::memory_ranges::<Aarch64>(handoff) {
            if range.kind != crate::handoff::memory_kind::RAM {
                map.reserve(PhysicalRange::from_start_length(range.start, range.length));
            }
        }
    }
    map
}
