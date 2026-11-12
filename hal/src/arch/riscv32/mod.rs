//! RISC-V 32-bit (RV32GC), part 1 (M9, D475): the boot path on QEMU `virt` with OpenSBI.
//!
//! OpenSBI enters `_start` in S-mode with `a0` = hart id and `a1` = device-tree address, the
//! MMU off, the image loaded at 4 MiB into RAM. The entry zeroes `.bss`, builds one Sv32 root
//! of 4 MiB megapages (D474): the direct map, physical RAM from `0x8000_0000` at
//! [`DIRECT_MAP_OFFSET`] above it, `0xc000_0000` on, which also holds the kernel image (linked
//! there); the image's megapage again at its physical address, only for the jump; and the
//! UART's megapage at its physical address, as device memory. It turns translation on, moves
//! to the kernel's virtual address, sets the stack and calls [`early_main`], which reports the
//! boot on the console and stops the machine through SBI. Exceptions, threads, user mode and
//! the rest of [`crate::Platform`] are part 2.

use crate::paging::riscv32::{BOOT_MEGAPAGE, SATP_MODE_SV32};
use core::sync::atomic::{AtomicUsize, Ordering};
use eggos_drivers::io::Mmio;
use eggos_drivers::serial::{Ns16550, Uart};

/// The direct map's offset: virtual address = physical address + this, for RAM.
pub const DIRECT_MAP_OFFSET: u32 = 0x4000_0000;
/// RAM's first physical address on QEMU `virt`.
pub const RAM_START: u32 = 0x8000_0000;
/// The megapages of RAM the boot direct map covers: 896 MiB, up to `0xf800_0000`.
pub const BOOT_DIRECT_MEGAPAGES: u32 = 224;
/// The UART's physical address on QEMU `virt`.
pub const UART: u32 = 0x1000_0000;
/// The timer's frequency on QEMU `virt` (the device tree's `timebase-frequency`).
pub const TIMEBASE_HZ: u64 = 10_000_000;
const UART_BYTES: usize = 8;
const UART_REGISTER_SHIFT: u32 = 0;
const MEGAPAGE_SHIFT: u32 = 22;
const SBI_TIME: usize = 0x5449_4d45;
const SBI_RESET: usize = 0x5352_5354;
const RESET_SHUTDOWN: usize = 0;
const NO_REASON: usize = 0;

#[repr(C, align(4096))]
struct BootRoot([u32; 1024]);

static mut BOOT_ROOT: BootRoot = BootRoot([0; 1024]);
static DEVICE_TREE: AtomicUsize = AtomicUsize::new(0);
static BOOT_HART: AtomicUsize = AtomicUsize::new(0);

core::arch::global_asm!(
    r#"
    .section .text.boot, "ax"
    .global _start
_start:
    lla     t0, __bss_start
    lla     t1, __bss_end
1:  bgeu    t0, t1, 2f
    sw      zero, 0(t0)
    addi    t0, t0, 4
    j       1b

2:  lla     t0, {device_tree}
    sw      a1, 0(t0)
    lla     t0, {boot_hart}
    sw      a0, 0(t0)

    lla     t0, {boot_root}
    li      t1, {ram_start}
    li      t2, {direct_first}
    slli    t2, t2, 2
    add     t2, t2, t0
    li      t3, {direct_megapages}
3:  srli    t4, t1, 2
    ori     t4, t4, {megapage}
    sw      t4, 0(t2)
    li      t5, 1 << {megapage_shift}
    add     t1, t1, t5
    addi    t2, t2, 4
    addi    t3, t3, -1
    bnez    t3, 3b

    lla     t1, __kernel_start
    li      t2, {offset}
    sub     t1, t1, t2
    srli    t3, t1, {megapage_shift}
    slli    t3, t3, 2
    add     t3, t3, t0
    srli    t4, t1, {megapage_shift}
    slli    t4, t4, {megapage_shift}
    srli    t4, t4, 2
    ori     t4, t4, {megapage}
    sw      t4, 0(t3)

    li      t1, {uart}
    srli    t3, t1, {megapage_shift}
    slli    t3, t3, 2
    add     t3, t3, t0
    srli    t4, t1, 2
    ori     t4, t4, {megapage}
    sw      t4, 0(t3)

    srli    t0, t0, 12
    li      t1, {satp_mode}
    or      t0, t0, t1
    sfence.vma
    csrw    satp, t0
    sfence.vma

    lla     t0, 4f
    li      t1, {offset}
    add     t0, t0, t1
    jr      t0

4:  lla     sp, __stack_top
    li      t0, (3 << 13) | (3 << 9)
    csrc    sstatus, t0
    call    {early_main}
5:  wfi
    j       5b
"#,
    device_tree = sym DEVICE_TREE,
    boot_hart = sym BOOT_HART,
    boot_root = sym BOOT_ROOT,
    ram_start = const RAM_START,
    direct_first = const (RAM_START + DIRECT_MAP_OFFSET) >> MEGAPAGE_SHIFT,
    direct_megapages = const BOOT_DIRECT_MEGAPAGES,
    megapage = const BOOT_MEGAPAGE,
    megapage_shift = const MEGAPAGE_SHIFT,
    offset = const DIRECT_MAP_OFFSET,
    uart = const UART,
    satp_mode = const SATP_MODE_SV32,
    early_main = sym early_main,
);

fn uart() -> Ns16550<Mmio> {
    // SAFETY: the boot table maps the UART's megapage at its physical address as device
    // memory, and only this console writes its registers.
    Ns16550::new(unsafe { Mmio::new(UART as usize, UART_BYTES) }, UART_REGISTER_SHIFT)
}

/// Writes `bytes` to the console.
pub fn console_write(bytes: &[u8]) {
    uart().write_bytes(bytes);
}

/// The time counter, read as its two halves until the high one holds still.
pub fn clock_ticks() -> u64 {
    loop {
        let high: u32;
        let low: u32;
        let again: u32;
        // SAFETY: reads the `time` and `timeh` CSRs, which S-mode may read; no side effects.
        unsafe {
            core::arch::asm!(
                "rdtimeh {high}",
                "rdtime {low}",
                "rdtimeh {again}",
                high = out(reg) high,
                low = out(reg) low,
                again = out(reg) again,
            );
        }
        if high == again {
            return u64::from(high) << 32 | u64::from(low);
        }
    }
}

fn sbi(extension: usize, function: usize, first: usize, second: usize) -> isize {
    let error: isize;
    // SAFETY: an SBI call, which takes its arguments in a0 to a7 and returns in a0 and a1;
    // the firmware preserves every other register.
    unsafe {
        core::arch::asm!(
            "ecall",
            inlateout("a0") first => error,
            inlateout("a1") second => _,
            in("a6") function,
            in("a7") extension,
        );
    }
    error
}

/// Asks for the timer interrupt at `deadline` ticks, the 64-bit value split across `a0`
/// and `a1` as RV32's SBI takes it.
pub fn set_timer_deadline(deadline: u64) {
    sbi(SBI_TIME, 0, deadline as u32 as usize, (deadline >> 32) as usize);
}

/// Stops the machine.
pub fn shut_down() -> ! {
    sbi(SBI_RESET, 0, RESET_SHUTDOWN, NO_REASON);
    loop {
        // SAFETY: waits for an interrupt, with no state to keep.
        unsafe { core::arch::asm!("wfi") };
    }
}

/// The device tree's physical address and the boot hart, as the firmware gave them.
pub fn boot_arguments() -> (usize, usize) {
    (
        DEVICE_TREE.load(Ordering::Relaxed),
        BOOT_HART.load(Ordering::Relaxed),
    )
}

extern "C" fn early_main() -> ! {
    console_write(b"EggOS riscv32: translation on, running in the direct map\n");
    let (_, hart) = boot_arguments();
    let started = clock_ticks();
    console_write(b"boot hart ");
    console_write(&[b'0' + (hart % 10) as u8]);
    console_write(b"\n");
    set_timer_deadline(started + TIMEBASE_HZ);
    shut_down()
}
