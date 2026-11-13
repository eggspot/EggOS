//! The adapter's host interface (D273): its control and status registers, the NIC lock that
//! keeps its MAC awake while the host reaches its internal (`PRPH`) registers, powering it up,
//! the default receive ring and the command queue's descriptor ring, and handing it the firmware:
//! for the AX200 family through one context-info block naming every section, for the AX210
//! family through a context info that points at a scratch area, the image loader and the rings.
//! The firmware's alive notification ends the handover. Everything here is polled.

use super::devices::{Adapter, Family};
use super::firmware::{Firmware, Image, Section};
use crate::dma::{DmaAllocator, DmaMemory};
use crate::io::RegisterIo;
use alloc::vec::Vec;
use core::sync::atomic::{Ordering, fence};

const CSR_HW_IF_CONFIG: usize = 0x000;
const CSR_INT_COALESCING: usize = 0x004;
const CSR_INT: usize = 0x008;
const CSR_INT_MASK: usize = 0x00c;
const CSR_FH_INT_STATUS: usize = 0x010;
const CSR_RESET: usize = 0x020;
const CSR_GP_CNTRL: usize = 0x024;
const CSR_HW_REV: usize = 0x028;
const CSR_GIO: usize = 0x03c;
const CSR_CONTEXT_INFO_BASE: usize = 0x040;
const CSR_UCODE_DRV_GP1_CLR: usize = 0x05c;
const CSR_MBOX_SET: usize = 0x088;
const CSR_MAC_SHADOW_CONTROL: usize = 0x0a8;
const CSR_LTR_LONG_VALUE: usize = 0x0d4;
const CSR_GIO_CHICKEN_BITS: usize = 0x100;
const CSR_CONTEXT_INFO_ADDRESS: usize = 0x118;
const CSR_IMAGE_LOADER_ADDRESS: usize = 0x120;
const CSR_IMAGE_LOADER_SIZE: usize = 0x128;
const CSR_DEBUG_HPET_MEMORY: usize = 0x240;
const CSR_DEBUG_LINK_POWER: usize = 0x250;
const HBUS_PRPH_WRITE_ADDRESS: usize = 0x444;
const HBUS_PRPH_READ_ADDRESS: usize = 0x448;
const HBUS_PRPH_WRITE_DATA: usize = 0x44c;
const HBUS_PRPH_READ_DATA: usize = 0x450;
const RX_FREE_WRITE_INDEX: usize = 0x1c80;

const HW_IF_NIC_READY: u32 = 0x0040_0000;
const HW_IF_PREPARE: u32 = 0x0800_0000;
const HW_IF_HAP_WAKE_L1A: u32 = 0x0008_0000;
const HW_IF_AUTO_FUNCTION_BOOT: u32 = 1 << 1;
const HW_IF_RADIO_MASK: u32 = 0x0000_ff0f;
const MBOX_OS_ALIVE: u32 = 0x20;
const RESET_SOFTWARE: u32 = 0x80;
const LINK_POWER_MANAGEMENT_DISABLED: u32 = 0x8000_0000;
const GP_MAC_CLOCK_READY: u32 = 0x01;
const GP_INIT_DONE: u32 = 0x04;
const GP_MAC_ACCESS_REQUEST: u32 = 0x08;
const GP_GOING_TO_SLEEP: u32 = 0x10;
const GP_RADIO_KILL_SWITCH: u32 = 0x0800_0000;
const GIO_L0S_DISABLED: u32 = 0x02;
const CHICKEN_L1A_NO_L0S_RX: u32 = 0x0080_0000;
const HPET_MEMORY_VALUE: u32 = 0xffff_0000;
const INT_ALIVE: u32 = 1 << 0;
const UCODE_RADIO_KILL: u32 = 0x02;
const UCODE_COMMAND_BLOCKED: u32 = 0x04;
const MAC_SHADOW_ALL: u32 = 0x800f_ffff;
const INTERRUPT_COALESCING_DEFAULT: u8 = 0x40;

const PRPH_TARGET: u32 = 3 << 24;
const UREG_CPU_INIT_RUN: u32 = 0x00a0_5c44;
const UMAC_PRPH_OFFSET: u32 = 0x0030_0000;
const HPM_DEBUG: u32 = 0x00a0_3440;
const PERSISTENCE: u32 = 1 << 12;
const WRITE_PROTECT_22000: u32 = 0x00a0_4d00;
const WFPM_ACCESS: u32 = 1 << 12;
const HIPM_GENERAL_CONFIG: u32 = 0x00a0_3458;
const HIPM_POWER_GATING: u32 = 1 << 0;
const HIPM_SLEEP: u32 = 1 << 1;
const HIPM_FORCE_ACTIVE: u32 = 1 << 10;
const HPM_MAC_LTR: u32 = 0x00a0_348c;
const HPM_MAC_LTR_ENABLE_ALL: u32 = 0xf;
const HPM_UMAC_LTR: u32 = 0x00a0_3480;
const LTR_BOOT_VALUE: u32 = 0x8000_0000 | (2 << 24) | (250 << 16) | 0x8000 | (2 << 8) | 250;

/// The receive ring's buffers.
pub const RX_BUFFERS: usize = 512;
/// One receive buffer's size.
pub const RX_BUFFER_BYTES: usize = 4096;
/// The command queue's descriptors.
pub const TX_DESCRIPTORS: usize = 256;
/// One transmit descriptor (TFD) in the long format.
pub const TFD_BYTES: usize = 256;
const FIRST_RX_WRITE_INDEX: u32 = 8;
const RX_RING_SIZE_LOG2: u32 = 9;
const TX_RING_SIZE_CODE: u32 = 5;
const DRAM_ENTRIES: usize = 64;

const CONTEXT_INFO: usize = 0x0000;
const PRPH_SCRATCH: usize = 0x0800;
const PRPH_INFO: usize = 0x1000;
const RX_FREE: usize = 0x2000;
const RX_USED: usize = 0x4000;
const RX_STATUS: usize = 0x8000;
const TX_RING: usize = 0x9000;
/// The bytes of the block the rings and the context info share.
pub const RING_BYTES: usize = 0x2_0000;
/// The bytes of the block the receive buffers fill.
pub const RX_BUFFER_AREA_BYTES: usize = RX_BUFFERS * RX_BUFFER_BYTES;

const CONTEXT_CONTROL_FLAGS: usize = 8;
const CONTEXT_FREE_RBD: usize = 24;
const CONTEXT_USED_RBD: usize = 32;
const CONTEXT_STATUS: usize = 40;
const CONTEXT_COMMAND_QUEUE: usize = 48;
const CONTEXT_COMMAND_QUEUE_SIZE: usize = 56;
const CONTEXT_DRAM: usize = 192;
const CONTEXT_INFO_BYTES: usize = 1792;
const CONTEXT_TFD_FORMAT_LONG: u32 = 1 << 8;
const CONTEXT_RB_CB_SIZE_SHIFT: u32 = 4;
const CONTEXT_RB_SIZE_4K: u32 = 4 << 9;

const GEN3_PRPH_INFO: usize = 8;
const GEN3_CR_HEAD_INDEXES: usize = 16;
const GEN3_TR_TAIL_INDEXES: usize = 24;
const GEN3_CR_TAIL_INDEXES: usize = 32;
const GEN3_MTR_BASE: usize = 52;
const GEN3_MCR_BASE: usize = 60;
const GEN3_MTR_SIZE: usize = 68;
const GEN3_MCR_SIZE: usize = 70;
const GEN3_PRPH_SCRATCH: usize = 88;
const GEN3_PRPH_SCRATCH_SIZE: usize = 96;
const SCRATCH_SIZE: usize = 4;
const SCRATCH_CONTROL_FLAGS: usize = 8;
const SCRATCH_FREE_RBD: usize = 48;
const SCRATCH_DRAM: usize = 124;
const SCRATCH_BYTES: usize = 1660;
const SCRATCH_RB_SIZE_4K: u32 = 1 << 16;
const SCRATCH_MTR_MODE: u32 = 1 << 17;
const SCRATCH_MTR_FORMAT_256B: u32 = 0x000c_0000;
const SCRATCH_IMR_DEBUG: u32 = 1 << 1;

const PACKET_LENGTH_MASK: u32 = 0x3fff;
const PACKET_HEADER_BYTES: usize = 8;
const PACKET_ALIGNMENT: usize = 0x40;
const FRAME_INVALID: u32 = 0x5555_0000;
const UNSOLICITED: u8 = 0x80;
const COMMAND_FAILED: u8 = 0x40;
const MAX_NOTIFICATIONS: usize = 256;
const COMMAND_QUEUE: u8 = 0;
const COMMAND_BUFFER: usize = 0x1c000;
const COMMAND_BUFFER_BYTES: usize = 0x1000;
const WIDE_HEADER_BYTES: usize = 8;
const FIRST_TB_BYTES: usize = 20;
const TB_BYTES: usize = 10;
const TB_LENGTH: usize = 2;
const TB_ADDRESS: usize = 4;
const GEN3_TX_QUEUE_SIZE: usize = 65_536;
const HBUS_WRITE_POINTER: usize = 0x460;
const UREG_DOORBELL_TO_ISR6: u32 = 0x00a0_5c04;
const DOORBELL_PNVM: u32 = 1 << 20;
const MAC_ADDRESS_OTP: usize = 0x380;
const MAC_ADDRESS_STRAP: usize = 0x388;
/// The firmware's legacy command group.
pub const LEGACY_GROUP: u8 = 0x0;
/// The long group, which group 0's commands are sent as.
pub const LONG_GROUP: u8 = 0x1;
/// The system group.
pub const SYSTEM_GROUP: u8 = 0x2;
/// The regulatory and NVM group.
pub const REGULATORY_AND_NVM_GROUP: u8 = 0xc;
const PNVM_INIT_COMPLETE: u8 = 0xfe;
const ALIVE: u8 = 0x01;
const ALIVE_OK: u16 = 0xcafe;
const LMAC_ALIVE_BYTES: usize = 48;
const ALIVE_UMAC_MAJOR: usize = 4 + 2 * LMAC_ALIVE_BYTES;

/// Why the adapter did not start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportError {
    /// It never said it was ready for the host.
    NotReady,
    /// It did not answer before the deadline.
    TimedOut,
    /// Its MAC did not wake for a register access.
    MacAsleep,
    /// Its persistence bit is set and write-protected.
    Persistent,
    /// No DMA memory for the firmware.
    NoMemory,
    /// The firmware file lacks what this adapter needs.
    BadFirmware,
    /// The firmware said it did not start, with this status.
    FirmwareFailed(u16),
    /// A command is longer than the command buffer.
    CommandTooLong,
    /// The firmware refused a command with this code.
    CommandFailed(u8),
    /// The adapter holds no valid MAC address.
    NoAddress,
    /// The NVM's answer is not one this driver reads.
    BadNvm,
}

impl TransportError {
    /// The reason, for the console.
    pub fn description(self) -> &'static str {
        match self {
            TransportError::NotReady => "the Wi-Fi adapter never became ready",
            TransportError::TimedOut => "the Wi-Fi adapter did not answer in time",
            TransportError::MacAsleep => "the Wi-Fi adapter did not wake for the host",
            TransportError::Persistent => "the Wi-Fi adapter is held in its persistent state",
            TransportError::NoMemory => "no memory for the Wi-Fi firmware",
            TransportError::BadFirmware => "the Wi-Fi firmware file is not one for this adapter",
            TransportError::FirmwareFailed(_) => "the Wi-Fi firmware did not start",
            TransportError::CommandTooLong => "a Wi-Fi command was too long",
            TransportError::CommandFailed(_) => "the Wi-Fi firmware refused a command",
            TransportError::NoAddress => "the Wi-Fi adapter has no MAC address",
            TransportError::BadNvm => "the Wi-Fi adapter's NVM answer is not one the driver reads",
        }
    }
}

/// How the host waits: short pauses between register polls, and whether it may go on waiting.
pub trait Wait {
    /// Waits about `microseconds`.
    fn pause(&mut self, microseconds: u32);
    /// Whether the operation's deadline has not passed.
    fn keep_waiting(&mut self) -> bool;
}

/// What the firmware's alive notification said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Alive {
    /// The notification's payload length, which tells its version apart.
    pub length: usize,
    /// The UMAC's major and minor version.
    pub umac_version: (u32, u32),
}

fn put16<M: DmaMemory>(memory: &M, at: usize, value: u16) {
    put_bytes(memory, at, &value.to_le_bytes());
}

fn put32<M: DmaMemory>(memory: &M, at: usize, value: u32) {
    put_bytes(memory, at, &value.to_le_bytes());
}

fn put64<M: DmaMemory>(memory: &M, at: usize, value: u64) {
    put_bytes(memory, at, &value.to_le_bytes());
}

fn put_bytes<M: DmaMemory>(memory: &M, at: usize, bytes: &[u8]) {
    assert!(at + bytes.len() <= memory.len());
    // SAFETY: the range lies inside the block (checked), which the driver owns; the device does
    // not write these bytes while the host does.
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), memory.as_mut_ptr().add(at), bytes.len())
    };
}

fn get_bytes<M: DmaMemory>(memory: &M, at: usize, bytes: &mut [u8]) {
    assert!(at + bytes.len() <= memory.len());
    fence(Ordering::SeqCst);
    // SAFETY: the range lies inside the block (checked); a volatile copy is not needed, as the
    // fence above orders the device's completed writes before the read.
    unsafe {
        core::ptr::copy_nonoverlapping(memory.as_mut_ptr().add(at), bytes.as_mut_ptr(), bytes.len())
    };
}

fn get16<M: DmaMemory>(memory: &M, at: usize) -> u16 {
    let mut bytes = [0; 2];
    get_bytes(memory, at, &mut bytes);
    u16::from_le_bytes(bytes)
}

fn get32<M: DmaMemory>(memory: &M, at: usize) -> u32 {
    let mut bytes = [0; 4];
    get_bytes(memory, at, &mut bytes);
    u32::from_le_bytes(bytes)
}

fn dma<M: DmaMemory>(memory: &M, at: usize) -> u64 {
    memory.dma_addr().get() + at as u64
}

/// `CSR_HW_REV` (`raw`) in the older layout the adapter table compares against: the step moved
/// into bits 2 and 3.
pub fn older_hw_rev(raw: u32) -> u32 {
    (raw & 0xfff0) | ((raw & 0x3) << 2)
}

/// The adapter's host interface, its rings and the firmware's sections while it loads them.
pub struct Transport<R: RegisterIo, M: DmaMemory> {
    regs: R,
    adapter: Adapter,
    hw_rev: u32,
    nic_locks: u32,
    rings: M,
    rx_buffers: M,
    sections: Vec<M>,
    paging: Vec<M>,
    image_loader: Option<M>,
    rx_next: usize,
    tx_next: usize,
    tx_hardware: usize,
    responses: Vec<(u8, Packet)>,
    notifications: Vec<Packet>,
}

/// A packet the firmware sent: a command's response or a notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub group: u8,
    pub code: u8,
    /// Whether it says the command failed.
    pub failed: bool,
    pub payload: Vec<u8>,
}

fn flip_address(low: u32, high: u32) -> [u8; 6] {
    let low = low.to_le_bytes();
    let high = high.to_le_bytes();
    [low[3], low[2], low[1], low[0], high[1], high[0]]
}

/// Whether `address` may be an adapter's own: not the firmware's reserved one, broadcast, all
/// zeros or multicast.
pub fn is_valid_address(address: &[u8; 6]) -> bool {
    const RESERVED: [u8; 6] = [0x02, 0xcc, 0xaa, 0xff, 0xee, 0x00];
    *address != RESERVED && *address != [0xff; 6] && *address != [0; 6] && address[0] & 1 == 0
}

impl<R: RegisterIo, M: DmaMemory> Transport<R, M> {
    /// The interface behind `regs` for `adapter`, with `rings` ([`RING_BYTES`]) and
    /// `rx_buffers` ([`RX_BUFFER_AREA_BYTES`]), both zeroed.
    pub fn new(regs: R, adapter: Adapter, rings: M, rx_buffers: M) -> Self {
        let hw_rev = older_hw_rev(regs.read32(CSR_HW_REV));
        Self {
            regs,
            adapter,
            hw_rev,
            nic_locks: 0,
            rings,
            rx_buffers,
            sections: Vec::new(),
            paging: Vec::new(),
            image_loader: None,
            rx_next: 0,
            tx_next: 0,
            tx_hardware: 0,
            responses: Vec::new(),
            notifications: Vec::new(),
        }
    }

    /// The `CSR_HW_REV` register in the older layout the adapter table compares against.
    pub fn hw_rev(&self) -> u32 {
        self.hw_rev
    }

    fn set_bits(&self, register: usize, bits: u32) {
        self.regs
            .write32(register, self.regs.read32(register) | bits);
    }

    fn clear_bits(&self, register: usize, bits: u32) {
        self.regs
            .write32(register, self.regs.read32(register) & !bits);
    }

    fn poll(
        &self,
        register: usize,
        bits: u32,
        mask: u32,
        microseconds: u32,
        wait: &mut dyn Wait,
    ) -> bool {
        let mut left = microseconds;
        loop {
            if self.regs.read32(register) & mask == bits & mask {
                return true;
            }
            if left < 10 {
                return false;
            }
            left -= 10;
            wait.pause(10);
        }
    }

    fn prph_mask(&self) -> u32 {
        match self.adapter.family {
            Family::Ax210 => 0x00ff_ffff,
            Family::Ax200 => 0x000f_ffff,
        }
    }

    fn read_prph(&self, address: u32) -> u32 {
        self.regs.write32(
            HBUS_PRPH_READ_ADDRESS,
            (address & self.prph_mask()) | PRPH_TARGET,
        );
        fence(Ordering::SeqCst);
        self.regs.read32(HBUS_PRPH_READ_DATA)
    }

    fn write_prph(&self, address: u32, value: u32) {
        self.regs.write32(
            HBUS_PRPH_WRITE_ADDRESS,
            (address & self.prph_mask()) | PRPH_TARGET,
        );
        fence(Ordering::SeqCst);
        self.regs.write32(HBUS_PRPH_WRITE_DATA, value);
    }

    fn update_prph(
        &mut self,
        address: u32,
        set: u32,
        clear: u32,
        wait: &mut dyn Wait,
    ) -> Result<(), TransportError> {
        self.lock_nic(wait)?;
        let value = (self.read_prph(address) & !clear) | set;
        self.write_prph(address, value);
        self.unlock_nic();
        Ok(())
    }

    /// Wakes the MAC for register accesses, nesting.
    pub fn lock_nic(&mut self, wait: &mut dyn Wait) -> Result<(), TransportError> {
        if self.nic_locks > 0 {
            self.nic_locks += 1;
            return Ok(());
        }
        self.set_bits(CSR_GP_CNTRL, GP_MAC_ACCESS_REQUEST);
        wait.pause(2);
        let mask = GP_MAC_CLOCK_READY | GP_GOING_TO_SLEEP;
        if !self.poll(CSR_GP_CNTRL, GP_MAC_CLOCK_READY, mask, 150_000, wait) {
            return Err(TransportError::MacAsleep);
        }
        self.nic_locks = 1;
        Ok(())
    }

    /// Lets the MAC sleep again once every lock is released.
    pub fn unlock_nic(&mut self) {
        if self.nic_locks == 1 {
            self.clear_bits(CSR_GP_CNTRL, GP_MAC_ACCESS_REQUEST);
        }
        self.nic_locks = self.nic_locks.saturating_sub(1);
    }

    fn hardware_ready(&self, wait: &mut dyn Wait) -> bool {
        self.set_bits(CSR_HW_IF_CONFIG, HW_IF_NIC_READY);
        let ready = self.poll(CSR_HW_IF_CONFIG, HW_IF_NIC_READY, HW_IF_NIC_READY, 50, wait);
        if ready {
            self.set_bits(CSR_MBOX_SET, MBOX_OS_ALIVE);
        }
        ready
    }

    fn prepare(&self, wait: &mut dyn Wait) -> Result<(), TransportError> {
        if self.hardware_ready(wait) {
            return Ok(());
        }
        self.set_bits(CSR_DEBUG_LINK_POWER, LINK_POWER_MANAGEMENT_DISABLED);
        wait.pause(1000);
        for _ in 0..10 {
            self.set_bits(CSR_HW_IF_CONFIG, HW_IF_PREPARE);
            let mut waited = 0;
            while waited < 150_000 {
                if self.hardware_ready(wait) {
                    return Ok(());
                }
                wait.pause(200);
                waited += 200;
            }
            wait.pause(25_000);
        }
        Err(TransportError::NotReady)
    }

    fn clear_persistence(&self) -> Result<(), TransportError> {
        let hpm = self.read_prph(HPM_DEBUG);
        if hpm != 0xa5a5_a5a0 && hpm & PERSISTENCE != 0 {
            if self.read_prph(WRITE_PROTECT_22000) & WFPM_ACCESS != 0 {
                return Err(TransportError::Persistent);
            }
            self.write_prph(HPM_DEBUG, hpm & !PERSISTENCE);
        }
        Ok(())
    }

    fn software_reset(&self, wait: &mut dyn Wait) {
        self.set_bits(CSR_RESET, RESET_SOFTWARE);
        wait.pause(5000);
    }

    fn force_power_gating(&mut self, wait: &mut dyn Wait) -> Result<(), TransportError> {
        self.update_prph(HIPM_GENERAL_CONFIG, HIPM_FORCE_ACTIVE, 0, wait)?;
        wait.pause(20);
        self.update_prph(HIPM_GENERAL_CONFIG, HIPM_POWER_GATING | HIPM_SLEEP, 0, wait)?;
        wait.pause(20);
        self.update_prph(HIPM_GENERAL_CONFIG, 0, HIPM_FORCE_ACTIVE, wait)
    }

    fn power_up(&self, wait: &mut dyn Wait) -> Result<(), TransportError> {
        self.set_bits(CSR_GIO_CHICKEN_BITS, CHICKEN_L1A_NO_L0S_RX);
        self.set_bits(CSR_DEBUG_HPET_MEMORY, HPET_MEMORY_VALUE);
        self.set_bits(CSR_HW_IF_CONFIG, HW_IF_HAP_WAKE_L1A);
        self.set_bits(CSR_GIO, GIO_L0S_DISABLED);
        self.set_bits(CSR_GP_CNTRL, GP_INIT_DONE);
        if !self.poll(
            CSR_GP_CNTRL,
            GP_MAC_CLOCK_READY,
            GP_MAC_CLOCK_READY,
            25_000,
            wait,
        ) {
            return Err(TransportError::TimedOut);
        }
        Ok(())
    }

    /// Readies, resets and powers the adapter up, its firmware not yet running.
    pub fn start_hardware(&mut self, wait: &mut dyn Wait) -> Result<(), TransportError> {
        self.prepare(wait)?;
        if self.adapter.family == Family::Ax200 {
            self.clear_persistence()?;
        }
        self.software_reset(wait);
        if self.adapter.family == Family::Ax200 && self.adapter.integrated {
            self.set_bits(CSR_GP_CNTRL, GP_INIT_DONE);
            wait.pause(20);
            if !self.poll(
                CSR_GP_CNTRL,
                GP_MAC_CLOCK_READY,
                GP_MAC_CLOCK_READY,
                25_000,
                wait,
            ) {
                return Err(TransportError::TimedOut);
            }
            self.force_power_gating(wait)?;
            self.software_reset(wait);
        }
        self.power_up(wait)
    }

    /// Whether the radio's hardware kill switch is on.
    pub fn radio_killed(&self) -> bool {
        self.regs.read32(CSR_GP_CNTRL) & GP_RADIO_KILL_SWITCH == 0
    }

    fn quiet_interrupts(&self) {
        self.regs.write32(CSR_INT_MASK, 0);
        self.regs.write32(CSR_INT, !0);
        self.regs.write32(CSR_FH_INT_STATUS, !0);
    }

    fn configure_radio(&self, firmware: &Firmware) {
        let phy = firmware.phy_sku;
        let mac_step = (self.hw_rev >> 2) & 0x3;
        let mac_dash = self.hw_rev & 0x3;
        let value = (mac_step << 2)
            | mac_dash
            | ((phy & 0x3) << 10)
            | (((phy >> 2) & 0x3) << 14)
            | (((phy >> 4) & 0x3) << 12);
        let current = self.regs.read32(CSR_HW_IF_CONFIG) & !HW_IF_RADIO_MASK;
        self.regs.write32(CSR_HW_IF_CONFIG, current | value);
    }

    fn set_latency_tolerance(&self) {
        if !self.adapter.integrated {
            self.regs.write32(CSR_LTR_LONG_VALUE, LTR_BOOT_VALUE);
        } else if self.adapter.family == Family::Ax200 {
            self.write_prph(HPM_MAC_LTR, HPM_MAC_LTR_ENABLE_ALL);
            self.write_prph(HPM_UMAC_LTR, LTR_BOOT_VALUE);
        }
    }

    fn fill_rx_descriptors(&self) {
        for index in 0..RX_BUFFERS {
            let buffer = dma(&self.rx_buffers, index * RX_BUFFER_BYTES);
            match self.adapter.family {
                Family::Ax210 => {
                    put16(&self.rings, RX_FREE + index * 16, index as u16);
                    put64(&self.rings, RX_FREE + index * 16 + 8, buffer);
                }
                Family::Ax200 => put64(
                    &self.rings,
                    RX_FREE + index * 8,
                    buffer | (index as u64 & 0xfff),
                ),
            }
        }
        fence(Ordering::SeqCst);
    }

    fn copy_sections<A: DmaAllocator<Memory = M>>(
        bytes: &[u8],
        sections: &[Section],
        allocator: &mut A,
    ) -> Result<Vec<M>, TransportError> {
        let mut copies = Vec::new();
        for section in sections {
            let data = bytes
                .get(section.bytes.clone())
                .ok_or(TransportError::BadFirmware)?;
            let memory = allocator
                .allocate(data.len().max(1))
                .ok_or(TransportError::NoMemory)?;
            put_bytes(&memory, 0, data);
            copies.push(memory);
        }
        Ok(copies)
    }

    fn write_dram(&self, base: usize, lmac: usize, umac: usize) {
        let addresses = |memories: &[M]| {
            memories
                .iter()
                .map(|memory| memory.dma_addr().get())
                .collect::<Vec<_>>()
        };
        let firmware = addresses(&self.sections);
        let (lmac_images, umac_images) = firmware.split_at(lmac.min(firmware.len()));
        for (index, address) in umac_images.iter().take(umac).enumerate() {
            put64(&self.rings, base + index * 8, *address);
        }
        for (index, address) in lmac_images.iter().enumerate() {
            put64(&self.rings, base + (DRAM_ENTRIES + index) * 8, *address);
        }
        for (index, address) in addresses(&self.paging).iter().enumerate() {
            put64(&self.rings, base + (2 * DRAM_ENTRIES + index) * 8, *address);
        }
    }

    fn write_context_info(&self, lmac: usize, umac: usize) {
        put16(
            &self.rings,
            CONTEXT_INFO,
            self.regs.read32(CSR_HW_REV) as u16,
        );
        put16(
            &self.rings,
            CONTEXT_INFO + 4,
            (CONTEXT_INFO_BYTES / 4) as u16,
        );
        put32(
            &self.rings,
            CONTEXT_INFO + CONTEXT_CONTROL_FLAGS,
            CONTEXT_TFD_FORMAT_LONG
                | (RX_RING_SIZE_LOG2 << CONTEXT_RB_CB_SIZE_SHIFT)
                | CONTEXT_RB_SIZE_4K,
        );
        put64(
            &self.rings,
            CONTEXT_INFO + CONTEXT_FREE_RBD,
            dma(&self.rings, RX_FREE),
        );
        put64(
            &self.rings,
            CONTEXT_INFO + CONTEXT_USED_RBD,
            dma(&self.rings, RX_USED),
        );
        put64(
            &self.rings,
            CONTEXT_INFO + CONTEXT_STATUS,
            dma(&self.rings, RX_STATUS),
        );
        put64(
            &self.rings,
            CONTEXT_INFO + CONTEXT_COMMAND_QUEUE,
            dma(&self.rings, TX_RING),
        );
        put_bytes(
            &self.rings,
            CONTEXT_INFO + CONTEXT_COMMAND_QUEUE_SIZE,
            &[TX_RING_SIZE_CODE as u8],
        );
        self.write_dram(CONTEXT_INFO + CONTEXT_DRAM, lmac, umac);
    }

    fn write_context_info_gen3(&self, lmac: usize, umac: usize) {
        let mut control = SCRATCH_RB_SIZE_4K | SCRATCH_MTR_MODE | SCRATCH_MTR_FORMAT_256B;
        if self.adapter.imr {
            control |= SCRATCH_IMR_DEBUG;
        }
        put16(
            &self.rings,
            PRPH_SCRATCH,
            self.regs.read32(CSR_HW_REV) as u16,
        );
        put16(
            &self.rings,
            PRPH_SCRATCH + SCRATCH_SIZE,
            (SCRATCH_BYTES / 4) as u16,
        );
        put32(&self.rings, PRPH_SCRATCH + SCRATCH_CONTROL_FLAGS, control);
        put64(
            &self.rings,
            PRPH_SCRATCH + SCRATCH_FREE_RBD,
            dma(&self.rings, RX_FREE),
        );
        self.write_dram(PRPH_SCRATCH + SCRATCH_DRAM, lmac, umac);
        let info = dma(&self.rings, PRPH_INFO);
        put64(&self.rings, CONTEXT_INFO + GEN3_PRPH_INFO, info);
        put64(
            &self.rings,
            CONTEXT_INFO + GEN3_PRPH_SCRATCH,
            dma(&self.rings, PRPH_SCRATCH),
        );
        put32(
            &self.rings,
            CONTEXT_INFO + GEN3_PRPH_SCRATCH_SIZE,
            SCRATCH_BYTES as u32,
        );
        put64(
            &self.rings,
            CONTEXT_INFO + GEN3_CR_HEAD_INDEXES,
            dma(&self.rings, RX_STATUS),
        );
        put64(
            &self.rings,
            CONTEXT_INFO + GEN3_TR_TAIL_INDEXES,
            info + 2048,
        );
        put64(
            &self.rings,
            CONTEXT_INFO + GEN3_CR_TAIL_INDEXES,
            info + 3072,
        );
        put64(
            &self.rings,
            CONTEXT_INFO + GEN3_MTR_BASE,
            dma(&self.rings, TX_RING),
        );
        put64(
            &self.rings,
            CONTEXT_INFO + GEN3_MCR_BASE,
            dma(&self.rings, RX_USED),
        );
        put16(
            &self.rings,
            CONTEXT_INFO + GEN3_MTR_SIZE,
            TX_RING_SIZE_CODE as u16,
        );
        put16(
            &self.rings,
            CONTEXT_INFO + GEN3_MCR_SIZE,
            RX_RING_SIZE_LOG2 as u16,
        );
    }

    fn write_address(&self, register: usize, address: u64) {
        self.regs.write32(register, address as u32);
        self.regs.write32(register + 4, (address >> 32) as u32);
    }

    /// Loads `firmware`'s regular image (the file's bytes are `bytes`) and waits for its alive
    /// notification; `allocator` gives the sections their DMA memory, which this keeps.
    pub fn load_firmware<A: DmaAllocator<Memory = M>>(
        &mut self,
        firmware: &Firmware,
        bytes: &[u8],
        allocator: &mut A,
        wait: &mut dyn Wait,
    ) -> Result<Alive, TransportError> {
        self.regs.write32(CSR_INT, !0);
        self.quiet_interrupts();
        self.regs.write32(CSR_UCODE_DRV_GP1_CLR, UCODE_RADIO_KILL);
        self.regs
            .write32(CSR_UCODE_DRV_GP1_CLR, UCODE_COMMAND_BLOCKED);
        self.regs.write32(CSR_INT, !0);
        self.power_up(wait)?;
        if self.adapter.family == Family::Ax200 {
            self.configure_radio(firmware);
        }
        self.regs
            .write8(CSR_INT_COALESCING, INTERRUPT_COALESCING_DEFAULT);
        self.set_bits(CSR_MAC_SHADOW_CONTROL, MAC_SHADOW_ALL);
        self.fill_rx_descriptors();
        let [lmac, umac, paging] = firmware.split(Image::Regular);
        if lmac.is_empty()
            || umac.is_empty()
            || lmac.len() > DRAM_ENTRIES
            || umac.len() > DRAM_ENTRIES
            || paging.len() > DRAM_ENTRIES
        {
            return Err(TransportError::BadFirmware);
        }
        let mut images = Self::copy_sections(bytes, lmac, allocator)?;
        images.extend(Self::copy_sections(bytes, umac, allocator)?);
        self.sections = images;
        self.paging = Self::copy_sections(bytes, paging, allocator)?;
        match self.adapter.family {
            Family::Ax200 => {
                self.write_context_info(lmac.len(), umac.len());
                fence(Ordering::SeqCst);
                self.write_address(CSR_CONTEXT_INFO_BASE, dma(&self.rings, CONTEXT_INFO));
                self.lock_nic(wait)?;
                self.set_latency_tolerance();
                self.write_prph(UREG_CPU_INIT_RUN, 1);
                self.unlock_nic();
            }
            Family::Ax210 => {
                let loader = firmware
                    .image_loader
                    .clone()
                    .ok_or(TransportError::BadFirmware)?;
                let loader = bytes.get(loader).ok_or(TransportError::BadFirmware)?;
                let memory = allocator
                    .allocate(loader.len())
                    .ok_or(TransportError::NoMemory)?;
                put_bytes(&memory, 0, loader);
                let loader_address = memory.dma_addr().get();
                self.image_loader = Some(memory);
                self.write_context_info_gen3(lmac.len(), umac.len());
                fence(Ordering::SeqCst);
                self.write_address(CSR_CONTEXT_INFO_ADDRESS, dma(&self.rings, CONTEXT_INFO));
                self.write_address(CSR_IMAGE_LOADER_ADDRESS, loader_address);
                self.regs
                    .write32(CSR_IMAGE_LOADER_SIZE, loader.len() as u32);
                self.set_bits(CSR_HW_IF_CONFIG, HW_IF_AUTO_FUNCTION_BOOT);
                self.lock_nic(wait)?;
                self.set_latency_tolerance();
                self.write_prph(UREG_CPU_INIT_RUN + UMAC_PRPH_OFFSET, 1);
                self.unlock_nic();
            }
        }
        self.wait_for_alive(wait)
    }

    fn rx_closed(&self) -> usize {
        usize::from(get16(&self.rings, RX_STATUS) & 0xfff) & (RX_BUFFERS - 1)
    }

    fn wait_for_alive(&mut self, wait: &mut dyn Wait) -> Result<Alive, TransportError> {
        while self.regs.read32(CSR_INT) & INT_ALIVE == 0 {
            if !wait.keep_waiting() {
                return Err(TransportError::TimedOut);
            }
            wait.pause(100);
        }
        self.regs.write32(CSR_INT, INT_ALIVE);
        self.fill_rx_descriptors();
        self.regs.write32(RX_FREE_WRITE_INDEX, FIRST_RX_WRITE_INDEX);
        let payload = self.wait_notification(LEGACY_GROUP, ALIVE, wait)?;
        let status = payload
            .get(..2)
            .map_or(0, |bytes| u16::from_le_bytes([bytes[0], bytes[1]]));
        if status != ALIVE_OK {
            return Err(TransportError::FirmwareFailed(status));
        }
        let word = |at: usize| {
            payload.get(at..at + 4).map_or(0, |bytes| {
                u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
            })
        };
        Ok(Alive {
            length: payload.len(),
            umac_version: (word(ALIVE_UMAC_MAJOR), word(ALIVE_UMAC_MAJOR + 4)),
        })
    }

    fn read_packets(&mut self) {
        let closed = self.rx_closed();
        if self.rx_next == closed {
            return;
        }
        while self.rx_next != closed {
            let buffer = self.rx_next * RX_BUFFER_BYTES;
            self.rx_next = (self.rx_next + 1) % RX_BUFFERS;
            let mut offset = 0;
            while offset + PACKET_HEADER_BYTES <= RX_BUFFER_BYTES {
                let at = buffer + offset;
                let length_and_flags = get32(&self.rx_buffers, at);
                let mut header = [0u8; 4];
                get_bytes(&self.rx_buffers, at + 4, &mut header);
                let [code, group, index, queue] = header;
                let length = (length_and_flags & PACKET_LENGTH_MASK) as usize;
                let invalid = (queue & !UNSOLICITED == 0 && index == 0 && code == 0)
                    || length_and_flags == FRAME_INVALID
                    || length < 4
                    || offset + 4 + length > RX_BUFFER_BYTES;
                if invalid {
                    break;
                }
                let mut payload = alloc::vec![0u8; length - 4];
                get_bytes(&self.rx_buffers, at + PACKET_HEADER_BYTES, &mut payload);
                let solicited = queue & UNSOLICITED == 0;
                let packet = Packet {
                    group,
                    code,
                    failed: solicited && group & COMMAND_FAILED != 0,
                    payload,
                };
                if solicited && queue == COMMAND_QUEUE {
                    self.responses.push((index, packet));
                } else {
                    if self.notifications.len() == MAX_NOTIFICATIONS {
                        self.notifications.remove(0);
                    }
                    self.notifications.push(packet);
                }
                if self.adapter.family == Family::Ax210 {
                    break;
                }
                offset += (4 + length).next_multiple_of(PACKET_ALIGNMENT);
            }
        }
        let returned = match closed {
            0 => RX_BUFFERS - 1,
            closed => closed - 1,
        };
        self.regs.write32(RX_FREE_WRITE_INDEX, returned as u32 & !7);
    }

    /// Waits for the firmware's notification `code` in `group`, taking it from those already
    /// read first: its payload.
    pub fn wait_notification(
        &mut self,
        group: u8,
        code: u8,
        wait: &mut dyn Wait,
    ) -> Result<Vec<u8>, TransportError> {
        loop {
            self.read_packets();
            let found = self
                .notifications
                .iter()
                .position(|packet| packet.code == code && packet.group == group);
            if let Some(at) = found {
                return Ok(self.notifications.remove(at).payload);
            }
            if !wait.keep_waiting() {
                return Err(TransportError::TimedOut);
            }
            wait.pause(100);
        }
    }

    /// The firmware's notifications read so far and not yet waited for, oldest first.
    pub fn take_notifications(&mut self) -> Vec<Packet> {
        self.read_packets();
        core::mem::take(&mut self.notifications)
    }

    /// Sends command `code` of `group` (group 0's commands go as the long group's, as the
    /// firmware asks) with `payload` through the command queue and waits for its response's
    /// payload.
    pub fn send_command(
        &mut self,
        group: u8,
        code: u8,
        payload: &[u8],
        wait: &mut dyn Wait,
    ) -> Result<Vec<u8>, TransportError> {
        if payload.len() > COMMAND_BUFFER_BYTES - WIDE_HEADER_BYTES {
            return Err(TransportError::CommandTooLong);
        }
        let group = match group {
            LEGACY_GROUP => LONG_GROUP,
            group => group,
        };
        let index = self.tx_next;
        let mut header = [0u8; WIDE_HEADER_BYTES];
        header[0] = code;
        header[1] = group;
        header[2] = index as u8;
        header[3] = COMMAND_QUEUE;
        header[4..6].copy_from_slice(&(payload.len() as u16).to_le_bytes());
        put_bytes(&self.rings, COMMAND_BUFFER, &header);
        put_bytes(&self.rings, COMMAND_BUFFER + WIDE_HEADER_BYTES, payload);
        let total = WIDE_HEADER_BYTES + payload.len();
        let descriptor = TX_RING + index * TFD_BYTES;
        put_bytes(&self.rings, descriptor, &[0; TFD_BYTES]);
        put16(
            &self.rings,
            descriptor + TB_LENGTH,
            total.min(FIRST_TB_BYTES) as u16,
        );
        put64(
            &self.rings,
            descriptor + TB_ADDRESS,
            dma(&self.rings, COMMAND_BUFFER),
        );
        let buffers = match total > FIRST_TB_BYTES {
            true => {
                put16(
                    &self.rings,
                    descriptor + TB_BYTES + TB_LENGTH,
                    (total - FIRST_TB_BYTES) as u16,
                );
                put64(
                    &self.rings,
                    descriptor + TB_BYTES + TB_ADDRESS,
                    dma(&self.rings, COMMAND_BUFFER + FIRST_TB_BYTES),
                );
                2
            }
            false => 1,
        };
        put16(&self.rings, descriptor, buffers);
        fence(Ordering::SeqCst);
        self.tx_next = (self.tx_next + 1) % TX_DESCRIPTORS;
        self.tx_hardware = (self.tx_hardware + 1) % self.tx_queue_size();
        self.regs.write32(
            HBUS_WRITE_POINTER,
            (u32::from(COMMAND_QUEUE) << 16) | self.tx_hardware as u32,
        );
        loop {
            self.read_packets();
            if let Some(at) = self
                .responses
                .iter()
                .position(|(answered, _)| usize::from(*answered) == index)
            {
                let (_, packet) = self.responses.remove(at);
                self.responses.clear();
                return match packet.failed {
                    true => Err(TransportError::CommandFailed(code)),
                    false => Ok(packet.payload),
                };
            }
            if !wait.keep_waiting() {
                return Err(TransportError::TimedOut);
            }
            wait.pause(50);
        }
    }

    /// Moves a transmit queue's write pointer: `value` is the queue's number in bits 16 and up
    /// and its next slot below.
    pub fn ring_doorbell(&self, value: u32) {
        fence(Ordering::SeqCst);
        self.regs.write32(HBUS_WRITE_POINTER, value);
    }

    fn tx_queue_size(&self) -> usize {
        match self.adapter.family {
            Family::Ax200 => TX_DESCRIPTORS,
            Family::Ax210 => GEN3_TX_QUEUE_SIZE,
        }
    }

    /// For the AX210 family after alive: tells the firmware to go on without a platform NVM and
    /// waits for it to say so.
    pub fn finish_platform_nvm(&mut self, wait: &mut dyn Wait) -> Result<(), TransportError> {
        if self.adapter.family != Family::Ax210 {
            return Ok(());
        }
        self.lock_nic(wait)?;
        self.write_prph(UREG_DOORBELL_TO_ISR6 + UMAC_PRPH_OFFSET, DOORBELL_PNVM);
        self.unlock_nic();
        self.wait_notification(REGULATORY_AND_NVM_GROUP, PNVM_INIT_COMPLETE, wait)
            .map(|_| ())
    }

    /// The adapter's MAC address: the one the maker strapped, else the one in its OTP memory.
    pub fn mac_address(&mut self, wait: &mut dyn Wait) -> Result<[u8; 6], TransportError> {
        self.lock_nic(wait)?;
        let read = |low: usize| flip_address(self.regs.read32(low), self.regs.read32(low + 4));
        let strapped = read(MAC_ADDRESS_STRAP);
        let address = match is_valid_address(&strapped) {
            true => strapped,
            false => read(MAC_ADDRESS_OTP),
        };
        self.unlock_nic();
        match is_valid_address(&address) {
            true => Ok(address),
            false => Err(TransportError::NoAddress),
        }
    }

    /// The block of DMA memory holding the rings and the context info.
    pub fn rings(&self) -> &M {
        &self.rings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dma::fake::HostDma;
    use crate::wifi::iwx::devices::identify;
    use core::cell::RefCell;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Registers(RefCell<BTreeMap<usize, u32>>);

    impl RegisterIo for Registers {
        fn read8(&self, offset: usize) -> u8 {
            self.read32(offset) as u8
        }
        fn write8(&self, offset: usize, value: u8) {
            self.write32(offset, u32::from(value));
        }
        fn read16(&self, offset: usize) -> u16 {
            self.read32(offset) as u16
        }
        fn write16(&self, offset: usize, value: u16) {
            self.write32(offset, u32::from(value));
        }
        fn read32(&self, offset: usize) -> u32 {
            let value = self.0.borrow().get(&offset).copied().unwrap_or(0);
            match offset {
                CSR_GP_CNTRL => value | GP_MAC_CLOCK_READY | GP_RADIO_KILL_SWITCH,
                CSR_HW_IF_CONFIG => value | HW_IF_NIC_READY,
                _ => value,
            }
        }
        fn write32(&self, offset: usize, value: u32) {
            self.0.borrow_mut().insert(offset, value);
        }
    }

    struct NoWait;

    impl Wait for NoWait {
        fn pause(&mut self, _microseconds: u32) {}
        fn keep_waiting(&mut self) -> bool {
            false
        }
    }

    #[test]
    fn the_hardware_starts_and_the_nic_lock_nests() {
        let adapter = identify(0x2723, 0, 0).unwrap();
        let mut transport = Transport::new(
            Registers::default(),
            adapter,
            HostDma::zeroed(RING_BYTES / 4096),
            HostDma::zeroed(RX_BUFFER_AREA_BYTES / 4096),
        );
        transport.start_hardware(&mut NoWait).unwrap();
        transport.lock_nic(&mut NoWait).unwrap();
        transport.lock_nic(&mut NoWait).unwrap();
        transport.unlock_nic();
        assert_ne!(
            transport.regs.read32(CSR_GP_CNTRL) & GP_MAC_ACCESS_REQUEST,
            0
        );
        transport.unlock_nic();
        assert_eq!(
            transport.regs.0.borrow()[&CSR_GP_CNTRL] & GP_MAC_ACCESS_REQUEST,
            0
        );
    }

    #[test]
    fn the_context_info_names_the_rings_and_each_section() {
        let adapter = identify(0x2723, 0, 0).unwrap();
        let transport = Transport::new(
            Registers::default(),
            adapter,
            HostDma::zeroed(RING_BYTES / 4096),
            HostDma::zeroed(RX_BUFFER_AREA_BYTES / 4096),
        );
        transport.write_context_info(1, 1);
        let rings = transport.rings();
        assert_eq!(
            get32(rings, CONTEXT_INFO + CONTEXT_CONTROL_FLAGS),
            (1 << 8) | (9 << 4) | (4 << 9)
        );
        assert_eq!(get16(rings, CONTEXT_INFO + 4), 448);
        let free = u64::from(get32(rings, CONTEXT_INFO + CONTEXT_FREE_RBD))
            | (u64::from(get32(rings, CONTEXT_INFO + CONTEXT_FREE_RBD + 4)) << 32);
        assert_eq!(free, dma(rings, RX_FREE));
        transport.fill_rx_descriptors();
        let second =
            u64::from(get32(rings, RX_FREE + 8)) | (u64::from(get32(rings, RX_FREE + 12)) << 32);
        assert_eq!(second, dma(&transport.rx_buffers, RX_BUFFER_BYTES) | 1);
    }

    #[test]
    fn an_alive_notification_with_a_bad_status_is_a_failure() {
        let adapter = identify(0x2725, 0x0020, 0).unwrap();
        let mut transport = Transport::new(
            Registers::default(),
            adapter,
            HostDma::zeroed(RING_BYTES / 4096),
            HostDma::zeroed(RX_BUFFER_AREA_BYTES / 4096),
        );
        transport.regs.write32(CSR_INT, INT_ALIVE);
        put32(&transport.rx_buffers, 0, 8 + 4);
        put_bytes(&transport.rx_buffers, 4, &[ALIVE, 0, 0, UNSOLICITED]);
        put16(&transport.rx_buffers, 8, 0xdead);
        put16(&transport.rings, RX_STATUS, 1);
        assert_eq!(
            transport.wait_for_alive(&mut NoWait),
            Err(TransportError::FirmwareFailed(0xdead))
        );
    }

    #[test]
    fn several_packets_in_a_buffer_are_read_and_addresses_flipped() {
        let adapter = identify(0x2723, 0, 0).unwrap();
        let mut transport = Transport::new(
            Registers::default(),
            adapter,
            HostDma::zeroed(RING_BYTES / 4096),
            HostDma::zeroed(RX_BUFFER_AREA_BYTES / 4096),
        );
        put32(&transport.rx_buffers, 0, 4 + 2);
        put_bytes(&transport.rx_buffers, 4, &[0x04, 0, 0, UNSOLICITED]);
        put_bytes(&transport.rx_buffers, 8, &[7, 7]);
        put32(&transport.rx_buffers, 0x40, 4 + 4);
        put_bytes(
            &transport.rx_buffers,
            0x44,
            &[0x03, SYSTEM_GROUP, 5, COMMAND_QUEUE],
        );
        put16(&transport.rings, RX_STATUS, 1);
        let notifications = transport.take_notifications();
        assert_eq!(notifications.len(), 1);
        assert_eq!(
            (notifications[0].code, &notifications[0].payload[..]),
            (0x04, &[7u8, 7][..])
        );
        assert_eq!(transport.responses.len(), 1);
        assert_eq!(transport.responses[0].0, 5);
        assert_eq!(transport.regs.read32(RX_FREE_WRITE_INDEX), 0);
        assert_eq!(
            flip_address(0x0011_2233, 0x0000_4455),
            [0x00, 0x11, 0x22, 0x33, 0x44, 0x55]
        );
        assert!(!is_valid_address(&[0x01, 0, 0, 0, 0, 1]));
        assert!(is_valid_address(&[0x00, 0x11, 0x22, 0x33, 0x44, 0x55]));
    }
}
