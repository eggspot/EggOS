//! What the layer does with a hardware fault in a Windows program (D151, D177), as pure
//! logic: the one instruction it completes itself, the Windows exception each fault becomes
//! (D177: an access violation, a division by zero, an illegal instruction, a breakpoint, ...),
//! and the line it writes when the program does not handle it and ends.

#[cfg(any(test, target_arch = "aarch64"))]
const READ_CURRENT_EL: u32 = 0xd538_4240;
#[cfg(any(test, target_arch = "aarch64"))]
const REGISTER_BITS: u32 = 0x1f;
/// The register number that names `xzr` in an `mrs`, whose result is thrown away.
#[cfg(any(test, target_arch = "aarch64"))]
pub const ZERO_REGISTER: usize = 31;
/// The exception level a program runs at, which `CurrentEL` holds in bits 2 and 3.
#[cfg(any(test, target_arch = "aarch64"))]
pub const USER_EXCEPTION_LEVEL: u64 = 0;

pub const SIGILL: i32 = 4;
pub const SIGTRAP: i32 = 5;
pub const SIGBUS: i32 = 7;
pub const SIGFPE: i32 = 8;
pub const SIGSEGV: i32 = 11;
const SI_KERNEL: i32 = 0x80;
const BUS_ADRALN: i32 = 1;
const TRAP_BRKPT: i32 = 1;
const INT3: u8 = 0xcc;
const INT_29: u32 = 0x29cd;
const ARM64_BRK_MASK: u32 = 0xffe0_001f;
const ARM64_BRK: u32 = 0xd420_0000;
const ARM64_BREAKPOINT: u32 = 0xf000;
const ARM64_ASSERTION: u32 = 0xf001;
const ARM64_DEBUG_SERVICE: u32 = 0xf002;
const ARM64_FAST_FAIL: u32 = 0xf003;
const ARM64_DIVIDE_BY_ZERO: u32 = 0xf004;

/// The Windows exception codes faults become (`ntstatus.h`).
pub mod status {
    pub const BREAKPOINT: u32 = 0x8000_0003;
    pub const DATATYPE_MISALIGNMENT: u32 = 0x8000_0002;
    pub const SINGLE_STEP: u32 = 0x8000_0004;
    pub const ACCESS_VIOLATION: u32 = 0xc000_0005;
    pub const IN_PAGE_ERROR: u32 = 0xc000_0006;
    pub const ILLEGAL_INSTRUCTION: u32 = 0xc000_001d;
    pub const FLOAT_DIVIDE_BY_ZERO: u32 = 0xc000_008e;
    pub const FLOAT_INEXACT_RESULT: u32 = 0xc000_008f;
    pub const FLOAT_INVALID_OPERATION: u32 = 0xc000_0090;
    pub const FLOAT_OVERFLOW: u32 = 0xc000_0091;
    pub const FLOAT_UNDERFLOW: u32 = 0xc000_0093;
    pub const INTEGER_DIVIDE_BY_ZERO: u32 = 0xc000_0094;
    pub const INTEGER_OVERFLOW: u32 = 0xc000_0095;
    pub const IO_DEVICE_ERROR: u32 = 0xc000_0185;
    pub const ASSERTION_FAILURE: u32 = 0xc000_0420;
    pub const STACK_BUFFER_OVERRUN: u32 = 0xc000_0409;
}

/// What an access violation's first parameter says the instruction did.
pub mod access {
    pub const READ: u64 = 0;
    pub const WRITE: u64 = 1;
    pub const EXECUTE: u64 = 8;
}

/// A fault as the kernel signalled it, with what the layer read around it: `access` (one of
/// [`access`]) from the trap information, the four bytes at `pc` and the byte before it (zero
/// where they were not read).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Signalled {
    pub signal: i32,
    pub code: i32,
    pub pc: u64,
    pub address: u64,
    pub access: u64,
    pub instruction: u32,
    pub byte_before: u8,
}

/// The Windows exception a fault becomes: its code, the address of the instruction and its
/// parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exception {
    pub code: u32,
    pub address: u64,
    pub parameters: [u64; 3],
    pub parameter_count: u32,
}

/// What a fault means for the program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// The program gets this exception, through its vectored and frame handlers.
    Exception(Exception),
    /// The program asked to end at once (`__fastfail`), which no handler sees.
    FailFast,
}

fn exception(code: u32, address: u64, parameters: &[u64]) -> Delivery {
    let mut all = [0; 3];
    all[..parameters.len()].copy_from_slice(parameters);
    Delivery::Exception(Exception {
        code,
        address,
        parameters: all,
        parameter_count: parameters.len() as u32,
    })
}

fn float_status(code: i32) -> u32 {
    match code {
        1 => status::INTEGER_DIVIDE_BY_ZERO,
        2 => status::INTEGER_OVERFLOW,
        3 => status::FLOAT_DIVIDE_BY_ZERO,
        4 => status::FLOAT_OVERFLOW,
        5 => status::FLOAT_UNDERFLOW,
        6 => status::FLOAT_INEXACT_RESULT,
        _ => status::FLOAT_INVALID_OPERATION,
    }
}

/// The exception `fault` becomes on Windows, on an ARM64 machine when `arm64`: memory faults
/// are access violations (a general protection fault, which names no address, reports
/// all ones), misaligned accesses misalignments, `SIGFPE`'s causes Windows' arithmetic codes,
/// illegal instructions `STATUS_ILLEGAL_INSTRUCTION`; `int3` and `brk #0xf000` breakpoints at
/// the instruction, and the `brk` immediates MSVC's ARM64 code uses for an assertion and a
/// division by zero those exceptions (any other `brk` is an illegal instruction, as on
/// Windows); `int 0x29` and `brk #0xf003` are `__fastfail`.
pub fn delivery(fault: &Signalled, arm64: bool) -> Delivery {
    let pc = fault.pc;
    match fault.signal {
        SIGSEGV if !arm64 && fault.code == SI_KERNEL && fault.instruction & 0xffff == INT_29 => {
            Delivery::FailFast
        }
        SIGSEGV if fault.code == SI_KERNEL => {
            exception(status::ACCESS_VIOLATION, pc, &[access::READ, u64::MAX])
        }
        SIGSEGV => exception(status::ACCESS_VIOLATION, pc, &[fault.access, fault.address]),
        SIGBUS if fault.code == BUS_ADRALN => exception(status::DATATYPE_MISALIGNMENT, pc, &[]),
        SIGBUS => exception(
            status::IN_PAGE_ERROR,
            pc,
            &[
                fault.access,
                fault.address,
                u64::from(status::IO_DEVICE_ERROR),
            ],
        ),
        SIGFPE => exception(float_status(fault.code), pc, &[]),
        SIGTRAP if arm64 && fault.instruction & ARM64_BRK_MASK == ARM64_BRK => {
            match fault.instruction >> 5 & 0xffff {
                ARM64_FAST_FAIL => Delivery::FailFast,
                ARM64_DIVIDE_BY_ZERO => exception(status::INTEGER_DIVIDE_BY_ZERO, pc, &[]),
                ARM64_ASSERTION => exception(status::ASSERTION_FAILURE, pc, &[]),
                ARM64_BREAKPOINT | ARM64_DEBUG_SERVICE => exception(status::BREAKPOINT, pc, &[0]),
                _ => exception(status::ILLEGAL_INSTRUCTION, pc, &[]),
            }
        }
        SIGTRAP if !arm64 && fault.code == SI_KERNEL && fault.byte_before == INT3 => {
            exception(status::BREAKPOINT, pc.wrapping_sub(1), &[0])
        }
        SIGTRAP if !arm64 && fault.code == TRAP_BRKPT => exception(status::SINGLE_STEP, pc, &[]),
        SIGTRAP => exception(status::BREAKPOINT, pc, &[0]),
        _ => exception(status::ILLEGAL_INSTRUCTION, pc, &[]),
    }
}

/// The faults the layer reports, each a signal the kernel sends for one.
pub const SIGNALS: [i32; 5] = [SIGILL, SIGTRAP, SIGBUS, SIGFPE, SIGSEGV];

/// The register an ARM64 `mrs <Xt>, CurrentEL` reads into, if `instruction` is one. User code
/// cannot read `CurrentEL`, so the processor refuses it as an undefined instruction; MSVC's
/// atomics helpers run it once to learn whether they run in the kernel, and their own handler
/// on Windows steps over it.
#[cfg(any(test, target_arch = "aarch64"))]
pub fn reads_current_exception_level(instruction: u32) -> Option<usize> {
    (instruction & !REGISTER_BITS == READ_CURRENT_EL)
        .then_some((instruction & REGISTER_BITS) as usize)
}

/// Where the program's image lies: its first address and its size in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Image {
    pub base: u64,
    pub size: u64,
}

/// A fault the kernel signalled: its signal, the instruction's address and, for a memory
/// fault, the address the instruction touched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fault {
    pub signal: i32,
    pub pc: u64,
    pub address: u64,
}

/// A line of text built without allocating, as a signal handler must.
pub struct Line {
    bytes: [u8; 512],
    length: usize,
}

impl Default for Line {
    fn default() -> Self {
        Self {
            bytes: [0; 512],
            length: 0,
        }
    }
}

impl Line {
    pub fn push(&mut self, text: &[u8]) {
        let room = self.bytes.len() - self.length;
        let taken = text.len().min(room);
        self.bytes[self.length..self.length + taken].copy_from_slice(&text[..taken]);
        self.length += taken;
    }

    pub fn push_hex(&mut self, value: u64) {
        let mut digits = [0u8; 16];
        let mut first = digits.len();
        let mut rest = value;
        loop {
            first -= 1;
            digits[first] = b"0123456789abcdef"[(rest & 0xf) as usize];
            rest >>= 4;
            if rest == 0 {
                break;
            }
        }
        self.push(b"0x");
        self.push(&digits[first..]);
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

fn what(signal: i32) -> &'static [u8] {
    match signal {
        SIGILL => b"illegal instruction",
        SIGTRAP => b"breakpoint",
        SIGBUS => b"bus error",
        SIGFPE => b"arithmetic exception",
        SIGSEGV => b"memory access violation",
        _ => b"fault",
    }
}

pub fn push_place(line: &mut Line, program: &[u8], image: Image, pc: u64) {
    match pc.checked_sub(image.base) {
        Some(offset) if offset < image.size => {
            line.push(b" (");
            line.push(file_name(program));
            line.push(b"+");
            line.push_hex(offset);
            line.push(b")");
        }
        _ => line.push(b" (outside the program's image)"),
    }
}

/// The line the layer writes to standard error when the exception `code`, raised at `pc`,
/// reached no handler, before the program ends with that code as its status.
pub fn report_exception(program: &[u8], image: Image, code: u32, pc: u64) -> Line {
    let mut line = Line::default();
    line.push(b"windows: ");
    line.push(program);
    line.push(b": exception ");
    line.push_hex(u64::from(code));
    line.push(b" was not handled, raised at ");
    line.push_hex(pc);
    push_place(&mut line, program, image, pc);
    line.push(b"\n");
    line
}

/// The line the layer writes to standard error when the program ended itself at once with
/// `__fastfail` at `pc`.
pub fn report_fail_fast(program: &[u8], image: Image, pc: u64) -> Line {
    let mut line = Line::default();
    line.push(b"windows: ");
    line.push(program);
    line.push(b": the program ended itself at once (__fastfail) at ");
    line.push_hex(pc);
    push_place(&mut line, program, image, pc);
    line.push(b"\n");
    line
}

fn file_name(path: &[u8]) -> &[u8] {
    path.rsplit(|byte| *byte == b'/' || *byte == b'\\')
        .next()
        .unwrap_or(path)
}

/// The line the layer writes to standard error for `fault` in `program` (its path), whose
/// image is `image`: what happened, where, as the program's module and offset when the
/// instruction is in its image, and the address a memory fault touched.
pub fn report(program: &[u8], image: Image, fault: Fault) -> Line {
    let mut line = Line::default();
    line.push(b"windows: ");
    line.push(program);
    line.push(b": ");
    line.push(what(fault.signal));
    line.push(b" at ");
    line.push_hex(fault.pc);
    push_place(&mut line, program, image, fault.pc);
    if matches!(fault.signal, SIGSEGV | SIGBUS) {
        line.push(b", address ");
        line.push_hex(fault.address);
    }
    line.push(b"\n");
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    const IMAGE: Image = Image {
        base: 0x1_4000_0000,
        size: 0x8000,
    };

    fn text(line: &Line) -> &str {
        core::str::from_utf8(line.as_bytes()).unwrap()
    }

    #[test]
    fn only_a_read_of_current_el_is_completed_and_its_register_is_named() {
        assert_eq!(reads_current_exception_level(0xd538_4250), Some(16));
        assert_eq!(reads_current_exception_level(0xd538_4240), Some(0));
        assert_eq!(
            reads_current_exception_level(0xd538_425f),
            Some(ZERO_REGISTER)
        );
        let id_aa64isar0 = 0xd538_0610;
        let msr_current_el = 0xd518_4250;
        let udf = 0;
        let brk_fastfail = 0xd43e_0060;
        for other in [id_aa64isar0, msr_current_el, udf, brk_fastfail] {
            assert_eq!(reads_current_exception_level(other), None);
        }
    }

    #[test]
    fn a_fault_in_the_image_names_the_module_and_offset() {
        let fault = Fault {
            signal: SIGILL,
            pc: 0x1_4000_115c,
            address: 0,
        };
        assert_eq!(
            text(&report(b"/bin/eggpad.exe", IMAGE, fault)),
            "windows: /bin/eggpad.exe: illegal instruction at 0x14000115c (eggpad.exe+0x115c)\n"
        );
        let windows_path = report(b"C:\\bin\\eggpad.exe", IMAGE, fault);
        assert!(text(&windows_path).contains("(eggpad.exe+0x115c)"));
    }

    #[test]
    fn a_memory_fault_names_the_address_and_one_outside_the_image_says_so() {
        let fault = Fault {
            signal: SIGSEGV,
            pc: 0x7f00_0000,
            address: 0x7ffe_0296,
        };
        assert_eq!(
            text(&report(b"eggpad.exe", IMAGE, fault)),
            "windows: eggpad.exe: memory access violation at 0x7f000000 \
             (outside the program's image), address 0x7ffe0296\n"
        );
        let end = Fault {
            signal: SIGTRAP,
            pc: IMAGE.base + IMAGE.size,
            address: 0,
        };
        assert!(text(&report(b"a.exe", IMAGE, end)).contains("breakpoint at 0x140008000 (outside"));
    }

    fn signalled(signal: i32, code: i32) -> Signalled {
        Signalled {
            signal,
            code,
            pc: 0x1_4000_2000,
            address: 0x10,
            access: access::WRITE,
            instruction: 0,
            byte_before: 0,
        }
    }

    fn code_of(delivery: Delivery) -> (u32, u64, Vec<u64>) {
        match delivery {
            Delivery::Exception(exception) => (
                exception.code,
                exception.address,
                exception.parameters[..exception.parameter_count as usize].to_vec(),
            ),
            Delivery::FailFast => (0, 0, Vec::new()),
        }
    }

    #[test]
    fn faults_become_the_windows_exceptions_of_their_cause() {
        let pc = 0x1_4000_2000;
        for arm64 in [false, true] {
            assert_eq!(
                code_of(delivery(&signalled(SIGSEGV, 1), arm64)),
                (status::ACCESS_VIOLATION, pc, vec![access::WRITE, 0x10])
            );
            assert_eq!(
                code_of(delivery(&signalled(SIGFPE, 1), arm64)).0,
                status::INTEGER_DIVIDE_BY_ZERO
            );
            assert_eq!(
                code_of(delivery(&signalled(SIGFPE, 3), arm64)).0,
                status::FLOAT_DIVIDE_BY_ZERO
            );
            assert_eq!(
                code_of(delivery(&signalled(SIGILL, 1), arm64)).0,
                status::ILLEGAL_INSTRUCTION
            );
            assert_eq!(
                code_of(delivery(&signalled(SIGBUS, BUS_ADRALN), arm64)).0,
                status::DATATYPE_MISALIGNMENT
            );
        }
        assert_eq!(
            code_of(delivery(&signalled(SIGSEGV, SI_KERNEL), false)),
            (status::ACCESS_VIOLATION, pc, vec![access::READ, u64::MAX])
        );
    }

    #[test]
    fn breakpoints_division_by_zero_and_fast_fail_follow_each_cpu_s_instructions() {
        let pc = 0x1_4000_2000;
        let int3 = Signalled {
            byte_before: INT3,
            ..signalled(SIGTRAP, SI_KERNEL)
        };
        assert_eq!(
            code_of(delivery(&int3, false)),
            (status::BREAKPOINT, pc - 1, vec![0])
        );
        let fast_fail = Signalled {
            instruction: 0x0000_29cd,
            ..signalled(SIGSEGV, SI_KERNEL)
        };
        assert_eq!(delivery(&fast_fail, false), Delivery::FailFast);
        let brk = |immediate: u32| Signalled {
            instruction: ARM64_BRK | immediate << 5,
            ..signalled(SIGTRAP, TRAP_BRKPT)
        };
        assert_eq!(
            code_of(delivery(&brk(0xf000), true)),
            (status::BREAKPOINT, pc, vec![0])
        );
        assert_eq!(
            code_of(delivery(&brk(0xf004), true)),
            (status::INTEGER_DIVIDE_BY_ZERO, pc, vec![])
        );
        assert_eq!(
            code_of(delivery(&brk(0xf001), true)).0,
            status::ASSERTION_FAILURE
        );
        assert_eq!(delivery(&brk(0xf003), true), Delivery::FailFast);
        assert_eq!(
            code_of(delivery(&brk(0), true)).0,
            status::ILLEGAL_INSTRUCTION
        );
    }

    #[test]
    fn an_unhandled_exception_and_a_fast_fail_name_where_they_happened() {
        assert_eq!(
            text(&report_exception(
                b"eggcpp.exe",
                IMAGE,
                0xe000_0001,
                0x1_4000_1010
            )),
            "windows: eggcpp.exe: exception 0xe0000001 was not handled, raised at 0x140001010 \
             (eggcpp.exe+0x1010)\n"
        );
        assert_eq!(
            text(&report_fail_fast(b"a.exe", IMAGE, 0x10)),
            "windows: a.exe: the program ended itself at once (__fastfail) at 0x10 \
             (outside the program's image)\n"
        );
    }

    #[test]
    fn a_long_program_path_is_cut_rather_than_overflowing_the_line() {
        let path = [b'a'; 600];
        let fault = Fault {
            signal: SIGFPE,
            pc: 0,
            address: 0,
        };
        assert_eq!(report(&path, IMAGE, fault).as_bytes().len(), 512);
    }
}
