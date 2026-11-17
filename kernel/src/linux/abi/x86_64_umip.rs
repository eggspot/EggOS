//! The five instructions UMIP makes fault in ring 3 (`SGDT`, `SIDT`, `SLDT`, `SMSW`, `STR`),
//! done for the program as Linux does them (D617): each stores a fixed value that tells the
//! program nothing about the kernel, and the program carries on after the instruction.

use eggos_hal::user::UserRegisters;

const LONGEST_INSTRUCTION: usize = 15;
const OPERAND_SIZE_PREFIX: u8 = 0x66;
const ADDRESS_SIZE_PREFIX: u8 = 0x67;
const FS_PREFIX: u8 = 0x64;
const GS_PREFIX: u8 = 0x65;
const IGNORED_PREFIXES: [u8; 6] = [0xf2, 0xf3, 0x2e, 0x36, 0x3e, 0x26];
const TWO_BYTE_ESCAPE: u8 = 0x0f;
const GROUP_6: u8 = 0x00;
const GROUP_7: u8 = 0x01;
const REX_W: u8 = 8;
const REX_X: u8 = 2;
const REX_B: u8 = 1;
const SP_INDEX: u8 = 4;

/// What `SGDT` and `SIDT` report: Linux's values, an address in the kernel half that is not the
/// table's, and a limit of 0.
pub const FAKE_GDT_BASE: u64 = 0xffff_ffff_fffe_0000;
pub const FAKE_IDT_BASE: u64 = 0xffff_ffff_ffff_0000;
/// What `SMSW` reports: `CR0`'s PE, MP, ET and NE, as Linux does.
pub const FAKE_MACHINE_STATUS: u16 = 0x0033;
/// What `STR` reports: the selector of a TSS, as Linux does.
pub const FAKE_TASK_REGISTER: u16 = 0x0040;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Instruction {
    StoreGdt,
    StoreIdt,
    StoreLdt,
    StoreMachineStatus,
    StoreTaskRegister,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operand {
    Register(u8),
    Memory(u64),
}

fn register(registers: &UserRegisters, number: u8) -> u64 {
    match number {
        0 => registers.rax,
        1 => registers.rcx,
        2 => registers.rdx,
        3 => registers.rbx,
        4 => registers.rsp,
        5 => registers.rbp,
        6 => registers.rsi,
        7 => registers.rdi,
        8 => registers.r8,
        9 => registers.r9,
        10 => registers.r10,
        11 => registers.r11,
        12 => registers.r12,
        13 => registers.r13,
        14 => registers.r14,
        _ => registers.r15,
    }
}

fn register_mut(registers: &mut UserRegisters, number: u8) -> &mut u64 {
    match number {
        0 => &mut registers.rax,
        1 => &mut registers.rcx,
        2 => &mut registers.rdx,
        3 => &mut registers.rbx,
        4 => &mut registers.rsp,
        5 => &mut registers.rbp,
        6 => &mut registers.rsi,
        7 => &mut registers.rdi,
        8 => &mut registers.r8,
        9 => &mut registers.r9,
        10 => &mut registers.r10,
        11 => &mut registers.r11,
        12 => &mut registers.r12,
        13 => &mut registers.r13,
        14 => &mut registers.r14,
        _ => &mut registers.r15,
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn next(&mut self) -> Option<u8> {
        let byte = *self.bytes.get(self.at)?;
        self.at += 1;
        Some(byte)
    }

    fn displacement(&mut self, bytes: usize) -> Option<i64> {
        let mut value = 0u32;
        for shift in 0..bytes {
            value |= u32::from(self.next()?) << (8 * shift);
        }
        Some(match bytes {
            1 => i64::from(value as u8 as i8),
            _ => i64::from(value as i32),
        })
    }
}

fn instruction(escape: u8, field: u8, register_form: bool) -> Option<Instruction> {
    match (escape, field, register_form) {
        (GROUP_7, 0, false) => Some(Instruction::StoreGdt),
        (GROUP_7, 1, false) => Some(Instruction::StoreIdt),
        (GROUP_7, 4, _) => Some(Instruction::StoreMachineStatus),
        (GROUP_6, 0, _) => Some(Instruction::StoreLdt),
        (GROUP_6, 1, _) => Some(Instruction::StoreTaskRegister),
        _ => None,
    }
}

fn memory_operand(
    cursor: &mut Cursor<'_>,
    registers: &UserRegisters,
    modrm: u8,
    rex: u8,
    address32: bool,
) -> Option<(u64, bool)> {
    let mode = modrm >> 6;
    let rm = modrm & 7;
    let mut rip_relative = false;
    let mut address = match rm {
        4 => {
            let sib = cursor.next()?;
            let scale = sib >> 6;
            let index = ((sib >> 3) & 7) | if rex & REX_X != 0 { 8 } else { 0 };
            let base = (sib & 7) | if rex & REX_B != 0 { 8 } else { 0 };
            let indexed = match index == SP_INDEX {
                true => 0,
                false => register(registers, index) << scale,
            };
            let based = match (base & 7 == 5, mode) {
                (true, 0) => cursor.displacement(4)? as u64,
                _ => register(registers, base),
            };
            indexed.wrapping_add(based)
        }
        5 if mode == 0 => {
            rip_relative = true;
            cursor.displacement(4)? as u64
        }
        rm => register(registers, rm | if rex & REX_B != 0 { 8 } else { 0 }),
    };
    let displacement = match mode {
        1 => cursor.displacement(1)?,
        2 => cursor.displacement(4)?,
        _ => 0,
    };
    address = address.wrapping_add_signed(displacement);
    if address32 {
        address &= u64::from(u32::MAX);
    }
    Some((address, rip_relative))
}

/// Does the UMIP instruction at the program's `rip`, reading its bytes with `read` and storing
/// its result with `write`: whether the bytes were one of the five (with a memory operand the
/// program may write), so the program goes on after it.
pub fn emulate(
    registers: &mut UserRegisters,
    read: &mut dyn FnMut(u64, &mut [u8]) -> bool,
    write: &mut dyn FnMut(u64, &[u8]) -> bool,
) -> bool {
    let mut bytes = [0u8; LONGEST_INSTRUCTION];
    let available = (1..=LONGEST_INSTRUCTION)
        .rev()
        .find(|&length| read(registers.rip, &mut bytes[..length]))
        .unwrap_or(0);
    let mut cursor = Cursor {
        bytes: &bytes[..available],
        at: 0,
    };
    let mut operand16 = false;
    let mut address32 = false;
    let mut rex = 0u8;
    let escape = loop {
        let Some(byte) = cursor.next() else {
            return false;
        };
        match byte {
            OPERAND_SIZE_PREFIX => operand16 = true,
            ADDRESS_SIZE_PREFIX => address32 = true,
            FS_PREFIX | GS_PREFIX => return false,
            byte if IGNORED_PREFIXES.contains(&byte) => {}
            0x40..=0x4f => rex = byte,
            TWO_BYTE_ESCAPE => match cursor.next() {
                Some(escape @ (GROUP_6 | GROUP_7)) => break escape,
                _ => return false,
            },
            _ => return false,
        }
    };
    let Some(modrm) = cursor.next() else {
        return false;
    };
    let register_form = modrm >> 6 == 3;
    let Some(found) = instruction(escape, (modrm >> 3) & 7, register_form) else {
        return false;
    };
    let operand = match register_form {
        true => Operand::Register((modrm & 7) | if rex & REX_B != 0 { 8 } else { 0 }),
        false => {
            let Some((address, rip_relative)) =
                memory_operand(&mut cursor, registers, modrm, rex, address32)
            else {
                return false;
            };
            Operand::Memory(address)
                .with_rip(rip_relative, registers.rip.wrapping_add(cursor.at as u64))
        }
    };
    let stored = match (found, operand) {
        (Instruction::StoreGdt | Instruction::StoreIdt, Operand::Memory(address)) => {
            let base = match found {
                Instruction::StoreGdt => FAKE_GDT_BASE,
                _ => FAKE_IDT_BASE,
            };
            let mut table = [0u8; 10];
            table[2..].copy_from_slice(&base.to_le_bytes());
            write(address, &table)
        }
        (_, Operand::Memory(address)) => write(address, &value_of(found).to_le_bytes()),
        (_, Operand::Register(number)) => {
            let value = u64::from(value_of(found));
            let target = register_mut(registers, number);
            *target = match (operand16, rex & REX_W != 0) {
                (true, false) => (*target & !0xffff) | value,
                _ => value,
            };
            true
        }
    };
    if stored {
        registers.rip = registers.rip.wrapping_add(cursor.at as u64);
    }
    stored
}

fn value_of(instruction: Instruction) -> u16 {
    match instruction {
        Instruction::StoreMachineStatus => FAKE_MACHINE_STATUS,
        Instruction::StoreTaskRegister => FAKE_TASK_REGISTER,
        _ => 0,
    }
}

impl Operand {
    fn with_rip(self, rip_relative: bool, next_instruction: u64) -> Self {
        match (self, rip_relative) {
            (Self::Memory(displacement), true) => {
                Self::Memory(next_instruction.wrapping_add(displacement))
            }
            (operand, _) => operand,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(code: &[u8], registers: &mut UserRegisters, memory: &mut [u8; 16]) -> bool {
        let base = 0x1000u64;
        registers.rip = 0x40_0000;
        let code = code.to_vec();
        let mut read = |address: u64, out: &mut [u8]| {
            let at = (address - 0x40_0000) as usize;
            match code.get(at..at + out.len()) {
                Some(bytes) => {
                    out.copy_from_slice(bytes);
                    true
                }
                None => false,
            }
        };
        let mut write = |address: u64, bytes: &[u8]| {
            let at = (address - base) as usize;
            memory[at..at + bytes.len()].copy_from_slice(bytes);
            true
        };
        emulate(registers, &mut read, &mut write)
    }

    #[test]
    fn sgdt_through_rax_stores_a_zero_limit_and_the_fake_base() {
        let mut registers = UserRegisters::default();
        registers.rax = 0x1000;
        let mut memory = [0u8; 16];
        assert!(run(&[0x0f, 0x01, 0x00], &mut registers, &mut memory));
        assert_eq!(&memory[..2], &[0, 0]);
        assert_eq!(&memory[2..10], &FAKE_GDT_BASE.to_le_bytes());
        assert_eq!(registers.rip, 0x40_0003);
    }

    #[test]
    fn smsw_into_a_register_and_str_with_a_displacement() {
        let mut registers = UserRegisters::default();
        let mut memory = [0u8; 16];
        assert!(run(&[0x0f, 0x01, 0xe0], &mut registers, &mut memory));
        assert_eq!(registers.rax, u64::from(FAKE_MACHINE_STATUS));
        registers.rbx = 0x0ff8;
        assert!(run(&[0x0f, 0x00, 0x4b, 0x08], &mut registers, &mut memory));
        assert_eq!(&memory[..2], &FAKE_TASK_REGISTER.to_le_bytes());
    }

    #[test]
    fn other_instructions_are_left_alone() {
        let mut registers = UserRegisters::default();
        let mut memory = [0u8; 16];
        assert!(!run(&[0x0f, 0x01, 0xc1], &mut registers, &mut memory));
        assert!(!run(&[0x90], &mut registers, &mut memory));
        assert!(!run(&[0x65, 0x0f, 0x01, 0x00], &mut registers, &mut memory));
    }
}
