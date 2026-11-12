//! 32-bit Arm's way into and out of user mode (D482). `arm_enter_user` saves the kernel's
//! callee-saved registers and the context's address on the SVC stack, sets `TPIDRURO` and
//! `SPSR`, loads `r0` to `r14` into user mode's registers with `LDM ^` and returns to the
//! program with `MOVS PC, LR`. Every exception vector first stacks its return address and
//! `SPSR` on the SVC stack (`SRSDB`) and switches to SVC mode; one taken from user mode then
//! saves the program's registers into the context through `STM ^`, records why, and returns
//! from `arm_enter_user` into [`run_user`]. One taken in the kernel is an IRQ, handled and
//! resumed, or a fault, which stops the machine.

use crate::user::arm::Registers;
use crate::user::{UserContext, UserExit};
use crate::{Exception, ExceptionKind};

const PC: usize = 60;
const CPSR: usize = 64;
const CAUSE: usize = 68;
const TLS: usize = 80;
const CAUSE_SVC: u32 = 1;
const CAUSE_UNDEFINED: u32 = 2;
const CAUSE_PREFETCH_ABORT: u32 = 3;
const CAUSE_DATA_ABORT: u32 = 4;
const CAUSE_IRQ: u32 = 5;
const USER_MODE: u32 = 0x10;
const PROGRAM_FLAGS: u32 = 0xf800_0000 | 0x000f_0000 | 1 << 5;
const PREFETCH_MARK: u64 = 1 << 31;

const _: () = assert!(core::mem::offset_of!(Registers, pc) == PC);
const _: () = assert!(core::mem::offset_of!(Registers, cpsr) == CPSR);
const _: () = assert!(core::mem::offset_of!(Registers, cause) == CAUSE);
const _: () = assert!(core::mem::offset_of!(Registers, thread_pointer) == TLS);

core::arch::global_asm!(
    r#"
    .section .text, "ax"
    .arm
    .global arm_enter_user
arm_enter_user:
    push    {{r4-r11, lr}}
    push    {{r0}}
    ldr     r1, [r0, #{tls}]
    mcr     p15, 0, r1, c13, c0, 3
    ldr     r1, [r0, #{cpsr}]
    msr     spsr_cxsf, r1
    mov     lr, r0
    ldmia   lr, {{r0-r14}}^
    nop
    ldr     lr, [lr, #{pc}]
    movs    pc, lr

arm_user_exit:
    push    {{r0}}
    ldr     r0, [sp, #12]
    add     r0, r0, #4
    stmia   r0, {{r1-r14}}^
    nop
    sub     r0, r0, #4
    str     lr, [r0, #{cause}]
    pop     {{r1}}
    str     r1, [r0]
    pop     {{r1}}
    str     r1, [r0, #{pc}]
    pop     {{r1}}
    str     r1, [r0, #{cpsr}]
    add     sp, sp, #4
    pop     {{r4-r11, lr}}
    bx      lr

    .macro  from_user_or target
    push    {{r0}}
    ldr     r0, [sp, #8]
    and     r0, r0, #0x1f
    cmp     r0, #{user_mode}
    pop     {{r0}}
    bne     \target
    .endm

    .global arm_svc_entry
arm_svc_entry:
    srsdb   sp!, #0x13
    from_user_or arm_fatal
    mov     lr, #{svc}
    b       arm_user_exit

    .global arm_undefined_entry
arm_undefined_entry:
    sub     lr, lr, #4
    srsdb   sp!, #0x13
    cps     #0x13
    from_user_or arm_fatal
    mov     lr, #{undefined}
    b       arm_user_exit

    .global arm_prefetch_entry
arm_prefetch_entry:
    sub     lr, lr, #4
    srsdb   sp!, #0x13
    cps     #0x13
    from_user_or arm_fatal
    mov     lr, #{prefetch}
    b       arm_user_exit

    .global arm_data_entry
arm_data_entry:
    sub     lr, lr, #8
    srsdb   sp!, #0x13
    cps     #0x13
    from_user_or arm_fatal
    mov     lr, #{data}
    b       arm_user_exit

    .global arm_irq_entry
arm_irq_entry:
    sub     lr, lr, #4
    srsdb   sp!, #0x13
    cps     #0x13
    from_user_or arm_irq_in_kernel
    mov     lr, #{irq}
    b       arm_user_exit
arm_irq_in_kernel:
    push    {{r0-r3, r12, lr}}
    bl      {irq_handler}
    pop     {{r0-r3, r12, lr}}
    rfeia   sp!
"#,
    tls = const TLS,
    cpsr = const CPSR,
    pc = const PC,
    cause = const CAUSE,
    user_mode = const USER_MODE,
    svc = const CAUSE_SVC,
    undefined = const CAUSE_UNDEFINED,
    prefetch = const CAUSE_PREFETCH_ABORT,
    data = const CAUSE_DATA_ABORT,
    irq = const CAUSE_IRQ,
    irq_handler = sym super::irq,
);

unsafe extern "C" {
    fn arm_enter_user(registers: *mut Registers);
}

fn fault_registers(prefetch: bool) -> (u32, u32) {
    let status: u32;
    let address: u32;
    // SAFETY: reads the fault status and address registers, which only the abort just taken
    // has set; no side effects.
    unsafe {
        match prefetch {
            true => core::arch::asm!(
                "mrc p15, 0, {status}, c5, c0, 1",
                "mrc p15, 0, {address}, c6, c0, 2",
                status = out(reg) status,
                address = out(reg) address,
            ),
            false => core::arch::asm!(
                "mrc p15, 0, {status}, c5, c0, 0",
                "mrc p15, 0, {address}, c6, c0, 0",
                status = out(reg) status,
                address = out(reg) address,
            ),
        }
    }
    (status, address)
}

/// [`crate::Platform::run_user`] on 32-bit Arm: the caller guarantees interrupts are masked
/// and the active tables' kernel half is the kernel's.
pub(super) unsafe fn run_user(context: &mut UserContext) -> UserExit {
    let registers = &mut context.arch;
    registers.cpsr = registers.cpsr & PROGRAM_FLAGS | USER_MODE;
    // SAFETY: interrupts are masked until the `MOVS PC, LR` that enters user mode; every
    // exception from user mode saves the program's registers into the context, which `&mut`
    // keeps valid and unaliased, and returns here with the kernel's registers restored.
    unsafe { arm_enter_user(registers) };
    match registers.cause {
        CAUSE_SVC => UserExit::Syscall,
        CAUSE_IRQ => {
            super::irq();
            UserExit::Interrupt
        }
        cause => {
            let prefetch = cause == CAUSE_PREFETCH_ABORT;
            if prefetch || cause == CAUSE_DATA_ABORT {
                let (status, address) = fault_registers(prefetch);
                registers.fault_status = status;
                registers.fault_address = address;
            }
            UserExit::Exception(registers.last_exception())
        }
    }
}

impl Registers {
    /// [`UserContext::last_exception`] on 32-bit Arm: from the cause, status and address
    /// `run_user` kept.
    pub(crate) fn last_exception(&self) -> Exception {
        let (kind, name, code, address) = match self.cause {
            CAUSE_UNDEFINED => (
                ExceptionKind::IllegalInstruction,
                "undefined instruction",
                0,
                None,
            ),
            CAUSE_PREFETCH_ABORT => (
                ExceptionKind::InstructionFault,
                "prefetch abort",
                u64::from(self.fault_status) | PREFETCH_MARK,
                Some(u64::from(self.fault_address)),
            ),
            CAUSE_DATA_ABORT => (
                ExceptionKind::DataFault,
                "data abort",
                u64::from(self.fault_status),
                Some(u64::from(self.fault_address)),
            ),
            _ => (ExceptionKind::Other, "exception", 0, None),
        };
        Exception {
            kind,
            name,
            code,
            pc: u64::from(self.pc),
            address,
        }
    }
}
