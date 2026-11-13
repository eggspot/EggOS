//! RISC-V 64-bit (RV64GC).
//!
//! Boot contract: the SBI firmware (OpenSBI, as bundled with QEMU) enters `_start` in S-mode
//! with `a0` = hart id and `a1` = device-tree address, MMU off. The device tree describes the
//! RAM and the firmware's own reservation (see [`Platform::memory_map`]).
//!
//! Board addresses are QEMU `virt`'s for now; device-tree discovery replaces them in M5.

mod console_input;
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
use eggos_drivers::serial::{Ns16550, Uart};

core::arch::global_asm!(
    r#"
    .section .text.boot, "ax"
    .global _start
_start:

    lla     t0, __bss_start
    lla     t1, __bss_end
1:  bgeu    t0, t1, 2f
    sd      zero, 0(t0)
    addi    t0, t0, 8
    j       1b

2:  lla     t0, {device_tree}
    sd      a1, 0(t0)
    lla     t0, {boot_hart}
    sd      a0, 0(t0)

    lla     t0, {boot_root}
    lla     t1, __kernel_start
    srli    t1, t1, 30
    slli    t2, t1, 28              // the gigabyte's PPN, in place
    li      t3, {kernel_leaf}
    or      t2, t2, t3
    slli    t1, t1, 3
    add     t1, t1, t0
    sd      t2, 0(t1)               // identity
    lla     t1, 8f
    ld      t1, 0(t1)               // the kernel's virtual address
    srli    t1, t1, 30
    andi    t1, t1, 511
    slli    t1, t1, 3
    add     t1, t1, t0
    sd      t2, 0(t1)               // the kernel's virtual address
    li      t1, {direct_map_index} * 8
    add     t1, t1, t0
    li      t2, {direct_leaf}
    li      t3, 1 << 28             // one gigabyte, as a PPN in place
    li      t4, {direct_map_gigabytes}
3:  sd      t2, 0(t1)
    add     t2, t2, t3
    addi    t1, t1, 8
    addi    t4, t4, -1
    bnez    t4, 3b

    call    boot_translation_on
    lla     t0, 9f
    ld      t0, 0(t0)
    jr      t0

    .balign 8
8:  .dword  __kernel_start
9:  .dword  4f

4:  lla     sp, __stack_top

    li      t0, (3 << 13) | (3 << 9)
    csrc    sstatus, t0
    lla     t0, exception_entry
    csrw    stvec, t0
    call    {init_timer}
    call    kernel_main
5:  wfi
    j       5b

boot_translation_on:
    lla     t0, {boot_root}
    srli    t0, t0, 12
    li      t1, {satp_mode}
    or      t0, t0, t1
    sfence.vma
    csrw    satp, t0
    sfence.vma
    ret

    .global eggos_secondary_start
eggos_secondary_start:
    mv      s1, a1
    call    boot_translation_on
    lla     t0, 10f
    ld      t0, 0(t0)
    jr      t0
    .balign 8
10: .dword  11f
11: ld      sp, {start_stack_top}(s1)
    li      t0, (3 << 13) | (3 << 9)
    csrc    sstatus, t0
    lla     t0, exception_entry
    csrw    stvec, t0
    mv      a1, a0
    mv      a0, s1
    call    {secondary_main}
12: wfi
    j       12b
"#,
    boot_hart = sym BOOT_HART,
    secondary_main = sym secondary_main,
    start_stack_top = const core::mem::offset_of!(CpuStart, stack_top),
    init_timer = sym timer::init,
    device_tree = sym DEVICE_TREE,
    boot_root = sym BOOT_ROOT,
    kernel_leaf = const paging::riscv64::leaf(0, BOOT_KERNEL),
    direct_leaf = const paging::riscv64::leaf(0, Attributes::READ_WRITE),
    direct_map_index = const paging::index(DIRECT_MAP_OFFSET as u64, 2),
    direct_map_gigabytes = const BOOT_DIRECT_MAP_END >> 30,
    satp_mode = const paging::riscv64::SATP_MODE_SV39,
);

/// The image's virtual address minus its physical address: the kernel links at
/// `0xffff_ffff_8020_0000`, in the top 2 GiB (D33). `linker.ld` uses the same value and checks
/// it against this one.
pub(crate) const KERNEL_VIRTUAL_OFFSET: u64 = 0xffff_ffff_0000_0000;

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
    unsafe { core::arch::asm!("fence w, o", options(nostack, preserves_flags)) };
}

/// The cache block Zicbom's instructions work on, 0 without Zicbom (D563).
pub(crate) fn data_cache_line() -> usize {
    match timer::has_zicbom() {
        true => 64,
        false => 0,
    }
}

/// Writes the cache block at `address` back to memory (`cbo.clean`, D563).
///
/// # Safety
/// `address` must be mapped, and the harts must have Zicbom ([`data_cache_line`] not 0).
pub(crate) unsafe fn clean_data_line(address: usize) {
    // SAFETY: `cbo.clean` writes the block back and changes no memory contents; the caller
    // vouches that `address` is mapped and that Zicbom is there.
    unsafe { core::arch::asm!(".insn i 0x0f, 2, x0, {0}, 1", in(reg) address, options(nostack)) };
}

/// Writes the cache block at `address` back and drops it (`cbo.flush`, D563).
///
/// # Safety
/// As for [`clean_data_line`].
pub(crate) unsafe fn flush_data_line(address: usize) {
    // SAFETY: as `clean_data_line`; `cbo.flush` also drops the clean block.
    unsafe { core::arch::asm!(".insn i 0x0f, 2, x0, {0}, 2", in(reg) address, options(nostack)) };
}

/// Orders the cache maintenance before it ahead of the hart's later memory and device accesses.
pub(crate) fn cache_maintenance_barrier() {
    // SAFETY: a barrier: it touches no memory or register.
    unsafe { core::arch::asm!("fence iorw, iorw", options(nostack, preserves_flags)) };
}

/// The direct map: the start of Sv39's high half (D29). It holds 254 GiB: the top 2 GiB are
/// the kernel's (D33).
const DIRECT_MAP_OFFSET: usize = 0xffff_ffc0_0000_0000;

/// The physical memory the boot direct map covers, and so the most the kernel reaches: 254 GiB.
const BOOT_DIRECT_MAP_END: u64 = 0xffff_ffff_8000_0000 - DIRECT_MAP_OFFSET as u64;

/// `sstatus.SUM`: S-mode may access U-mode pages (see `with_user_access`).
/// The `seed` CSR's `OPST` field: 16 bits of entropy given (`ES16`), or the source failed for
/// good (`DEAD`); the other states ask to read again.
const SEED_ES16: u64 = 2;
const SEED_DEAD: u64 = 3;
/// Reads of the `seed` CSR for one sample before the hart counts as having none to give.
const SEED_TRIES: usize = 100;

/// The calling hart's id (D555): what `set_cpu_local` recorded in its per-CPU block, or the
/// boot hart's before there is one.
pub(super) fn current_hart() -> u64 {
    let block = <Riscv64 as Platform>::cpu_local();
    if block == 0 {
        return BOOT_HART.load(Ordering::Relaxed);
    }
    // SAFETY: a non-zero `tp` is this hart's per-CPU block, with a `HalCpuLocal` at its offset.
    let local = unsafe {
        &*((block + crate::cpu::HAL_CPU_LOCAL_OFFSET) as *const crate::cpu::HalCpuLocal)
    };
    match local.hart_id.load(Ordering::Relaxed) {
        u64::MAX => BOOT_HART.load(Ordering::Relaxed),
        hart => hart,
    }
}

/// `satp.ASID`, bits 59:44 in Sv39.
const SATP_ASID_SHIFT: u32 = 44;
const UNPROBED: u8 = 0;
const ASIDS_USABLE: u8 = 1;
const ASIDS_TOO_FEW: u8 = 2;
static ASID_PROBE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(UNPROBED);

/// Whether the harts implement at least four ASID bits, enough for the tags' eight slots (D552):
/// probed once by writing every ASID bit into `satp` with the active root and reading back what
/// stuck (the privileged specification's way to learn ASIDLEN).
fn asids_work() -> bool {
    match ASID_PROBE.load(core::sync::atomic::Ordering::Relaxed) {
        UNPROBED => {
            let before: u64;
            let after: u64;
            // SAFETY: the same root goes back in at once, with only the ASID field changed, which
            // no translation of the kernel depends on; interrupts are masked as the kernel runs.
            unsafe {
                core::arch::asm!(
                    "csrr {before}, satp",
                    "or {probe}, {before}, {all}",
                    "csrw satp, {probe}",
                    "csrr {after}, satp",
                    "csrw satp, {before}",
                    before = out(reg) before,
                    after = out(reg) after,
                    probe = out(reg) _,
                    all = in(reg) 0xffffu64 << SATP_ASID_SHIFT,
                    options(nostack),
                );
            }
            let _ = before;
            let bits = ((after >> SATP_ASID_SHIFT) & 0xffff).count_ones();
            let usable = bits >= 4;
            ASID_PROBE.store(
                if usable { ASIDS_USABLE } else { ASIDS_TOO_FEW },
                core::sync::atomic::Ordering::Relaxed,
            );
            usable
        }
        state => state == ASIDS_USABLE,
    }
}

const SSTATUS_SUM: u64 = 1 << 18;

/// The kernel's boot mapping: read, write and execute until its own page tables are active.
const BOOT_KERNEL: Attributes = Attributes {
    writable: true,
    executable: true,
    memory_type: MemoryType::Normal,
    user: false,
};

/// The boot page table (see `_start`).
#[repr(C, align(4096))]
struct BootTable([u64; paging::ENTRIES]);

/// Built by `_start` before any Rust code runs. Once the kernel's own page tables are active,
/// only secondary harts use it, to turn translation on (D40); nothing writes it again.
static mut BOOT_ROOT: BootTable = BootTable([0; paging::ENTRIES]);

/// Physical address of the device tree, passed by the SBI firmware in `a1` and stored by the
/// entry code.
static DEVICE_TREE: AtomicUsize = AtomicUsize::new(0);

/// The boot hart's id, passed by the SBI firmware in `a0` and stored by the entry code. Any
/// hart may boot: OpenSBI picks one.
static BOOT_HART: AtomicU64 = AtomicU64::new(0);

/// Called by `eggos_secondary_start` on the `CpuStart`'s stack, at the kernel's virtual address
/// with the boot page table, with the hart's id: switches to the kernel's page tables, sets this
/// hart's timer and external interrupts up (D41, D536), leaves its id in `sscratch` for
/// `set_cpu_local`, then calls the entry.
extern "C" fn secondary_main(start: &CpuStart, hart: u64) -> ! {
    // SAFETY: `start_cpu`'s caller guarantees the tables are the kernel's own, which map
    // everything this hart uses at the addresses it uses now, as they did for the boot hart;
    // interrupts are masked (HSM starts the hart with `sstatus.SIE` clear).
    unsafe { Riscv64::activate_page_tables(&start.page_table_roots[..1]) };
    timer::init();
    console_input::prepare_hart(plic_context_of(hart));
    // SAFETY: `sscratch` holds a user context only once this hart first enters user mode,
    // long after `set_cpu_local` read the id back.
    unsafe { core::arch::asm!("csrw sscratch, {}", in(reg) hart, options(nomem, nostack)) };
    (start.entry)(start.argument)
}

/// The supervisor-mode PLIC context of hart `hart`, which the device tree names.
fn plic_context_of(hart: u64) -> Option<usize> {
    // SAFETY: as in `memory_map`.
    unsafe { boot_memory::device_tree_plic_supervisor_context(device_tree(), hart) }
}

static BOOT_CPU_LOCAL_SET: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// An SBI call (SBI spec v2.0, chapter 3): returns the error (`a0`) and value (`a1`).
///
/// # Safety
/// The call must touch none of the kernel's memory, other than as the caller allows.
unsafe fn sbi_call(extension: usize, function: usize, arguments: [usize; 3]) -> (isize, usize) {
    let (error, value);
    // SAFETY: an SBI call per the SBI calling convention: a7 = extension, a6 = function,
    // a0-a2 = arguments, a0/a1 = returned error/value; the caller guarantees the rest.
    unsafe {
        core::arch::asm!(
            "ecall",
            inout("a0") arguments[0] => error,
            inout("a1") arguments[1] => value,
            in("a2") arguments[2],
            in("a6") function,
            in("a7") extension,
            options(nostack),
        );
    }
    (error, value)
}

/// As [`sbi_call`], with a fourth argument in `a3`.
///
/// # Safety
/// As [`sbi_call`].
unsafe fn sbi_call4(extension: usize, function: usize, arguments: [usize; 4]) -> (isize, usize) {
    let (error, value);
    // SAFETY: as in `sbi_call`, with a3 the fourth argument.
    unsafe {
        core::arch::asm!(
            "ecall",
            inout("a0") arguments[0] => error,
            inout("a1") arguments[1] => value,
            in("a2") arguments[2],
            in("a3") arguments[3],
            in("a6") function,
            in("a7") extension,
            options(nostack),
        );
    }
    (error, value)
}

/// SBI RFENCE extension (SBI spec v2.0, chapter 8): `sbi_remote_sfence_vma`.
const SBI_EXT_RFENCE: usize = 0x5246_4e43;
const SBI_RFENCE_REMOTE_SFENCE_VMA: usize = 1;

/// SBI Base extension `sbi_probe_extension` (SBI spec v2.0, 4.8) and Hart State Management
/// `sbi_hart_start` (9.1), with the errors `hart_start` gives (3.2).
const SBI_EXT_BASE: usize = 0x10;
const SBI_BASE_PROBE_EXTENSION: usize = 3;
const SBI_EXT_HSM: usize = 0x48_534d;
const SBI_HSM_HART_START: usize = 0;
const SBI_SUCCESS: isize = 0;
const SBI_ERR_INVALID_PARAM: isize = -3;
const SBI_ERR_ALREADY_AVAILABLE: isize = -6;

/// The device tree's physical address, or 0 (no tree) if the largest tree read there would
/// reach beyond the boot direct map (the kernel reads it before its own page tables exist).
/// The firmware's device tree, whole, through the boot direct map.
pub(super) fn device_tree_bytes() -> Option<&'static [u8]> {
    // SAFETY: `device_tree()` is 0 or the address OpenSBI passed, which nothing writes and the
    // boot direct map covers (checked there).
    unsafe { boot_memory::device_tree_blob(device_tree()) }
}

fn device_tree() -> usize {
    let address = DEVICE_TREE.load(Ordering::Relaxed);
    if address as u64 > BOOT_DIRECT_MAP_END - boot_memory::MAX_DEVICE_TREE_SIZE as u64 {
        return 0;
    }
    address
}

/// QEMU `virt`: NS16550A UART0, registers 1 byte apart.
const UART0_BASE: usize = 0x1000_0000;
const UART0_REG_SHIFT: u32 = 0;

/// SBI System Reset extension (SBI spec v2.0, chapter 10).
const SBI_EXT_SRST: usize = 0x5352_5354;
const SBI_SRST_SHUTDOWN: usize = 0;
const SBI_SRST_REASON_NONE: usize = 0;
const SBI_SRST_REASON_FAILURE: usize = 1;
const SBI_SRST_COLD_REBOOT: usize = 1;
const SBI_LEGACY_SHUTDOWN: usize = 8;

fn sbi_system_reset(kind: usize, reason: usize) {
    // SAFETY: an SBI call per the SBI calling convention: a7 = extension, a6 = function,
    // a0/a1 = arguments, a0/a1 = returned error/value. It does not return on success; on
    // failure (no SRST) the caller goes on to its fallback.
    unsafe {
        core::arch::asm!(
            "ecall",
            inout("a0") kind => _,
            inout("a1") reason => _,
            in("a6") 0usize,
            in("a7") SBI_EXT_SRST,
            options(nostack),
        );
    }
}

/// The early console, UART0.
#[inline]
fn uart0() -> Ns16550<Mmio> {
    // SAFETY: on QEMU `virt` UART0's register block is at UART0_BASE, which the boot page table
    // and the kernel's own tables map at the direct map offset; only the early console's
    // transmit and receive paths use it. Turning receive interrupts on changes registers
    // transmit relies on, which is sound because the boot CPU does it once, before any other
    // CPU starts.
    let regs = unsafe {
        Mmio::new(
            DIRECT_MAP_OFFSET + UART0_BASE,
            Ns16550::<Mmio>::register_block_size(UART0_REG_SHIFT),
        )
    };
    Ns16550::new(regs, UART0_REG_SHIFT)
}

#[derive(Debug)]
pub struct Riscv64;

impl Platform for Riscv64 {
    const ARCH: &'static str = "riscv64";

    #[inline]
    fn console_write(bytes: &[u8]) {
        uart0().write_bytes(bytes);
    }

    fn console_read(buffer: &mut [u8]) -> usize {
        uart0().read_bytes(buffer)
    }

    fn enable_console_input() -> Result<(), &'static str> {
        let context = plic_context_of(BOOT_HART.load(Ordering::Relaxed));
        console_input::enable(context)?;
        uart0().enable_receive_interrupt();
        Ok(())
    }

    fn route_device_interrupt(
        specifier: &crate::fdt::InterruptSpecifier,
    ) -> Result<crate::DeviceLine, &'static str> {
        use eggos_drivers::interrupt::{aplic, plic};
        let (source, mode) = match console_input::uses_aia() {
            true => aplic::source_of_specifier(specifier.cells())?,
            false => (
                plic::source_of_specifier(specifier.cells())?,
                aplic::SourceMode::LevelHigh,
            ),
        };
        let context = plic_context_of(BOOT_HART.load(Ordering::Relaxed));
        console_input::route_device(context, source, mode)
    }

    fn release_device_interrupt(line: crate::DeviceLine) {
        console_input::release_device(line);
    }

    fn message_interrupt() -> Result<(crate::DeviceLine, crate::InterruptMessage), &'static str> {
        console_input::take_message()
    }

    fn message_for(
        line: crate::DeviceLine,
        cpu: u64,
    ) -> Result<crate::InterruptMessage, &'static str> {
        console_input::message_for(line, cpu)
    }

    fn steer_device_interrupt(line: crate::DeviceLine, cpu: u64) -> Result<(), &'static str> {
        console_input::steer_device(line, cpu, plic_context_of(cpu))
    }

    fn exit(status: ExitStatus) -> ! {
        let reason = match status {
            ExitStatus::Success => SBI_SRST_REASON_NONE,
            ExitStatus::Failure => SBI_SRST_REASON_FAILURE,
        };
        sbi_system_reset(SBI_SRST_SHUTDOWN, reason);
        // SAFETY: the legacy SBI v0.1 shutdown takes no arguments and does not return when the
        // firmware has it; firmware without it returns an error and we halt.
        unsafe {
            core::arch::asm!(
                "ecall",
                inout("a0") 0usize => _,
                in("a7") SBI_LEGACY_SHUTDOWN,
                options(nostack),
            );
        }
        Self::halt()
    }

    fn restart() -> ! {
        sbi_system_reset(SBI_SRST_COLD_REBOOT, SBI_SRST_REASON_NONE);
        Self::halt()
    }

    fn halt() -> ! {
        loop {
            // SAFETY: `wfi` only pauses the hart until an interrupt is pending.
            unsafe { core::arch::asm!("wfi", options(nomem, nostack, preserves_flags)) };
        }
    }

    fn breakpoint() {
        // SAFETY: `ebreak` traps to the vector installed at boot, whose handler saves and
        // restores every general-purpose register and resumes after the `ebreak`.
        // No `nomem`/`nostack`: the kernel's handler may read and write any memory, and the
        // exception frame is pushed below the stack pointer.
        unsafe { core::arch::asm!("ebreak") };
    }

    fn execute_illegal_instruction() -> ! {
        // SAFETY: `unimp` is defined to be illegal; it traps to the vector installed at boot,
        // and the kernel never resumes from that exception.
        unsafe { core::arch::asm!("unimp", options(noreturn)) }
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
        // SAFETY: `wfi` wakes once an interrupt enabled in `sie` is pending, even with
        // `sstatus.SIE` clear; setting SIE then takes it through the trap vector installed at
        // boot, which restores every register, and clearing SIE masks interrupts again.
        // No `nomem`/`nostack`: the kernel's handler may read and write any memory, and the
        // trap frame is pushed below the stack pointer.
        unsafe { core::arch::asm!("wfi", "csrsi sstatus, 2", "csrci sstatus, 2") };
    }

    #[inline]
    fn interrupt_window() {
        // SAFETY: as in `wait_for_interrupt`, without the `wfi`: an interrupt already pending
        // is taken as soon as the write sets SIE, before the next instruction clears it.
        unsafe { core::arch::asm!("csrsi sstatus, 2", "csrci sstatus, 2") };
    }

    fn memory_map() -> MemoryMap {
        // SAFETY: the SBI firmware passes the address of its device tree in `a1` (the RISC-V
        // boot convention, which OpenSBI follows), which stays readable and unwritten; it is in
        // RAM, which the direct map covers (`device_tree` checks it is inside the boot direct
        // map).
        let mut map = unsafe { boot_memory::device_tree_memory_map(device_tree()) };
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

    type PageTableFormat = paging::riscv64::Riscv64Sv39;

    fn direct_map_offset() -> usize {
        DIRECT_MAP_OFFSET
    }

    fn kernel_image() -> KernelImage {
        boot_memory::kernel_image_parts()
    }

    fn physical_memory() -> MemoryMap {
        // SAFETY: as in `memory_map`.
        let mut map = unsafe { boot_memory::device_tree_ram(device_tree()) };
        map.retain_within(PhysicalRange {
            start: 0,
            end: BOOT_DIRECT_MAP_END,
        });
        map
    }

    fn early_device_regions(region: &mut dyn FnMut(PhysicalRange)) {
        region(PhysicalRange::from_start_length(
            UART0_BASE as u64,
            Ns16550::<Mmio>::register_block_size(UART0_REG_SHIFT) as u64,
        ));
        console_input::device_regions(region);
    }

    fn device_tree() -> Option<&'static [u8]> {
        // SAFETY: as in `memory_map`.
        unsafe { boot_memory::device_tree_blob(device_tree()) }
    }

    fn make_kernel_mappings_visible(_start: usize, _length: usize) {
        // SAFETY: a full fence orders the table writes before the walks that follow, and
        // `sfence.vma` drops whatever this hart cached, invalid entries included; neither changes
        // memory contents.
        unsafe { core::arch::asm!("fence rw, rw", "sfence.vma", options(nostack)) };
    }

    unsafe fn activate_page_tables(roots: &[u64]) {
        let [root] = roots else {
            panic!("Sv39 page tables have one root");
        };
        let satp = paging::riscv64::SATP_MODE_SV39 | root >> 12;
        // SAFETY: the caller guarantees the tables map everything this hart uses from here on
        // at the addresses it uses now: the kernel image at its virtual address, the direct
        // map and the UART in it. The privileged spec lets `satp` change under running code
        // mapped at the same address in both trees; the first `sfence.vma` orders the table
        // writes before the walker reads them, the second drops translations cached from the
        // boot table. No `nomem`: memory accesses must not move across the switch.
        unsafe {
            core::arch::asm!(
                "sfence.vma",
                "csrw satp, {satp}",
                "sfence.vma",
                satp = in(reg) satp,
                options(nostack, preserves_flags),
            );
        }
    }

    unsafe fn switch_address_space(roots: &[u64]) {
        // SAFETY: a full fence orders this hart's earlier stores before its later accesses and
        // table walks; it changes no memory contents.
        unsafe { core::arch::asm!("fence rw, rw", options(nostack)) };
        // SAFETY: the caller's guarantees include `activate_page_tables`'.
        unsafe { Self::activate_page_tables(roots) };
        // SAFETY: as in `make_instructions_visible`: it changes no memory contents.
        unsafe { core::arch::asm!("fence.i", options(nostack)) };
    }

    unsafe fn switch_tagged_address_space(roots: &[u64], tag: crate::asid::AddressSpaceTag) {
        let [root] = roots else {
            panic!("Sv39 page tables have one root");
        };
        if !asids_work() {
            // SAFETY: the caller's guarantees include `switch_address_space`'s.
            return unsafe { Self::switch_address_space(roots) };
        }
        let block = Self::cpu_local();
        // SAFETY: the per-CPU register holds this hart's block, with a `HalCpuLocal` at its
        // offset; only this hart uses its tags, with interrupts masked (the caller's duty).
        let choice = unsafe {
            let hal = &*((block + crate::cpu::HAL_CPU_LOCAL_OFFSET) as *const crate::cpu::HalCpuLocal);
            hal.address_space_tags.choose(tag)
        };
        let asid = u64::from(choice.hardware_tag);
        let satp = paging::riscv64::SATP_MODE_SV39 | asid << SATP_ASID_SHIFT | root >> 12;
        // SAFETY: as in `switch_address_space`: the fence orders this hart's stores before the
        // new tables' walks, the kernel half is the active one's, and `fence.i` makes other
        // harts' code visible here. Translations under the ASID are dropped only when the tags
        // say they are stale; the kernel's global ones stay.
        unsafe {
            core::arch::asm!("fence rw, rw", "csrw satp, {satp}", satp = in(reg) satp, options(nostack));
            if choice.drop_old {
                core::arch::asm!("sfence.vma zero, {asid}", asid = in(reg) asid, options(nostack));
            }
            core::arch::asm!("fence.i", options(nostack));
        }
    }

    fn with_user_access<R>(access: impl FnOnce() -> R) -> R {
        let before: u64;
        // SAFETY: `sstatus.SUM` only widens what S-mode may load and store to U-mode pages; it
        // goes back to what it was below (clear unless an outer window is open), and nothing
        // else in the kernel sets it. No `nomem`: the accesses in `access` must not move out of
        // the window.
        unsafe {
            core::arch::asm!(
                "csrrs {before}, sstatus, {sum}",
                before = out(reg) before,
                sum = in(reg) SSTATUS_SUM,
                options(nostack),
            );
        }
        let result = access();
        if before & SSTATUS_SUM == 0 {
            // SAFETY: as above: closes the window this call opened.
            unsafe { core::arch::asm!("csrc sstatus, {}", in(reg) SSTATUS_SUM, options(nostack)) };
        }
        result
    }

    unsafe fn run_user(context: &mut crate::user::UserContext) -> crate::user::UserExit {
        // SAFETY: the caller's guarantees are `run_user`'s.
        unsafe { exception::run_user(context) }
    }

    fn user_test_program() -> crate::user::TestProgram {
        exception::user_test_program()
    }

    fn linux_hardware_capabilities() -> [u64; 2] {
        let letter = |extension: u8| 1u64 << (extension - b'A');
        [
            letter(b'I')
                | letter(b'M')
                | letter(b'A')
                | letter(b'F')
                | letter(b'D')
                | letter(b'C')
                | if fp::vector_bytes() != 0 { letter(b'V') } else { 0 },
            0,
        ]
    }

    fn make_instructions_visible(_start: usize, _length: usize) {
        // SAFETY: `fence.i` orders this hart's instruction fetches after its earlier stores;
        // it changes no memory contents.
        unsafe { core::arch::asm!("fence.i", options(nostack)) };
    }

    fn page_fault(_kind: crate::ExceptionKind, code: u64) -> Option<paging::PageFault> {
        use crate::paging::{Access, PageFault};
        let access = match code {
            12 => Access::Execute,
            13 => Access::Read,
            15 => Access::Write,
            _ => return None,
        };
        Some(PageFault {
            access,
            may_be_stale: true,
        })
    }

    fn drop_kernel_translations(start: u64, length: u64) {
        for page in 0..length / crate::memory::FRAME_SIZE {
            let address = start + page * crate::memory::FRAME_SIZE;
            // SAFETY: as in `drop_user_translations`: one page, every address space, global
            // translations included.
            unsafe { core::arch::asm!("sfence.vma {}, zero", in(reg) address, options(nostack)) };
        }
    }

    fn drop_kernel_translations_everywhere(start: u64, length: u64) -> bool {
        Self::drop_user_translations_everywhere(start, length)
    }

    fn drop_user_translations_everywhere(start: u64, length: u64) -> bool {
        // SAFETY: the SBI RFENCE extension's `sbi_remote_sfence_vma` (SBI spec v2.0, 8.2) with
        // `hart_mask_base` -1, every hart: the firmware runs `sfence.vma` over the range on each
        // and returns once they have; it touches none of the kernel's memory.
        let (error, _) = unsafe {
            sbi_call4(
                SBI_EXT_RFENCE,
                SBI_RFENCE_REMOTE_SFENCE_VMA,
                [0, usize::MAX, start as usize, length as usize],
            )
        };
        error == SBI_SUCCESS
    }

    fn drop_user_translations(start: u64, length: u64) {
        let pages = length / crate::memory::FRAME_SIZE;
        if pages > crate::paging::USER_PAGES_DROPPED_ONE_BY_ONE {
            // SAFETY: `sfence.vma` orders this hart's earlier page-table writes before its later
            // walks and drops cached translations; it changes no memory contents. Every address
            // space (no ASIDs are used), the kernel's global ones too, which costs refills only.
            unsafe { core::arch::asm!("sfence.vma zero, zero", options(nostack)) };
            return;
        }
        if timer::has_svinval() {
            // SAFETY: Svinval's three instructions (the device tree listed `svinval`): the first
            // orders earlier page-table writes before the invalidations, `sinval.vma` drops one
            // page's translations in every address space without waiting, and the last orders
            // them before later walks. Encoded as words for assemblers without the extension.
            unsafe {
                core::arch::asm!(".4byte 0x18000073", options(nostack));
                for page in 0..pages {
                    let address = start + page * crate::memory::FRAME_SIZE;
                    core::arch::asm!(".4byte 0x16050073", in("a0") address, options(nostack));
                }
                core::arch::asm!(".4byte 0x18100073", options(nostack));
            }
            return;
        }
        for page in 0..pages {
            let address = start + page * crate::memory::FRAME_SIZE;
            // SAFETY: as above, for one page, in every address space.
            unsafe { core::arch::asm!("sfence.vma {}, zero", in(reg) address, options(nostack)) };
        }
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
        unsafe { boot_memory::device_tree_take_seeds(device_tree(), seed) };
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
        if !timer::has_zkr() {
            return false;
        }
        for pair in bytes.chunks_exact_mut(2) {
            let mut sample = None;
            for _ in 0..SEED_TRIES {
                let value: u64;
                // SAFETY: the `seed` CSR (0x015) exists with Zkr, which the device tree listed,
                // and the firmware gives S-mode access to it there (`mseccfg.SSEED`); reading it
                // with a write of zero only takes a sample.
                unsafe { core::arch::asm!("csrrw {}, 0x015, zero", out(reg) value, options(nomem, nostack)) };
                match value >> 30 & 3 {
                    SEED_ES16 => {
                        sample = Some(value as u16);
                        break;
                    }
                    SEED_DEAD => return false,
                    _ => {}
                }
            }
            let Some(value) = sample else {
                return false;
            };
            pair.copy_from_slice(&value.to_ne_bytes());
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
        // SAFETY: as in `memory_map`.
        unsafe { boot_memory::device_tree_cpus(device_tree(), cpu) };
    }

    fn boot_cpu() -> u64 {
        BOOT_HART.load(Ordering::Relaxed)
    }

    unsafe fn start_cpu(cpu: u64, start: &CpuStart) -> Result<(), StartCpuError> {
        unsafe extern "C" {
            /// The secondary entry point in the boot code above.
            fn eggos_secondary_start();
        }
        // SAFETY: probing an extension touches no memory.
        let (_, hsm) =
            unsafe { sbi_call(SBI_EXT_BASE, SBI_BASE_PROBE_EXTENSION, [SBI_EXT_HSM, 0, 0]) };
        if hsm == 0 {
            return Err(StartCpuError::Refused(0));
        }
        let entry = (eggos_secondary_start as *const () as u64).wrapping_sub(KERNEL_VIRTUAL_OFFSET);
        let opaque = core::ptr::from_ref(start) as usize;
        core::sync::atomic::fence(Ordering::SeqCst);
        // SAFETY: `hart_start` starts hart `cpu` at the physical address `entry`, the secondary
        // entry code, with `opaque` in a1; the caller guarantees what that code relies on (the
        // kernel's tables, `start` and its stack). The new hart reads `*start`: the fence
        // above orders the writes to it before the call, for the compiler (`sbi_call` is no
        // `nomem` asm either) and for the other harts (`fence rw, rw`).
        let (error, _) = unsafe {
            sbi_call(
                SBI_EXT_HSM,
                SBI_HSM_HART_START,
                [cpu as usize, entry as usize, opaque],
            )
        };
        match error {
            SBI_SUCCESS => Ok(()),
            SBI_ERR_INVALID_PARAM => Err(StartCpuError::InvalidCpu),
            SBI_ERR_ALREADY_AVAILABLE => Err(StartCpuError::AlreadyRunning),
            error => Err(StartCpuError::Refused(error as i64)),
        }
    }

    unsafe fn set_cpu_local(block: usize) {
        // SAFETY: the kernel never uses thread-local storage, so nothing else reads or writes
        // `tp` while in the kernel (D40); the trap entry neither saves nor changes it.
        unsafe { core::arch::asm!("mv tp, {}", in(reg) block, options(nomem, nostack)) };
        let hart = match BOOT_CPU_LOCAL_SET.swap(true, Ordering::Relaxed) {
            false => BOOT_HART.load(Ordering::Relaxed),
            true => {
                let hart: u64;
                // SAFETY: reading `sscratch` has no side effects; on a secondary hart it still
                // holds the id `secondary_main` left there.
                unsafe {
                    core::arch::asm!("csrr {}, sscratch", out(reg) hart, options(nomem, nostack))
                };
                hart
            }
        };
        let context = plic_context_of(hart).map_or(u64::MAX, |context| context as u64);
        // SAFETY: the caller guarantees `block` is a lasting per-CPU block with a `HalCpuLocal`
        // at `HAL_CPU_LOCAL_OFFSET`, which only the HAL uses.
        let local = unsafe {
            &*((block + crate::cpu::HAL_CPU_LOCAL_OFFSET) as *const crate::cpu::HalCpuLocal)
        };
        local.plic_context.store(context, Ordering::Relaxed);
        local.hart_id.store(hart, Ordering::Relaxed);
    }

    #[inline]
    fn cpu_local() -> usize {
        let block: usize;
        // SAFETY: reading `tp` has no side effects.
        unsafe { core::arch::asm!("mv {}, tp", out(reg) block, options(nomem, nostack)) };
        block
    }

    #[inline]
    unsafe fn cpu_local_u64<const OFFSET: usize>() -> u64 {
        let value: u64;
        // SAFETY: the caller guarantees `tp` holds this hart's block, with an aligned
        // `AtomicU64` (or a word nothing writes meanwhile) at `OFFSET`: one aligned 8-byte load,
        // which never tears.
        unsafe {
            core::arch::asm!(
                "ld {}, {offset}(tp)",
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
