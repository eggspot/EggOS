//! Intel's I225 and I226 adapters (igc), with advanced descriptors on queue 0 (Intel's
//! Ethernet Controller I225 datasheet; D240).

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
const IMC: usize = 0x150c;
const EIMC: usize = 0x1528;
const RCTL: usize = 0x0100;
const TCTL: usize = 0x0400;
const RDBAL: usize = 0xc000;
const RDBAH: usize = 0xc004;
const RDLEN: usize = 0xc008;
const SRRCTL: usize = 0xc00c;
const RDH: usize = 0xc010;
const RDT: usize = 0xc018;
const RXDCTL: usize = 0xc028;
const TDBAL: usize = 0xe000;
const TDBAH: usize = 0xe004;
const TDLEN: usize = 0xe008;
const TDH: usize = 0xe010;
const TDT: usize = 0xe018;
const TXDCTL: usize = 0xe028;
const MTA: usize = 0x5200;
const RAL: usize = 0x5400;
const RAH: usize = 0x5404;

const CTRL_SET_LINK_UP: u32 = 1 << 6;
const CTRL_RESET: u32 = 1 << 26;
const STATUS_LINK_UP: u32 = 1 << 1;
const RCTL_ENABLE: u32 = 1 << 1;
const RCTL_BROADCAST: u32 = 1 << 15;
const RCTL_STRIP_CRC: u32 = 1 << 26;
const TCTL_ENABLE: u32 = 1 << 1;
const TCTL_PAD_SHORT: u32 = 1 << 3;
const SRRCTL_2_KIB_ONE_BUFFER: u32 = 2 | (1 << 25);
const QUEUE_ENABLE: u32 = 1 << 25;
const RAH_VALID: u32 = 1 << 31;
const DESCRIPTOR_DONE: u32 = 1;
const END_OF_PACKET: u32 = 1 << 1;
const RECEIVE_ERRORS: u32 = 0xff << 24;
const DATA_DESCRIPTOR: u32 = 0b0011 << 20;
const COMMAND_END_OF_PACKET: u32 = 1 << 24;
const COMMAND_INSERT_CRC: u32 = 1 << 25;
const COMMAND_REPORT_STATUS: u32 = 1 << 27;
const COMMAND_EXTENDED: u32 = 1 << 29;
const PAYLOAD_SHIFT: u32 = 14;
const DESCRIPTOR_BYTES: usize = 16;
const MULTICAST_TABLE_WORDS: usize = 128;

/// Whether the Intel device id is an I225 or I226 adapter.
pub fn is_igc(device: u16) -> bool {
    matches!(
        device,
        0x15f2 | 0x15f3 | 0x15fd | 0x0d9f | 0x3100 | 0x3101 | 0x3102 | 0x5502 | 0x5503 | 0x125b
            | 0x125c | 0x125d | 0x125f
    )
}

/// An I225 or I226 adapter.
#[derive(Debug)]
pub struct Igc<R: RegisterIo, M: DmaMemory> {
    regs: R,
    memory: M,
    mac: [u8; 6],
    receive_next: usize,
    transmit_next: usize,
    transmit_used: [bool; RING_ENTRIES],
}

impl<R: RegisterIo, M: DmaMemory> Igc<R, M> {
    /// Resets the adapter at `regs` and starts queue 0 with its rings in `memory`.
    pub fn new(regs: R, memory: M, keep_waiting: &mut dyn FnMut() -> bool) -> Result<Self, (Error, M)> {
        if memory.len() < super::MEMORY_BYTES {
            return Err((Error::Unsupported("too little DMA memory for a network adapter"), memory));
        }
        regs.write32(IMC, u32::MAX);
        regs.write32(EIMC, u32::MAX);
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
        regs.write32(EIMC, u32::MAX);
        let low = regs.read32(RAL).to_le_bytes();
        let high = regs.read32(RAH).to_le_bytes();
        let mac = [low[0], low[1], low[2], low[3], high[0], high[1]];
        if !is_valid_mac(mac) {
            return Err((Error::NoAddress, memory));
        }
        let mut adapter = Self {
            regs,
            memory,
            mac,
            receive_next: 0,
            transmit_next: 0,
            transmit_used: [false; RING_ENTRIES],
        };
        adapter.start(keep_waiting);
        Ok(adapter)
    }

    fn start(&mut self, keep_waiting: &mut dyn FnMut() -> bool) {
        let low = u32::from_le_bytes([self.mac[0], self.mac[1], self.mac[2], self.mac[3]]);
        self.regs.write32(RAL, low);
        self.regs
            .write32(RAH, u32::from(u16::from_le_bytes([self.mac[4], self.mac[5]])) | RAH_VALID);
        for word in 0..MULTICAST_TABLE_WORDS {
            self.regs.write32(MTA + word * 4, 0);
        }
        zero(&self.memory, RECEIVE_RING, TRANSMIT_RING + RING_ENTRIES * DESCRIPTOR_BYTES - RECEIVE_RING);
        for index in 0..RING_ENTRIES {
            self.give_receive_buffer(index);
        }
        let base = self.memory.dma_addr().get();
        let ring_bytes = (RING_ENTRIES * DESCRIPTOR_BYTES) as u32;
        let receive = base + RECEIVE_RING as u64;
        self.regs.write32(RXDCTL, 0);
        self.regs.write32(RDBAL, receive as u32);
        self.regs.write32(RDBAH, (receive >> 32) as u32);
        self.regs.write32(RDLEN, ring_bytes);
        self.regs.write32(SRRCTL, SRRCTL_2_KIB_ONE_BUFFER);
        self.regs.write32(RDH, 0);
        self.regs.write32(RDT, 0);
        self.regs.write32(RXDCTL, QUEUE_ENABLE);
        wait_enabled(&self.regs, RXDCTL, keep_waiting);
        self.regs.write32(RDT, (RING_ENTRIES - 1) as u32);
        self.regs.write32(RCTL, RCTL_ENABLE | RCTL_BROADCAST | RCTL_STRIP_CRC);
        let transmit = base + TRANSMIT_RING as u64;
        self.regs.write32(TXDCTL, 0);
        self.regs.write32(TDBAL, transmit as u32);
        self.regs.write32(TDBAH, (transmit >> 32) as u32);
        self.regs.write32(TDLEN, ring_bytes);
        self.regs.write32(TDH, 0);
        self.regs.write32(TDT, 0);
        self.regs.write32(TXDCTL, QUEUE_ENABLE);
        wait_enabled(&self.regs, TXDCTL, keep_waiting);
        self.regs.write32(TCTL, TCTL_ENABLE | TCTL_PAD_SHORT);
        let control = self.regs.read32(CTRL);
        self.regs.write32(CTRL, control | CTRL_SET_LINK_UP);
    }

    fn give_receive_buffer(&self, index: usize) {
        let descriptor = RECEIVE_RING + index * DESCRIPTOR_BYTES;
        write64(&self.memory, descriptor, buffer_address(&self.memory, RECEIVE_BUFFERS, index));
        write64(&self.memory, descriptor + 8, 0);
    }

    pub fn mac(&self) -> [u8; 6] {
        self.mac
    }

    pub fn is_link_up(&self) -> bool {
        self.regs.read32(STATUS) & STATUS_LINK_UP != 0
    }

    pub fn receive(&mut self, frame: &mut [u8]) -> Option<usize> {
        loop {
            let index = self.receive_next;
            let descriptor = RECEIVE_RING + index * DESCRIPTOR_BYTES;
            let status = read32(&self.memory, descriptor + 8);
            if status & DESCRIPTOR_DONE == 0 {
                return None;
            }
            fence(Ordering::SeqCst);
            let length = (read32(&self.memory, descriptor + 12) & 0xffff) as usize;
            let complete = status & END_OF_PACKET != 0 && status & RECEIVE_ERRORS == 0;
            let taken = complete.then(|| take_frame(&self.memory, index, length.min(BUFFER_BYTES), frame)).flatten();
            self.give_receive_buffer(index);
            fence(Ordering::SeqCst);
            self.regs.write32(RDT, index as u32);
            self.receive_next = (index + 1) % RING_ENTRIES;
            if taken.is_some() {
                return taken;
            }
        }
    }

    pub fn transmit(&mut self, frame: &[u8]) -> Result<(), Error> {
        let index = self.transmit_next;
        let descriptor = TRANSMIT_RING + index * DESCRIPTOR_BYTES;
        if self.transmit_used[index] && read32(&self.memory, descriptor + 12) & DESCRIPTOR_DONE == 0 {
            return Err(Error::QueueFull);
        }
        put_frame(&self.memory, index, frame)?;
        let length = frame.len() as u32;
        write64(&self.memory, descriptor, buffer_address(&self.memory, TRANSMIT_BUFFERS, index));
        write32(
            &self.memory,
            descriptor + 8,
            length | DATA_DESCRIPTOR | COMMAND_END_OF_PACKET | COMMAND_INSERT_CRC | COMMAND_REPORT_STATUS | COMMAND_EXTENDED,
        );
        write32(&self.memory, descriptor + 12, length << PAYLOAD_SHIFT);
        self.transmit_used[index] = true;
        self.transmit_next = (index + 1) % RING_ENTRIES;
        fence(Ordering::SeqCst);
        self.regs.write32(TDT, self.transmit_next as u32);
        Ok(())
    }
}

fn wait_enabled<R: RegisterIo>(regs: &R, register: usize, keep_waiting: &mut dyn FnMut() -> bool) {
    while regs.read32(register) & QUEUE_ENABLE == 0 && keep_waiting() {
        core::hint::spin_loop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn i225_and_i226_ids_are_taken() {
        assert!(is_igc(0x15f3));
        assert!(is_igc(0x125c));
        assert!(!is_igc(0x15b8));
    }
}
