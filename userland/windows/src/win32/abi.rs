//! The boundary between the program's code and the layer's, per CPU (D124). The layer's
//! functions use Rust's `extern "efiapi"`, which is the Microsoft x64 convention on x86_64 and
//! the Arm64 procedure call standard on aarch64: Windows' own conventions there for functions
//! that are not variadic. On aarch64 Windows also keeps the TEB's address in `x18`, which the
//! program's code reads at any time and never writes, while the layer's code, built for Linux,
//! may use `x18` as a scratch register. So on aarch64 every export the program calls is a small
//! entry that saves the program's `x18` with the return address, calls the function and
//! restores both (one marked `two_stack_arguments`, `four_stack_arguments` or
//! `six_stack_arguments` copies that many of the caller's stack words for the function, whose
//! arguments past the eighth are there; `direct` exports are assembly that never writes `x18`,
//! called as they are); the layer keeps each thread's TEB in `tpidr_el0` for its own use, and
//! every call into the program loads `x18` from there first.

#[cfg(target_arch = "x86_64")]
macro_rules! export_entry {
    ($function:path $(, $entry_kind:ident)?) => {
        $function as *const ()
    };
}

#[cfg(target_arch = "aarch64")]
macro_rules! export_entry {
    ($function:path, direct) => {
        $function as *const ()
    };
    ($function:path) => {{
        #[unsafe(naked)]
        unsafe extern "C" fn entry() {
            core::arch::naked_asm!(
                "stp x18, x30, [sp, #-16]!",
                "bl {function}",
                "ldp x18, x30, [sp], #16",
                "ret",
                function = sym $function,
            )
        }
        entry as *const ()
    }};
    ($function:path, two_stack_arguments) => {{
        #[unsafe(naked)]
        unsafe extern "C" fn entry() {
            core::arch::naked_asm!(
                "sub sp, sp, #32",
                "stp x18, x30, [sp, #16]",
                "ldp x16, x17, [sp, #32]",
                "stp x16, x17, [sp]",
                "bl {function}",
                "ldp x18, x30, [sp, #16]",
                "add sp, sp, #32",
                "ret",
                function = sym $function,
            )
        }
        entry as *const ()
    }};
    ($function:path, four_stack_arguments) => {{
        #[unsafe(naked)]
        unsafe extern "C" fn entry() {
            core::arch::naked_asm!(
                "sub sp, sp, #48",
                "stp x18, x30, [sp, #32]",
                "ldp x16, x17, [sp, #48]",
                "stp x16, x17, [sp]",
                "ldp x16, x17, [sp, #64]",
                "stp x16, x17, [sp, #16]",
                "bl {function}",
                "ldp x18, x30, [sp, #32]",
                "add sp, sp, #48",
                "ret",
                function = sym $function,
            )
        }
        entry as *const ()
    }};
    ($function:path, six_stack_arguments) => {{
        #[unsafe(naked)]
        unsafe extern "C" fn entry() {
            core::arch::naked_asm!(
                "sub sp, sp, #64",
                "stp x18, x30, [sp, #48]",
                "ldp x16, x17, [sp, #64]",
                "stp x16, x17, [sp]",
                "ldp x16, x17, [sp, #80]",
                "stp x16, x17, [sp, #16]",
                "ldp x16, x17, [sp, #96]",
                "stp x16, x17, [sp, #32]",
                "bl {function}",
                "ldp x18, x30, [sp, #48]",
                "add sp, sp, #64",
                "ret",
                function = sym $function,
            )
        }
        entry as *const ()
    }};
}

/// Calls the program's function at `address` with `arguments` (a function that takes fewer
/// ignores the rest) as Windows calls it on this CPU, and returns its result register: for a
/// function that returns a 32-bit value, only the low half is defined.
///
/// # Safety
/// `address` must be a function of the program that takes at most four integer or pointer
/// arguments, called with values it accepts, on a thread whose TEB register is set.
#[cfg(target_arch = "x86_64")]
pub unsafe fn call_program(address: usize, arguments: [usize; 4]) -> usize {
    // SAFETY: the caller's duty; under the Microsoft x64 convention a function ignores the
    // argument registers it does not take, and the caller removes nothing from the stack.
    let function: extern "win64" fn(usize, usize, usize, usize) -> usize =
        unsafe { core::mem::transmute(address) };
    function(arguments[0], arguments[1], arguments[2], arguments[3])
}

/// Calls the program's function at `address` with `arguments` (a function that takes fewer
/// ignores the rest) as Windows calls it on this CPU, with `x18` holding the thread's TEB, and
/// returns its result register: for a function that returns a 32-bit value, only the low half
/// is defined.
///
/// # Safety
/// `address` must be a function of the program that takes at most four integer or pointer
/// arguments, called with values it accepts, on a thread whose TEB register is set.
#[cfg(target_arch = "aarch64")]
pub unsafe fn call_program(address: usize, arguments: [usize; 4]) -> usize {
    let result: usize;
    // SAFETY: the caller's duty; the program's function keeps the registers the Arm64
    // procedure call standard says a callee keeps, and `x18` is declared clobbered.
    unsafe {
        core::arch::asm!(
            "mrs x18, tpidr_el0",
            "blr {address}",
            address = in(reg) address,
            inlateout("x0") arguments[0] => result,
            in("x1") arguments[1],
            in("x2") arguments[2],
            in("x3") arguments[3],
            out("x18") _,
            clobber_abi("C"),
        )
    };
    result
}
