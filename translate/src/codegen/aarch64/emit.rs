//! The code buffer: instruction words, labels bound to positions, branches and `adr`s to
//! labels patched when the code is finished (with their ranges checked), and a literal pool of
//! 64-bit values placed after the code, 8-byte aligned relative to the code's start.

use alloc::vec::Vec;

use super::encode;
use super::{Cond, Reg, Size};

/// A position in the code, bound once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Label(u32);

/// A 64-bit value in the literal pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Literal(u32);

impl Literal {
    /// Its index among the pool's literals, which [`Assembled::literals`] is ordered by.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Range {
    Bits26,
    Bits19,
    Bits14,
}

impl Range {
    const fn limit(self) -> i64 {
        match self {
            Range::Bits26 => 1 << 25,
            Range::Bits19 => 1 << 18,
            Range::Bits14 => 1 << 13,
        }
    }

    const fn patch(self, word: u32, offset: i64) -> u32 {
        let offset = offset as u32;
        match self {
            Range::Bits26 => word | (offset & 0x03FF_FFFF),
            Range::Bits19 => word | ((offset & 0x7_FFFF) << 5),
            Range::Bits14 => word | ((offset & 0x3FFF) << 5),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Target {
    Label(Label),
    Literal(Literal),
}

#[derive(Clone, Copy, Debug)]
struct Fixup {
    at: u32,
    target: Target,
    range: Range,
}

/// Why finishing the code failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmitError {
    /// A label was branched to but never bound.
    Unbound,
    /// A branch's target is beyond its encoding's reach.
    OutOfRange { at: u32 },
}

/// Finished code: the words, then the literal pool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assembled {
    /// The instruction words followed by the literals, two words each, low word first.
    pub words: Vec<u32>,
    /// The word index of each literal, in the order they were added.
    pub literals: Vec<u32>,
}

/// The code being emitted.
#[derive(Debug, Default)]
pub struct Assembler {
    words: Vec<u32>,
    labels: Vec<Option<u32>>,
    fixups: Vec<Fixup>,
    literals: Vec<u64>,
}

impl Assembler {
    /// An empty buffer.
    pub fn new() -> Assembler {
        Assembler::default()
    }

    /// The word index the next instruction gets.
    pub fn position(&self) -> u32 {
        self.words.len() as u32
    }

    /// The emitted words so far.
    pub fn words(&self) -> &[u32] {
        &self.words
    }

    /// Appends one instruction word.
    pub fn emit(&mut self, word: u32) {
        self.words.push(word);
    }

    /// A new, unbound label.
    pub fn label(&mut self) -> Label {
        self.labels.push(None);
        Label(self.labels.len() as u32 - 1)
    }

    /// Binds `label` to the next instruction.
    pub fn bind(&mut self, label: Label) {
        self.labels[label.0 as usize] = Some(self.position());
    }

    fn reference(&mut self, word: u32, target: Target, range: Range) {
        let at = self.position();
        self.fixups.push(Fixup { at, target, range });
        self.emit(word);
    }

    /// `b label`.
    pub fn b(&mut self, label: Label) {
        self.reference(encode::b(0), Target::Label(label), Range::Bits26);
    }

    /// `b.cond label`.
    pub fn b_cond(&mut self, cond: Cond, label: Label) {
        self.reference(encode::b_cond(cond, 0), Target::Label(label), Range::Bits19);
    }

    /// `cbz`, or `cbnz` when `nonzero`.
    pub fn cbz(&mut self, size: Size, t: Reg, label: Label, nonzero: bool) {
        let word = encode::cbz(size, t, 0, nonzero);
        self.reference(word, Target::Label(label), Range::Bits19);
    }

    /// `tbz`, or `tbnz` when `nonzero`.
    pub fn tbz(&mut self, t: Reg, bit: u8, label: Label, nonzero: bool) {
        let word = encode::tbz(t, bit, 0, nonzero);
        self.reference(word, Target::Label(label), Range::Bits14);
    }

    /// `adr d, label`.
    pub fn adr(&mut self, d: Reg, label: Label) {
        self.reference(encode::adr(d, 0), Target::Label(label), Range::Bits19);
    }

    /// Adds `value` to the literal pool.
    pub fn literal(&mut self, value: u64) -> Literal {
        self.literals.push(value);
        Literal(self.literals.len() as u32 - 1)
    }

    /// `ldr t, literal`.
    pub fn load_literal(&mut self, t: Reg, literal: Literal) {
        let word = encode::load_literal(t, 0);
        self.reference(word, Target::Literal(literal), Range::Bits19);
    }

    /// Places the literal pool, patches every reference and returns the words.
    pub fn finish(mut self) -> Result<Assembled, EmitError> {
        if !self.literals.is_empty() && self.words.len() % 2 == 1 {
            self.words.push(encode::nop());
        }
        let mut literals = Vec::with_capacity(self.literals.len());
        for value in &self.literals {
            literals.push(self.words.len() as u32);
            self.words.push(*value as u32);
            self.words.push((*value >> 32) as u32);
        }
        for fixup in &self.fixups {
            let target = match fixup.target {
                Target::Label(label) => self.labels[label.0 as usize].ok_or(EmitError::Unbound)?,
                Target::Literal(literal) => literals[literal.0 as usize],
            };
            let offset = i64::from(target) - i64::from(fixup.at);
            let limit = fixup.range.limit();
            if offset < -limit || offset >= limit {
                return Err(EmitError::OutOfRange { at: fixup.at });
            }
            let at = fixup.at as usize;
            self.words[at] = fixup.range.patch(self.words[at], offset);
        }
        Ok(Assembled {
            words: self.words,
            literals,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branches_forward_and_backward() {
        let mut asm = Assembler::new();
        let top = asm.label();
        let end = asm.label();
        asm.bind(top);
        asm.emit(encode::nop());
        asm.b_cond(Cond::Ne, top);
        asm.cbz(Size::W, Reg(0), end, false);
        asm.b(top);
        asm.bind(end);
        let code = asm.finish().unwrap();
        assert_eq!(
            code.words,
            [encode::nop(), 0x54FF_FFE1, 0x3400_0040, 0x17FF_FFFD]
        );
    }

    #[test]
    fn literals_follow_the_code_aligned() {
        let mut asm = Assembler::new();
        let literal = asm.literal(0x1122_3344_5566_7788);
        asm.load_literal(Reg(16), literal);
        let code = asm.finish().unwrap();
        assert_eq!(code.literals, [2]);
        assert_eq!(
            code.words,
            [0x5800_0050, encode::nop(), 0x5566_7788, 0x1122_3344]
        );
    }

    #[test]
    fn an_unbound_label_is_an_error() {
        let mut asm = Assembler::new();
        let nowhere = asm.label();
        asm.b(nowhere);
        assert_eq!(asm.finish(), Err(EmitError::Unbound));
    }

    #[test]
    fn a_test_branch_past_32_kib_is_out_of_range() {
        let mut asm = Assembler::new();
        let far = asm.label();
        asm.tbz(Reg(0), 0, far, false);
        for _ in 0..(1 << 13) {
            asm.emit(encode::nop());
        }
        asm.bind(far);
        assert_eq!(asm.finish(), Err(EmitError::OutOfRange { at: 0 }));
    }
}
