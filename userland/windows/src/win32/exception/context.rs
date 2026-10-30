//! Windows' `CONTEXT` on each CPU, at the offsets of `winnt.h` (x64: 1232 bytes; ARM64: 912),
//! and the assembly that captures a caller's registers into one (`RtlCaptureContext`, and the
//! entries of `RaiseException` and its kin, which capture and then dispatch) and that resumes a
//! thread at one (`RtlRestoreContext`). On ARM64 `x16` carries the address the thread resumes
//! at, so its value in the `CONTEXT` is not restored (it is a scratch register between calls).
//! A fault's `CONTEXT` is filled from the kernel's signal frame.

#[cfg(target_arch = "aarch64")]
pub use crate::unwind::arm64 as unwinder;
#[cfg(target_arch = "x86_64")]
pub use crate::unwind::x64 as unwinder;

pub use unwinder::Registers;

#[cfg(target_arch = "x86_64")]
mod arch {
    use super::Registers;
    use crate::unwind::x64::register;

    pub const CONTEXT_FULL: u32 = 0x0010_000b;
    pub const SIZE: usize = 0x4d0;
    const UCONTEXT_GREGS: usize = 40;
    const UCONTEXT_FPSTATE: usize = 224;
    const FXSAVE_MXCSR: usize = 24;
    const FXSAVE_XMM: usize = 160;
    const LINUX_ORDER: [usize; 16] = [13, 14, 12, 11, 15, 10, 9, 8, 0, 1, 2, 3, 4, 5, 6, 7];
    const GREG_RIP: usize = 16;
    const GREG_EFLAGS: usize = 17;

    /// x64's `CONTEXT`.
    #[repr(C, align(16))]
    #[derive(Clone, Copy)]
    pub struct Context {
        pub home: [u64; 6],
        pub flags: u32,
        pub mx_csr: u32,
        pub segments: [u16; 6],
        pub e_flags: u32,
        pub debug: [u64; 6],
        pub general: [u64; 16],
        pub rip: u64,
        pub float_header: [u8; 160],
        pub xmm: [[u64; 2]; 16],
        pub float_reserved: [u8; 96],
        pub vector: [[u64; 2]; 26],
        pub vector_control: u64,
        pub debug_control: u64,
        pub last_branches: [u64; 4],
    }

    const _: () = assert!(core::mem::size_of::<Context>() == SIZE);
    const _: () = assert!(core::mem::offset_of!(Context, general) == 0x78);
    const _: () = assert!(core::mem::offset_of!(Context, rip) == 0xf8);
    const _: () = assert!(core::mem::offset_of!(Context, xmm) == 0x1a0);

    impl Context {
        pub fn pc(&self) -> u64 {
            self.rip
        }

        pub fn set_pc(&mut self, pc: u64) {
            self.rip = pc;
        }

        pub fn sp(&self) -> u64 {
            self.general[register::RSP]
        }

        /// Sets the register a function returns its value in (`rax`).
        pub fn set_return_value(&mut self, value: u64) {
            self.general[register::RAX] = value;
        }

        pub fn registers(&self) -> Registers {
            Registers {
                general: self.general,
                rip: self.rip,
                xmm: self.xmm,
            }
        }

        pub fn set_registers(&mut self, registers: &Registers) {
            self.general = registers.general;
            self.rip = registers.rip;
            self.xmm = registers.xmm;
        }

        /// The registers the kernel saved in a signal frame's `ucontext_t`.
        ///
        /// # Safety
        /// `ucontext` must be the `ucontext_t` the kernel passed to a signal handler.
        pub unsafe fn from_signal(ucontext: *const u8) -> Self {
            let mut context = Self::zeroed();
            let greg = |index: usize| {
                ucontext
                    .wrapping_add(UCONTEXT_GREGS + index * 8)
                    .cast::<u64>()
            };
            for (windows, linux) in LINUX_ORDER.iter().enumerate() {
                // SAFETY: the caller's frame holds the 23 general register words.
                context.general[windows] = unsafe { greg(*linux).read_unaligned() };
            }
            // SAFETY: as above.
            context.rip = unsafe { greg(GREG_RIP).read_unaligned() };
            // SAFETY: as above.
            context.e_flags = unsafe { greg(GREG_EFLAGS).read_unaligned() } as u32;
            // SAFETY: as above; `fpstate` points at the frame's `FXSAVE` image, or is null.
            let fpstate = unsafe {
                ucontext
                    .wrapping_add(UCONTEXT_FPSTATE)
                    .cast::<u64>()
                    .read_unaligned()
            };
            if fpstate != 0 {
                let image = fpstate as *const u8;
                // SAFETY: an `FXSAVE` image holds `mxcsr` at 24 and the sixteen `xmm` at 160.
                context.mx_csr = unsafe { image.add(FXSAVE_MXCSR).cast::<u32>().read_unaligned() };
                for (index, xmm) in context.xmm.iter_mut().enumerate() {
                    let at = image.wrapping_add(FXSAVE_XMM + index * 16).cast::<u64>();
                    // SAFETY: as above.
                    *xmm = unsafe { [at.read_unaligned(), at.add(1).read_unaligned()] };
                }
            }
            context.flags = CONTEXT_FULL;
            context
        }
    }

    /// `RtlCaptureContext`: fills the `CONTEXT` with the caller's registers as they are when
    /// the call returns.
    ///
    /// # Safety
    /// `_context` must point at a writable `CONTEXT`.
    #[unsafe(naked)]
    pub unsafe extern "efiapi" fn capture(_context: *mut Context) {
        core::arch::naked_asm!(
            "mov [rcx + 0x78], rax",
            "mov [rcx + 0x80], rcx",
            "mov [rcx + 0x88], rdx",
            "mov [rcx + 0x90], rbx",
            "mov [rcx + 0xa0], rbp",
            "mov [rcx + 0xa8], rsi",
            "mov [rcx + 0xb0], rdi",
            "mov [rcx + 0xb8], r8",
            "mov [rcx + 0xc0], r9",
            "mov [rcx + 0xc8], r10",
            "mov [rcx + 0xd0], r11",
            "mov [rcx + 0xd8], r12",
            "mov [rcx + 0xe0], r13",
            "mov [rcx + 0xe8], r14",
            "mov [rcx + 0xf0], r15",
            "lea rax, [rsp + 8]",
            "mov [rcx + 0x98], rax",
            "mov rax, [rsp]",
            "mov [rcx + 0xf8], rax",
            "movups [rcx + 0x1a0], xmm0",
            "movups [rcx + 0x1b0], xmm1",
            "movups [rcx + 0x1c0], xmm2",
            "movups [rcx + 0x1d0], xmm3",
            "movups [rcx + 0x1e0], xmm4",
            "movups [rcx + 0x1f0], xmm5",
            "movups [rcx + 0x200], xmm6",
            "movups [rcx + 0x210], xmm7",
            "movups [rcx + 0x220], xmm8",
            "movups [rcx + 0x230], xmm9",
            "movups [rcx + 0x240], xmm10",
            "movups [rcx + 0x250], xmm11",
            "movups [rcx + 0x260], xmm12",
            "movups [rcx + 0x270], xmm13",
            "movups [rcx + 0x280], xmm14",
            "movups [rcx + 0x290], xmm15",
            "stmxcsr [rcx + 0x34]",
            "pushfq",
            "pop rax",
            "mov [rcx + 0x44], eax",
            "mov dword ptr [rcx + 0x30], {full}",
            "mov rax, [rcx + 0x78]",
            "ret",
            full = const CONTEXT_FULL,
        )
    }

    /// `RtlRestoreContext`: resumes the thread with the `CONTEXT`'s registers.
    ///
    /// # Safety
    /// `_context` must hold registers the thread can run with: a stack and a code address of
    /// its own, with the registers that code expects.
    #[unsafe(naked)]
    pub unsafe extern "efiapi" fn restore(_context: *const Context, _record: usize) -> ! {
        core::arch::naked_asm!(
            "movups xmm0, [rcx + 0x1a0]",
            "movups xmm1, [rcx + 0x1b0]",
            "movups xmm2, [rcx + 0x1c0]",
            "movups xmm3, [rcx + 0x1d0]",
            "movups xmm4, [rcx + 0x1e0]",
            "movups xmm5, [rcx + 0x1f0]",
            "movups xmm6, [rcx + 0x200]",
            "movups xmm7, [rcx + 0x210]",
            "movups xmm8, [rcx + 0x220]",
            "movups xmm9, [rcx + 0x230]",
            "movups xmm10, [rcx + 0x240]",
            "movups xmm11, [rcx + 0x250]",
            "movups xmm12, [rcx + 0x260]",
            "movups xmm13, [rcx + 0x270]",
            "movups xmm14, [rcx + 0x280]",
            "movups xmm15, [rcx + 0x290]",
            "ldmxcsr [rcx + 0x34]",
            "mov r11, [rcx + 0x98]",
            "sub r11, 24",
            "mov r10, [rcx + 0xf8]",
            "mov [r11 + 16], r10",
            "mov r10, [rcx + 0x80]",
            "mov [r11 + 8], r10",
            "mov r10d, dword ptr [rcx + 0x44]",
            "and r10d, 0xcd5",
            "mov [r11], r10",
            "mov rsp, r11",
            "mov rax, [rcx + 0x78]",
            "mov rdx, [rcx + 0x88]",
            "mov rbx, [rcx + 0x90]",
            "mov rbp, [rcx + 0xa0]",
            "mov rsi, [rcx + 0xa8]",
            "mov rdi, [rcx + 0xb0]",
            "mov r8, [rcx + 0xb8]",
            "mov r9, [rcx + 0xc0]",
            "mov r10, [rcx + 0xc8]",
            "mov r11, [rcx + 0xd0]",
            "mov r12, [rcx + 0xd8]",
            "mov r13, [rcx + 0xe0]",
            "mov r14, [rcx + 0xe8]",
            "mov r15, [rcx + 0xf0]",
            "popfq",
            "pop rcx",
            "ret",
        )
    }

    /// Defines `$entry`, a function the program calls with Windows' convention, which
    /// captures its caller's registers into a `CONTEXT` on the stack (as `RtlCaptureContext`
    /// would, as the caller sees them after the call returns) and calls `$target` with the
    /// entry's first four arguments and that `CONTEXT`; `$target` never returns.
    macro_rules! capturing_entry {
        ($visibility:vis $entry:ident => $target:path) => {
            #[unsafe(naked)]
            $visibility unsafe extern "efiapi" fn $entry() {
                core::arch::naked_asm!(
                    "sub rsp, 1240",
                    "mov [rsp + 0x78], rax",
                    "mov [rsp + 0x80], rcx",
                    "mov [rsp + 0x88], rdx",
                    "mov [rsp + 0x90], rbx",
                    "mov [rsp + 0xa0], rbp",
                    "mov [rsp + 0xa8], rsi",
                    "mov [rsp + 0xb0], rdi",
                    "mov [rsp + 0xb8], r8",
                    "mov [rsp + 0xc0], r9",
                    "mov [rsp + 0xc8], r10",
                    "mov [rsp + 0xd0], r11",
                    "mov [rsp + 0xd8], r12",
                    "mov [rsp + 0xe0], r13",
                    "mov [rsp + 0xe8], r14",
                    "mov [rsp + 0xf0], r15",
                    "lea rax, [rsp + 1248]",
                    "mov [rsp + 0x98], rax",
                    "mov rax, [rsp + 1240]",
                    "mov [rsp + 0xf8], rax",
                    "movups [rsp + 0x1a0], xmm0",
                    "movups [rsp + 0x1b0], xmm1",
                    "movups [rsp + 0x1c0], xmm2",
                    "movups [rsp + 0x1d0], xmm3",
                    "movups [rsp + 0x1e0], xmm4",
                    "movups [rsp + 0x1f0], xmm5",
                    "movups [rsp + 0x200], xmm6",
                    "movups [rsp + 0x210], xmm7",
                    "movups [rsp + 0x220], xmm8",
                    "movups [rsp + 0x230], xmm9",
                    "movups [rsp + 0x240], xmm10",
                    "movups [rsp + 0x250], xmm11",
                    "movups [rsp + 0x260], xmm12",
                    "movups [rsp + 0x270], xmm13",
                    "movups [rsp + 0x280], xmm14",
                    "movups [rsp + 0x290], xmm15",
                    "stmxcsr [rsp + 0x34]",
                    "pushfq",
                    "pop rax",
                    "mov [rsp + 0x44], eax",
                    "mov dword ptr [rsp + 0x30], 0x10000b",
                    "mov rdi, rcx",
                    "mov rsi, rdx",
                    "mov rdx, r8",
                    "mov rcx, r9",
                    "mov r8, rsp",
                    "call {target}",
                    "ud2",
                    target = sym $target,
                )
            }
        };
    }

    pub(in crate::win32::exception) use capturing_entry;

    /// Where a fault's dispatch starts once the signal handler returns: the handler pointed
    /// the thread here with the record, the `CONTEXT` and the signal in the first three
    /// argument registers and `rsp` 16-byte aligned below them.
    ///
    /// # Safety
    /// Only a signal handler's rewritten frame enters it.
    #[unsafe(naked)]
    pub unsafe extern "C" fn fault_entry() {
        core::arch::naked_asm!(
            "call {dispatch}",
            "ud2",
            dispatch = sym super::super::dispatch::dispatch_fault,
        )
    }

    /// The `ucontext_t` offsets of `rsp`, `rip` and the first three argument registers.
    pub const UCONTEXT_SP: usize = UCONTEXT_GREGS + 15 * 8;
    pub const UCONTEXT_PC: usize = UCONTEXT_GREGS + GREG_RIP * 8;
    pub const UCONTEXT_ARGUMENTS: [usize; 3] = [
        UCONTEXT_GREGS + 8 * 8,
        UCONTEXT_GREGS + 9 * 8,
        UCONTEXT_GREGS + 12 * 8,
    ];
}

#[cfg(target_arch = "aarch64")]
mod arch {
    use super::Registers;

    pub const CONTEXT_FULL: u32 = 0x0040_0007;
    pub const SIZE: usize = 0x390;
    const UCONTEXT_REGS: usize = 184;
    const UCONTEXT_FAULT_SP: usize = 432;
    const UCONTEXT_FAULT_PC: usize = 440;
    const UCONTEXT_PSTATE: usize = 448;
    const UCONTEXT_FPSIMD: usize = 464;
    const FPSIMD_MAGIC: u32 = 0x4650_8001;
    const FIRST_SAVED_FLOAT: usize = 8;

    /// ARM64's `CONTEXT`.
    #[repr(C, align(16))]
    #[derive(Clone, Copy)]
    pub struct Context {
        pub flags: u32,
        pub cpsr: u32,
        pub x: [u64; 31],
        pub sp: u64,
        pub pc: u64,
        pub v: [[u64; 2]; 32],
        pub fpcr: u32,
        pub fpsr: u32,
        pub breakpoint_control: [u32; 8],
        pub breakpoint_value: [u64; 8],
        pub watchpoint_control: [u32; 2],
        pub watchpoint_value: [u64; 2],
    }

    const _: () = assert!(core::mem::size_of::<Context>() == SIZE);
    const _: () = assert!(core::mem::offset_of!(Context, sp) == 0x100);
    const _: () = assert!(core::mem::offset_of!(Context, v) == 0x110);
    const _: () = assert!(core::mem::offset_of!(Context, fpcr) == 0x310);

    impl Context {
        pub fn pc(&self) -> u64 {
            self.pc
        }

        pub fn set_pc(&mut self, pc: u64) {
            self.pc = pc;
        }

        pub fn sp(&self) -> u64 {
            self.sp
        }

        /// Sets the register a function returns its value in (`x0`).
        pub fn set_return_value(&mut self, value: u64) {
            self.x[0] = value;
        }

        pub fn registers(&self) -> Registers {
            let mut d = [0; 8];
            for (index, value) in d.iter_mut().enumerate() {
                *value = self.v[FIRST_SAVED_FLOAT + index][0];
            }
            Registers {
                x: self.x,
                sp: self.sp,
                pc: self.pc,
                d,
            }
        }

        pub fn set_registers(&mut self, registers: &Registers) {
            self.x = registers.x;
            self.sp = registers.sp;
            self.pc = registers.pc;
            for (index, value) in registers.d.iter().enumerate() {
                self.v[FIRST_SAVED_FLOAT + index][0] = *value;
            }
        }

        /// The registers the kernel saved in a signal frame's `ucontext_t`.
        ///
        /// # Safety
        /// `ucontext` must be the `ucontext_t` the kernel passed to a signal handler.
        pub unsafe fn from_signal(ucontext: *const u8) -> Self {
            let mut context = Self::zeroed();
            let word = |offset: usize| ucontext.wrapping_add(offset).cast::<u64>();
            for (index, x) in context.x.iter_mut().enumerate() {
                // SAFETY: the caller's frame holds `x0` to `x30`.
                *x = unsafe { word(UCONTEXT_REGS + index * 8).read_unaligned() };
            }
            // SAFETY: as above, then `sp`, `pc` and `pstate`.
            unsafe {
                context.sp = word(UCONTEXT_FAULT_SP).read_unaligned();
                context.pc = word(UCONTEXT_FAULT_PC).read_unaligned();
                context.cpsr = word(UCONTEXT_PSTATE).read_unaligned() as u32;
            }
            let record = ucontext.wrapping_add(UCONTEXT_FPSIMD);
            // SAFETY: the first record after `pstate` is the kernel's `fpsimd_context`, whose
            // magic number says so: then `fpsr`, `fpcr` and `v0` to `v31`.
            unsafe {
                if record.cast::<u32>().read_unaligned() == FPSIMD_MAGIC {
                    context.fpsr = record.add(8).cast::<u32>().read_unaligned();
                    context.fpcr = record.add(12).cast::<u32>().read_unaligned();
                    for (index, v) in context.v.iter_mut().enumerate() {
                        let at = record.add(16 + index * 16).cast::<u64>();
                        *v = [at.read_unaligned(), at.add(1).read_unaligned()];
                    }
                }
            }
            context.flags = CONTEXT_FULL;
            context
        }
    }

    /// `RtlCaptureContext`: fills the `CONTEXT` with the caller's registers as they are when
    /// the call returns.
    ///
    /// # Safety
    /// `_context` must point at a writable `CONTEXT`.
    #[unsafe(naked)]
    pub unsafe extern "efiapi" fn capture(_context: *mut Context) {
        core::arch::naked_asm!(
            "stp x0, x1, [x0, #0x08]",
            "stp x2, x3, [x0, #0x18]",
            "stp x4, x5, [x0, #0x28]",
            "stp x6, x7, [x0, #0x38]",
            "stp x8, x9, [x0, #0x48]",
            "stp x10, x11, [x0, #0x58]",
            "stp x12, x13, [x0, #0x68]",
            "stp x14, x15, [x0, #0x78]",
            "stp x16, x17, [x0, #0x88]",
            "stp x18, x19, [x0, #0x98]",
            "stp x20, x21, [x0, #0xa8]",
            "stp x22, x23, [x0, #0xb8]",
            "stp x24, x25, [x0, #0xc8]",
            "stp x26, x27, [x0, #0xd8]",
            "stp x28, x29, [x0, #0xe8]",
            "str x30, [x0, #0xf8]",
            "mov x9, sp",
            "str x9, [x0, #0x100]",
            "str x30, [x0, #0x108]",
            "stp q0, q1, [x0, #0x110]",
            "stp q2, q3, [x0, #0x130]",
            "stp q4, q5, [x0, #0x150]",
            "stp q6, q7, [x0, #0x170]",
            "stp q8, q9, [x0, #0x190]",
            "stp q10, q11, [x0, #0x1b0]",
            "stp q12, q13, [x0, #0x1d0]",
            "stp q14, q15, [x0, #0x1f0]",
            "stp q16, q17, [x0, #0x210]",
            "stp q18, q19, [x0, #0x230]",
            "stp q20, q21, [x0, #0x250]",
            "stp q22, q23, [x0, #0x270]",
            "stp q24, q25, [x0, #0x290]",
            "stp q26, q27, [x0, #0x2b0]",
            "stp q28, q29, [x0, #0x2d0]",
            "stp q30, q31, [x0, #0x2f0]",
            "mrs x9, fpcr",
            "str w9, [x0, #0x310]",
            "mrs x9, fpsr",
            "str w9, [x0, #0x314]",
            "mrs x9, nzcv",
            "str w9, [x0, #4]",
            "mov w9, #7",
            "movk w9, #0x40, lsl #16",
            "str w9, [x0]",
            "ldr x9, [x0, #0x50]",
            "ret",
        )
    }

    /// `RtlRestoreContext`: resumes the thread with the `CONTEXT`'s registers.
    ///
    /// # Safety
    /// `_context` must hold registers the thread can run with: a stack and a code address of
    /// its own, with the registers that code expects.
    #[unsafe(naked)]
    pub unsafe extern "efiapi" fn restore(_context: *const Context, _record: usize) -> ! {
        core::arch::naked_asm!(
            "ldp q0, q1, [x0, #0x110]",
            "ldp q2, q3, [x0, #0x130]",
            "ldp q4, q5, [x0, #0x150]",
            "ldp q6, q7, [x0, #0x170]",
            "ldp q8, q9, [x0, #0x190]",
            "ldp q10, q11, [x0, #0x1b0]",
            "ldp q12, q13, [x0, #0x1d0]",
            "ldp q14, q15, [x0, #0x1f0]",
            "ldp q16, q17, [x0, #0x210]",
            "ldp q18, q19, [x0, #0x230]",
            "ldp q20, q21, [x0, #0x250]",
            "ldp q22, q23, [x0, #0x270]",
            "ldp q24, q25, [x0, #0x290]",
            "ldp q26, q27, [x0, #0x2b0]",
            "ldp q28, q29, [x0, #0x2d0]",
            "ldp q30, q31, [x0, #0x2f0]",
            "ldr w1, [x0, #0x310]",
            "msr fpcr, x1",
            "ldr w1, [x0, #0x314]",
            "msr fpsr, x1",
            "ldr w1, [x0, #4]",
            "msr nzcv, x1",
            "ldr x1, [x0, #0x100]",
            "mov sp, x1",
            "ldp x2, x3, [x0, #0x18]",
            "ldp x4, x5, [x0, #0x28]",
            "ldp x6, x7, [x0, #0x38]",
            "ldp x8, x9, [x0, #0x48]",
            "ldp x10, x11, [x0, #0x58]",
            "ldp x12, x13, [x0, #0x68]",
            "ldp x14, x15, [x0, #0x78]",
            "ldp x16, x17, [x0, #0x88]",
            "ldp x18, x19, [x0, #0x98]",
            "ldp x20, x21, [x0, #0xa8]",
            "ldp x22, x23, [x0, #0xb8]",
            "ldp x24, x25, [x0, #0xc8]",
            "ldp x26, x27, [x0, #0xd8]",
            "ldp x28, x29, [x0, #0xe8]",
            "ldr x30, [x0, #0xf8]",
            "ldr x16, [x0, #0x108]",
            "ldp x0, x1, [x0, #0x08]",
            "br x16",
        )
    }

    /// Defines `$entry`, a function the program calls with Windows' convention, which
    /// captures its caller's registers (`x18`, the TEB, included) into a `CONTEXT` on the stack
    /// (as the caller sees them after the call returns) and calls `$target` with the entry's
    /// first four arguments and that `CONTEXT`; `$target` never returns.
    macro_rules! capturing_entry {
        ($visibility:vis $entry:ident => $target:path) => {
            #[unsafe(naked)]
            $visibility unsafe extern "efiapi" fn $entry() {
                core::arch::naked_asm!(
                    "sub sp, sp, #0x390",
                    "stp x0, x1, [sp, #0x08]",
                    "stp x2, x3, [sp, #0x18]",
                    "stp x4, x5, [sp, #0x28]",
                    "stp x6, x7, [sp, #0x38]",
                    "stp x8, x9, [sp, #0x48]",
                    "stp x10, x11, [sp, #0x58]",
                    "stp x12, x13, [sp, #0x68]",
                    "stp x14, x15, [sp, #0x78]",
                    "stp x16, x17, [sp, #0x88]",
                    "stp x18, x19, [sp, #0x98]",
                    "stp x20, x21, [sp, #0xa8]",
                    "stp x22, x23, [sp, #0xb8]",
                    "stp x24, x25, [sp, #0xc8]",
                    "stp x26, x27, [sp, #0xd8]",
                    "stp x28, x29, [sp, #0xe8]",
                    "str x30, [sp, #0xf8]",
                    "add x9, sp, #0x390",
                    "str x9, [sp, #0x100]",
                    "str x30, [sp, #0x108]",
                    "stp q0, q1, [sp, #0x110]",
                    "stp q2, q3, [sp, #0x130]",
                    "stp q4, q5, [sp, #0x150]",
                    "stp q6, q7, [sp, #0x170]",
                    "stp q8, q9, [sp, #0x190]",
                    "stp q10, q11, [sp, #0x1b0]",
                    "stp q12, q13, [sp, #0x1d0]",
                    "stp q14, q15, [sp, #0x1f0]",
                    "stp q16, q17, [sp, #0x210]",
                    "stp q18, q19, [sp, #0x230]",
                    "stp q20, q21, [sp, #0x250]",
                    "stp q22, q23, [sp, #0x270]",
                    "stp q24, q25, [sp, #0x290]",
                    "stp q26, q27, [sp, #0x2b0]",
                    "stp q28, q29, [sp, #0x2d0]",
                    "stp q30, q31, [sp, #0x2f0]",
                    "mrs x9, fpcr",
                    "str w9, [sp, #0x310]",
                    "mrs x9, fpsr",
                    "str w9, [sp, #0x314]",
                    "mrs x9, nzcv",
                    "str w9, [sp, #4]",
                    "mov w9, #7",
                    "movk w9, #0x40, lsl #16",
                    "str w9, [sp]",
                    "mov x4, sp",
                    "bl {target}",
                    "brk #1",
                    target = sym $target,
                )
            }
        };
    }

    pub(in crate::win32::exception) use capturing_entry;

    /// Where a fault's dispatch starts once the signal handler returns: the handler pointed
    /// the thread here with the record, the `CONTEXT` and the signal in `x0` to `x2` and `sp`
    /// 16-byte aligned below them.
    ///
    /// # Safety
    /// Only a signal handler's rewritten frame enters it.
    #[unsafe(naked)]
    pub unsafe extern "C" fn fault_entry() {
        core::arch::naked_asm!(
            "bl {dispatch}",
            "brk #1",
            dispatch = sym super::super::dispatch::dispatch_fault,
        )
    }

    /// The `ucontext_t` offsets of `sp`, `pc` and the first three argument registers.
    pub const UCONTEXT_SP: usize = UCONTEXT_FAULT_SP;
    pub const UCONTEXT_PC: usize = UCONTEXT_FAULT_PC;
    pub const UCONTEXT_ARGUMENTS: [usize; 3] =
        [UCONTEXT_REGS, UCONTEXT_REGS + 8, UCONTEXT_REGS + 16];
}

pub(super) use arch::capturing_entry;
pub use arch::{
    Context, UCONTEXT_ARGUMENTS, UCONTEXT_PC, UCONTEXT_SP, capture, fault_entry, restore,
};

impl Context {
    /// A `CONTEXT` of zeros.
    pub fn zeroed() -> Self {
        // SAFETY: every field is an integer or an array of integers, for which zero is valid.
        unsafe { core::mem::zeroed() }
    }
}

const _: () = assert!(core::mem::size_of::<Context>() == arch::SIZE);
