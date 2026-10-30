use super::Refusal;
use crate::instruction::{Mnemonic, Width};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SizeRule {
    Normal,
    Default64,
    Near64,
    Fixed(Width),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Size {
    Fixed(Width),
    Operand,
}

pub(super) const B: Size = Size::Fixed(Width::Byte);
pub(super) const W: Size = Size::Fixed(Width::Word);
pub(super) const D: Size = Size::Fixed(Width::Dword);
pub(super) const Q: Size = Size::Fixed(Width::Qword);
pub(super) const DQ: Size = Size::Fixed(Width::Dqword);
pub(super) const V: Size = Size::Operand;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Immediate {
    SignedByte,
    UnsignedByte,
    UnsignedWord,
    OperandSized32,
    OperandSized64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Relative {
    Byte,
    Dword,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Spec {
    Rm(Size),
    Reg(Size),
    RmXmm(Width),
    RegXmm,
    OpcodeReg(Size),
    Accumulator(Size),
    Cl,
    One,
    Imm(Immediate),
    Rel(Relative),
    St0,
    StRm,
    Moffs(Size),
}

impl Spec {
    fn uses_modrm(self) -> bool {
        matches!(
            self,
            Spec::Rm(_) | Spec::Reg(_) | Spec::RmXmm(_) | Spec::RegXmm | Spec::StRm
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ModRule {
    Any,
    RegisterOnly,
    MemoryOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    Decoded(Mnemonic),
    Refused(Refusal),
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Form {
    pub(super) outcome: Outcome,
    pub(super) size: SizeRule,
    specs: [Option<Spec>; 4],
    modrm: bool,
    pub(super) mod_rule: ModRule,
    pub(super) lockable: bool,
    pub(super) mandatory_prefix: bool,
}

impl Form {
    pub(super) fn new(mnemonic: Mnemonic, size: SizeRule) -> Form {
        Form {
            outcome: Outcome::Decoded(mnemonic),
            size,
            specs: [None; 4],
            modrm: false,
            mod_rule: ModRule::Any,
            lockable: false,
            mandatory_prefix: false,
        }
    }

    pub(super) fn refused(refusal: Refusal) -> Form {
        Form {
            outcome: Outcome::Refused(refusal),
            ..Form::new(Mnemonic::Nop, SizeRule::Normal)
        }
    }

    pub(super) fn with(mut self, spec: Spec) -> Form {
        if let Some(slot) = self.specs.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(spec);
        }
        self
    }

    pub(super) fn modrm(mut self) -> Form {
        self.modrm = true;
        self
    }

    pub(super) fn register_only(mut self) -> Form {
        self.mod_rule = ModRule::RegisterOnly;
        self
    }

    pub(super) fn memory_only(mut self) -> Form {
        self.mod_rule = ModRule::MemoryOnly;
        self
    }

    pub(super) fn lockable(mut self) -> Form {
        self.lockable = true;
        self
    }

    pub(super) fn sse(mut self) -> Form {
        self.mandatory_prefix = true;
        self
    }

    pub(super) fn refuse(mut self, refusal: Refusal) -> Form {
        self.outcome = Outcome::Refused(refusal);
        self
    }

    pub(super) fn specs(&self) -> impl Iterator<Item = Spec> + '_ {
        self.specs.iter().map_while(|spec| *spec)
    }

    pub(super) fn needs_modrm(&self) -> bool {
        self.modrm || self.specs().any(Spec::uses_modrm)
    }
}
