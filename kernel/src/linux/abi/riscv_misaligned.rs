//! Misaligned loads and stores on RISC-V harts that trap them (D560): the faulting instruction
//! is decoded and done a byte at a time through the program's memory, as Linux does, so a
//! program that accesses unaligned data runs on every hart, slower where the hart traps. Integer
//! loads and stores, 32-bit and compressed; a floating-point access, an atomic one or an
//! instruction that cannot be read is left to the fault's signal (`SIGBUS`).

/// Bytes of an `x` register in the program's ABI.
const XLEN_BYTES: usize = super::WORD_BYTES;

struct Access {
    store: bool,
    width: usize,
    signed: bool,
    register: usize,
    base: usize,
    offset: i64,
    length: u64,
}

fn sign_extend(value: u64, bits: u32) -> i64 {
    ((value << (64 - bits)) as i64) >> (64 - bits)
}

fn decode_full(instruction: u32) -> Option<Access> {
    let opcode = instruction & 0x7f;
    let funct3 = instruction >> 12 & 7;
    let rd = (instruction >> 7 & 31) as usize;
    let rs1 = (instruction >> 15 & 31) as usize;
    let rs2 = (instruction >> 20 & 31) as usize;
    match opcode {
        0x03 => {
            let (width, signed) = match funct3 {
                1 => (2, true),
                2 => (4, true),
                3 if XLEN_BYTES == 8 => (8, true),
                5 => (2, false),
                6 if XLEN_BYTES == 8 => (4, false),
                _ => return None,
            };
            Some(Access {
                store: false,
                width,
                signed,
                register: rd,
                base: rs1,
                offset: sign_extend(u64::from(instruction >> 20), 12),
                length: 4,
            })
        }
        0x23 => {
            let width = match funct3 {
                1 => 2,
                2 => 4,
                3 if XLEN_BYTES == 8 => 8,
                _ => return None,
            };
            let immediate = (instruction >> 25) << 5 | (instruction >> 7 & 31);
            Some(Access {
                store: true,
                width,
                signed: false,
                register: rs2,
                base: rs1,
                offset: sign_extend(u64::from(immediate), 12),
                length: 4,
            })
        }
        _ => None,
    }
}

fn bits(instruction: u16, high: u32, low: u32) -> u64 {
    u64::from(instruction >> low) & ((1 << (high - low + 1)) - 1)
}

fn decode_compressed(instruction: u16) -> Option<Access> {
    let quadrant = instruction & 3;
    let funct3 = instruction >> 13;
    let rd_prime = 8 + bits(instruction, 4, 2) as usize;
    let rs1_prime = 8 + bits(instruction, 9, 7) as usize;
    let wide = XLEN_BYTES == 8;
    let access = |store, width, register, base, offset: u64| Access {
        store,
        width,
        signed: true,
        register,
        base,
        offset: offset as i64,
        length: 2,
    };
    match (quadrant, funct3) {
        (0, 2) => Some(access(
            false,
            4,
            rd_prime,
            rs1_prime,
            bits(instruction, 12, 10) << 3 | bits(instruction, 6, 6) << 2 | bits(instruction, 5, 5) << 6,
        )),
        (0, 3) if wide => Some(access(
            false,
            8,
            rd_prime,
            rs1_prime,
            bits(instruction, 12, 10) << 3 | bits(instruction, 6, 5) << 6,
        )),
        (0, 6) => Some(access(
            true,
            4,
            rd_prime,
            rs1_prime,
            bits(instruction, 12, 10) << 3 | bits(instruction, 6, 6) << 2 | bits(instruction, 5, 5) << 6,
        )),
        (0, 7) if wide => Some(access(
            true,
            8,
            rd_prime,
            rs1_prime,
            bits(instruction, 12, 10) << 3 | bits(instruction, 6, 5) << 6,
        )),
        (2, 2) => Some(access(
            false,
            4,
            bits(instruction, 11, 7) as usize,
            2,
            bits(instruction, 12, 12) << 5 | bits(instruction, 6, 4) << 2 | bits(instruction, 3, 2) << 6,
        )),
        (2, 3) if wide => Some(access(
            false,
            8,
            bits(instruction, 11, 7) as usize,
            2,
            bits(instruction, 12, 12) << 5 | bits(instruction, 6, 5) << 3 | bits(instruction, 4, 2) << 6,
        )),
        (2, 6) => Some(access(
            true,
            4,
            bits(instruction, 6, 2) as usize,
            2,
            bits(instruction, 12, 9) << 2 | bits(instruction, 8, 7) << 6,
        )),
        (2, 7) if wide => Some(access(
            true,
            8,
            bits(instruction, 6, 2) as usize,
            2,
            bits(instruction, 12, 10) << 3 | bits(instruction, 9, 7) << 6,
        )),
        _ => None,
    }
}

/// Does the misaligned integer load or store at `registers.pc` that trapped, reading and writing
/// the program's memory through `read` and `write`: whether it did, the program then going on
/// past it.
pub fn emulate(
    registers: &mut eggos_hal::user::UserRegisters,
    read: &mut dyn FnMut(u64, &mut [u8]) -> bool,
    write: &mut dyn FnMut(u64, &[u8]) -> bool,
) -> bool {
    let mut first = [0u8; 2];
    if !read(registers.pc, &mut first) {
        return false;
    }
    let half = u16::from_le_bytes(first);
    let access = if half & 3 == 3 {
        let mut second = [0u8; 2];
        if !read(registers.pc + 2, &mut second) {
            return false;
        }
        let instruction = u32::from(half) | u32::from(u16::from_le_bytes(second)) << 16;
        decode_full(instruction)
    } else {
        decode_compressed(half)
    };
    let Some(access) = access else {
        return false;
    };
    let address = registers.x[access.base].wrapping_add(access.offset as u64);
    if access.store {
        let value = registers.x[access.register].to_le_bytes();
        if !write(address, &value[..access.width]) {
            return false;
        }
    } else {
        let mut bytes = [0u8; 8];
        if !read(address, &mut bytes[..access.width]) {
            return false;
        }
        let raw = u64::from_le_bytes(bytes);
        let value = match access.signed {
            true => sign_extend(raw, access.width as u32 * 8) as u64,
            false => raw,
        };
        let value = match XLEN_BYTES {
            4 => value & 0xffff_ffff,
            _ => value,
        };
        if access.register != 0 {
            registers.x[access.register] = value;
        }
    }
    registers.pc = registers.pc.wrapping_add(access.length);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_and_stores_decode_with_their_widths_and_offsets() {
        let lw = 0x0045_2283u32;
        let access = decode_full(lw).unwrap();
        assert_eq!((access.store, access.width, access.register, access.base, access.offset), (false, 4, 5, 10, 4));
        let sd = 0x00b5_3423u32;
        let access = decode_full(sd).unwrap();
        assert_eq!((access.store, access.width, access.register, access.base, access.offset), (true, 8, 11, 10, 8));
    }
}
