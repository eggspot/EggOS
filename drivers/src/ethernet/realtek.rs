//! Realtek's RTL8111/8168 (gigabit) and RTL8125 (2.5 gigabit) adapters, with their descriptor
//! rings of `OWN`-flagged descriptors (D240).

use super::{
    BUFFER_BYTES, Error, RECEIVE_BUFFERS, RECEIVE_RING, RING_ENTRIES, TRANSMIT_BUFFERS,
    TRANSMIT_RING, buffer_address, is_valid_mac, put_frame, take_frame,
};
use crate::dma::DmaMemory;
use crate::io::RegisterIo;
use crate::storage::{read32, write32, write64, zero};
use core::sync::atomic::{Ordering, fence};

const ADDRESS: usize = 0x00;
const MULTICAST: usize = 0x08;
const TRANSMIT_DESCRIPTORS: usize = 0x20;
const COMMAND: usize = 0x37;
const TRANSMIT_POLL_8168: usize = 0x38;
const INTERRUPT_MASK_8168: usize = 0x3c;
const INTERRUPT_STATUS_8168: usize = 0x3e;
const INTERRUPT_MASK_8125: usize = 0x38;
const INTERRUPT_STATUS_8125: usize = 0x3c;
const TRANSMIT_POLL_8125: usize = 0x90;
const TRANSMIT_CONFIGURATION: usize = 0x40;
const RECEIVE_CONFIGURATION: usize = 0x44;
const CONFIGURATION_LOCK: usize = 0x50;
const PHY_STATUS: usize = 0x6c;
const RECEIVE_MAX_SIZE: usize = 0xda;
const C_PLUS_COMMAND: usize = 0xe0;
const RECEIVE_DESCRIPTORS: usize = 0xe4;
const TRANSMIT_MAX_SIZE: usize = 0xec;

const COMMAND_RESET: u8 = 1 << 4;
const COMMAND_RECEIVE: u8 = 1 << 3;
const COMMAND_TRANSMIT: u8 = 1 << 2;
const POLL_NORMAL_QUEUE_8168: u8 = 1 << 6;
const POLL_NORMAL_QUEUE_8125: u16 = 1;
const UNLOCK: u8 = 0xc0;
const LOCK: u8 = 0x00;
const ACCEPT_OWN: u32 = 1 << 1;
const ACCEPT_MULTICAST: u32 = 1 << 2;
const ACCEPT_BROADCAST: u32 = 1 << 3;
const RECEIVE_NO_THRESHOLD_UNLIMITED_DMA: u32 = (0b111 << 13) | (0b111 << 8);
const RECEIVE_FETCH_8125: u32 = 0b1 << 27;
const TRANSMIT_DEFAULT: u32 = (0b11 << 24) | (0b111 << 8);
const C_PLUS_DEFAULT: u16 = (1 << 5) | (1 << 3);
const LINK_UP: u8 = 1 << 1;

const OWN: u32 = 1 << 31;
const END_OF_RING: u32 = 1 << 30;
const FIRST_SEGMENT: u32 = 1 << 29;
const LAST_SEGMENT: u32 = 1 << 28;
const RECEIVE_ERROR: u32 = 1 << 21;
const LENGTH_MASK: u32 = 0x3fff;
const CRC_BYTES: usize = 4;
const DESCRIPTOR_BYTES: usize = 16;
const MAX_FRAME_WITH_CRC: u16 = 1522;

/// Which register layout the chip has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Generation {
    /// RTL8111/8168 (and the RTL8101/8102 fast Ethernet chips of the same family).
    Rtl8168,
    /// RTL8125.
    Rtl8125,
}

/// The register layout of the Realtek device id, if this driver takes it.
pub fn generation_of(device: u16) -> Option<Generation> {
    match device {
        0x8161 | 0x8168 | 0x8136 | 0x2502 | 0x2600 => Some(Generation::Rtl8168),
        0x8125 | 0x3000 | 0x8126 => Some(Generation::Rtl8125),
        _ => None,
    }
}

/// A Realtek adapter.
#[derive(Debug)]
pub struct Realtek<R: RegisterIo, M: DmaMemory> {
    regs: R,
    memory: M,
    generation: Generation,
    mac: [u8; 6],
    receive_next: usize,
    transmit_next: usize,
}

fn end_of_ring(index: usize) -> u32 {
    if index == RING_ENTRIES - 1 {
        END_OF_RING
    } else {
        0
    }
}

impl<R: RegisterIo, M: DmaMemory> Realtek<R, M> {
    /// Resets the chip at `regs` and starts it with its rings in `memory`.
    pub fn new(
        regs: R,
        memory: M,
        generation: Generation,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<Self, (Error, M)> {
        if memory.len() < super::MEMORY_BYTES {
            return Err((
                Error::Unsupported("too little DMA memory for a network adapter"),
                memory,
            ));
        }
        let mut mac = [0u8; 6];
        for (index, byte) in mac.iter_mut().enumerate() {
            *byte = regs.read8(ADDRESS + index);
        }
        if !is_valid_mac(mac) {
            return Err((Error::NoAddress, memory));
        }
        let (mask, status) = match generation {
            Generation::Rtl8168 => (INTERRUPT_MASK_8168, INTERRUPT_STATUS_8168),
            Generation::Rtl8125 => (INTERRUPT_MASK_8125, INTERRUPT_STATUS_8125),
        };
        match generation {
            Generation::Rtl8168 => {
                regs.write16(mask, 0);
                regs.write16(status, u16::MAX);
            }
            Generation::Rtl8125 => {
                regs.write32(mask, 0);
                regs.write32(status, u32::MAX);
            }
        }
        regs.write8(COMMAND, COMMAND_RESET);
        loop {
            if regs.read8(COMMAND) & COMMAND_RESET == 0 {
                break;
            }
            if !keep_waiting() {
                return Err((Error::TimedOut, memory));
            }
            core::hint::spin_loop();
        }
        let mut adapter = Self {
            regs,
            memory,
            generation,
            mac,
            receive_next: 0,
            transmit_next: 0,
        };
        adapter.start();
        Ok(adapter)
    }

    fn start(&mut self) {
        zero(
            &self.memory,
            RECEIVE_RING,
            TRANSMIT_RING + RING_ENTRIES * DESCRIPTOR_BYTES - RECEIVE_RING,
        );
        for index in 0..RING_ENTRIES {
            self.give_receive_buffer(index);
        }
        let base = self.memory.dma_addr().get();
        self.regs.write8(CONFIGURATION_LOCK, UNLOCK);
        self.regs.write16(C_PLUS_COMMAND, C_PLUS_DEFAULT);
        self.regs.write16(RECEIVE_MAX_SIZE, MAX_FRAME_WITH_CRC);
        self.regs
            .write8(TRANSMIT_MAX_SIZE, (BUFFER_BYTES / 128) as u8);
        let transmit = base + TRANSMIT_RING as u64;
        let receive = base + RECEIVE_RING as u64;
        self.regs.write32(TRANSMIT_DESCRIPTORS, transmit as u32);
        self.regs
            .write32(TRANSMIT_DESCRIPTORS + 4, (transmit >> 32) as u32);
        self.regs.write32(RECEIVE_DESCRIPTORS, receive as u32);
        self.regs
            .write32(RECEIVE_DESCRIPTORS + 4, (receive >> 32) as u32);
        self.regs.write32(MULTICAST, u32::MAX);
        self.regs.write32(MULTICAST + 4, u32::MAX);
        self.regs
            .write8(COMMAND, COMMAND_RECEIVE | COMMAND_TRANSMIT);
        let fetch = match self.generation {
            Generation::Rtl8168 => 0,
            Generation::Rtl8125 => RECEIVE_FETCH_8125,
        };
        self.regs.write32(
            RECEIVE_CONFIGURATION,
            ACCEPT_OWN
                | ACCEPT_MULTICAST
                | ACCEPT_BROADCAST
                | RECEIVE_NO_THRESHOLD_UNLIMITED_DMA
                | fetch,
        );
        self.regs.write32(TRANSMIT_CONFIGURATION, TRANSMIT_DEFAULT);
        self.regs.write8(CONFIGURATION_LOCK, LOCK);
    }

    fn give_receive_buffer(&self, index: usize) {
        let descriptor = RECEIVE_RING + index * DESCRIPTOR_BYTES;
        write64(
            &self.memory,
            descriptor + 8,
            buffer_address(&self.memory, RECEIVE_BUFFERS, index),
        );
        write32(&self.memory, descriptor + 4, 0);
        fence(Ordering::SeqCst);
        write32(
            &self.memory,
            descriptor,
            OWN | end_of_ring(index) | BUFFER_BYTES as u32,
        );
    }

    pub fn mac(&self) -> [u8; 6] {
        self.mac
    }

    pub fn is_link_up(&self) -> bool {
        self.regs.read8(PHY_STATUS) & LINK_UP != 0
    }

    pub fn receive(&mut self, frame: &mut [u8]) -> Option<usize> {
        loop {
            let index = self.receive_next;
            let descriptor = RECEIVE_RING + index * DESCRIPTOR_BYTES;
            let status = read32(&self.memory, descriptor);
            if status & OWN != 0 {
                return None;
            }
            fence(Ordering::SeqCst);
            let whole = status & (FIRST_SEGMENT | LAST_SEGMENT) == FIRST_SEGMENT | LAST_SEGMENT;
            let length = ((status & LENGTH_MASK) as usize).saturating_sub(CRC_BYTES);
            let taken = (whole && status & RECEIVE_ERROR == 0)
                .then(|| take_frame(&self.memory, index, length.min(BUFFER_BYTES), frame))
                .flatten();
            self.give_receive_buffer(index);
            self.receive_next = (index + 1) % RING_ENTRIES;
            if taken.is_some() {
                return taken;
            }
        }
    }

    pub fn transmit(&mut self, frame: &[u8]) -> Result<(), Error> {
        let index = self.transmit_next;
        let descriptor = TRANSMIT_RING + index * DESCRIPTOR_BYTES;
        if read32(&self.memory, descriptor) & OWN != 0 {
            return Err(Error::QueueFull);
        }
        put_frame(&self.memory, index, frame)?;
        let length = frame.len().max(60) as u32;
        write64(
            &self.memory,
            descriptor + 8,
            buffer_address(&self.memory, TRANSMIT_BUFFERS, index),
        );
        write32(&self.memory, descriptor + 4, 0);
        fence(Ordering::SeqCst);
        write32(
            &self.memory,
            descriptor,
            OWN | FIRST_SEGMENT | LAST_SEGMENT | end_of_ring(index) | length,
        );
        self.transmit_next = (index + 1) % RING_ENTRIES;
        fence(Ordering::SeqCst);
        match self.generation {
            Generation::Rtl8168 => self.regs.write8(TRANSMIT_POLL_8168, POLL_NORMAL_QUEUE_8168),
            Generation::Rtl8125 => self
                .regs
                .write16(TRANSMIT_POLL_8125, POLL_NORMAL_QUEUE_8125),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_chips_have_their_register_layouts() {
        assert_eq!(generation_of(0x8168), Some(Generation::Rtl8168));
        assert_eq!(generation_of(0x8125), Some(Generation::Rtl8125));
        assert_eq!(generation_of(0x8139), None);
    }

    #[test]
    fn only_the_last_descriptor_ends_the_ring() {
        assert_eq!(end_of_ring(0), 0);
        assert_eq!(end_of_ring(RING_ENTRIES - 1), END_OF_RING);
    }
}
