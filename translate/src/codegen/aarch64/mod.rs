//! The AArch64 side of code generation: instruction encodings ([`encode`]) and the code buffer
//! with labels, branch fixups and a literal pool ([`emit`]).

pub mod emit;
pub mod encode;

#[cfg(test)]
mod encode_tests;

/// A general-purpose register number, 0 to 30; 31 is the zero register or `sp`, depending on
/// the instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Reg(pub u8);

impl Reg {
    /// The zero register (`xzr`/`wzr`) where the encoding reads 31 as zero.
    pub const ZR: Reg = Reg(31);
}

/// The operation size of a general-purpose instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Size {
    /// 32 bits (`w` registers).
    W,
    /// 64 bits (`x` registers).
    X,
}

impl Size {
    /// The size in bits.
    pub const fn bits(self) -> u32 {
        match self {
            Size::W => 32,
            Size::X => 64,
        }
    }
}

/// The size of a memory access.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Access {
    /// 1 byte.
    B,
    /// 2 bytes.
    H,
    /// 4 bytes.
    W,
    /// 8 bytes.
    X,
}

impl Access {
    /// The size field (bits 30 and 31) of load, store and atomic encodings.
    pub const fn size_field(self) -> u32 {
        match self {
            Access::B => 0,
            Access::H => 1,
            Access::W => 2,
            Access::X => 3,
        }
    }
}

/// An AArch64 condition code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cond {
    /// Z set.
    Eq = 0,
    /// Z clear.
    Ne = 1,
    /// C set.
    Cs = 2,
    /// C clear.
    Cc = 3,
    /// N set.
    Mi = 4,
    /// N clear.
    Pl = 5,
    /// V set.
    Vs = 6,
    /// V clear.
    Vc = 7,
    /// C set and Z clear.
    Hi = 8,
    /// C clear or Z set.
    Ls = 9,
    /// N equals V.
    Ge = 10,
    /// N differs from V.
    Lt = 11,
    /// Z clear and N equals V.
    Gt = 12,
    /// Z set or N differs from V.
    Le = 13,
    /// Always.
    Al = 14,
}

impl Cond {
    /// The opposite condition.
    pub const fn invert(self) -> Cond {
        match self {
            Cond::Eq => Cond::Ne,
            Cond::Ne => Cond::Eq,
            Cond::Cs => Cond::Cc,
            Cond::Cc => Cond::Cs,
            Cond::Mi => Cond::Pl,
            Cond::Pl => Cond::Mi,
            Cond::Vs => Cond::Vc,
            Cond::Vc => Cond::Vs,
            Cond::Hi => Cond::Ls,
            Cond::Ls => Cond::Hi,
            Cond::Ge => Cond::Lt,
            Cond::Lt => Cond::Ge,
            Cond::Gt => Cond::Le,
            Cond::Le => Cond::Gt,
            Cond::Al => Cond::Al,
        }
    }
}

/// The shift applied to the second register of a shifted-register instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shift {
    /// Logical shift left.
    Lsl = 0,
    /// Logical shift right.
    Lsr = 1,
    /// Arithmetic shift right.
    Asr = 2,
    /// Rotate right (logical instructions only).
    Ror = 3,
}
