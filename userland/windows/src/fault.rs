//! What the layer does with a hardware fault in a Windows program (D151), as pure logic: the
//! one instruction it completes itself, and the line it writes before the program ends.

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

    fn push_hex(&mut self, value: u64) {
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
    match fault.pc.checked_sub(image.base) {
        Some(offset) if offset < image.size => {
            line.push(b" (");
            line.push(file_name(program));
            line.push(b"+");
            line.push_hex(offset);
            line.push(b")");
        }
        _ => line.push(b" (outside the program's image)"),
    }
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
