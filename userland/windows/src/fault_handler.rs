//! The layer's handler for a Windows program's hardware faults (D151). On aarch64 it completes
//! a read of `CurrentEL`, which user code cannot make, as reading exception level 0, so MSVC's
//! atomics helpers go on as their own handler lets them on Windows. Any other fault is written
//! to standard error with its address as the program's module and offset, then ends the
//! process by the signal's default action, as before.

use crate::fault::{self, Fault, Image};
use crate::linux;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

const STANDARD_ERROR: usize = 2;
const SIGINFO_CODE: usize = 8;
const SIGINFO_ADDRESS: usize = 16;
#[cfg(target_arch = "aarch64")]
const UCONTEXT_REGISTERS: usize = 184;
#[cfg(target_arch = "aarch64")]
const UCONTEXT_PC: usize = 440;
#[cfg(target_arch = "x86_64")]
const UCONTEXT_PC: usize = 168;
#[cfg(target_arch = "aarch64")]
const INSTRUCTION_SIZE: u64 = 4;

static PROGRAM: AtomicUsize = AtomicUsize::new(0);
static PROGRAM_LENGTH: AtomicUsize = AtomicUsize::new(0);
static IMAGE_BASE: AtomicU64 = AtomicU64::new(0);
static IMAGE_SIZE: AtomicU64 = AtomicU64::new(0);

/// Handles the faults of every thread of `program` (its path, which lives as long as the
/// process), whose image is `image`.
pub fn install(program: &'static [u8], image: Image) {
    PROGRAM.store(program.as_ptr() as usize, Ordering::Relaxed);
    PROGRAM_LENGTH.store(program.len(), Ordering::Relaxed);
    IMAGE_BASE.store(image.base, Ordering::Relaxed);
    IMAGE_SIZE.store(image.size, Ordering::Release);
    for signal in fault::SIGNALS {
        let _ = linux::handle_signal(signal, on_fault);
    }
}

fn image() -> Image {
    Image {
        size: IMAGE_SIZE.load(Ordering::Acquire),
        base: IMAGE_BASE.load(Ordering::Relaxed),
    }
}

fn program() -> &'static [u8] {
    let length = PROGRAM_LENGTH.load(Ordering::Acquire);
    // SAFETY: `install` stored a `'static` slice's address and length before any handler ran.
    unsafe { core::slice::from_raw_parts(PROGRAM.load(Ordering::Relaxed) as *const u8, length) }
}

fn word(context: *mut u8, offset: usize) -> *mut u64 {
    context.wrapping_add(offset).cast::<u64>()
}

#[cfg(target_arch = "aarch64")]
fn completed_current_el_read(pc: u64, context: *mut u8) -> bool {
    let image = image();
    let inside = pc
        .checked_sub(image.base)
        .is_some_and(|offset| offset + INSTRUCTION_SIZE <= image.size);
    if !inside || !pc.is_multiple_of(INSTRUCTION_SIZE) {
        return false;
    }
    // SAFETY: `pc` is an aligned address inside the program's image, which stays mapped and
    // readable for the process's life.
    let instruction = unsafe { (pc as *const u32).read() };
    let Some(register) = fault::reads_current_exception_level(instruction) else {
        return false;
    };
    if register != fault::ZERO_REGISTER {
        let target = word(context, UCONTEXT_REGISTERS + register * 8);
        // SAFETY: the kernel's `ucontext_t` for this signal holds `x0` to `x30`, which
        // `rt_sigreturn` loads back.
        unsafe { target.write(fault::USER_EXCEPTION_LEVEL) };
    }
    let resume = word(context, UCONTEXT_PC);
    // SAFETY: as above, for the `pc` it holds: the program resumes at the next instruction.
    unsafe { resume.write(pc + INSTRUCTION_SIZE) };
    true
}

#[cfg(target_arch = "x86_64")]
fn completed_current_el_read(_pc: u64, _context: *mut u8) -> bool {
    false
}

extern "C" fn on_fault(signal: i32, info: *const u8, context: *mut u8) {
    // SAFETY: the kernel calls this handler (`SA_SIGINFO`) with its `siginfo_t`, whose
    // `si_code` is the third 32-bit word.
    let code = unsafe { info.wrapping_add(SIGINFO_CODE).cast::<i32>().read() };
    // SAFETY: as above; a fault's `si_addr` is the 64-bit word at 16.
    let address = unsafe { info.wrapping_add(SIGINFO_ADDRESS).cast::<u64>().read() };
    // SAFETY: and with its `ucontext_t` for the signal, which holds the interrupted `pc`.
    let pc = unsafe { word(context, UCONTEXT_PC).read() };
    let from_the_processor = code > 0;
    if from_the_processor && signal == fault::SIGILL && completed_current_el_read(pc, context) {
        return;
    }
    if from_the_processor {
        let fault = Fault {
            signal,
            pc,
            address,
        };
        let line = fault::report(program(), image(), fault);
        let _ = linux::write_all(STANDARD_ERROR, line.as_bytes());
    }
    let _ = linux::default_signal_action(signal);
    let _ = linux::signal_this_thread(signal);
}
