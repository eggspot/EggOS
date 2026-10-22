//! RISC-V 64-bit (RV64GC).
//!
//! Boot contract: the SBI firmware (OpenSBI, as bundled with QEMU) enters `_start` in S-mode
//! with `a0` = hart id and `a1` = device-tree address, MMU off. The device tree describes the
//! RAM and the firmware's own reservation (see [`Platform::memory_map`]).
//!
//! Board addresses are QEMU `virt`'s for now; device-tree discovery replaces them in M5.

mod exception;
mod timer;

use crate::arch::boot_memory;
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
    // SBI starts only the boot hart; others stay stopped until started through SBI HSM. The
    // MMU is off, so `lla` (PC-relative) gives physical addresses until the jump below.

    // Zero .bss (8-byte aligned and sized by the linker script); the boot page table is there.
    lla     t0, __bss_start
    lla     t1, __bss_end
1:  bgeu    t0, t1, 2f
    sd      zero, 0(t0)
    addi    t0, t0, 8
    j       1b

    // Keep the device-tree address (after zeroing .bss, where it is stored).
2:  lla     t0, {device_tree}
    sd      a1, 0(t0)

    // The boot page table (D33), one Sv39 root of 1 GiB leaves: the gigabyte holding the
    // kernel, read, write and execute, at its physical address (only for the jump below) and
    // at the kernel's virtual address, and the boot direct map: the first
    // {direct_map_gigabytes} GiB of physical memory, read and write, at the direct map offset.
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

    // Turn on translation, then continue at the kernel's virtual address.
    srli    t0, t0, 12
    li      t1, {satp_mode}
    or      t0, t0, t1
    sfence.vma
    csrw    satp, t0
    sfence.vma
    lla     t0, 9f
    ld      t0, 0(t0)
    jr      t0

    .balign 8
8:  .dword  __kernel_start
9:  .dword  4f

    // From here on the kernel runs at its virtual address, and `lla` gives virtual addresses.
4:  lla     sp, __stack_top

    // FPU off: the kernel uses no FP registers and the trap entry does not save them, so any
    // FP instruction traps as illegal. Then the trap vector, before any Rust code runs, and
    // the timer.
    li      t0, 3 << 13
    csrc    sstatus, t0
    lla     t0, exception_entry
    csrw    stvec, t0
    call    {init_timer}
    call    kernel_main
5:  wfi
    j       5b
"#,
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

/// The direct map: the start of Sv39's high half (D29). It holds 254 GiB: the top 2 GiB are
/// the kernel's (D33).
const DIRECT_MAP_OFFSET: usize = 0xffff_ffc0_0000_0000;

/// The physical memory the boot direct map covers, and so the most the kernel reaches: 254 GiB.
const BOOT_DIRECT_MAP_END: u64 = 0xffff_ffff_8000_0000 - DIRECT_MAP_OFFSET as u64;

/// The kernel's boot mapping: read, write and execute until its own page tables are active.
const BOOT_KERNEL: Attributes = Attributes {
    writable: true,
    executable: true,
    memory_type: MemoryType::Normal,
};

/// The boot page table (see `_start`).
#[repr(C, align(4096))]
struct BootTable([u64; paging::ENTRIES]);

/// Built by `_start` before any Rust code runs; never used again once the kernel's own page
/// tables are active.
static mut BOOT_ROOT: BootTable = BootTable([0; paging::ENTRIES]);

/// Physical address of the device tree, passed by the SBI firmware in `a1` and stored by the
/// entry code.
static DEVICE_TREE: AtomicUsize = AtomicUsize::new(0);

/// The device tree's physical address, or 0 (no tree) if the largest tree read there would
/// reach beyond the boot direct map (the kernel reads it before its own page tables exist).
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

#[derive(Debug)]
pub struct Riscv64;

impl Platform for Riscv64 {
    const ARCH: &'static str = "riscv64";

    #[inline]
    fn console_write(bytes: &[u8]) {
        // SAFETY: on QEMU `virt` UART0's register block is at UART0_BASE, which the boot page
        // table and the kernel's own tables map at the direct map offset; the early console is
        // its only user.
        let regs = unsafe {
            Mmio::new(
                DIRECT_MAP_OFFSET + UART0_BASE,
                Ns16550::<Mmio>::register_block_size(UART0_REG_SHIFT),
            )
        };
        Ns16550::new(regs, UART0_REG_SHIFT).write_bytes(bytes);
    }

    fn exit(status: ExitStatus) -> ! {
        let reason = match status {
            ExitStatus::Success => SBI_SRST_REASON_NONE,
            ExitStatus::Failure => SBI_SRST_REASON_FAILURE,
        };
        // SAFETY: an SBI call per the SBI calling convention: a7 = extension, a6 = function,
        // a0/a1 = arguments, a0/a1 = returned error/value. It does not return on success; on
        // failure (no SRST) we fall through and halt.
        unsafe {
            core::arch::asm!(
                "ecall",
                inout("a0") SBI_SRST_SHUTDOWN => _,
                inout("a1") reason => _,
                in("a6") 0usize,
                in("a7") SBI_EXT_SRST,
                options(nostack),
            );
        }
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
        timer::TIMEBASE_FREQUENCY
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

    fn memory_map() -> MemoryMap {
        // SAFETY: the SBI firmware passes the address of its device tree in `a1` (the RISC-V
        // boot convention, which OpenSBI follows), which stays readable and unwritten; it is in
        // RAM, which the direct map covers (`device_tree` checks it is inside the boot direct
        // map).
        let mut map = unsafe { boot_memory::device_tree_memory_map(device_tree()) };
        // The kernel's page tables are built through the boot direct map.
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
        // RAM beyond the boot direct map is never handed out (see `memory_map`), so it is left
        // unmapped.
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
}
