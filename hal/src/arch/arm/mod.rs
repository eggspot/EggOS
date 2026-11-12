//! 32-bit Arm (Armv7-A with LPAE, and AArch32), part 1 (M9, D476): the boot path on QEMU
//! `virt` with a Cortex-A15.
//!
//! QEMU loads the ELF image at its physical address, 4 MiB into RAM (`0x4040_0000`, the device
//! tree sitting at RAM's start), and enters `_start` in SVC mode with the MMU off. The entry
//! masks interrupts, zeroes `.bss`, and fills the LPAE root (D474) with three 1 GiB blocks:
//! RAM's first gigabyte at [`DIRECT_MAP_OFFSET`] above it (`0xc000_0000` on, holding the kernel
//! image, linked there), the same gigabyte at its physical address for the jump, and the
//! first gigabyte of the physical space (the GIC and the PL011 UART among QEMU `virt`'s
//! devices) at its own address as device memory. It loads `MAIR0`, `TTBCR` (LPAE, `TTBR0`
//! alone, write-back walks) and `TTBR0`, turns the MMU and caches on, jumps to the virtual
//! address, sets the stack and calls `early_main`, which reports on the console, installs the
//! vectors and takes a few virtual-timer interrupts through the GICv2 (D478), and powers the
//! machine off through PSCI. Threads, user mode and the rest of [`crate::Platform`] come next.

use core::sync::atomic::{AtomicU32, Ordering};

/// The direct map's offset: virtual address = physical address + this, for RAM.
pub const DIRECT_MAP_OFFSET: u32 = 0x8000_0000;
/// RAM's first physical address on QEMU `virt`, where the device tree sits.
pub const RAM_START: u32 = 0x4000_0000;
/// The PL011 UART's physical address on QEMU `virt`.
pub const UART: u32 = 0x0900_0000;
const UART_DATA: usize = 0x00;
const UART_FLAGS: usize = 0x18;
const TRANSMIT_FULL: u32 = 1 << 5;
const ROOT_ENTRIES: usize = 4;
const GIGABYTE_SHIFT: u32 = 30;
const BLOCK: u32 = 0b01;
const ACCESS_FLAG: u32 = 1 << 10;
const INNER_SHAREABLE: u32 = 0b11 << 8;
const NORMAL_INDEX: u32 = 1;
const DEVICE_INDEX: u32 = 0;
const ATTRIBUTE_SHIFT: u32 = 2;
const EXECUTE_NEVER_HIGH: u32 = (1u64 << 54 >> 32) as u32 | (1u64 << 53 >> 32) as u32;
const NORMAL_BLOCK: u32 =
    BLOCK | ACCESS_FLAG | INNER_SHAREABLE | NORMAL_INDEX << ATTRIBUTE_SHIFT;
const DEVICE_BLOCK: u32 = BLOCK | ACCESS_FLAG | DEVICE_INDEX << ATTRIBUTE_SHIFT;
const MAIR0: u32 = 0x0044_ff00;
const TTBCR_LPAE: u32 = 1 << 31 | 0b11 << 12 | 0b01 << 10 | 0b01 << 8;
const SCTLR_MMU_AND_CACHES: u32 = 1 | 1 << 2 | 1 << 12;
const PSCI_SYSTEM_OFF: u32 = 0x8400_0008;

#[repr(C, align(32))]
struct BootRoot([u64; ROOT_ENTRIES]);

static mut BOOT_ROOT: BootRoot = BootRoot([0; ROOT_ENTRIES]);
static DEVICE_TREE: AtomicU32 = AtomicU32::new(RAM_START);

core::arch::global_asm!(
    r#"
    .section .text.boot, "ax"
    .arm
    .global _start
_start:
    cpsid   aif
    ldr     r4, ={offset}
    ldr     r0, =__bss_start
    sub     r0, r0, r4
    ldr     r1, =__bss_end
    sub     r1, r1, r4
    mov     r2, #0
1:  cmp     r0, r1
    strlo   r2, [r0], #4
    blo     1b

    ldr     r0, ={boot_root}
    sub     r0, r0, r4
    ldr     r1, ={ram_start}
    ldr     r2, ={normal_block}
    orr     r2, r2, r1
    mov     r3, #0
    lsr     r5, r1, #{gigabyte_shift}
    add     r6, r0, r5, lsl #3
    strd    r2, r3, [r6]
    ldr     r5, ={direct_index}
    add     r6, r0, r5, lsl #3
    strd    r2, r3, [r6]
    ldr     r2, ={device_block}
    ldr     r3, ={execute_never}
    strd    r2, r3, [r0]

    ldr     r1, ={mair0}
    mcr     p15, 0, r1, c10, c2, 0
    ldr     r1, ={ttbcr}
    mcr     p15, 0, r1, c2, c0, 2
    mov     r1, #0
    mcrr    p15, 0, r0, r1, c2
    isb
    mrc     p15, 0, r1, c1, c0, 0
    ldr     r2, ={sctlr}
    orr     r1, r1, r2
    dsb
    mcr     p15, 0, r1, c1, c0, 0
    isb

    ldr     pc, =2f
2:  ldr     sp, =__stack_top
    bl      {early_main}
3:  wfi
    b       3b
    .ltorg
"#,
    offset = const DIRECT_MAP_OFFSET,
    boot_root = sym BOOT_ROOT,
    ram_start = const RAM_START,
    normal_block = const NORMAL_BLOCK,
    device_block = const DEVICE_BLOCK,
    execute_never = const EXECUTE_NEVER_HIGH,
    gigabyte_shift = const GIGABYTE_SHIFT,
    direct_index = const (RAM_START + DIRECT_MAP_OFFSET) >> GIGABYTE_SHIFT,
    mair0 = const MAIR0,
    ttbcr = const TTBCR_LPAE,
    sctlr = const SCTLR_MMU_AND_CACHES,
    early_main = sym early_main,
);

fn uart_register(offset: usize) -> *mut u32 {
    (UART as usize + offset) as *mut u32
}

/// Writes `bytes` to the console, a carriage return before each line feed.
pub fn console_write(bytes: &[u8]) {
    for &byte in bytes {
        if byte == b'\n' {
            put_byte(b'\r');
        }
        put_byte(byte);
    }
}

fn put_byte(byte: u8) {
    // SAFETY: the boot table maps the UART's gigabyte at its physical address as device memory;
    // the flag and data registers are 32-bit and only this console uses them.
    unsafe {
        while uart_register(UART_FLAGS).read_volatile() & TRANSMIT_FULL != 0 {}
        uart_register(UART_DATA).write_volatile(u32::from(byte));
    }
}

/// The generic timer's virtual count.
pub fn clock_ticks() -> u64 {
    let low: u32;
    let high: u32;
    // SAFETY: reads `CNTVCT`, which PL1 may read; no side effects.
    unsafe {
        core::arch::asm!(
            "isb",
            "mrrc p15, 1, {low}, {high}, c14",
            low = out(reg) low,
            high = out(reg) high,
        )
    };
    u64::from(high) << 32 | u64::from(low)
}

/// The generic timer's frequency, from `CNTFRQ`.
pub fn clock_frequency() -> u64 {
    let frequency: u32;
    // SAFETY: reads `CNTFRQ`; no side effects.
    unsafe { core::arch::asm!("mrc p15, 0, {f}, c14, c0, 0", f = out(reg) frequency) };
    u64::from(frequency)
}

/// The device tree's physical address: RAM's start, where QEMU puts it for an ELF image.
pub fn device_tree() -> u32 {
    DEVICE_TREE.load(Ordering::Relaxed)
}

/// Powers the machine off through PSCI's `SYSTEM_OFF`.
pub fn power_off() -> ! {
    // SAFETY: a PSCI call through the hypervisor conduit QEMU `virt` answers; `SYSTEM_OFF`
    // does not return when it works.
    unsafe { core::arch::asm!("hvc #0", inout("r0") PSCI_SYSTEM_OFF => _) };
    loop {
        // SAFETY: waits for an interrupt, with no state to keep.
        unsafe { core::arch::asm!("wfi") };
    }
}

const GIC_DISTRIBUTOR: usize = 0x0800_0000;
const GIC_CPU: usize = 0x0801_0000;
const DISTRIBUTOR_CONTROL: usize = 0x000;
const SET_ENABLE: usize = 0x100;
const PRIORITY: usize = 0x400;
const CPU_CONTROL: usize = 0x000;
const PRIORITY_MASK: usize = 0x004;
const ACKNOWLEDGE: usize = 0x00c;
const END_OF_INTERRUPT: usize = 0x010;
const INTERRUPT_ID: u32 = 0x3ff;
const VIRTUAL_TIMER: u32 = 27;
const MIDDLE_PRIORITY: u8 = 0x80;
const EVERY_PRIORITY: u32 = 0xff;
const TIMER_ENABLE: u32 = 1;
const TICKS_A_SECOND: u64 = 100;
const TICKS_SHOWN: u32 = 3;

static TICKS: AtomicU32 = AtomicU32::new(0);

core::arch::global_asm!(
    r#"
    .section .text, "ax"
    .arm
    .balign 32
    .global arm_vectors
arm_vectors:
    b       arm_fatal
    b       arm_fatal
    b       arm_fatal
    b       arm_fatal
    b       arm_fatal
    b       arm_fatal
    b       arm_irq_entry
    b       arm_fatal

arm_irq_entry:
    sub     lr, lr, #4
    srsdb   sp!, #0x13
    cps     #0x13
    push    {{r0-r3, r12, lr}}
    bl      {irq}
    pop     {{r0-r3, r12, lr}}
    rfeia   sp!

arm_fatal:
    ldr     sp, =__stack_top
    bl      {fatal}
    .ltorg
"#,
    irq = sym irq,
    fatal = sym fatal,
);

fn gic(base: usize, offset: usize) -> *mut u32 {
    (base + offset) as *mut u32
}

fn set_timer(ticks: u32) {
    // SAFETY: writes `CNTV_TVAL` and `CNTV_CTL`: the virtual timer fires `ticks` from now.
    unsafe {
        core::arch::asm!(
            "mcr p15, 0, {ticks}, c14, c3, 0",
            "mcr p15, 0, {enable}, c14, c3, 1",
            "isb",
            ticks = in(reg) ticks,
            enable = in(reg) TIMER_ENABLE,
        )
    };
}

fn tick_interval() -> u32 {
    (clock_frequency() / TICKS_A_SECOND) as u32
}

extern "C" fn irq() {
    // SAFETY: the GIC's CPU interface, mapped as device memory by the boot table's first
    // gigabyte; acknowledging and ending an interrupt are this handler's alone.
    unsafe {
        let acknowledged = gic(GIC_CPU, ACKNOWLEDGE).read_volatile();
        if acknowledged & INTERRUPT_ID == VIRTUAL_TIMER {
            TICKS.fetch_add(1, Ordering::Relaxed);
            set_timer(tick_interval());
        }
        gic(GIC_CPU, END_OF_INTERRUPT).write_volatile(acknowledged);
    }
}

extern "C" fn fatal() -> ! {
    console_write(b"EggOS arm: an exception the boot path does not handle\n");
    power_off()
}

fn enable_timer_interrupts() {
    // SAFETY: points `VBAR` at the vector table, turns on the GIC's distributor and CPU
    // interface for the virtual timer's interrupt, arms the timer, and unmasks IRQs; the
    // vectors and handler are in place first.
    unsafe {
        core::arch::asm!(
            "mcr p15, 0, {vectors}, c12, c0, 0",
            "isb",
            vectors = in(reg) arm_vectors_address(),
        );
        let priority = (GIC_DISTRIBUTOR + PRIORITY + VIRTUAL_TIMER as usize) as *mut u8;
        priority.write_volatile(MIDDLE_PRIORITY);
        gic(GIC_DISTRIBUTOR, SET_ENABLE).write_volatile(1 << VIRTUAL_TIMER);
        gic(GIC_DISTRIBUTOR, DISTRIBUTOR_CONTROL).write_volatile(1);
        gic(GIC_CPU, PRIORITY_MASK).write_volatile(EVERY_PRIORITY);
        gic(GIC_CPU, CPU_CONTROL).write_volatile(1);
        set_timer(tick_interval());
        core::arch::asm!("cpsie i");
    }
}

fn arm_vectors_address() -> usize {
    unsafe extern "C" {
        static arm_vectors: u8;
    }
    &raw const arm_vectors as usize
}

extern "C" fn early_main() -> ! {
    console_write(b"EggOS arm: MMU on, running in the direct map\n");
    enable_timer_interrupts();
    while TICKS.load(Ordering::Relaxed) < TICKS_SHOWN {
        // SAFETY: waits for the next interrupt; the timer's comes within a tick.
        unsafe { core::arch::asm!("wfi") };
    }
    console_write(b"timer interrupts arriving\n");
    power_off()
}
