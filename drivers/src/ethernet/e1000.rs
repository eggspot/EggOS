//! Intel's 8254x (e1000) and 82574, 82583 and I217-I219 (e1000e) adapters, with legacy
//! descriptors (Intel's 8254x and 82574 software developer's manuals; D240).

use super::{
    BUFFER_BYTES, Error, RECEIVE_BUFFERS, RECEIVE_RING, RING_ENTRIES, TRANSMIT_BUFFERS, TRANSMIT_RING,
    buffer_address, is_valid_mac, put_frame, take_frame,
};
use crate::dma::DmaMemory;
use crate::io::RegisterIo;
use crate::storage::{read32, write32, write64, zero};
use core::sync::atomic::{Ordering, fence};

const CTRL: usize = 0x0000;
const STATUS: usize = 0x0008;
const EERD: usize = 0x0014;
const ICR: usize = 0x00c0;
const IMS: usize = 0x00d0;
const INTERRUPT_LINK_CHANGE: u32 = 1 << 2;
const INTERRUPT_RECEIVE_THRESHOLD: u32 = 1 << 4;
const INTERRUPT_RECEIVE_TIMER: u32 = 1 << 7;
const IMC: usize = 0x00d8;
const RCTL: usize = 0x0100;
const TCTL: usize = 0x0400;
const TIPG: usize = 0x0410;
const RDBAL: usize = 0x2800;
const RDBAH: usize = 0x2804;
const RDLEN: usize = 0x2808;
const RDH: usize = 0x2810;
const RDT: usize = 0x2818;
const TDBAL: usize = 0x3800;
const TDBAH: usize = 0x3804;
const TDLEN: usize = 0x3808;
const TDH: usize = 0x3810;
const TDT: usize = 0x3818;
const MTA: usize = 0x5200;
const RAL: usize = 0x5400;
const RAH: usize = 0x5404;

const CTRL_AUTO_SPEED: u32 = 1 << 5;
const CTRL_SET_LINK_UP: u32 = 1 << 6;
const CTRL_RESET: u32 = 1 << 26;
const STATUS_LINK_UP: u32 = 1 << 1;
const RCTL_ENABLE: u32 = 1 << 1;
const RCTL_BROADCAST: u32 = 1 << 15;
const RCTL_STRIP_CRC: u32 = 1 << 26;
const TCTL_ENABLE: u32 = 1 << 1;
const TCTL_PAD_SHORT: u32 = 1 << 3;
const TCTL_COLLISION_THRESHOLD: u32 = 0x0f << 4;
const TCTL_COLLISION_DISTANCE: u32 = 0x40 << 12;
const TIPG_DEFAULT: u32 = 0x0060_200a;
const RAH_VALID: u32 = 1 << 31;
const DESCRIPTOR_DONE: u8 = 1;
const END_OF_PACKET: u8 = 1 << 1;
const COMMAND_END_OF_PACKET: u32 = 1 << 24;
const COMMAND_INSERT_CRC: u32 = 1 << 25;
const COMMAND_REPORT_STATUS: u32 = 1 << 27;
const DESCRIPTOR_BYTES: usize = 16;
const MULTICAST_TABLE_WORDS: usize = 128;

/// How the adapter reads its EEPROM through `EERD`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// The 8254x: the word address at bit 8, done at bit 4.
    Classic,
    /// The 82574 and later: the word address at bit 2, done at bit 1.
    Express,
}

/// Whether the Intel device id is an e1000 (8254x) or an e1000e adapter this driver takes.
pub fn family_of(device: u16) -> Option<Family> {
    match device {
        0x100e | 0x100f | 0x1010 | 0x1011 | 0x1012 | 0x1013 | 0x1015 | 0x1016 | 0x1017 | 0x101d
        | 0x1026 | 0x1027 | 0x1028 | 0x1075 | 0x1076 | 0x1077 | 0x1078 | 0x1079 | 0x107a
        | 0x107b | 0x107c | 0x108a | 0x1099 | 0x10b5 => Some(Family::Classic),
        0x10d3 | 0x10f6 | 0x150c | 0x153a | 0x153b | 0x1559 | 0x155a | 0x156f | 0x1570
        | 0x15a0..=0x15a3 | 0x15b7..=0x15be | 0x15d6..=0x15d8 | 0x15df..=0x15e3
        | 0x15f4..=0x15fc | 0x0d4c..=0x0d4f | 0x0d53 | 0x0d55 | 0x0dc5..=0x0dc8
        | 0x1a1c..=0x1a1f | 0x550a..=0x5511 | 0x57a0 | 0x57a1 | 0x57b3..=0x57ba => {
            Some(Family::Express)
        }
        _ => None,
    }
}

/// An e1000 or e1000e adapter.
#[derive(Debug)]
pub struct E1000<R: RegisterIo, M: DmaMemory> {
    regs: R,
    memory: M,
    mac: [u8; 6],
    receive_next: usize,
    transmit_next: usize,
    transmit_used: [bool; RING_ENTRIES],
}

impl<R: RegisterIo, M: DmaMemory> E1000<R, M> {
    /// Resets the adapter at `regs` and starts it with its rings in `memory`
    /// ([`super::MEMORY_BYTES`]). On failure `memory` is handed back.
    pub fn new(regs: R, memory: M, family: Family, keep_waiting: &mut dyn FnMut() -> bool) -> Result<Self, (Error, M)> {
        if memory.len() < super::MEMORY_BYTES {
            return Err((Error::Unsupported("too little DMA memory for a network adapter"), memory));
        }
        regs.write32(IMC, u32::MAX);
        let control = regs.read32(CTRL);
        regs.write32(CTRL, control | CTRL_RESET);
        loop {
            if regs.read32(CTRL) & CTRL_RESET == 0 {
                break;
            }
            if !keep_waiting() {
                return Err((Error::TimedOut, memory));
            }
            core::hint::spin_loop();
        }
        regs.write32(IMC, u32::MAX);
        regs.read32(ICR);
        let mac = read_mac(&regs, family, keep_waiting).ok_or(Error::NoAddress);
        let mac = match mac {
            Ok(mac) => mac,
            Err(error) => return Err((error, memory)),
        };
        let mut adapter = Self {
            regs,
            memory,
            mac,
            receive_next: 0,
            transmit_next: 0,
            transmit_used: [false; RING_ENTRIES],
        };
        adapter.start();
        Ok(adapter)
    }

    fn start(&mut self) {
        let low = u32::from_le_bytes([self.mac[0], self.mac[1], self.mac[2], self.mac[3]]);
        let high = u32::from(u16::from_le_bytes([self.mac[4], self.mac[5]])) | RAH_VALID;
        self.regs.write32(RAL, low);
        self.regs.write32(RAH, high);
        for word in 0..MULTICAST_TABLE_WORDS {
            self.regs.write32(MTA + word * 4, 0);
        }
        zero(&self.memory, RECEIVE_RING, TRANSMIT_RING + RING_ENTRIES * DESCRIPTOR_BYTES - RECEIVE_RING);
        for index in 0..RING_ENTRIES {
            let descriptor = RECEIVE_RING + index * DESCRIPTOR_BYTES;
            write64(&self.memory, descriptor, buffer_address(&self.memory, RECEIVE_BUFFERS, index));
        }
        let base = self.memory.dma_addr().get();
        let ring_bytes = (RING_ENTRIES * DESCRIPTOR_BYTES) as u32;
        let receive = base + RECEIVE_RING as u64;
        self.regs.write32(RDBAL, receive as u32);
        self.regs.write32(RDBAH, (receive >> 32) as u32);
        self.regs.write32(RDLEN, ring_bytes);
        self.regs.write32(RDH, 0);
        self.regs.write32(RDT, (RING_ENTRIES - 1) as u32);
        self.regs.write32(RCTL, RCTL_ENABLE | RCTL_BROADCAST | RCTL_STRIP_CRC);
        let transmit = base + TRANSMIT_RING as u64;
        self.regs.write32(TDBAL, transmit as u32);
        self.regs.write32(TDBAH, (transmit >> 32) as u32);
        self.regs.write32(TDLEN, ring_bytes);
        self.regs.write32(TDH, 0);
        self.regs.write32(TDT, 0);
        self.regs.write32(TCTL, TCTL_ENABLE | TCTL_PAD_SHORT | TCTL_COLLISION_THRESHOLD | TCTL_COLLISION_DISTANCE);
        self.regs.write32(TIPG, TIPG_DEFAULT);
        let control = self.regs.read32(CTRL);
        self.regs.write32(CTRL, control | CTRL_SET_LINK_UP | CTRL_AUTO_SPEED);
    }

    pub fn mac(&self) -> [u8; 6] {
        self.mac
    }

    pub fn is_link_up(&self) -> bool {
        self.regs.read32(STATUS) & STATUS_LINK_UP != 0
    }

    /// Asks the adapter to interrupt when frames arrive or the link changes (D255).
    pub fn enable_interrupts(&mut self) {
        self.regs.read32(ICR);
        self.regs.write32(
            IMS,
            INTERRUPT_RECEIVE_TIMER | INTERRUPT_RECEIVE_THRESHOLD | INTERRUPT_LINK_CHANGE,
        );
    }

    /// Copies the next frame received into `frame`: its length, or `None` when none came.
    pub fn receive(&mut self, frame: &mut [u8]) -> Option<usize> {
        self.regs.read32(ICR);
        loop {
            let index = self.receive_next;
            let descriptor = RECEIVE_RING + index * DESCRIPTOR_BYTES;
            let word = read32(&self.memory, descriptor + 12);
            let status = word as u8;
            if status & DESCRIPTOR_DONE == 0 {
                return None;
            }
            fence(Ordering::SeqCst);
            let length = (read32(&self.memory, descriptor + 8) & 0xffff) as usize;
            let complete = status & END_OF_PACKET != 0 && (word >> 8) as u8 == 0;
            let taken = complete.then(|| take_frame(&self.memory, index, length.min(BUFFER_BYTES), frame)).flatten();
            write32(&self.memory, descriptor + 12, 0);
            fence(Ordering::SeqCst);
            self.regs.write32(RDT, index as u32);
            self.receive_next = (index + 1) % RING_ENTRIES;
            if taken.is_some() {
                return taken;
            }
        }
    }

    /// Queues `frame` for sending.
    pub fn transmit(&mut self, frame: &[u8]) -> Result<(), Error> {
        let index = self.transmit_next;
        let descriptor = TRANSMIT_RING + index * DESCRIPTOR_BYTES;
        if self.transmit_used[index] && read32(&self.memory, descriptor + 12) as u8 & DESCRIPTOR_DONE == 0 {
            return Err(Error::QueueFull);
        }
        put_frame(&self.memory, index, frame)?;
        write64(&self.memory, descriptor, buffer_address(&self.memory, TRANSMIT_BUFFERS, index));
        write32(
            &self.memory,
            descriptor + 8,
            frame.len() as u32 | COMMAND_END_OF_PACKET | COMMAND_INSERT_CRC | COMMAND_REPORT_STATUS,
        );
        write32(&self.memory, descriptor + 12, 0);
        self.transmit_used[index] = true;
        self.transmit_next = (index + 1) % RING_ENTRIES;
        fence(Ordering::SeqCst);
        self.regs.write32(TDT, self.transmit_next as u32);
        Ok(())
    }
}

fn read_mac<R: RegisterIo>(regs: &R, family: Family, keep_waiting: &mut dyn FnMut() -> bool) -> Option<[u8; 6]> {
    let low = regs.read32(RAL).to_le_bytes();
    let high = regs.read32(RAH).to_le_bytes();
    let mac = [low[0], low[1], low[2], low[3], high[0], high[1]];
    if is_valid_mac(mac) {
        return Some(mac);
    }
    let (shift, done) = match family {
        Family::Classic => (8, 1 << 4),
        Family::Express => (2, 1 << 1),
    };
    let mut mac = [0u8; 6];
    for word in 0..3u32 {
        regs.write32(EERD, 1 | (word << shift));
        let value = loop {
            let value = regs.read32(EERD);
            if value & done != 0 {
                break value;
            }
            if !keep_waiting() {
                return None;
            }
            core::hint::spin_loop();
        };
        let bytes = ((value >> 16) as u16).to_le_bytes();
        mac[word as usize * 2] = bytes[0];
        mac[word as usize * 2 + 1] = bytes[1];
    }
    is_valid_mac(mac).then_some(mac)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::fake::FakeRegs;

    #[test]
    fn the_address_comes_from_the_receive_address_registers() {
        let regs = FakeRegs::default();
        regs.script_reads(RAL, &[0x1200_5452]);
        regs.script_reads(RAH, &[0x8000_5634]);
        assert_eq!(read_mac(&regs, Family::Classic, &mut || false), Some([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]));
    }

    #[test]
    fn the_known_adapters_have_their_family() {
        assert_eq!(family_of(0x100e), Some(Family::Classic));
        assert_eq!(family_of(0x10d3), Some(Family::Express));
        assert_eq!(family_of(0x15b8), Some(Family::Express));
        assert_eq!(family_of(0x1533), None);
    }
}
