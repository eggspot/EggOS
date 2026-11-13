//! The virtio IOMMU (virtio 1.2, 5.13), polled: one request queue through which the driver
//! puts endpoints (the devices behind it) into domains and maps ranges of a domain's device
//! addresses to memory (D563). A device in a domain reaches only what is mapped in it.

use super::queue::{Buffer, SplitQueue};
use super::{Error, ResetDone, Transport, device_type};
use crate::dma::DmaMemory;

/// Entries of the request queue the driver sets up, at most.
pub const QUEUE_SIZE: u16 = 8;
/// Where the request buffer starts in the driver's DMA memory; the queue is at its start.
pub const REQUEST_OFFSET: usize = 1024;
/// The DMA memory the driver needs.
pub const MEMORY_BYTES: usize = 4096;

const REQUEST_QUEUE: u16 = 0;
const REQUEST_BYTES: usize = 40;
const TAIL_BYTES: usize = 4;

/// The virtio IOMMU's feature bits (virtio 1.2, 5.13.3).
pub mod feature {
    /// The configuration's `input_range` is valid.
    pub const INPUT_RANGE: u64 = 1 << 0;
    /// The configuration's `domain_range` is valid.
    pub const DOMAIN_RANGE: u64 = 1 << 1;
    /// The device takes `MAP` and `UNMAP` requests.
    pub const MAP_UNMAP: u64 = 1 << 2;
    /// Endpoints in no domain reach memory untranslated.
    pub const BYPASS: u64 = 1 << 3;
    /// The configuration's `bypass` field chooses that.
    pub const BYPASS_CONFIG: u64 = 1 << 6;
}

/// The device configuration's fields (virtio 1.2, 5.13.4).
mod config {
    pub const PAGE_SIZE_MASK: usize = 0;
    pub const INPUT_START: usize = 8;
    pub const INPUT_END: usize = 16;
    pub const DOMAIN_START: usize = 24;
    pub const DOMAIN_END: usize = 28;
    pub const BYPASS: usize = 36;
}

/// Request types (virtio 1.2, 5.13.6).
mod request {
    pub const ATTACH: u8 = 1;
    pub const DETACH: u8 = 2;
    pub const MAP: u8 = 3;
    pub const UNMAP: u8 = 4;
}

/// What a mapping lets the device do.
pub mod access {
    /// The device reads the memory.
    pub const READ: u32 = 1;
    /// The device writes the memory.
    pub const WRITE: u32 = 2;
}

/// What the device says of the domains and addresses it handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The page sizes it maps, a bit per size; the lowest set bit is the granule.
    pub page_sizes: u64,
    /// The device addresses a domain maps, inclusive.
    pub input: (u64, u64),
    /// The domain numbers it takes, inclusive.
    pub domains: (u32, u32),
}

/// A virtio IOMMU, set up and polled.
#[derive(Debug)]
pub struct VirtioIommu<T: Transport, M: DmaMemory> {
    transport: T,
    memory: M,
    queue: SplitQueue,
    limits: Limits,
}

impl<T: Transport, M: DmaMemory> VirtioIommu<T, M> {
    /// Sets the IOMMU behind `transport` up with `memory` ([`MEMORY_BYTES`] or more) for its
    /// queue and request buffer, polling while `keep_waiting` says to. Endpoints in no domain
    /// keep reaching memory untranslated where the device lets the driver choose that, so the
    /// devices probed before their endpoints are attached keep working. On an error the
    /// transport comes back, and the memory too unless the device may still use it.
    pub fn new(
        transport: T,
        memory: M,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<Self, (Error, T, Option<M>)> {
        if memory.len() < MEMORY_BYTES {
            return Err((Error::MemoryTooSmall, transport, Some(memory)));
        }
        if transport.device_type() != device_type::IOMMU {
            return Err((Error::WrongDeviceType, transport, Some(memory)));
        }
        let wanted = feature::INPUT_RANGE
            | feature::DOMAIN_RANGE
            | feature::MAP_UNMAP
            | feature::BYPASS
            | feature::BYPASS_CONFIG;
        let accepted = match super::initialize(&transport, wanted, keep_waiting) {
            Ok(accepted) => accepted,
            Err(error) => return Err((error, transport, Some(memory))),
        };
        if accepted & feature::MAP_UNMAP == 0 {
            super::fail(&transport);
            return Err((Error::MissingFeatures, transport, Some(memory)));
        }
        let read64 = |at: usize| {
            u64::from(transport.read_config32(at)) | u64::from(transport.read_config32(at + 4)) << 32
        };
        let limits = Limits {
            page_sizes: read64(config::PAGE_SIZE_MASK),
            input: match accepted & feature::INPUT_RANGE {
                0 => (0, u64::MAX),
                _ => (read64(config::INPUT_START), read64(config::INPUT_END)),
            },
            domains: match accepted & feature::DOMAIN_RANGE {
                0 => (0, u32::MAX),
                _ => (
                    transport.read_config32(config::DOMAIN_START),
                    transport.read_config32(config::DOMAIN_END),
                ),
            },
        };
        if accepted & feature::BYPASS_CONFIG != 0 {
            transport.write_config8(config::BYPASS, 1);
        }
        let max = transport.max_queue_size(REQUEST_QUEUE).min(QUEUE_SIZE);
        if max == 0 {
            super::fail(&transport);
            return Err((Error::QueueUnavailable, transport, Some(memory)));
        }
        let size = 1 << max.ilog2();
        // SAFETY: `memory` holds at least `MEMORY_BYTES`, more than the queue's
        // `memory_size(QUEUE_SIZE)` before `REQUEST_OFFSET`, 4096-aligned, and owned by this
        // driver, which keeps it for as long as the queue lives; the device reaches it at its
        // DMA address.
        let queue = unsafe { SplitQueue::new(memory.as_mut_ptr(), memory.dma_addr(), size) };
        let mut queue = match queue {
            Ok(queue) => queue,
            Err(error) => return Err((error, transport, Some(memory))),
        };
        queue.disable_used_notifications();
        queue.set_cache_maintenance(memory.cache_maintenance());
        if let Err(error) = transport.set_up_queue(REQUEST_QUEUE, size, &queue.addresses()) {
            super::fail(&transport);
            let memory = match super::reset(&transport, keep_waiting) {
                Ok(_) => Some(memory),
                Err(_) => {
                    core::mem::forget(memory);
                    None
                }
            };
            return Err((error, transport, memory));
        }
        super::driver_ok(&transport);
        Ok(Self {
            transport,
            memory,
            queue,
            limits,
        })
    }

    /// What the device handles.
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// The transport the device is reached through.
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// Puts `endpoint` into `domain`: from now on it reaches only what the domain maps.
    pub fn attach(
        &mut self,
        domain: u32,
        endpoint: u32,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        let mut body = [0u8; 16];
        body[..4].copy_from_slice(&domain.to_le_bytes());
        body[4..8].copy_from_slice(&endpoint.to_le_bytes());
        self.request(request::ATTACH, &body, keep_waiting)
    }

    /// Takes `endpoint` out of `domain`.
    pub fn detach(
        &mut self,
        domain: u32,
        endpoint: u32,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        let mut body = [0u8; 16];
        body[..4].copy_from_slice(&domain.to_le_bytes());
        body[4..8].copy_from_slice(&endpoint.to_le_bytes());
        self.request(request::DETACH, &body, keep_waiting)
    }

    /// Maps `length` bytes of `domain`'s device addresses from `device_address` to the memory
    /// at `physical`, with `permissions` ([`access`]); all three aligned to the granule.
    pub fn map(
        &mut self,
        domain: u32,
        device_address: u64,
        physical: u64,
        length: u64,
        permissions: u32,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        let last = device_address
            .checked_add(length.checked_sub(1).ok_or(Error::CommandFailed(INVALID))?)
            .ok_or(Error::CommandFailed(INVALID))?;
        let mut body = [0u8; 32];
        body[..4].copy_from_slice(&domain.to_le_bytes());
        body[4..12].copy_from_slice(&device_address.to_le_bytes());
        body[12..20].copy_from_slice(&last.to_le_bytes());
        body[20..28].copy_from_slice(&physical.to_le_bytes());
        body[28..32].copy_from_slice(&permissions.to_le_bytes());
        self.request(request::MAP, &body, keep_waiting)
    }

    /// Unmaps `length` bytes of `domain`'s device addresses from `device_address`.
    pub fn unmap(
        &mut self,
        domain: u32,
        device_address: u64,
        length: u64,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        let last = device_address
            .checked_add(length.checked_sub(1).ok_or(Error::CommandFailed(INVALID))?)
            .ok_or(Error::CommandFailed(INVALID))?;
        let mut body = [0u8; 24];
        body[..4].copy_from_slice(&domain.to_le_bytes());
        body[4..12].copy_from_slice(&device_address.to_le_bytes());
        body[12..20].copy_from_slice(&last.to_le_bytes());
        self.request(request::UNMAP, &body, keep_waiting)
    }

    fn request(
        &mut self,
        kind: u8,
        body: &[u8],
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        let readable = 4 + body.len();
        if readable > REQUEST_BYTES {
            return Err(Error::MemoryTooSmall);
        }
        let start = self.memory.as_mut_ptr().wrapping_add(REQUEST_OFFSET);
        let mut bytes = [0u8; REQUEST_BYTES + TAIL_BYTES];
        bytes[0] = kind;
        bytes[4..readable].copy_from_slice(body);
        bytes[REQUEST_BYTES] = NO_STATUS;
        for (index, &byte) in bytes.iter().enumerate() {
            // SAFETY: inside the request buffer of the driver's DMA memory (`REQUEST_OFFSET`
            // plus 44 bytes, inside `MEMORY_BYTES`), which the device does not hold: the last
            // request was answered before this one is written.
            unsafe { start.wrapping_add(index).write_volatile(byte) };
        }
        self.memory.prepare_for_device(REQUEST_OFFSET, readable);
        self.memory
            .prepare_for_cpu(REQUEST_OFFSET + REQUEST_BYTES, TAIL_BYTES);
        let at = |offset: usize| {
            self.memory
                .dma_addr()
                .checked_add((REQUEST_OFFSET + offset) as u64)
                .ok_or(Error::MemoryTooSmall)
        };
        let chain = [
            Buffer {
                addr: at(0)?,
                len: readable as u32,
                device_writes: false,
            },
            Buffer {
                addr: at(REQUEST_BYTES)?,
                len: TAIL_BYTES as u32,
                device_writes: true,
            },
        ];
        self.queue.submit(&chain)?;
        if self.queue.should_notify() {
            self.transport.notify(REQUEST_QUEUE);
        }
        loop {
            if self.queue.pop_used().is_some() {
                break;
            }
            if !keep_waiting() {
                return Err(Error::TimedOut);
            }
        }
        self.memory
            .prepare_for_cpu(REQUEST_OFFSET + REQUEST_BYTES, TAIL_BYTES);
        // SAFETY: the tail's status byte, which the device wrote and handed back.
        let status = unsafe { start.wrapping_add(REQUEST_BYTES).read_volatile() };
        match status {
            0 => Ok(()),
            failed => Err(Error::CommandFailed(failed.into())),
        }
    }

    /// Resets the device and hands back the transport and the memory, with the proof that the
    /// device no longer uses it. Every endpoint it held goes back to its bypass setting.
    pub fn reset(self, keep_waiting: &mut dyn FnMut() -> bool) -> Result<(T, M, ResetDone), Error> {
        let Self {
            transport, memory, ..
        } = self;
        match super::reset(&transport, keep_waiting) {
            Ok(done) => Ok((transport, memory, done)),
            Err(error) => {
                core::mem::forget(memory);
                Err(error)
            }
        }
    }
}

const NO_STATUS: u8 = 0xff;
const INVALID: u32 = 4;
