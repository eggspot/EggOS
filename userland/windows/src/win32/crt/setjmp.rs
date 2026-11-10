//! `setjmp` and `longjmp` (D229) on Microsoft's `_JUMP_BUFFER` layouts (`setjmp.h`): `_setjmp`
//! and `_setjmpex` save the caller's non-volatile registers, stack, return address and
//! floating-point control, and the establisher frame MSVC passes as a hidden second argument.
//! `longjmp` with a frame unwinds to it through the layer's unwinder with a `STATUS_LONGJMP`
//! record carrying the buffer, so the `__finally` blocks and C++ destructors in between run,
//! then resumes with the buffer's registers and the value (0 becomes 1). A buffer without a
//! frame, or with one no frame on the stack has, is resumed directly.

use super::super::exception::{
    self, Context, ExceptionRecord, NO_STATE, UnwindTarget, capturing_entry, status,
};
use crate::library::Export;

#[cfg(target_arch = "x86_64")]
mod arch {
    use super::Context;

    const RBX: usize = 3;
    const RSP: usize = 4;
    const RBP: usize = 5;
    const RSI: usize = 6;
    const RDI: usize = 7;
    const R12: usize = 12;
    const FIRST_SAVED_XMM: usize = 6;

    /// x64's `_JUMP_BUFFER`.
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct JumpBuffer {
        pub frame: u64,
        rbx: u64,
        rsp: u64,
        rbp: u64,
        rsi: u64,
        rdi: u64,
        r12_to_r15: [u64; 4],
        rip: u64,
        mx_csr: u32,
        fp_csr: u16,
        _spare: u16,
        xmm6_to_xmm15: [[u64; 2]; 10],
    }

    const _: () = assert!(core::mem::size_of::<JumpBuffer>() == 256);
    const _: () = assert!(core::mem::offset_of!(JumpBuffer, rip) == 80);
    const _: () = assert!(core::mem::offset_of!(JumpBuffer, mx_csr) == 88);
    const _: () = assert!(core::mem::offset_of!(JumpBuffer, fp_csr) == 92);
    const _: () = assert!(core::mem::offset_of!(JumpBuffer, xmm6_to_xmm15) == 96);

    impl JumpBuffer {
        pub fn resume_address(&self) -> u64 {
            self.rip
        }

        /// Puts the saved registers into `context`.
        pub fn restore_into(&self, context: &mut Context) {
            context.general[RBX] = self.rbx;
            context.general[RSP] = self.rsp;
            context.general[RBP] = self.rbp;
            context.general[RSI] = self.rsi;
            context.general[RDI] = self.rdi;
            context.general[R12..R12 + 4].copy_from_slice(&self.r12_to_r15);
            context.rip = self.rip;
            context.mx_csr = self.mx_csr;
            context.xmm[FIRST_SAVED_XMM..FIRST_SAVED_XMM + 10].copy_from_slice(&self.xmm6_to_xmm15);
        }

        /// Loads the saved x87 control word, which no code of the layer changes.
        pub fn restore_x87_control(&self) {
            let control = self.fp_csr;
            // SAFETY: loads a control word the thread ran with, which only changes how later
            // x87 instructions round and trap.
            unsafe {
                core::arch::asm!(
                    "fldcw word ptr [{control}]",
                    control = in(reg) &raw const control,
                    options(nostack, readonly),
                )
            };
        }
    }

    /// `_setjmp`: fills the `_JUMP_BUFFER` at `rcx` with the caller's registers and the frame
    /// in `rdx`, and returns 0.
    ///
    /// # Safety
    /// `rcx` must be a writable 256-byte jump buffer.
    #[unsafe(naked)]
    pub unsafe extern "efiapi" fn set_jump() {
        core::arch::naked_asm!(
            "mov [rcx], rdx",
            "mov [rcx + 8], rbx",
            "lea rax, [rsp + 8]",
            "mov [rcx + 16], rax",
            "mov [rcx + 24], rbp",
            "mov [rcx + 32], rsi",
            "mov [rcx + 40], rdi",
            "mov [rcx + 48], r12",
            "mov [rcx + 56], r13",
            "mov [rcx + 64], r14",
            "mov [rcx + 72], r15",
            "mov rax, [rsp]",
            "mov [rcx + 80], rax",
            "stmxcsr [rcx + 88]",
            "fnstcw [rcx + 92]",
            "mov word ptr [rcx + 94], 0",
            "movups [rcx + 96], xmm6",
            "movups [rcx + 112], xmm7",
            "movups [rcx + 128], xmm8",
            "movups [rcx + 144], xmm9",
            "movups [rcx + 160], xmm10",
            "movups [rcx + 176], xmm11",
            "movups [rcx + 192], xmm12",
            "movups [rcx + 208], xmm13",
            "movups [rcx + 224], xmm14",
            "movups [rcx + 240], xmm15",
            "xor eax, eax",
            "ret",
        )
    }
}

#[cfg(target_arch = "aarch64")]
mod arch {
    use super::Context;

    const X19: usize = 19;
    const FP: usize = 29;
    const LR: usize = 30;
    const FIRST_SAVED_FLOAT: usize = 8;

    /// ARM64's `_JUMP_BUFFER`.
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct JumpBuffer {
        pub frame: u64,
        _reserved: u64,
        x19_to_x28: [u64; 10],
        fp: u64,
        lr: u64,
        sp: u64,
        fpcr: u32,
        fpsr: u32,
        d8_to_d15: [u64; 8],
    }

    const _: () = assert!(core::mem::size_of::<JumpBuffer>() == 192);
    const _: () = assert!(core::mem::offset_of!(JumpBuffer, fp) == 96);
    const _: () = assert!(core::mem::offset_of!(JumpBuffer, sp) == 112);
    const _: () = assert!(core::mem::offset_of!(JumpBuffer, d8_to_d15) == 128);

    impl JumpBuffer {
        pub fn resume_address(&self) -> u64 {
            self.lr
        }

        /// Puts the saved registers into `context`.
        pub fn restore_into(&self, context: &mut Context) {
            context.x[X19..X19 + 10].copy_from_slice(&self.x19_to_x28);
            context.x[FP] = self.fp;
            context.x[LR] = self.lr;
            context.sp = self.sp;
            context.pc = self.lr;
            context.fpcr = self.fpcr;
            context.fpsr = self.fpsr;
            for (index, value) in self.d8_to_d15.iter().enumerate() {
                context.v[FIRST_SAVED_FLOAT + index] = [*value, 0];
            }
        }

        pub fn restore_x87_control(&self) {}
    }

    /// `_setjmp`: fills the `_JUMP_BUFFER` at `x0` with the caller's registers and the frame
    /// in `x1`, and returns 0.
    ///
    /// # Safety
    /// `x0` must be a writable 192-byte jump buffer.
    #[unsafe(naked)]
    pub unsafe extern "efiapi" fn set_jump() {
        core::arch::naked_asm!(
            "str x1, [x0]",
            "str xzr, [x0, #8]",
            "stp x19, x20, [x0, #16]",
            "stp x21, x22, [x0, #32]",
            "stp x23, x24, [x0, #48]",
            "stp x25, x26, [x0, #64]",
            "stp x27, x28, [x0, #80]",
            "stp x29, x30, [x0, #96]",
            "mov x2, sp",
            "str x2, [x0, #112]",
            "mrs x2, fpcr",
            "str w2, [x0, #120]",
            "mrs x2, fpsr",
            "str w2, [x0, #124]",
            "stp d8, d9, [x0, #128]",
            "stp d10, d11, [x0, #144]",
            "stp d12, d13, [x0, #160]",
            "stp d14, d15, [x0, #176]",
            "mov w0, #0",
            "ret",
        )
    }
}

use arch::JumpBuffer;

capturing_entry!(long_jump_entry => long_jump);

extern "C" fn long_jump(buffer: u64, value: u64, _: u64, _: u64, context: *mut Context) -> ! {
    // SAFETY: `longjmp`'s contract: a buffer `_setjmp` filled in a frame still on the stack.
    let jump = unsafe { (buffer as *const JumpBuffer).read_unaligned() };
    let value = match value as u32 {
        0 => 1,
        value => value,
    };
    // SAFETY: the entry's `CONTEXT`.
    let start = unsafe { *context };
    let mut reached = match jump.frame {
        0 => start,
        frame if !exception::reaches_frame(&start, frame) => start,
        frame => {
            let mut record = ExceptionRecord::new(status::LONGJMP, 0, start.pc(), &[buffer]);
            let target = UnwindTarget {
                frame,
                ip: jump.resume_address(),
                state: NO_STATE,
            };
            exception::unwind(&start, true, &target, &raw mut record)
        }
    };
    jump.restore_into(&mut reached);
    reached.set_return_value(u64::from(value));
    jump.restore_x87_control();
    exception::resume(&reached)
}

/// `setjmp.h`'s functions, in both runtimes.
pub(super) const EXPORTS: &[Export] = exports![
    b"_setjmp" => arch::set_jump [direct],
    b"_setjmpex" => arch::set_jump [direct],
    b"longjmp" => long_jump_entry [direct],
];

/// The names vcruntime140 gives them: MSVC compiles `setjmp` to a call of
/// `__intrinsic_setjmp` (x64) or `__intrinsic_setjmpex` (ARM64), and imports `longjmp` from it.
pub(super) const VCRUNTIME_EXPORTS: &[Export] = exports![
    b"__intrinsic_setjmp" => arch::set_jump [direct],
    b"__intrinsic_setjmpex" => arch::set_jump [direct],
    b"longjmp" => long_jump_entry [direct],
];
