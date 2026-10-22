//! RISC-V 64-bit (RV64GC).
//!
//! Boot contract: the SBI firmware (OpenSBI, as bundled with QEMU) enters `_start` in S-mode
//! with `a0` = hart id and `a1` = device-tree address, MMU off. The device tree describes the
//! RAM and the firmware's own reservation (see [`Platform::memory_map`]).
//!
//! Board addresses are QEMU `virt`'s for now; device-tree discovery replaces them in M5.

mod exception;
mod thread;
mod timer;

use crate::arch::boot_memory;
use crate::cpu::{CpuStart, StartCpuError};
use crate::memory::PhysicalRange;
use crate::paging::{self, Attributes, KernelImage, MemoryType};
use crate::{ExitStatus, MemoryMap, Platform};
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
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

    // Keep the device-tree address and this hart's id (after zeroing .bss, where they are
    // stored).
2:  lla     t0, {device_tree}
    sd      a1, 0(t0)
    lla     t0, {boot_hart}
    sd      a0, 0(t0)

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
    call    boot_translation_on
    lla     t0, 9f
    ld      t0, 0(t0)
    jr      t0

    .balign 8
8:  .dword  __kernel_start
9:  .dword  4f

    // From here on the kernel runs at its virtual address, and `lla` gives virtual addresses.
4:  lla     sp, __stack_top

    // FPU and vector unit off (FS and VS = Off; VS reads as zero without V): the kernel uses
    // neither and the trap entry saves neither, so any FP or vector instruction traps as
    // illegal, in U-mode too (D49). Then the trap vector, before any Rust code runs, and
    // the timer.
    li      t0, (3 << 13) | (3 << 9)
    csrc    sstatus, t0
    lla     t0, exception_entry
    csrw    stvec, t0
    call    {init_timer}
    call    kernel_main
5:  wfi
    j       5b

    // Turns translation on with the boot page table, from code running at its physical
    // address: the identity gigabyte keeps the return address mapped. Clobbers t0 and t1.
boot_translation_on:
    lla     t0, {boot_root}
    srli    t0, t0, 12
    li      t1, {satp_mode}
    or      t0, t0, t1
    sfence.vma
    csrw    satp, t0
    sfence.vma
    ret

    // A secondary hart, started by SBI HSM `hart_start` (D40): S-mode, translation off,
    // interrupts masked, a0 = its hart id, a1 = the opaque value, the kernel virtual address
    // of the `CpuStart`. The boot page table is still in .bss and maps the `CpuStart` and its
    // stack (the kernel image and the direct map).
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

/// The direct map: the start of Sv39's high half (D29). It holds 254 GiB: the top 2 GiB are
/// the kernel's (D33).
const DIRECT_MAP_OFFSET: usize = 0xffff_ffc0_0000_0000;

/// The physical memory the boot direct map covers, and so the most the kernel reaches: 254 GiB.
const BOOT_DIRECT_MAP_END: u64 = 0xffff_ffff_8000_0000 - DIRECT_MAP_OFFSET as u64;

/// `sstatus.SUM`: S-mode may access U-mode pages (see `with_user_access`).
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
/// with the boot page table: switches to the kernel's page tables, sets this hart's timer and
/// interrupts up (D41), then calls the entry.
extern "C" fn secondary_main(start: &CpuStart) -> ! {
    // SAFETY: `start_cpu`'s caller guarantees the tables are the kernel's own, which map
    // everything this hart uses at the addresses it uses now, as they did for the boot hart;
    // interrupts are masked (HSM starts the hart with `sstatus.SIE` clear).
    unsafe { Riscv64::activate_page_tables(&start.page_table_roots[..1]) };
    timer::init();
    (start.entry)(start.argument)
}

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

    unsafe fn switch_address_space(roots: &[u64]) {
        // The same switch: `satp` holds one root for both halves, and the kernel's entries in
        // the new root are the active one's, so the kernel runs on at the same addresses.
        // SAFETY: the caller's guarantees include `activate_page_tables`'.
        unsafe { Self::activate_page_tables(roots) };
        // `fence.i` is this hart's alone: code another hart wrote for the new address space
        // (`make_instructions_visible` ran there) becomes visible to this one's fetch here.
        // SAFETY: as in `make_instructions_visible`: it changes no memory contents.
        unsafe { core::arch::asm!("fence.i", options(nostack)) };
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
        // One bit per single-letter extension, `1 << (letter - 'A')`: the base the kernel's
        // target requires (RV64IMAC), without F, D or V, which trap in user mode (D49).
        let letter = |extension: u8| 1u64 << (extension - b'A');
        [letter(b'I') | letter(b'M') | letter(b'A') | letter(b'C'), 0]
    }

    fn make_instructions_visible(_start: usize, _length: usize) {
        // SAFETY: `fence.i` orders this hart's instruction fetches after its earlier stores;
        // it changes no memory contents.
        unsafe { core::arch::asm!("fence.i", options(nostack)) };
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
