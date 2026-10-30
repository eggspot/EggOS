//! The compact unwind information Apple's linker writes in an image's `__TEXT,__unwind_info`
//! section (D175): for each function its start, an encoding of how its frame is laid out, its
//! language-specific data area (LSDA) and its personality. [`find`] looks a program counter up;
//! [`step_arm64`] and [`step_x86_64`] use an encoding to recover the caller's registers from a
//! frame's. Encodings that defer to DWARF (`__eh_frame`) are refused. Pure logic with host tests.

const VERSION: u32 = 1;
const INDEX_ENTRY_BYTES: usize = 12;
const LSDA_ENTRY_BYTES: usize = 8;
const REGULAR_PAGE: u32 = 2;
const COMPRESSED_PAGE: u32 = 3;
const COMPRESSED_OFFSET_MASK: u32 = 0x00ff_ffff;
const COMPRESSED_ENCODING_SHIFT: u32 = 24;

const HAS_LSDA: u32 = 0x4000_0000;
const PERSONALITY_MASK: u32 = 0x3000_0000;
const PERSONALITY_SHIFT: u32 = 28;
const MODE_MASK: u32 = 0x0f00_0000;

#[cfg(any(test, target_arch = "aarch64"))]
const ARM64_FRAMELESS: u32 = 0x0200_0000;
#[cfg(any(test, target_arch = "aarch64"))]
const ARM64_DWARF: u32 = 0x0300_0000;
#[cfg(any(test, target_arch = "aarch64"))]
const ARM64_FRAME: u32 = 0x0400_0000;
#[cfg(any(test, target_arch = "aarch64"))]
const ARM64_STACK_SIZE_SHIFT: u32 = 12;
#[cfg(any(test, target_arch = "aarch64"))]
const ARM64_STACK_SIZE_MASK: u32 = 0xfff;
#[cfg(any(test, target_arch = "aarch64"))]
const ARM64_GENERAL_PAIRS: u32 = 5;
#[cfg(any(test, target_arch = "aarch64"))]
const ARM64_VECTOR_PAIRS_SHIFT: u32 = 8;
#[cfg(any(test, target_arch = "aarch64"))]
const ARM64_VECTOR_PAIRS: u32 = 4;
#[cfg(any(test, target_arch = "aarch64"))]
const ARM64_FIRST_PAIRED: usize = 19;
#[cfg(any(test, target_arch = "aarch64"))]
const ARM64_FRAME_POINTER: usize = 29;
#[cfg(any(test, target_arch = "aarch64"))]
const ARM64_LINK_REGISTER: usize = 30;

#[cfg(any(test, target_arch = "x86_64"))]
const X86_64_RBP_FRAME: u32 = 0x0100_0000;
#[cfg(any(test, target_arch = "x86_64"))]
const X86_64_STACK_IMMEDIATE: u32 = 0x0200_0000;
#[cfg(any(test, target_arch = "x86_64"))]
const X86_64_STACK_INDIRECT: u32 = 0x0300_0000;
#[cfg(any(test, target_arch = "x86_64"))]
const X86_64_DWARF: u32 = 0x0400_0000;
#[cfg(any(test, target_arch = "x86_64"))]
const X86_64_SAVED_REGISTERS: usize = 5;
#[cfg(any(test, target_arch = "x86_64"))]
const X86_64_MAX_STACK_REGISTERS: usize = 6;

/// The x86_64 registers' DWARF numbers, the index into [`X86_64Registers::general`].
#[cfg(any(test, target_arch = "x86_64"))]
pub mod x86_64 {
    pub const RAX: usize = 0;
    pub const RDX: usize = 1;
    pub const RBX: usize = 3;
    pub const RBP: usize = 6;
    pub const RSP: usize = 7;
    pub const R12: usize = 12;
    pub const R13: usize = 13;
    pub const R14: usize = 14;
    pub const R15: usize = 15;
}

/// What the section says about the function holding a program counter. Offsets are from the
/// image's Mach-O header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FunctionInfo {
    pub start: u32,
    pub end: u32,
    pub encoding: u32,
    /// Its LSDA's offset, when it has one.
    pub lsda: Option<u32>,
    /// The offset of the pointer to its personality routine, when it has one.
    pub personality: Option<u32>,
}

fn word(bytes: &[u8], at: usize) -> Option<u32> {
    let slice = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn half(bytes: &[u8], at: usize) -> Option<u16> {
    let slice = bytes.get(at..at.checked_add(2)?)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

fn last_at_most(count: usize, target: u32, value: &dyn Fn(usize) -> Option<u32>) -> Option<usize> {
    if count == 0 || value(0)? > target {
        return None;
    }
    let (mut low, mut high) = (0, count);
    while high - low > 1 {
        let middle = low + (high - low) / 2;
        if value(middle)? <= target {
            low = middle;
        } else {
            high = middle;
        }
    }
    Some(low)
}

/// The function `pc` (an offset from the image's header, already one before a return address)
/// lies in, as the section `section` describes it; `None` when the section does not cover it
/// or is malformed.
pub fn find(section: &[u8], pc: u32) -> Option<FunctionInfo> {
    if word(section, 0)? != VERSION {
        return None;
    }
    let common_offset = word(section, 4)? as usize;
    let common_count = word(section, 8)?;
    let personality_offset = word(section, 12)? as usize;
    let personality_count = word(section, 16)?;
    let index_offset = word(section, 20)? as usize;
    let index_count = word(section, 24)? as usize;
    if index_count < 2 {
        return None;
    }
    let index_field = |entry: usize, field: usize| {
        word(
            section,
            index_offset
                .checked_add(entry.checked_mul(INDEX_ENTRY_BYTES)?)?
                .checked_add(field)?,
        )
    };
    let last = index_count - 1;
    if pc >= index_field(last, 0)? {
        return None;
    }
    let entry = last_at_most(last, pc, &|at| index_field(at, 0))?;
    let first_function = index_field(entry, 0)?;
    let next_function = index_field(entry + 1, 0)?;
    let page = index_field(entry, 4)? as usize;
    let lsda_start = index_field(entry, 8)? as usize;
    let lsda_end = index_field(entry + 1, 8)? as usize;
    if page == 0 {
        return None;
    }
    let entries = page.checked_add(usize::from(half(section, page.checked_add(4)?)?))?;
    let count = usize::from(half(section, page.checked_add(6)?)?);
    let (start, end, encoding) = match word(section, page)? {
        REGULAR_PAGE => {
            let function = |at: usize| word(section, entries.checked_add(at.checked_mul(8)?)?);
            let at = last_at_most(count, pc, &function)?;
            let end = match at + 1 < count {
                true => function(at + 1)?,
                false => next_function,
            };
            let encoding = word(section, entries + at * 8 + 4)?;
            (function(at)?, end, encoding)
        }
        COMPRESSED_PAGE => {
            let encodings = page.checked_add(usize::from(half(section, page + 8)?))?;
            let local_count = u32::from(half(section, page + 10)?);
            let raw = |at: usize| word(section, entries.checked_add(at.checked_mul(4)?)?);
            let function = |at: usize| {
                raw(at).and_then(|value| first_function.checked_add(value & COMPRESSED_OFFSET_MASK))
            };
            let at = last_at_most(count, pc, &function)?;
            let end = match at + 1 < count {
                true => function(at + 1)?,
                false => next_function,
            };
            let index = raw(at)? >> COMPRESSED_ENCODING_SHIFT;
            let encoding = match index.checked_sub(common_count) {
                None => word(section, common_offset.checked_add(index as usize * 4)?)?,
                Some(local) if local < local_count => {
                    word(section, encodings.checked_add(local as usize * 4)?)?
                }
                Some(_) => return None,
            };
            (function(at)?, end, encoding)
        }
        _ => return None,
    };
    if pc >= end {
        return None;
    }
    let lsda = match encoding & HAS_LSDA {
        0 => None,
        _ => lsda_of(section, lsda_start, lsda_end, start),
    };
    let personality = match (encoding & PERSONALITY_MASK) >> PERSONALITY_SHIFT {
        0 => None,
        index if index <= personality_count => word(
            section,
            personality_offset.checked_add((index as usize - 1) * 4)?,
        ),
        _ => None,
    };
    Some(FunctionInfo {
        start,
        end,
        encoding,
        lsda,
        personality,
    })
}

fn lsda_of(section: &[u8], start: usize, end: usize, function: u32) -> Option<u32> {
    let count = end.checked_sub(start)? / LSDA_ENTRY_BYTES;
    let (mut low, mut high) = (0, count);
    while low < high {
        let middle = low + (high - low) / 2;
        let at = start.checked_add(middle * LSDA_ENTRY_BYTES)?;
        let found = word(section, at)?;
        match found.cmp(&function) {
            core::cmp::Ordering::Equal => return word(section, at + 4),
            core::cmp::Ordering::Less => low = middle + 1,
            core::cmp::Ordering::Greater => high = middle,
        }
    }
    None
}

/// Why a frame could not be stepped over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepError {
    /// Only the DWARF unwind information (`__eh_frame`) describes the frame.
    Dwarf,
    /// The encoding's mode is not one the format defines.
    Unknown,
    /// A saved register's slot could not be read.
    Unreadable,
}

/// An arm64 frame's registers, as the exception stubs save them: x0 to x30, the stack pointer,
/// the program counter and d8 to d15.
#[cfg(any(test, target_arch = "aarch64"))]
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Arm64Registers {
    pub x: [u64; 31],
    pub sp: u64,
    pub pc: u64,
    pub d: [u64; 8],
}

#[cfg(any(test, target_arch = "aarch64"))]
fn restore_arm64_pairs(
    encoding: u32,
    at: &mut u64,
    registers: &mut Arm64Registers,
    read: &dyn Fn(u64) -> Option<u64>,
) -> Result<(), StepError> {
    for pair in 0..ARM64_GENERAL_PAIRS {
        if encoding & (1 << pair) != 0 {
            let first = ARM64_FIRST_PAIRED + 2 * pair as usize;
            registers.x[first] = read(*at).ok_or(StepError::Unreadable)?;
            *at = at.wrapping_sub(8);
            registers.x[first + 1] = read(*at).ok_or(StepError::Unreadable)?;
            *at = at.wrapping_sub(8);
        }
    }
    for pair in 0..ARM64_VECTOR_PAIRS {
        if encoding & (1 << (ARM64_VECTOR_PAIRS_SHIFT + pair)) != 0 {
            let first = 2 * pair as usize;
            registers.d[first] = read(*at).ok_or(StepError::Unreadable)?;
            *at = at.wrapping_sub(8);
            registers.d[first + 1] = read(*at).ok_or(StepError::Unreadable)?;
            *at = at.wrapping_sub(8);
        }
    }
    Ok(())
}

/// Turns `registers`, an arm64 frame of a function whose compact encoding is `encoding`, into
/// its caller's at the return address; `read` reads a word of the stack.
#[cfg(any(test, target_arch = "aarch64"))]
pub fn step_arm64(
    encoding: u32,
    registers: &mut Arm64Registers,
    read: &dyn Fn(u64) -> Option<u64>,
) -> Result<(), StepError> {
    match encoding & MODE_MASK {
        ARM64_FRAME => {
            let frame = registers.x[ARM64_FRAME_POINTER];
            let mut at = frame.wrapping_sub(8);
            restore_arm64_pairs(encoding, &mut at, registers, read)?;
            let caller_frame = read(frame).ok_or(StepError::Unreadable)?;
            let return_address = read(frame.wrapping_add(8)).ok_or(StepError::Unreadable)?;
            registers.x[ARM64_FRAME_POINTER] = caller_frame;
            registers.sp = frame.wrapping_add(16);
            registers.pc = return_address;
            Ok(())
        }
        ARM64_FRAMELESS => {
            let size = u64::from((encoding >> ARM64_STACK_SIZE_SHIFT) & ARM64_STACK_SIZE_MASK) * 16;
            let mut at = registers.sp.wrapping_add(size).wrapping_sub(8);
            restore_arm64_pairs(encoding, &mut at, registers, read)?;
            registers.sp = registers.sp.wrapping_add(size);
            registers.pc = registers.x[ARM64_LINK_REGISTER];
            Ok(())
        }
        ARM64_DWARF => Err(StepError::Dwarf),
        _ => Err(StepError::Unknown),
    }
}

/// An x86_64 frame's registers, as the exception stubs save them: the sixteen general
/// registers by their DWARF numbers ([`x86_64`]), then the instruction pointer.
#[cfg(any(test, target_arch = "x86_64"))]
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct X86_64Registers {
    pub general: [u64; 16],
    pub rip: u64,
}

#[cfg(any(test, target_arch = "x86_64"))]
fn compact_register(number: u32) -> Result<Option<usize>, StepError> {
    Ok(match number {
        0 => None,
        1 => Some(x86_64::RBX),
        2 => Some(x86_64::R12),
        3 => Some(x86_64::R13),
        4 => Some(x86_64::R14),
        5 => Some(x86_64::R15),
        6 => Some(x86_64::RBP),
        _ => return Err(StepError::Unknown),
    })
}

/// The registers a frameless x86_64 function saved, in the order it pushed them, from the
/// encoding's permutation number of `count` registers.
#[cfg(any(test, target_arch = "x86_64"))]
pub fn stack_registers(
    count: usize,
    permutation: u32,
) -> Option<[u32; X86_64_MAX_STACK_REGISTERS]> {
    let mut rest = permutation;
    let mut digits = [0u32; X86_64_MAX_STACK_REGISTERS];
    let bases: &[u32] = match count {
        0 => &[],
        1 => &[1],
        2 => &[5, 1],
        3 => &[20, 4, 1],
        4 => &[60, 12, 3, 1],
        5 | 6 => &[120, 24, 6, 2, 1],
        _ => return None,
    };
    for (digit, &base) in digits.iter_mut().zip(bases) {
        *digit = rest / base;
        rest -= *digit * base;
    }
    let mut used = [false; 7];
    let mut order = [0u32; X86_64_MAX_STACK_REGISTERS];
    for (slot, &digit) in order.iter_mut().zip(&digits).take(count) {
        let mut renumbered = 0;
        for (register, taken) in used.iter_mut().enumerate().skip(1) {
            if *taken {
                continue;
            }
            if renumbered == digit {
                *slot = register as u32;
                *taken = true;
                break;
            }
            renumbered += 1;
        }
        if *slot == 0 {
            return None;
        }
    }
    Some(order)
}

/// Turns `registers`, an x86_64 frame of a function starting at `function_start` whose compact
/// encoding is `encoding`, into its caller's at the return address; `read` reads a word (at any
/// alignment) of the stack or the program's code.
#[cfg(any(test, target_arch = "x86_64"))]
pub fn step_x86_64(
    encoding: u32,
    function_start: u64,
    registers: &mut X86_64Registers,
    read: &dyn Fn(u64) -> Option<u64>,
) -> Result<(), StepError> {
    let mode = encoding & MODE_MASK;
    match mode {
        X86_64_RBP_FRAME => {
            let offset = u64::from((encoding >> 16) & 0xff);
            let mut locations = encoding & 0x7fff;
            let frame = registers.general[x86_64::RBP];
            let mut at = frame.wrapping_sub(8 * offset);
            for _ in 0..X86_64_SAVED_REGISTERS {
                if let Some(register) = compact_register(locations & 7)? {
                    registers.general[register] = read(at).ok_or(StepError::Unreadable)?;
                }
                at = at.wrapping_add(8);
                locations >>= 3;
            }
            registers.general[x86_64::RBP] = read(frame).ok_or(StepError::Unreadable)?;
            registers.rip = read(frame.wrapping_add(8)).ok_or(StepError::Unreadable)?;
            registers.general[x86_64::RSP] = frame.wrapping_add(16);
            Ok(())
        }
        X86_64_STACK_IMMEDIATE | X86_64_STACK_INDIRECT => {
            let size_field = u64::from((encoding >> 16) & 0xff);
            let size = match mode {
                X86_64_STACK_IMMEDIATE => size_field * 8,
                _ => {
                    let instruction = read(function_start.wrapping_add(size_field))
                        .ok_or(StepError::Unreadable)?;
                    let adjust = u64::from((encoding >> 13) & 7);
                    (instruction & 0xffff_ffff) + adjust * 8
                }
            };
            let count = ((encoding >> 10) & 7) as usize;
            let order = stack_registers(count, encoding & 0x3ff).ok_or(StepError::Unknown)?;
            let stack = registers.general[x86_64::RSP];
            let mut at = stack
                .wrapping_add(size)
                .wrapping_sub(8)
                .wrapping_sub(8 * count as u64);
            for &number in &order[..count] {
                if let Some(register) = compact_register(number)? {
                    registers.general[register] = read(at).ok_or(StepError::Unreadable)?;
                }
                at = at.wrapping_add(8);
            }
            registers.rip =
                read(stack.wrapping_add(size).wrapping_sub(8)).ok_or(StepError::Unreadable)?;
            registers.general[x86_64::RSP] = stack.wrapping_add(size);
            Ok(())
        }
        X86_64_DWARF => Err(StepError::Dwarf),
        _ => Err(StepError::Unknown),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn put_half(bytes: &mut Vec<u8>, value: u16) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    const FRAME_ENCODING: u32 = ARM64_FRAME | 1;
    const LSDA_ENCODING: u32 = ARM64_FRAME | HAS_LSDA | (1 << PERSONALITY_SHIFT);

    fn section() -> Vec<u8> {
        let mut bytes = Vec::new();
        let header = 28;
        let common = header;
        let personalities = common + 4;
        let index = personalities + 4;
        let lsdas = index + 3 * 12;
        let regular = lsdas + 8;
        let compressed = regular + 8 + 2 * 8;
        put(&mut bytes, VERSION);
        put(&mut bytes, common as u32);
        put(&mut bytes, 1);
        put(&mut bytes, personalities as u32);
        put(&mut bytes, 1);
        put(&mut bytes, index as u32);
        put(&mut bytes, 3);
        put(&mut bytes, FRAME_ENCODING);
        put(&mut bytes, 0x8010);
        for (function, page, lsda) in [
            (0x1000, regular, lsdas),
            (0x2000, compressed, lsdas + 8),
            (0x3000, 0, lsdas + 8),
        ] {
            put(&mut bytes, function);
            put(&mut bytes, page as u32);
            put(&mut bytes, lsda as u32);
        }
        put(&mut bytes, 0x1100);
        put(&mut bytes, 0x4400);
        put(&mut bytes, REGULAR_PAGE);
        put_half(&mut bytes, 8);
        put_half(&mut bytes, 2);
        for (function, encoding) in [(0x1000, FRAME_ENCODING), (0x1100, LSDA_ENCODING)] {
            put(&mut bytes, function);
            put(&mut bytes, encoding);
        }
        put(&mut bytes, COMPRESSED_PAGE);
        put_half(&mut bytes, 12);
        put_half(&mut bytes, 2);
        put_half(&mut bytes, 20);
        put_half(&mut bytes, 1);
        put(&mut bytes, 0);
        put(&mut bytes, (1 << 24) | 0x80);
        put(&mut bytes, ARM64_FRAMELESS | (2 << ARM64_STACK_SIZE_SHIFT));
        bytes
    }

    #[test]
    fn a_regular_page_gives_the_function_its_encoding_lsda_and_personality() {
        let bytes = section();
        assert_eq!(
            find(&bytes, 0x1004),
            Some(FunctionInfo {
                start: 0x1000,
                end: 0x1100,
                encoding: FRAME_ENCODING,
                lsda: None,
                personality: None,
            })
        );
        assert_eq!(
            find(&bytes, 0x1180),
            Some(FunctionInfo {
                start: 0x1100,
                end: 0x2000,
                encoding: LSDA_ENCODING,
                lsda: Some(0x4400),
                personality: Some(0x8010),
            })
        );
    }

    #[test]
    fn a_compressed_page_uses_common_and_local_encodings() {
        let bytes = section();
        let first = find(&bytes, 0x2010).unwrap();
        assert_eq!(
            (first.start, first.end, first.encoding),
            (0x2000, 0x2080, FRAME_ENCODING)
        );
        let second = find(&bytes, 0x2fff).unwrap();
        assert_eq!(
            (second.start, second.end, second.encoding),
            (
                0x2080,
                0x3000,
                ARM64_FRAMELESS | (2 << ARM64_STACK_SIZE_SHIFT)
            )
        );
    }

    #[test]
    fn addresses_outside_the_table_or_a_bad_version_find_nothing() {
        let bytes = section();
        assert_eq!(find(&bytes, 0xfff), None);
        assert_eq!(find(&bytes, 0x3000), None);
        let mut wrong = bytes.clone();
        wrong[0] = 2;
        assert_eq!(find(&wrong, 0x1004), None);
        assert_eq!(find(&bytes[..20], 0x1004), None);
    }

    #[test]
    fn an_arm64_frame_restores_its_pairs_frame_pointer_and_return_address() {
        let mut registers = Arm64Registers::default();
        registers.x[29] = 0x7000;
        registers.sp = 0x6f00;
        let stack = [
            (0x7000 - 8, 19u64),
            (0x7000 - 16, 20),
            (0x7000, 0x7100),
            (0x7008, 0x1_0000_1234),
        ];
        let read = |address: u64| {
            stack
                .iter()
                .find(|(at, _)| *at == address)
                .map(|(_, value)| *value)
        };
        step_arm64(ARM64_FRAME | 1, &mut registers, &read).unwrap();
        assert_eq!((registers.x[19], registers.x[20]), (19, 20));
        assert_eq!(registers.x[29], 0x7100);
        assert_eq!(registers.sp, 0x7010);
        assert_eq!(registers.pc, 0x1_0000_1234);
    }

    #[test]
    fn an_arm64_frameless_function_returns_through_the_link_register() {
        let mut registers = Arm64Registers::default();
        registers.sp = 0x8000;
        registers.x[30] = 0x1_0000_0040;
        step_arm64(
            ARM64_FRAMELESS | (3 << ARM64_STACK_SIZE_SHIFT),
            &mut registers,
            &|_| None,
        )
        .unwrap();
        assert_eq!(registers.sp, 0x8030);
        assert_eq!(registers.pc, 0x1_0000_0040);
        assert_eq!(
            step_arm64(ARM64_DWARF, &mut registers, &|_| None),
            Err(StepError::Dwarf)
        );
    }

    #[test]
    fn an_x86_64_rbp_frame_restores_its_saved_registers() {
        let mut registers = X86_64Registers::default();
        registers.general[x86_64::RBP] = 0x9000;
        let stack = [
            (0x9000 - 16, 0xb0u64),
            (0x9000 - 8, 0xc0),
            (0x9000, 0x9100),
            (0x9008, 0x1_0000_5678),
        ];
        let read = |address: u64| {
            stack
                .iter()
                .find(|(at, _)| *at == address)
                .map(|(_, value)| *value)
        };
        let encoding = X86_64_RBP_FRAME | (2 << 16) | 1 | (2 << 3);
        step_x86_64(encoding, 0, &mut registers, &read).unwrap();
        assert_eq!(registers.general[x86_64::RBX], 0xb0);
        assert_eq!(registers.general[x86_64::R12], 0xc0);
        assert_eq!(registers.general[x86_64::RBP], 0x9100);
        assert_eq!(registers.general[x86_64::RSP], 0x9010);
        assert_eq!(registers.rip, 0x1_0000_5678);
    }

    #[test]
    fn the_stack_permutation_names_the_registers_in_push_order() {
        assert_eq!(stack_registers(3, 0).unwrap()[..3], [1, 2, 3]);
        assert_eq!(stack_registers(3, 101).unwrap()[..3], [6, 1, 3]);
        assert_eq!(stack_registers(1, 4).unwrap()[..1], [5]);
        assert_eq!(stack_registers(7, 0), None);
    }

    #[test]
    fn an_x86_64_frameless_function_pops_its_registers_and_returns() {
        let mut registers = X86_64Registers::default();
        registers.general[x86_64::RSP] = 0xa000;
        let stack = [(0xa000 + 8, 0xb1u64), (0xa000 + 16, 0x1_0000_9999)];
        let read = |address: u64| {
            stack
                .iter()
                .find(|(at, _)| *at == address)
                .map(|(_, value)| *value)
        };
        let encoding = X86_64_STACK_IMMEDIATE | (3 << 16) | (1 << 10);
        step_x86_64(encoding, 0, &mut registers, &read).unwrap();
        assert_eq!(registers.general[x86_64::RBX], 0xb1);
        assert_eq!(registers.rip, 0x1_0000_9999);
        assert_eq!(registers.general[x86_64::RSP], 0xa018);
    }
}
