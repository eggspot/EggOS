//! DWARF call frame information (D234), the `__TEXT,__eh_frame` section a Mach-O image carries
//! for the functions its compact unwind information defers to DWARF: common information
//! entries (CIEs) and frame description entries (FDEs) as the DWARF 4 and 5 specifications and
//! the System V and ARM64 ABIs define them, the call frame program that gives each address's
//! row of rules, and [`apply`], which turns a frame's registers into its caller's with a row.
//! The tables are the program's, so untrusted: every read is bounded by the section, programs
//! by an instruction count, `DW_CFA_remember_state` by a depth and expressions by an operation
//! count and a stack. Pure logic with host tests.

const MAX_INSTRUCTIONS: usize = 4096;
const MAX_REMEMBERED: usize = 4;
const MAX_EXPRESSION_OPERATIONS: usize = 256;
const MAX_EXPRESSION_STACK: usize = 32;
const MAX_AUGMENTATION: usize = 8;
const MAX_SCANNED_ENTRIES: usize = 1 << 20;
const TRACKED_REGISTERS: usize = 80;
const EXTENDED_LENGTH: u32 = 0xffff_ffff;

const OMIT: u8 = 0xff;
const FORMAT_MASK: u8 = 0x0f;
const APPLICATION_MASK: u8 = 0x70;
const INDIRECT: u8 = 0x80;
const ABSOLUTE_POINTER: u8 = 0x00;
const ULEB128: u8 = 0x01;
const UDATA2: u8 = 0x02;
const UDATA4: u8 = 0x03;
const UDATA8: u8 = 0x04;
const SLEB128: u8 = 0x09;
const SDATA2: u8 = 0x0a;
const SDATA4: u8 = 0x0b;
const SDATA8: u8 = 0x0c;
const ABSOLUTE: u8 = 0x00;
const PC_RELATIVE: u8 = 0x10;

const CFA_ADVANCE_LOC: u8 = 0x40;
const CFA_OFFSET: u8 = 0x80;
const CFA_RESTORE: u8 = 0xc0;
const CFA_NOP: u8 = 0x00;
const CFA_SET_LOC: u8 = 0x01;
const CFA_ADVANCE_LOC1: u8 = 0x02;
const CFA_ADVANCE_LOC2: u8 = 0x03;
const CFA_ADVANCE_LOC4: u8 = 0x04;
const CFA_OFFSET_EXTENDED: u8 = 0x05;
const CFA_RESTORE_EXTENDED: u8 = 0x06;
const CFA_UNDEFINED: u8 = 0x07;
const CFA_SAME_VALUE: u8 = 0x08;
const CFA_REGISTER: u8 = 0x09;
const CFA_REMEMBER_STATE: u8 = 0x0a;
const CFA_RESTORE_STATE: u8 = 0x0b;
const CFA_DEF_CFA: u8 = 0x0c;
const CFA_DEF_CFA_REGISTER: u8 = 0x0d;
const CFA_DEF_CFA_OFFSET: u8 = 0x0e;
const CFA_DEF_CFA_EXPRESSION: u8 = 0x0f;
const CFA_EXPRESSION: u8 = 0x10;
const CFA_OFFSET_EXTENDED_SF: u8 = 0x11;
const CFA_DEF_CFA_SF: u8 = 0x12;
const CFA_DEF_CFA_OFFSET_SF: u8 = 0x13;
const CFA_VAL_OFFSET: u8 = 0x14;
const CFA_VAL_OFFSET_SF: u8 = 0x15;
const CFA_VAL_EXPRESSION: u8 = 0x16;
const CFA_AARCH64_NEGATE_RA_STATE: u8 = 0x2d;
const CFA_GNU_ARGS_SIZE: u8 = 0x2e;
const CFA_GNU_NEGATIVE_OFFSET_EXTENDED: u8 = 0x2f;

const OP_ADDR: u8 = 0x03;
const OP_DEREF: u8 = 0x06;
const OP_CONST1U: u8 = 0x08;
const OP_CONST1S: u8 = 0x09;
const OP_CONST2U: u8 = 0x0a;
const OP_CONST2S: u8 = 0x0b;
const OP_CONST4U: u8 = 0x0c;
const OP_CONST4S: u8 = 0x0d;
const OP_CONST8U: u8 = 0x0e;
const OP_CONST8S: u8 = 0x0f;
const OP_CONSTU: u8 = 0x10;
const OP_CONSTS: u8 = 0x11;
const OP_DUP: u8 = 0x12;
const OP_DROP: u8 = 0x13;
const OP_OVER: u8 = 0x14;
const OP_PICK: u8 = 0x15;
const OP_SWAP: u8 = 0x16;
const OP_ROT: u8 = 0x17;
const OP_ABS: u8 = 0x19;
const OP_AND: u8 = 0x1a;
const OP_DIV: u8 = 0x1b;
const OP_MINUS: u8 = 0x1c;
const OP_MOD: u8 = 0x1d;
const OP_MUL: u8 = 0x1e;
const OP_NEG: u8 = 0x1f;
const OP_NOT: u8 = 0x20;
const OP_OR: u8 = 0x21;
const OP_PLUS: u8 = 0x22;
const OP_PLUS_UCONST: u8 = 0x23;
const OP_SHL: u8 = 0x24;
const OP_SHR: u8 = 0x25;
const OP_SHRA: u8 = 0x26;
const OP_XOR: u8 = 0x27;
const OP_BRA: u8 = 0x28;
const OP_EQ: u8 = 0x29;
const OP_GE: u8 = 0x2a;
const OP_GT: u8 = 0x2b;
const OP_LE: u8 = 0x2c;
const OP_LT: u8 = 0x2d;
const OP_NE: u8 = 0x2e;
const OP_SKIP: u8 = 0x2f;
const OP_LIT0: u8 = 0x30;
const OP_LIT31: u8 = 0x4f;
const OP_BREG0: u8 = 0x70;
const OP_BREG31: u8 = 0x8f;
const OP_BREGX: u8 = 0x92;
const OP_DEREF_SIZE: u8 = 0x94;
const OP_NOP: u8 = 0x96;

/// Why a table could not be read or a frame not stepped over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CfiError {
    /// A field runs past its entry or the section.
    Truncated,
    /// A field holds a value the format does not allow.
    Malformed,
    /// A valid construct this reader does not handle (an encoding, operation or version).
    Unsupported,
    /// A program, expression or state stack passed its limit.
    TooLong,
    /// No FDE covers the address.
    NotCovered,
    /// A word of memory a rule names could not be read.
    Unreadable,
    /// The return address is undefined: the outermost frame.
    EndOfStack,
}

/// A common information entry: what the FDEs that point at it share.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cie {
    code_alignment: u64,
    data_alignment: i64,
    /// The DWARF number of the column that holds the return address.
    pub return_register: u16,
    pointer_encoding: u8,
    lsda_encoding: u8,
    augmented: bool,
    /// The address of the personality routine, when the CIE names one.
    pub personality: Option<u64>,
    instructions: (usize, usize),
}

/// A frame description entry: one function's address range, LSDA and call frame program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fde {
    pub start: u64,
    pub end: u64,
    /// Its language-specific data area's address, when it has one.
    pub lsda: Option<u64>,
    pub cie: Cie,
    instructions: (usize, usize),
}

/// How to recover one register of the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// The caller's value is the frame's.
    Unchanged,
    /// The caller's value cannot be recovered.
    Undefined,
    /// Saved at the CFA plus this offset.
    Offset(i64),
    /// The CFA plus this offset is the value.
    ValueOffset(i64),
    /// Held in this other register of the frame.
    Register(u16),
    /// Saved at the address an expression at this place of the section computes.
    Expression(u32, u32),
    /// The value an expression at this place of the section computes.
    ValueExpression(u32, u32),
}

/// How to compute the canonical frame address, the caller's stack pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CfaRule {
    RegisterOffset(u16, i64),
    Expression(u32, u32),
}

/// The rules in force at one address of a function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub cfa: CfaRule,
    rules: [Rule; TRACKED_REGISTERS],
}

impl Row {
    const EMPTY: Self = Self {
        cfa: CfaRule::RegisterOffset(0, 0),
        rules: [Rule::Unchanged; TRACKED_REGISTERS],
    };

    /// The rule for register `number`; registers past the tracked ones are unchanged.
    pub fn rule(&self, number: u16) -> Rule {
        self.rules
            .get(usize::from(number))
            .copied()
            .unwrap_or(Rule::Unchanged)
    }

    fn set(&mut self, number: u64, rule: Rule) {
        if let Some(slot) = usize::try_from(number)
            .ok()
            .and_then(|index| self.rules.get_mut(index))
        {
            *slot = rule;
        }
    }
}

/// A frame's registers by their DWARF numbers, as [`apply`] reads and writes them.
pub trait CfiRegisters: Copy {
    /// The stack pointer's DWARF number.
    const STACK_POINTER: u16;
    /// Register `number`'s value, `None` for one the frame does not hold.
    fn get(&self, number: u16) -> Option<u64>;
    /// Sets register `number`, ignoring one the frame does not hold.
    fn set(&mut self, number: u16, value: u64);
    /// Sets the program counter.
    fn set_program_counter(&mut self, value: u64);
}

#[cfg(any(test, target_arch = "aarch64"))]
impl CfiRegisters for super::unwind_info::Arm64Registers {
    const STACK_POINTER: u16 = 31;

    fn get(&self, number: u16) -> Option<u64> {
        match number {
            0..=30 => Some(self.x[usize::from(number)]),
            31 => Some(self.sp),
            72..=79 => Some(self.d[usize::from(number - 72)]),
            _ => None,
        }
    }

    fn set(&mut self, number: u16, value: u64) {
        match number {
            0..=30 => self.x[usize::from(number)] = value,
            31 => self.sp = value,
            72..=79 => self.d[usize::from(number - 72)] = value,
            _ => {}
        }
    }

    fn set_program_counter(&mut self, value: u64) {
        self.pc = value;
    }
}

#[cfg(any(test, target_arch = "x86_64"))]
impl CfiRegisters for super::unwind_info::X86_64Registers {
    const STACK_POINTER: u16 = 7;

    fn get(&self, number: u16) -> Option<u64> {
        match number {
            0..=15 => Some(self.general[usize::from(number)]),
            16 => Some(self.rip),
            _ => None,
        }
    }

    fn set(&mut self, number: u16, value: u64) {
        match number {
            0..=15 => self.general[usize::from(number)] = value,
            16 => self.rip = value,
            _ => {}
        }
    }

    fn set_program_counter(&mut self, value: u64) {
        self.rip = value;
    }
}

struct Cursor<'s> {
    section: &'s [u8],
    base: u64,
    at: usize,
    end: usize,
}

impl Cursor<'_> {
    fn byte(&mut self) -> Result<u8, CfiError> {
        if self.at >= self.end {
            return Err(CfiError::Truncated);
        }
        let byte = *self.section.get(self.at).ok_or(CfiError::Truncated)?;
        self.at += 1;
        Ok(byte)
    }

    fn fixed(&mut self, bytes: u32) -> Result<u64, CfiError> {
        let mut value = 0u64;
        for index in 0..bytes {
            value |= u64::from(self.byte()?) << (8 * index);
        }
        Ok(value)
    }

    fn unsigned(&mut self) -> Result<u64, CfiError> {
        let mut value = 0u64;
        for group in 0..10 {
            let byte = self.byte()?;
            value |= u64::from(byte & 0x7f).checked_shl(7 * group).unwrap_or(0);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(CfiError::Malformed)
    }

    fn signed(&mut self) -> Result<i64, CfiError> {
        let mut value = 0i64;
        for group in 0..10u32 {
            let byte = self.byte()?;
            value |= i64::from(byte & 0x7f).checked_shl(7 * group).unwrap_or(0);
            if byte & 0x80 == 0 {
                let shift = 7 * (group + 1);
                if shift < 64 && byte & 0x40 != 0 {
                    value |= -1i64 << shift;
                }
                return Ok(value);
            }
        }
        Err(CfiError::Malformed)
    }

    fn register(&mut self) -> Result<u16, CfiError> {
        u16::try_from(self.unsigned()?).map_err(|_| CfiError::Malformed)
    }

    fn address(&self) -> u64 {
        self.base.wrapping_add(self.at as u64)
    }

    fn value(&mut self, format: u8) -> Result<u64, CfiError> {
        Ok(match format {
            ABSOLUTE_POINTER | UDATA8 | SDATA8 => self.fixed(8)?,
            ULEB128 => self.unsigned()?,
            UDATA2 => self.fixed(2)?,
            UDATA4 => self.fixed(4)?,
            SLEB128 => self.signed()? as u64,
            SDATA2 => self.fixed(2)? as u16 as i16 as i64 as u64,
            SDATA4 => self.fixed(4)? as u32 as i32 as i64 as u64,
            _ => return Err(CfiError::Unsupported),
        })
    }

    fn pointer(
        &mut self,
        encoding: u8,
        read: &dyn Fn(u64) -> Option<u64>,
    ) -> Result<Option<u64>, CfiError> {
        if encoding == OMIT {
            return Ok(None);
        }
        let position = self.address();
        let value = self.value(encoding & FORMAT_MASK)?;
        if value == 0 {
            return Ok(Some(0));
        }
        let value = match encoding & APPLICATION_MASK {
            ABSOLUTE => value,
            PC_RELATIVE => position.wrapping_add(value),
            _ => return Err(CfiError::Unsupported),
        };
        match encoding & INDIRECT {
            0 => Ok(Some(value)),
            _ => read(value).map(Some).ok_or(CfiError::Unreadable),
        }
    }

    fn block(&mut self) -> Result<(u32, u32), CfiError> {
        let length = usize::try_from(self.unsigned()?).map_err(|_| CfiError::Malformed)?;
        let start = self.at;
        let end = start.checked_add(length).ok_or(CfiError::Truncated)?;
        if end > self.end {
            return Err(CfiError::Truncated);
        }
        self.at = end;
        Ok((
            u32::try_from(start).map_err(|_| CfiError::Malformed)?,
            u32::try_from(length).map_err(|_| CfiError::Malformed)?,
        ))
    }
}

fn word_at(section: &[u8], at: usize) -> Result<u32, CfiError> {
    let bytes = section
        .get(at..at.checked_add(4).ok_or(CfiError::Truncated)?)
        .ok_or(CfiError::Truncated)?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

struct Entry {
    content: usize,
    end: usize,
}

fn entry(section: &[u8], offset: usize) -> Result<Option<Entry>, CfiError> {
    let length = word_at(section, offset)?;
    if length == 0 {
        return Ok(None);
    }
    let (content, length) = match length {
        EXTENDED_LENGTH => {
            let low = word_at(section, offset.checked_add(4).ok_or(CfiError::Truncated)?)?;
            let high = word_at(section, offset.checked_add(8).ok_or(CfiError::Truncated)?)?;
            let length = (u64::from(high) << 32) | u64::from(low);
            (
                offset + 12,
                usize::try_from(length).map_err(|_| CfiError::Truncated)?,
            )
        }
        length => (offset + 4, length as usize),
    };
    let end = content.checked_add(length).ok_or(CfiError::Truncated)?;
    if end > section.len() || length < 4 {
        return Err(CfiError::Truncated);
    }
    Ok(Some(Entry { content, end }))
}

/// Reads the CIE at `offset` of the section `section`, mapped at `base`; `read` reads a word
/// of memory for indirect pointers.
pub fn cie_at(
    section: &[u8],
    base: u64,
    offset: usize,
    read: &dyn Fn(u64) -> Option<u64>,
) -> Result<Cie, CfiError> {
    let Entry { content, end } = entry(section, offset)?.ok_or(CfiError::Malformed)?;
    if word_at(section, content)? != 0 {
        return Err(CfiError::Malformed);
    }
    let mut cursor = Cursor {
        section,
        base,
        at: content + 4,
        end,
    };
    let version = cursor.byte()?;
    if !matches!(version, 1 | 3 | 4) {
        return Err(CfiError::Unsupported);
    }
    let mut augmentation = [0u8; MAX_AUGMENTATION];
    let mut length = 0;
    loop {
        let byte = cursor.byte()?;
        if byte == 0 {
            break;
        }
        *augmentation.get_mut(length).ok_or(CfiError::Unsupported)? = byte;
        length += 1;
    }
    let augmentation = &augmentation[..length];
    if version == 4 {
        let address_size = cursor.byte()?;
        let segment_size = cursor.byte()?;
        if address_size != 8 || segment_size != 0 {
            return Err(CfiError::Unsupported);
        }
    }
    let code_alignment = cursor.unsigned()?;
    let data_alignment = cursor.signed()?;
    let return_register = match version {
        1 => u16::from(cursor.byte()?),
        _ => cursor.register()?,
    };
    let mut cie = Cie {
        code_alignment,
        data_alignment,
        return_register,
        pointer_encoding: ABSOLUTE_POINTER,
        lsda_encoding: OMIT,
        augmented: false,
        personality: None,
        instructions: (0, 0),
    };
    match augmentation.first() {
        None => {}
        Some(b'z') => {
            cie.augmented = true;
            let data_length =
                usize::try_from(cursor.unsigned()?).map_err(|_| CfiError::Truncated)?;
            let data_end = cursor
                .at
                .checked_add(data_length)
                .filter(|&data_end| data_end <= end)
                .ok_or(CfiError::Truncated)?;
            for &letter in &augmentation[1..] {
                match letter {
                    b'L' => cie.lsda_encoding = cursor.byte()?,
                    b'P' => {
                        let encoding = cursor.byte()?;
                        cie.personality = cursor.pointer(encoding, read)?.filter(|&at| at != 0);
                    }
                    b'R' => cie.pointer_encoding = cursor.byte()?,
                    b'S' | b'B' => {}
                    _ => break,
                }
                if cursor.at > data_end {
                    return Err(CfiError::Truncated);
                }
            }
            cursor.at = data_end;
        }
        Some(_) => return Err(CfiError::Unsupported),
    }
    cie.instructions = (cursor.at, end);
    Ok(cie)
}

/// Reads the FDE at `offset` of the section (the offset a compact unwind encoding that
/// defers to DWARF carries) and its CIE.
pub fn fde_at(
    section: &[u8],
    base: u64,
    offset: usize,
    read: &dyn Fn(u64) -> Option<u64>,
) -> Result<Fde, CfiError> {
    let Entry { content, end } = entry(section, offset)?.ok_or(CfiError::Malformed)?;
    let pointer = word_at(section, content)? as usize;
    if pointer == 0 {
        return Err(CfiError::Malformed);
    }
    let cie_offset = content.checked_sub(pointer).ok_or(CfiError::Malformed)?;
    let cie = cie_at(section, base, cie_offset, read)?;
    let mut cursor = Cursor {
        section,
        base,
        at: content + 4,
        end,
    };
    let start = cursor
        .pointer(cie.pointer_encoding, read)?
        .ok_or(CfiError::Malformed)?;
    let range = cursor.value(cie.pointer_encoding & FORMAT_MASK)?;
    let function_end = start.checked_add(range).ok_or(CfiError::Malformed)?;
    let mut lsda = None;
    if cie.augmented {
        let data_length = usize::try_from(cursor.unsigned()?).map_err(|_| CfiError::Truncated)?;
        let data_end = cursor
            .at
            .checked_add(data_length)
            .filter(|&data_end| data_end <= end)
            .ok_or(CfiError::Truncated)?;
        if cie.lsda_encoding != OMIT {
            lsda = cursor
                .pointer(cie.lsda_encoding, read)?
                .filter(|&at| at != 0);
            if cursor.at > data_end {
                return Err(CfiError::Truncated);
            }
        }
        cursor.at = data_end;
    }
    Ok(Fde {
        start,
        end: function_end,
        lsda,
        cie,
        instructions: (cursor.at, end),
    })
}

/// Finds the FDE covering `pc` by reading every entry of the section in turn, for an image
/// whose compact unwind information does not name one; an FDE (or its CIE) that cannot be read
/// is passed over.
pub fn find(
    section: &[u8],
    base: u64,
    pc: u64,
    read: &dyn Fn(u64) -> Option<u64>,
) -> Result<Fde, CfiError> {
    let mut offset = 0;
    for _ in 0..MAX_SCANNED_ENTRIES {
        if offset >= section.len() {
            return Err(CfiError::NotCovered);
        }
        let Some(Entry { content, end }) = entry(section, offset)? else {
            return Err(CfiError::NotCovered);
        };
        if word_at(section, content)? != 0
            && let Ok(fde) = fde_at(section, base, offset, read)
            && (fde.start..fde.end).contains(&pc)
        {
            return Ok(fde);
        }
        offset = end;
    }
    Err(CfiError::TooLong)
}

struct Machine<'s> {
    section: &'s [u8],
    base: u64,
    cie: &'s Cie,
    location: u64,
    target: u64,
    budget: usize,
    remembered: [Row; MAX_REMEMBERED],
    depth: usize,
}

impl Machine<'_> {
    fn offset(&self, factored: i64) -> Result<i64, CfiError> {
        factored
            .checked_mul(self.cie.data_alignment)
            .ok_or(CfiError::Malformed)
    }

    fn unsigned_offset(&self, factored: u64) -> Result<i64, CfiError> {
        self.offset(i64::try_from(factored).map_err(|_| CfiError::Malformed)?)
    }

    fn advance(&mut self, delta: u64) -> Result<bool, CfiError> {
        let step = delta
            .checked_mul(self.cie.code_alignment)
            .ok_or(CfiError::Malformed)?;
        self.location = self.location.checked_add(step).ok_or(CfiError::Malformed)?;
        Ok(self.location > self.target)
    }

    fn run(
        &mut self,
        (start, end): (usize, usize),
        row: &mut Row,
        initial: Option<&Row>,
    ) -> Result<(), CfiError> {
        let mut cursor = Cursor {
            section: self.section,
            base: self.base,
            at: start,
            end,
        };
        while cursor.at < cursor.end {
            self.budget = self.budget.checked_sub(1).ok_or(CfiError::TooLong)?;
            let opcode = cursor.byte()?;
            let low = u64::from(opcode & 0x3f);
            let passed = match opcode & 0xc0 {
                CFA_ADVANCE_LOC => self.advance(low)?,
                CFA_OFFSET => {
                    let offset = self.unsigned_offset(cursor.unsigned()?)?;
                    row.set(low, Rule::Offset(offset));
                    false
                }
                CFA_RESTORE => {
                    let rule = initial.map_or(Rule::Unchanged, |initial| initial.rule(low as u16));
                    row.set(low, rule);
                    false
                }
                _ => self.extended(opcode, &mut cursor, row, initial)?,
            };
            if passed {
                break;
            }
        }
        Ok(())
    }

    fn extended(
        &mut self,
        opcode: u8,
        cursor: &mut Cursor<'_>,
        row: &mut Row,
        initial: Option<&Row>,
    ) -> Result<bool, CfiError> {
        match opcode {
            CFA_NOP | CFA_AARCH64_NEGATE_RA_STATE => {}
            CFA_SET_LOC => {
                let location = cursor
                    .pointer(self.cie.pointer_encoding, &|_| None)?
                    .ok_or(CfiError::Malformed)?;
                if location < self.location {
                    return Err(CfiError::Malformed);
                }
                self.location = location;
                return Ok(self.location > self.target);
            }
            CFA_ADVANCE_LOC1 => return self.advance(cursor.fixed(1)?),
            CFA_ADVANCE_LOC2 => return self.advance(cursor.fixed(2)?),
            CFA_ADVANCE_LOC4 => return self.advance(cursor.fixed(4)?),
            CFA_OFFSET_EXTENDED => {
                let register = cursor.unsigned()?;
                let offset = self.unsigned_offset(cursor.unsigned()?)?;
                row.set(register, Rule::Offset(offset));
            }
            CFA_OFFSET_EXTENDED_SF => {
                let register = cursor.unsigned()?;
                let offset = self.offset(cursor.signed()?)?;
                row.set(register, Rule::Offset(offset));
            }
            CFA_GNU_NEGATIVE_OFFSET_EXTENDED => {
                let register = cursor.unsigned()?;
                let offset = self.unsigned_offset(cursor.unsigned()?)?;
                row.set(register, Rule::Offset(offset.wrapping_neg()));
            }
            CFA_VAL_OFFSET => {
                let register = cursor.unsigned()?;
                let offset = self.unsigned_offset(cursor.unsigned()?)?;
                row.set(register, Rule::ValueOffset(offset));
            }
            CFA_VAL_OFFSET_SF => {
                let register = cursor.unsigned()?;
                let offset = self.offset(cursor.signed()?)?;
                row.set(register, Rule::ValueOffset(offset));
            }
            CFA_RESTORE_EXTENDED => {
                let register = cursor.unsigned()?;
                let rule = initial.map_or(Rule::Unchanged, |initial| {
                    u16::try_from(register).map_or(Rule::Unchanged, |number| initial.rule(number))
                });
                row.set(register, rule);
            }
            CFA_UNDEFINED => row.set(cursor.unsigned()?, Rule::Undefined),
            CFA_SAME_VALUE => row.set(cursor.unsigned()?, Rule::Unchanged),
            CFA_REGISTER => {
                let register = cursor.unsigned()?;
                let source = cursor.register()?;
                row.set(register, Rule::Register(source));
            }
            CFA_REMEMBER_STATE => {
                let slot = self
                    .remembered
                    .get_mut(self.depth)
                    .ok_or(CfiError::TooLong)?;
                *slot = *row;
                self.depth += 1;
            }
            CFA_RESTORE_STATE => {
                self.depth = self.depth.checked_sub(1).ok_or(CfiError::Malformed)?;
                *row = self.remembered[self.depth];
            }
            CFA_DEF_CFA => {
                let register = cursor.register()?;
                let offset = i64::try_from(cursor.unsigned()?).map_err(|_| CfiError::Malformed)?;
                row.cfa = CfaRule::RegisterOffset(register, offset);
            }
            CFA_DEF_CFA_SF => {
                let register = cursor.register()?;
                let offset = self.offset(cursor.signed()?)?;
                row.cfa = CfaRule::RegisterOffset(register, offset);
            }
            CFA_DEF_CFA_REGISTER => {
                let register = cursor.register()?;
                let CfaRule::RegisterOffset(_, offset) = row.cfa else {
                    return Err(CfiError::Malformed);
                };
                row.cfa = CfaRule::RegisterOffset(register, offset);
            }
            CFA_DEF_CFA_OFFSET => {
                let offset = i64::try_from(cursor.unsigned()?).map_err(|_| CfiError::Malformed)?;
                let CfaRule::RegisterOffset(register, _) = row.cfa else {
                    return Err(CfiError::Malformed);
                };
                row.cfa = CfaRule::RegisterOffset(register, offset);
            }
            CFA_DEF_CFA_OFFSET_SF => {
                let offset = self.offset(cursor.signed()?)?;
                let CfaRule::RegisterOffset(register, _) = row.cfa else {
                    return Err(CfiError::Malformed);
                };
                row.cfa = CfaRule::RegisterOffset(register, offset);
            }
            CFA_DEF_CFA_EXPRESSION => {
                let (at, length) = cursor.block()?;
                row.cfa = CfaRule::Expression(at, length);
            }
            CFA_EXPRESSION => {
                let register = cursor.unsigned()?;
                let (at, length) = cursor.block()?;
                row.set(register, Rule::Expression(at, length));
            }
            CFA_VAL_EXPRESSION => {
                let register = cursor.unsigned()?;
                let (at, length) = cursor.block()?;
                row.set(register, Rule::ValueExpression(at, length));
            }
            CFA_GNU_ARGS_SIZE => {
                cursor.unsigned()?;
            }
            _ => return Err(CfiError::Unsupported),
        }
        Ok(false)
    }
}

/// The row of rules in force at `pc` in the function `fde` describes: the CIE's initial
/// instructions, then the FDE's up to `pc`.
pub fn row(section: &[u8], base: u64, fde: &Fde, pc: u64) -> Result<Row, CfiError> {
    if !(fde.start..fde.end).contains(&pc) {
        return Err(CfiError::NotCovered);
    }
    let mut machine = Machine {
        section,
        base,
        cie: &fde.cie,
        location: fde.start,
        target: u64::MAX,
        budget: MAX_INSTRUCTIONS,
        remembered: [Row::EMPTY; MAX_REMEMBERED],
        depth: 0,
    };
    let mut row = Row::EMPTY;
    machine.run(fde.cie.instructions, &mut row, None)?;
    let initial = row;
    machine.location = fde.start;
    machine.target = pc;
    machine.depth = 0;
    machine.run(fde.instructions, &mut row, Some(&initial))?;
    Ok(row)
}

struct Stack {
    values: [u64; MAX_EXPRESSION_STACK],
    depth: usize,
}

impl Stack {
    fn push(&mut self, value: u64) -> Result<(), CfiError> {
        let slot = self.values.get_mut(self.depth).ok_or(CfiError::TooLong)?;
        *slot = value;
        self.depth += 1;
        Ok(())
    }

    fn pop(&mut self) -> Result<u64, CfiError> {
        self.depth = self.depth.checked_sub(1).ok_or(CfiError::Malformed)?;
        Ok(self.values[self.depth])
    }

    fn peek(&self, from_top: usize) -> Result<u64, CfiError> {
        let index = self
            .depth
            .checked_sub(1)
            .and_then(|top| top.checked_sub(from_top))
            .ok_or(CfiError::Malformed)?;
        Ok(self.values[index])
    }
}

fn binary(stack: &mut Stack, operation: u8) -> Result<(), CfiError> {
    let right = stack.pop()?;
    let left = stack.pop()?;
    let (signed_left, signed_right) = (left as i64, right as i64);
    let value = match operation {
        OP_AND => left & right,
        OP_DIV => match signed_right {
            0 => return Err(CfiError::Malformed),
            _ => signed_left.wrapping_div(signed_right) as u64,
        },
        OP_MINUS => left.wrapping_sub(right),
        OP_MOD => match right {
            0 => return Err(CfiError::Malformed),
            _ => left % right,
        },
        OP_MUL => left.wrapping_mul(right),
        OP_OR => left | right,
        OP_PLUS => left.wrapping_add(right),
        OP_SHL => left.checked_shl(right.min(64) as u32).unwrap_or(0),
        OP_SHR => left.checked_shr(right.min(64) as u32).unwrap_or(0),
        OP_SHRA => signed_left.wrapping_shr(right.min(63) as u32) as u64,
        OP_XOR => left ^ right,
        OP_EQ => u64::from(signed_left == signed_right),
        OP_GE => u64::from(signed_left >= signed_right),
        OP_GT => u64::from(signed_left > signed_right),
        OP_LE => u64::from(signed_left <= signed_right),
        OP_LT => u64::from(signed_left < signed_right),
        _ => u64::from(signed_left != signed_right),
    };
    stack.push(value)
}

fn jump(cursor: &mut Cursor<'_>, start: usize, offset: i64) -> Result<(), CfiError> {
    let target = (cursor.at as i64)
        .checked_add(offset)
        .and_then(|target| usize::try_from(target).ok())
        .filter(|&target| target >= start && target <= cursor.end)
        .ok_or(CfiError::Malformed)?;
    cursor.at = target;
    Ok(())
}

/// Evaluates the DWARF expression at `at` (`length` bytes) of the section with `registers`'
/// values, `initial` pushed first when given (the CFA, for register rules); the value on top
/// of the stack at its end.
pub fn evaluate<R: CfiRegisters>(
    section: &[u8],
    (at, length): (u32, u32),
    initial: Option<u64>,
    registers: &R,
    read: &dyn Fn(u64) -> Option<u64>,
) -> Result<u64, CfiError> {
    let start = at as usize;
    let end = start
        .checked_add(length as usize)
        .filter(|&end| end <= section.len())
        .ok_or(CfiError::Truncated)?;
    let mut cursor = Cursor {
        section,
        base: 0,
        at: start,
        end,
    };
    let mut stack = Stack {
        values: [0; MAX_EXPRESSION_STACK],
        depth: 0,
    };
    if let Some(value) = initial {
        stack.push(value)?;
    }
    let register_value = |number: u16| registers.get(number).ok_or(CfiError::Unsupported);
    for _ in 0..MAX_EXPRESSION_OPERATIONS {
        if cursor.at >= cursor.end {
            return stack.pop();
        }
        let operation = cursor.byte()?;
        match operation {
            OP_ADDR | OP_CONST8U | OP_CONST8S => stack.push(cursor.fixed(8)?)?,
            OP_DEREF => {
                let address = stack.pop()?;
                stack.push(read(address).ok_or(CfiError::Unreadable)?)?;
            }
            OP_DEREF_SIZE => {
                let size = cursor.byte()?;
                let address = stack.pop()?;
                let word = read(address).ok_or(CfiError::Unreadable)?;
                let value = match size {
                    1 => word & 0xff,
                    2 => word & 0xffff,
                    4 => word & 0xffff_ffff,
                    8 => word,
                    _ => return Err(CfiError::Malformed),
                };
                stack.push(value)?;
            }
            OP_CONST1U => stack.push(cursor.fixed(1)?)?,
            OP_CONST1S => stack.push(cursor.fixed(1)? as u8 as i8 as i64 as u64)?,
            OP_CONST2U => stack.push(cursor.fixed(2)?)?,
            OP_CONST2S => stack.push(cursor.fixed(2)? as u16 as i16 as i64 as u64)?,
            OP_CONST4U => stack.push(cursor.fixed(4)?)?,
            OP_CONST4S => stack.push(cursor.fixed(4)? as u32 as i32 as i64 as u64)?,
            OP_CONSTU => stack.push(cursor.unsigned()?)?,
            OP_CONSTS => stack.push(cursor.signed()? as u64)?,
            OP_DUP => stack.push(stack.peek(0)?)?,
            OP_DROP => {
                stack.pop()?;
            }
            OP_OVER => stack.push(stack.peek(1)?)?,
            OP_PICK => {
                let index = usize::from(cursor.byte()?);
                stack.push(stack.peek(index)?)?;
            }
            OP_SWAP => {
                let top = stack.pop()?;
                let below = stack.pop()?;
                stack.push(top)?;
                stack.push(below)?;
            }
            OP_ROT => {
                let first = stack.pop()?;
                let second = stack.pop()?;
                let third = stack.pop()?;
                stack.push(first)?;
                stack.push(third)?;
                stack.push(second)?;
            }
            OP_ABS => {
                let value = stack.pop()? as i64;
                stack.push(value.unsigned_abs())?;
            }
            OP_NEG => {
                let value = stack.pop()? as i64;
                stack.push(value.wrapping_neg() as u64)?;
            }
            OP_NOT => {
                let value = stack.pop()?;
                stack.push(!value)?;
            }
            OP_PLUS_UCONST => {
                let value = stack.pop()?;
                stack.push(value.wrapping_add(cursor.unsigned()?))?;
            }
            OP_AND | OP_DIV | OP_MINUS | OP_MOD | OP_MUL | OP_OR | OP_PLUS | OP_SHL | OP_SHR
            | OP_SHRA | OP_XOR | OP_EQ | OP_GE | OP_GT | OP_LE | OP_LT | OP_NE => {
                binary(&mut stack, operation)?
            }
            OP_SKIP => {
                let offset = cursor.fixed(2)? as u16 as i16;
                jump(&mut cursor, start, i64::from(offset))?;
            }
            OP_BRA => {
                let offset = cursor.fixed(2)? as u16 as i16;
                if stack.pop()? != 0 {
                    jump(&mut cursor, start, i64::from(offset))?;
                }
            }
            OP_LIT0..=OP_LIT31 => stack.push(u64::from(operation - OP_LIT0))?,
            OP_BREG0..=OP_BREG31 => {
                let base = register_value(u16::from(operation - OP_BREG0))?;
                stack.push(base.wrapping_add(cursor.signed()? as u64))?;
            }
            OP_BREGX => {
                let base = register_value(cursor.register()?)?;
                stack.push(base.wrapping_add(cursor.signed()? as u64))?;
            }
            OP_NOP => {}
            _ => return Err(CfiError::Unsupported),
        }
    }
    Err(CfiError::TooLong)
}

/// Turns `registers`, a frame of the function whose row at its program counter is `row`, into
/// its caller's at the return address; `read` reads a word of memory. Every rule reads the
/// frame's values as they were before any register changes.
pub fn apply<R: CfiRegisters>(
    section: &[u8],
    row: &Row,
    return_register: u16,
    registers: &mut R,
    read: &dyn Fn(u64) -> Option<u64>,
) -> Result<(), CfiError> {
    let old = *registers;
    let cfa = match row.cfa {
        CfaRule::RegisterOffset(register, offset) => old
            .get(register)
            .ok_or(CfiError::Unsupported)?
            .wrapping_add(offset as u64),
        CfaRule::Expression(at, length) => evaluate(section, (at, length), None, &old, read)?,
    };
    if row.rule(return_register) == Rule::Undefined {
        return Err(CfiError::EndOfStack);
    }
    let mut new = old;
    for (number, rule) in row.rules.iter().enumerate() {
        let number = number as u16;
        let value = match *rule {
            Rule::Unchanged | Rule::Undefined => continue,
            Rule::Offset(offset) => {
                read(cfa.wrapping_add(offset as u64)).ok_or(CfiError::Unreadable)?
            }
            Rule::ValueOffset(offset) => cfa.wrapping_add(offset as u64),
            Rule::Register(source) => old.get(source).ok_or(CfiError::Unsupported)?,
            Rule::Expression(at, length) => {
                let address = evaluate(section, (at, length), Some(cfa), &old, read)?;
                read(address).ok_or(CfiError::Unreadable)?
            }
            Rule::ValueExpression(at, length) => {
                evaluate(section, (at, length), Some(cfa), &old, read)?
            }
        };
        new.set(number, value);
    }
    let return_address = new.get(return_register).ok_or(CfiError::Unsupported)?;
    new.set(R::STACK_POINTER, cfa);
    new.set_program_counter(return_address);
    *registers = new;
    Ok(())
}

/// Steps `registers` over a frame of the function `fde` describes: finds its row at the frame's
/// program counter `pc` (one before a return address) and applies it.
pub fn step<R: CfiRegisters>(
    section: &[u8],
    base: u64,
    fde: &Fde,
    pc: u64,
    registers: &mut R,
    read: &dyn Fn(u64) -> Option<u64>,
) -> Result<(), CfiError> {
    let row = row(section, base, fde, pc)?;
    apply(section, &row, fde.cie.return_register, registers, read)
}

#[cfg(test)]
mod tests {
    use super::super::unwind_info::{Arm64Registers, X86_64Registers};
    use super::*;

    const BASE: u64 = 0x4000;
    const FUNCTION: u64 = 0x1000;
    const PERSONALITY_SLOT: u64 = 0x9000;
    const PERSONALITY: u64 = 0x7777_0000;

    fn put(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn finish_entry(bytes: &mut [u8], start: usize) {
        let length = (bytes.len() - start - 4) as u32;
        bytes[start..start + 4].copy_from_slice(&length.to_le_bytes());
    }

    fn pcrel(bytes: &[u8], target: u64) -> u32 {
        target.wrapping_sub(BASE + bytes.len() as u64) as u32
    }

    fn x86_64_section(fde_instructions: &[u8], lsda: Option<u64>) -> Vec<u8> {
        x86_64_section_with(fde_instructions, lsda, &[])
    }

    fn x86_64_section_with(
        fde_instructions: &[u8],
        lsda: Option<u64>,
        entries_before_fde: &[u8],
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        put(&mut bytes, 0);
        put(&mut bytes, 0);
        bytes.push(1);
        bytes.extend_from_slice(b"zPLR\0");
        bytes.push(1);
        bytes.push(0x78);
        bytes.push(16);
        bytes.push(7);
        bytes.push(0x9b);
        let slot = pcrel(&bytes, PERSONALITY_SLOT);
        put(&mut bytes, slot);
        bytes.push(0x1b);
        bytes.push(0x1b);
        bytes.extend_from_slice(&[CFA_DEF_CFA, 7, 8, CFA_OFFSET | 16, 1, CFA_NOP, CFA_NOP]);
        finish_entry(&mut bytes, 0);
        bytes.extend_from_slice(entries_before_fde);
        let fde = bytes.len();
        put(&mut bytes, 0);
        let cie_distance = bytes.len() as u32;
        put(&mut bytes, cie_distance);
        let begin = pcrel(&bytes, FUNCTION);
        put(&mut bytes, begin);
        put(&mut bytes, 0x100);
        bytes.push(4);
        let lsda = lsda.map_or(0, |address| pcrel(&bytes, address));
        put(&mut bytes, lsda);
        bytes.extend_from_slice(fde_instructions);
        finish_entry(&mut bytes, fde);
        put(&mut bytes, 0);
        bytes
    }

    fn standard_x86_64() -> Vec<u8> {
        x86_64_section(
            &[
                CFA_ADVANCE_LOC | 1,
                CFA_DEF_CFA_OFFSET,
                16,
                CFA_OFFSET | 6,
                2,
                CFA_ADVANCE_LOC | 3,
                CFA_DEF_CFA_REGISTER,
                6,
                CFA_ADVANCE_LOC | 4,
                CFA_OFFSET | 3,
                3,
            ],
            Some(0x5000),
        )
    }

    fn memory(cells: &[(u64, u64)]) -> impl Fn(u64) -> Option<u64> + '_ {
        move |address| {
            if address == PERSONALITY_SLOT {
                return Some(PERSONALITY);
            }
            cells
                .iter()
                .find(|(at, _)| *at == address)
                .map(|(_, value)| *value)
        }
    }

    const FDE_OFFSET: usize = 32;

    #[test]
    fn a_damaged_fde_is_passed_over_and_the_scan_goes_on() {
        let mut damaged = Vec::new();
        put(&mut damaged, 4);
        put(&mut damaged, u32::MAX);
        let bytes = x86_64_section_with(&[], None, &damaged);
        let read = memory(&[]);
        assert_eq!(
            fde_at(&bytes, BASE, FDE_OFFSET, &read).err(),
            Some(CfiError::Malformed)
        );
        let found = find(&bytes, BASE, FUNCTION + 0x80, &read).unwrap();
        assert_eq!((found.start, found.end), (FUNCTION, FUNCTION + 0x100));
    }

    #[test]
    fn an_fde_gives_its_range_lsda_and_its_cie_s_personality() {
        let bytes = standard_x86_64();
        let read = memory(&[]);
        let fde = fde_at(&bytes, BASE, FDE_OFFSET, &read).unwrap();
        assert_eq!((fde.start, fde.end), (FUNCTION, FUNCTION + 0x100));
        assert_eq!(fde.lsda, Some(0x5000));
        assert_eq!(fde.cie.personality, Some(PERSONALITY));
        assert_eq!(fde.cie.return_register, 16);
        assert_eq!(find(&bytes, BASE, FUNCTION + 0x80, &read), Ok(fde));
        assert_eq!(
            find(&bytes, BASE, FUNCTION + 0x100, &read),
            Err(CfiError::NotCovered)
        );
        assert_eq!(
            fde_at(&bytes, BASE, 0, &read),
            Err(CfiError::Malformed),
            "a CIE is not an FDE"
        );
    }

    #[test]
    fn the_row_follows_the_program_up_to_the_address() {
        let bytes = standard_x86_64();
        let read = memory(&[]);
        let fde = fde_at(&bytes, BASE, FDE_OFFSET, &read).unwrap();
        let at_entry = row(&bytes, BASE, &fde, FUNCTION).unwrap();
        assert_eq!(at_entry.cfa, CfaRule::RegisterOffset(7, 8));
        assert_eq!(at_entry.rule(16), Rule::Offset(-8));
        assert_eq!(at_entry.rule(6), Rule::Unchanged);
        let after_push = row(&bytes, BASE, &fde, FUNCTION + 1).unwrap();
        assert_eq!(after_push.cfa, CfaRule::RegisterOffset(7, 16));
        assert_eq!(after_push.rule(6), Rule::Offset(-16));
        let framed = row(&bytes, BASE, &fde, FUNCTION + 0x40).unwrap();
        assert_eq!(framed.cfa, CfaRule::RegisterOffset(6, 16));
        assert_eq!(framed.rule(3), Rule::Offset(-24));
        assert_eq!(
            row(&bytes, BASE, &fde, FUNCTION - 1),
            Err(CfiError::NotCovered)
        );
    }

    #[test]
    fn applying_a_framed_row_restores_the_caller() {
        let bytes = standard_x86_64();
        let fde = fde_at(&bytes, BASE, FDE_OFFSET, &memory(&[])).unwrap();
        let mut registers = X86_64Registers::default();
        registers.general[6] = 0x8000;
        registers.general[7] = 0x7f00;
        registers.rip = FUNCTION + 0x40;
        let cells = [
            (0x8010 - 8, 0x1_0000_2222),
            (0x8010 - 16, 0x8100),
            (0x8010 - 24, 0xb0b0),
        ];
        let read = memory(&cells);
        step(&bytes, BASE, &fde, FUNCTION + 0x40, &mut registers, &read).unwrap();
        assert_eq!(registers.rip, 0x1_0000_2222);
        assert_eq!(registers.general[7], 0x8010);
        assert_eq!(registers.general[6], 0x8100);
        assert_eq!(registers.general[3], 0xb0b0);
    }

    #[test]
    fn an_arm64_frame_restores_its_pairs_and_link_register() {
        let mut bytes = Vec::new();
        put(&mut bytes, 0);
        put(&mut bytes, 0);
        bytes.push(1);
        bytes.extend_from_slice(b"zR\0");
        bytes.push(1);
        bytes.push(0x78);
        bytes.push(30);
        bytes.push(1);
        bytes.push(0x1b);
        bytes.extend_from_slice(&[CFA_DEF_CFA, 31, 0]);
        finish_entry(&mut bytes, 0);
        let fde = bytes.len();
        put(&mut bytes, 0);
        put(&mut bytes, fde as u32 + 4);
        let begin = pcrel(&bytes, FUNCTION);
        put(&mut bytes, begin);
        put(&mut bytes, 0x40);
        bytes.push(0);
        bytes.extend_from_slice(&[
            CFA_ADVANCE_LOC | 2,
            CFA_DEF_CFA,
            29,
            16,
            CFA_OFFSET | 30,
            1,
            CFA_OFFSET | 29,
            2,
            CFA_OFFSET | 19,
            3,
            CFA_OFFSET_EXTENDED,
            72,
            4,
        ]);
        finish_entry(&mut bytes, fde);
        let parsed = fde_at(&bytes, BASE, fde, &|_| None).unwrap();
        assert_eq!(parsed.lsda, None);
        assert_eq!(parsed.cie.personality, None);
        let mut registers = Arm64Registers::default();
        registers.x[29] = 0x6000;
        registers.sp = 0x5f00;
        let cells = [
            (0x6008, 0x1_0000_3333),
            (0x6000, 0x6100),
            (0x5ff8, 0x1919),
            (0x5ff0, 0xd8d8),
        ];
        let read = memory(&cells);
        step(
            &bytes,
            BASE,
            &parsed,
            FUNCTION + 0x10,
            &mut registers,
            &read,
        )
        .unwrap();
        assert_eq!(registers.pc, 0x1_0000_3333);
        assert_eq!(registers.x[30], 0x1_0000_3333);
        assert_eq!(registers.x[29], 0x6100);
        assert_eq!(registers.x[19], 0x1919);
        assert_eq!(registers.d[0], 0xd8d8);
        assert_eq!(registers.sp, 0x6010);
    }

    #[test]
    fn remembered_state_comes_back_and_its_depth_is_limited() {
        let bytes = x86_64_section(
            &[
                CFA_ADVANCE_LOC | 1,
                CFA_DEF_CFA_OFFSET,
                16,
                CFA_REMEMBER_STATE,
                CFA_ADVANCE_LOC | 1,
                CFA_DEF_CFA_OFFSET,
                32,
                CFA_ADVANCE_LOC | 1,
                CFA_RESTORE_STATE,
            ],
            None,
        );
        let read = memory(&[]);
        let fde = fde_at(&bytes, BASE, FDE_OFFSET, &read).unwrap();
        assert_eq!(fde.lsda, None);
        let inner = row(&bytes, BASE, &fde, FUNCTION + 2).unwrap();
        assert_eq!(inner.cfa, CfaRule::RegisterOffset(7, 32));
        let restored = row(&bytes, BASE, &fde, FUNCTION + 3).unwrap();
        assert_eq!(restored.cfa, CfaRule::RegisterOffset(7, 16));
        let deep = x86_64_section(&[CFA_REMEMBER_STATE; MAX_REMEMBERED + 1], None);
        let fde = fde_at(&deep, BASE, FDE_OFFSET, &read).unwrap();
        assert_eq!(row(&deep, BASE, &fde, FUNCTION), Err(CfiError::TooLong));
        let unbalanced = x86_64_section(&[CFA_RESTORE_STATE], None);
        let fde = fde_at(&unbalanced, BASE, FDE_OFFSET, &read).unwrap();
        assert_eq!(
            row(&unbalanced, BASE, &fde, FUNCTION),
            Err(CfiError::Malformed)
        );
    }

    #[test]
    fn a_program_longer_than_the_limit_is_refused() {
        let nops = [CFA_NOP; MAX_INSTRUCTIONS + 1];
        let bytes = x86_64_section(&nops, None);
        let read = memory(&[]);
        let fde = fde_at(&bytes, BASE, FDE_OFFSET, &read).unwrap();
        assert_eq!(row(&bytes, BASE, &fde, FUNCTION), Err(CfiError::TooLong));
    }

    #[test]
    fn expressions_compute_the_cfa_and_saved_slots() {
        let mut instructions = vec![CFA_DEF_CFA_EXPRESSION, 3, OP_BREG0 + 7, 0x10, OP_NOP];
        instructions.extend_from_slice(&[CFA_EXPRESSION, 3, 3, OP_LIT0 + 8, OP_MINUS, OP_NOP]);
        instructions.extend_from_slice(&[CFA_VAL_EXPRESSION, 12, 2, OP_CONST1U, 5]);
        let bytes = x86_64_section(&instructions, None);
        let fde = fde_at(&bytes, BASE, FDE_OFFSET, &memory(&[])).unwrap();
        let mut registers = X86_64Registers::default();
        registers.general[7] = 0x3000;
        let cells = [(0x3010 - 8, 0x1_0000_4444)];
        let read = memory(&cells);
        step(&bytes, BASE, &fde, FUNCTION, &mut registers, &read).unwrap();
        assert_eq!(registers.general[7], 0x3010);
        assert_eq!(registers.general[3], 0x1_0000_4444);
        assert_eq!(registers.general[12], 5);
    }

    #[test]
    fn the_expression_machine_branches_and_guards_its_stack() {
        let registers = X86_64Registers::default();
        let read = |_: u64| None;
        let program = [OP_LIT0 + 1, OP_BRA, 1, 0, OP_LIT0 + 9, OP_LIT0 + 2];
        assert_eq!(
            evaluate(&program, (0, program.len() as u32), None, &registers, &read),
            Ok(2)
        );
        let underflow = [OP_DROP];
        assert_eq!(
            evaluate(&underflow, (0, 1), None, &registers, &read),
            Err(CfiError::Malformed)
        );
        let overflow = [OP_LIT0; MAX_EXPRESSION_STACK + 1];
        assert_eq!(
            evaluate(
                &overflow,
                (0, overflow.len() as u32),
                None,
                &registers,
                &read
            ),
            Err(CfiError::TooLong)
        );
        let forever = [OP_SKIP, 0xfd, 0xff];
        assert_eq!(
            evaluate(&forever, (0, 3), None, &registers, &read),
            Err(CfiError::TooLong)
        );
        let outside = [OP_SKIP, 0x10, 0x00];
        assert_eq!(
            evaluate(&outside, (0, 3), None, &registers, &read),
            Err(CfiError::Malformed)
        );
        let division = [OP_LIT0 + 1, OP_LIT0, OP_DIV];
        assert_eq!(
            evaluate(&division, (0, 3), None, &registers, &read),
            Err(CfiError::Malformed)
        );
    }

    #[test]
    fn an_undefined_return_address_ends_the_stack() {
        let bytes = x86_64_section(&[CFA_UNDEFINED, 16], None);
        let fde = fde_at(&bytes, BASE, FDE_OFFSET, &memory(&[])).unwrap();
        let mut registers = X86_64Registers::default();
        assert_eq!(
            step(&bytes, BASE, &fde, FUNCTION, &mut registers, &memory(&[])),
            Err(CfiError::EndOfStack)
        );
    }

    #[test]
    fn unknown_versions_and_augmentations_are_refused() {
        let read = memory(&[]);
        let mut version = standard_x86_64();
        version[8] = 2;
        assert_eq!(
            fde_at(&version, BASE, FDE_OFFSET, &read),
            Err(CfiError::Unsupported)
        );
        let mut augmentation = standard_x86_64();
        augmentation[9] = b'e';
        assert_eq!(
            fde_at(&augmentation, BASE, FDE_OFFSET, &read),
            Err(CfiError::Unsupported)
        );
        let bad_opcode = x86_64_section(&[0x3f], None);
        let fde = fde_at(&bad_opcode, BASE, FDE_OFFSET, &read).unwrap();
        assert_eq!(
            row(&bad_opcode, BASE, &fde, FUNCTION),
            Err(CfiError::Unsupported)
        );
    }

    #[test]
    fn every_truncation_and_byte_flip_is_an_error_not_a_panic() {
        let bytes = standard_x86_64();
        let read = memory(&[(0x8008, 1), (0x8000, 2)]);
        for length in 0..bytes.len() {
            let cut = &bytes[..length];
            if let Ok(fde) = fde_at(cut, BASE, FDE_OFFSET, &read) {
                let _ = row(cut, BASE, &fde, FUNCTION + 0x40);
            }
            let _ = find(cut, BASE, FUNCTION, &read);
        }
        for position in 0..bytes.len() {
            for mask in [0x01u8, 0x80, 0xff] {
                let mut flipped = bytes.clone();
                flipped[position] ^= mask;
                let _ = find(&flipped, BASE, FUNCTION + 0x40, &read);
                let Ok(fde) = fde_at(&flipped, BASE, FDE_OFFSET, &read) else {
                    continue;
                };
                let Ok(found) = row(&flipped, BASE, &fde, FUNCTION + 0x40) else {
                    continue;
                };
                let mut registers = X86_64Registers::default();
                registers.general[6] = 0x8000;
                let _ = apply(&flipped, &found, 16, &mut registers, &read);
            }
        }
        assert_eq!(
            fde_at(&bytes, BASE, bytes.len(), &read),
            Err(CfiError::Truncated)
        );
        assert_eq!(
            fde_at(&bytes, BASE, usize::MAX - 2, &read),
            Err(CfiError::Truncated)
        );
    }
}
