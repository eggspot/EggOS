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
static NEXT_CONTEXT: crate::atomic::AtomicU64 = crate::atomic::AtomicU64::new(1);
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
    .macro SYS number
    mov         \number, %g1
    ta          0x6d
    .endm
    .macro OK
    bcs,pn      %xcc, 9f
     nop
    brnz,pn     %o0, 9f
     nop
    .endm
    .macro FAILED_WITH errno
    bcc,pn      %xcc, 9f
     nop
    cmp         %o0, \errno
    bne,pn      %xcc, 9f
     nop
    .endm
    .macro SET64 value, register
    sethi       %hh(\value), %l7
    or          %l7, %hm(\value), %l7
    sllx        %l7, 32, %l7
    sethi       %hi(\value), \register
    or          \register, %lo(\value), \register
    or          \register, %l7, \register
    .endm
    .macro EXPECT register, value
    SET64       \value, %g1
    cmp         \register, %g1
    bne,pn      %xcc, 9f
     nop
    .endm
    .macro EXIT_OK_OR_FAIL
    mov         0, %o0
    SYS         188
9:  mov         1, %o0
    SYS         188
8:  ba          8b
     nop
    .endm

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
    add         %sp, {bias} + 128, %l5
    and         %l5, 15, %l1
    brnz,pn     %l1, 9f
     nop
    add         %sp, -336, %sp
    add         %sp, {bias} + 176, %l6
    SYS         20
    mov         %o0, %l0
    SYS         143
    cmp         %o0, %l0
    bne,pn      %xcc, 9f
     nop
    mov         %l6, %o0
    mov         16, %o1
    mov         0, %o2
    SYS         347
    bcs,pn      %xcc, 9f
     nop
    cmp         %o0, 16
    bne,pn      %xcc, 9f
     nop
    ldx         [%l6], %l1
    ldx         [%l6 + 8], %l2
    or          %l1, %l2, %l1
    brz,pn      %l1, 9f
     nop
    mov         0, %o0
    mov         3, %o1
    mov         0, %o2
    add         %l6, 16, %o3
    SYS         331
    OK
    ldx         [%l6 + 16], %l1
    EXPECT      %l1, 0x800000
    ldx         [%l6 + 24], %l1
    EXPECT      %l1, -1
    mov         1, %l1
    stx         %l1, [%l6 + 32]
    stx         %g0, [%l6 + 40]
    stx         %g0, [%l6 + 48]
    stx         %g0, [%l6 + 56]
    mov         13, %o0
    add         %l6, 32, %o1
    mov         0, %o2
    mov         0, %o3
    mov         8, %o4
    SYS         102
    OK
    mov         13, %o0
    mov         0, %o1
    add         %l6, 64, %o2
    mov         0, %o3
    mov         8, %o4
    SYS         102
    OK
    ldx         [%l6 + 64], %l1
    EXPECT      %l1, 1
    mov         0, %o0
    add         %l6, 96, %o1
    SYS         28
    OK
    ldsw        [%l6 + 104], %l1
    EXPECT      %l1, 2
    ldx         [%l5], %l0
    add         %l5, 8, %l1
    mov         2, %l2
6:  ldx         [%l1], %o1
    add         %l1, 8, %l1
    brz,pn      %o1, 8f
     mov        0, %o2
7:  ldub        [%o1 + %o2], %l3
    brz,pn      %l3, 5f
     nop
    ba          7b
     add        %o2, 1, %o2
5:  stx         %o1, [%l6 + 128]
    stx         %o2, [%l6 + 136]
    mov         1, %o0
    add         %l6, 128, %o1
    mov         1, %o2
    SYS         121
    ba          6b
     nop
8:  subcc       %l2, 1, %l2
    bne,pn      %xcc, 6b
     nop
    mov         %l0, %o0
    SYS         188
9:  mov         255, %o0
    SYS         188
4:  ba          4b
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

    .global sparc64_user_test_checks
sparc64_user_test_checks:
    .irp        n, 2,3,4,5,6,7
    mov         0x100 + \n, %g\n
    .endr
    .irp        n, 0,1,2,3,4,5,6,7
    mov         0x110 + \n, %l\n
    .endr
    .irp        n, 0,1,2,3,4,5,7
    mov         0x120 + \n, %i\n
    .endr
    mov         0x137, %o7
    mov         1, %o0
    mov         2, %o1
    mov         3, %o2
    mov         4, %o3
    mov         5, %o4
    mov         6, %o5
    mov         {syscall_first}, %g1
    ta          0x6d
    sub         %o1, 2, %o1
    .irp        n, 2,3,4,5
    sub         %o\n, \n + 1, %o\n
    or          %o1, %o\n, %o1
    .endr
    sub         %o7, 0x137, %o7
    or          %o1, %o7, %o1
    .irp        n, 2,3,4,5,6,7
    sub         %g\n, 0x100 + \n, %g\n
    or          %o1, %g\n, %o1
    .endr
    .irp        n, 0,1,2,3,4,5,6,7
    sub         %l\n, 0x110 + \n, %l\n
    or          %o1, %l\n, %o1
    .endr
    .irp        n, 0,1,2,3,4,5,7
    sub         %i\n, 0x120 + \n, %i\n
    or          %o1, %i\n, %o1
    .endr
    or          %o1, %g1, %o1
    mov         {syscall_report}, %g1
    ta          0x6d
    ldx         [%o0], %o2
1:  ba          1b
     nop

    .global sparc64_user_test_fp_registers
sparc64_user_test_fp_registers:
    add         %sp, -192, %sp
    add         %sp, {bias} + 176, %l6
    mov         {syscall_first}, %g1
    ta          0x6d
    mov         %o0, %l0
    mov         0, %l1
    .irp        i, 0,2,4,6,8,10,12,14,16,18,20,22,24,26,28,30,32,34,36,38,40,42,44,46,48,50,52,54,56,58,60,62
    std         %f\i, [%l6]
    ldx         [%l6], %l2
    or          %l1, %l2, %l1
    .endr
    stx         %fsr, [%l6]
    ldx         [%l6], %l2
    SET64       {fsr_version}, %l3
    andn        %l2, %l3, %l2
    or          %l1, %l2, %l1
    and         %l0, 3, %l2
    sllx        %l2, 30, %l2
    stx         %l2, [%l6]
    ldx         [%l6], %fsr
    .irp        i, 0,2,4,6,8,10,12,14,16,18,20,22,24,26,28,30,32,34,36,38,40,42,44,46,48,50,52,54,56,58,60,62
    add         %l0, \i, %l2
    stx         %l2, [%l6]
    ldd         [%l6], %f\i
    .endr
1:  mov         {syscall_null}, %g1
    ta          0x6d
    mov         %o0, %l3
    .irp        i, 0,2,4,6,8,10,12,14,16,18,20,22,24,26,28,30,32,34,36,38,40,42,44,46,48,50,52,54,56,58,60,62
    add         %l0, \i, %l2
    std         %f\i, [%l6]
    ldx         [%l6], %l4
    xor         %l4, %l2, %l4
    or          %l1, %l4, %l1
    .endr
    stx         %fsr, [%l6]
    ldx         [%l6], %l2
    srlx        %l2, 30, %l2
    and         %l2, 3, %l2
    and         %l0, 3, %l4
    xor         %l2, %l4, %l2
    or          %l1, %l2, %l1
    brz,pt      %l3, 1b
     nop
    mov         %l1, %o0
    mov         {syscall_report}, %g1
    ta          0x6d
2:  ba          2b
     nop

    .global sparc64_user_test_linux_thread_pointer
sparc64_user_test_linux_thread_pointer:
    mov         %g7, %l0
    SYS         20
    sllx        %o0, 12, %l1
    mov         %l1, %g7
    mov         {thread_pointer_rounds}, %l2
7:  SYS         245
    or          %l0, %o0, %l0
    xor         %g7, %l1, %l3
    or          %l0, %l3, %l0
    subcc       %l2, 1, %l2
    bne,pt      %xcc, 7b
     nop
    movrnz      %l0, 1, %l0
    mov         %l0, %o0
    SYS         188
8:  ba          8b
     nop

    .global sparc64_user_test_linux_poll
sparc64_user_test_linux_poll:
    add         %sp, -240, %sp
    add         %sp, {bias} + 176, %l6
    stx         %g0, [%l6]
    mov         1, %l1
    stw         %l1, [%l6 + 8]
    mov         4, %l1
    sth         %l1, [%l6 + 12]
    sth         %g0, [%l6 + 14]
    mov         2, %l1
    stw         %l1, [%l6 + 16]
    stw         %g0, [%l6 + 20]
    stx         %g0, [%l6 + 32]
    stx         %g0, [%l6 + 40]
    mov         %l6, %o0
    mov         3, %o1
    add         %l6, 32, %o2
    mov         0, %o3
    mov         0, %o4
    SYS         298
    bcs,pn      %xcc, 9f
     nop
    EXPECT      %o0, 1
    lduh        [%l6 + 6], %l1
    EXPECT      %l1, 0
    lduh        [%l6 + 14], %l1
    EXPECT      %l1, 4
    lduh        [%l6 + 22], %l1
    EXPECT      %l1, 0
    mov         1, %o0
    SET64       0x40087468, %o1
    add         %l6, 48, %o2
    SYS         54
    OK
    stx         %g0, [%l6 + 32]
    SET64       {poll_wait}, %l1
    stx         %l1, [%l6 + 40]
    mov         0, %o0
    mov         0, %o1
    add         %l6, 32, %o2
    mov         0, %o3
    mov         0, %o4
    SYS         298
    OK
    ldx         [%l6 + 32], %l1
    ldx         [%l6 + 40], %l2
    or          %l1, %l2, %l1
    EXPECT      %l1, 0
    EXIT_OK_OR_FAIL

    .global sparc64_user_test_linux_signals
sparc64_user_test_linux_signals:
    add         %sp, -240, %sp
    add         %sp, {bias} + 176, %l6
    mov         0, %o0
    mov         20, %o1
    SYS         37
    OK
    sethi       %hi(0x20000000), %l1
    stx         %l1, [%l6]
    mov         1, %o0
    mov         %l6, %o1
    mov         0, %o2
    mov         8, %o3
    SYS         103
    OK
    SYS         20
    mov         %o0, %o1
    mov         30, %o2
    SYS         211
    OK
    add         %l6, 16, %o0
    mov         8, %o1
    SYS         104
    OK
    ldx         [%l6 + 16], %l1
    EXPECT      %l1, 0x20000000
    mov         2, %o0
    mov         %l6, %o1
    mov         0, %o2
    mov         8, %o3
    SYS         103
9:  mov         1, %o0
    SYS         188
8:  ba          8b
     nop

    .global sparc64_user_test_linux_signal_handler
sparc64_user_test_linux_signal_handler:
    add         %sp, -304, %sp
    add         %sp, {bias} + 176, %l6
    sethi       %hi(0x2800), %l1
    sub         %l6, %l1, %l1
    stx         %l1, [%l6]
    stx         %g0, [%l6 + 8]
    sethi       %hi(0x2000), %l1
    stx         %l1, [%l6 + 16]
    mov         %l6, %o0
    mov         0, %o1
    SYS         28
    OK
1:  rd          %pc, %l1
    add         %l1, 5f - 1b, %l1
    stx         %l1, [%l6 + 32]
    mov         0x201, %l1
    stx         %l1, [%l6 + 40]
    stx         %g0, [%l6 + 48]
    stx         %g0, [%l6 + 56]
    mov         30, %o0
    add         %l6, 32, %o1
    mov         0, %o2
    mov         0, %o3
    mov         8, %o4
    SYS         102
    OK
    SYS         20
    mov         %o0, %o1
    mov         30, %o2
    mov         0x5a5, %g3
    mov         1, %g2
    SET64       {signal_fp_kept}, %l1
    stx         %l1, [%l6 + 64]
    ldd         [%l6 + 64], %f8
    SET64       {signal_rounding}<<30, %l1
    stx         %l1, [%l6 + 64]
    ldx         [%l6 + 64], %fsr
    SYS         211
    OK
    EXPECT      %g3, 0x5a5
    EXPECT      %g2, 2
    std         %f8, [%l6 + 64]
    ldx         [%l6 + 64], %l1
    EXPECT      %l1, {signal_fp_kept}
    std         %f10, [%l6 + 64]
    ldx         [%l6 + 64], %l1
    EXPECT      %l1, {signal_fp_changed}
    stx         %fsr, [%l6 + 64]
    ldx         [%l6 + 64], %l1
    srlx        %l1, 30, %l1
    and         %l1, 3, %l1
    EXPECT      %l1, {signal_rounding}
    mov         1, %o0
    mov         0, %o1
    add         %l6, 72, %o2
    mov         8, %o3
    SYS         103
    OK
    ldx         [%l6 + 72], %l1
    EXPECT      %l1, 0
    EXIT_OK_OR_FAIL
5:  save        %sp, -192, %sp
    cmp         %i0, 30
    bne,pn      %xcc, 7f
     nop
    ldsw        [%i1], %l0
    cmp         %l0, 30
    bne,pn      %xcc, 7f
     nop
    ldsw        [%i1 + 8], %l0
    cmp         %l0, -6
    bne,pn      %xcc, 7f
     nop
    add         %i6, {bias}, %l1
    ldx         [%i1 + {frame_stack_sp}], %l2
    cmp         %l1, %l2
    blu,pn      %xcc, 7f
     nop
    ldx         [%i1 + {frame_stack_size}], %l3
    add         %l2, %l3, %l2
    cmp         %l1, %l2
    bgeu,pn     %xcc, 7f
     nop
    SET64       {signal_fp_kept}, %l1
    std         %f8, [%sp + {bias} + 176]
    ldx         [%sp + {bias} + 176], %l0
    cmp         %l0, %l1
    bne,pn      %xcc, 7f
     nop
    ldx         [%i1 + {frame_fpu}], %l2
    ldx         [%l2 + 32], %l0
    cmp         %l0, %l1
    bne,pn      %xcc, 7f
     nop
    ldx         [%l2 + 256], %l0
    srlx        %l0, 30, %l0
    and         %l0, 3, %l0
    cmp         %l0, {signal_rounding}
    bne,pn      %xcc, 7f
     nop
    SET64       {signal_fp_changed}, %l0
    stx         %l0, [%l2 + 40]
    stx         %g0, [%sp + {bias} + 176]
    ldd         [%sp + {bias} + 176], %f8
    mov         2, %l0
    stx         %l0, [%i1 + {saved_g2}]
7:  mov         0, %g3
    ret
     restore

    .global sparc64_user_test_linux_fault_handler
sparc64_user_test_linux_fault_handler:
    add         %sp, -272, %sp
    add         %sp, {bias} + 176, %l6
1:  rd          %pc, %l1
    add         %l1, 5f - 1b, %l1
    stx         %l1, [%l6]
    mov         0x200, %l1
    stx         %l1, [%l6 + 8]
    stx         %g0, [%l6 + 16]
    stx         %g0, [%l6 + 24]
    mov         11, %o0
    mov         %l6, %o1
    mov         0, %o2
    mov         0, %o3
    mov         8, %o4
    SYS         102
    OK
    mov         0, %o0
    sethi       %hi(8192), %o1
    mov         0, %o2
    mov         0x22, %o3
    mov         -1, %o4
    mov         0, %o5
    SYS         71
    bcs,pn      %xcc, 9f
     nop
    mov         %o0, %l0
    mov         0, %g2
    SET64       {fault_address}, %g3
    mov         1, %g4
2:  rd          %pc, %g5
    add         %g5, 3f - 2b, %g5
    ldx         [%g3], %l1
    ba          9f
     nop
3:  mov         %l0, %g3
    mov         2, %g4
4:  rd          %pc, %g5
    add         %g5, 6f - 4b, %g5
    ldx         [%g3], %l1
    ba          9f
     nop
6:  EXPECT      %g2, 2
    EXIT_OK_OR_FAIL
5:  save        %sp, -192, %sp
    cmp         %i0, 11
    bne,pn      %xcc, 7f
     nop
    ldsw        [%i1], %l0
    cmp         %l0, 11
    bne,pn      %xcc, 7f
     nop
    ldsw        [%i1 + 8], %l0
    ldx         [%i1 + {saved_g4}], %l1
    cmp         %l0, %l1
    bne,pn      %xcc, 7f
     nop
    ldx         [%i1 + 16], %l0
    ldx         [%i1 + {saved_g3}], %l1
    cmp         %l0, %l1
    bne,pn      %xcc, 7f
     nop
    ldx         [%i1 + {saved_g2}], %l0
    add         %l0, 1, %l0
    stx         %l0, [%i1 + {saved_g2}]
7:  ldx         [%i1 + {saved_g5}], %l0
    stx         %l0, [%i1 + {saved_tpc}]
    add         %l0, 4, %l0
    stx         %l0, [%i1 + {saved_tnpc}]
    ret
     restore

    .global sparc64_user_test_linux_signal_queue
sparc64_user_test_linux_signal_queue:
    add         %sp, -496, %sp
    add         %sp, {bias} + 176, %l6
    SET64       1<<({queued_signal}-1), %l1
    stx         %l1, [%l6 + 272]
    stx         %g0, [%l6 + 256]
    stx         %g0, [%l6 + 264]
    mov         1, %o0
    add         %l6, 272, %o1
    mov         0, %o2
    mov         8, %o3
    SYS         103
    OK
    SYS         20
    mov         %o0, %l0
    mov         {queued_signal}, %l1
    stw         %l1, [%l6 + 128]
    stw         %g0, [%l6 + 132]
    mov         -1, %l1
    stw         %l1, [%l6 + 136]
    stw         %g0, [%l6 + 140]
    stw         %l0, [%l6 + 144]
    stw         %g0, [%l6 + 148]
    SET64       {first_value}, %l1
    stx         %l1, [%l6 + 152]
    mov         %l0, %o0
    mov         {queued_signal}, %o1
    add         %l6, 128, %o2
    SYS         106
    OK
    SET64       {second_value}, %l1
    stx         %l1, [%l6 + 152]
    mov         %l0, %o0
    mov         {queued_signal}, %o1
    add         %l6, 128, %o2
    SYS         106
    OK
    mov         %l0, %o0
    mov         {queued_signal}, %o1
    SYS         37
    OK
    mov         -1, %l2
    SET64       {first_value}, %l3
    call        6f
     nop
    SET64       {second_value}, %l3
    call        6f
     nop
    mov         0, %l2
    mov         0, %l3
    call        6f
     nop
    add         %l6, 272, %o0
    mov         %l6, %o1
    add         %l6, 256, %o2
    mov         8, %o3
    SYS         105
    FAILED_WITH 11
    EXIT_OK_OR_FAIL
6:  add         %l6, 272, %o0
    mov         %l6, %o1
    add         %l6, 256, %o2
    mov         8, %o3
    SYS         105
    bcs,pn      %xcc, 9b
     nop
    cmp         %o0, {queued_signal}
    bne,pn      %xcc, 9b
     nop
    ldsw        [%l6], %l1
    cmp         %l1, {queued_signal}
    bne,pn      %xcc, 9b
     nop
    ldsw        [%l6 + 8], %l1
    cmp         %l1, %l2
    bne,pn      %xcc, 9b
     nop
    ldsw        [%l6 + 16], %l1
    cmp         %l1, %l0
    bne,pn      %xcc, 9b
     nop
    ldx         [%l6 + 24], %l1
    cmp         %l1, %l3
    bne,pn      %xcc, 9b
     nop
    retl
     nop

    .global sparc64_user_test_linux_signal_suspend
sparc64_user_test_linux_signal_suspend:
    add         %sp, -272, %sp
    add         %sp, {bias} + 176, %l6
1:  rd          %pc, %l1
    add         %l1, 5f - 1b, %l1
    stx         %l1, [%l6]
    mov         0x200, %l1
    stx         %l1, [%l6 + 8]
    stx         %g0, [%l6 + 16]
    stx         %g0, [%l6 + 24]
    mov         30, %o0
    mov         %l6, %o1
    mov         0, %o2
    mov         0, %o3
    mov         8, %o4
    SYS         102
    OK
    sethi       %hi(0x20000000), %l1
    stx         %l1, [%l6 + 32]
    stx         %g0, [%l6 + 40]
    mov         1, %o0
    add         %l6, 32, %o1
    mov         0, %o2
    mov         8, %o3
    SYS         103
    OK
    SYS         20
    mov         %o0, %o1
    mov         30, %o2
    SYS         211
    OK
    mov         1, %g2
    add         %l6, 40, %o0
    mov         8, %o1
    SYS         107
    FAILED_WITH 4
    EXPECT      %g2, 2
    mov         1, %o0
    mov         0, %o1
    add         %l6, 48, %o2
    mov         8, %o3
    SYS         103
    OK
    ldx         [%l6 + 48], %l1
    EXPECT      %l1, 0x20000000
    EXIT_OK_OR_FAIL
5:  save        %sp, -192, %sp
    cmp         %i0, 30
    bne,pn      %xcc, 7f
     nop
    ldx         [%i1 + {frame_sigmask}], %l0
    sethi       %hi(0x20000000), %l1
    cmp         %l0, %l1
    bne,pn      %xcc, 7f
     nop
    mov         2, %l0
    stx         %l0, [%i1 + {saved_g2}]
7:  ret
     restore

    .global sparc64_user_test_linux_clock
sparc64_user_test_linux_clock:
    add         %sp, -240, %sp
    add         %sp, {bias} + 176, %l6
    mov         1, %o0
    mov         %l6, %o1
    SYS         257
    OK
    stx         %g0, [%l6 + 32]
    SET64       {clock_wait}, %l1
    stx         %l1, [%l6 + 40]
    add         %l6, 32, %o0
    mov         0, %o1
    SYS         249
    OK
    mov         1, %o0
    add         %l6, 16, %o1
    SYS         257
    OK
    ldx         [%l6 + 16], %l1
    ldx         [%l6], %l2
    sub         %l1, %l2, %l1
    SET64       1000000000, %l2
    mulx        %l1, %l2, %l1
    ldx         [%l6 + 24], %l2
    add         %l1, %l2, %l1
    ldx         [%l6 + 8], %l2
    sub         %l1, %l2, %l1
    ldx         [%l6 + 40], %l2
    cmp         %l1, %l2
    blu,pn      %xcc, 9f
     nop
    mov         2, %o0
    add         %l6, 16, %o1
    SYS         257
    OK
    EXIT_OK_OR_FAIL

    .global sparc64_user_test_linux_stat
sparc64_user_test_linux_stat:
    add         %sp, -448, %sp
    add         %sp, {bias} + 176, %l6
    stb         %g0, [%l6 + 256]
    mov         1, %o0
    mov         %l6, %o1
    SYS         62
    OK
    lduw        [%l6 + 16], %l1
    srl         %l1, 12, %l1
    and         %l1, 15, %l1
    EXPECT      %l1, 2
    lduw        [%l6 + 32], %l1
    EXPECT      %l1, 0x501
    stw         %g0, [%l6 + 32]
    mov         1, %o0
    add         %l6, 256, %o1
    mov         %l6, %o2
    sethi       %hi(0x1000), %o3
    SYS         289
    OK
    lduw        [%l6 + 32], %l1
    EXPECT      %l1, 0x501
    sth         %g0, [%l6 + 28]
    stw         %g0, [%l6 + 128]
    mov         1, %o0
    add         %l6, 256, %o1
    sethi       %hi(0x1000), %o2
    mov         0x7ff, %o3
    mov         %l6, %o4
    SYS         360
    OK
    lduh        [%l6 + 28], %l1
    srl         %l1, 12, %l1
    and         %l1, 15, %l1
    EXPECT      %l1, 2
    lduw        [%l6 + 128], %l1
    EXPECT      %l1, 5
    mov         1, %o0
    mov         0, %o1
    mov         1, %o2
    SYS         19
    FAILED_WITH 29
    mov         3, %o0
    mov         %l6, %o1
    SYS         62
    FAILED_WITH 9
    EXIT_OK_OR_FAIL

    .global sparc64_user_test_linux_credentials
sparc64_user_test_linux_credentials:
    SYS         49
    OK
    mov         1000, %o0
    SYS         23
    OK
    SYS         24
    bcs,pn      %xcc, 9f
     nop
    EXPECT      %o0, 1000
    mov         0, %o0
    SYS         23
    FAILED_WITH 1
    EXIT_OK_OR_FAIL

    .global sparc64_user_test_linux_move_and_discard
sparc64_user_test_linux_move_and_discard:
    mov         0, %o0
    sethi       %hi(8192), %o1
    mov         3, %o2
    mov         0x22, %o3
    mov         -1, %o4
    mov         0, %o5
    SYS         71
    bcs,pn      %xcc, 9f
     nop
    mov         %o0, %l0
    sethi       %hi(8192), %o1
    mov         23, %o2
    SYS         75
    OK
    SET64       0x12345678, %l2
    stx         %l2, [%l0]
    sethi       %hi(0x100000), %l1
    sub         %l0, %l1, %l1
    mov         %l0, %o0
    sethi       %hi(8192), %o1
    sethi       %hi(8192), %o2
    mov         3, %o3
    mov         %l1, %o4
    SYS         250
    bcs,pn      %xcc, 9f
     nop
    cmp         %o0, %l1
    bne,pn      %xcc, 9f
     nop
    ldx         [%l1], %l3
    cmp         %l3, %l2
    bne,pn      %xcc, 9f
     nop
    mov         %l0, %o0
    sethi       %hi(8192), %o1
    mov         3, %o2
    mov         0x32, %o3
    mov         -1, %o4
    mov         0, %o5
    SYS         71
    bcs,pn      %xcc, 9f
     nop
    cmp         %o0, %l0
    bne,pn      %xcc, 9f
     nop
    ldx         [%l0], %l3
    EXPECT      %l3, 0
    mov         %l1, %o0
    sethi       %hi(8192), %o1
    mov         4, %o2
    SYS         75
    OK
    ldx         [%l1], %l3
    EXPECT      %l3, 0
    EXIT_OK_OR_FAIL
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
    syscall_first = const TestProgram::SYSCALL_FIRST,
    syscall_report = const TestProgram::SYSCALL_REPORT,
    fsr_version = const FSR_VERSION,
    thread_pointer_rounds = const TestProgram::LINUX_THREAD_POINTER_ROUNDS,
    poll_wait = const TestProgram::LINUX_POLL_WAIT_NS,
    clock_wait = const TestProgram::LINUX_CLOCK_WAIT_NS,
    signal_fp_kept = const TestProgram::LINUX_SIGNAL_FP_KEPT,
    signal_fp_changed = const TestProgram::LINUX_SIGNAL_FP_CHANGED,
    signal_rounding = const 2,
    frame_fpu = const FRAME_REGS + 160,
    frame_stack_sp = const FRAME_REGS + 168,
    frame_stack_size = const FRAME_REGS + 184,
    frame_sigmask = const FRAME_REGS + 192,
    saved_g2 = const FRAME_REGS + 2 * 8,
    saved_g3 = const FRAME_REGS + 3 * 8,
    saved_g4 = const FRAME_REGS + 4 * 8,
    saved_g5 = const FRAME_REGS + 5 * 8,
    saved_tpc = const FRAME_REGS + 136,
    saved_tnpc = const FRAME_REGS + 144,
    fault_address = const TestProgram::LINUX_FAULT_ADDRESS,
    queued_signal = const TestProgram::LINUX_QUEUED_SIGNAL,
    first_value = const TestProgram::LINUX_QUEUED_VALUES[0],
    second_value = const TestProgram::LINUX_QUEUED_VALUES[1],
);

/// Where the test programs' handlers find the interrupted state, from the `siginfo_t` they
/// get: `struct pt_regs` right after it (`u_regs[16]`, `%g0`-`%o7`, then `TSTATE`, `%pc`,
/// `%npc`), then the pointer to the FP registers, the `stack_t` and the mask.
const FRAME_REGS: usize = 128;
/// `%fsr`'s read-only `ver` field, which differs between CPUs and is not program state.
const FSR_VERSION: u64 = 0x7 << 17;

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
    static sparc64_user_test_checks: u8;
    static sparc64_user_test_fp_registers: u8;
    static sparc64_user_test_linux_thread_pointer: u8;
    static sparc64_user_test_linux_poll: u8;
    static sparc64_user_test_linux_signals: u8;
    static sparc64_user_test_linux_signal_handler: u8;
    static sparc64_user_test_linux_fault_handler: u8;
    static sparc64_user_test_linux_signal_queue: u8;
    static sparc64_user_test_linux_signal_suspend: u8;
    static sparc64_user_test_linux_clock: u8;
    static sparc64_user_test_linux_stat: u8;
    static sparc64_user_test_linux_credentials: u8;
    static sparc64_user_test_linux_move_and_discard: u8;
    static sparc64_user_test_end: u8;
}

/// [`crate::Platform::user_test_program`]: every program, the Linux ones in SPARC64's numbers
/// and values (D418).
pub(super) fn user_test_program() -> TestProgram {
    let start = (&raw const sparc64_user_test_start) as usize;
    let end = (&raw const sparc64_user_test_end) as usize;
    let at = |symbol: *const u8| symbol as usize - start;
    // SAFETY: the symbols bound the programs' bytes in `.rodata.user_test`, part of the kernel
    // image, read-only and never written, for as long as the kernel runs.
    let code = unsafe { core::slice::from_raw_parts(start as *const u8, end - start) };
    TestProgram {
        code,
        checks: at(&raw const sparc64_user_test_checks),
        floating_point: at(&raw const sparc64_user_test_floating_point),
        null_syscalls: at(&raw const sparc64_user_test_null_syscalls),
        spin: at(&raw const sparc64_user_test_spin),
        fp_registers: at(&raw const sparc64_user_test_fp_registers),
        illegal_instruction: at(&raw const sparc64_user_test_illegal_instruction),
        linux_hello: at(&raw const sparc64_user_test_linux_hello),
        linux_start: at(&raw const sparc64_user_test_linux_start),
        linux_start_end: at(&raw const sparc64_user_test_linux_start_end),
        linux_unknown_syscalls: at(&raw const sparc64_user_test_linux_unknown_syscalls),
        stack_growth: at(&raw const sparc64_user_test_stack_growth),
        linux_thread_pointer: at(&raw const sparc64_user_test_linux_thread_pointer),
        linux_poll: at(&raw const sparc64_user_test_linux_poll),
        linux_signals: at(&raw const sparc64_user_test_linux_signals),
        linux_signal_handler: at(&raw const sparc64_user_test_linux_signal_handler),
        linux_fault_handler: at(&raw const sparc64_user_test_linux_fault_handler),
        linux_signal_queue: at(&raw const sparc64_user_test_linux_signal_queue),
        linux_signal_suspend: at(&raw const sparc64_user_test_linux_signal_suspend),
        linux_clock: at(&raw const sparc64_user_test_linux_clock),
        linux_stat: at(&raw const sparc64_user_test_linux_stat),
        linux_credentials: at(&raw const sparc64_user_test_linux_credentials),
        linux_move_and_discard: at(&raw const sparc64_user_test_linux_move_and_discard),
    }
}
