//! The program's entry point, `_start`, and its panic handler.

use crate::relocation;
use crate::startup::{Startup, auxv, segment};
use crate::syscall::{self, number};
use core::arch::global_asm;

unsafe extern "Rust" {
    safe fn eggos_main(startup: &Startup) -> i32;
}

#[cfg(target_arch = "x86_64")]
global_asm!(
    ".globl _start",
    ".type _start, @function",
    "_start:",
    "xor ebp, ebp",
    "mov rdi, rsp",
    "lea rsi, [rip + _DYNAMIC]",
    "and rsp, -16",
    "call {start}",
    "ud2",
    start = sym start,
);

#[cfg(target_arch = "aarch64")]
global_asm!(
    ".globl _start",
    ".type _start, %function",
    "_start:",
    "mov x29, xzr",
    "mov x30, xzr",
    "mov x0, sp",
    "adrp x1, _DYNAMIC",
    "add x1, x1, :lo12:_DYNAMIC",
    "bl {start}",
    "brk #0",
    start = sym start,
);

#[cfg(target_arch = "riscv64")]
global_asm!(
    ".globl _start",
    ".type _start, @function",
    "_start:",
    "mv s0, zero",
    "mv ra, zero",
    "mv a0, sp",
    "lla a1, _DYNAMIC",
    "andi sp, sp, -16",
    "call {start}",
    "unimp",
    start = sym start,
);

unsafe extern "C" fn start(stack: *const usize, dynamic: *const [usize; 2]) -> ! {
    // SAFETY: `_start` passes the stack as Linux laid it out, which stays for good.
    let startup = unsafe { Startup::from_stack(stack) };
    let Some(bias) = startup.load_bias(dynamic as usize) else {
        fail(b"eggos-runtime: the program has no PT_DYNAMIC header\n")
    };
    // SAFETY: `dynamic` is the program's dynamic section, linked with it, so its relocations
    // name the program's own writable words, at `bias` from their linked addresses; nothing
    // has used them yet.
    if unsafe { relocation::relocate(bias, dynamic) }.is_err() {
        fail(b"eggos-runtime: the program has relocations the runtime cannot apply\n")
    }
    if !protect_relocated_data(&startup, bias) {
        fail(b"eggos-runtime: the program's relocated data could not be made read-only\n")
    }
    syscall::exit(eggos_main(&startup))
}

fn protect_relocated_data(startup: &Startup, bias: usize) -> bool {
    const PROT_READ: usize = 1;
    let page = startup
        .auxiliary_value(auxv::PAGESZ)
        .filter(|page| page.is_power_of_two())
        .unwrap_or(4096);
    startup
        .program_headers()
        .iter()
        .filter(|header| header.kind == segment::GNU_RELRO)
        .all(|header| {
            let start = bias.wrapping_add(header.virt as usize) & !(page - 1);
            let end = bias.wrapping_add((header.virt + header.memory_size) as usize) & !(page - 1);
            // SAFETY: the range holds only the program's relocated data, which it only reads
            // from now on.
            end <= start
                || unsafe {
                    syscall::call(number::MPROTECT, [start, end - start, PROT_READ, 0, 0, 0])
                } == 0
        })
}

fn fail(message: &[u8]) -> ! {
    syscall::write_all(2, message);
    syscall::exit(127)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    syscall::write_all(2, b"eggos-runtime: the program panicked\n");
    syscall::exit(101)
}

#[allow(non_snake_case)]
#[unsafe(no_mangle)]
extern "C" fn _Unwind_Resume(_: *mut u8) -> ! {
    fail(b"eggos-runtime: the program tried to unwind\n")
}

#[unsafe(no_mangle)]
extern "C" fn rust_eh_personality() {}
