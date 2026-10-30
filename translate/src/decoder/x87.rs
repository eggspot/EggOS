use super::form::{Form, Size, SizeRule, Spec, W};
use super::modrm::{is_register_form, reg_field};
use super::{DecodeErrorKind, Refusal};
use crate::instruction::{Mnemonic, Width};

pub(super) fn form(
    opcode: u8,
    modrm: Result<u8, DecodeErrorKind>,
) -> Result<Form, DecodeErrorKind> {
    let modrm = modrm?;
    if is_register_form(modrm) {
        register_form(opcode, modrm)
    } else {
        memory_form(opcode, reg_field(modrm))
    }
}

fn memory(mnemonic: Mnemonic, width: Width) -> Form {
    Form::new(mnemonic, SizeRule::Fixed(width))
        .with(Spec::Rm(Size::Fixed(width)))
        .memory_only()
}

fn refused(refusal: Refusal) -> Form {
    Form::refused(refusal).modrm()
}

fn memory_form(opcode: u8, operation: u8) -> Result<Form, DecodeErrorKind> {
    use Mnemonic as M;
    use Width::{Dword, Qword, Tbyte, Word};
    const ARITHMETIC: [Option<Mnemonic>; 8] = [
        Some(Mnemonic::Fadd),
        Some(Mnemonic::Fmul),
        None,
        None,
        Some(Mnemonic::Fsub),
        Some(Mnemonic::Fsubr),
        Some(Mnemonic::Fdiv),
        Some(Mnemonic::Fdivr),
    ];
    Ok(match (opcode, operation) {
        (0xD8, _) => ARITHMETIC[usize::from(operation)]
            .map_or_else(|| refused(Refusal::X87), |mnemonic| memory(mnemonic, Dword)),
        (0xDC, _) => ARITHMETIC[usize::from(operation)]
            .map_or_else(|| refused(Refusal::X87), |mnemonic| memory(mnemonic, Qword)),
        (0xD9, 0) => memory(M::Fld, Dword),
        (0xD9, 2) => memory(M::Fst, Dword),
        (0xD9, 3) => memory(M::Fstp, Dword),
        (0xD9, 5) => memory(M::Fldcw, Word),
        (0xD9, 7) => memory(M::Fnstcw, Word),
        (0xDB, 0) => memory(M::Fild, Dword),
        (0xDB, 2) => memory(M::Fist, Dword),
        (0xDB, 3) => memory(M::Fistp, Dword),
        (0xDB, 5) => memory(M::Fld, Tbyte),
        (0xDB, 7) => memory(M::Fstp, Tbyte),
        (0xDD, 0) => memory(M::Fld, Qword),
        (0xDD, 2) => memory(M::Fst, Qword),
        (0xDD, 3) => memory(M::Fstp, Qword),
        (0xDD, 7) => memory(M::Fnstsw, Word),
        (0xDF, 0) => memory(M::Fild, Word),
        (0xDF, 2) => memory(M::Fist, Word),
        (0xDF, 3) => memory(M::Fistp, Word),
        (0xDF, 5) => memory(M::Fild, Qword),
        (0xDF, 7) => memory(M::Fistp, Qword),
        (0xDB | 0xDD | 0xDF, 1) => refused(Refusal::Sse3),
        (0xD9, 1) | (0xDB, 4 | 6) | (0xDD, 5) => return Err(DecodeErrorKind::Invalid),
        _ => refused(Refusal::X87),
    })
}

fn register_form(opcode: u8, modrm: u8) -> Result<Form, DecodeErrorKind> {
    use Mnemonic as M;
    use Spec::{St0, StRm};
    let stack = SizeRule::Fixed(Width::Tbyte);
    let with_top = |mnemonic| Form::new(mnemonic, stack).with(St0).with(StRm);
    let into_other = |mnemonic| Form::new(mnemonic, stack).with(StRm).with(St0);
    let alone = |mnemonic| Form::new(mnemonic, stack).modrm();
    Ok(match (opcode, reg_field(modrm)) {
        (0xD8, 0) => with_top(M::Fadd),
        (0xD8, 1) => with_top(M::Fmul),
        (0xD8, 4) => with_top(M::Fsub),
        (0xD8, 5) => with_top(M::Fsubr),
        (0xD8, 6) => with_top(M::Fdiv),
        (0xD8, 7) => with_top(M::Fdivr),
        (0xDC, 0) => into_other(M::Fadd),
        (0xDC, 1) => into_other(M::Fmul),
        (0xDC, 4) => into_other(M::Fsubr),
        (0xDC, 5) => into_other(M::Fsub),
        (0xDC, 6) => into_other(M::Fdivr),
        (0xDC, 7) => into_other(M::Fdiv),
        (0xDE, 0) => into_other(M::Faddp),
        (0xDE, 1) => into_other(M::Fmulp),
        (0xDE, 4) => into_other(M::Fsubrp),
        (0xDE, 5) => into_other(M::Fsubp),
        (0xDE, 6) => into_other(M::Fdivrp),
        (0xDE, 7) => into_other(M::Fdivp),
        (0xD9, 0) => Form::new(M::Fld, stack).with(StRm),
        (0xD9, 1) => Form::new(M::Fxch, stack).with(StRm),
        (0xDD, 2) => Form::new(M::Fst, stack).with(StRm),
        (0xDD, 3) => Form::new(M::Fstp, stack).with(StRm),
        (0xDB, 5) => with_top(M::Fucomi),
        (0xDB, 6) => with_top(M::Fcomi),
        (0xDF, 5) => with_top(M::Fucomip),
        (0xDF, 6) => with_top(M::Fcomip),
        _ => match (opcode, modrm) {
            (0xD9, 0xD0) => alone(M::Nop),
            (0xD9, 0xE0) => alone(M::Fchs),
            (0xD9, 0xE1) => alone(M::Fabs),
            (0xD9, 0xE8) => alone(M::Fld1),
            (0xD9, 0xEE) => alone(M::Fldz),
            (0xDB, 0xE2) => alone(M::Fnclex),
            (0xDB, 0xE3) => alone(M::Fninit),
            (0xDF, 0xE0) => Form::new(M::Fnstsw, SizeRule::Fixed(Width::Word))
                .with(Spec::Accumulator(W))
                .modrm(),
            _ => refused(Refusal::X87),
        },
    })
}
