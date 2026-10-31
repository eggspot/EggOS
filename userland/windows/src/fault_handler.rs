//! The layer's handler for a Windows program's hardware faults (D151, D177). On aarch64 it
//! completes a read of `CurrentEL`, which user code cannot make, as reading exception level 0,
//! so MSVC's atomics helpers go on as their own handler lets them on Windows. Any other fault
//! the CPU raised in the program's code, or in code the program made, becomes the Windows
//! exception its cause stands for (`fault::delivery`), which the program's vectored and frame
//! handlers get (`win32::deliver_fault`); `__fastfail` ends the program at once. A fault in the
//! layer's own code, or one no handler took, is written to standard error with its address as
//! the program's module and offset, and ends the process by the signal's default action. A
//! trap or fault in the translated code of an x64 program on ARM64 (D191) ends the program with
//! a line that names its guest address, and the Windows status of its cause.

use crate::fault::{self, Delivery, Fault, Image, Signalled};
use crate::linux;
use crate::win32;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

const STANDARD_ERROR: usize = 2;
const SIGINFO_CODE: usize = 8;
const SIGINFO_ADDRESS: usize = 16;
#[cfg(target_arch = "aarch64")]
const UCONTEXT_REGISTERS: usize = 184;
#[cfg(target_arch = "aarch64")]
const UCONTEXT_PC: usize = 440;
#[cfg(target_arch = "aarch64")]
const UCONTEXT_ESR_RECORD: usize = 992;
#[cfg(target_arch = "aarch64")]
const ESR_MAGIC: u32 = 0x4553_5201;
#[cfg(target_arch = "x86_64")]
const UCONTEXT_PC: usize = 168;
#[cfg(target_arch = "x86_64")]
const UCONTEXT_ERR: usize = 192;
#[cfg(target_arch = "x86_64")]
const UCONTEXT_TRAPNO: usize = 200;
#[cfg(target_arch = "x86_64")]
const PAGE_FAULT: u64 = 14;
#[cfg(target_arch = "aarch64")]
const INSTRUCTION_SIZE: u64 = 4;
const SIGNAL_STATUS_BASE: u32 = 128;

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

fn in_image(pc: u64) -> bool {
    let image = image();
    pc.checked_sub(image.base)
        .is_some_and(|offset| offset < image.size)
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

#[cfg(target_arch = "x86_64")]
fn access(context: *mut u8) -> u64 {
    // SAFETY: the kernel's `ucontext_t` for a fault holds `err` and `trapno`.
    let error = unsafe { word(context, UCONTEXT_ERR).read_unaligned() };
    // SAFETY: as above.
    let trap = unsafe { word(context, UCONTEXT_TRAPNO).read_unaligned() };
    match trap {
        PAGE_FAULT if error & 0x10 != 0 => fault::access::EXECUTE,
        PAGE_FAULT if error & 0x02 != 0 => fault::access::WRITE,
        _ => fault::access::READ,
    }
}

#[cfg(target_arch = "aarch64")]
fn access(context: *mut u8) -> u64 {
    let record = context.wrapping_add(UCONTEXT_ESR_RECORD);
    // SAFETY: the `ucontext_t`'s reserved area, after the FP/SIMD record, holds the fault's
    // `esr_context` when its magic number is there.
    let magic = unsafe { record.cast::<u32>().read_unaligned() };
    if magic != ESR_MAGIC {
        return fault::access::READ;
    }
    // SAFETY: as above; the syndrome follows the record's header.
    let syndrome = unsafe { record.wrapping_add(8).cast::<u64>().read_unaligned() };
    match syndrome >> 26 & 0x3f {
        0x20 | 0x21 => fault::access::EXECUTE,
        0x24 | 0x25 if syndrome & 1 << 6 != 0 => fault::access::WRITE,
        _ => fault::access::READ,
    }
}

fn dispatches(pc: u64) -> bool {
    in_image(pc) || !win32::in_layer_code(pc)
}

#[cfg(target_arch = "aarch64")]
fn completed_misaligned_access(pc: u64, context: *mut u8) -> bool {
    let registers = word(context, UCONTEXT_REGISTERS).cast::<[u64; 32]>();
    // SAFETY: the kernel's `ucontext_t` for this signal holds `x0` to `x30` and then `sp`,
    // 8-byte aligned, which `rt_sigreturn` loads back; nothing else uses them meanwhile.
    let registers = unsafe { &mut *registers };
    // SAFETY: they are the faulting thread's registers at `pc`.
    if !unsafe { crate::translation::complete_misaligned_access(pc, registers) } {
        return false;
    }
    // SAFETY: as above, for the `pc` it holds: the program resumes at the next instruction.
    unsafe { word(context, UCONTEXT_PC).write(pc + INSTRUCTION_SIZE) };
    true
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
    #[cfg(target_arch = "aarch64")]
    if from_the_processor && signal == fault::SIGBUS && completed_misaligned_access(pc, context) {
        return;
    }
    #[cfg(target_arch = "aarch64")]
    if from_the_processor {
        // SAFETY: the `ucontext_t` holds `x0` to `x30`; `x16` is the ninth pair's first.
        let x16 = unsafe { word(context, UCONTEXT_REGISTERS + 16 * 8).read() };
        if let Some((cause, guest)) = crate::translation::guest_fault(signal, pc, x16) {
            end_translated(signal, cause, guest);
        }
    }
    if from_the_processor && signal == fault::SIGILL && completed_current_el_read(pc, context) {
        return;
    }
    if from_the_processor && dispatches(pc) {
        let fetching = matches!(signal, fault::SIGSEGV | fault::SIGBUS) && address == pc;
        let instruction = match fetching {
            true => 0,
            // SAFETY: the instruction at `pc` ran (or was decoded), so its bytes are mapped
            // and readable, as the program's code and the code it maps are.
            false => unsafe { (pc as *const u32).read_unaligned() },
        };
        let byte_before = match signal == fault::SIGTRAP && cfg!(target_arch = "x86_64") {
            // SAFETY: as above: an `int3` that trapped ended just before `pc`.
            true => unsafe { (pc.wrapping_sub(1) as *const u8).read() },
            false => 0,
        };
        let signalled = Signalled {
            signal,
            code,
            pc,
            address,
            access: access(context),
            instruction,
            byte_before,
        };
        match fault::delivery(&signalled, cfg!(target_arch = "aarch64")) {
            Delivery::Exception(exception) => {
                win32::deliver_fault(context, &exception, signal);
                return;
            }
            Delivery::FailFast => end_fail_fast(pc),
        }
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

/// Ends the process for a fault no handler took: the line that names it, then the signal's
/// default action, as for a fault the layer does not dispatch.
pub fn end_with_fault(signal: i32, pc: u64, address: u64) -> ! {
    let fault = Fault {
        signal,
        pc,
        address,
    };
    let line = fault::report(program(), image(), fault);
    let _ = linux::write_all(STANDARD_ERROR, line.as_bytes());
    let _ = linux::default_signal_action(signal);
    let _ = linux::signal_this_thread(signal);
    linux::exit(SIGNAL_STATUS_BASE + signal as u32)
}

/// Ends the process for an exception the program raised and no handler took, with the
/// exception's code as its status, as Windows does.
pub fn end_with_exception(code: u32, pc: u64) -> ! {
    let line = fault::report_exception(program(), image(), code, pc);
    let _ = linux::write_all(STANDARD_ERROR, line.as_bytes());
    linux::exit(code)
}

fn end_fail_fast(pc: u64) -> ! {
    let line = fault::report_fail_fast(program(), image(), pc);
    let _ = linux::write_all(STANDARD_ERROR, line.as_bytes());
    linux::exit(fault::status::STACK_BUFFER_OVERRUN)
}

#[cfg(target_arch = "aarch64")]
fn end_translated(signal: i32, cause: Option<crate::translated::TrapCause>, guest: u64) -> ! {
    let line = crate::translated::report(program(), image(), cause, guest);
    let _ = linux::write_all(STANDARD_ERROR, line.as_bytes());
    let status = cause.map_or(
        crate::translated::fault_status(signal),
        crate::translated::trap_status,
    );
    linux::exit(status)
}
