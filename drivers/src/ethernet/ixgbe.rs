//! Intel's 10 Gb/s adapters (ixgbe): the 82599, X540 and X550, with advanced descriptors on
//! queue 0 (Intel's 82599 10 GbE Controller, X540 and X550 datasheets; D615).

use super::{
    BUFFER_BYTES, Error, RECEIVE_BUFFERS, RECEIVE_RING, RING_ENTRIES, TRANSMIT_BUFFERS,
    TRANSMIT_RING, buffer_address, is_valid_mac, put_frame, take_frame,
};
use crate::dma::DmaMemory;
use crate::io::RegisterIo;
use crate::storage::{read32, write32, write64, zero};
use core::sync::atomic::{Ordering, fence};

const CTRL: usize = 0x0_0000;
const EIMC: usize = 0x0_0888;
const EEC: usize = 0x1_0010;
const RDRXCTL: usize = 0x0_2f00;
const RXCTRL: usize = 0x0_3000;
const HLREG0: usize = 0x0_4240;
const LINKS: usize = 0x0_42a4;
const DMATXCTL: usize = 0x0_4a80;
const FCTRL: usize = 0x0_5080;
const MTA: usize = 0x0_5200;
const RAL: usize = 0x0_a200;
const RAH: usize = 0x0_a204;
const RDBAL: usize = 0x0_1000;
const RDBAH: usize = 0x0_1004;
const RDLEN: usize = 0x0_1008;
const RDH: usize = 0x0_1010;
const RDT: usize = 0x0_1018;
const RXDCTL: usize = 0x0_1028;
const SRRCTL: usize = 0x0_2100;
const TDBAL: usize = 0x0_6000;
const TDBAH: usize = 0x0_6004;
const TDLEN: usize = 0x0_6008;
const TDH: usize = 0x0_6010;
const TDT: usize = 0x0_6018;
const TXDCTL: usize = 0x0_6028;

const CTRL_RESET: u32 = (1 << 26) | (1 << 3);
const EEC_AUTO_READ_DONE: u32 = 1 << 9;
const RDRXCTL_DMA_INIT_DONE: u32 = 1 << 3;
const RDRXCTL_STRIP_CRC: u32 = 1 << 1;
const RXCTRL_ENABLE: u32 = 1;
const HLREG0_INSERT_CRC: u32 = 1;
const HLREG0_STRIP_CRC: u32 = 1 << 1;
const HLREG0_PAD_SHORT: u32 = 1 << 10;
const LINKS_UP: u32 = 1 << 30;
const DMATXCTL_ENABLE: u32 = 1;
const FCTRL_BROADCAST: u32 = 1 << 10;
const SRRCTL_2_KIB_ONE_BUFFER: u32 = 2 | (1 << 25) | (1 << 28);
const QUEUE_ENABLE: u32 = 1 << 25;
const RAH_VALID: u32 = 1 << 31;
const DESCRIPTOR_DONE: u32 = 1;
const END_OF_PACKET: u32 = 1 << 1;
const RECEIVE_ERRORS: u32 = 0xfff << 20;
const DATA_DESCRIPTOR: u32 = 0b0011 << 20;
const COMMAND_END_OF_PACKET: u32 = 1 << 24;
const COMMAND_INSERT_CRC: u32 = 1 << 25;
const COMMAND_REPORT_STATUS: u32 = 1 << 27;
const COMMAND_EXTENDED: u32 = 1 << 29;
const PAYLOAD_SHIFT: u32 = 14;
const DESCRIPTOR_BYTES: usize = 16;
const MULTICAST_TABLE_WORDS: usize = 128;

/// Whether the Intel device id is an 82599, X540 or X550 adapter.
pub fn is_ixgbe(device: u16) -> bool {
    matches!(
        device,
        0x10f7
            | 0x10f8
            | 0x10f9
            | 0x10fb
            | 0x10fc
            | 0x1507
            | 0x1514
            | 0x1517
            | 0x151c
            | 0x1529
            | 0x152a
            | 0x154d
            | 0x1557
            | 0x1558
            | 0x154a
            | 0x10f1
            | 0x1528
            | 0x1560
            | 0x1563
            | 0x15d1
            | 0x15aa
            | 0x15ab
            | 0x15ac
            | 0x15ad
            | 0x15ae
            | 0x15c2
            | 0x15c3
            | 0x15c4
            | 0x15c6
            | 0x15c7
            | 0x15c8
            | 0x15ce
            | 0x15e4
            | 0x15e5
    )
}

/// An 82599, X540 or X550 adapter.
#[derive(Debug)]
pub struct Ixgbe<R: RegisterIo, M: DmaMemory> {
    regs: R,
    memory: M,
    mac: [u8; 6],
    receive_next: usize,
    transmit_next: usize,
    transmit_used: [bool; RING_ENTRIES],
}

fn wait_until<R: RegisterIo>(
    regs: &R,
    register: usize,
    bits: u32,
    set: bool,
    keep_waiting: &mut dyn FnMut() -> bool,
) -> bool {
    loop {
        if (regs.read32(register) & bits == bits) == set {
            return true;
        }
        if !keep_waiting() {
            return false;
        }
        core::hint::spin_loop();
    }
}

impl<R: RegisterIo, M: DmaMemory> Ixgbe<R, M> {
    /// Resets the adapter at `regs`, waits for its EEPROM and DMA set-up, and starts queue 0
    /// with its rings in `memory`.
    pub fn new(
        regs: R,
        memory: M,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<Self, (Error, M)> {
        if memory.len() < super::MEMORY_BYTES {
            return Err((
                Error::Unsupported("too little DMA memory for a network adapter"),
                memory,
            ));
        }
        regs.write32(EIMC, u32::MAX);
        let control = regs.read32(CTRL);
        regs.write32(CTRL, control | CTRL_RESET);
        if !wait_until(&regs, CTRL, CTRL_RESET, false, keep_waiting) {
            return Err((Error::TimedOut, memory));
        }
        regs.write32(EIMC, u32::MAX);
        let ready = wait_until(&regs, EEC, EEC_AUTO_READ_DONE, true, keep_waiting)
            && wait_until(&regs, RDRXCTL, RDRXCTL_DMA_INIT_DONE, true, keep_waiting);
        if !ready {
            return Err((Error::TimedOut, memory));
        }
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
        self.regs.write32(
            RAH,
            u32::from(u16::from_le_bytes([self.mac[4], self.mac[5]])) | RAH_VALID,
        );
        for word in 0..MULTICAST_TABLE_WORDS {
            self.regs.write32(MTA + word * 4, 0);
        }
        let link_layer = self.regs.read32(HLREG0);
        self.regs.write32(
            HLREG0,
            link_layer | HLREG0_INSERT_CRC | HLREG0_STRIP_CRC | HLREG0_PAD_SHORT,
        );
        let dma = self.regs.read32(RDRXCTL);
        self.regs.write32(RDRXCTL, dma | RDRXCTL_STRIP_CRC);
        zero(
            &self.memory,
            RECEIVE_RING,
            TRANSMIT_RING + RING_ENTRIES * DESCRIPTOR_BYTES - RECEIVE_RING,
        );
        for index in 0..RING_ENTRIES {
            self.give_receive_buffer(index);
        }
        let base = self.memory.dma_addr().get();
        let ring_bytes = (RING_ENTRIES * DESCRIPTOR_BYTES) as u32;
        let receive = base + RECEIVE_RING as u64;
        self.regs.write32(RXCTRL, 0);
        self.regs.write32(RDBAL, receive as u32);
        self.regs.write32(RDBAH, (receive >> 32) as u32);
        self.regs.write32(RDLEN, ring_bytes);
        self.regs.write32(SRRCTL, SRRCTL_2_KIB_ONE_BUFFER);
        self.regs.write32(RDH, 0);
        self.regs.write32(RDT, 0);
        self.regs.write32(RXDCTL, QUEUE_ENABLE);
        wait_until(&self.regs, RXDCTL, QUEUE_ENABLE, true, keep_waiting);
        self.regs.write32(RDT, (RING_ENTRIES - 1) as u32);
        let filter = self.regs.read32(FCTRL);
        self.regs.write32(FCTRL, filter | FCTRL_BROADCAST);
        self.regs.write32(RXCTRL, RXCTRL_ENABLE);
        let transmit = base + TRANSMIT_RING as u64;
        self.regs.write32(TDBAL, transmit as u32);
        self.regs.write32(TDBAH, (transmit >> 32) as u32);
        self.regs.write32(TDLEN, ring_bytes);
        self.regs.write32(TDH, 0);
        self.regs.write32(TDT, 0);
        self.regs.write32(DMATXCTL, DMATXCTL_ENABLE);
        self.regs.write32(TXDCTL, QUEUE_ENABLE);
        wait_until(&self.regs, TXDCTL, QUEUE_ENABLE, true, keep_waiting);
    }

    fn give_receive_buffer(&self, index: usize) {
        let descriptor = RECEIVE_RING + index * DESCRIPTOR_BYTES;
        write64(
            &self.memory,
            descriptor,
            buffer_address(&self.memory, RECEIVE_BUFFERS, index),
        );
        write64(&self.memory, descriptor + 8, 0);
    }

    pub fn mac(&self) -> [u8; 6] {
        self.mac
    }

    pub fn is_link_up(&self) -> bool {
        self.regs.read32(LINKS) & LINKS_UP != 0
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
            let taken = complete
                .then(|| take_frame(&self.memory, index, length.min(BUFFER_BYTES), frame))
                .flatten();
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
        if self.transmit_used[index] && read32(&self.memory, descriptor + 12) & DESCRIPTOR_DONE == 0
        {
            return Err(Error::QueueFull);
        }
        put_frame(&self.memory, index, frame)?;
        let length = frame.len() as u32;
        write64(
            &self.memory,
            descriptor,
            buffer_address(&self.memory, TRANSMIT_BUFFERS, index),
        );
        write32(
            &self.memory,
            descriptor + 8,
            length
                | DATA_DESCRIPTOR
                | COMMAND_END_OF_PACKET
                | COMMAND_INSERT_CRC
                | COMMAND_REPORT_STATUS
                | COMMAND_EXTENDED,
        );
        write32(&self.memory, descriptor + 12, length << PAYLOAD_SHIFT);
        self.transmit_used[index] = true;
        self.transmit_next = (index + 1) % RING_ENTRIES;
        fence(Ordering::SeqCst);
        self.regs.write32(TDT, self.transmit_next as u32);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_82599_x540_and_x550_ids_are_taken() {
        assert!(is_ixgbe(0x10fb));
        assert!(is_ixgbe(0x1528));
        assert!(is_ixgbe(0x1563));
        assert!(!is_ixgbe(0x1533));
    }
}
