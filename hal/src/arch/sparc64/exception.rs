//! SPARC64 traps (D326): the trap table, the window spill, fill and clean handlers, the MMU's
//! software TLB loads, and the ways into and out of user mode.
//!
//! - **The table** (`%tba`, 32 KiB): 512 entries for traps taken at TL 0, then 512 for TL above
//!   0. Spill and fill handlers come in two kinds, picked by `%wstate`: 0 for the kernel's
//!   windows on the kernel stack, 1 for a program's on its own stack through
//!   `ASI_AS_IF_USER_PRIMARY`. Everything else goes to [`sparc64_trap_common`] on the alternate
//!   globals; at TL above 1 that is fatal.
//! - **TLB loads** run on the MMU globals, `%g6` the program's root and `%g7` the kernel's
//!   (physical). The direct map gets 4 MiB entries computed from the address; anything else is
//!   walked, and an aligned pair of 4 KiB leaves that are present, contiguous and alike becomes
//!   one 8 KiB entry, the MMU's smallest. Other misses are faults.
//! - **From the kernel** (only at the points where it opens interrupts, or a fault): the trap
//!   saves the state on a new kernel window, drops to TL 0, calls
//!   [`sparc64_kernel_exception`], then returns with `retry`, or `done` after a trap
//!   instruction.
//! - **User mode**: `sparc64_enter_user` flushes the kernel's windows, keeps its stack pointer
//!   in the context, puts the context's address in the alternate `%g6`, loads the program's
//!   registers and `retry`s into it. A trap from it flushes its windows to its stack, saves its
//!   registers into the context and returns from `sparc64_enter_user`.

use super::{fp, timer};
use crate::Interrupt;
use crate::exception::{Exception, ExceptionKind, deliver};
use crate::paging::{Access, PageFault};
use crate::user::{TestProgram, UserContext, UserExit};

pub(super) mod pstate {
    pub const ALTERNATE_GLOBALS: u64 = 1 << 0;
    pub const INTERRUPTS: u64 = 1 << 1;
    pub const PRIVILEGED: u64 = 1 << 2;
    pub const FP: u64 = 1 << 4;
    pub const MMU_GLOBALS: u64 = 1 << 10;
    /// The kernel: privileged, the FP unit on for the save routines, interrupts off.
    pub const KERNEL: u64 = PRIVILEGED | FP;
    /// A program: interrupts on, the FP unit on (`%fprs.FEF` decides).
    pub const USER: u64 = INTERRUPTS | FP;
}

/// `TSTATE.PSTATE.PRIV`: the trap came from the kernel.
const TSTATE_PRIVILEGED: u64 = pstate::PRIVILEGED << 8;
const TSTATE_PSTATE_SHIFT: u32 = 8;
/// The register windows of the UltraSPARC IIi QEMU's `sun4u` models.
pub(super) const WINDOWS: u64 = 8;
const STACK_BIAS: u64 = 2047;
const FRAME: u64 = 192;
const KERNEL_TRAP_FRAME: u64 = FRAME + 64;

/// Trap types.
mod trap {
    pub const INSTRUCTION_ACCESS_EXCEPTION: u64 = 0x008;
    pub const INSTRUCTION_ACCESS_ERROR: u64 = 0x00a;
    pub const ILLEGAL_INSTRUCTION: u64 = 0x010;
    pub const PRIVILEGED_OPCODE: u64 = 0x011;
    pub const FP_DISABLED: u64 = 0x020;
    pub const FP_EXCEPTION_IEEE: u64 = 0x021;
    pub const FP_EXCEPTION_OTHER: u64 = 0x022;
    pub const TAG_OVERFLOW: u64 = 0x023;
    pub const DIVISION_BY_ZERO: u64 = 0x028;
    pub const DATA_ACCESS_EXCEPTION: u64 = 0x030;
    pub const DATA_ACCESS_ERROR: u64 = 0x032;
    pub const ADDRESS_NOT_ALIGNED: u64 = 0x034;
    pub const LAST_NOT_ALIGNED: u64 = 0x039;
    pub const FIRST_INTERRUPT: u64 = 0x041;
    pub const LAST_INTERRUPT: u64 = 0x04f;
    pub const INSTRUCTION_MISS: u64 = 0x064;
    pub const DATA_MISS: u64 = 0x068;
    pub const DATA_PROTECTION: u64 = 0x06c;
    /// `ta 1`, the breakpoint.
    pub const BREAKPOINT: u64 = 0x101;
    /// `ta 0x6d`, Linux's 64-bit system call.
    pub const SYSCALL: u64 = 0x16d;
    pub const FIRST_TRAP_INSTRUCTION: u64 = 0x100;
}

core::arch::global_asm!(
    r#"
    .macro TO target
    ba,a        \target
    .balign 32
    .endm

    .macro SPILL_KERNEL
    .irp n, 0,1,2,3,4,5,6,7
    stx         %l\n, [%sp + {bias} + \n * 8]
    .endr
    .irp n, 0,1,2,3,4,5,6,7
    stx         %i\n, [%sp + {bias} + 64 + \n * 8]
    .endr
    saved
    retry
    .balign 128
    .endm

    .macro SPILL_USER
    wr          %g0, {asi_user}, %asi
    .irp n, 0,1,2,3,4,5,6,7
    stxa        %l\n, [%sp + {bias} + \n * 8] %asi
    .endr
    .irp n, 0,1,2,3,4,5,6,7
    stxa        %i\n, [%sp + {bias} + 64 + \n * 8] %asi
    .endr
    saved
    retry
    .balign 128
    .endm

    .macro FILL_KERNEL
    .irp n, 0,1,2,3,4,5,6,7
    ldx         [%sp + {bias} + \n * 8], %l\n
    .endr
    .irp n, 0,1,2,3,4,5,6,7
    ldx         [%sp + {bias} + 64 + \n * 8], %i\n
    .endr
    restored
    retry
    .balign 128
    .endm

    .macro FILL_USER
    wr          %g0, {asi_user}, %asi
    .irp n, 0,1,2,3,4,5,6,7
    ldxa        [%sp + {bias} + \n * 8] %asi, %l\n
    .endr
    .irp n, 0,1,2,3,4,5,6,7
    ldxa        [%sp + {bias} + 64 + \n * 8] %asi, %i\n
    .endr
    restored
    retry
    .balign 128
    .endm

    .macro CLEAN_WINDOW
    rdpr        %cleanwin, %l0
    add         %l0, 1, %l0
    wrpr        %l0, 0, %cleanwin
    .irp n, 0,1,2,3,4,5,6,7
    clr         %l\n
    clr         %o\n
    .endr
    retry
    .balign 128
    .endm

    .macro MISS target
    ba,a        \target
    .balign 128
    .endm

    .macro TRAPS other
    .rept 0x24
    TO \other
    .endr
    CLEAN_WINDOW
    .rept 0x64 - 0x28
    TO \other
    .endr
    MISS sparc64_itlb_miss
    MISS sparc64_dtlb_miss
    MISS sparc64_dtlb_protection
    .rept 0x80 - 0x70
    TO \other
    .endr
    SPILL_KERNEL
    SPILL_USER
    .rept 0xc0 - 0x88
    TO sparc64_fatal
    .endr
    FILL_KERNEL
    FILL_USER
    .rept 0x200 - 0xc8
    TO \other
    .endr
    .endm

    .section .text.traps, "ax"
    .balign 32768
    .global sparc64_trap_table
sparc64_trap_table:
    TRAPS sparc64_trap_common
    TRAPS sparc64_fatal

    .text
    .balign 4
sparc64_itlb_miss:
    mov         {tag_access}, %g1
    ldxa        [%g1] {asi_immu}, %g2
    ba          sparc64_tlb_walk
     mov        1, %g5
sparc64_dtlb_protection:
    mov         {tag_access}, %g1
    ldxa        [%g1] {asi_dmmu}, %g2
    srlx        %g2, 13, %g3
    sllx        %g3, 13, %g3
    stxa        %g0, [%g3] {asi_dmmu_demap}
    membar      #Sync
    ba          sparc64_tlb_walk
     mov        2, %g5
sparc64_dtlb_miss:
    mov         {tag_access}, %g1
    ldxa        [%g1] {asi_dmmu}, %g2
    mov         0, %g5
sparc64_tlb_walk:
    srlx        %g2, 13, %g3
    sllx        %g3, 13, %g3
    srax        %g3, 41, %g4
    cmp         %g4, -4
    bne         %xcc, 1f
     cmp        %g5, 1
    be          %xcc, sparc64_tlb_fault
     sllx       %g3, 23, %g4
    srlx        %g4, 45, %g4
    sllx        %g4, 22, %g4
    mov         -1, %g1
    sllx        %g1, 61, %g1
    or          %g4, %g1, %g4
    or          %g4, {direct_bits}, %g4
    stxa        %g4, [%g0] {asi_dtlb_in}
    retry
1:  srlx        %g3, 43, %g4
    brz         %g4, 2f
     mov        %g6, %g1
    srax        %g3, 43, %g4
    cmp         %g4, -1
    bne         %xcc, sparc64_tlb_fault
     mov        %g7, %g1
2:  brz         %g1, sparc64_tlb_fault
     nop
    .irp shift, 39, 30, 21
    srlx        %g3, \shift, %g4
    and         %g4, 511, %g4
    sllx        %g4, 3, %g4
    ldxa        [%g1 + %g4] {asi_physical}, %g1
    brz         %g1, sparc64_tlb_fault
     nop
    .endr
    srlx        %g3, 12, %g4
    and         %g4, 510, %g4
    sllx        %g4, 3, %g4
    ldxa        [%g1 + %g4] {asi_physical}, %g2
    add         %g4, 8, %g4
    ldxa        [%g1 + %g4] {asi_physical}, %g4
    brgez       %g2, sparc64_tlb_fault
     nop
    brgez       %g4, sparc64_tlb_fault
     sub        %g4, %g2, %g4
    sethi       %hi(0x1000), %g1
    cmp         %g4, %g1
    bne         %xcc, sparc64_tlb_fault
     srlx       %g2, 12, %g4
    andcc       %g4, 1, %g0
    bne         %xcc, sparc64_tlb_fault
     srlx       %g2, 13, %g4
    sllx        %g4, 13, %g4
    and         %g2, 0x7f, %g1
    or          %g4, %g1, %g4
    cmp         %g5, 1
    bne         %xcc, 3f
     cmp        %g5, 2
    andcc       %g2, {executable}, %g0
    be          %xcc, sparc64_tlb_fault
     nop
    stxa        %g4, [%g0] {asi_itlb_in}
    retry
3:  bne         %xcc, 4f
     andcc      %g2, {writable}, %g0
    be          %xcc, sparc64_tlb_fault
     nop
4:  stxa        %g4, [%g0] {asi_dtlb_in}
    retry
sparc64_tlb_fault:
    wrpr        %g0, {pstate_kernel_alternate}, %pstate
    ba,a        sparc64_trap_common

sparc64_fatal:
    wrpr        %g0, {pstate_kernel_alternate}, %pstate
5:  ba          5b
     nop

sparc64_trap_common:
    rdpr        %tl, %g1
    cmp         %g1, 1
    bne         %xcc, sparc64_fatal
     rdpr       %tstate, %g1
    andcc       %g1, {tstate_privileged}, %g0
    bne         %xcc, sparc64_kernel_trap
     nop

    flushw
    .irp n, 0,1,2,3,4,5,6,7
    stx         %o\n, [%g6 + 64 + \n * 8]
    stx         %l\n, [%g6 + 128 + \n * 8]
    stx         %i\n, [%g6 + 192 + \n * 8]
    .endr
    rdpr        %tpc, %g2
    stx         %g2, [%g6 + 256]
    rdpr        %tnpc, %g2
    stx         %g2, [%g6 + 264]
    stx         %g1, [%g6 + 272]
    rd          %y, %g2
    stx         %g2, [%g6 + 280]
    rdpr        %tt, %g1
    stx         %g1, [%g6 + 296]
    call        sparc64_fault_registers
     nop
    stx         %g2, [%g6 + 304]
    stx         %g3, [%g6 + 312]
    mov         %g6, %l0
    wrpr        %g0, {pstate_kernel}, %pstate
    .irp n, 1,2,3,4,5,6,7
    stx         %g\n, [%l0 + \n * 8]
    .endr
    wrpr        %g0, 0, %wstate
    wrpr        %g0, {windows} - 1, %cleanwin
    ldx         [%l0 + 288], %sp
    add         %sp, {frame}, %fp
    ldx         [%sp + {bias} + 176], %i7
    wrpr        %g0, 0, %tl
    ret
     restore

    // From the kernel, %g1 TSTATE.
sparc64_kernel_trap:
    save        %sp, -{kernel_trap_frame}, %sp
    rdpr        %tstate, %l0
    rdpr        %tpc, %l1
    rdpr        %tnpc, %l2
    rd          %y, %l3
    rdpr        %tt, %l4
    mov         %l4, %g1
    call        sparc64_fault_registers
     nop
    mov         %g2, %l5
    mov         %g3, %l6
    wrpr        %g0, {pstate_kernel}, %pstate
    .irp n, 1,2,3,4,5,6,7
    stx         %g\n, [%sp + {bias} + {frame} + (\n - 1) * 8]
    .endr
    wrpr        %g0, 0, %tl
    mov         %l4, %o0
    mov         %l1, %o1
    mov         %l5, %o2
    call        {kernel_exception}
     mov        %l6, %o3
    .irp n, 1,2,3,4,5,6,7
    ldx         [%sp + {bias} + {frame} + (\n - 1) * 8], %g\n
    .endr
    wrpr        %g0, 1, %tl
    wrpr        %l0, 0, %tstate
    wrpr        %l1, 0, %tpc
    wrpr        %l2, 0, %tnpc
    wr          %l3, 0, %y
    sub         %l4, {first_trap_instruction}, %l5
    cmp         %l5, 0x80
    blu         %xcc, 6f
     nop
    restore
    retry
6:  restore
    done

    // %g1 the trap type: %g2 the fault address (the tag access of a TLB miss, else the data
    // fault address) and %g3 the data fault status. Clobbers %g4.
sparc64_fault_registers:
    mov         {tag_access}, %g4
    sub         %g1, {instruction_miss}, %g2
    cmp         %g2, 4
    blu,a       %xcc, 7f
     ldxa       [%g4] {asi_immu}, %g2
    sub         %g1, {data_miss}, %g2
    cmp         %g2, 8
    blu,a       %xcc, 7f
     ldxa       [%g4] {asi_dmmu}, %g2
    mov         {fault_address}, %g4
    ldxa        [%g4] {asi_dmmu}, %g2
7:  mov         {fault_status}, %g4
    retl
     ldxa       [%g4] {asi_dmmu}, %g3

    // %o0 the context, %o1 the program's %fprs.
    .global sparc64_enter_user
sparc64_enter_user:
    save        %sp, -{frame}, %sp
    flushw
    stx         %i7, [%sp + {bias} + 176]
    stx         %sp, [%i0 + 288]
    wr          %i1, 0, %fprs
    .irp n, 1,2,3,4,5,6,7
    ldx         [%i0 + \n * 8], %g\n
    .endr
    wrpr        %g0, 0, %canrestore
    wrpr        %g0, 0, %otherwin
    wrpr        %g0, {windows} - 2, %cansave
    wrpr        %g0, 0, %cleanwin
    wrpr        %g0, 1, %wstate
    wrpr        %g0, 1, %tl
    ldx         [%i0 + 256], %l0
    wrpr        %l0, 0, %tpc
    ldx         [%i0 + 264], %l0
    wrpr        %l0, 0, %tnpc
    ldx         [%i0 + 280], %l0
    wr          %l0, 0, %y
    ldx         [%i0 + 272], %l0
    srlx        %l0, 24, %l0
    sllx        %l0, 24, %l0
    rdpr        %cwp, %l1
    or          %l0, %l1, %l0
    or          %l0, {tstate_user}, %l0
    wrpr        %l0, 0, %tstate
    wrpr        %g0, {pstate_kernel_alternate}, %pstate
    mov         %i0, %g6
    .irp n, 0,1,2,3,4,5,6,7
    ldx         [%g6 + 64 + \n * 8], %o\n
    ldx         [%g6 + 128 + \n * 8], %l\n
    ldx         [%g6 + 192 + \n * 8], %i\n
    .endr
    retry

    // %o0 the program's root, %o1 the kernel's (physical; 0 to keep it): the MMU globals.
    .global sparc64_set_roots
sparc64_set_roots:
    rdpr        %pstate, %o3
    wrpr        %o3, {mmu_globals}, %pstate
    mov         %o0, %g6
    brnz,a      %o1, 8f
     mov        %o1, %g7
8:  retl
     wrpr       %o3, 0, %pstate
"#,
    bias = const STACK_BIAS,
    frame = const FRAME,
    kernel_trap_frame = const KERNEL_TRAP_FRAME,
    windows = const WINDOWS,
    asi_user = const 0x10,
    asi_physical = const 0x14,
    asi_immu = const 0x50,
    asi_itlb_in = const 0x54,
    asi_dmmu = const 0x58,
    asi_dtlb_in = const 0x5c,
    asi_dmmu_demap = const 0x5f,
    tag_access = const 0x30,
    fault_status = const 0x18,
    fault_address = const 0x20,
    direct_bits = const 0x37,
    executable = const crate::paging::sparc64::EXECUTABLE,
    writable = const crate::paging::sparc64::WRITABLE,
    pstate_kernel = const pstate::KERNEL,
    pstate_kernel_alternate = const pstate::KERNEL | pstate::ALTERNATE_GLOBALS,
    mmu_globals = const pstate::MMU_GLOBALS,
    tstate_privileged = const TSTATE_PRIVILEGED,
    tstate_user = const pstate::USER << TSTATE_PSTATE_SHIFT,
    first_trap_instruction = const trap::FIRST_TRAP_INSTRUCTION,
    instruction_miss = const trap::INSTRUCTION_MISS,
    data_miss = const trap::DATA_MISS,
    kernel_exception = sym sparc64_kernel_exception,
);

unsafe extern "C" {
    fn sparc64_enter_user(context: *mut crate::user::sparc64::Registers, fprs: u64);
    fn sparc64_set_roots(user: u64, kernel: u64);
    static sparc64_trap_table: u8;
}

/// The trap table's address, for `%tba`.
pub(super) fn trap_table() -> u64 {
    (&raw const sparc64_trap_table) as u64
}

/// Sets the roots the TLB loads walk: the program's, and the kernel's unless `None`.
pub(super) fn set_roots(user: u64, kernel: Option<u64>) {
    // SAFETY: only the MMU globals change, which only the TLB loads read; interrupts are off
    // while the routine has them selected.
    unsafe { sparc64_set_roots(user, kernel.unwrap_or(0)) };
}

/// Drops every unlocked translation of this CPU's TLBs.
pub(super) fn flush_tlb() {
    const DEMAP_ALL_PRIMARY: u64 = 0x80;
    const DEMAP_ALL_NUCLEUS: u64 = 0xa0;
    // SAFETY: demap operations only drop TLB entries; locked ones (the image) stay.
    unsafe {
        core::arch::asm!(
            "stxa %g0, [{primary}] 0x57",
            "stxa %g0, [{primary}] 0x5f",
            "stxa %g0, [{nucleus}] 0x57",
            "stxa %g0, [{nucleus}] 0x5f",
            "membar #Sync",
            primary = in(reg) DEMAP_ALL_PRIMARY,
            nucleus = in(reg) DEMAP_ALL_NUCLEUS,
            options(nostack),
        )
    };
}

/// Drops the translations of the pages from `start` for `length` bytes in the primary context.
pub(super) fn drop_translations(start: u64, length: u64) {
    const LARGEST_RANGE: u64 = 64 << 13;
    if length > LARGEST_RANGE {
        flush_tlb();
        return;
    }
    let mut page = start & !0x1fff;
    while page < start + length {
        // SAFETY: a demap-page operation in the primary context only drops TLB entries.
        unsafe {
            core::arch::asm!(
                "stxa %g0, [{0}] 0x57",
                "stxa %g0, [{0}] 0x5f",
                in(reg) page,
                options(nostack),
            )
        };
        page += 0x2000;
    }
    // SAFETY: completes the demaps.
    unsafe { core::arch::asm!("membar #Sync", options(nostack)) };
}

/// The context number the next address space gets: 1 to 8191, handed out round, the TLB
/// flushed when they wrap so a new one never meets an old one's translations.
static NEXT_CONTEXT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);
const LAST_CONTEXT: u64 = 8191;

/// Gives the address space just switched to a fresh context, in the primary and secondary
/// context registers.
pub(super) fn next_address_space() {
    use core::sync::atomic::Ordering;
    let mut context = NEXT_CONTEXT.fetch_add(1, Ordering::Relaxed);
    if context > LAST_CONTEXT {
        flush_tlb();
        NEXT_CONTEXT.store(2, Ordering::Relaxed);
        context = 1;
    }
    // SAFETY: the context registers only decide which translations programs' addresses use;
    // the kernel's are global.
    unsafe {
        core::arch::asm!(
            "stxa {context}, [{primary}] 0x58",
            "stxa {context}, [{secondary}] 0x58",
            "membar #Sync",
            context = in(reg) context,
            primary = in(reg) 0x08u64,
            secondary = in(reg) 0x10u64,
            options(nostack),
        )
    };
}

/// Where traps from the kernel land, at TL 0 with interrupts off: `trap_type`, the trapping
/// `pc`, and the fault registers.
extern "C" fn sparc64_kernel_exception(trap_type: u64, pc: u64, address: u64, status: u64) {
    if interrupt(trap_type) {
        return;
    }
    deliver(&decode(trap_type, address, status, pc));
}

/// Handles interrupt trap `trap_type`, if it is one; whether it was.
fn interrupt(trap_type: u64) -> bool {
    if !(trap::FIRST_INTERRUPT..=trap::LAST_INTERRUPT).contains(&trap_type) {
        return false;
    }
    let level = trap_type - 0x40;
    if level == timer::TICK_LEVEL {
        timer::end_timer_interrupt();
        crate::interrupt::deliver(Interrupt::Timer);
    } else {
        // SAFETY: `%clear_softint` clears only the level's soft interrupt bit.
        unsafe { core::arch::asm!("wr {0}, 0, %asr21", in(reg) 1u64 << level, options(nomem, nostack)) };
    }
    true
}

/// [`crate::Platform::run_user`]: the caller guarantees interrupts are off and the roots are
/// the process's.
pub(super) unsafe fn run_user(context: &mut UserContext) -> UserExit {
    let registers = &mut context.arch;
    // SAFETY: interrupts are off (the caller's duty), so nothing but TLB loads, which use only
    // the MMU globals, arrives before the `retry`; the trap that ends the program saves its
    // registers into the context, which `&mut` keeps valid, and returns here on the kernel
    // stack with the kernel's windows filled back from it. The program runs unprivileged,
    // where the kernel's pages, all privileged, are out of reach.
    unsafe { sparc64_enter_user(registers, fp::user_fprs()) };
    match registers.trap_type {
        trap::SYSCALL => {
            registers.syscall_o0 = registers.r[8];
            registers.pc = registers.npc;
            registers.npc += 4;
            UserExit::Syscall
        }
        trap_type if interrupt(trap_type) => UserExit::Interrupt,
        trap_type => UserExit::Exception(decode(
            trap_type,
            registers.fault_address,
            registers.fault_status,
            registers.pc,
        )),
    }
}

impl crate::user::sparc64::Registers {
    /// [`UserContext::last_exception`]: the exception `run_user` returned, from the trap type
    /// and fault registers it kept.
    pub(crate) fn last_exception(&self) -> Exception {
        decode(self.trap_type, self.fault_address, self.fault_status, self.pc)
    }
}

/// [`crate::Platform::page_fault`]: the TLB misses, which a stale entry may also raise (the
/// address is the 8 KiB page's), and the protection fault.
pub(super) fn page_fault(_kind: ExceptionKind, trap_type: u64) -> Option<PageFault> {
    let (access, may_be_stale) = match trap_type & 0x1ff {
        trap::INSTRUCTION_MISS => (Access::Execute, true),
        trap::DATA_MISS => (Access::Read, true),
        trap::DATA_PROTECTION => (Access::Write, true),
        trap::DATA_ACCESS_EXCEPTION => (Access::Write, false),
        _ => return None,
    };
    Some(PageFault {
        access,
        may_be_stale,
    })
}

fn decode(trap_type: u64, address: u64, status: u64, pc: u64) -> Exception {
    let page = address & !0x1fff;
    let (kind, name, fault_address) = match trap_type {
        trap::INSTRUCTION_ACCESS_EXCEPTION => (ExceptionKind::InstructionFault, "instruction access", Some(pc)),
        trap::INSTRUCTION_ACCESS_ERROR | trap::DATA_ACCESS_ERROR => (ExceptionKind::Other, "access error", None),
        trap::ILLEGAL_INSTRUCTION | trap::PRIVILEGED_OPCODE | trap::FP_DISABLED => {
            (ExceptionKind::IllegalInstruction, "illegal instruction", None)
        }
        trap::FP_EXCEPTION_IEEE | trap::FP_EXCEPTION_OTHER | trap::DIVISION_BY_ZERO | trap::TAG_OVERFLOW => {
            (ExceptionKind::Arithmetic, "arithmetic", None)
        }
        trap::DATA_ACCESS_EXCEPTION => (ExceptionKind::DataFault, "data access", Some(address)),
        trap::ADDRESS_NOT_ALIGNED..=trap::LAST_NOT_ALIGNED => (ExceptionKind::Alignment, "address not aligned", Some(address)),
        trap::FIRST_INTERRUPT..=trap::LAST_INTERRUPT => (ExceptionKind::Interrupt, "interrupt", None),
        trap::INSTRUCTION_MISS => (ExceptionKind::InstructionFault, "TLB miss (fetch)", Some(page)),
        trap::DATA_MISS => (ExceptionKind::DataFault, "TLB miss (data)", Some(page)),
        trap::DATA_PROTECTION => (ExceptionKind::DataFault, "TLB protection", Some(page)),
        trap::BREAKPOINT => (ExceptionKind::Breakpoint, "breakpoint (ta 1)", None),
        trap::SYSCALL => (ExceptionKind::Other, "syscall", None),
        _ => (ExceptionKind::Other, "trap", None),
    };
    let _ = status;
    Exception {
        kind,
        name,
        code: trap_type,
        pc,
        address: fault_address,
    }
}

core::arch::global_asm!(
    r#"
    .pushsection .rodata.user_test, "a"
    .balign 8
    .global sparc64_user_test_start
sparc64_user_test_start:
    .global sparc64_user_test_floating_point
sparc64_user_test_floating_point:
    fmovd       %f0, %f2
    .global sparc64_user_test_null_syscalls
sparc64_user_test_null_syscalls:
    mov         {syscall_null}, %g1
1:  ta          0x6d
    ba          1b
     nop
    .global sparc64_user_test_spin
sparc64_user_test_spin:
2:  ba          2b
     nop
    .global sparc64_user_test_illegal_instruction
sparc64_user_test_illegal_instruction:
    illtrap     0
    .global sparc64_user_test_linux_hello
sparc64_user_test_linux_hello:
    mov         1, %o0
3:  rd          %pc, %o1
    add         %o1, 9f - 3b, %o1
    mov         {hello_length}, %o2
    mov         {linux_write}, %g1
    ta          0x6d
    movcs       %xcc, -1, %o0
    sub         %o0, {hello_length}, %l0
    mov         1, %o0
    mov         0, %o1
    mov         1, %o2
    mov         {linux_write}, %g1
    ta          0x6d
    movcc       %xcc, 0, %o0
    sub         %o0, {efault}, %l1
    or          %l0, %l1, %l0
    mov         {unknown_syscall}, %g1
    ta          0x6d
    movcc       %xcc, 0, %o0
    sub         %o0, {enosys}, %l1
    or          %l0, %l1, %l0
    movrnz      %l0, 1, %l0
    mov         %l0, %o0
    mov         {linux_exit_group}, %g1
    ta          0x6d
4:  ba          4b
     nop
9:  .ascii      "EGGOS-USER: hello from user mode\n"
    .balign 4
    .global sparc64_user_test_linux_start
sparc64_user_test_linux_start:
    .global sparc64_user_test_linux_pending
sparc64_user_test_linux_pending:
    mov         1, %o0
    mov         {linux_exit_group}, %g1
    ta          0x6d
5:  ba          5b
     nop
    .global sparc64_user_test_linux_start_end
sparc64_user_test_linux_start_end:
    .global sparc64_user_test_linux_unknown_syscalls
sparc64_user_test_linux_unknown_syscalls:
    mov         {unknown_syscall}, %g1
6:  ta          0x6d
    ba          6b
     nop
    .global sparc64_user_test_stack_growth
sparc64_user_test_stack_growth:
    add         %sp, {bias}, %l0
    srlx        %l0, 12, %l0
    sllx        %l0, 12, %l0
    sethi       %hi(4096), %l1
7:  stx         %l0, [%l0]
    ba          7b
     sub        %l0, %l1, %l0
    .global sparc64_user_test_end
sparc64_user_test_end:
    .popsection
"#,
    syscall_null = const TestProgram::SYSCALL_NULL,
    hello_length = const TestProgram::LINUX_HELLO.len(),
    linux_write = const 4,
    linux_exit_group = const 188,
    efault = const 14,
    enosys = const 90,
    unknown_syscall = const TestProgram::LINUX_UNKNOWN_SYSCALL,
    bias = const STACK_BIAS,
);

unsafe extern "C" {
    static sparc64_user_test_start: u8;
    static sparc64_user_test_floating_point: u8;
    static sparc64_user_test_null_syscalls: u8;
    static sparc64_user_test_spin: u8;
    static sparc64_user_test_illegal_instruction: u8;
    static sparc64_user_test_linux_hello: u8;
    static sparc64_user_test_linux_start: u8;
    static sparc64_user_test_linux_start_end: u8;
    static sparc64_user_test_linux_unknown_syscalls: u8;
    static sparc64_user_test_stack_growth: u8;
    static sparc64_user_test_linux_pending: u8;
    static sparc64_user_test_end: u8;
}

/// [`crate::Platform::user_test_program`]. The programs past `linux_hello`, and `checks` and
/// `fp_registers`, are not written for SPARC64 yet: they exit with 1 (D326).
pub(super) fn user_test_program() -> TestProgram {
    let start = (&raw const sparc64_user_test_start) as usize;
    let end = (&raw const sparc64_user_test_end) as usize;
    let at = |symbol: *const u8| symbol as usize - start;
    // SAFETY: the symbols bound the programs' bytes in `.rodata.user_test`, part of the kernel
    // image, read-only and never written, for as long as the kernel runs.
    let code = unsafe { core::slice::from_raw_parts(start as *const u8, end - start) };
    let pending = at(&raw const sparc64_user_test_linux_pending);
    TestProgram {
        code,
        checks: pending,
        floating_point: at(&raw const sparc64_user_test_floating_point),
        null_syscalls: at(&raw const sparc64_user_test_null_syscalls),
        spin: at(&raw const sparc64_user_test_spin),
        fp_registers: pending,
        illegal_instruction: at(&raw const sparc64_user_test_illegal_instruction),
        linux_hello: at(&raw const sparc64_user_test_linux_hello),
        linux_start: at(&raw const sparc64_user_test_linux_start),
        linux_start_end: at(&raw const sparc64_user_test_linux_start_end),
        linux_unknown_syscalls: at(&raw const sparc64_user_test_linux_unknown_syscalls),
        stack_growth: at(&raw const sparc64_user_test_stack_growth),
        linux_thread_pointer: pending,
        linux_poll: pending,
        linux_signals: pending,
        linux_signal_handler: pending,
        linux_fault_handler: pending,
        linux_signal_queue: pending,
        linux_signal_suspend: pending,
        linux_clock: pending,
        linux_stat: pending,
        linux_credentials: pending,
        linux_move_and_discard: pending,
    }
}
