//! x86_64 exception and interrupt vectors (Intel SDM vol. 3A, chapter 6 "Interrupt and
//! Exception Handling").
//!
//! The IDT has a gate for each of the 32 architecture-defined exception vectors and for each
//! interrupt vector in use (the legacy PICs' 0x20-0x2f, the APIC timer's 0x30, the wake-up
//! interrupt's 0x31 and the APIC's spurious vector 0xff, see [`super::timer`]). Each points at a 16-byte stub that pushes the
//! vector number (and a 0 error code for the vectors where the CPU pushes none), so every
//! exception and interrupt reaches `exception_common` with the same frame.
//! That saves the general-purpose registers, calls [`x86_64_exception`] with the frame, and
//! restores everything and `iretq`s if it returns. The kernel target uses no SSE/x87
//! registers (`x86_64-unknown-none` is soft-float), so they are not saved.
//!
//! All exceptions run on the current stack: there is no IST stack yet, so a stack overflow
//! that faults again ends in a triple fault (a reset, which the QEMU runner detects). Separate
//! stacks for double faults and NMIs come with the kernel's own page tables (M2).

use super::timer::{self, APIC_SPURIOUS_VECTOR, LEGACY_PIC_VECTORS, TIMER_VECTOR, WAKEUP_VECTOR};
use crate::exception::{Exception, ExceptionKind, deliver};
use core::cell::UnsafeCell;

core::arch::global_asm!(
    r#"
    // Each stub is at most 15 bytes (two pushes of up to 5 bytes and a 5-byte jmp), padded
    // to 16.
    .macro EXCEPTION_STUB vector
    .balign 16
    pushq   $0
    pushq   $\vector
    jmp     exception_common
    .endm

    .macro EXCEPTION_STUB_WITH_ERROR_CODE vector
    .balign 16
    pushq   $\vector
    jmp     exception_common
    .endm

    .pushsection .text.exceptions, "ax"
    .balign 16
    .global exception_stubs
exception_stubs:
    EXCEPTION_STUB 0
    EXCEPTION_STUB 1
    EXCEPTION_STUB 2
    EXCEPTION_STUB 3
    EXCEPTION_STUB 4
    EXCEPTION_STUB 5
    EXCEPTION_STUB 6
    EXCEPTION_STUB 7
    EXCEPTION_STUB_WITH_ERROR_CODE 8
    EXCEPTION_STUB 9
    EXCEPTION_STUB_WITH_ERROR_CODE 10
    EXCEPTION_STUB_WITH_ERROR_CODE 11
    EXCEPTION_STUB_WITH_ERROR_CODE 12
    EXCEPTION_STUB_WITH_ERROR_CODE 13
    EXCEPTION_STUB_WITH_ERROR_CODE 14
    EXCEPTION_STUB 15
    EXCEPTION_STUB 16
    EXCEPTION_STUB_WITH_ERROR_CODE 17
    EXCEPTION_STUB 18
    EXCEPTION_STUB 19
    EXCEPTION_STUB 20
    EXCEPTION_STUB_WITH_ERROR_CODE 21
    EXCEPTION_STUB 22
    EXCEPTION_STUB 23
    EXCEPTION_STUB 24
    EXCEPTION_STUB 25
    EXCEPTION_STUB 26
    EXCEPTION_STUB 27
    EXCEPTION_STUB 28
    EXCEPTION_STUB_WITH_ERROR_CODE 29
    EXCEPTION_STUB_WITH_ERROR_CODE 30
    EXCEPTION_STUB 31
    // Interrupts 0x20-0x31 (legacy PIC inputs 0-15, APIC timer, wake-up); the CPU pushes no
    // error code.
    .irp vector, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49
    EXCEPTION_STUB \vector
    .endr
    .balign 16                      // so the label is the stub, not the padding before it
    .global apic_spurious_stub
apic_spurious_stub:
    EXCEPTION_STUB 255

exception_common:
    pushq   %rax
    pushq   %rbx
    pushq   %rcx
    pushq   %rdx
    pushq   %rsi
    pushq   %rdi
    pushq   %rbp
    pushq   %r8
    pushq   %r9
    pushq   %r10
    pushq   %r11
    pushq   %r12
    pushq   %r13
    pushq   %r14
    pushq   %r15
    // The CPU aligned the stack to 16 bytes before pushing its 5-word frame; with the error
    // code, the vector and 15 registers (22 words) it is aligned again, as the call needs.
    movq    %rsp, %rdi
    cld
    call    {handler}
    popq    %r15
    popq    %r14
    popq    %r13
    popq    %r12
    popq    %r11
    popq    %r10
    popq    %r9
    popq    %r8
    popq    %rbp
    popq    %rdi
    popq    %rsi
    popq    %rdx
    popq    %rcx
    popq    %rbx
    popq    %rax
    addq    $16, %rsp               // vector and error code
    iretq
    .popsection
"#,
    handler = sym x86_64_exception,
    options(att_syntax)
);

/// Every vector the IDT can hold.
const VECTORS: usize = 256;
/// The vectors with a stub at `exception_stubs + vector * STUB_SIZE`: the exceptions 0-31 and
/// the interrupts up to the wake-up interrupt's.
const CONTIGUOUS_STUBS: usize = WAKEUP_VECTOR as usize + 1;
const _: () = assert!(TIMER_VECTOR < WAKEUP_VECTOR);
/// Vectors below this are the architecture's exceptions.
const FIRST_INTERRUPT_VECTOR: u64 = 32;
/// Every stub starts on a 16-byte boundary.
const STUB_SIZE: usize = 16;

/// Code segment selector of the boot GDT (`boot_gdt` in the entry code).
const KERNEL_CODE_SELECTOR: u16 = 0x08;
/// Present, DPL 0, 64-bit interrupt gate (interrupts stay masked in the handler).
const INTERRUPT_GATE: u8 = 0x8e;

/// Page-fault error code bit 4: the fault was an instruction fetch.
const PAGE_FAULT_INSTRUCTION_FETCH: u64 = 1 << 4;

unsafe extern "C" {
    /// Start of the stubs; not callable from Rust, only its address is used.
    fn exception_stubs();
    /// The stub for [`APIC_SPURIOUS_VECTOR`]; likewise only its address is used.
    fn apic_spurious_stub();
}

/// The registers saved by the stub and `exception_common`, lowest address first, followed by
/// what the CPU pushed.
#[repr(C)]
struct TrapFrame {
    r15: u64,
    r14: u64,
    r13: u64,
    r12: u64,
    r11: u64,
    r10: u64,
    r9: u64,
    r8: u64,
    rbp: u64,
    rdi: u64,
    rsi: u64,
    rdx: u64,
    rcx: u64,
    rbx: u64,
    rax: u64,
    vector: u64,
    error_code: u64,
    rip: u64,
    cs: u64,
    rflags: u64,
    rsp: u64,
    ss: u64,
}

const _: () = assert!(core::mem::size_of::<TrapFrame>() == 22 * 8);

/// A 64-bit IDT gate descriptor (SDM vol. 3A, figure 6-8).
#[derive(Clone, Copy)]
#[repr(C)]
struct GateDescriptor {
    offset_low: u16,
    selector: u16,
    ist: u8,
    attributes: u8,
    offset_middle: u16,
    offset_high: u32,
    reserved: u32,
}

const _: () = assert!(core::mem::size_of::<GateDescriptor>() == 16);

impl GateDescriptor {
    const MISSING: Self = Self {
        offset_low: 0,
        selector: 0,
        ist: 0,
        attributes: 0,
        offset_middle: 0,
        offset_high: 0,
        reserved: 0,
    };

    fn interrupt_gate(handler: u64) -> Self {
        Self {
            offset_low: handler as u16,
            selector: KERNEL_CODE_SELECTOR,
            ist: 0,
            attributes: INTERRUPT_GATE,
            offset_middle: (handler >> 16) as u16,
            offset_high: (handler >> 32) as u32,
            reserved: 0,
        }
    }
}

/// The operand of `lidt`.
#[repr(C, packed)]
struct DescriptorTablePointer {
    limit: u16,
    base: u64,
}

struct InterruptDescriptorTable(UnsafeCell<[GateDescriptor; VECTORS]>);

// SAFETY: the table is written only by `install`, which runs once on the boot CPU before any
// other code (and before any other CPU starts); afterwards only the CPU reads it.
unsafe impl Sync for InterruptDescriptorTable {}

static IDT: InterruptDescriptorTable =
    InterruptDescriptorTable(UnsafeCell::new([GateDescriptor::MISSING; VECTORS]));

/// Fills the IDT and loads it. Called once by the entry code, before `kernel_main`.
pub(super) extern "C" fn install() {
    fill();
    load();
}

/// Fills the IDT, before any CPU loads it.
fn fill() {
    let stubs = exception_stubs as *const () as u64;
    let gates = IDT.0.get();
    let stub_gates = (0..CONTIGUOUS_STUBS)
        .map(|vector| (vector, stubs + (vector * STUB_SIZE) as u64))
        .chain([(
            APIC_SPURIOUS_VECTOR.into(),
            apic_spurious_stub as *const () as u64,
        )]);
    // Vectors without a stub keep a not-present gate: delivering one raises #NP (reported).
    for (vector, stub) in stub_gates {
        // SAFETY: `fill` is the only writer and runs once, from `install`, before any CPU loads
        // the IDT, so nothing else accesses the table; `vector` is below 256, in bounds.
        unsafe { (*gates)[vector] = GateDescriptor::interrupt_gate(stub) };
    }
}

/// Loads the IDT on this CPU: the boot CPU's entry code once `fill` has run, and each secondary
/// CPU's (D40).
pub(super) extern "C" fn load() {
    let gates = IDT.0.get();
    let pointer = DescriptorTablePointer {
        limit: (core::mem::size_of::<[GateDescriptor; VECTORS]>() - 1) as u16,
        base: gates as u64,
    };
    // SAFETY: the IDT is a static, filled by `fill` before any CPU loads it, whose gates point
    // at the stubs in this module with the boot GDT's code selector.
    unsafe {
        core::arch::asm!("lidt [{}]", in(reg) &pointer, options(readonly, nostack, preserves_flags));
    }
}

/// Called by `exception_common` with the saved registers.
extern "C" fn x86_64_exception(frame: &mut TrapFrame) {
    match frame.vector {
        vector if vector == u64::from(TIMER_VECTOR) => return timer::interrupt(),
        // The exceptions, without the rarer interrupts' checks.
        vector if vector < FIRST_INTERRUPT_VECTOR => {}
        vector if vector == u64::from(WAKEUP_VECTOR) => return timer::wakeup_interrupt(),
        // Spurious: the APIC needs no end of interrupt for it.
        vector if vector == u64::from(APIC_SPURIOUS_VECTOR) => return,
        vector if LEGACY_PIC_VECTORS.contains(&vector) && timer::legacy_pic_interrupt(vector) => {
            return;
        }
        _ => {}
    }
    let fault_address = if frame.vector == 14 {
        let cr2: u64;
        // SAFETY: reading CR2 at ring 0 has no side effects.
        unsafe {
            core::arch::asm!("mov {}, cr2", out(reg) cr2, options(nomem, nostack, preserves_flags))
        };
        Some(cr2)
    } else {
        None
    };
    // `int3` is a trap: the saved `rip` already points after it, so resuming needs no fix-up.
    deliver(&decode(
        frame.vector,
        frame.error_code,
        frame.rip,
        fault_address,
    ));
}

fn decode(vector: u64, error_code: u64, pc: u64, address: Option<u64>) -> Exception {
    let (kind, name) = match vector {
        0 => (ExceptionKind::Other, "#DE divide error"),
        1 => (ExceptionKind::Other, "#DB debug"),
        2 => (ExceptionKind::Interrupt, "NMI"),
        3 => (ExceptionKind::Breakpoint, "#BP breakpoint"),
        4 => (ExceptionKind::Other, "#OF overflow"),
        5 => (ExceptionKind::Other, "#BR bound range exceeded"),
        6 => (ExceptionKind::IllegalInstruction, "#UD invalid opcode"),
        7 => (ExceptionKind::Other, "#NM device not available"),
        8 => (ExceptionKind::Other, "#DF double fault"),
        10 => (ExceptionKind::Other, "#TS invalid TSS"),
        11 => (ExceptionKind::Other, "#NP segment not present"),
        12 => (ExceptionKind::Other, "#SS stack-segment fault"),
        13 => (ExceptionKind::Other, "#GP general protection"),
        14 if error_code & PAGE_FAULT_INSTRUCTION_FETCH != 0 => {
            (ExceptionKind::InstructionFault, "#PF page fault")
        }
        14 => (ExceptionKind::DataFault, "#PF page fault"),
        16 => (ExceptionKind::Other, "#MF x87 floating-point error"),
        17 => (ExceptionKind::Alignment, "#AC alignment check"),
        18 => (ExceptionKind::Other, "#MC machine check"),
        19 => (ExceptionKind::Other, "#XM SIMD floating-point exception"),
        20 => (ExceptionKind::Other, "#VE virtualization exception"),
        21 => (ExceptionKind::Other, "#CP control protection"),
        28 => (ExceptionKind::Other, "#HV hypervisor injection"),
        29 => (ExceptionKind::Other, "#VC VMM communication"),
        30 => (ExceptionKind::Other, "#SX security exception"),
        32.. => (ExceptionKind::Interrupt, "unexpected interrupt"),
        _ => (ExceptionKind::Other, "reserved exception vector"),
    };
    Exception {
        kind,
        name,
        code: error_code,
        pc,
        address,
    }
}
