//! CPU exceptions, described the same way on every architecture.
//!
//! Each architecture module installs its exception vectors before the kernel starts, decodes
//! every exception into an [`Exception`] and passes it to the kernel's `kernel_exception`
//! function. If that function returns, the interrupted code resumes: after the breakpoint
//! instruction for a [`ExceptionKind::Breakpoint`], otherwise at the same instruction.

/// What kind of exception happened, coarse enough for the kernel to decide what to do with it
/// without knowing the architecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExceptionKind {
    /// A breakpoint instruction (`brk`, `ebreak`, `int3`).
    Breakpoint,
    /// An undefined, illegal or privileged instruction.
    IllegalInstruction,
    /// Fetching an instruction failed (page fault, access fault or abort).
    InstructionFault,
    /// A load or store failed (page fault, access fault or abort).
    DataFault,
    /// A misaligned instruction, data or stack access.
    Alignment,
    /// An arithmetic error the CPU traps: integer division by zero or overflow (x86_64's
    /// `#DE`), or an unmasked floating-point exception (`#MF`, `#XM`).
    Arithmetic,
    /// A debug exception: a hardware breakpoint or watchpoint, a single step, or x86_64's
    /// `int1`.
    Debug,
    /// A segment check failed: a segment not present, or a stack access outside its segment,
    /// such as a push with a non-canonical stack pointer (x86_64's `#NP` and `#SS`).
    Segment,
    /// An interrupt arrived that the kernel does not handle yet.
    Interrupt,
    /// Anything else; [`Exception::name`] says what.
    Other,
}

/// One exception, as decoded by the architecture module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exception {
    pub kind: ExceptionKind,
    /// The architecture's own name for it, e.g. `"data abort"` or `"#GP general protection"`.
    pub name: &'static str,
    /// The architecture's raw cause value: `ESR_EL1` on AArch64, `scause` on RISC-V, the
    /// error code pushed by the CPU on x86_64 (0 when it pushes none). For an unexpected
    /// interrupt from an interrupt controller: its number there (GIC interrupt ID, x86
    /// vector).
    pub code: u64,
    /// Address of the instruction that was interrupted or faulted.
    pub pc: u64,
    /// The faulting memory address, for faults that report one.
    pub address: Option<u64>,
}

// The kernel binary defines `kernel_exception` with exactly this signature (Rust ABI), which is
// what makes declaring it `safe` sound.
#[cfg(target_os = "none")]
unsafe extern "Rust" {
    /// Defined by the kernel binary. Returns to resume the interrupted code, or never returns.
    safe fn kernel_exception(exception: &Exception);
}

/// Hands `exception` to the kernel. Returns if the kernel wants the interrupted code resumed.
#[cfg(target_os = "none")]
pub(crate) fn deliver(exception: &Exception) {
    kernel_exception(exception);
}
