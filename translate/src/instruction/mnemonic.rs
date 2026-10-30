/// An x86 condition, as the low four bits of `jcc`, `setcc` and `cmovcc` encode it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Condition {
    /// `o`: OF set.
    Overflow,
    /// `no`: OF clear.
    NoOverflow,
    /// `b`/`c`: CF set.
    Below,
    /// `ae`/`nc`: CF clear.
    AboveOrEqual,
    /// `e`/`z`: ZF set.
    Equal,
    /// `ne`/`nz`: ZF clear.
    NotEqual,
    /// `be`: CF or ZF set.
    BelowOrEqual,
    /// `a`: CF and ZF clear.
    Above,
    /// `s`: SF set.
    Sign,
    /// `ns`: SF clear.
    NoSign,
    /// `p`/`pe`: PF set.
    Parity,
    /// `np`/`po`: PF clear.
    NoParity,
    /// `l`: SF differs from OF.
    Less,
    /// `ge`: SF equals OF.
    GreaterOrEqual,
    /// `le`: ZF set or SF differs from OF.
    LessOrEqual,
    /// `g`: ZF clear and SF equals OF.
    Greater,
}

impl Condition {
    /// The condition the low four bits of `code` encode.
    pub const fn from_code(code: u8) -> Condition {
        match code & 0xF {
            0x0 => Condition::Overflow,
            0x1 => Condition::NoOverflow,
            0x2 => Condition::Below,
            0x3 => Condition::AboveOrEqual,
            0x4 => Condition::Equal,
            0x5 => Condition::NotEqual,
            0x6 => Condition::BelowOrEqual,
            0x7 => Condition::Above,
            0x8 => Condition::Sign,
            0x9 => Condition::NoSign,
            0xA => Condition::Parity,
            0xB => Condition::NoParity,
            0xC => Condition::Less,
            0xD => Condition::GreaterOrEqual,
            0xE => Condition::LessOrEqual,
            _ => Condition::Greater,
        }
    }
}

/// What an instruction does. Names follow Intel's, except where one encoding has a name per
/// width; those carry the width in [`Instruction::width`](super::Instruction::width).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mnemonic {
    /// `add`.
    Add,
    /// `or`.
    Or,
    /// `adc`.
    Adc,
    /// `sbb`.
    Sbb,
    /// `and`.
    And,
    /// `sub`.
    Sub,
    /// `xor`.
    Xor,
    /// `cmp`.
    Cmp,
    /// `test`.
    Test,
    /// `inc`.
    Inc,
    /// `dec`.
    Dec,
    /// `neg`.
    Neg,
    /// `not`.
    Not,
    /// `mul`, unsigned, into `rdx:rax`.
    Mul,
    /// `imul` with one (into `rdx:rax`), two or three operands.
    Imul,
    /// `div`, unsigned, of `rdx:rax`.
    Div,
    /// `idiv`, signed, of `rdx:rax`.
    Idiv,
    /// `rol`.
    Rol,
    /// `ror`.
    Ror,
    /// `rcl`.
    Rcl,
    /// `rcr`.
    Rcr,
    /// `shl`/`sal`.
    Shl,
    /// `shr`.
    Shr,
    /// `sar`.
    Sar,
    /// `shld`.
    Shld,
    /// `shrd`.
    Shrd,
    /// `bt`.
    Bt,
    /// `bts`.
    Bts,
    /// `btr`.
    Btr,
    /// `btc`.
    Btc,
    /// `bsf`.
    Bsf,
    /// `bsr`.
    Bsr,
    /// `tzcnt` (BMI1).
    Tzcnt,
    /// `lzcnt` (ABM).
    Lzcnt,
    /// `popcnt`.
    Popcnt,
    /// `bswap`.
    Bswap,
    /// `mov`.
    Mov,
    /// `movzx`.
    Movzx,
    /// `movsx`.
    Movsx,
    /// `movsxd`.
    Movsxd,
    /// `lea`.
    Lea,
    /// `xchg`, atomic when one operand is memory.
    Xchg,
    /// `xadd`.
    Xadd,
    /// `cmpxchg`.
    Cmpxchg,
    /// `cmpxchg8b`.
    Cmpxchg8b,
    /// `cmpxchg16b`.
    Cmpxchg16b,
    /// `cmovcc`.
    Cmov(Condition),
    /// `setcc`.
    Set(Condition),
    /// `jcc`.
    Jcc(Condition),
    /// `jmp`.
    Jmp,
    /// `call`.
    Call,
    /// `ret`, with the bytes to release as an optional operand.
    Ret,
    /// `loop`.
    Loop,
    /// `loope`.
    Loope,
    /// `loopne`.
    Loopne,
    /// `jrcxz`.
    Jrcxz,
    /// `push`.
    Push,
    /// `pop`.
    Pop,
    /// `pushf`.
    Pushf,
    /// `popf`.
    Popf,
    /// `lahf`.
    Lahf,
    /// `sahf`.
    Sahf,
    /// `enter`.
    Enter,
    /// `leave`.
    Leave,
    /// `cbw`, `cwde` or `cdqe`: the accumulator's lower half sign-extended into all of it.
    ExtendAccumulator,
    /// `cwd`, `cdq` or `cqo`: the accumulator's sign copied into `rdx`.
    ExtendIntoRdx,
    /// `clc`.
    Clc,
    /// `stc`.
    Stc,
    /// `cmc`.
    Cmc,
    /// `cld`.
    Cld,
    /// `std`.
    Std,
    /// `movs`.
    Movs,
    /// `cmps`.
    Cmps,
    /// `stos`.
    Stos,
    /// `lods`.
    Lods,
    /// `scas`.
    Scas,
    /// `xlat`.
    Xlat,
    /// `nop`, including the multi-byte forms, `fnop` and the reserved-nop hints.
    Nop,
    /// `pause`.
    Pause,
    /// `endbr64`.
    Endbr64,
    /// `endbr32`.
    Endbr32,
    /// `int3`.
    Int3,
    /// `int n`.
    Int,
    /// `ud0`, `ud1` or `ud2`: raises the invalid-opcode exception.
    Undefined,
    /// `syscall`.
    Syscall,
    /// `cpuid`.
    Cpuid,
    /// `rdtsc`.
    Rdtsc,
    /// `rdtscp`.
    Rdtscp,
    /// `xgetbv`.
    Xgetbv,
    /// `rdrand`.
    Rdrand,
    /// `rdseed`.
    Rdseed,
    /// `lfence`.
    Lfence,
    /// `mfence`.
    Mfence,
    /// `sfence`.
    Sfence,
    /// `prefetch*`: a hint with no architectural effect.
    Prefetch,
    /// `clflush` and `clflushopt`.
    Clflush,
    /// `movnti`.
    Movnti,
    /// `ldmxcsr`.
    Ldmxcsr,
    /// `stmxcsr`.
    Stmxcsr,
    /// `movups`.
    Movups,
    /// `movupd`.
    Movupd,
    /// `movss`.
    Movss,
    /// `movsd` (the SSE2 scalar move, not the string instruction, which is [`Mnemonic::Movs`]).
    Movsd,
    /// `movaps`.
    Movaps,
    /// `movapd`.
    Movapd,
    /// `movlps`.
    Movlps,
    /// `movlpd`.
    Movlpd,
    /// `movhps`.
    Movhps,
    /// `movhpd`.
    Movhpd,
    /// `movhlps`.
    Movhlps,
    /// `movlhps`.
    Movlhps,
    /// `movntps`.
    Movntps,
    /// `movntpd`.
    Movntpd,
    /// `movntdq`.
    Movntdq,
    /// `movmskps`.
    Movmskps,
    /// `movmskpd`.
    Movmskpd,
    /// `movd`.
    Movd,
    /// `movq`.
    Movq,
    /// `movdqa`.
    Movdqa,
    /// `movdqu`.
    Movdqu,
    /// `unpcklps`.
    Unpcklps,
    /// `unpcklpd`.
    Unpcklpd,
    /// `unpckhps`.
    Unpckhps,
    /// `unpckhpd`.
    Unpckhpd,
    /// `shufps`.
    Shufps,
    /// `shufpd`.
    Shufpd,
    /// `cvtsi2ss`.
    Cvtsi2ss,
    /// `cvtsi2sd`.
    Cvtsi2sd,
    /// `cvttss2si`.
    Cvttss2si,
    /// `cvttsd2si`.
    Cvttsd2si,
    /// `cvtss2si`.
    Cvtss2si,
    /// `cvtsd2si`.
    Cvtsd2si,
    /// `cvtps2pd`.
    Cvtps2pd,
    /// `cvtpd2ps`.
    Cvtpd2ps,
    /// `cvtss2sd`.
    Cvtss2sd,
    /// `cvtsd2ss`.
    Cvtsd2ss,
    /// `cvtdq2ps`.
    Cvtdq2ps,
    /// `cvtps2dq`.
    Cvtps2dq,
    /// `cvttps2dq`.
    Cvttps2dq,
    /// `cvttpd2dq`.
    Cvttpd2dq,
    /// `cvtdq2pd`.
    Cvtdq2pd,
    /// `cvtpd2dq`.
    Cvtpd2dq,
    /// `ucomiss`.
    Ucomiss,
    /// `ucomisd`.
    Ucomisd,
    /// `comiss`.
    Comiss,
    /// `comisd`.
    Comisd,
    /// `sqrtps`.
    Sqrtps,
    /// `sqrtpd`.
    Sqrtpd,
    /// `sqrtss`.
    Sqrtss,
    /// `sqrtsd`.
    Sqrtsd,
    /// `rsqrtps`.
    Rsqrtps,
    /// `rsqrtss`.
    Rsqrtss,
    /// `rcpps`.
    Rcpps,
    /// `rcpss`.
    Rcpss,
    /// `andps`.
    Andps,
    /// `andpd`.
    Andpd,
    /// `andnps`.
    Andnps,
    /// `andnpd`.
    Andnpd,
    /// `orps`.
    Orps,
    /// `orpd`.
    Orpd,
    /// `xorps`.
    Xorps,
    /// `xorpd`.
    Xorpd,
    /// `addps`.
    Addps,
    /// `addpd`.
    Addpd,
    /// `addss`.
    Addss,
    /// `addsd`.
    Addsd,
    /// `mulps`.
    Mulps,
    /// `mulpd`.
    Mulpd,
    /// `mulss`.
    Mulss,
    /// `mulsd`.
    Mulsd,
    /// `subps`.
    Subps,
    /// `subpd`.
    Subpd,
    /// `subss`.
    Subss,
    /// `subsd`.
    Subsd,
    /// `minps`.
    Minps,
    /// `minpd`.
    Minpd,
    /// `minss`.
    Minss,
    /// `minsd`.
    Minsd,
    /// `divps`.
    Divps,
    /// `divpd`.
    Divpd,
    /// `divss`.
    Divss,
    /// `divsd`.
    Divsd,
    /// `maxps`.
    Maxps,
    /// `maxpd`.
    Maxpd,
    /// `maxss`.
    Maxss,
    /// `maxsd`.
    Maxsd,
    /// `cmpps`, with its predicate as an immediate.
    Cmpps,
    /// `cmppd`.
    Cmppd,
    /// `cmpss`.
    Cmpss,
    /// `cmpsd` (the SSE2 compare, not the string instruction, which is [`Mnemonic::Cmps`]).
    Cmpsd,
    /// `punpcklbw`.
    Punpcklbw,
    /// `punpcklwd`.
    Punpcklwd,
    /// `punpckldq`.
    Punpckldq,
    /// `punpcklqdq`.
    Punpcklqdq,
    /// `punpckhbw`.
    Punpckhbw,
    /// `punpckhwd`.
    Punpckhwd,
    /// `punpckhdq`.
    Punpckhdq,
    /// `punpckhqdq`.
    Punpckhqdq,
    /// `packsswb`.
    Packsswb,
    /// `packssdw`.
    Packssdw,
    /// `packuswb`.
    Packuswb,
    /// `pcmpgtb`.
    Pcmpgtb,
    /// `pcmpgtw`.
    Pcmpgtw,
    /// `pcmpgtd`.
    Pcmpgtd,
    /// `pcmpeqb`.
    Pcmpeqb,
    /// `pcmpeqw`.
    Pcmpeqw,
    /// `pcmpeqd`.
    Pcmpeqd,
    /// `pshufd`.
    Pshufd,
    /// `pshufhw`.
    Pshufhw,
    /// `pshuflw`.
    Pshuflw,
    /// `psrlw`, by an immediate or an XMM count.
    Psrlw,
    /// `psrld`.
    Psrld,
    /// `psrlq`.
    Psrlq,
    /// `psraw`.
    Psraw,
    /// `psrad`.
    Psrad,
    /// `psllw`.
    Psllw,
    /// `pslld`.
    Pslld,
    /// `psllq`.
    Psllq,
    /// `psrldq`, by bytes.
    Psrldq,
    /// `pslldq`, by bytes.
    Pslldq,
    /// `pinsrw`.
    Pinsrw,
    /// `pextrw`.
    Pextrw,
    /// `pmovmskb`.
    Pmovmskb,
    /// `maskmovdqu`, storing through `rdi`.
    Maskmovdqu,
    /// `paddb`.
    Paddb,
    /// `paddw`.
    Paddw,
    /// `paddd`.
    Paddd,
    /// `paddq`.
    Paddq,
    /// `paddsb`.
    Paddsb,
    /// `paddsw`.
    Paddsw,
    /// `paddusb`.
    Paddusb,
    /// `paddusw`.
    Paddusw,
    /// `psubb`.
    Psubb,
    /// `psubw`.
    Psubw,
    /// `psubd`.
    Psubd,
    /// `psubq`.
    Psubq,
    /// `psubsb`.
    Psubsb,
    /// `psubsw`.
    Psubsw,
    /// `psubusb`.
    Psubusb,
    /// `psubusw`.
    Psubusw,
    /// `pmullw`.
    Pmullw,
    /// `pmulhw`.
    Pmulhw,
    /// `pmulhuw`.
    Pmulhuw,
    /// `pmuludq`.
    Pmuludq,
    /// `pmaddwd`.
    Pmaddwd,
    /// `psadbw`.
    Psadbw,
    /// `pavgb`.
    Pavgb,
    /// `pavgw`.
    Pavgw,
    /// `pminub`.
    Pminub,
    /// `pmaxub`.
    Pmaxub,
    /// `pminsw`.
    Pminsw,
    /// `pmaxsw`.
    Pmaxsw,
    /// `pand`.
    Pand,
    /// `pandn`.
    Pandn,
    /// `por`.
    Por,
    /// `pxor`.
    Pxor,
    /// `fld`, from memory or `st(i)`.
    Fld,
    /// `fst`.
    Fst,
    /// `fstp`.
    Fstp,
    /// `fild`.
    Fild,
    /// `fist`.
    Fist,
    /// `fistp`.
    Fistp,
    /// `fld1`.
    Fld1,
    /// `fldz`.
    Fldz,
    /// `fxch`.
    Fxch,
    /// `fchs`.
    Fchs,
    /// `fabs`.
    Fabs,
    /// `fadd`.
    Fadd,
    /// `faddp`.
    Faddp,
    /// `fsub`.
    Fsub,
    /// `fsubp`.
    Fsubp,
    /// `fsubr`.
    Fsubr,
    /// `fsubrp`.
    Fsubrp,
    /// `fmul`.
    Fmul,
    /// `fmulp`.
    Fmulp,
    /// `fdiv`.
    Fdiv,
    /// `fdivp`.
    Fdivp,
    /// `fdivr`.
    Fdivr,
    /// `fdivrp`.
    Fdivrp,
    /// `fcomi`.
    Fcomi,
    /// `fcomip`.
    Fcomip,
    /// `fucomi`.
    Fucomi,
    /// `fucomip`.
    Fucomip,
    /// `fldcw`.
    Fldcw,
    /// `fnstcw`.
    Fnstcw,
    /// `fnstsw`, to memory or `ax`.
    Fnstsw,
    /// `fnclex`.
    Fnclex,
    /// `fninit`.
    Fninit,
    /// `fwait`.
    Fwait,
}

impl Mnemonic {
    /// `movs`, `cmps`, `stos`, `lods` or `scas`: the instructions a repeat prefix applies to.
    pub const fn is_string(self) -> bool {
        matches!(
            self,
            Mnemonic::Movs | Mnemonic::Cmps | Mnemonic::Stos | Mnemonic::Lods | Mnemonic::Scas
        )
    }

    /// An instruction whose implicit addresses or counter follow the address size: the string
    /// instructions, `xlat`, `maskmovdqu`, the loops and `jrcxz`.
    pub const fn has_implicit_address(self) -> bool {
        self.is_string()
            || matches!(
                self,
                Mnemonic::Xlat
                    | Mnemonic::Maskmovdqu
                    | Mnemonic::Loop
                    | Mnemonic::Loope
                    | Mnemonic::Loopne
                    | Mnemonic::Jrcxz
            )
    }
}
