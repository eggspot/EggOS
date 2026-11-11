//! MIPS64 exception entry and the TLB (D313).
//!
//! `EBase` points at [`mips64_vectors`]: both TLB refill vectors jump to the refill handler,
//! which walks the page tables (`paging::mips64`) with `k0` and `k1` only and writes a random
//! TLB entry, an invalid pair where no page is mapped so the access raises a TLB-invalid
//! exception the kernel sees; the general vector saves the registers on the kernel stack, clears
//! `Status.EXL` so the kernel's handler may take TLB refills itself, calls
//! [`mips64_exception`] and restores everything, unless the exception came from user mode
//! (`Status.KSU`), whose registers go into its `UserContext` (`KScratch2` holds it) before
//! [`run_user`] returns (D47). `KScratch1` holds a program's root table and [`KERNEL_ROOT`] the
//! kernel's, both as cached-window addresses.

use super::{CACHED_WINDOW, timer};
use crate::Interrupt;
use crate::exception::{Exception, ExceptionKind, deliver};
use crate::paging::{Access, PageFault};
use crate::user::{self, TestProgram, UserContext, UserExit};
use core::sync::atomic::{AtomicU64, Ordering};

core::arch::global_asm!(
    r#"
    .set        noat
    .pushsection .text.vectors, "ax"
    .balign 4096
    .global mips64_vectors
mips64_vectors:
    j           mips64_tlb_refill
    .org        mips64_vectors + 0x080
    j           mips64_tlb_refill
    .org        mips64_vectors + 0x100
    j           mips64_cache_error
    .org        mips64_vectors + 0x180
    j           mips64_general_entry
    .org        mips64_vectors + 0x200
    j           mips64_general_entry
    .org        mips64_vectors + 0x280

mips64_tlb_refill:
    dmfc0       $26, $8
    bltz        $26, 1f
    dmfc0       $27, $31, 2
    b           2f
1:  dla         $27, {kernel_root}
    ld          $27, 0($27)
2:  beqz        $27, 9f
    dsrl        $26, $26, 36
    andi        $26, $26, 0xff8
    daddu       $27, $27, $26
    ld          $27, 0($27)
    beqz        $27, 9f
    dmfc0       $26, $8
    dsrl        $26, $26, 27
    andi        $26, $26, 0xff8
    daddu       $27, $27, $26
    ld          $27, 0($27)
    beqz        $27, 9f
    dmfc0       $26, $8
    dsrl        $26, $26, 18
    andi        $26, $26, 0xff8
    daddu       $27, $27, $26
    ld          $27, 0($27)
    beqz        $27, 9f
    dmfc0       $26, $8
    dsrl        $26, $26, 9
    andi        $26, $26, 0xff0
    daddu       $27, $27, $26
    ld          $26, 0($27)
    ld          $27, 8($27)
    dmtc0       $26, $2
    dmtc0       $27, $3
    ehb
    tlbwr
    eret
9:  dmtc0       $zero, $2
    dmtc0       $zero, $3
    ehb
    tlbwr
    eret

mips64_cache_error:
    wait
    b           mips64_cache_error

mips64_general_entry:
    mfc0        $26, $12
    andi        $26, $26, 0x18
    bnez        $26, mips64_user_entry
    move        $27, $29
    daddiu      $29, $29, -{frame_size}
    sd          $27, 29 * 8($29)
    .irp        n, 1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,28,30,31
    sd          $\n, \n * 8($29)
    .endr
    mfhi        $12
    sd          $12, 256($29)
    mflo        $12
    sd          $12, 264($29)
    dmfc0       $12, $14
    sd          $12, 272($29)
    mfc0        $12, $12
    sd          $12, 280($29)
    ori         $12, $12, 0x1f
    xori        $12, $12, 0x1f
    mtc0        $12, $12
    ehb
    move        $4, $29
    jal         {handler}
    ld          $12, 280($29)
    mtc0        $12, $12
    ehb
    ld          $12, 272($29)
    dmtc0       $12, $14
    ld          $12, 256($29)
    mthi        $12
    ld          $12, 264($29)
    mtlo        $12
    .irp        n, 1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,28,30,31
    ld          $\n, \n * 8($29)
    .endr
    ld          $29, 29 * 8($29)
    eret

    // From user mode (D47): the registers go into the `UserContext` `KScratch2` holds, then
    // `mips64_enter_user` returns on the kernel stack it left, with the kernel's callee-saved
    // registers and `gp`, in kernel mode with `EXL` clear and interrupts masked.
mips64_user_entry:
    dmfc0       $26, $31, 3
    .irp        n, 1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,28,29,30,31
    sd          $\n, \n * 8($26)
    .endr
    mfhi        $12
    sd          $12, 264($26)
    mflo        $12
    sd          $12, 272($26)
    dmfc0       $12, $14
    sd          $12, 256($26)
    dmfc0       $12, $4, 2
    sd          $12, 280($26)
    ld          $29, 288($26)
    ld          $31, 0($29)
    .irp        n, 16,17,18,19,20,21,22,23
    ld          $\n, (\n - 15) * 8($29)
    .endr
    ld          $30, 72($29)
    ld          $28, 80($29)
    daddiu      $29, $29, 96
    mfc0        $12, $12
    ori         $12, $12, 0x1f
    xori        $12, $12, 0x1f
    mtc0        $12, $12
    ehb
    jr          $31

    // `mips64_enter_user(context)`: keeps the kernel's callee-saved registers and `gp` on its
    // stack and the stack pointer in the context, points `KScratch2` at the context, sets
    // `UserLocal`, `EPC`, `hi` and `lo`, then `Status` to user mode with interrupts enabled
    // (`EXL` keeps both off until the `eret`), loads the program's registers and returns to it.
    .global mips64_enter_user
mips64_enter_user:
    daddiu      $29, $29, -96
    sd          $31, 0($29)
    .irp        n, 16,17,18,19,20,21,22,23
    sd          $\n, (\n - 15) * 8($29)
    .endr
    sd          $30, 72($29)
    sd          $28, 80($29)
    sd          $29, 288($4)
    dmtc0       $4, $31, 3
    ld          $12, 280($4)
    dmtc0       $12, $4, 2
    ld          $12, 256($4)
    dmtc0       $12, $14
    ld          $12, 264($4)
    mthi        $12
    ld          $12, 272($4)
    mtlo        $12
    mfc0        $12, $12
    ori         $12, $12, 0x1f
    xori        $12, $12, 0x0c
    mtc0        $12, $12
    ehb
    .irp        n, 1,2,3,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,28,29,30,31
    ld          $\n, \n * 8($4)
    .endr
    ld          $4, 4 * 8($4)
    eret
    .popsection

    // The user-mode test programs (D47, `TestProgram`): position-independent, no data. Copied to
    // a user page, never run where they are linked. The Linux ones use n64's numbers, whose
    // errors set `a3` and leave the error number positive in `v0`.
    .pushsection .rodata.user_test, "a"
    .set        hardfloat
    .balign 4
    .global mips64_user_test_start
mips64_user_test_start:
    .global mips64_user_test_checks
mips64_user_test_checks:
    li          $4, 1
    li          $5, 2
    li          $6, 3
    li          $7, 4
    li          $8, 5
    li          $9, 6
    .irp        n, 1,3,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,28,30,31
    li          $\n, 0x100 + \n
    .endr
    li          $2, {syscall_first}
    syscall
    daddiu      $5, $5, -2
    daddiu      $6, $6, -3
    or          $5, $5, $6
    daddiu      $8, $8, -5
    or          $5, $5, $8
    daddiu      $9, $9, -6
    or          $5, $5, $9
    .irp        n, 1,3,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,28,30,31
    daddiu      $\n, $\n, -(0x100 + \n)
    or          $5, $5, $\n
    .endr
    move        $4, $2
    li          $2, {syscall_report}
    syscall
    ld          $5, 0($2)
1:  b           1b
    .global mips64_user_test_floating_point
mips64_user_test_floating_point:
    dmtc1       $zero, $f0
    .global mips64_user_test_null_syscalls
mips64_user_test_null_syscalls:
    li          $2, {syscall_null}
2:  syscall
    b           2b
    .global mips64_user_test_spin
mips64_user_test_spin:
3:  b           3b
    .global mips64_user_test_linux_hello
mips64_user_test_linux_hello:
    li          $4, 1
    bal         5f
5:  daddiu      $5, $31, 9f - 5b
    li          $6, {hello_length}
    li          $2, {linux_write}
    syscall
    daddiu      $16, $2, -{hello_length}
    or          $16, $16, $7
    li          $4, 1
    li          $5, 0
    li          $6, 1
    li          $2, {linux_write}
    syscall
    daddiu      $12, $2, -{efault}
    or          $16, $16, $12
    daddiu      $12, $7, -1
    or          $16, $16, $12
    li          $2, {unknown_syscall}
    syscall
    daddiu      $12, $2, -{enosys}
    or          $16, $16, $12
    daddiu      $12, $7, -1
    or          $16, $16, $12
    sltu        $4, $zero, $16
    li          $2, {linux_exit_group}
    syscall
4:  b           4b
9:  .ascii      "EGGOS-USER: hello from user mode\n"
    .balign 4
    .global mips64_user_test_linux_start
mips64_user_test_linux_start:
    li          $4, 1
    li          $2, {linux_exit_group}
    syscall
6:  b           6b
    .global mips64_user_test_linux_start_end
mips64_user_test_linux_start_end:
    .global mips64_user_test_linux_unknown_syscalls
mips64_user_test_linux_unknown_syscalls:
    li          $2, {unknown_syscall}
7:  syscall
    b           7b
    .global mips64_user_test_stack_growth
mips64_user_test_stack_growth:
    li          $12, -4096
    and         $12, $29, $12
    li          $13, 4096
8:  sd          $12, 0($12)
    dsubu       $12, $12, $13
    b           8b
    .global mips64_user_test_linux_pending
mips64_user_test_linux_pending:
    li          $4, 1
    li          $2, {linux_exit_group}
    syscall
9:  b           9b
    .global mips64_user_test_fp_registers
mips64_user_test_fp_registers:
    li          $2, {syscall_first}
    syscall
    move        $16, $2
    li          $17, 0
    cfc1        $12, $31
    or          $17, $17, $12
    .irp        i, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    dmfc1       $12, $f\i
    or          $17, $17, $12
    .endr
    andi        $12, $16, 3
    ctc1        $12, $31
    .irp        i, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    daddiu      $12, $16, \i
    dmtc1       $12, $f\i
    .endr
10: li          $2, {syscall_null}
    syscall
    move        $18, $2
    .irp        i, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    daddiu      $12, $16, \i
    dmfc1       $13, $f\i
    xor         $13, $13, $12
    or          $17, $17, $13
    .endr
    cfc1        $12, $31
    andi        $13, $16, 3
    xor         $12, $12, $13
    or          $17, $17, $12
    beqz        $18, 10b
    move        $4, $17
    li          $2, {syscall_report}
    syscall
11: b           11b
    .global mips64_user_test_illegal_instruction
mips64_user_test_illegal_instruction:
    .word       {reserved_instruction}
    .global mips64_user_test_end
mips64_user_test_end:
    .popsection
"#,
    kernel_root = sym KERNEL_ROOT,
    frame_size = const core::mem::size_of::<TrapFrame>(),
    handler = sym mips64_exception,
    syscall_first = const TestProgram::SYSCALL_FIRST,
    syscall_report = const TestProgram::SYSCALL_REPORT,
    syscall_null = const TestProgram::SYSCALL_NULL,
    hello_length = const TestProgram::LINUX_HELLO.len(),
    unknown_syscall = const TestProgram::LINUX_UNKNOWN_SYSCALL,
    linux_write = const 5001,
    linux_exit_group = const 5205,
    efault = const 14,
    enosys = const 89,
    reserved_instruction = const 5,
);

/// The kernel's root table, as a cached-window address; 0 until the kernel's tables are active.
static KERNEL_ROOT: AtomicU64 = AtomicU64::new(0);

/// The registers saved by the kernel entry, in the order it stores them.
#[repr(C)]
struct TrapFrame {
    /// `$0` to `$31` by register number; `r[0]`, `r[26]` and `r[27]` are unused and `r[29]` is
    /// the `sp` at the time of the exception.
    r: [u64; 32],
    hi: u64,
    lo: u64,
    epc: u64,
    status: u64,
}

const _: () = assert!(core::mem::size_of::<TrapFrame>() == 288);

/// `Cause.ExcCode` of the exceptions the HAL tells apart.
const INTERRUPT: u64 = 0;
const TLB_MODIFIED: u64 = 1;
const TLB_LOAD: u64 = 2;
const TLB_STORE: u64 = 3;
const ADDRESS_ERROR_LOAD: u64 = 4;
const ADDRESS_ERROR_STORE: u64 = 5;
const BUS_ERROR_FETCH: u64 = 6;
const BUS_ERROR_DATA: u64 = 7;
const SYSCALL: u64 = 8;
const BREAKPOINT: u64 = 9;
const RESERVED_INSTRUCTION: u64 = 10;
const COPROCESSOR_UNUSABLE: u64 = 11;
const OVERFLOW: u64 = 12;
const TRAP: u64 = 13;
const FLOATING_POINT: u64 = 15;
const TLB_READ_INHIBIT: u64 = 19;
const TLB_EXECUTE_INHIBIT: u64 = 20;
const WATCH: u64 = 23;
/// The size of every MIPS64 instruction.
const INSTRUCTION_SIZE: u64 = 4;
/// `Cause.BD`: the exception's instruction is in a branch delay slot, `EPC` the branch's.
const BRANCH_DELAY: u64 = 1 << 31;

/// TLB entries written by [`flush_tlb`] match addresses in this unused part of `XKSEG`, each
/// its own page pair, so no two entries match one address.
const UNUSED_PAGES: u64 = 0xc000_7000_0000_0000;
/// `EntryHi.ASID`.
const ASID_MASK: u64 = 0xff;
/// `Index.P`: `tlbp` found no entry.
const PROBE_FAILED: u64 = 1 << 31;

fn exception_code(cause: u64) -> u64 {
    (cause >> 2) & 0x1f
}

/// The number of TLB entries, from `Config1.MMUSize`.
fn tlb_entries() -> u64 {
    let config1: u64;
    // SAFETY: reading `Config1` has no side effects.
    unsafe { core::arch::asm!("mfc0 {0}, $16, 1", out(reg) config1, options(nomem, nostack)) };
    ((config1 >> 25) & 0x3f) + 1
}

fn entry_hi() -> u64 {
    let value: u64;
    // SAFETY: reading `EntryHi` has no side effects.
    unsafe { core::arch::asm!("dmfc0 {0}, $10", out(reg) value, options(nomem, nostack)) };
    value
}

/// Writes an invalid entry matching nothing used at TLB index `index`, `asid` its identifier.
fn invalidate_entry(index: u64, asid: u64) {
    // SAFETY: an entry with both halves invalid translates nothing; its page pair is one of
    // `UNUSED_PAGES`'s, distinct per index, so it duplicates no entry. The sequence touches no
    // mapped memory, so no refill changes `EntryHi` in between.
    unsafe {
        core::arch::asm!(
            "dmtc0 {hi}, $10",
            "dmtc0 $zero, $2",
            "dmtc0 $zero, $3",
            "mtc0 {index}, $0",
            "ehb",
            "tlbwi",
            "ehb",
            hi = in(reg) UNUSED_PAGES + (index << 13) | asid,
            index = in(reg) index,
            options(nomem, nostack),
        )
    };
}

/// Drops every TLB entry; `EntryHi` keeps this CPU's address-space identifier.
pub(super) extern "C" fn flush_tlb() {
    let asid = entry_hi() & ASID_MASK;
    for index in 0..tlb_entries() {
        invalidate_entry(index, asid);
    }
    set_entry_hi(asid);
}

fn set_entry_hi(value: u64) {
    // SAFETY: `EntryHi`'s identifier is the one the refills and probes that follow use; its page
    // field is only an argument to them.
    unsafe { core::arch::asm!("dmtc0 {0}, $10", "ehb", in(reg) value, options(nomem, nostack)) };
}

/// Points the refill handler at a program's root table `low` and, when given, the kernel's
/// `high` (physical addresses).
pub(super) fn set_roots(low: u64, high: Option<u64>) {
    if let Some(high) = high {
        KERNEL_ROOT.store(CACHED_WINDOW | high, Ordering::Release);
    }
    // SAFETY: `KScratch1` is the refill handler's alone; the next refill from the low half walks
    // from this root.
    unsafe { core::arch::asm!("dmtc0 {0}, $31, 2", "ehb", in(reg) CACHED_WINDOW | low, options(nomem, nostack)) };
}

/// Gives this CPU a fresh address-space identifier, so no entry another program's tables left
/// matches; when they run out, every entry goes and they start again.
pub(super) fn next_address_space() {
    let next = (entry_hi() & ASID_MASK) + 1;
    if next > ASID_MASK {
        set_entry_hi(1);
        flush_tlb();
        return;
    }
    set_entry_hi(next);
}

/// [`crate::Platform::drop_user_translations`]: probes for each page pair of the range and
/// invalidates what it finds; a long range takes a fresh identifier instead.
pub(super) fn drop_translations(start: u64, length: u64) {
    let pages = length / crate::memory::FRAME_SIZE;
    if pages > crate::paging::USER_PAGES_DROPPED_ONE_BY_ONE {
        return next_address_space();
    }
    let asid = entry_hi() & ASID_MASK;
    let mut pair = start & !0x1fff;
    while pair < start + length {
        let index: u64;
        // SAFETY: `tlbp` only looks the pair up and writes `Index`; nothing mapped is touched
        // in between, so no refill changes `EntryHi`.
        unsafe {
            core::arch::asm!(
                "dmtc0 {hi}, $10",
                "ehb",
                "tlbp",
                "ehb",
                "mfc0 {index}, $0",
                hi = in(reg) pair | asid,
                index = out(reg) index,
                options(nomem, nostack),
            )
        };
        if index & PROBE_FAILED == 0 {
            invalidate_entry(index & 0xfff, asid);
        }
        pair += 0x2000;
    }
    set_entry_hi(asid);
}

/// Called by the kernel entry with the saved registers.
extern "C" fn mips64_exception(frame: &mut TrapFrame) {
    let cause = read_cause();
    let bad = read_bad_address();
    if exception_code(cause) == INTERRUPT && interrupt(cause) {
        return;
    }
    let exception = decode(cause, bad, frame.epc);
    deliver(&exception);
    if exception.kind == ExceptionKind::Breakpoint && cause & BRANCH_DELAY == 0 {
        frame.epc += INSTRUCTION_SIZE;
    }
}

fn read_cause() -> u64 {
    let value: u64;
    // SAFETY: reading `Cause` has no side effects.
    unsafe { core::arch::asm!("mfc0 {0}, $13", out(reg) value, options(nomem, nostack)) };
    value & 0xffff_ffff
}

fn read_bad_address() -> u64 {
    let value: u64;
    // SAFETY: reading `BadVAddr` has no side effects.
    unsafe { core::arch::asm!("dmfc0 {0}, $8", out(reg) value, options(nomem, nostack)) };
    value
}

/// Handles the pending interrupts `cause` names that the kernel takes; returns whether there
/// was one (if not, the interrupt is reported).
#[inline(always)]
fn interrupt(cause: u64) -> bool {
    let (timer_due, wakeup) = timer::pending(cause);
    if timer_due {
        timer::end_timer_interrupt();
        crate::interrupt::deliver(Interrupt::Timer);
    }
    if wakeup {
        timer::end_wakeup();
        crate::interrupt::deliver(Interrupt::Wakeup);
    }
    timer_due || wakeup
}

unsafe extern "C" {
    /// Enters user mode with `context`'s registers; returns once an exception from it saved
    /// them back.
    fn mips64_enter_user(context: *mut user::mips64::Registers);
    static mips64_user_test_start: u8;
    static mips64_user_test_checks: u8;
    static mips64_user_test_floating_point: u8;
    static mips64_user_test_null_syscalls: u8;
    static mips64_user_test_spin: u8;
    static mips64_user_test_fp_registers: u8;
    static mips64_user_test_illegal_instruction: u8;
    static mips64_user_test_linux_hello: u8;
    static mips64_user_test_linux_start: u8;
    static mips64_user_test_linux_start_end: u8;
    static mips64_user_test_linux_unknown_syscalls: u8;
    static mips64_user_test_stack_growth: u8;
    static mips64_user_test_linux_pending: u8;
    static mips64_user_test_end: u8;
}

/// [`crate::Platform::run_user`]: the caller guarantees interrupts are masked and the active
/// tables' kernel half is the kernel's.
pub(super) unsafe fn run_user(context: &mut UserContext) -> UserExit {
    let registers = &mut context.arch;
    // SAFETY: interrupts are masked (the caller's duty), so nothing but TLB refills, which do
    // not use `KScratch2`, arrives before the `eret`; the exception that ends the program saves
    // its registers into the context, which `&mut` keeps valid and unaliased, and returns here
    // on the kernel stack with the callee-saved registers and `gp` restored. The program runs in
    // user mode, where only `XUSEG` is reachable.
    unsafe { mips64_enter_user(registers) };
    let cause = read_cause();
    let bad = read_bad_address();
    match exception_code(cause) {
        SYSCALL => {
            registers.pc += INSTRUCTION_SIZE;
            registers.syscall_a3 = registers.r[7];
            return UserExit::Syscall;
        }
        INTERRUPT if interrupt(cause) => return UserExit::Interrupt,
        _ => {}
    }
    registers.cause = cause;
    registers.bad_address = bad;
    UserExit::Exception(decode(cause, bad, registers.pc))
}

impl crate::user::mips64::Registers {
    /// [`UserContext::last_exception`]: the exception `run_user` returned, from the `Cause` and
    /// `BadVAddr` it kept.
    pub(crate) fn last_exception(&self) -> Exception {
        decode(self.cause, self.bad_address, self.pc)
    }
}

/// [`crate::Platform::user_test_program`]. The Linux programs beyond `linux_hello` wait for
/// MIPS's Linux ABI (D313 part 2): each exits with 1 until then.
pub(super) fn user_test_program() -> TestProgram {
    let start = (&raw const mips64_user_test_start) as usize;
    let end = (&raw const mips64_user_test_end) as usize;
    let at = |symbol: *const u8| symbol as usize - start;
    // SAFETY: the symbols bound the programs' bytes in `.rodata.user_test`, part of the kernel
    // image, read-only and never written, for as long as the kernel runs.
    let code = unsafe { core::slice::from_raw_parts(start as *const u8, end - start) };
    let pending = at(&raw const mips64_user_test_linux_pending);
    TestProgram {
        code,
        checks: at(&raw const mips64_user_test_checks),
        floating_point: at(&raw const mips64_user_test_floating_point),
        null_syscalls: at(&raw const mips64_user_test_null_syscalls),
        spin: at(&raw const mips64_user_test_spin),
        fp_registers: at(&raw const mips64_user_test_fp_registers),
        illegal_instruction: at(&raw const mips64_user_test_illegal_instruction),
        linux_hello: at(&raw const mips64_user_test_linux_hello),
        linux_start: at(&raw const mips64_user_test_linux_start),
        linux_start_end: at(&raw const mips64_user_test_linux_start_end),
        linux_unknown_syscalls: at(&raw const mips64_user_test_linux_unknown_syscalls),
        stack_growth: at(&raw const mips64_user_test_stack_growth),
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

/// [`crate::Platform::page_fault`]: the TLB exceptions, which a stale entry may also raise, and
/// the read and execute inhibits.
pub(super) fn page_fault(kind: ExceptionKind, cause: u64) -> Option<PageFault> {
    let (access, may_be_stale) = match exception_code(cause) {
        TLB_MODIFIED => (Access::Write, true),
        TLB_LOAD if kind == ExceptionKind::InstructionFault => (Access::Execute, true),
        TLB_LOAD => (Access::Read, true),
        TLB_STORE => (Access::Write, true),
        TLB_READ_INHIBIT => (Access::Read, false),
        TLB_EXECUTE_INHIBIT => (Access::Execute, false),
        _ => return None,
    };
    Some(PageFault {
        access,
        may_be_stale,
    })
}

fn decode(cause: u64, bad: u64, pc: u64) -> Exception {
    let fetch = bad == pc;
    let (kind, name, has_address) = match exception_code(cause) {
        INTERRUPT => (ExceptionKind::Interrupt, "interrupt", false),
        TLB_MODIFIED => (ExceptionKind::DataFault, "TLB modified", true),
        TLB_LOAD if fetch => (ExceptionKind::InstructionFault, "TLB (fetch)", true),
        TLB_LOAD => (ExceptionKind::DataFault, "TLB (load)", true),
        TLB_STORE => (ExceptionKind::DataFault, "TLB (store)", true),
        ADDRESS_ERROR_LOAD if fetch => (ExceptionKind::InstructionFault, "address error (fetch)", true),
        ADDRESS_ERROR_LOAD | ADDRESS_ERROR_STORE if bad < (1 << 40) && bad & 7 != 0 => {
            (ExceptionKind::Alignment, "address not aligned", true)
        }
        ADDRESS_ERROR_LOAD => (ExceptionKind::DataFault, "address error (load)", true),
        ADDRESS_ERROR_STORE => (ExceptionKind::DataFault, "address error (store)", true),
        BUS_ERROR_FETCH | BUS_ERROR_DATA => (ExceptionKind::Other, "bus error", false),
        SYSCALL => (ExceptionKind::Other, "syscall", false),
        BREAKPOINT => (ExceptionKind::Breakpoint, "breakpoint (break)", false),
        RESERVED_INSTRUCTION => (ExceptionKind::IllegalInstruction, "reserved instruction", false),
        COPROCESSOR_UNUSABLE => (ExceptionKind::IllegalInstruction, "coprocessor unusable", false),
        OVERFLOW => (ExceptionKind::Arithmetic, "integer overflow", false),
        TRAP => (ExceptionKind::Arithmetic, "trap", false),
        FLOATING_POINT => (ExceptionKind::Arithmetic, "floating-point exception", false),
        TLB_READ_INHIBIT => (ExceptionKind::DataFault, "TLB read inhibit", true),
        TLB_EXECUTE_INHIBIT => (ExceptionKind::InstructionFault, "TLB execute inhibit", true),
        WATCH => (ExceptionKind::Debug, "watch", true),
        _ => (ExceptionKind::Other, "exception", false),
    };
    Exception {
        kind,
        name,
        code: cause,
        pc,
        address: has_address.then_some(bad),
    }
}
