//! The decoded form of one x86-64 instruction: what it does ([`Mnemonic`]), at what size
//! ([`Width`]) and on what ([`Operand`]s), as the code generator consumes it.

mod mnemonic;

pub use mnemonic::{Condition, Mnemonic};

/// A general-purpose register by its encoding number: 0 is `rax`, 15 is `r15`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Gpr(pub u8);

impl Gpr {
    /// `rax`, the accumulator.
    pub const RAX: Gpr = Gpr(0);
    /// `rcx`, the count register.
    pub const RCX: Gpr = Gpr(1);
    /// `rdx`.
    pub const RDX: Gpr = Gpr(2);
    /// `rbx`.
    pub const RBX: Gpr = Gpr(3);
    /// `rsp`, the stack pointer.
    pub const RSP: Gpr = Gpr(4);
    /// `rbp`.
    pub const RBP: Gpr = Gpr(5);
    /// `rsi`, the string source.
    pub const RSI: Gpr = Gpr(6);
    /// `rdi`, the string destination.
    pub const RDI: Gpr = Gpr(7);
    /// `r8`.
    pub const R8: Gpr = Gpr(8);
    /// `r9`.
    pub const R9: Gpr = Gpr(9);
    /// `r10`.
    pub const R10: Gpr = Gpr(10);
    /// `r11`.
    pub const R11: Gpr = Gpr(11);
    /// `r12`.
    pub const R12: Gpr = Gpr(12);
    /// `r13`.
    pub const R13: Gpr = Gpr(13);
    /// `r14`.
    pub const R14: Gpr = Gpr(14);
    /// `r15`.
    pub const R15: Gpr = Gpr(15);
}

/// The size of an operand or of the operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Width {
    /// 8 bits.
    Byte,
    /// 16 bits.
    Word,
    /// 32 bits.
    Dword,
    /// 64 bits.
    Qword,
    /// 80 bits, the x87 extended format in memory.
    Tbyte,
    /// 128 bits, an XMM register or a 16-byte memory operand.
    Dqword,
}

impl Width {
    /// The size in bytes.
    pub const fn bytes(self) -> u8 {
        match self {
            Width::Byte => 1,
            Width::Word => 2,
            Width::Dword => 4,
            Width::Qword => 8,
            Width::Tbyte => 10,
            Width::Dqword => 16,
        }
    }
}

/// A register operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Register {
    /// The low `width` bits of a general-purpose register (`al`, `spl`, `r9w`, `eax`, `rax`, ...).
    General {
        /// Which register.
        gpr: Gpr,
        /// How much of it.
        width: Width,
    },
    /// Bits 8 to 15 of `rax`, `rcx`, `rdx` or `rbx` (`ah`, `ch`, `dh`, `bh`), only reachable
    /// without a REX prefix.
    HighByte(Gpr),
    /// An SSE register, `xmm0` to `xmm15`.
    Xmm(u8),
    /// An x87 stack register, `st(0)` to `st(7)`, relative to the stack's top.
    St(u8),
}

/// The segment override a memory operand keeps; 64-bit mode ignores the others.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Segment {
    /// `fs:`, which Windows and macOS programs leave to the system.
    Fs,
    /// `gs:`, the TEB on Windows and the thread's TSD on macOS.
    Gs,
}

/// A scaled index register in an address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Index {
    /// The register.
    pub register: Gpr,
    /// 1, 2, 4 or 8.
    pub scale: u8,
}

/// How a memory operand's address is formed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Address {
    /// `base + index * scale + displacement`; with neither base nor index, the sign-extended
    /// displacement is the address.
    Based {
        /// The base register, if any.
        base: Option<Gpr>,
        /// The scaled index, if any.
        index: Option<Index>,
        /// The sign-extended displacement.
        displacement: i32,
    },
    /// A RIP-relative operand, already resolved to the address it names.
    Rip {
        /// The absolute address.
        target: u64,
    },
    /// A 64-bit absolute address (`mov` to or from `moffs`).
    Absolute(u64),
}

/// A memory operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Memory {
    /// `fs:` or `gs:` when the instruction has that override.
    pub segment: Option<Segment>,
    /// The address.
    pub address: Address,
    /// How many bytes the instruction reads or writes there (for `lea`, the operand size).
    pub width: Width,
}

/// One operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Operand {
    /// A register.
    Register(Register),
    /// Memory.
    Memory(Memory),
    /// An immediate value: sign-extended from its encoding for arithmetic, logic, `mov`,
    /// `push`, `imul` and `test`; zero-extended for the fields that are unsigned by definition
    /// (shift and rotate counts, `bt`'s bit index, `ret`'s and `enter`'s sizes, `enter`'s
    /// level, `int`'s vector and SSE selectors). A consumer keeps the low `width` bits.
    Immediate(i64),
    /// The absolute target of a relative branch.
    Target(u64),
}

/// The repeat prefix a string instruction carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Repeat {
    /// `rep` / `repe` (`F3`).
    Rep,
    /// `repne` (`F2`).
    Repne,
}

/// The most operands one instruction has.
const MAX_OPERANDS: usize = 4;

/// One decoded instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Instruction {
    /// Where it starts.
    pub address: u64,
    /// Its length in bytes, 1 to 15.
    pub length: u8,
    /// What it does.
    pub mnemonic: Mnemonic,
    /// The operation size: the general-purpose operand size for integer instructions, the r/m
    /// operand's size for SSE and x87 instructions, 64 bits for branches.
    pub width: Width,
    /// A `lock` prefix on an instruction that accepts one (`xchg` with memory is atomic
    /// without it).
    pub lock: bool,
    /// The repeat prefix of a string instruction; `None` for every other instruction, whose
    /// `F2`/`F3` are either part of the opcode or ignored.
    pub repeat: Option<Repeat>,
    operands: [Operand; MAX_OPERANDS],
    operand_count: u8,
}

impl Instruction {
    pub(crate) fn new(address: u64, mnemonic: Mnemonic, width: Width) -> Instruction {
        Instruction {
            address,
            length: 0,
            mnemonic,
            width,
            lock: false,
            repeat: None,
            operands: [Operand::Immediate(0); MAX_OPERANDS],
            operand_count: 0,
        }
    }

    pub(crate) fn push(&mut self, operand: Operand) {
        self.operands[usize::from(self.operand_count)] = operand;
        self.operand_count += 1;
    }

    pub(crate) fn operands_mut(&mut self) -> &mut [Operand] {
        &mut self.operands[..usize::from(self.operand_count)]
    }

    /// The operands, destination first, in Intel order.
    pub fn operands(&self) -> &[Operand] {
        &self.operands[..usize::from(self.operand_count)]
    }

    /// The address of the next instruction.
    pub fn end(&self) -> u64 {
        self.address.wrapping_add(u64::from(self.length))
    }
}
