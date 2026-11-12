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
//! every call into the program loads `x18` from there first. A variadic export (the old C
//! runtime's `printf` family, D189) is an entry marked `variadic_after_one`, `_two` or
//! `_three`: on both CPUs Windows passes variadic arguments in the integer registers, then on
//! the stack, so the entry stores the registers next to the caller's stack arguments and gives
//! the function, after its fixed arguments, a pointer to the first variadic one: a `va_list`.

#[cfg(target_arch = "x86_64")]
macro_rules! export_entry {
    ($function:path, variadic_after_one) => {{
        #[unsafe(naked)]
        unsafe extern "C" fn entry() {
            core::arch::naked_asm!(
                "mov [rsp + 8], rcx",
                "mov [rsp + 16], rdx",
                "mov [rsp + 24], r8",
                "mov [rsp + 32], r9",
                "sub rsp, 40",
                "lea rdx, [rsp + 56]",
                "call {function}",
                "add rsp, 40",
                "ret",
                function = sym $function,
            )
        }
        entry as *const ()
    }};
    ($function:path, variadic_after_two) => {{
        #[unsafe(naked)]
        unsafe extern "C" fn entry() {
            core::arch::naked_asm!(
                "mov [rsp + 8], rcx",
                "mov [rsp + 16], rdx",
                "mov [rsp + 24], r8",
                "mov [rsp + 32], r9",
                "sub rsp, 40",
                "lea r8, [rsp + 64]",
                "call {function}",
                "add rsp, 40",
                "ret",
                function = sym $function,
            )
        }
        entry as *const ()
    }};
    ($function:path, variadic_after_three) => {{
        #[unsafe(naked)]
        unsafe extern "C" fn entry() {
            core::arch::naked_asm!(
                "mov [rsp + 8], rcx",
                "mov [rsp + 16], rdx",
                "mov [rsp + 24], r8",
                "mov [rsp + 32], r9",
                "sub rsp, 40",
                "lea r9, [rsp + 72]",
                "call {function}",
                "add rsp, 40",
                "ret",
                function = sym $function,
            )
        }
        entry as *const ()
    }};
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
    ($function:path, variadic_after_one) => {{
        #[unsafe(naked)]
        unsafe extern "C" fn entry() {
            core::arch::naked_asm!(
                "sub sp, sp, #80",
                "stp x18, x30, [sp]",
                "stp x0, x1, [sp, #16]",
                "stp x2, x3, [sp, #32]",
                "stp x4, x5, [sp, #48]",
                "stp x6, x7, [sp, #64]",
                "add x1, sp, #24",
                "bl {function}",
                "ldp x18, x30, [sp]",
                "add sp, sp, #80",
                "ret",
                function = sym $function,
            )
        }
        entry as *const ()
    }};
    ($function:path, variadic_after_two) => {{
        #[unsafe(naked)]
        unsafe extern "C" fn entry() {
            core::arch::naked_asm!(
                "sub sp, sp, #80",
                "stp x18, x30, [sp]",
                "stp x0, x1, [sp, #16]",
                "stp x2, x3, [sp, #32]",
                "stp x4, x5, [sp, #48]",
                "stp x6, x7, [sp, #64]",
                "add x2, sp, #32",
                "bl {function}",
                "ldp x18, x30, [sp]",
                "add sp, sp, #80",
                "ret",
                function = sym $function,
            )
        }
        entry as *const ()
    }};
    ($function:path, variadic_after_three) => {{
        #[unsafe(naked)]
        unsafe extern "C" fn entry() {
            core::arch::naked_asm!(
                "sub sp, sp, #80",
                "stp x18, x30, [sp]",
                "stp x0, x1, [sp, #16]",
                "stp x2, x3, [sp, #32]",
                "stp x4, x5, [sp, #48]",
                "stp x6, x7, [sp, #64]",
                "add x3, sp, #40",
                "bl {function}",
                "ldp x18, x30, [sp]",
                "add sp, sp, #80",
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
    let address = crate::translation::native_procedure(address).unwrap_or(address);
    // SAFETY: the caller's duty, as for a native call; an x64 program's function on ARM64 runs
    // through the translation's entry trampoline (D191).
    if let Some(result) = unsafe { crate::translation::call_guest(address, arguments) } {
        return result;
    }
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

/// Calls the program's function at `address` with nine integer or pointer `arguments`, as
/// Windows calls it on this CPU (the four or eight in registers, the rest on the stack), and
/// returns its result register.
///
/// # Safety
/// `address` must be a function of the program that takes at most nine integer or pointer
/// arguments, called with values it accepts, on a thread whose TEB register is set.
#[cfg(target_arch = "x86_64")]
pub unsafe fn call_program_nine(address: usize, arguments: [usize; 9]) -> Option<usize> {
    type Nine =
        extern "win64" fn(usize, usize, usize, usize, usize, usize, usize, usize, usize) -> usize;
    // SAFETY: the caller's duty; under the Microsoft x64 convention a function ignores the
    // arguments it does not take, and the caller removes the stack's.
    let function: Nine = unsafe { core::mem::transmute(address) };
    let [a, b, c, d, e, f, g, h, i] = arguments;
    Some(function(a, b, c, d, e, f, g, h, i))
}

/// Calls the program's function at `address` with nine integer or pointer `arguments`, eight
/// in `x0` to `x7` and the ninth on the stack, with `x18` holding the thread's TEB; `None` for
/// an x64 program's function on ARM64, whose translation takes at most four.
///
/// # Safety
/// `address` must be a function of the program that takes at most nine integer or pointer
/// arguments, called with values it accepts, on a thread whose TEB register is set.
#[cfg(target_arch = "aarch64")]
pub unsafe fn call_program_nine(address: usize, arguments: [usize; 9]) -> Option<usize> {
    let address = crate::translation::native_procedure(address).unwrap_or(address);
    if crate::translation::holds_guest_code(address) {
        return None;
    }
    let result: usize;
    // SAFETY: the caller's duty; the stack slot the callee reads is this call's own, 16-byte
    // aligned, and given back after the call; `x18` is declared clobbered.
    unsafe {
        core::arch::asm!(
            "sub sp, sp, #16",
            "str {ninth}, [sp]",
            "mrs x18, tpidr_el0",
            "blr {address}",
            "add sp, sp, #16",
            address = in(reg) address,
            ninth = in(reg) arguments[8],
            inlateout("x0") arguments[0] => result,
            in("x1") arguments[1],
            in("x2") arguments[2],
            in("x3") arguments[3],
            in("x4") arguments[4],
            in("x5") arguments[5],
            in("x6") arguments[6],
            in("x7") arguments[7],
            out("x18") _,
            clobber_abi("C"),
        )
    };
    Some(result)
}
