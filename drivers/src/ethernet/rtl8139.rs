//! Realtek's RTL8139 (and the 8100 and 8130 built on it): a ring receive buffer the adapter
//! writes frames into one after the other, each behind a 4-byte header, and four transmit
//! buffers it reads from (Realtek's RTL8139 datasheet; D615). It reaches only 32-bit addresses.

use super::{BUFFER_BYTES, Error, MAX_FRAME_BYTES, RECEIVE_BUFFERS, TRANSMIT_BUFFERS, is_valid_mac};
use crate::dma::DmaMemory;
use crate::io::RegisterIo;
use crate::storage::{copy_in, copy_out};
use core::sync::atomic::{Ordering, fence};

const IDR0: usize = 0x00;
const MAR0: usize = 0x08;
const TSD0: usize = 0x10;
const TSAD0: usize = 0x20;
const RBSTART: usize = 0x30;
const CR: usize = 0x37;
const CAPR: usize = 0x38;
const IMR: usize = 0x3c;
const ISR: usize = 0x3e;
const TCR: usize = 0x40;
const RCR: usize = 0x44;
const CONFIG1: usize = 0x52;
const MSR: usize = 0x58;

const CR_RESET: u8 = 1 << 4;
const CR_RECEIVE: u8 = 1 << 3;
const CR_TRANSMIT: u8 = 1 << 2;
const CR_BUFFER_EMPTY: u8 = 1;
const CONFIG1_POWER_ON: u8 = 0;
const RCR_PHYSICAL_MATCH: u32 = 1 << 1;
const RCR_MULTICAST: u32 = 1 << 2;
const RCR_BROADCAST: u32 = 1 << 3;
const RCR_WRAP: u32 = 1 << 7;
const RCR_UNLIMITED_DMA_BURST: u32 = 0b111 << 8;
const RCR_8_KIB_RING: u32 = 0;
const TCR_DMA_BURST_1024: u32 = 0b110 << 8;
const TCR_INTERFRAME_GAP: u32 = 0b11 << 24;
const MSR_LINK_DOWN: u8 = 1 << 2;
const TSD_OWNED_BY_HOST: u32 = 1 << 13;
const TSD_EARLY_THRESHOLD: u32 = 0b11_1111 << 16;
const STATUS_RECEIVE_OK: u16 = 1;
const HEADER_BYTES: usize = 4;
const CRC_BYTES: usize = 4;
const RING_BYTES: usize = 8192;
const RING_SLACK: usize = 16 + 1536;
const RING_OFFSET_BIAS: u16 = 16;
const TRANSMIT_SLOTS: usize = 4;
const MAX_TRANSMIT_BYTES: usize = 1792;
const SHORTEST_FRAME_BYTES: usize = 60;

const _: () = assert!(RECEIVE_BUFFERS + RING_BYTES + RING_SLACK <= TRANSMIT_BUFFERS);
const _: () = assert!(MAX_FRAME_BYTES <= MAX_TRANSMIT_BYTES);
const _: () = assert!(MAX_TRANSMIT_BYTES <= BUFFER_BYTES);

/// Whether the Realtek device id is an RTL8139, 8100 or 8130.
pub fn is_rtl8139(device: u16) -> bool {
    matches!(device, 0x8139 | 0x8138 | 0x8100 | 0x8130)
}

/// An RTL8139 adapter.
#[derive(Debug)]
pub struct Rtl8139<R: RegisterIo, M: DmaMemory> {
    regs: R,
    memory: M,
    mac: [u8; 6],
    receive_offset: usize,
    transmit_next: usize,
    transmit_used: [bool; TRANSMIT_SLOTS],
}

impl<R: RegisterIo, M: DmaMemory> Rtl8139<R, M> {
    /// Powers the adapter at `regs` on, resets it and starts it with its receive ring and
    /// transmit buffers in `memory`, which must lie below 4 GiB.
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
        let end = memory.dma_addr().get().saturating_add(memory.len() as u64);
        if end > u64::from(u32::MAX) {
            return Err((
                Error::Unsupported("the RTL8139 reaches only 32-bit addresses"),
                memory,
            ));
        }
        regs.write8(CONFIG1, CONFIG1_POWER_ON);
        regs.write8(CR, CR_RESET);
        loop {
            if regs.read8(CR) & CR_RESET == 0 {
                break;
            }
            if !keep_waiting() {
                return Err((Error::TimedOut, memory));
            }
            core::hint::spin_loop();
        }
        let mut mac = [0u8; 6];
        for (index, byte) in mac.iter_mut().enumerate() {
            *byte = regs.read8(IDR0 + index);
        }
        if !is_valid_mac(mac) {
            return Err((Error::NoAddress, memory));
        }
        let adapter = Self {
            regs,
            memory,
            mac,
            receive_offset: 0,
            transmit_next: 0,
            transmit_used: [false; TRANSMIT_SLOTS],
        };
        adapter.start();
        Ok(adapter)
    }

    fn start(&self) {
        let base = self.memory.dma_addr().get();
        self.regs.write16(IMR, 0);
        self.regs.write16(ISR, u16::MAX);
        self.regs
            .write32(RBSTART, (base + RECEIVE_BUFFERS as u64) as u32);
        for slot in 0..TRANSMIT_SLOTS {
            let buffer = base + (TRANSMIT_BUFFERS + slot * BUFFER_BYTES) as u64;
            self.regs.write32(TSAD0 + slot * 4, buffer as u32);
        }
        self.regs.write32(MAR0, 0);
        self.regs.write32(MAR0 + 4, 0);
        self.regs.write8(CR, CR_RECEIVE | CR_TRANSMIT);
        self.regs.write32(
            RCR,
            RCR_PHYSICAL_MATCH
                | RCR_MULTICAST
                | RCR_BROADCAST
                | RCR_WRAP
                | RCR_UNLIMITED_DMA_BURST
                | RCR_8_KIB_RING,
        );
        self.regs
            .write32(TCR, TCR_DMA_BURST_1024 | TCR_INTERFRAME_GAP);
    }

    pub fn mac(&self) -> [u8; 6] {
        self.mac
    }

    pub fn is_link_up(&self) -> bool {
        self.regs.read8(MSR) & MSR_LINK_DOWN == 0
    }

    pub fn receive(&mut self, frame: &mut [u8]) -> Option<usize> {
        loop {
            if self.regs.read8(CR) & CR_BUFFER_EMPTY != 0 {
                return None;
            }
            fence(Ordering::SeqCst);
            let mut header = [0u8; HEADER_BYTES];
            copy_out(
                &self.memory,
                RECEIVE_BUFFERS + self.receive_offset,
                &mut header,
            );
            let status = u16::from_le_bytes([header[0], header[1]]);
            let length = usize::from(u16::from_le_bytes([header[2], header[3]]));
            let frame_bytes = length.saturating_sub(CRC_BYTES);
            let good = status & STATUS_RECEIVE_OK != 0 && frame_bytes <= MAX_FRAME_BYTES;
            let taken = good.then(|| {
                let copied = frame_bytes.min(frame.len());
                copy_out(
                    &self.memory,
                    RECEIVE_BUFFERS + self.receive_offset + HEADER_BYTES,
                    &mut frame[..copied],
                );
                copied
            });
            let next = (self.receive_offset + HEADER_BYTES + length + 3) & !3;
            self.receive_offset = next % RING_BYTES;
            self.regs.write16(
                CAPR,
                (self.receive_offset as u16).wrapping_sub(RING_OFFSET_BIAS),
            );
            if !good && length == 0 {
                return None;
            }
            if taken.is_some() {
                return taken;
            }
        }
    }

    pub fn transmit(&mut self, frame: &[u8]) -> Result<(), Error> {
        if frame.len() > MAX_FRAME_BYTES {
            return Err(Error::FrameTooLarge);
        }
        let slot = self.transmit_next;
        let status = self.regs.read32(TSD0 + slot * 4);
        if self.transmit_used[slot] && status & TSD_OWNED_BY_HOST == 0 {
            return Err(Error::QueueFull);
        }
        let buffer = TRANSMIT_BUFFERS + slot * BUFFER_BYTES;
        copy_in(&self.memory, buffer, frame);
        let padding = [0u8; SHORTEST_FRAME_BYTES];
        if frame.len() < SHORTEST_FRAME_BYTES {
            copy_in(
                &self.memory,
                buffer + frame.len(),
                &padding[frame.len()..],
            );
        }
        fence(Ordering::SeqCst);
        let length = frame.len().max(SHORTEST_FRAME_BYTES) as u32;
        self.regs
            .write32(TSD0 + slot * 4, length | TSD_EARLY_THRESHOLD);
        self.transmit_used[slot] = true;
        self.transmit_next = (slot + 1) % TRANSMIT_SLOTS;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_8139_family_ids_are_taken() {
        assert!(is_rtl8139(0x8139));
        assert!(!is_rtl8139(0x8168));
    }
}
