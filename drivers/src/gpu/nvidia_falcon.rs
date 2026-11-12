//! NVIDIA's Falcon microcontrollers (D497): GSP's and SEC2's, each a register block in BAR 0,
//! the engines that run the firmware which boots the GPU System Processor. Running code on one
//! is: reset the engine, wait for its memories to be scrubbed, pick the Falcon core over the
//! RISC-V one, copy the code into IMEM (each 256-byte block tagged with its virtual block, the
//! secure bit for heavy-secure code) and the data into DMEM through their auto-incrementing
//! ports, set the boot vector and the two mailboxes, start the core and wait for it to halt.
//! Register places are from NVIDIA's published `dev_falcon_v4.h`, `dev_riscv_pri.h`,
//! `dev_gsp.h` and `dev_sec_pri.h` (MIT, `NVIDIA-NOTICE.md`).
//!
//! Heavy-secure code (D513), such as FWSEC, is loaded by the engine's own DMA from coherent
//! system memory, IMEM blocks marked secure, after the boot ROM is told where the code's
//! RSA-3K signature sits in DMEM, which engines it may run on and its ucode id
//! (`dev_falcon_second_pri.h`, `dev_fbif_v4.h`, `dev_fuse.h`).

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
const RISCV_CPU_CONTROL: usize = RISCV + 0x388;
const OS_VERSION: usize = 0x080;
const QUEUE_HEAD: usize = 0xc00;
const BOOT_CONFIG_VALID: u32 = 1;
const CORE_SELECT_RISCV: u32 = 1 << 4;
const BOOT_ROM_FETCH: u32 = 1 << 8;
const RISCV_ACTIVE: u32 = 1 << 7;
const DMA_BASE: usize = 0x110;
const DMA_MEMORY_OFFSET: usize = 0x114;
const DMA_COMMAND: usize = 0x118;
const DMA_SOURCE_OFFSET: usize = 0x11c;
const DMA_BASE_HIGH: usize = 0x128;
const DMA_FULL: u32 = 1;
const DMA_IDLE: u32 = 1 << 1;
const DMA_SECURE: u32 = 1 << 2;
const DMA_TO_IMEM: u32 = 1 << 4;
const DMA_256_BYTES: u32 = 6 << 8;
const BOOT_ROM_MODULE: usize = RISCV + 0x180;
const BOOT_ROM_UCODE_ID: usize = RISCV + 0x198;
const BOOT_ROM_ENGINES: usize = RISCV + 0x19c;
const BOOT_ROM_SIGNATURE: usize = RISCV + 0x210;
const RSA_3K: u32 = 1;
const TRANSFER_CONFIG: usize = 0;
const FBIF_CONTROL: usize = 0x24;
const TARGET_COHERENT_SYSTEM_MEMORY: u32 = 1;
const PHYSICAL: u32 = 1 << 2;
const ALLOW_PHYSICAL_WITHOUT_CONTEXT: u32 = 1 << 7;

/// GSP's interface to memory (FBIF) in BAR 0.
pub const GSP_FBIF: usize = 0x11_0600;
/// SEC2's interface to memory (FBIF) in BAR 0.
pub const SEC2_FBIF: usize = 0x84_0600;
/// The fuse holding GSP's first ucode's version; the others follow a word apart.
pub const GSP_UCODE_VERSION_FUSE: usize = 0x82_41c0;

/// The version the fuses hold for GSP ucode `ucode_id` (1 on): the highest bit set, plus
/// one, or 0 for none.
pub fn fuse_version(regs: &impl RegisterIo, ucode_id: u8) -> u32 {
    if ucode_id == 0 || ucode_id > 16 {
        return 0;
    }
    match regs.read32(GSP_UCODE_VERSION_FUSE + 4 * (usize::from(ucode_id) - 1)) {
        0 => 0,
        value => 32 - value.leading_zeros(),
    }
}

/// Heavy-secure code in coherent system memory at physical `image`: its IMEM bytes, then its
/// DMEM bytes, with where each goes, where in DMEM its signature is, which engines it may run
/// on and its ucode id.
#[derive(Debug, Clone, Copy)]
pub struct SecureProgram {
    pub image: u64,
    pub imem_physical: u32,
    pub imem_virtual: u32,
    pub imem_bytes: u32,
    pub dmem_physical: u32,
    pub dmem_bytes: u32,
    pub signature_place: u32,
    pub engine_mask: u16,
    pub ucode_id: u8,
}

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

    /// Points a freshly [`Self::reset`] engine's boot at its RISC-V core (D522), which the
    /// booter starts, and leaves libos's arguments page, at device address `arguments`, in
    /// its mailboxes.
    pub fn prepare_riscv(&self, arguments: u64) {
        let control = self.read(BOOT_CORE_CONTROL);
        self.write(
            BOOT_CORE_CONTROL,
            control | BOOT_CONFIG_VALID | CORE_SELECT_RISCV | BOOT_ROM_FETCH,
        );
        self.write(MAILBOX_0, arguments as u32);
        self.write(MAILBOX_1, (arguments >> 32) as u32);
    }

    /// Starts the core at its boot vector, through the control register's alias when the
    /// engine has it enabled.
    pub fn start(&self) {
        match self.read(CPU_CONTROL) & ALIAS_ENABLED != 0 {
            true => self.write(CPU_CONTROL_ALIAS, START_CPU),
            false => self.write(CPU_CONTROL, START_CPU),
        }
    }

    /// Waits, while `keep_waiting` allows, for the core to halt.
    pub fn wait_for_halt(&self, keep_waiting: &mut dyn FnMut() -> bool) -> Result<(), FalconError> {
        while self.read(CPU_CONTROL) & HALTED == 0 {
            if !keep_waiting() {
                return Err(FalconError::DidNotHalt);
            }
            core::hint::spin_loop();
        }
        Ok(())
    }

    /// Rings GSP's command queue doorbell (D523): a new command waits.
    pub fn announce_command(&self) {
        self.write(QUEUE_HEAD, 0);
    }

    /// Tells the RISC-V firmware the boot binary's version.
    pub fn set_os_version(&self, version: u32) {
        self.write(OS_VERSION, version);
    }

    /// Whether the RISC-V core runs.
    pub fn riscv_active(&self) -> bool {
        self.read(RISCV_CPU_CONTROL) & RISCV_ACTIVE != 0
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
        self.start();
        self.wait_for_halt(keep_waiting)?;
        Ok((self.read(MAILBOX_0), self.read(MAILBOX_1)))
    }

    fn dma_wait(&self, mask: u32, wanted: u32, keep_waiting: &mut dyn FnMut() -> bool) -> bool {
        while self.read(DMA_COMMAND) & mask != wanted {
            if !keep_waiting() {
                return false;
            }
            core::hint::spin_loop();
        }
        true
    }

    fn dma(
        &self,
        source: u64,
        (destination, memory_offset, bytes): (u32, u32, u32),
        command: u32,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), FalconError> {
        if !self.dma_wait(DMA_FULL, 0, keep_waiting) {
            return Err(FalconError::DidNotHalt);
        }
        self.write(DMA_BASE, (source >> 8) as u32);
        self.write(DMA_BASE_HIGH, (source >> 40) as u32 & 0x1ff);
        for block in (0..bytes).step_by(BLOCK_BYTES) {
            if !self.dma_wait(DMA_FULL, 0, keep_waiting) {
                return Err(FalconError::DidNotHalt);
            }
            self.write(DMA_MEMORY_OFFSET, destination + block);
            self.write(DMA_SOURCE_OFFSET, memory_offset + block);
            self.write(DMA_COMMAND, command);
        }
        match self.dma_wait(DMA_IDLE, DMA_IDLE, keep_waiting) {
            true => Ok(()),
            false => Err(FalconError::DidNotHalt),
        }
    }

    /// Loads heavy-secure `program` through the engine's DMA (its FBIF at `fbif`), points the
    /// boot ROM at its signature, starts it at its IMEM virtual address with `mailboxes`, and
    /// waits for it to halt; its mailboxes then. The engine must be [`Self::reset`] first.
    pub fn run_secure(
        &self,
        fbif: usize,
        program: &SecureProgram,
        mailboxes: (u32, u32),
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(u32, u32), FalconError> {
        let (imem, dmem) = self.memory_bytes();
        if (program.imem_physical + program.imem_bytes) as usize > imem
            || (program.dmem_physical + program.dmem_bytes) as usize > dmem
        {
            return Err(FalconError::TooLarge);
        }
        let config = self.regs.read32(fbif + TRANSFER_CONFIG) & !0x7;
        self.regs.write32(
            fbif + TRANSFER_CONFIG,
            config | TARGET_COHERENT_SYSTEM_MEMORY | PHYSICAL,
        );
        let control = self.regs.read32(fbif + FBIF_CONTROL);
        self.regs.write32(fbif + FBIF_CONTROL, control | ALLOW_PHYSICAL_WITHOUT_CONTEXT);
        let code_source = program.image.wrapping_sub(u64::from(program.imem_virtual));
        self.dma(
            code_source,
            (program.imem_physical, program.imem_virtual, program.imem_bytes),
            DMA_256_BYTES | DMA_TO_IMEM | DMA_SECURE,
            keep_waiting,
        )?;
        let data_source = program.image + u64::from(program.imem_bytes);
        self.dma(
            data_source,
            (program.dmem_physical, 0, program.dmem_bytes),
            DMA_256_BYTES,
            keep_waiting,
        )?;
        self.write(BOOT_ROM_SIGNATURE, program.signature_place);
        self.write(BOOT_ROM_ENGINES, u32::from(program.engine_mask));
        self.write(BOOT_ROM_UCODE_ID, u32::from(program.ucode_id));
        self.write(BOOT_ROM_MODULE, RSA_3K);
        self.write(BOOT_VECTOR, program.imem_virtual);
        self.write(MAILBOX_0, mailboxes.0);
        self.write(MAILBOX_1, mailboxes.1);
        self.start();
        self.wait_for_halt(keep_waiting)?;
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

    struct Fuse(u32);

    impl RegisterIo for Fuse {
        fn read8(&self, _: usize) -> u8 {
            0
        }
        fn write8(&self, _: usize, _: u8) {}
        fn read16(&self, _: usize) -> u16 {
            0
        }
        fn write16(&self, _: usize, _: u16) {}
        fn read32(&self, offset: usize) -> u32 {
            match offset == GSP_UCODE_VERSION_FUSE + 4 {
                true => self.0,
                false => 0,
            }
        }
        fn write32(&self, _: usize, _: u32) {}
    }

    #[test]
    fn the_fuse_version_is_the_highest_bit_set_plus_one() {
        assert_eq!(fuse_version(&Fuse(0b0110), 2), 3);
        assert_eq!(fuse_version(&Fuse(0), 2), 0);
        assert_eq!(fuse_version(&Fuse(0b0110), 1), 0);
        assert_eq!(fuse_version(&Fuse(1), 0), 0);
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
