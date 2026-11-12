//! RISC-V 32-bit (RV32GC), part 1 (M9, D475): the boot path on QEMU `virt` with OpenSBI.
//!
//! OpenSBI enters `_start` in S-mode with `a0` = hart id and `a1` = device-tree address, the
//! MMU off, the image loaded at 4 MiB into RAM. The entry zeroes `.bss`, builds one Sv32 root
//! of 4 MiB megapages (D474): the direct map, physical RAM from `0x8000_0000` at
//! [`DIRECT_MAP_OFFSET`] above it, `0xc000_0000` on, which also holds the kernel image (linked
//! there); the image's megapage again at its physical address, only for the jump; and the
//! UART's megapage at its physical address, as device memory. It turns translation on, moves
//! to the kernel's virtual address, sets the stack and the trap vector, and calls
//! `early_main`, which reports the boot on the console, waits for a few timer interrupts
//! (D477: a trap saves every register and the trap CSRs in a [`TrapFrame`] on the stack) and
//! stops the machine through SBI. Threads, user mode and the rest of [`crate::Platform`] come
//! next.


mod thread;

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
    lla     t0, riscv32_trap_entry
    csrw    stvec, t0
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

/// The registers a trap saves on the stack: `x0` to `x31` (`x0` unused, `x2` the stack pointer
/// before the trap), then `sepc`, `sstatus`, `scause` and `stval`.
#[repr(C, align(16))]
#[derive(Debug)]
pub struct TrapFrame {
    pub registers: [u32; 32],
    pub pc: u32,
    pub status: u32,
    pub cause: u32,
    pub value: u32,
}

static TICKS: AtomicUsize = AtomicUsize::new(0);
const INTERRUPT: u32 = 1 << 31;
const SUPERVISOR_TIMER: u32 = 5;
const TIMER_ENABLE: usize = 1 << 5;
const INTERRUPTS_ENABLE: usize = 1 << 1;
const TICKS_A_SECOND: u64 = 100;
const TICKS_SHOWN: usize = 3;

extern "C" fn trap(frame: &mut TrapFrame) {
    match (frame.cause & INTERRUPT != 0, frame.cause & !INTERRUPT) {
        (true, SUPERVISOR_TIMER) => {
            TICKS.fetch_add(1, Ordering::Relaxed);
            set_timer_deadline(clock_ticks() + TIMEBASE_HZ / TICKS_A_SECOND);
        }
        _ => {
            console_write(b"EggOS riscv32: an exception the boot path does not handle\n");
            shut_down()
        }
    }
}

fn enable_timer_interrupts() {
    // SAFETY: sets `sie.STIE` and `sstatus.SIE`; the trap vector is installed, so a timer
    // interrupt reaches `trap`.
    unsafe {
        core::arch::asm!(
            "csrs sie, {timer}",
            "csrs sstatus, {interrupts}",
            timer = in(reg) TIMER_ENABLE,
            interrupts = in(reg) INTERRUPTS_ENABLE,
        )
    };
}

core::arch::global_asm!(
    r#"
    .section .text, "ax"
    .balign 4
    .global riscv32_trap_entry
riscv32_trap_entry:
    addi    sp, sp, -{frame}
    sw      x1, 4(sp)
    sw      x3, 12(sp)
    sw      x4, 16(sp)
    sw      x5, 20(sp)
    sw      x6, 24(sp)
    sw      x7, 28(sp)
    sw      x8, 32(sp)
    sw      x9, 36(sp)
    sw      x10, 40(sp)
    sw      x11, 44(sp)
    sw      x12, 48(sp)
    sw      x13, 52(sp)
    sw      x14, 56(sp)
    sw      x15, 60(sp)
    sw      x16, 64(sp)
    sw      x17, 68(sp)
    sw      x18, 72(sp)
    sw      x19, 76(sp)
    sw      x20, 80(sp)
    sw      x21, 84(sp)
    sw      x22, 88(sp)
    sw      x23, 92(sp)
    sw      x24, 96(sp)
    sw      x25, 100(sp)
    sw      x26, 104(sp)
    sw      x27, 108(sp)
    sw      x28, 112(sp)
    sw      x29, 116(sp)
    sw      x30, 120(sp)
    sw      x31, 124(sp)
    addi    t0, sp, {frame}
    sw      t0, 8(sp)
    csrr    t0, sepc
    sw      t0, {pc}(sp)
    csrr    t0, sstatus
    sw      t0, {status}(sp)
    csrr    t0, scause
    sw      t0, {cause}(sp)
    csrr    t0, stval
    sw      t0, {value}(sp)
    mv      a0, sp
    call    {trap}
    lw      t0, {pc}(sp)
    csrw    sepc, t0
    lw      t0, {status}(sp)
    csrw    sstatus, t0
    lw      x1, 4(sp)
    lw      x3, 12(sp)
    lw      x4, 16(sp)
    lw      x5, 20(sp)
    lw      x6, 24(sp)
    lw      x7, 28(sp)
    lw      x8, 32(sp)
    lw      x9, 36(sp)
    lw      x10, 40(sp)
    lw      x11, 44(sp)
    lw      x12, 48(sp)
    lw      x13, 52(sp)
    lw      x14, 56(sp)
    lw      x15, 60(sp)
    lw      x16, 64(sp)
    lw      x17, 68(sp)
    lw      x18, 72(sp)
    lw      x19, 76(sp)
    lw      x20, 80(sp)
    lw      x21, 84(sp)
    lw      x22, 88(sp)
    lw      x23, 92(sp)
    lw      x24, 96(sp)
    lw      x25, 100(sp)
    lw      x26, 104(sp)
    lw      x27, 108(sp)
    lw      x28, 112(sp)
    lw      x29, 116(sp)
    lw      x30, 120(sp)
    lw      x31, 124(sp)
    addi    sp, sp, {frame}
    sret
"#,
    frame = const core::mem::size_of::<TrapFrame>(),
    pc = const core::mem::offset_of!(TrapFrame, pc),
    status = const core::mem::offset_of!(TrapFrame, status),
    cause = const core::mem::offset_of!(TrapFrame, cause),
    value = const core::mem::offset_of!(TrapFrame, value),
    trap = sym trap,
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
    console_write(b"boot hart ");
    console_write(&[b'0' + (hart % 10) as u8]);
    console_write(b"\n");
    set_timer_deadline(clock_ticks() + TIMEBASE_HZ / TICKS_A_SECOND);
    enable_timer_interrupts();
    while TICKS.load(Ordering::Relaxed) < TICKS_SHOWN {
        // SAFETY: waits for the next interrupt; the timer's comes within a tick.
        unsafe { core::arch::asm!("wfi") };
    }
    console_write(b"timer interrupts arriving\n");
    shut_down()
}
