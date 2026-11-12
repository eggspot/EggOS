//! AMD's ATOM command tables run (D467): the byte-code programs an ATOM video BIOS carries
//! for the steps a board needs in its own way (setting a pixel clock, driving an encoder or a
//! transmitter, initialising the chip). Each opcode works on a destination and a source; an
//! operand is a register, a parameter, a workspace slot, scratch memory, a constant in the
//! image, an immediate, or a PLL or memory-controller register, read and written through the
//! alignment its attribute byte names (a whole dword, a word or a byte at an offset). Jumps go
//! to offsets within the table, `CALL_TABLE` runs another table on the parameters past the
//! caller's, and a table ends at `EOT`. Only memory-mapped register access is taken: a table
//! that switches to I/O ports stops with [`AtomError::Unsupported`].

use alloc::vec;
use alloc::vec::Vec;

const ROM_HEADER_POINTER: usize = 0x48;
const MASTER_COMMAND_TABLE: usize = 30;
const MASTER_DATA_TABLE: usize = 32;
const TABLE_HEADER_BYTES: usize = 4;
const WORKSPACE_SIZE: usize = 4;
const PARAMETER_SIZE: usize = 5;
const PARAMETER_SIZE_MASK: u8 = 0x7f;
const CODE: usize = 6;
const MOST_NESTED_CALLS: usize = 16;
const MOST_STEPS: u32 = 1_000_000;
const CASE_MAGIC: u8 = 0x63;
const CASE_END: u16 = 0x5a5a;
const DATA_BLOCK_THIS_TABLE: u8 = 255;

const WS_QUOTIENT: u8 = 0x40;
const WS_REMAINDER: u8 = 0x41;
const WS_DATA_POINTER: u8 = 0x42;
const WS_SHIFT: u8 = 0x43;
const WS_OR_MASK: u8 = 0x44;
const WS_AND_MASK: u8 = 0x45;
const WS_FB_WINDOW: u8 = 0x46;
const WS_ATTRIBUTES: u8 = 0x47;
const WS_REGISTER_POINTER: u8 = 0x48;

const ALIGN_MASKS: [u32; 8] = [
    0xffff_ffff,
    0xffff,
    0xffff,
    0xffff,
    0xff,
    0xff,
    0xff,
    0xff,
];
const ALIGN_SHIFTS: [u32; 8] = [0, 0, 8, 16, 0, 8, 16, 24];
const DESTINATION_ALIGN: [[u8; 4]; 8] = [
    [0, 0, 0, 0],
    [1, 2, 3, 0],
    [1, 2, 3, 0],
    [1, 2, 3, 0],
    [4, 5, 6, 7],
    [4, 5, 6, 7],
    [4, 5, 6, 7],
    [4, 5, 6, 7],
];
const DEFAULT_DESTINATION: [u8; 8] = [0, 0, 1, 2, 0, 1, 2, 3];
const DESTINATIONS: [Operand; 6] = [
    Operand::Register,
    Operand::Parameter,
    Operand::Workspace,
    Operand::Scratch,
    Operand::Pll,
    Operand::MemoryController,
];

/// What runs a table touches on the board.
pub trait AtomCard {
    /// Reads register `index` (in dwords from the register block).
    fn read_register(&mut self, index: u32) -> u32;
    /// Writes register `index`.
    fn write_register(&mut self, index: u32, value: u32);
    /// Reads PLL register `index`.
    fn read_pll(&mut self, index: u32) -> u32;
    /// Writes PLL register `index`.
    fn write_pll(&mut self, index: u32, value: u32);
    /// Reads memory-controller register `index`.
    fn read_memory_controller(&mut self, index: u32) -> u32;
    /// Writes memory-controller register `index`.
    fn write_memory_controller(&mut self, index: u32, value: u32);
    /// Waits `microseconds`.
    fn delay(&mut self, microseconds: u32);
}

/// Why a table did not run to its end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtomError {
    /// The image is not an ATOM BIOS, or has no table with that index.
    NoTable,
    /// The table reads past its image or names an opcode that does not exist.
    Malformed,
    /// The table uses I/O ports or another access this interpreter does not take.
    Unsupported,
    /// The table ran past the step limit: a loop that never ends.
    Runaway,
    /// Calls nested deeper than the limit.
    TooDeep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operand {
    Register,
    Parameter,
    Workspace,
    Scratch,
    Constant,
    Immediate,
    Pll,
    MemoryController,
}

impl Operand {
    fn of(code: u8) -> Self {
        match code & 7 {
            0 => Self::Register,
            1 => Self::Parameter,
            2 => Self::Workspace,
            3 => Self::Scratch,
            4 => Self::Constant,
            5 => Self::Immediate,
            6 => Self::Pll,
            _ => Self::MemoryController,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operation {
    Move,
    And,
    Or,
    ShiftLeftBy,
    ShiftRightBy,
    Multiply,
    Divide,
    Add,
    Subtract,
    Compare,
    Test,
    Clear,
    Mask,
    Xor,
    ShiftLeft,
    ShiftRight,
    Multiply32,
    Divide32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Condition {
    Always,
    Equal,
    Below,
    Above,
    BelowOrEqual,
    AboveOrEqual,
    NotEqual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Instruction {
    Arithmetic(Operation, Operand),
    SetAtiPort,
    SetPciPort,
    SetSystemIoPort,
    SetRegisterBlock,
    SetScratchBase,
    Switch,
    Jump(Condition),
    DelayMilliseconds,
    DelayMicroseconds,
    CallTable,
    Nothing,
    End,
    SkipByte,
    SetDataBlock,
    SkipData,
}

fn decode(opcode: u8) -> Option<Instruction> {
    use Instruction::*;
    use Operation::*;
    let group = |first: u8, operation: Operation| {
        let index = usize::from(opcode - first);
        Arithmetic(operation, DESTINATIONS[index])
    };
    Some(match opcode {
        1..=6 => group(1, Move),
        7..=12 => group(7, And),
        13..=18 => group(13, Or),
        19..=24 => group(19, ShiftLeftBy),
        25..=30 => group(25, ShiftRightBy),
        31..=36 => group(31, Multiply),
        37..=42 => group(37, Divide),
        43..=48 => group(43, Add),
        49..=54 => group(49, Subtract),
        55 => SetAtiPort,
        56 => SetPciPort,
        57 => SetSystemIoPort,
        58 => SetRegisterBlock,
        59 => SetScratchBase,
        60..=65 => group(60, Compare),
        66 => Switch,
        67 => Jump(Condition::Always),
        68 => Jump(Condition::Equal),
        69 => Jump(Condition::Below),
        70 => Jump(Condition::Above),
        71 => Jump(Condition::BelowOrEqual),
        72 => Jump(Condition::AboveOrEqual),
        73 => Jump(Condition::NotEqual),
        74..=79 => group(74, Test),
        80 => DelayMilliseconds,
        81 => DelayMicroseconds,
        82 => CallTable,
        83 | 90 | 99 | 100 | 101 => Nothing,
        84..=89 => group(84, Clear),
        91 => End,
        92..=97 => group(92, Mask),
        98 | 121 => SkipByte,
        102 => SetDataBlock,
        103..=108 => group(103, Xor),
        109..=114 => group(109, ShiftLeft),
        115..=120 => group(115, ShiftRight),
        122 => SkipData,
        123..=128 => group(123, Multiply32),
        129..=134 => group(129, Divide32),
        _ => return None,
    })
}

fn half(image: &[u8], at: usize) -> Option<u16> {
    let bytes = image.get(at..at + 2)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn table_at(image: &[u8], master: usize, index: usize) -> Option<usize> {
    let at = usize::from(half(image, master + TABLE_HEADER_BYTES + 2 * index)?);
    (at != 0 && at < image.len()).then_some(at)
}

/// An ATOM image's command and data tables, ready to run.
#[derive(Debug, Clone, Copy)]
pub struct Interpreter<'a> {
    image: &'a [u8],
    commands: usize,
    data: usize,
}

struct State<'a, C: AtomCard> {
    image: &'a [u8],
    card: &'a mut C,
    data: usize,
    register_block: u32,
    data_block: u32,
    scratch_base: u32,
    shift: u32,
    attributes: u32,
    quotient: u32,
    remainder: u32,
    equal: bool,
    above: bool,
    steps: u32,
    scratch: Vec<u32>,
}

impl<'a> Interpreter<'a> {
    /// The tables of ATOM video BIOS `image`; `None` when it is not one.
    pub fn new(image: &'a [u8]) -> Option<Self> {
        let header = usize::from(half(image, ROM_HEADER_POINTER)?);
        if image.get(header + 4..header + 8)? != b"ATOM" {
            return None;
        }
        Some(Self {
            image,
            commands: usize::from(half(image, header + MASTER_COMMAND_TABLE)?),
            data: usize::from(half(image, header + MASTER_DATA_TABLE)?),
        })
    }

    /// Whether command table `index` exists.
    pub fn has_table(&self, index: usize) -> bool {
        table_at(self.image, self.commands, index).is_some()
    }

    /// Runs command table `index` on `card` with `parameters`, which it may change.
    pub fn run<C: AtomCard>(
        &self,
        card: &mut C,
        index: usize,
        parameters: &mut [u32],
    ) -> Result<(), AtomError> {
        let mut state = State {
            image: self.image,
            card,
            data: self.data,
            register_block: 0,
            data_block: 0,
            scratch_base: 0,
            shift: 0,
            attributes: 0,
            quotient: 0,
            remainder: 0,
            equal: false,
            above: false,
            steps: 0,
            scratch: Vec::new(),
        };
        self.call(&mut state, index, parameters, 0)
    }

    fn call<C: AtomCard>(
        &self,
        state: &mut State<'_, C>,
        index: usize,
        parameters: &mut [u32],
        depth: usize,
    ) -> Result<(), AtomError> {
        if depth >= MOST_NESTED_CALLS {
            return Err(AtomError::TooDeep);
        }
        let base = table_at(self.image, self.commands, index).ok_or(AtomError::NoTable)?;
        let size = usize::from(half(self.image, base).ok_or(AtomError::Malformed)?);
        let end = base.checked_add(size).filter(|&end| end <= self.image.len());
        let end = end.ok_or(AtomError::Malformed)?;
        let header_byte =
            |at: usize| self.image.get(base + at).copied().ok_or(AtomError::Malformed);
        let workspace_dwords = usize::from(header_byte(WORKSPACE_SIZE)?);
        let parameter_bytes = usize::from(header_byte(PARAMETER_SIZE)? & PARAMETER_SIZE_MASK);
        let mut workspace = vec![0u32; workspace_dwords];
        let mut frame = Frame {
            base,
            end,
            at: base + CODE,
            workspace: &mut workspace,
            parameters,
            parameter_dwords: parameter_bytes / 4,
        };
        loop {
            state.steps += 1;
            if state.steps > MOST_STEPS {
                return Err(AtomError::Runaway);
            }
            let opcode = frame.byte(state.image)?;
            let instruction = decode(opcode).ok_or(AtomError::Malformed)?;
            match instruction {
                Instruction::End => return Ok(()),
                Instruction::CallTable => {
                    let called = usize::from(frame.byte(state.image)?);
                    let shift = frame.parameter_dwords.min(frame.parameters.len());
                    self.call(state, called, &mut frame.parameters[shift..], depth + 1)?;
                }
                instruction => state.step(&mut frame, instruction)?,
            }
        }
    }
}

struct Frame<'p> {
    base: usize,
    end: usize,
    at: usize,
    workspace: &'p mut [u32],
    parameters: &'p mut [u32],
    parameter_dwords: usize,
}

impl Frame<'_> {
    fn byte(&mut self, image: &[u8]) -> Result<u8, AtomError> {
        if self.at >= self.end {
            return Err(AtomError::Malformed);
        }
        let byte = *image.get(self.at).ok_or(AtomError::Malformed)?;
        self.at += 1;
        Ok(byte)
    }

    fn half(&mut self, image: &[u8]) -> Result<u16, AtomError> {
        Ok(u16::from_le_bytes([self.byte(image)?, self.byte(image)?]))
    }

    fn word(&mut self, image: &[u8]) -> Result<u32, AtomError> {
        Ok(u32::from(self.half(image)?) | u32::from(self.half(image)?) << 16)
    }

    fn skip(&mut self, image: &[u8], operand: Operand) -> Result<(), AtomError> {
        match operand {
            Operand::Register | Operand::Constant => self.half(image).map(|_| ()),
            _ => self.byte(image).map(|_| ()),
        }
    }

    fn jump(&mut self, target: u16) -> Result<(), AtomError> {
        let at = self.base + usize::from(target);
        if at >= self.end {
            return Err(AtomError::Malformed);
        }
        self.at = at;
        Ok(())
    }
}

impl<C: AtomCard> State<'_, C> {
    fn read(
        &mut self,
        frame: &mut Frame<'_>,
        operand: Operand,
        align: u8,
    ) -> Result<(u32, u32), AtomError> {
        let image = self.image;
        let whole = match operand {
            Operand::Register => {
                let index = u32::from(frame.half(image)?) + self.register_block;
                self.card.read_register(index)
            }
            Operand::Parameter => {
                let index = usize::from(frame.byte(image)?);
                *frame.parameters.get(index).ok_or(AtomError::Malformed)?
            }
            Operand::Workspace => {
                let index = frame.byte(image)?;
                match index {
                    WS_QUOTIENT => self.quotient,
                    WS_REMAINDER => self.remainder,
                    WS_DATA_POINTER => self.data_block,
                    WS_SHIFT => self.shift,
                    WS_OR_MASK => 1u32.wrapping_shl(self.shift),
                    WS_AND_MASK => !1u32.wrapping_shl(self.shift),
                    WS_FB_WINDOW => self.scratch_base,
                    WS_ATTRIBUTES => self.attributes,
                    WS_REGISTER_POINTER => self.register_block,
                    index => *frame
                        .workspace
                        .get(usize::from(index))
                        .ok_or(AtomError::Malformed)?,
                }
            }
            Operand::Scratch => {
                let index = (self.scratch_base / 4) as usize + usize::from(frame.byte(image)?);
                self.scratch.get(index).copied().unwrap_or(0)
            }
            Operand::Constant => {
                let at = usize::from(frame.half(image)?) + self.data_block as usize;
                let bytes = image.get(at..at + 4).ok_or(AtomError::Malformed)?;
                u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
            }
            Operand::Immediate => {
                let value = match align {
                    0 => frame.word(image)?,
                    1..=3 => u32::from(frame.half(image)?),
                    _ => u32::from(frame.byte(image)?),
                };
                return Ok((value, value));
            }
            Operand::Pll => {
                let index = u32::from(frame.byte(image)?);
                self.card.read_pll(index)
            }
            Operand::MemoryController => {
                let index = u32::from(frame.byte(image)?);
                self.card.read_memory_controller(index)
            }
        };
        let align = usize::from(align);
        Ok(((whole >> ALIGN_SHIFTS[align]) & ALIGN_MASKS[align], whole))
    }

    fn write(
        &mut self,
        frame: &mut Frame<'_>,
        operand: Operand,
        align: u8,
        value: u32,
        saved: u32,
    ) -> Result<(), AtomError> {
        let image = self.image;
        let align = usize::from(align);
        let mask = ALIGN_MASKS[align] << ALIGN_SHIFTS[align];
        let whole = (saved & !mask) | ((value & ALIGN_MASKS[align]) << ALIGN_SHIFTS[align]);
        match operand {
            Operand::Register => {
                let index = u32::from(frame.half(image)?) + self.register_block;
                self.card.write_register(index, whole);
            }
            Operand::Parameter => {
                let index = usize::from(frame.byte(image)?);
                *frame.parameters.get_mut(index).ok_or(AtomError::Malformed)? = whole;
            }
            Operand::Workspace => match frame.byte(image)? {
                WS_QUOTIENT => self.quotient = whole,
                WS_REMAINDER => self.remainder = whole,
                WS_DATA_POINTER => self.data_block = whole,
                WS_SHIFT => self.shift = whole,
                WS_OR_MASK | WS_AND_MASK => {}
                WS_FB_WINDOW => self.scratch_base = whole,
                WS_ATTRIBUTES => self.attributes = whole,
                WS_REGISTER_POINTER => self.register_block = whole,
                index => {
                    *frame
                        .workspace
                        .get_mut(usize::from(index))
                        .ok_or(AtomError::Malformed)? = whole
                }
            },
            Operand::Scratch => {
                let index = (self.scratch_base / 4) as usize + usize::from(frame.byte(image)?);
                if index >= self.scratch.len() {
                    self.scratch.resize(index + 1, 0);
                }
                self.scratch[index] = whole;
            }
            Operand::Pll => {
                let index = u32::from(frame.byte(image)?);
                self.card.write_pll(index, whole);
            }
            Operand::MemoryController => {
                let index = u32::from(frame.byte(image)?);
                self.card.write_memory_controller(index, whole);
            }
            Operand::Constant | Operand::Immediate => return Err(AtomError::Malformed),
        }
        Ok(())
    }

    fn step(&mut self, frame: &mut Frame<'_>, instruction: Instruction) -> Result<(), AtomError> {
        let image = self.image;
        match instruction {
            Instruction::Arithmetic(operation, destination) => {
                self.arithmetic(frame, operation, destination)
            }
            Instruction::SetAtiPort => match frame.half(image)? {
                0 => Ok(()),
                _ => Err(AtomError::Unsupported),
            },
            Instruction::SetPciPort | Instruction::SetSystemIoPort => Err(AtomError::Unsupported),
            Instruction::SetRegisterBlock => {
                self.register_block = u32::from(frame.half(image)?);
                Ok(())
            }
            Instruction::SetScratchBase => {
                let attribute = frame.byte(image)?;
                let source = Operand::of(attribute);
                let (value, _) = self.read(frame, source, attribute >> 3 & 7)?;
                self.scratch_base = value;
                Ok(())
            }
            Instruction::Switch => self.switch(frame),
            Instruction::Jump(condition) => {
                let target = frame.half(image)?;
                let taken = match condition {
                    Condition::Always => true,
                    Condition::Equal => self.equal,
                    Condition::Below => !(self.above || self.equal),
                    Condition::Above => self.above,
                    Condition::BelowOrEqual => !self.above,
                    Condition::AboveOrEqual => self.above || self.equal,
                    Condition::NotEqual => !self.equal,
                };
                match taken {
                    true => frame.jump(target),
                    false => Ok(()),
                }
            }
            Instruction::DelayMilliseconds => {
                let count = u32::from(frame.byte(image)?);
                self.card.delay(count * 1000);
                Ok(())
            }
            Instruction::DelayMicroseconds => {
                let count = u32::from(frame.byte(image)?);
                self.card.delay(count);
                Ok(())
            }
            Instruction::Nothing => Ok(()),
            Instruction::SkipByte => frame.byte(image).map(|_| ()),
            Instruction::SkipData => {
                let length = usize::from(frame.half(image)?);
                frame.at = frame.at.checked_add(length).ok_or(AtomError::Malformed)?;
                Ok(())
            }
            Instruction::SetDataBlock => {
                self.data_block = match frame.byte(image)? {
                    0 => 0,
                    DATA_BLOCK_THIS_TABLE => frame.base as u32,
                    index => table_at(image, self.data, usize::from(index))
                        .ok_or(AtomError::NoTable)? as u32,
                };
                Ok(())
            }
            Instruction::End | Instruction::CallTable => Ok(()),
        }
    }

    fn switch(&mut self, frame: &mut Frame<'_>) -> Result<(), AtomError> {
        let image = self.image;
        let attribute = frame.byte(image)?;
        let (value, _) = self.read(frame, Operand::of(attribute), attribute >> 3 & 7)?;
        loop {
            let at = frame.at;
            if half(image, at).ok_or(AtomError::Malformed)? == CASE_END {
                frame.at += 2;
                return Ok(());
            }
            if frame.byte(image)? != CASE_MAGIC {
                return Err(AtomError::Malformed);
            }
            let (case, _) = self.read(frame, Operand::Immediate, attribute >> 3 & 7)?;
            let target = frame.half(image)?;
            if case == value {
                return frame.jump(target);
            }
        }
    }

    fn arithmetic(
        &mut self,
        frame: &mut Frame<'_>,
        operation: Operation,
        destination: Operand,
    ) -> Result<(), AtomError> {
        let image = self.image;
        let mut attribute = frame.byte(image)?;
        let default_destination = matches!(
            operation,
            Operation::ShiftLeftBy | Operation::ShiftRightBy | Operation::Clear
        );
        if default_destination {
            attribute &= 0x38;
            attribute |= DEFAULT_DESTINATION[usize::from(attribute >> 3)] << 6;
        }
        let source_align = attribute >> 3 & 7;
        let destination_align =
            DESTINATION_ALIGN[usize::from(source_align)][usize::from(attribute >> 6)];
        let destination_at = frame.at;
        let (current, saved) = match operation {
            Operation::Move if source_align == 0 => {
                frame.skip(image, destination)?;
                (0, 0)
            }
            _ => self.read(frame, destination, destination_align)?,
        };
        let source = |state: &mut Self, frame: &mut Frame<'_>| {
            state
                .read(frame, Operand::of(attribute), source_align)
                .map(|(value, _)| value)
        };
        let result = match operation {
            Operation::Move => Some(source(self, frame)?),
            Operation::And => Some(current & source(self, frame)?),
            Operation::Or => Some(current | source(self, frame)?),
            Operation::Xor => Some(current ^ source(self, frame)?),
            Operation::Add => Some(current.wrapping_add(source(self, frame)?)),
            Operation::Subtract => Some(current.wrapping_sub(source(self, frame)?)),
            Operation::ShiftLeftBy => Some(current.wrapping_shl(u32::from(frame.byte(image)?))),
            Operation::ShiftRightBy => Some(current.wrapping_shr(u32::from(frame.byte(image)?))),
            Operation::ShiftLeft | Operation::ShiftRight => {
                let by = source(self, frame)?;
                let shifted = match operation {
                    Operation::ShiftLeft => saved.wrapping_shl(by),
                    _ => saved.wrapping_shr(by),
                };
                let align = usize::from(destination_align);
                let mask = ALIGN_MASKS[align] << ALIGN_SHIFTS[align];
                Some((shifted & mask) >> ALIGN_SHIFTS[align])
            }
            Operation::Clear => Some(0),
            Operation::Mask => {
                let mask = self.read(frame, Operand::Immediate, source_align)?.0;
                let bits = source(self, frame)?;
                Some(current & mask | bits)
            }
            Operation::Multiply => {
                self.quotient = current.wrapping_mul(source(self, frame)?);
                None
            }
            Operation::Divide => {
                let divisor = source(self, frame)?;
                (self.quotient, self.remainder) = match divisor {
                    0 => (0, 0),
                    divisor => (current / divisor, current % divisor),
                };
                None
            }
            Operation::Multiply32 => {
                let product = u64::from(current) * u64::from(source(self, frame)?);
                (self.quotient, self.remainder) = (product as u32, (product >> 32) as u32);
                None
            }
            Operation::Divide32 => {
                let divisor = u64::from(source(self, frame)?);
                let dividend = u64::from(self.remainder) << 32 | u64::from(current);
                (self.quotient, self.remainder) = match divisor {
                    0 => (0, 0),
                    divisor => ((dividend / divisor) as u32, (dividend / divisor >> 32) as u32),
                };
                None
            }
            Operation::Compare => {
                let other = source(self, frame)?;
                self.equal = current == other;
                self.above = current > other;
                None
            }
            Operation::Test => {
                self.equal = current & source(self, frame)? == 0;
                None
            }
        };
        if let Some(result) = result {
            let after = frame.at;
            frame.at = destination_at;
            self.write(frame, destination, destination_align, result, saved)?;
            frame.at = after;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Card {
        registers: BTreeMap<u32, u32>,
        delayed: u32,
    }

    impl AtomCard for Card {
        fn read_register(&mut self, index: u32) -> u32 {
            self.registers.get(&index).copied().unwrap_or(0)
        }
        fn write_register(&mut self, index: u32, value: u32) {
            self.registers.insert(index, value);
        }
        fn read_pll(&mut self, _: u32) -> u32 {
            0
        }
        fn write_pll(&mut self, _: u32, _: u32) {}
        fn read_memory_controller(&mut self, _: u32) -> u32 {
            0
        }
        fn write_memory_controller(&mut self, _: u32, _: u32) {}
        fn delay(&mut self, microseconds: u32) {
            self.delayed += microseconds;
        }
    }

    const MOVE_REG: u8 = 1;
    const MOVE_PS: u8 = 2;
    const MOVE_WS: u8 = 3;
    const OR_REG: u8 = 13;
    const ADD_WS: u8 = 45;
    const COMPARE_WS: u8 = 62;
    const JUMP_BELOW: u8 = 69;
    const DELAY_US: u8 = 81;
    const CALL: u8 = 82;
    const EOT: u8 = 91;
    const SWITCH: u8 = 66;
    const IMM: u8 = 5;
    const PS: u8 = 1;
    const WS: u8 = 2;
    const BYTE0: u8 = 4 << 3;
    const WORD8: u8 = 2 << 3;

    fn image_with(tables: &[Vec<u8>]) -> Vec<u8> {
        let mut image = vec![0u8; 0x100];
        image[0] = 0x55;
        image[1] = 0xaa;
        image[ROM_HEADER_POINTER..ROM_HEADER_POINTER + 2].copy_from_slice(&0x80u16.to_le_bytes());
        image[0x84..0x88].copy_from_slice(b"ATOM");
        image[0x80 + MASTER_COMMAND_TABLE..0x80 + MASTER_COMMAND_TABLE + 2]
            .copy_from_slice(&0xc0u16.to_le_bytes());
        for (index, body) in tables.iter().enumerate() {
            let at = image.len();
            let entry = 0xc0 + TABLE_HEADER_BYTES + 2 * index;
            image[entry..entry + 2].copy_from_slice(&(at as u16).to_le_bytes());
            let mut table = vec![0u8; CODE];
            table[0..2].copy_from_slice(&((CODE + body.len()) as u16).to_le_bytes());
            table[WORKSPACE_SIZE] = 4;
            table[PARAMETER_SIZE] = 8;
            table.extend_from_slice(body);
            image.extend(table);
        }
        image
    }

    #[test]
    fn a_register_takes_a_parameter_then_bits_ored_in() {
        let body = vec![
            MOVE_REG, PS, 0x10, 0x00, 0,
            OR_REG, IMM | BYTE0, 0x10, 0x00, 0x80,
            EOT,
        ];
        let image = image_with(&[body]);
        let interpreter = Interpreter::new(&image).unwrap();
        let mut card = Card::default();
        interpreter.run(&mut card, 0, &mut [0x1234_0000, 0]).unwrap();
        assert_eq!(card.registers[&0x10], 0x1234_0080);
    }

    #[test]
    fn a_loop_counts_with_a_compare_and_a_jump_below() {
        let body = vec![
            ADD_WS, IMM | BYTE0, 0, 1,
            DELAY_US, 5,
            COMPARE_WS, IMM | BYTE0, 0, 3,
            JUMP_BELOW, CODE as u8, 0,
            MOVE_PS, WS, 1, 0,
            EOT,
        ];
        let image = image_with(&[body]);
        let interpreter = Interpreter::new(&image).unwrap();
        let mut card = Card::default();
        let mut parameters = [0u32; 2];
        interpreter.run(&mut card, 0, &mut parameters).unwrap();
        assert_eq!(card.delayed, 15);
        assert_eq!(parameters[1], 3);
    }

    #[test]
    fn a_word_destination_keeps_the_rest_of_the_register() {
        let body = vec![
            MOVE_REG, IMM | WORD8 | 1 << 6, 0x20, 0x00, 0xcd, 0xab,
            EOT,
        ];
        let image = image_with(&[body]);
        let interpreter = Interpreter::new(&image).unwrap();
        let mut card = Card::default();
        card.registers.insert(0x20, 0x1122_3344);
        interpreter.run(&mut card, 0, &mut [0; 2]).unwrap();
        assert_eq!(card.registers[&0x20], 0x11ab_cd44);
    }

    #[test]
    fn a_called_table_sees_the_parameters_past_the_caller_s() {
        let callee = vec![MOVE_PS, IMM | BYTE0, 0, 0x5a, EOT];
        let caller = vec![CALL, 1, MOVE_WS, PS, 0, 2, EOT];
        let image = image_with(&[caller, callee]);
        let interpreter = Interpreter::new(&image).unwrap();
        let mut parameters = [7u32, 0, 0];
        interpreter.run(&mut Card::default(), 0, &mut parameters).unwrap();
        assert_eq!(parameters, [7, 0, 0x5a]);
    }

    #[test]
    fn a_switch_jumps_to_its_matching_case() {
        let body = vec![
            SWITCH, PS | BYTE0, 0,
            CASE_MAGIC, 1, 21, 0,
            CASE_MAGIC, 2, 26, 0,
            0x5a, 0x5a,
            EOT, EOT,
            MOVE_PS, IMM | BYTE0, 1, 0x11, EOT,
            MOVE_PS, IMM | BYTE0, 1, 0x22, EOT,
        ];
        let image = image_with(&[body]);
        let interpreter = Interpreter::new(&image).unwrap();
        let mut parameters = [2u32, 0];
        interpreter.run(&mut Card::default(), 0, &mut parameters).unwrap();
        assert_eq!(parameters[1], 0x22);
    }

    #[test]
    fn io_ports_and_runaway_loops_stop_the_table() {
        let ports = vec![55, 1, 0, EOT];
        let forever = vec![67, CODE as u8, 0];
        let image = image_with(&[ports, forever]);
        let interpreter = Interpreter::new(&image).unwrap();
        assert_eq!(
            interpreter.run(&mut Card::default(), 0, &mut [0; 2]),
            Err(AtomError::Unsupported)
        );
        assert_eq!(
            interpreter.run(&mut Card::default(), 1, &mut [0; 2]),
            Err(AtomError::Runaway)
        );
        assert_eq!(
            interpreter.run(&mut Card::default(), 5, &mut [0; 2]),
            Err(AtomError::NoTable)
        );
    }
}
