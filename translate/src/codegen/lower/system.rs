use super::{
    CarrySense, Flags, Instruction, Logic, Lowered, Lowerer, Mnemonic, Operand, Place, Reg,
    SCRATCH, Shift, Size, T0, T1, T2, T3, T4, T6, Width, encode, state, state_access,
};
use crate::codegen::aarch64::encode::{AddSub, LoadStore, Multiply, OneSource, Ordered, TwoSource};
use crate::codegen::aarch64::{Access, simd};
use crate::codegen::registers::{ADDRESS, GUEST_STACK, guest};
use crate::instruction::Gpr;

const RAX: Reg = guest(Gpr::RAX);
const RCX: Reg = guest(Gpr::RCX);
const RDX: Reg = guest(Gpr::RDX);
const RBX: Reg = guest(Gpr::RBX);

const BRAND: &[u8] = b"EggOS x86-64 translated to Arm";

const fn brand_word(index: usize) -> u32 {
    let mut word = 0u32;
    let mut byte = 0;
    while byte < 4 {
        let at = index * 4 + byte;
        if at < BRAND.len() {
            word |= (BRAND[at] as u32) << (8 * byte);
        }
        byte += 1;
    }
    word
}

const fn brand_leaf(leaf: usize) -> [u32; 4] {
    [
        brand_word(leaf * 4),
        brand_word(leaf * 4 + 1),
        brand_word(leaf * 4 + 2),
        brand_word(leaf * 4 + 3),
    ]
}

/// The translated `cpuid`: each leaf with `[eax, ebx, ecx, edx]`; any other leaf returns
/// zeros. Leaf 1 reports FPU, TSC, CX8, CMOV, CLFSH, SSE and SSE2 in `edx` and CX16 and POPCNT
/// in `ecx`; leaf 7 reports nothing (no BMI1, no AVX2); leaf `0x8000_0001` reports LAHF/SAHF
/// and LZCNT (ABM) in `ecx` and SYSCALL, NX and long mode in `edx`.
pub(super) const CPUID_LEAVES: [(u32, [u32; 4]); 8] = [
    (0, [7, 0x756E_6547, 0x6C65_746E, 0x4965_6E69]),
    (1, [0x0000_06F6, 0x0001_0800, 0x0080_2000, 0x0608_8111]),
    (7, [0, 0, 0, 0]),
    (0x8000_0000, [0x8000_0004, 0, 0, 0]),
    (0x8000_0001, [0, 0, 0x0000_0021, 0x2010_0800]),
    (0x8000_0002, brand_leaf(0)),
    (0x8000_0003, brand_leaf(1)),
    (0x8000_0004, brand_leaf(2)),
];

const TIME_STAMP_HZ: u64 = 1_000_000_000;
const MXCSR_DENORMALS_ARE_ZERO: u8 = 6;
const MXCSR_ROUNDING_LOW: u8 = 13;
const MXCSR_ROUNDING_HIGH: u8 = 14;
const MXCSR_FLUSH_TO_ZERO: u8 = 15;
const FPCR_ROUNDING_LOW: u8 = 22;
const FPCR_ROUNDING_HIGH: u8 = 23;
const FPCR_FLUSH_TO_ZERO: u8 = 24;
const NZCV_BITS: [(u8, u8); 3] = [(0, 29), (6, 30), (7, 31)];
const OVERFLOW_FLAG_BIT: u8 = 11;
const DIRECTION_FLAG_BIT: u8 = 10;
const RFLAGS_FIXED: u64 = 0x202;
const LAHF_FIXED: u64 = 0x2;

impl Lowerer {
    pub(super) fn system(&mut self, instruction: &Instruction) -> Lowered<bool> {
        use Mnemonic as M;
        match instruction.mnemonic {
            M::Cpuid => self.cpuid(),
            M::Rdtsc | M::Rdtscp => self.time_stamp(instruction.mnemonic == M::Rdtscp),
            M::Xgetbv => {
                self.move_immediate(Size::W, RAX, 3);
                self.move_immediate(Size::W, RDX, 0);
            }
            M::Lahf => {
                self.flags_word(T0, false);
                self.bfi(RAX, T0, 8, 8);
            }
            M::Sahf => {
                self.ubfx(Size::W, T0, RAX, 8, 8);
                self.load_flags(T0, false);
            }
            M::Pushf => {
                self.flags_word(T0, true);
                let bytes = instruction.width.bytes();
                self.emit(encode::add_sub_immediate(
                    AddSub::Sub,
                    Size::X,
                    GUEST_STACK,
                    GUEST_STACK,
                    u16::from(bytes),
                    false,
                ));
                self.store_memory(GUEST_STACK, instruction.width, T0);
            }
            M::Popf => {
                self.load_memory(GUEST_STACK, instruction.width, T0);
                self.emit(encode::add_sub_immediate(
                    AddSub::Add,
                    Size::X,
                    GUEST_STACK,
                    GUEST_STACK,
                    u16::from(instruction.width.bytes()),
                    false,
                ));
                self.load_flags(T0, true);
            }
            M::Cld => self.emit(state_access(LoadStore::Strb, Reg::ZR, state::DIRECTION)),
            M::Std => {
                self.move_immediate(Size::W, SCRATCH, 1);
                self.emit(state_access(LoadStore::Strb, SCRATCH, state::DIRECTION));
            }
            M::Ldmxcsr => self.load_mxcsr(instruction)?,
            M::Stmxcsr => {
                self.emit(state_access(LoadStore::LdrW, T0, state::MXCSR));
                let place = self.place(&self.operand(instruction, 0)?)?;
                self.store(place, T0);
            }
            M::Clflush => match self.place(&self.operand(instruction, 0)?)? {
                Place::Memory { address, .. } => self.emit(simd::dc_civac(address)),
                _ => return Err(self.operands_error()),
            },
            M::Movnti => self.mov(instruction)?,
            M::Cmpxchg8b | M::Cmpxchg16b => self.compare_exchange_wide(instruction)?,
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn cpuid(&mut self) {
        let done = self.asm.label();
        for (leaf, values) in CPUID_LEAVES {
            let next = self.asm.label();
            if leaf == 0 {
                self.asm.cbz(Size::W, RAX, next, true);
            } else {
                self.move_immediate(Size::W, SCRATCH, u64::from(leaf));
                self.emit(encode::logical_shifted(
                    Logic::Eor,
                    false,
                    Size::W,
                    SCRATCH,
                    RAX,
                    SCRATCH,
                    Shift::Lsl,
                    0,
                ));
                self.asm.cbz(Size::W, SCRATCH, next, true);
            }
            self.cpuid_result(values);
            self.asm.b(done);
            self.asm.bind(next);
        }
        self.cpuid_result([0; 4]);
        self.asm.bind(done);
    }

    fn cpuid_result(&mut self, values: [u32; 4]) {
        for (register, value) in [RAX, RBX, RCX, RDX].into_iter().zip(values) {
            self.move_immediate(Size::W, register, u64::from(value));
        }
    }

    fn time_stamp(&mut self, with_processor: bool) {
        let scaled = self.asm.label();
        self.emit(simd::mrs_cntvct(SCRATCH));
        self.emit(simd::mrs_cntfrq(ADDRESS));
        self.move_immediate(Size::W, T0, TIME_STAMP_HZ);
        self.emit(encode::two_source(
            TwoSource::Udiv,
            Size::X,
            T0,
            T0,
            ADDRESS,
        ));
        self.asm.cbz(Size::X, T0, scaled, true);
        self.move_immediate(Size::W, T0, 1);
        self.asm.bind(scaled);
        self.emit(encode::multiply(
            Multiply::Madd,
            Size::X,
            SCRATCH,
            SCRATCH,
            T0,
            Reg::ZR,
        ));
        self.copy(Size::W, RAX, SCRATCH);
        self.shift_right(Size::X, RDX, SCRATCH, 32);
        if with_processor {
            self.move_immediate(Size::W, RCX, 0);
        }
    }

    fn flags_word(&mut self, into: Reg, full: bool) {
        self.ensure_carry(CarrySense::Direct);
        self.emit(encode::mrs_nzcv(SCRATCH));
        self.move_immediate(Size::W, into, if full { RFLAGS_FIXED } else { LAHF_FIXED });
        for (flag, nzcv) in NZCV_BITS {
            self.ubfx(Size::W, T4, SCRATCH, nzcv, 1);
            self.bfi(into, T4, flag, 1);
        }
        if full {
            self.ubfx(Size::W, T4, SCRATCH, 28, 1);
            self.bfi(into, T4, OVERFLOW_FLAG_BIT, 1);
            self.emit(state_access(LoadStore::Ldrb, T4, state::DIRECTION));
            self.bfi(into, T4, DIRECTION_FLAG_BIT, 1);
        }
        self.parity_odd(T4);
        self.logic_immediate(Logic::Eor, Size::W, T4, T4, 1);
        self.bfi(into, T4, 2, 1);
        self.emit(state_access(LoadStore::Ldrb, T4, state::ADJUST));
        self.ubfx(Size::W, T4, T4, 4, 1);
        self.bfi(into, T4, 4, 1);
    }

    fn load_flags(&mut self, value: Reg, full: bool) {
        self.emit(encode::mrs_nzcv(SCRATCH));
        for (flag, nzcv) in NZCV_BITS {
            self.ubfx(Size::W, T4, value, flag, 1);
            self.bfi(SCRATCH, T4, nzcv, 1);
        }
        if full {
            self.ubfx(Size::W, T4, value, OVERFLOW_FLAG_BIT, 1);
            self.bfi(SCRATCH, T4, 28, 1);
            self.ubfx(Size::W, T4, value, DIRECTION_FLAG_BIT, 1);
            self.emit(state_access(LoadStore::Strb, T4, state::DIRECTION));
        }
        self.emit(encode::msr_nzcv(SCRATCH));
        self.carry = CarrySense::Direct;
        if self.live.any(Flags::PF) {
            self.ubfx(Size::W, T4, value, 2, 1);
            self.logic_immediate(Logic::Eor, Size::W, T4, T4, 1);
            self.emit(state_access(LoadStore::Strb, T4, state::PARITY));
        }
        if self.live.any(Flags::AF) {
            self.logic_immediate(Logic::And, Size::W, T4, value, 0x10);
            self.emit(state_access(LoadStore::Strb, T4, state::ADJUST));
        }
    }

    fn load_mxcsr(&mut self, instruction: &Instruction) -> Lowered {
        let place = self.place(&self.operand(instruction, 0)?)?;
        let value = self.load(place, T0);
        self.emit(state_access(LoadStore::StrW, value, state::MXCSR));
        self.emit(simd::mrs_fpcr(SCRATCH));
        self.ubfx(Size::W, T1, value, MXCSR_ROUNDING_HIGH, 1);
        self.bfi(SCRATCH, T1, FPCR_ROUNDING_LOW, 1);
        self.ubfx(Size::W, T1, value, MXCSR_ROUNDING_LOW, 1);
        self.bfi(SCRATCH, T1, FPCR_ROUNDING_HIGH, 1);
        self.ubfx(Size::W, T1, value, MXCSR_FLUSH_TO_ZERO, 1);
        self.ubfx(Size::W, T2, value, MXCSR_DENORMALS_ARE_ZERO, 1);
        self.emit(encode::logical_shifted(
            Logic::Orr,
            false,
            Size::W,
            T1,
            T1,
            T2,
            Shift::Lsl,
            0,
        ));
        self.bfi(SCRATCH, T1, FPCR_FLUSH_TO_ZERO, 1);
        self.emit(simd::msr_fpcr(SCRATCH));
        Ok(())
    }

    fn compare_exchange_wide(&mut self, instruction: &Instruction) -> Lowered {
        let memory = match self.operand(instruction, 0)? {
            Operand::Memory(memory) => memory,
            _ => return Err(self.operands_error()),
        };
        let address = self.effective_address(&memory, ADDRESS, memory.segment);
        let wide = memory.width == Width::Dqword;
        if wide {
            self.exchange_pair(address);
        } else {
            self.exchange_joined(address);
        }
        let equal = self.asm.label();
        self.asm.cbz(Size::X, SCRATCH, equal, false);
        if wide {
            self.copy(Size::X, RAX, T0);
            self.copy(Size::X, RDX, T1);
        } else {
            self.copy(Size::W, RAX, T0);
            self.shift_right(Size::X, RDX, T0, 32);
        }
        self.asm.bind(equal);
        if self.defined_and_live(instruction).any(Flags::ZF) {
            self.emit(encode::one_source(
                OneSource::Clz,
                Size::X,
                SCRATCH,
                SCRATCH,
            ));
            self.shift_right(Size::X, SCRATCH, SCRATCH, 6);
            self.emit(encode::mrs_nzcv(T6));
            self.bfi(T6, SCRATCH, 30, 1);
            self.emit(encode::msr_nzcv(T6));
        }
        Ok(())
    }

    fn differs(&mut self, low: Reg, high: Reg, expected_low: Reg, expected_high: Reg) {
        let eor =
            |d, n, m| encode::logical_shifted(Logic::Eor, false, Size::X, d, n, m, Shift::Lsl, 0);
        self.emit(eor(SCRATCH, low, expected_low));
        self.emit(eor(T2, high, expected_high));
        self.emit(encode::logical_shifted(
            Logic::Orr,
            false,
            Size::X,
            SCRATCH,
            SCRATCH,
            T2,
            Shift::Lsl,
            0,
        ));
    }

    fn exchange_pair(&mut self, address: Reg) {
        if self.mode.lse {
            self.copy(Size::X, T0, RAX);
            self.copy(Size::X, T1, RDX);
            self.copy(Size::X, T2, RBX);
            self.copy(Size::X, T3, RCX);
            self.emit(simd::caspal(T0, T2, address));
            self.differs(T0, T1, RAX, RDX);
            return;
        }
        let retry = self.asm.label();
        let failed = self.asm.label();
        let done = self.asm.label();
        self.asm.bind(retry);
        self.emit(simd::ldaxp(T0, T1, address));
        self.differs(T0, T1, RAX, RDX);
        self.asm.cbz(Size::X, SCRATCH, failed, true);
        self.emit(simd::stlxp(T2, RBX, RCX, address));
        self.asm.cbz(Size::W, T2, retry, true);
        self.asm.b(done);
        self.asm.bind(failed);
        self.emit(encode::clrex());
        self.asm.bind(done);
    }

    fn exchange_joined(&mut self, address: Reg) {
        self.copy(Size::W, T0, RAX);
        self.bfi(T0, RDX, 32, 32);
        self.copy(Size::W, T1, RBX);
        self.bfi(T1, RCX, 32, 32);
        self.copy(Size::X, T2, T0);
        if self.mode.lse {
            self.emit(encode::casal(Access::X, T0, T1, address));
        } else {
            let retry = self.asm.label();
            let failed = self.asm.label();
            let done = self.asm.label();
            self.asm.bind(retry);
            self.emit(encode::ordered(Ordered::Ldaxr, Access::X, T0, address));
            self.emit(encode::logical_shifted(
                Logic::Eor,
                false,
                Size::X,
                SCRATCH,
                T0,
                T2,
                Shift::Lsl,
                0,
            ));
            self.asm.cbz(Size::X, SCRATCH, failed, true);
            self.emit(encode::stlxr(Access::X, T3, T1, address));
            self.asm.cbz(Size::W, T3, retry, true);
            self.asm.b(done);
            self.asm.bind(failed);
            self.emit(encode::clrex());
            self.asm.bind(done);
        }
        self.emit(encode::logical_shifted(
            Logic::Eor,
            false,
            Size::X,
            SCRATCH,
            T0,
            T2,
            Shift::Lsl,
            0,
        ));
    }
}
