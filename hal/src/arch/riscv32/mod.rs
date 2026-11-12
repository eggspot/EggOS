//! RISC-V 32-bit (RV32GC), part 1 (M9, D475): the boot path on QEMU `virt` with OpenSBI.
//!
//! OpenSBI enters `_start` in S-mode with `a0` = hart id and `a1` = device-tree address, the
//! MMU off, the image loaded at 4 MiB into RAM. The entry zeroes `.bss`, builds one Sv32 root
//! of 4 MiB megapages (D474): the direct map, physical RAM from `0x8000_0000` at
//! [`DIRECT_MAP_OFFSET`] above it, `0xc000_0000` on, which also holds the kernel image (linked
//! there); the image's megapage again at its physical address, only for the jump; and the
//! UART's megapage at its physical address, as device memory. It turns translation on, moves
//! to the kernel's virtual address, sets the stack and the trap vector (D477: a trap saves
//! every register and the trap CSRs in a [`TrapFrame`] on the stack), enables the timer and
//! wake-up interrupts, and calls the kernel's `kernel_main`, [`Riscv32`] being its
//! [`crate::Platform`] (D481).


mod fp;
mod test_program;
mod thread;
mod user;

use crate::arch::boot_memory;
use crate::cpu::{CpuStart, StartCpuError};
use crate::interrupt::Interrupt;
use crate::memory::PhysicalRange;
use crate::paging::riscv32::{BOOT_MEGAPAGE, SATP_MODE_SV32};
use crate::paging::{self, KernelImage};
use crate::{Exception, ExitStatus, MemoryMap, Platform};
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
    call    {enable_timer}
    call    kernel_main
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
    enable_timer = sym enable_timer_and_wake_ups,
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

const INTERRUPT: u32 = 1 << 31;
const SUPERVISOR_TIMER: u32 = 5;
const SUPERVISOR_SOFTWARE: u32 = 1;
const SOFTWARE_PENDING: usize = 1 << 1;
const TIMER_ENABLE: usize = 1 << 5;
const SOFTWARE_ENABLE: usize = 1 << 1;
const INTERRUPTS_ENABLE: usize = 1 << 1;
const USER_MEMORY_ACCESS: usize = 1 << 18;
const SBI_IPI: usize = 0x0073_5049;
const SECONDARY_HARTS_NOT_YET: i64 = 0;
const DEVICE_WINDOW: core::ops::Range<u64> = 0xff80_0000..0xffc0_0000;

/// Handles interrupt `cause` (`scause`), from the kernel or from user mode: the timer's is
/// disarmed and handed to the kernel, as is a wake-up from another hart.
pub(super) fn interrupt(cause: u32) {
    match cause & !INTERRUPT {
        SUPERVISOR_TIMER => {
            set_timer_deadline(u64::MAX);
            crate::interrupt::deliver(Interrupt::Timer);
        }
        SUPERVISOR_SOFTWARE => {
            // SAFETY: clearing `sip.SSIP` only marks the wake-up handled.
            unsafe { core::arch::asm!("csrc sip, {}", in(reg) SOFTWARE_PENDING) };
            crate::interrupt::deliver(Interrupt::Wakeup);
        }
        _ => {}
    }
}

extern "C" fn trap(frame: &mut TrapFrame) {
    match frame.cause & INTERRUPT != 0 {
        true => interrupt(frame.cause),
        false => crate::exception::deliver(&Exception {
            kind: crate::ExceptionKind::Other,
            name: "kernel exception",
            code: u64::from(frame.cause),
            pc: u64::from(frame.pc),
            address: Some(u64::from(frame.value)),
        }),
    }
}

extern "C" fn enable_timer_and_wake_ups() {
    set_timer_deadline(u64::MAX);
    // SAFETY: sets `sie.STIE` and `sie.SSIE`; the trap vector is installed, and `sstatus.SIE`
    // stays clear but in `wait_for_interrupt`.
    unsafe { core::arch::asm!("csrs sie, {}", in(reg) TIMER_ENABLE | SOFTWARE_ENABLE) };
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

/// The image's virtual address minus its physical address, the direct map's offset too.
pub(crate) const KERNEL_VIRTUAL_OFFSET: u64 = DIRECT_MAP_OFFSET as u64;
const BOOT_DIRECT_MAP_END: u64 = RAM_START as u64 + ((BOOT_DIRECT_MEGAPAGES as u64) << 22);

/// Zeroes the `length` bytes at `start` ([`crate::memory::zero`]).
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
    // SAFETY: a barrier: it orders stores and touches nothing.
    unsafe { core::arch::asm!("fence w, o", options(nostack, preserves_flags)) };
}

fn sbi_call(extension: usize, function: usize, arguments: [usize; 3]) -> (isize, usize) {
    let error: isize;
    let value: usize;
    // SAFETY: an SBI call; the firmware preserves every register but a0 and a1.
    unsafe {
        core::arch::asm!(
            "ecall",
            inlateout("a0") arguments[0] => error,
            inlateout("a1") arguments[1] => value,
            in("a2") arguments[2],
            in("a6") function,
            in("a7") extension,
        );
    }
    (error, value)
}

fn device_tree_address() -> usize {
    DEVICE_TREE.load(Ordering::Relaxed)
}

/// RV32 on QEMU `virt` (D481).
#[derive(Debug)]
pub struct Riscv32;

impl Platform for Riscv32 {
    const ARCH: &'static str = "riscv32";

    type PageTableFormat = paging::riscv32::Riscv32Sv32;

    fn console_write(bytes: &[u8]) {
        console_write(bytes);
    }

    fn console_read(buffer: &mut [u8]) -> usize {
        uart().read_bytes(buffer)
    }

    fn enable_console_input() -> Result<(), &'static str> {
        Err("RV32 has no console input interrupt yet")
    }

    fn exit(_status: ExitStatus) -> ! {
        shut_down()
    }

    fn restart() -> ! {
        sbi(SBI_RESET, 0, 1, NO_REASON);
        Self::halt()
    }

    fn halt() -> ! {
        loop {
            // SAFETY: pauses the hart until an interrupt is pending.
            unsafe { core::arch::asm!("wfi") };
        }
    }

    fn breakpoint() {
        // SAFETY: traps to the vector installed at boot, which resumes after it.
        unsafe { core::arch::asm!("ebreak") };
    }

    fn execute_illegal_instruction() -> ! {
        // SAFETY: `unimp` traps; the kernel never resumes from it.
        unsafe { core::arch::asm!("unimp", options(noreturn)) }
    }

    fn clock_ticks() -> u64 {
        clock_ticks()
    }

    fn clock_frequency() -> u64 {
        TIMEBASE_HZ
    }

    fn set_timer_deadline(deadline: u64) {
        set_timer_deadline(deadline);
    }

    fn wait_for_interrupt() {
        // SAFETY: as on RV64: `wfi` wakes on an enabled pending interrupt, which setting SIE
        // takes through the trap vector; clearing SIE masks interrupts again.
        unsafe { core::arch::asm!("wfi", "csrsi sstatus, 2", "csrci sstatus, 2") };
    }

    fn interrupt_window() {
        // SAFETY: as in `wait_for_interrupt`, without the `wfi`.
        unsafe { core::arch::asm!("csrsi sstatus, 2", "csrci sstatus, 2") };
    }

    fn memory_map() -> MemoryMap {
        // SAFETY: OpenSBI passes its device tree's address in `a1`; it stays readable and
        // unwritten, in RAM the boot direct map covers.
        let mut map = unsafe { boot_memory::device_tree_memory_map(device_tree_address()) };
        map.retain_within(PhysicalRange {
            start: 0,
            end: BOOT_DIRECT_MAP_END,
        });
        map
    }

    fn physical_memory_offset() -> usize {
        DIRECT_MAP_OFFSET as usize
    }

    fn direct_map_offset() -> usize {
        DIRECT_MAP_OFFSET as usize
    }

    fn kernel_image() -> KernelImage {
        boot_memory::kernel_image_parts()
    }

    fn physical_memory() -> MemoryMap {
        // SAFETY: as in `memory_map`.
        let mut map = unsafe { boot_memory::device_tree_ram(device_tree_address()) };
        map.retain_within(PhysicalRange {
            start: 0,
            end: BOOT_DIRECT_MAP_END,
        });
        map
    }

    fn early_device_regions(region: &mut dyn FnMut(PhysicalRange)) {
        region(PhysicalRange::from_start_length(u64::from(UART), UART_BYTES as u64));
    }

    fn device_tree() -> Option<&'static [u8]> {
        // SAFETY: as in `memory_map`.
        unsafe { boot_memory::device_tree_blob(device_tree_address()) }
    }

    fn device_window() -> Option<core::ops::Range<u64>> {
        Some(DEVICE_WINDOW)
    }

    fn make_kernel_mappings_visible(_start: usize, _length: usize) {
        // SAFETY: orders the table writes before later walks and drops cached translations.
        unsafe { core::arch::asm!("fence rw, rw", "sfence.vma") };
    }

    unsafe fn activate_page_tables(roots: &[u64]) {
        let [root] = roots else {
            panic!("Sv32 page tables have one root");
        };
        let satp = SATP_MODE_SV32 as usize | (root >> 12) as usize;
        // SAFETY: the caller guarantees the tables map the kernel and the direct map at the
        // addresses in use; the fences order the writes and drop the boot table's entries.
        unsafe { core::arch::asm!("sfence.vma", "csrw satp, {}", "sfence.vma", in(reg) satp) };
    }

    unsafe fn switch_address_space(roots: &[u64]) {
        // SAFETY: as on RV64: a full fence, the same switch, then `fence.i` for code other
        // harts wrote.
        unsafe {
            core::arch::asm!("fence rw, rw");
            Self::activate_page_tables(roots);
            core::arch::asm!("fence.i");
        }
    }

    fn with_user_access<R>(access: impl FnOnce() -> R) -> R {
        let before: usize;
        // SAFETY: `sstatus.SUM` only lets S-mode reach U-mode pages, put back below.
        unsafe {
            core::arch::asm!("csrrs {}, sstatus, {}", out(reg) before, in(reg) USER_MEMORY_ACCESS)
        };
        let result = access();
        if before & USER_MEMORY_ACCESS == 0 {
            // SAFETY: closes the window this call opened.
            unsafe { core::arch::asm!("csrc sstatus, {}", in(reg) USER_MEMORY_ACCESS) };
        }
        result
    }

    unsafe fn run_user(context: &mut crate::user::UserContext) -> crate::user::UserExit {
        // SAFETY: the caller's guarantees are `run_user`'s.
        unsafe { user::run_user(context) }
    }

    fn user_test_program() -> crate::user::TestProgram {
        test_program::user_test_program()
    }

    fn linux_hardware_capabilities() -> [u64; 2] {
        let letter = |extension: u8| 1u64 << (extension - b'A');
        [
            letter(b'I') | letter(b'M') | letter(b'A') | letter(b'F') | letter(b'D') | letter(b'C'),
            0,
        ]
    }

    fn random_seeds(seed: &mut dyn FnMut(crate::random::SeedSource, &[u8])) {
        // SAFETY: as in `memory_map`, once, on the boot hart.
        unsafe { boot_memory::device_tree_take_seeds(device_tree_address(), seed) };
    }

    fn make_instructions_visible(_start: usize, _length: usize) {
        // SAFETY: orders this hart's fetches after its stores.
        unsafe { core::arch::asm!("fence.i") };
    }

    fn drop_user_translations(start: u64, length: u64) {
        let pages = length / crate::memory::FRAME_SIZE;
        if pages > crate::paging::USER_PAGES_DROPPED_ONE_BY_ONE {
            // SAFETY: drops every cached translation.
            unsafe { core::arch::asm!("sfence.vma zero, zero") };
            return;
        }
        for page in 0..pages {
            let address = (start + page * crate::memory::FRAME_SIZE) as usize;
            // SAFETY: drops one page's cached translations.
            unsafe { core::arch::asm!("sfence.vma {}, zero", in(reg) address) };
        }
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

    unsafe fn prepare_thread(
        stack_top: usize,
        entry: extern "C" fn(usize) -> !,
        argument: usize,
    ) -> usize {
        // SAFETY: the frame fits in `INITIAL_FRAME_MAX`, writable and unused below the top.
        unsafe {
            crate::thread::write_initial_frame(stack_top, &thread::initial_frame(entry, argument))
        }
    }

    unsafe fn switch_thread(save: *mut usize, resume: usize) {
        // SAFETY: the caller's contract is the switch's.
        unsafe { thread::eggos_switch_thread(save, resume) }
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
        unsafe { boot_memory::device_tree_cpus(device_tree_address(), cpu) };
    }

    fn boot_cpu() -> u64 {
        BOOT_HART.load(Ordering::Relaxed) as u64
    }

    unsafe fn start_cpu(_cpu: u64, _start: &CpuStart) -> Result<(), StartCpuError> {
        Err(StartCpuError::Refused(SECONDARY_HARTS_NOT_YET))
    }

    unsafe fn set_cpu_local(block: usize) {
        // SAFETY: the kernel keeps no thread-local storage, so `tp` is free for the block.
        unsafe { core::arch::asm!("mv tp, {}", in(reg) block) };
    }

    fn cpu_local() -> usize {
        let block: usize;
        // SAFETY: reads `tp`.
        unsafe { core::arch::asm!("mv {}, tp", out(reg) block) };
        block
    }

    fn wake_cpu(cpu: u64) {
        // SAFETY: a barrier only.
        unsafe { core::arch::asm!("fence rw, rw") };
        sbi_call(SBI_IPI, 0, [1, cpu as usize, 0]);
    }
}
