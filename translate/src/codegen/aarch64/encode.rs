//! AArch64 instruction encodings, one function per instruction form, from the encoding
//! diagrams of Arm's Architecture Reference Manual (A64). Every function is pure and returns
//! the 32-bit instruction word; register numbers are taken modulo 32 and immediates are
//! truncated to their field, so callers check ranges first.

use super::{Access, Cond, Reg, Shift, Size};

fn sf(size: Size) -> u32 {
    match size {
        Size::W => 0,
        Size::X => 1 << 31,
    }
}

fn rd(r: Reg) -> u32 {
    u32::from(r.0 & 31)
}

fn rn(r: Reg) -> u32 {
    u32::from(r.0 & 31) << 5
}

fn rm(r: Reg) -> u32 {
    u32::from(r.0 & 31) << 16
}

/// The add and subtract operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddSub {
    /// `add`.
    Add,
    /// `adds`, setting NZCV.
    Adds,
    /// `sub`.
    Sub,
    /// `subs`, setting NZCV.
    Subs,
}

impl AddSub {
    const fn bits(self) -> u32 {
        match self {
            AddSub::Add => 0,
            AddSub::Adds => 1 << 29,
            AddSub::Sub => 1 << 30,
            AddSub::Subs => 3 << 29,
        }
    }
}

/// `add`/`adds`/`sub`/`subs` with a 12-bit unsigned immediate, optionally shifted left by 12.
/// Register 31 is `sp` for `rn`, and for `rd` unless the form sets flags.
pub fn add_sub_immediate(op: AddSub, size: Size, d: Reg, n: Reg, imm12: u16, lsl12: bool) -> u32 {
    0x1100_0000
        | op.bits()
        | sf(size)
        | (u32::from(lsl12) << 22)
        | (u32::from(imm12 & 0xFFF) << 10)
        | rn(n)
        | rd(d)
}

/// `add`/`adds`/`sub`/`subs` with a shifted register; register 31 is the zero register.
pub fn add_sub_shifted(
    op: AddSub,
    size: Size,
    d: Reg,
    n: Reg,
    m: Reg,
    shift: Shift,
    amount: u8,
) -> u32 {
    0x0B00_0000
        | op.bits()
        | sf(size)
        | ((shift as u32) << 22)
        | rm(m)
        | (u32::from(amount & 63) << 10)
        | rn(n)
        | rd(d)
}

/// The logical operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Logic {
    /// `and` (with `negate`: `bic`).
    And = 0,
    /// `orr` (with `negate`: `orn`).
    Orr = 1,
    /// `eor` (with `negate`: `eon`).
    Eor = 2,
    /// `ands`, setting N and Z and clearing C and V (with `negate`: `bics`).
    Ands = 3,
}

/// A logical instruction with a shifted register; `negate` inverts the shifted operand
/// (`bic`, `orn`, `eon`, `bics`).
#[allow(clippy::too_many_arguments)]
pub fn logical_shifted(
    op: Logic,
    negate: bool,
    size: Size,
    d: Reg,
    n: Reg,
    m: Reg,
    shift: Shift,
    amount: u8,
) -> u32 {
    0x0A00_0000
        | ((op as u32) << 29)
        | sf(size)
        | ((shift as u32) << 22)
        | (u32::from(negate) << 21)
        | rm(m)
        | (u32::from(amount & 63) << 10)
        | rn(n)
        | rd(d)
}

/// The `N:immr:imms` fields of a logical immediate, or `None` when `value` is not a
/// replicated, rotated run of ones (all zeros and all ones never are).
pub fn bitmask_immediate(value: u64, size: Size) -> Option<u32> {
    let value = match size {
        Size::W => {
            let low = value & 0xFFFF_FFFF;
            low | (low << 32)
        }
        Size::X => value,
    };
    if value == 0 || value == u64::MAX {
        return None;
    }
    let mut element = 64u32;
    while element > 2 {
        let half = element / 2;
        let mask = (1u64 << half) - 1;
        if (value & mask) != ((value >> half) & mask) {
            break;
        }
        element = half;
    }
    let mask = if element == 64 {
        u64::MAX
    } else {
        (1u64 << element) - 1
    };
    let pattern = value & mask;
    let ones = pattern.count_ones();
    let run = if ones == 64 {
        u64::MAX
    } else {
        (1u64 << ones) - 1
    };
    let rotation = (0..element).find(|&r| rotate_right(run, r, element, mask) == pattern)?;
    let n = u32::from(element == 64);
    let imms = (!(element * 2 - 1) & 0x3F) | (ones - 1);
    Some((n << 22) | (rotation << 16) | (imms << 10))
}

fn rotate_right(value: u64, amount: u32, element: u32, mask: u64) -> u64 {
    if amount == 0 {
        value
    } else {
        ((value >> amount) | (value << (element - amount))) & mask
    }
}

/// A logical instruction with an immediate (`and`, `orr`, `eor`, `ands`), or `None` when the
/// value has no bitmask encoding. Register 31 is `sp` for `rd` except in `ands`.
pub fn logical_immediate(op: Logic, size: Size, d: Reg, n: Reg, value: u64) -> Option<u32> {
    let fields = bitmask_immediate(value, size)?;
    Some(0x1200_0000 | ((op as u32) << 29) | sf(size) | fields | rn(n) | rd(d))
}

/// The move-wide operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveWide {
    /// `movn`: the inverted, shifted immediate.
    Movn = 0,
    /// `movz`: the shifted immediate, other bits zero.
    Movz = 2,
    /// `movk`: the immediate into one 16-bit field, other bits kept.
    Movk = 3,
}

/// `movn`/`movz`/`movk` of a 16-bit immediate at `16 * hw`.
pub fn move_wide(op: MoveWide, size: Size, d: Reg, imm16: u16, hw: u8) -> u32 {
    0x1280_0000
        | ((op as u32) << 29)
        | sf(size)
        | (u32::from(hw & 3) << 21)
        | (u32::from(imm16) << 5)
        | rd(d)
}

/// The bitfield operations, base of `lsl`/`lsr`/`asr` by immediate, `ubfx`, `sbfx`, `bfi`,
/// `bfxil`, `sxtb`, `uxtb`, ...
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bitfield {
    /// `sbfm`.
    Sbfm = 0,
    /// `bfm`.
    Bfm = 1,
    /// `ubfm`.
    Ubfm = 2,
}

/// `sbfm`/`bfm`/`ubfm` with `immr` and `imms`.
pub fn bitfield(op: Bitfield, size: Size, d: Reg, n: Reg, immr: u8, imms: u8) -> u32 {
    let n_bit = match size {
        Size::W => 0,
        Size::X => 1 << 22,
    };
    0x1300_0000
        | ((op as u32) << 29)
        | sf(size)
        | n_bit
        | (u32::from(immr & 63) << 16)
        | (u32::from(imms & 63) << 10)
        | rn(n)
        | rd(d)
}

/// `extr`: bits `lsb` and up of the concatenation `n:m`.
pub fn extr(size: Size, d: Reg, n: Reg, m: Reg, lsb: u8) -> u32 {
    let n_bit = match size {
        Size::W => 0,
        Size::X => 1 << 22,
    };
    0x1380_0000 | sf(size) | n_bit | rm(m) | (u32::from(lsb & 63) << 10) | rn(n) | rd(d)
}

/// `adr`: `d` = this instruction's address + `offset` bytes (within 1 MiB).
pub fn adr(d: Reg, offset: i32) -> u32 {
    let imm = offset as u32;
    0x1000_0000 | ((imm & 3) << 29) | (((imm >> 2) & 0x7_FFFF) << 5) | rd(d)
}

/// The add and subtract with carry operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Carry {
    /// `adc`: n + m + C.
    Adc,
    /// `adcs`.
    Adcs,
    /// `sbc`: n - m - !C.
    Sbc,
    /// `sbcs`.
    Sbcs,
}

/// `adc`/`adcs`/`sbc`/`sbcs`.
pub fn carry(op: Carry, size: Size, d: Reg, n: Reg, m: Reg) -> u32 {
    let bits = match op {
        Carry::Adc => 0,
        Carry::Adcs => 1 << 29,
        Carry::Sbc => 1 << 30,
        Carry::Sbcs => 3 << 29,
    };
    0x1A00_0000 | bits | sf(size) | rm(m) | rn(n) | rd(d)
}

/// The conditional select operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Select {
    /// `csel`: cond ? n : m.
    Csel,
    /// `csinc`: cond ? n : m + 1.
    Csinc,
    /// `csinv`: cond ? n : !m.
    Csinv,
    /// `csneg`: cond ? n : -m.
    Csneg,
}

/// `csel`/`csinc`/`csinv`/`csneg`.
pub fn select(op: Select, size: Size, d: Reg, n: Reg, m: Reg, cond: Cond) -> u32 {
    let bits = match op {
        Select::Csel => 0,
        Select::Csinc => 0x400,
        Select::Csinv => 1 << 30,
        Select::Csneg => (1 << 30) | 0x400,
    };
    0x1A80_0000 | bits | sf(size) | rm(m) | ((cond as u32) << 12) | rn(n) | rd(d)
}

/// `cset`: `d` = 1 when `cond` holds, else 0.
pub fn cset(size: Size, d: Reg, cond: Cond) -> u32 {
    select(Select::Csinc, size, d, Reg::ZR, Reg::ZR, cond.invert())
}

/// The two-source data-processing operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TwoSource {
    /// `udiv` (division by zero gives zero).
    Udiv = 0x800,
    /// `sdiv`.
    Sdiv = 0xC00,
    /// `lslv`: shift by `m` modulo the size.
    Lslv = 0x2000,
    /// `lsrv`.
    Lsrv = 0x2400,
    /// `asrv`.
    Asrv = 0x2800,
    /// `rorv`.
    Rorv = 0x2C00,
}

/// `udiv`/`sdiv`/`lslv`/`lsrv`/`asrv`/`rorv`.
pub fn two_source(op: TwoSource, size: Size, d: Reg, n: Reg, m: Reg) -> u32 {
    0x1AC0_0000 | (op as u32) | sf(size) | rm(m) | rn(n) | rd(d)
}

/// The one-source data-processing operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OneSource {
    /// `rbit`: bit reversal.
    Rbit,
    /// `rev16`: bytes reversed in each halfword.
    Rev16,
    /// `rev`: all bytes reversed.
    Rev,
    /// `clz`: leading zeros.
    Clz,
}

/// `rbit`/`rev16`/`rev`/`clz`.
pub fn one_source(op: OneSource, size: Size, d: Reg, n: Reg) -> u32 {
    let opcode = match (op, size) {
        (OneSource::Rbit, _) => 0,
        (OneSource::Rev16, _) => 1,
        (OneSource::Rev, Size::W) => 2,
        (OneSource::Rev, Size::X) => 3,
        (OneSource::Clz, _) => 4,
    };
    0x5AC0_0000 | sf(size) | (opcode << 10) | rn(n) | rd(d)
}

/// The three-source multiply operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Multiply {
    /// `madd`: a + n * m.
    Madd,
    /// `msub`: a - n * m.
    Msub,
    /// `smaddl`: a + n * m, 32-bit signed sources, 64-bit result.
    Smaddl,
    /// `umaddl`: a + n * m, 32-bit unsigned sources, 64-bit result.
    Umaddl,
    /// `smulh`: the high 64 bits of the signed 128-bit product (`a` must be 31).
    Smulh,
    /// `umulh`: the high 64 bits of the unsigned 128-bit product (`a` must be 31).
    Umulh,
}

/// `madd`/`msub` (sized by `size`) and the always-64-bit `smaddl`/`umaddl`/`smulh`/`umulh`.
pub fn multiply(op: Multiply, size: Size, d: Reg, n: Reg, m: Reg, a: Reg) -> u32 {
    let base = match op {
        Multiply::Madd => 0x1B00_0000 | sf(size),
        Multiply::Msub => 0x1B00_8000 | sf(size),
        Multiply::Smaddl => 0x9B20_0000,
        Multiply::Umaddl => 0x9BA0_0000,
        Multiply::Smulh => 0x9B40_0000,
        Multiply::Umulh => 0x9BC0_0000,
    };
    base | rm(m) | (u32::from(a.0 & 31) << 10) | rn(n) | rd(d)
}

/// The plain loads and stores with an unsigned, scaled 12-bit offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadStore {
    /// `strb`.
    Strb,
    /// `ldrb`, zero-extending.
    Ldrb,
    /// `ldrsb` into an `x` register.
    Ldrsb,
    /// `strh`.
    Strh,
    /// `ldrh`, zero-extending.
    Ldrh,
    /// `ldrsh` into an `x` register.
    Ldrsh,
    /// `str` of a `w` register.
    StrW,
    /// `ldr` into a `w` register, zero-extending.
    LdrW,
    /// `ldrsw` into an `x` register.
    Ldrsw,
    /// `str` of an `x` register.
    StrX,
    /// `ldr` into an `x` register.
    LdrX,
}

/// A load or store at `n + offset`, where `offset` is a multiple of the access size below
/// 4096 times it; `None` otherwise. Register 31 is `sp` for `n`.
pub fn load_store(op: LoadStore, t: Reg, n: Reg, offset: u32) -> Option<u32> {
    let (base, scale) = match op {
        LoadStore::Strb => (0x3900_0000, 0),
        LoadStore::Ldrb => (0x3940_0000, 0),
        LoadStore::Ldrsb => (0x3980_0000, 0),
        LoadStore::Strh => (0x7900_0000, 1),
        LoadStore::Ldrh => (0x7940_0000, 1),
        LoadStore::Ldrsh => (0x7980_0000, 1),
        LoadStore::StrW => (0xB900_0000, 2),
        LoadStore::LdrW => (0xB940_0000, 2),
        LoadStore::Ldrsw => (0xB980_0000, 2),
        LoadStore::StrX => (0xF900_0000, 3),
        LoadStore::LdrX => (0xF940_0000, 3),
    };
    let unit = 1u32 << scale;
    if offset % unit != 0 || offset / unit > 0xFFF {
        return None;
    }
    Some(base | ((offset / unit) << 10) | rn(n) | rd(t))
}

/// `ldr` of an `x` register from a literal `offset_words` instructions away (within 1 MiB).
pub fn load_literal(t: Reg, offset_words: i32) -> u32 {
    0x5800_0000 | (((offset_words as u32) & 0x7_FFFF) << 5) | rd(t)
}

/// The ordered and exclusive single-register accesses, all addressing `[n]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ordered {
    /// `ldar`: load-acquire.
    Ldar,
    /// `ldapr`: load-acquire RCpc (FEAT_LRCPC).
    Ldapr,
    /// `stlr`: store-release.
    Stlr,
    /// `ldaxr`: load-acquire exclusive.
    Ldaxr,
}

/// `ldar`/`ldapr`/`stlr`/`ldaxr` of the given size at `[n]`.
pub fn ordered(op: Ordered, access: Access, t: Reg, n: Reg) -> u32 {
    let base = match op {
        Ordered::Ldar => 0x08DF_FC00,
        Ordered::Ldapr => 0x38BF_C000,
        Ordered::Stlr => 0x089F_FC00,
        Ordered::Ldaxr => 0x085F_FC00,
    };
    base | (access.size_field() << 30) | rn(n) | rd(t)
}

/// `stlxr`: store-release exclusive of `t` at `[n]`; `s` gets 0 on success, 1 on failure.
pub fn stlxr(access: Access, s: Reg, t: Reg, n: Reg) -> u32 {
    0x0800_FC00 | (access.size_field() << 30) | rm(s) | rn(n) | rd(t)
}

/// `clrex`: clears the local exclusive monitor.
pub const fn clrex() -> u32 {
    0xD503_3F5F
}

/// The LSE atomic memory operations (FEAT_LSE), all acquire-release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Atomic {
    /// `ldaddal`: memory += s.
    Ldadd,
    /// `ldclral`: memory &= !s.
    Ldclr,
    /// `ldeoral`: memory ^= s.
    Ldeor,
    /// `ldsetal`: memory |= s.
    Ldset,
    /// `swpal`: memory = s.
    Swp,
}

/// An LSE atomic at `[n]` with operand `s`, the old value into `t`.
pub fn atomic(op: Atomic, access: Access, s: Reg, t: Reg, n: Reg) -> u32 {
    let opcode = match op {
        Atomic::Ldadd => 0,
        Atomic::Ldclr => 0x1000,
        Atomic::Ldeor => 0x2000,
        Atomic::Ldset => 0x3000,
        Atomic::Swp => 0x8000,
    };
    0x38E0_0000 | (access.size_field() << 30) | rm(s) | opcode | rn(n) | rd(t)
}

/// `casal`: when `[n]` equals `s`, store `t` there; `s` gets the old value either way.
pub fn casal(access: Access, s: Reg, t: Reg, n: Reg) -> u32 {
    0x08E0_FC00 | (access.size_field() << 30) | rm(s) | rn(n) | rd(t)
}

/// The 64-bit register-pair accesses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pair {
    /// `stp` at `[n, #offset]`.
    Stp,
    /// `ldp` from `[n, #offset]`.
    Ldp,
}

/// `stp`/`ldp` of two `x` registers at `n + offset`, a multiple of 8 from -512 to 504.
pub fn pair(op: Pair, t1: Reg, t2: Reg, n: Reg, offset: i32) -> u32 {
    let base = match op {
        Pair::Stp => 0xA900_0000,
        Pair::Ldp => 0xA940_0000,
    };
    let imm7 = ((offset / 8) as u32) & 0x7F;
    base | (imm7 << 15) | (u32::from(t2.0 & 31) << 10) | rn(n) | rd(t1)
}

/// `b`: a branch `offset_words` instructions away (within 128 MiB).
pub fn b(offset_words: i32) -> u32 {
    0x1400_0000 | ((offset_words as u32) & 0x03FF_FFFF)
}

/// `b.cond`: a conditional branch within 1 MiB.
pub fn b_cond(cond: Cond, offset_words: i32) -> u32 {
    0x5400_0000 | (((offset_words as u32) & 0x7_FFFF) << 5) | (cond as u32)
}

/// `cbz` (or `cbnz` when `nonzero`) within 1 MiB.
pub fn cbz(size: Size, t: Reg, offset_words: i32, nonzero: bool) -> u32 {
    0x3400_0000
        | sf(size)
        | (u32::from(nonzero) << 24)
        | (((offset_words as u32) & 0x7_FFFF) << 5)
        | rd(t)
}

/// `tbz` (or `tbnz` when `nonzero`) on bit `bit` of `t`, within 32 KiB.
pub fn tbz(t: Reg, bit: u8, offset_words: i32, nonzero: bool) -> u32 {
    0x3600_0000
        | (u32::from(bit >> 5) << 31)
        | (u32::from(nonzero) << 24)
        | (u32::from(bit & 31) << 19)
        | (((offset_words as u32) & 0x3FFF) << 5)
        | rd(t)
}

/// `br`: a branch to the address in `n`.
pub fn br(n: Reg) -> u32 {
    0xD61F_0000 | rn(n)
}

/// `brk` with a 16-bit immediate the exception handler reads.
pub fn brk(imm16: u16) -> u32 {
    0xD420_0000 | (u32::from(imm16) << 5)
}

/// `nop`.
pub const fn nop() -> u32 {
    0xD503_201F
}

/// `yield`, the spin-wait hint.
pub const fn yield_hint() -> u32 {
    0xD503_203F
}

/// The data memory barrier domains used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Barrier {
    /// `dmb ish`: all accesses, inner shareable.
    Ish = 0xB,
    /// `dmb ishst`: stores only.
    IshSt = 0xA,
    /// `dmb ishld`: loads against later loads and stores.
    IshLd = 0x9,
}

/// `dmb`.
pub fn dmb(barrier: Barrier) -> u32 {
    0xD503_30BF | ((barrier as u32) << 8)
}

/// `cfinv`: inverts C (FEAT_FlagM).
pub const fn cfinv() -> u32 {
    0xD500_401F
}

/// `mrs t, nzcv`.
pub fn mrs_nzcv(t: Reg) -> u32 {
    0xD53B_4200 | rd(t)
}

/// `msr nzcv, t`.
pub fn msr_nzcv(t: Reg) -> u32 {
    0xD51B_4200 | rd(t)
}

/// `fmov d<v>, x<n>`.
pub fn fmov_d_from_x(v: u8, n: Reg) -> u32 {
    0x9E67_0000 | rn(n) | u32::from(v & 31)
}

/// `fmov w<d>, s<v>`.
pub fn fmov_w_from_s(d: Reg, v: u8) -> u32 {
    0x1E26_0000 | (u32::from(v & 31) << 5) | rd(d)
}

/// `cnt v<d>.8b, v<n>.8b`: the set bits of each byte.
pub fn cnt_8b(d: u8, n: u8) -> u32 {
    0x0E20_5800 | (u32::from(n & 31) << 5) | u32::from(d & 31)
}

/// `addv b<d>, v<n>.8b`: the sum of the eight bytes.
pub fn addv_8b(d: u8, n: u8) -> u32 {
    0x0E31_B800 | (u32::from(n & 31) << 5) | u32::from(d & 31)
}
