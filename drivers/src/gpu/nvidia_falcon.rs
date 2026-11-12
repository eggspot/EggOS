//! NVIDIA's Falcon microcontrollers (D497): GSP's and SEC2's, each a register block in BAR 0,
//! the engines that run the firmware which boots the GPU System Processor. Running code on one
//! is: reset the engine, wait for its memories to be scrubbed, pick the Falcon core over the
//! RISC-V one, copy the code into IMEM (each 256-byte block tagged with its virtual block, the
//! secure bit for heavy-secure code) and the data into DMEM through their auto-incrementing
//! ports, set the boot vector and the two mailboxes, start the core and wait for it to halt.
//! Register places are from NVIDIA's published `dev_falcon_v4.h`, `dev_riscv_pri.h`,
//! `dev_gsp.h` and `dev_sec_pri.h` (MIT, `NVIDIA-NOTICE.md`).

use crate::io::RegisterIo;

/// GSP's Falcon block in BAR 0.
pub const GSP_BASE: usize = 0x11_0000;
/// SEC2's Falcon block in BAR 0.
pub const SEC2_BASE: usize = 0x84_0000;
/// An IMEM block's bytes, the unit of its tags.
pub const BLOCK_BYTES: usize = 256;

const RISCV: usize = 0x1000;
const IRQ_STATUS_CLEAR: usize = 0x004;
const MAILBOX_0: usize = 0x040;
const MAILBOX_1: usize = 0x044;
const HWCFG_2: usize = 0x0f4;
const CPU_CONTROL: usize = 0x100;
const BOOT_VECTOR: usize = 0x104;
const HWCFG: usize = 0x108;
const DMA_CONTROL: usize = 0x10c;
const CPU_CONTROL_ALIAS: usize = 0x130;
const IMEM_CONTROL: usize = 0x180;
const IMEM_DATA: usize = 0x184;
const IMEM_TAG: usize = 0x188;
const DMEM_CONTROL: usize = 0x1c0;
const DMEM_DATA: usize = 0x1c4;
const ENGINE: usize = 0x3c0;
const BOOT_CORE_CONTROL: usize = RISCV + 0x668;

const ENGINE_RESET: u32 = 1;
const START_CPU: u32 = 1 << 1;
const HALTED: u32 = 1 << 4;
const ALIAS_ENABLED: u32 = 1 << 6;
const SCRUBBING_PENDING: u32 = 1 << 12;
const DMA_REQUIRE_CONTEXT: u32 = 1;
const AUTO_INCREMENT_WRITE: u32 = 1 << 24;
const SECURE: u32 = 1 << 28;
const IMEM_SIZE_MASK: u32 = 0x1ff;
const DMEM_SIZE_SHIFT: u32 = 9;
const CORE_FALCON: u32 = 0;
const IRQ_ALL: u32 = 0xffff_ffff;

/// Why code did not run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FalconError {
    /// The engine's memories were still being scrubbed when the time ran out.
    ScrubbingTimedOut,
    /// The code or data does not fit the engine's memory.
    TooLarge,
    /// The core did not halt in time.
    DidNotHalt,
}

impl FalconError {
    pub fn description(self) -> &'static str {
        match self {
            Self::ScrubbingTimedOut => "the falcon's memories were not scrubbed in time",
            Self::TooLarge => "the firmware does not fit the falcon's memory",
            Self::DidNotHalt => "the falcon did not finish in time",
        }
    }
}

/// Code to run: its IMEM bytes, where they go (IMEM's physical and virtual offsets), whether
/// they are heavy-secure, its DMEM bytes and where they go, its entry and its mailboxes.
#[derive(Debug, Clone, Copy)]
pub struct Program<'a> {
    pub code: &'a [u8],
    pub code_physical: u32,
    pub code_virtual: u32,
    pub secure: bool,
    pub data: &'a [u8],
    pub data_physical: u32,
    pub entry: u32,
    pub mailboxes: (u32, u32),
}

/// One Falcon engine at `base` in BAR 0, whose registers `regs` reaches.
pub struct Falcon<R: RegisterIo> {
    regs: R,
    base: usize,
}

impl<R: RegisterIo> Falcon<R> {
    pub fn new(regs: R, base: usize) -> Self {
        Self { regs, base }
    }

    fn read(&self, offset: usize) -> u32 {
        self.regs.read32(self.base + offset)
    }

    fn write(&self, offset: usize, value: u32) {
        self.regs.write32(self.base + offset, value);
    }

    /// IMEM's and DMEM's bytes, from `HWCFG` (256-byte blocks each).
    pub fn memory_bytes(&self) -> (usize, usize) {
        let config = self.read(HWCFG);
        let imem = (config & IMEM_SIZE_MASK) as usize * BLOCK_BYTES;
        let dmem = (config >> DMEM_SIZE_SHIFT & IMEM_SIZE_MASK) as usize * BLOCK_BYTES;
        (imem, dmem)
    }

    /// Resets the engine and waits, while `keep_waiting` allows, for its memories to be
    /// scrubbed; then picks the Falcon core and lets DMA run without a context.
    pub fn reset(&self, keep_waiting: &mut dyn FnMut() -> bool) -> Result<(), FalconError> {
        self.write(ENGINE, ENGINE_RESET);
        self.write(ENGINE, 0);
        while self.read(HWCFG_2) & SCRUBBING_PENDING != 0 {
            if !keep_waiting() {
                return Err(FalconError::ScrubbingTimedOut);
            }
            core::hint::spin_loop();
        }
        self.write(BOOT_CORE_CONTROL, CORE_FALCON);
        let control = self.read(DMA_CONTROL);
        self.write(DMA_CONTROL, control & !DMA_REQUIRE_CONTEXT);
        self.write(IRQ_STATUS_CLEAR, IRQ_ALL);
        Ok(())
    }

    /// Copies `program` in and starts it; then waits, while `keep_waiting` allows, for it to
    /// halt, and returns its mailboxes.
    pub fn run(
        &self,
        program: &Program<'_>,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(u32, u32), FalconError> {
        let (imem, dmem) = self.memory_bytes();
        let code_end = program.code_physical as usize + program.code.len();
        let data_end = program.data_physical as usize + program.data.len();
        if code_end > imem || data_end > dmem {
            return Err(FalconError::TooLarge);
        }
        self.load_code(program);
        self.load_data(program.data, program.data_physical);
        self.write(MAILBOX_0, program.mailboxes.0);
        self.write(MAILBOX_1, program.mailboxes.1);
        self.write(BOOT_VECTOR, program.entry);
        match self.read(CPU_CONTROL) & ALIAS_ENABLED != 0 {
            true => self.write(CPU_CONTROL_ALIAS, START_CPU),
            false => self.write(CPU_CONTROL, START_CPU),
        }
        while self.read(CPU_CONTROL) & HALTED == 0 {
            if !keep_waiting() {
                return Err(FalconError::DidNotHalt);
            }
            core::hint::spin_loop();
        }
        Ok((self.read(MAILBOX_0), self.read(MAILBOX_1)))
    }

    fn load_code(&self, program: &Program<'_>) {
        let secure = if program.secure { SECURE } else { 0 };
        self.write(IMEM_CONTROL, program.code_physical | AUTO_INCREMENT_WRITE | secure);
        let mut tag = program.code_virtual / BLOCK_BYTES as u32;
        for (index, word) in program.code.chunks(4).enumerate() {
            if index * 4 % BLOCK_BYTES == 0 {
                self.write(IMEM_TAG, tag);
                tag += 1;
            }
            self.write(IMEM_DATA, little_endian_word(word));
        }
        let padding = program.code.len().next_multiple_of(BLOCK_BYTES) - program.code.len();
        for _ in 0..padding.div_ceil(4) {
            self.write(IMEM_DATA, 0);
        }
    }

    fn load_data(&self, data: &[u8], physical: u32) {
        self.write(DMEM_CONTROL, physical | AUTO_INCREMENT_WRITE);
        for word in data.chunks(4) {
            self.write(DMEM_DATA, little_endian_word(word));
        }
    }
}

fn little_endian_word(bytes: &[u8]) -> u32 {
    let mut word = [0; 4];
    word[..bytes.len()].copy_from_slice(bytes);
    u32::from_le_bytes(word)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;
    use core::cell::RefCell;

    #[derive(Default)]
    struct Registers {
        writes: RefCell<Vec<(usize, u32)>>,
        hwcfg: u32,
        halted_after: RefCell<u32>,
    }

    impl RegisterIo for &Registers {
        fn read8(&self, _: usize) -> u8 {
            0
        }
        fn write8(&self, _: usize, _: u8) {}
        fn read16(&self, _: usize) -> u16 {
            0
        }
        fn write16(&self, _: usize, _: u16) {}
        fn read32(&self, offset: usize) -> u32 {
            match offset - GSP_BASE {
                HWCFG => self.hwcfg,
                CPU_CONTROL => {
                    let mut left = self.halted_after.borrow_mut();
                    *left = left.saturating_sub(1);
                    if *left == 0 { HALTED } else { 0 }
                }
                MAILBOX_0 => 0x600d,
                _ => 0,
            }
        }
        fn write32(&self, offset: usize, value: u32) {
            self.writes.borrow_mut().push((offset - GSP_BASE, value));
        }
    }

    #[test]
    fn code_is_tagged_by_block_padded_and_started_at_its_entry() {
        let regs = Registers {
            hwcfg: 64 | 32 << DMEM_SIZE_SHIFT,
            halted_after: RefCell::new(3),
            ..Registers::default()
        };
        let falcon = Falcon::new(&regs, GSP_BASE);
        assert_eq!(falcon.memory_bytes(), (64 * 256, 32 * 256));
        let code = [0xaau8; 260];
        let program = Program {
            code: &code,
            code_physical: 0,
            code_virtual: 0x200,
            secure: true,
            data: &[1, 2, 3, 4, 5],
            data_physical: 0x100,
            entry: 0x200,
            mailboxes: (7, 8),
        };
        assert_eq!(falcon.run(&program, &mut || true), Ok((0x600d, 0)));
        let writes = regs.writes.borrow();
        assert_eq!(writes[0], (IMEM_CONTROL, AUTO_INCREMENT_WRITE | SECURE));
        let tags: Vec<u32> = writes.iter().filter(|w| w.0 == IMEM_TAG).map(|w| w.1).collect();
        assert_eq!(tags, [2, 3]);
        let code_words = writes.iter().filter(|w| w.0 == IMEM_DATA).count();
        assert_eq!(code_words, 512 / 4);
        let data: Vec<u32> = writes.iter().filter(|w| w.0 == DMEM_DATA).map(|w| w.1).collect();
        assert_eq!(data, [0x0403_0201, 5]);
        assert!(writes.contains(&(BOOT_VECTOR, 0x200)));
        assert!(writes.contains(&(MAILBOX_0, 7)) && writes.contains(&(MAILBOX_1, 8)));
        assert_eq!(writes.last(), Some(&(CPU_CONTROL, START_CPU)));
    }

    #[test]
    fn code_past_imem_is_refused_before_anything_is_written() {
        let regs = Registers {
            hwcfg: 1 | 1 << DMEM_SIZE_SHIFT,
            ..Registers::default()
        };
        let falcon = Falcon::new(&regs, GSP_BASE);
        let code = [0u8; 300];
        let program = Program {
            code: &code,
            code_physical: 0,
            code_virtual: 0,
            secure: false,
            data: &[],
            data_physical: 0,
            entry: 0,
            mailboxes: (0, 0),
        };
        assert_eq!(falcon.run(&program, &mut || true), Err(FalconError::TooLarge));
        assert!(regs.writes.borrow().is_empty());
    }
}
