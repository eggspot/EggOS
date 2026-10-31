//! x64 unwinding (D177): `RUNTIME_FUNCTION` entries in `.pdata` and the `UNWIND_INFO` records
//! they point at, as Microsoft's "x64 exception handling" documents them. A step undoes the
//! prolog's unwind codes (only those whose instructions ran, when the program stopped inside
//! the prolog), follows chained records, and pops the return address; a program stopped inside
//! an epilog is unwound by carrying out the rest of the epilog, which is recognised by its
//! instructions (`add rsp`/`lea rsp`, `pop`s, then `ret` or a jump out of the function).

use super::{HandlerKinds, Image, Memory, UnwindError, Unwound};

/// The size of a `RUNTIME_FUNCTION`.
pub const RUNTIME_FUNCTION_SIZE: u64 = 12;
/// The general registers in the order unwind codes number them, which is also `CONTEXT`'s.
pub mod register {
    pub const RAX: usize = 0;
    pub const RSP: usize = 4;
    #[cfg(test)]
    pub const RBP: usize = 5;
}

const FLAG_CHAIN_INFO: u32 = 4;
const PUSH_NONVOL: u8 = 0;
const ALLOC_LARGE: u8 = 1;
const ALLOC_SMALL: u8 = 2;
const SET_FPREG: u8 = 3;
const SAVE_NONVOL: u8 = 4;
const SAVE_NONVOL_FAR: u8 = 5;
const EPILOG: u8 = 6;
const SPARE_CODE: u8 = 7;
const SAVE_XMM128: u8 = 8;
const SAVE_XMM128_FAR: u8 = 9;
const PUSH_MACHFRAME: u8 = 10;
const MAX_CHAIN: usize = 32;
const MAX_EPILOG_INSTRUCTIONS: usize = 24;

/// A thread's registers as an unwind step reads and changes them: the sixteen general
/// registers in unwind-code order, `rip` and the sixteen `xmm` registers (low, high halves).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Registers {
    pub general: [u64; 16],
    pub rip: u64,
    pub xmm: [[u64; 2]; 16],
}

impl Registers {
    pub fn rsp(&self) -> u64 {
        self.general[register::RSP]
    }

    fn set_rsp(&mut self, value: u64) {
        self.general[register::RSP] = value;
    }
}

/// One `RUNTIME_FUNCTION`: where it lies in `.pdata` and its three image-relative addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Function {
    pub entry: u64,
    pub begin: u32,
    pub end: u32,
    pub unwind_info: u32,
}

fn function_at(image: &Image<'_>, rva: u64) -> Option<Function> {
    Some(Function {
        entry: rva,
        begin: image.u32_at(rva)?,
        end: image.u32_at(rva + 4)?,
        unwind_info: image.u32_at(rva + 8)?,
    })
}

/// The function whose code holds `pc_rva`, from the table of `RUNTIME_FUNCTION`s at
/// `table_rva` (`table_size` bytes, sorted by start, as the linker writes it).
pub fn lookup(image: &Image<'_>, table_rva: u32, table_size: u32, pc_rva: u64) -> Option<Function> {
    let count = u64::from(table_size) / RUNTIME_FUNCTION_SIZE;
    let (mut low, mut high) = (0u64, count);
    while low < high {
        let middle = low + (high - low) / 2;
        let function = function_at(image, u64::from(table_rva) + middle * RUNTIME_FUNCTION_SIZE)?;
        if pc_rva < u64::from(function.begin) {
            high = middle;
        } else if pc_rva >= u64::from(function.end) {
            low = middle + 1;
        } else {
            return Some(function);
        }
    }
    None
}

struct Info {
    rva: u64,
    flags: u32,
    prolog_size: u64,
    code_count: u64,
    frame_register: usize,
    frame_offset: u64,
}

impl Info {
    fn read(image: &Image<'_>, rva: u32) -> Result<Self, UnwindError> {
        let rva = u64::from(rva);
        let first = image.u8_at(rva).ok_or(UnwindError::Truncated)?;
        let frame = image.u8_at(rva + 3).ok_or(UnwindError::Truncated)?;
        Ok(Self {
            rva,
            flags: u32::from(first >> 3),
            prolog_size: u64::from(image.u8_at(rva + 1).ok_or(UnwindError::Truncated)?),
            code_count: u64::from(image.u8_at(rva + 2).ok_or(UnwindError::Truncated)?),
            frame_register: usize::from(frame & 0xf),
            frame_offset: u64::from(frame >> 4) * 16,
        })
    }

    fn slot(&self, image: &Image<'_>, index: u64) -> Result<u16, UnwindError> {
        image
            .u16_at(self.rva + 4 + index * 2)
            .ok_or(UnwindError::Truncated)
    }

    fn after_codes(&self) -> u64 {
        self.rva + 4 + self.code_count.next_multiple_of(2) * 2
    }

    fn set_fpreg_offset(&self, image: &Image<'_>) -> Result<Option<u64>, UnwindError> {
        let mut index = 0;
        while index < self.code_count {
            let slot = self.slot(image, index)?;
            let operation = (slot >> 8) as u8 & 0xf;
            if operation == SET_FPREG {
                return Ok(Some(u64::from(slot & 0xff)));
            }
            index += slots(operation, (slot >> 12) as u8);
        }
        Ok(None)
    }
}

fn slots(operation: u8, operation_info: u8) -> u64 {
    match operation {
        ALLOC_LARGE if operation_info != 0 => 3,
        ALLOC_LARGE | SAVE_NONVOL | EPILOG | SAVE_XMM128 => 2,
        SAVE_NONVOL_FAR | SPARE_CODE | SAVE_XMM128_FAR => 3,
        _ => 1,
    }
}

fn read(memory: &impl Memory, address: u64) -> Result<u64, UnwindError> {
    memory.u64_at(address).ok_or(UnwindError::UnreadableStack)
}

fn establisher(
    image: &Image<'_>,
    info: &Info,
    offset: u64,
    registers: &Registers,
) -> Result<u64, UnwindError> {
    if info.frame_register == 0 {
        return Ok(registers.rsp());
    }
    let frame_set = offset >= info.prolog_size
        || info
            .set_fpreg_offset(image)?
            .is_some_and(|set_at| offset >= set_at);
    Ok(match frame_set {
        true => registers.general[info.frame_register].wrapping_sub(info.frame_offset),
        false => registers.rsp(),
    })
}

/// Undoes one frame of the function `function`, whose code holds `registers.rip`: on success
/// `registers` hold the caller's registers (`rip` the return address) and the result names the
/// frame's establisher and, if `kinds` asks for one the function has and the program was past
/// the prolog and not in an epilog, its language handler and the handler's data.
pub fn virtual_unwind(
    image: &Image<'_>,
    function: Function,
    kinds: HandlerKinds,
    registers: &mut Registers,
    memory: &impl Memory,
) -> Result<Unwound, UnwindError> {
    let offset = image
        .rva_of(registers.rip)
        .and_then(|rva| rva.checked_sub(u64::from(function.begin)))
        .unwrap_or(0);
    let mut info = Info::read(image, function.unwind_info)?;
    let frame = establisher(image, &info, offset, registers)?;
    if offset >= info.prolog_size && in_epilog(image, function, registers, memory)? {
        return Ok(Unwound {
            handler: None,
            handler_data: 0,
            establisher: frame,
        });
    }
    let in_prolog = offset < info.prolog_size;
    let mut prolog_offset = offset;
    if info.frame_register != 0 && !in_prolog {
        registers.set_rsp(frame);
    }
    let mut machine_frame = false;
    for _ in 0..MAX_CHAIN {
        machine_frame |= undo_codes(image, &info, prolog_offset, registers, memory)?;
        if info.flags & FLAG_CHAIN_INFO == 0 {
            break;
        }
        let parent = function_at(image, info.after_codes()).ok_or(UnwindError::Truncated)?;
        info = Info::read(image, parent.unwind_info)?;
        prolog_offset = u64::MAX;
    }
    if !machine_frame {
        let rsp = registers.rsp();
        registers.rip = read(memory, rsp)?;
        registers.set_rsp(rsp.wrapping_add(8));
    }
    let handler = match !in_prolog && kinds.matches(info.flags) {
        true => {
            let at = info.after_codes();
            let rva = image.u32_at(at).ok_or(UnwindError::Truncated)?;
            Some((image.base + u64::from(rva), image.base + at + 4))
        }
        false => None,
    };
    Ok(Unwound {
        handler: handler.map(|(address, _)| address),
        handler_data: handler.map_or(0, |(_, data)| data),
        establisher: frame,
    })
}

fn undo_codes(
    image: &Image<'_>,
    info: &Info,
    prolog_offset: u64,
    registers: &mut Registers,
    memory: &impl Memory,
) -> Result<bool, UnwindError> {
    let mut machine_frame = false;
    let mut index = 0;
    while index < info.code_count {
        let slot = info.slot(image, index)?;
        let code_offset = u64::from(slot & 0xff);
        let operation = (slot >> 8) as u8 & 0xf;
        let operation_info = (slot >> 12) as u8;
        let size = slots(operation, operation_info);
        let ran = prolog_offset >= info.prolog_size || code_offset <= prolog_offset;
        if ran {
            let rsp = registers.rsp();
            match operation {
                PUSH_NONVOL => {
                    registers.general[usize::from(operation_info)] = read(memory, rsp)?;
                    registers.set_rsp(rsp.wrapping_add(8));
                }
                ALLOC_LARGE => {
                    let size = match operation_info {
                        0 => u64::from(info.slot(image, index + 1)?) * 8,
                        _ => {
                            u64::from(info.slot(image, index + 1)?)
                                | u64::from(info.slot(image, index + 2)?) << 16
                        }
                    };
                    registers.set_rsp(rsp.wrapping_add(size));
                }
                ALLOC_SMALL => {
                    registers.set_rsp(rsp.wrapping_add(u64::from(operation_info) * 8 + 8));
                }
                SET_FPREG => {
                    let frame = registers.general[info.frame_register];
                    registers.set_rsp(frame.wrapping_sub(info.frame_offset));
                }
                SAVE_NONVOL => {
                    let at = rsp.wrapping_add(u64::from(info.slot(image, index + 1)?) * 8);
                    registers.general[usize::from(operation_info)] = read(memory, at)?;
                }
                SAVE_NONVOL_FAR => {
                    let offset = u64::from(info.slot(image, index + 1)?)
                        | u64::from(info.slot(image, index + 2)?) << 16;
                    registers.general[usize::from(operation_info)] =
                        read(memory, rsp.wrapping_add(offset))?;
                }
                SAVE_XMM128 | SAVE_XMM128_FAR => {
                    let offset = match operation {
                        SAVE_XMM128 => u64::from(info.slot(image, index + 1)?) * 16,
                        _ => {
                            u64::from(info.slot(image, index + 1)?)
                                | u64::from(info.slot(image, index + 2)?) << 16
                        }
                    };
                    let at = rsp.wrapping_add(offset);
                    registers.xmm[usize::from(operation_info)] =
                        [read(memory, at)?, read(memory, at.wrapping_add(8))?];
                }
                PUSH_MACHFRAME => {
                    let frame = rsp.wrapping_add(if operation_info != 0 { 8 } else { 0 });
                    registers.rip = read(memory, frame)?;
                    registers.set_rsp(read(memory, frame.wrapping_add(24))?);
                    machine_frame = true;
                }
                EPILOG | SPARE_CODE => {}
                _ => return Err(UnwindError::UnknownCode),
            }
        }
        index += size;
    }
    Ok(machine_frame)
}

#[derive(Clone, Copy)]
enum EpilogStep {
    AddRsp(u64),
    LoadRsp { register: usize, displacement: u64 },
    Pop(usize),
}

fn in_epilog(
    image: &Image<'_>,
    function: Function,
    registers: &mut Registers,
    memory: &impl Memory,
) -> Result<bool, UnwindError> {
    let Some(mut at) = image.rva_of(registers.rip) else {
        return Ok(false);
    };
    let byte = |rva: u64| image.u8_at(rva);
    let mut steps = [None; MAX_EPILOG_INSTRUCTIONS];
    let mut count = 0;
    if let Some(rex @ 0x48..=0x4f) = byte(at) {
        match (byte(at + 1), byte(at + 2)) {
            (Some(0x83), Some(0xc4)) if rex == 0x48 => {
                let value = byte(at + 3).ok_or(UnwindError::Truncated)?;
                steps[count] = Some(EpilogStep::AddRsp(sign_extend_8(u64::from(value))));
                count += 1;
                at += 4;
            }
            (Some(0x81), Some(0xc4)) if rex == 0x48 => {
                let value = image.u32_at(at + 3).ok_or(UnwindError::Truncated)?;
                steps[count] = Some(EpilogStep::AddRsp(value as i32 as i64 as u64));
                count += 1;
                at += 7;
            }
            (Some(0x8d), Some(modrm)) if modrm & 0x38 == 0x20 && modrm & 0x07 != 4 => {
                let register = usize::from(modrm & 7) | usize::from(rex & 1) << 3;
                let (displacement, length) = match modrm >> 6 {
                    1 => {
                        let value = byte(at + 3).ok_or(UnwindError::Truncated)?;
                        (sign_extend_8(u64::from(value)), 4)
                    }
                    2 => {
                        let value = image.u32_at(at + 3).ok_or(UnwindError::Truncated)?;
                        (value as i32 as i64 as u64, 7)
                    }
                    _ => return Ok(false),
                };
                steps[count] = Some(EpilogStep::LoadRsp {
                    register,
                    displacement,
                });
                count += 1;
                at += length;
            }
            _ => {}
        }
    }
    loop {
        let Some(first) = byte(at) else {
            return Ok(false);
        };
        let (register, length) = match first {
            0x58..=0x5f if first != 0x5c => (usize::from(first - 0x58), 1),
            0x41 => match byte(at + 1) {
                Some(second @ 0x58..=0x5f) => (usize::from(second - 0x58) + 8, 2),
                _ => break,
            },
            _ => break,
        };
        if count == MAX_EPILOG_INSTRUCTIONS {
            return Ok(false);
        }
        steps[count] = Some(EpilogStep::Pop(register));
        count += 1;
        at += length;
    }
    if !ends_epilog(image, function, at) {
        return Ok(false);
    }
    for step in steps.iter().take(count).flatten() {
        let rsp = registers.rsp();
        match *step {
            EpilogStep::AddRsp(value) => registers.set_rsp(rsp.wrapping_add(value)),
            EpilogStep::LoadRsp {
                register,
                displacement,
            } => {
                let base = registers.general[register];
                registers.set_rsp(base.wrapping_add(displacement));
            }
            EpilogStep::Pop(register) => {
                registers.general[register] = read(memory, rsp)?;
                registers.set_rsp(rsp.wrapping_add(8));
            }
        }
    }
    let rsp = registers.rsp();
    registers.rip = read(memory, rsp)?;
    registers.set_rsp(rsp.wrapping_add(8));
    Ok(true)
}

fn sign_extend_8(value: u64) -> u64 {
    value as u8 as i8 as i64 as u64
}

fn ends_epilog(image: &Image<'_>, function: Function, at: u64) -> bool {
    let outside =
        |target: u64| target < u64::from(function.begin) || target >= u64::from(function.end);
    match (image.u8_at(at), image.u8_at(at + 1), image.u8_at(at + 2)) {
        (Some(0xc3), _, _) | (Some(0xf3), Some(0xc3), _) => true,
        (Some(0xe9), _, _) => image
            .u32_at(at + 1)
            .is_some_and(|relative| outside((at + 5).wrapping_add(relative as i32 as i64 as u64))),
        (Some(0xeb), Some(relative), _) => {
            outside((at + 2).wrapping_add(sign_extend_8(u64::from(relative))))
        }
        (Some(0xff), Some(modrm), _) => modrm & 0x38 == 0x20,
        (Some(0x48), Some(0xff), Some(modrm)) => modrm & 0x38 == 0x20,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unwind::test_memory::Stack;

    const BASE: u64 = 0x1_4000_0000;
    const TEXT: u64 = 0x1000;
    const UNWIND: u64 = 0x2000;
    const PDATA: u64 = 0x3000;
    const STACK: u64 = 0x7000_0000;

    struct Builder {
        bytes: Vec<u8>,
    }

    impl Builder {
        fn new() -> Self {
            Self {
                bytes: vec![0xcc; 0x4000],
            }
        }

        fn put(&mut self, rva: u64, data: &[u8]) -> &mut Self {
            self.bytes[rva as usize..rva as usize + data.len()].copy_from_slice(data);
            self
        }

        fn function(&mut self, index: u64, begin: u32, end: u32, unwind: u32) -> &mut Self {
            let mut entry = Vec::new();
            for value in [begin, end, unwind] {
                entry.extend(value.to_le_bytes());
            }
            self.put(PDATA + index * 12, &entry)
        }

        fn image(&self) -> Image<'_> {
            Image {
                base: BASE,
                bytes: &self.bytes,
            }
        }
    }

    fn info(flags: u8, prolog: u8, frame: u8, codes: &[u16], tail: &[u8]) -> Vec<u8> {
        let mut bytes = vec![1 | flags << 3, prolog, codes.len() as u8, frame];
        for code in codes {
            bytes.extend(code.to_le_bytes());
        }
        if codes.len() % 2 == 1 {
            bytes.extend([0, 0]);
        }
        bytes.extend(tail);
        bytes
    }

    fn code(offset: u8, operation: u8, operation_info: u8) -> u16 {
        u16::from(offset) | u16::from(operation | operation_info << 4) << 8
    }

    fn stack(words: &[u64]) -> Stack {
        Stack {
            base: STACK,
            words: words.to_vec(),
        }
    }

    const BOTH: HandlerKinds = HandlerKinds {
        exception: true,
        termination: true,
    };

    #[test]
    fn a_function_is_found_by_binary_search_of_the_table() {
        let mut builder = Builder::new();
        builder
            .function(0, 0x1000, 0x1010, 0x2000)
            .function(1, 0x1010, 0x1040, 0x2010)
            .function(2, 0x1100, 0x1180, 0x2020);
        let image = builder.image();
        let found = lookup(&image, PDATA as u32, 36, 0x1020).unwrap();
        assert_eq!(
            (found.begin, found.end, found.unwind_info),
            (0x1010, 0x1040, 0x2010)
        );
        assert_eq!(found.entry, PDATA + 12);
        assert_eq!(
            lookup(&image, PDATA as u32, 36, 0x1100).unwrap().begin,
            0x1100
        );
        assert_eq!(lookup(&image, PDATA as u32, 36, 0x1050), None);
        assert_eq!(lookup(&image, PDATA as u32, 36, 0x1180), None);
        assert_eq!(lookup(&image, PDATA as u32, 0, 0x1000), None);
    }

    fn pushes_and_allocation() -> Builder {
        let mut builder = Builder::new();
        let codes = [
            code(6, ALLOC_SMALL, 3),
            code(2, PUSH_NONVOL, 3),
            code(1, PUSH_NONVOL, 5),
        ];
        builder
            .put(UNWIND, &info(1, 6, 0, &codes, &0x1800u32.to_le_bytes()))
            .put(UNWIND + 16, &0xdead_beefu32.to_le_bytes())
            .function(0, 0x1000, 0x1080, UNWIND as u32);
        builder
    }

    #[test]
    fn the_body_undoes_every_code_and_returns_the_handler() {
        let builder = pushes_and_allocation();
        let image = builder.image();
        let function = lookup(&image, PDATA as u32, 12, 0x1040).unwrap();
        let memory = stack(&[0, 0, 0, 0, 0x55, 0x33, BASE + 0x1234, 0]);
        let mut registers = Registers {
            rip: BASE + 0x1040,
            ..Registers::default()
        };
        registers.set_rsp(STACK);
        let unwound = virtual_unwind(&image, function, BOTH, &mut registers, &memory).unwrap();
        assert_eq!(registers.rip, BASE + 0x1234);
        assert_eq!(registers.rsp(), STACK + 56);
        assert_eq!(registers.general[3], 0x55);
        assert_eq!(registers.general[5], 0x33);
        assert_eq!(unwound.establisher, STACK);
        assert_eq!(unwound.handler, Some(BASE + 0x1800));
        assert_eq!(unwound.handler_data, BASE + UNWIND + 16);
        let mut again = Registers {
            rip: BASE + 0x1040,
            ..Registers::default()
        };
        again.set_rsp(STACK);
        let none = HandlerKinds {
            exception: false,
            termination: false,
        };
        let plain = virtual_unwind(&image, function, none, &mut again, &memory).unwrap();
        assert_eq!(plain.handler, None);
    }

    #[test]
    fn inside_the_prolog_only_the_codes_that_ran_are_undone_and_no_handler_is_called() {
        let builder = pushes_and_allocation();
        let image = builder.image();
        let function = lookup(&image, PDATA as u32, 12, 0x1002).unwrap();
        let memory = stack(&[0x55, 0x33, BASE + 0x1234]);
        let mut registers = Registers {
            rip: BASE + 0x1002,
            ..Registers::default()
        };
        registers.set_rsp(STACK);
        let unwound = virtual_unwind(&image, function, BOTH, &mut registers, &memory).unwrap();
        assert_eq!(registers.general[3], 0x55);
        assert_eq!(registers.general[5], 0x33);
        assert_eq!(registers.rip, BASE + 0x1234);
        assert_eq!(registers.rsp(), STACK + 24);
        assert_eq!(unwound.handler, None);
    }

    #[test]
    fn a_frame_pointer_gives_the_establisher_and_restores_rsp_after_alloca() {
        let mut builder = Builder::new();
        let codes = [
            code(12, SAVE_NONVOL, 7),
            0x0004,
            code(8, SET_FPREG, 0),
            code(4, ALLOC_SMALL, 7),
            code(1, PUSH_NONVOL, 5),
        ];
        builder
            .put(UNWIND, &info(0, 16, 5 | 2 << 4, &codes, &[]))
            .function(0, 0x1000, 0x1100, UNWIND as u32);
        let image = builder.image();
        let function = lookup(&image, PDATA as u32, 12, 0x1050).unwrap();
        let frame_base = STACK + 0x100;
        let mut words = vec![0u64; 0x40];
        words[(0x100 + 32) / 8] = 0x7777;
        words[(0x100 + 64) / 8] = 0xaaaa;
        words[(0x100 + 72) / 8] = BASE + 0x2222;
        let memory = stack(&words);
        let mut registers = Registers {
            rip: BASE + 0x1050,
            ..Registers::default()
        };
        registers.set_rsp(STACK);
        registers.general[register::RBP] = frame_base + 32;
        let unwound = virtual_unwind(&image, function, BOTH, &mut registers, &memory).unwrap();
        assert_eq!(unwound.establisher, frame_base);
        assert_eq!(registers.general[7], 0x7777);
        assert_eq!(registers.general[register::RBP], 0xaaaa);
        assert_eq!(registers.rip, BASE + 0x2222);
        assert_eq!(registers.rsp(), frame_base + 80);
    }

    #[test]
    fn large_allocations_and_far_saves_read_their_extra_slots() {
        let mut builder = Builder::new();
        let codes = [
            code(20, SAVE_NONVOL_FAR, 12),
            0x0010,
            0x0001,
            code(10, ALLOC_LARGE, 1),
            0x0000,
            0x0002,
            code(3, ALLOC_LARGE, 0),
            0x0002,
        ];
        builder.put(UNWIND, &info(0, 24, 0, &codes, &[])).function(
            0,
            0x1000,
            0x1100,
            UNWIND as u32,
        );
        let image = builder.image();
        let function = lookup(&image, PDATA as u32, 12, 0x1040).unwrap();
        let after = 0x2_0000 + 16;
        let mut words = vec![0u64; (after / 8 + 1) as usize];
        words[(0x1_0010 / 8) as usize] = 0x1212;
        words[(after / 8) as usize] = BASE + 0x3333;
        let memory = stack(&words);
        let mut registers = Registers {
            rip: BASE + 0x1040,
            ..Registers::default()
        };
        registers.set_rsp(STACK);
        virtual_unwind(&image, function, BOTH, &mut registers, &memory).unwrap();
        assert_eq!(registers.general[12], 0x1212);
        assert_eq!(registers.rip, BASE + 0x3333);
        assert_eq!(registers.rsp(), STACK + after + 8);
    }

    #[test]
    fn chained_records_undo_the_parent_s_codes_too() {
        let mut builder = Builder::new();
        let mut parent_entry = Vec::new();
        for value in [0x1000u32, 0x1040, UNWIND as u32 + 0x40] {
            parent_entry.extend(value.to_le_bytes());
        }
        builder
            .put(
                UNWIND,
                &info(4, 0, 0, &[code(0, ALLOC_SMALL, 1)], &parent_entry),
            )
            .put(
                UNWIND + 0x40,
                &info(0, 2, 0, &[code(1, PUSH_NONVOL, 6)], &[]),
            )
            .function(0, 0x1000, 0x1040, UNWIND as u32 + 0x40)
            .function(1, 0x1200, 0x1240, UNWIND as u32);
        let image = builder.image();
        let function = lookup(&image, PDATA as u32, 24, 0x1210).unwrap();
        let memory = stack(&[0, 0, 0x6666, BASE + 0x4444]);
        let mut registers = Registers {
            rip: BASE + 0x1210,
            ..Registers::default()
        };
        registers.set_rsp(STACK);
        virtual_unwind(&image, function, BOTH, &mut registers, &memory).unwrap();
        assert_eq!(registers.general[6], 0x6666);
        assert_eq!(registers.rip, BASE + 0x4444);
        assert_eq!(registers.rsp(), STACK + 32);
    }

    #[test]
    fn an_epilog_is_carried_out_instead_of_the_codes() {
        let mut builder = pushes_and_allocation();
        builder.put(0x1070, &[0x48, 0x83, 0xc4, 0x20, 0x5b, 0x41, 0x5c, 0xc3]);
        let image = builder.image();
        let function = lookup(&image, PDATA as u32, 12, 0x1070).unwrap();
        let memory = stack(&[0, 0, 0, 0, 0x55, 0xcc, BASE + 0x1234]);
        let mut registers = Registers {
            rip: BASE + 0x1070,
            ..Registers::default()
        };
        registers.set_rsp(STACK);
        let unwound = virtual_unwind(&image, function, BOTH, &mut registers, &memory).unwrap();
        assert_eq!(unwound.handler, None);
        assert_eq!(registers.general[3], 0x55);
        assert_eq!(registers.general[12], 0xcc);
        assert_eq!(registers.rip, BASE + 0x1234);
        assert_eq!(registers.rsp(), STACK + 56);
        let mut from_pop = Registers {
            rip: BASE + 0x1075,
            ..Registers::default()
        };
        from_pop.set_rsp(STACK + 40);
        virtual_unwind(&image, function, BOTH, &mut from_pop, &memory).unwrap();
        assert_eq!(from_pop.general[12], 0xcc);
        assert_eq!(from_pop.rip, BASE + 0x1234);
    }

    #[test]
    fn a_jump_inside_the_function_is_not_an_epilog() {
        let mut builder = pushes_and_allocation();
        builder.put(0x1070, &[0x5b, 0xeb, 0x00]);
        builder.put(0x1078, &[0x5b, 0xe9, 0x00, 0x10, 0x00, 0x00]);
        let image = builder.image();
        let function = lookup(&image, PDATA as u32, 12, 0x1070).unwrap();
        let memory = stack(&[0, 0, 0, 0, 0x55, 0x33, BASE + 0x1234, 0]);
        let mut inside = Registers {
            rip: BASE + 0x1070,
            ..Registers::default()
        };
        inside.set_rsp(STACK);
        let unwound = virtual_unwind(&image, function, BOTH, &mut inside, &memory).unwrap();
        assert!(unwound.handler.is_some());
        assert_eq!(inside.rsp(), STACK + 56);
        let mut tail_call = Registers {
            rip: BASE + 0x1078,
            ..Registers::default()
        };
        tail_call.set_rsp(STACK + 32);
        let unwound = virtual_unwind(&image, function, BOTH, &mut tail_call, &memory).unwrap();
        assert_eq!(unwound.handler, None);
        assert_eq!(tail_call.general[3], 0x55);
        assert_eq!(tail_call.rip, 0x33);
    }

    #[test]
    fn an_unknown_code_or_an_unreadable_stack_is_an_error() {
        let mut builder = Builder::new();
        builder
            .put(UNWIND, &info(0, 4, 0, &[code(1, 11, 0)], &[]))
            .function(0, 0x1000, 0x1100, UNWIND as u32);
        let image = builder.image();
        let function = lookup(&image, PDATA as u32, 12, 0x1040).unwrap();
        let mut registers = Registers {
            rip: BASE + 0x1040,
            ..Registers::default()
        };
        registers.set_rsp(STACK);
        let memory = stack(&[]);
        assert_eq!(
            virtual_unwind(&image, function, BOTH, &mut registers, &memory),
            Err(UnwindError::UnknownCode)
        );
        let builder = pushes_and_allocation();
        let image = builder.image();
        let function = lookup(&image, PDATA as u32, 12, 0x1040).unwrap();
        let mut registers = Registers {
            rip: BASE + 0x1040,
            ..Registers::default()
        };
        registers.set_rsp(STACK);
        assert_eq!(
            virtual_unwind(&image, function, BOTH, &mut registers, &memory),
            Err(UnwindError::UnreadableStack)
        );
    }
}
