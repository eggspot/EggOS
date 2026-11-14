//! The virtio GPU device (virtio 1.2, 5.7), 2D and polled: the control queue, one scanout
//! showing one resource whose backing is the driver's framebuffer in guest memory, and the cursor
//! queue, which shows a 64x64 cursor image over the scanout and moves it without any redraw
//! (D186).
//!
//! Every command is fenced (`VIRTIO_GPU_FLAG_FENCE`), so its answer means the device finished
//! it: the spec lets a device answer an unfenced command before processing it and does not
//! promise to process commands in order. A flush therefore sends the damaged rectangles'
//! transfers, waits for their answers, then sends their flushes.
//!
//! A device that offers `VIRTIO_GPU_F_VIRGL` also takes 3D commands (D415): with memory for
//! them ([`VirtioGpu::attach_three_d`]), [`VirtioGpu::run_three_d`] sends one and waits for its
//! answer. Contexts' command streams go through [`STREAM_SLOTS`] slots of their own, posted
//! with [`VirtioGpu::start_stream`] and answered in any order, so several are in flight while
//! the display keeps flushing (D460): answers are told apart by their chain's head.

use super::queue::{Buffer, SplitQueue};
use super::{Error, ResetDone, Transport, device_type};
use crate::display::{BYTES_PER_PIXEL, Mode, Rect};
use crate::dma::DmaMemory;

#[cfg(test)]
mod tests;
pub mod three_d;

use three_d::ThreeDCommand;

/// Entries of the control queue the driver sets up, at most.
pub const QUEUE_SIZE: u16 = 32;
/// The DMA memory the driver needs for its queue, requests and responses (the framebuffer is
/// separate, see [`VirtioGpu::attach_framebuffer`]).
pub const MEMORY_BYTES: usize = 4096;
/// `VIRTIO_GPU_FORMAT_B8G8R8X8_UNORM`: bytes `B, G, R, X` per pixel, a little-endian
/// `0x00RRGGBB`.
pub const FORMAT_B8G8R8X8_UNORM: u32 = 2;
/// The mode the spec suggests when the device reports no display.
pub const FALLBACK_MODE: Mode = Mode::new(1024, 768);
/// Entries of the cursor queue the driver sets up, at most: one command is in flight at a time.
pub const CURSOR_QUEUE_SIZE: u16 = 4;
/// The side of the square cursor image, in pixels.
pub const CURSOR_SIDE: u32 = 64;
/// The DMA memory the cursor's image needs ([`VirtioGpu::attach_cursor`]).
pub const CURSOR_BYTES: usize =
    CURSOR_SIDE as usize * CURSOR_SIDE as usize * BYTES_PER_PIXEL as usize;
/// `VIRTIO_GPU_FORMAT_B8G8R8A8_UNORM`: bytes `B, G, R, A` per pixel, a little-endian
/// `0xAARRGGBB`.
pub const FORMAT_B8G8R8A8_UNORM: u32 = 1;

const CONTROL_QUEUE: u16 = 0;
const CURSOR_QUEUE: u16 = 1;
const CURSOR_RING: usize = 3072;
const CURSOR_REQUEST: usize = 3584;
const CURSOR_RESOURCE_ID: u32 = 2;
const NO_RESOURCE: u32 = 0;
const REQUESTS: usize = 1024;
const REQUEST_SLOT: usize = 64;
const RESPONSES: usize = 2048;
const RESPONSE_SLOT: usize = 32;
const DISPLAY_INFO: usize = 2560;
const MAX_BATCH: usize = (RESPONSES - REQUESTS) / REQUEST_SLOT;
const RESOURCE_ID: u32 = 1;
const OTHER_RESOURCE_ID: u32 = 3;
const RESOURCE_ONLY_BYTES: usize = 32;
const CONFIG_EVENTS_READ_OFFSET: usize = 0;
const CONFIG_EVENTS_CLEAR_OFFSET: usize = 4;
const EVENT_DISPLAY: u32 = 1;
const SCANOUT_ID: u32 = 0;
const CONFIG_NUM_SCANOUTS_OFFSET: usize = 8;
const CONFIG_NUM_CAPSETS_OFFSET: usize = 12;
const THREE_D_REQUEST: usize = 0;
const THREE_D_ANSWER: usize = 256;
const THREE_D_STREAM: usize = 8192;
/// The bytes of command stream one [`VirtioGpu::start_stream`] carries, at most.
pub const COMMAND_STREAM_BYTES: usize = 256 * 1024;
/// How many command streams may be in flight at once (D460).
pub const STREAM_SLOTS: usize = 2;
const STREAM_HEADERS: usize = THREE_D_STREAM + STREAM_SLOTS * COMMAND_STREAM_BYTES;
const STREAM_HEADER: usize = 128;
const STREAM_ANSWER: usize = 64;
/// The DMA memory the 3D commands need ([`VirtioGpu::attach_three_d`]).
pub const THREE_D_BYTES: usize = STREAM_HEADERS + STREAM_SLOTS * STREAM_HEADER;
/// The most bytes of capability set one answer carries.
pub const CAPSET_ROOM: usize = THREE_D_STREAM - THREE_D_ANSWER - three_d::HEADER_BYTES;
const FLAG_FENCE: u32 = 1;

const HEADER_BYTES: usize = 24;
const DISPLAY_INFO_BYTES: usize = HEADER_BYTES + 16 * 24;
const CREATE_2D_BYTES: usize = 40;
const ATTACH_BACKING_BYTES: usize = 48;
const ATTACH_BACKING_ENTRIES: usize = 32;
const BACKING_ENTRY_BYTES: usize = 16;
const SET_SCANOUT_BYTES: usize = 48;
const TRANSFER_TO_HOST_2D_BYTES: usize = 56;
const RESOURCE_FLUSH_BYTES: usize = 48;
const UPDATE_CURSOR_BYTES: usize = 56;

const _: () = assert!(super::queue::memory_size(QUEUE_SIZE) <= REQUESTS);
const _: () = assert!(RESPONSES + MAX_BATCH * RESPONSE_SLOT <= DISPLAY_INFO);
const _: () = assert!(DISPLAY_INFO + DISPLAY_INFO_BYTES <= MEMORY_BYTES);
const _: () = assert!(TRANSFER_TO_HOST_2D_BYTES <= REQUEST_SLOT);
const _: () = assert!(DISPLAY_INFO + DISPLAY_INFO_BYTES <= CURSOR_RING);
const _: () = assert!(CURSOR_RING + super::queue::memory_size(CURSOR_QUEUE_SIZE) <= CURSOR_REQUEST);
const _: () = assert!(CURSOR_REQUEST + UPDATE_CURSOR_BYTES <= MEMORY_BYTES);
const _: () = assert!(THREE_D_REQUEST + three_d::LONGEST_REQUEST <= THREE_D_ANSWER);
const _: () = assert!(three_d::SUBMIT_BYTES <= STREAM_ANSWER);
const _: () = assert!(STREAM_ANSWER + three_d::HEADER_BYTES <= STREAM_HEADER);

mod command {
    pub const GET_DISPLAY_INFO: u32 = 0x0100;
    pub const RESOURCE_CREATE_2D: u32 = 0x0101;
    pub const RESOURCE_UNREF: u32 = 0x0102;
    pub const SET_SCANOUT: u32 = 0x0103;
    pub const RESOURCE_FLUSH: u32 = 0x0104;
    pub const TRANSFER_TO_HOST_2D: u32 = 0x0105;
    pub const RESOURCE_ATTACH_BACKING: u32 = 0x0106;
    pub const RESOURCE_DETACH_BACKING: u32 = 0x0107;
    pub const UPDATE_CURSOR: u32 = 0x0300;
    pub const MOVE_CURSOR: u32 = 0x0301;
}

mod response {
    pub const OK_NODATA: u32 = 0x1100;
    pub const OK_DISPLAY_INFO: u32 = 0x1101;
}

/// What [`VirtioGpu::reset`] hands back: the transport, the memory, and the proof that the
/// device no longer uses that memory.
#[derive(Debug)]
pub struct Released<T, M> {
    pub transport: T,
    pub control: M,
    /// The framebuffer, if one was attached.
    pub framebuffer: Option<M>,
    /// The cursor's image, if one was attached.
    pub cursor: Option<M>,
    /// The 3D commands' memory, if it was attached.
    pub three_d: Option<M>,
    pub reset: ResetDone,
}

#[derive(Debug)]
struct Scanout<M> {
    framebuffer: M,
    mode: Mode,
    shown: bool,
    resource: u32,
}

#[derive(Debug)]
struct Cursor<M> {
    image: M,
    attached: bool,
}

/// A virtio GPU device, set up and polled.
#[derive(Debug)]
pub struct VirtioGpu<T: Transport, M: DmaMemory> {
    transport: T,
    control: M,
    queue: SplitQueue,
    cursor_queue: Option<SplitQueue>,
    display: Mode,
    scanout: Option<Scanout<M>>,
    cursor: Option<Cursor<M>>,
    fence_id: u64,
    unanswered: bool,
    started: Option<StartedFlush>,
    features: u64,
    three_d: Option<M>,
    posted_three_d: Option<PostedThreeD>,
    answered_three_d: usize,
    posted_streams: [Option<PostedStream>; STREAM_SLOTS],
    refused_streams: u64,
    mirrored: u16,
}

/// The most scanouts a virtio GPU has (virtio 1.2, 5.7.4: `VIRTIO_GPU_MAX_SCANOUTS`).
pub const MAX_SCANOUTS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PostedThreeD {
    head: u16,
    expected: u32,
    answer_bytes: usize,
    answered: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PostedStream {
    head: u16,
    fence: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StartedFlush {
    commands: usize,
    answered: usize,
    rects: usize,
}

impl<T: Transport, M: DmaMemory> VirtioGpu<T, M> {
    /// Sets the GPU behind `transport` up with `control` ([`MEMORY_BYTES`] or more) for its
    /// control queue, requests and responses, and asks it for scanout 0's mode
    /// ([`Self::display_mode`]), polling while `keep_waiting` says to. On an error the transport
    /// comes back, and the memory too unless the device may still use it (a reset that did not
    /// read back 0): then it is leaked.
    pub fn new(
        transport: T,
        control: M,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<Self, (Error, T, Option<M>)> {
        if control.len() < MEMORY_BYTES {
            return Err((Error::MemoryTooSmall, transport, Some(control)));
        }
        if transport.device_type() != device_type::GPU {
            return Err((Error::WrongDeviceType, transport, Some(control)));
        }
        let wanted =
            three_d::FEATURE_VIRGL | three_d::FEATURE_CONTEXT_INIT | three_d::FEATURE_RESOURCE_BLOB;
        let features = match super::initialize(&transport, wanted, keep_waiting) {
            Ok(features) => features,
            Err(error) => return Err((error, transport, Some(control))),
        };
        let max = transport.max_queue_size(CONTROL_QUEUE).min(QUEUE_SIZE);
        if max < 2 || transport.read_config32(CONFIG_NUM_SCANOUTS_OFFSET) == 0 {
            super::fail(&transport);
            let error = match max < 2 {
                true => Error::QueueUnavailable,
                false => Error::NoScanout,
            };
            return Err((error, transport, Some(control)));
        }
        let size = 1 << max.ilog2();
        // SAFETY: `control` holds at least `MEMORY_BYTES`, more than the queue's
        // `memory_size(QUEUE_SIZE)` before `REQUESTS`, 4096-aligned, and owned by this driver,
        // which keeps it for as long as the queue lives; the device reaches it at its DMA address.
        let queue = unsafe { SplitQueue::new(control.as_mut_ptr(), control.dma_addr(), size) };
        let mut queue = match queue {
            Ok(queue) => queue,
            Err(error) => return Err((error, transport, Some(control))),
        };
        queue.disable_used_notifications();
        if let Err(error) = transport.set_up_queue(CONTROL_QUEUE, size, &queue.addresses()) {
            super::fail(&transport);
            let control = match super::reset(&transport, keep_waiting) {
                Ok(_) => Some(control),
                Err(_) => {
                    core::mem::forget(control);
                    None
                }
            };
            return Err((error, transport, control));
        }
        let cursor_queue = cursor_queue_of(&transport, &control);
        super::driver_ok(&transport);
        let mut gpu = Self {
            transport,
            control,
            queue,
            cursor_queue,
            display: FALLBACK_MODE,
            scanout: None,
            cursor: None,
            fence_id: 0,
            unanswered: false,
            started: None,
            features,
            three_d: None,
            posted_three_d: None,
            answered_three_d: 0,
            posted_streams: [None; STREAM_SLOTS],
            refused_streams: 0,
            mirrored: 0,
        };
        match gpu.query_display(keep_waiting) {
            Ok(mode) => {
                gpu.display = mode;
                Ok(gpu)
            }
            Err(error) => {
                super::fail(&gpu.transport);
                match gpu.reset(keep_waiting) {
                    Ok(released) => Err((error, released.transport, Some(released.control))),
                    Err((_, transport)) => Err((error, transport, None)),
                }
            }
        }
    }

    /// The transport the device is reached through.
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// Scanout 0's mode as the device reported it, or [`FALLBACK_MODE`] when it reported none.
    pub fn display_mode(&self) -> Mode {
        self.display
    }

    /// The mode of the framebuffer on the scanout, once one is attached and shown.
    pub fn mode(&self) -> Option<Mode> {
        self.shown().map(|scanout| scanout.mode)
    }

    fn shown(&self) -> Option<&Scanout<M>> {
        self.scanout.as_ref().filter(|scanout| scanout.shown)
    }

    /// Makes `framebuffer` ([`Mode::bytes`] of `mode` or more) the backing of a `mode`-sized
    /// resource in [`FORMAT_B8G8R8X8_UNORM`] and shows it on scanout 0, which the device scales
    /// or crops to the display as it does. Refused with the framebuffer handed back when a
    /// scanout already exists, the mode is empty or too large, or the memory too small; once the
    /// device was given the memory, it stays with the driver even on an error, unseen by
    /// [`Self::mode`], [`Self::pixels`] and [`Self::flush`], and comes back from [`Self::reset`].
    pub fn attach_framebuffer(
        &mut self,
        framebuffer: M,
        mode: Mode,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), (Error, Option<M>)> {
        if self.unanswered {
            return Err((Error::TimedOut, Some(framebuffer)));
        }
        if self.scanout.is_some() {
            return Err((Error::ScanoutInUse, Some(framebuffer)));
        }
        let Ok(_) = u32::try_from(mode.bytes()) else {
            return Err((Error::UnsupportedMode, Some(framebuffer)));
        };
        if mode.rect().is_empty() {
            return Err((Error::UnsupportedMode, Some(framebuffer)));
        }
        if framebuffer.len() < mode.bytes() {
            return Err((Error::MemoryTooSmall, Some(framebuffer)));
        }
        let request = self.header(0, command::RESOURCE_CREATE_2D);
        self.put32(request + 24, RESOURCE_ID);
        self.put32(request + 28, FORMAT_B8G8R8X8_UNORM);
        self.put32(request + 32, mode.width);
        self.put32(request + 36, mode.height);
        if let Err(error) = self.run(1, CREATE_2D_BYTES, keep_waiting) {
            return Err((error, Some(framebuffer)));
        }
        let request = self.header(0, command::RESOURCE_ATTACH_BACKING);
        self.put32(request + 24, RESOURCE_ID);
        let Some(attach_bytes) = self.put_backing(request, &framebuffer) else {
            return Err((Error::MemoryTooSmall, Some(framebuffer)));
        };
        self.scanout = Some(Scanout {
            framebuffer,
            mode,
            shown: false,
            resource: RESOURCE_ID,
        });
        self.run(1, attach_bytes, keep_waiting)
            .map_err(|error| (error, None))?;
        let request = self.header(0, command::SET_SCANOUT);
        self.put_rect(request + 24, mode.rect());
        self.put32(request + 40, SCANOUT_ID);
        self.put32(request + 44, RESOURCE_ID);
        self.run(1, SET_SCANOUT_BYTES, keep_waiting)
            .map_err(|error| (error, None))?;
        if let Some(scanout) = self.scanout.as_mut() {
            scanout.shown = true;
        }
        Ok(())
    }

    /// Shows `framebuffer` ([`Mode::bytes`] of `mode` or more) on scanout 0 in place of the
    /// shown one, as a new resource (D287): the display changes mode. The old framebuffer comes
    /// back once the device let go of it (its resource detached and destroyed). Refused with
    /// the new framebuffer handed back when nothing is shown, the mode is empty or too large, or
    /// the memory too small; once the device was given the new memory it stays with the driver
    /// on an error, as in [`Self::attach_framebuffer`].
    pub fn replace_framebuffer(
        &mut self,
        framebuffer: M,
        mode: Mode,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<M, (Error, Option<M>)> {
        if self.unanswered {
            return Err((Error::TimedOut, Some(framebuffer)));
        }
        let Some(old_resource) = self.shown().map(|scanout| scanout.resource) else {
            return Err((Error::NoScanout, Some(framebuffer)));
        };
        let Ok(_) = u32::try_from(mode.bytes()) else {
            return Err((Error::UnsupportedMode, Some(framebuffer)));
        };
        if mode.rect().is_empty() {
            return Err((Error::UnsupportedMode, Some(framebuffer)));
        }
        if framebuffer.len() < mode.bytes() {
            return Err((Error::MemoryTooSmall, Some(framebuffer)));
        }
        let resource = match old_resource {
            RESOURCE_ID => OTHER_RESOURCE_ID,
            _ => RESOURCE_ID,
        };
        let request = self.header(0, command::RESOURCE_CREATE_2D);
        self.put32(request + 24, resource);
        self.put32(request + 28, FORMAT_B8G8R8X8_UNORM);
        self.put32(request + 32, mode.width);
        self.put32(request + 36, mode.height);
        if let Err(error) = self.run(1, CREATE_2D_BYTES, keep_waiting) {
            return Err((error, Some(framebuffer)));
        }
        let request = self.header(0, command::RESOURCE_ATTACH_BACKING);
        self.put32(request + 24, resource);
        let Some(attach_bytes) = self.put_backing(request, &framebuffer) else {
            return Err((Error::MemoryTooSmall, Some(framebuffer)));
        };
        let replaced = self.scanout.replace(Scanout {
            framebuffer,
            mode,
            shown: false,
            resource,
        });
        let Some(old) = replaced else {
            return Err((Error::NoScanout, None));
        };
        self.run(1, attach_bytes, keep_waiting)
            .map_err(|error| (error, None))?;
        let request = self.header(0, command::SET_SCANOUT);
        self.put_rect(request + 24, mode.rect());
        self.put32(request + 40, SCANOUT_ID);
        self.put32(request + 44, resource);
        self.run(1, SET_SCANOUT_BYTES, keep_waiting)
            .map_err(|error| (error, None))?;
        if let Some(scanout) = self.scanout.as_mut() {
            scanout.shown = true;
        }
        let request = self.header(0, command::RESOURCE_DETACH_BACKING);
        self.put32(request + 24, old.resource);
        self.run(1, RESOURCE_ONLY_BYTES, keep_waiting)
            .map_err(|error| (error, None))?;
        let request = self.header(0, command::RESOURCE_UNREF);
        self.put32(request + 24, old.resource);
        self.run(1, RESOURCE_ONLY_BYTES, keep_waiting)
            .map_err(|error| (error, None))?;
        Ok(old.framebuffer)
    }

    /// Whether the device says its display changed (a host window resized, a monitor plugged
    /// in) since [`Self::clear_display_event`] last cleared it.
    pub fn display_event_pending(&self) -> bool {
        self.transport.read_config32(CONFIG_EVENTS_READ_OFFSET) & EVENT_DISPLAY != 0
    }

    /// Clears the display-changed event.
    pub fn clear_display_event(&self) {
        self.transport
            .write_config8(CONFIG_EVENTS_CLEAR_OFFSET, EVENT_DISPLAY as u8);
    }

    /// Every scanout's mode as the device reports it (D563): `None` for a scanout that is not
    /// enabled (no monitor, or unplugged).
    pub fn scanout_modes(
        &mut self,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<[Option<Mode>; MAX_SCANOUTS], Error> {
        if self.unanswered {
            return Err(Error::TimedOut);
        }
        self.header(0, command::GET_DISPLAY_INFO);
        self.put32(DISPLAY_INFO, 0);
        self.post(REQUESTS, HEADER_BYTES, DISPLAY_INFO, DISPLAY_INFO_BYTES)?;
        self.wait(1, keep_waiting)?;
        let mut modes = [None; MAX_SCANOUTS];
        if self.get32(DISPLAY_INFO) != response::OK_DISPLAY_INFO {
            return Ok(modes);
        }
        let count = (self.transport.read_config32(CONFIG_NUM_SCANOUTS_OFFSET) as usize).min(MAX_SCANOUTS);
        for (index, mode) in modes.iter_mut().enumerate().take(count) {
            let entry = DISPLAY_INFO + HEADER_BYTES + index * 24;
            let (width, height) = (self.get32(entry + 8), self.get32(entry + 12));
            if self.get32(entry + 16) != 0 && width > 0 && height > 0 {
                *mode = Some(Mode::new(width, height));
            }
        }
        Ok(modes)
    }

    /// The scanouts besides scanout 0 that show its framebuffer, a bit each.
    pub fn mirrored_scanouts(&self) -> u16 {
        self.mirrored
    }

    /// Shows the shown framebuffer on every other enabled scanout too, which the device scales
    /// to each monitor (mirroring: no copy, one resource on several scanouts), and stops
    /// showing it on the ones that were unplugged (D563). Called when the device reports a
    /// display change. Returns the scanouts mirrored after it.
    pub fn follow_scanouts(&mut self, keep_waiting: &mut dyn FnMut() -> bool) -> Result<u16, Error> {
        let Some((resource, mode)) = self.shown().map(|scanout| (scanout.resource, scanout.mode))
        else {
            return Ok(self.mirrored);
        };
        let modes = self.scanout_modes(keep_waiting)?;
        for (index, scanout_mode) in modes.iter().enumerate().skip(1) {
            let bit = 1u16 << index;
            match (scanout_mode.is_some(), self.mirrored & bit != 0) {
                (true, false) => {
                    self.set_scanout(index as u32, resource, mode, keep_waiting)?;
                    self.mirrored |= bit;
                }
                (false, true) => {
                    self.set_scanout(index as u32, 0, Mode::new(0, 0), keep_waiting)?;
                    self.mirrored &= !bit;
                }
                _ => {}
            }
        }
        Ok(self.mirrored)
    }

    fn set_scanout(
        &mut self,
        scanout: u32,
        resource: u32,
        mode: Mode,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        let request = self.header(0, command::SET_SCANOUT);
        self.put_rect(request + 24, mode.rect());
        self.put32(request + 40, scanout);
        self.put32(request + 44, resource);
        self.run(1, SET_SCANOUT_BYTES, keep_waiting)
    }

    /// Stops showing the framebuffer on every scanout and gives it back once the device let
    /// go of it (its resource detached and destroyed, D563): for a display that was unplugged,
    /// or a driver that hands the GPU over. `None` when nothing is attached.
    pub fn detach_framebuffer(
        &mut self,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<Option<M>, Error> {
        if self.unanswered {
            return Err(Error::TimedOut);
        }
        let Some(resource) = self.scanout.as_ref().map(|scanout| scanout.resource) else {
            return Ok(None);
        };
        for index in 1..MAX_SCANOUTS {
            if self.mirrored & 1 << index != 0 {
                self.set_scanout(index as u32, 0, Mode::new(0, 0), keep_waiting)?;
            }
        }
        self.mirrored = 0;
        self.set_scanout(SCANOUT_ID, 0, Mode::new(0, 0), keep_waiting)?;
        if let Some(scanout) = self.scanout.as_mut() {
            scanout.shown = false;
        }
        let request = self.header(0, command::RESOURCE_DETACH_BACKING);
        self.put32(request + 24, resource);
        self.run(1, RESOURCE_ONLY_BYTES, keep_waiting)?;
        let request = self.header(0, command::RESOURCE_UNREF);
        self.put32(request + 24, resource);
        self.run(1, RESOURCE_ONLY_BYTES, keep_waiting)?;
        Ok(self.scanout.take().map(|scanout| scanout.framebuffer))
    }

    /// Asks the device for scanout 0's preferred mode again, as [`Self::new`] did.
    pub fn preferred_mode(
        &mut self,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<Mode, Error> {
        if self.unanswered {
            return Err(Error::TimedOut);
        }
        let mode = self.query_display(keep_waiting)?;
        self.display = mode;
        Ok(mode)
    }

    /// Whether the device has a cursor queue the driver set up, so [`Self::attach_cursor`] can
    /// work.
    pub fn has_cursor_queue(&self) -> bool {
        self.cursor_queue.is_some()
    }

    /// Makes `image` ([`CURSOR_BYTES`] or more) the backing of a [`CURSOR_SIDE`]-square resource
    /// in [`FORMAT_B8G8R8A8_UNORM`], the cursor [`Self::show_cursor`] shows. Refused with the
    /// memory handed back when the device has no cursor queue, a cursor is attached already or
    /// the memory is too small; once the device was given the memory, it stays with the driver
    /// even on an error, as a framebuffer does, and comes back from [`Self::reset`].
    pub fn attach_cursor(
        &mut self,
        image: M,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), (Error, Option<M>)> {
        if self.unanswered {
            return Err((Error::TimedOut, Some(image)));
        }
        if self.cursor_queue.is_none() || self.cursor.is_some() {
            return Err((Error::NoCursor, Some(image)));
        }
        if image.len() < CURSOR_BYTES {
            return Err((Error::MemoryTooSmall, Some(image)));
        }
        let request = self.header(0, command::RESOURCE_CREATE_2D);
        self.put32(request + 24, CURSOR_RESOURCE_ID);
        self.put32(request + 28, FORMAT_B8G8R8A8_UNORM);
        self.put32(request + 32, CURSOR_SIDE);
        self.put32(request + 36, CURSOR_SIDE);
        if let Err(error) = self.run(1, CREATE_2D_BYTES, keep_waiting) {
            return Err((error, Some(image)));
        }
        let request = self.header(0, command::RESOURCE_ATTACH_BACKING);
        self.put32(request + 24, CURSOR_RESOURCE_ID);
        self.put32(request + 28, 1);
        self.put64(request + 32, image.dma_addr().get());
        self.put32(request + 40, CURSOR_BYTES as u32);
        self.put32(request + 44, 0);
        self.cursor = Some(Cursor {
            image,
            attached: false,
        });
        self.run(1, ATTACH_BACKING_BYTES, keep_waiting)
            .map_err(|error| (error, None))?;
        if let Some(cursor) = self.cursor.as_mut() {
            cursor.attached = true;
        }
        Ok(())
    }

    /// The cursor image's pixels, [`CURSOR_SIDE`] rows of [`CURSOR_SIDE`], each a `0xAARRGGBB`
    /// (alpha 0 is transparent); `None` before [`Self::attach_cursor`]. What is written here
    /// shows at the next [`Self::show_cursor`].
    pub fn cursor_pixels(&mut self) -> Option<&mut [u32]> {
        let cursor = self.cursor.as_mut().filter(|cursor| cursor.attached)?;
        let pixels = CURSOR_BYTES / BYTES_PER_PIXEL as usize;
        // SAFETY: the image holds at least `CURSOR_BYTES` (checked when attached), 4096-aligned
        // so aligned for `u32`, owned by this driver while it lives; the device reads it only
        // during the transfer inside `show_cursor`, which takes `&mut self`, so never while this
        // borrow lives.
        Some(unsafe {
            core::slice::from_raw_parts_mut(cursor.image.as_mut_ptr().cast::<u32>(), pixels)
        })
    }

    /// Shows the cursor image as it is now with its hotspot (`hotspot` pixels into the image)
    /// at `at` on scanout 0: the image's transfer to the device, then the cursor queue's
    /// `UPDATE_CURSOR`, polling while `keep_waiting` says to.
    pub fn show_cursor(
        &mut self,
        hotspot: (u32, u32),
        at: (u32, u32),
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        if self.cursor.as_ref().is_none_or(|cursor| !cursor.attached) {
            return Err(Error::NoCursor);
        }
        let request = self.header(0, command::TRANSFER_TO_HOST_2D);
        self.put_rect(request + 24, Rect::new(0, 0, CURSOR_SIDE, CURSOR_SIDE));
        self.put64(request + 40, 0);
        self.put32(request + 48, CURSOR_RESOURCE_ID);
        self.put32(request + 52, 0);
        self.run(1, TRANSFER_TO_HOST_2D_BYTES, keep_waiting)?;
        self.send_cursor(
            command::UPDATE_CURSOR,
            CURSOR_RESOURCE_ID,
            hotspot,
            at,
            keep_waiting,
        )
    }

    /// Moves the shown cursor's hotspot to `at`: one `MOVE_CURSOR`, nothing redrawn.
    pub fn move_cursor(
        &mut self,
        at: (u32, u32),
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        self.send_cursor(
            command::MOVE_CURSOR,
            CURSOR_RESOURCE_ID,
            (0, 0),
            at,
            keep_waiting,
        )
    }

    /// Hides the cursor: an `UPDATE_CURSOR` with no resource.
    pub fn hide_cursor(&mut self, keep_waiting: &mut dyn FnMut() -> bool) -> Result<(), Error> {
        self.send_cursor(
            command::UPDATE_CURSOR,
            NO_RESOURCE,
            (0, 0),
            (0, 0),
            keep_waiting,
        )
    }

    fn send_cursor(
        &mut self,
        command: u32,
        resource: u32,
        hotspot: (u32, u32),
        at: (u32, u32),
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        if self.unanswered {
            return Err(Error::TimedOut);
        }
        if self.cursor.as_ref().is_none_or(|cursor| !cursor.attached) {
            return Err(Error::NoCursor);
        }
        let fields = [
            (0, command),
            (4, 0),
            (16, 0),
            (20, 0),
            (24, SCANOUT_ID),
            (28, at.0),
            (32, at.1),
            (36, 0),
            (40, resource),
            (44, hotspot.0),
            (48, hotspot.1),
            (52, 0),
        ];
        for (offset, value) in fields {
            self.put32(CURSOR_REQUEST + offset, value);
        }
        self.put64(CURSOR_REQUEST + 8, 0);
        let addr = self
            .control
            .dma_addr()
            .checked_add(CURSOR_REQUEST as u64)
            .ok_or(Error::MemoryTooSmall)?;
        let queue = self.cursor_queue.as_mut().ok_or(Error::NoCursor)?;
        queue.submit(&[Buffer {
            addr,
            len: UPDATE_CURSOR_BYTES as u32,
            device_writes: false,
        }])?;
        if queue.should_notify() {
            self.transport.notify(CURSOR_QUEUE);
        }
        while queue.pop_used().is_none() {
            if !keep_waiting() {
                self.unanswered = true;
                return Err(Error::TimedOut);
            }
        }
        Ok(())
    }

    /// The framebuffer's pixels, row after row, each a `0x00RRGGBB`; `None` before
    /// [`Self::attach_framebuffer`]. What is drawn shows once [`Self::flush`]ed.
    pub fn pixels(&mut self) -> Option<&mut [u32]> {
        let scanout = self.scanout.as_mut().filter(|scanout| scanout.shown)?;
        let pixels = scanout.mode.bytes() / BYTES_PER_PIXEL as usize;
        // SAFETY: the framebuffer holds at least `mode.bytes()` bytes (checked when attached),
        // 4096-aligned so aligned for `u32`, owned by this driver while it lives; the device
        // reads it only while a transfer runs inside `flush`, which takes `&mut self`, so never
        // while this borrow lives.
        Some(unsafe {
            core::slice::from_raw_parts_mut(scanout.framebuffer.as_mut_ptr().cast::<u32>(), pixels)
        })
    }

    /// Shows the pixels inside `rects` (clipped to the framebuffer, empty ones skipped) on the
    /// display: each rectangle's transfer to the device, then, once all are answered, each
    /// one's flush, polling while `keep_waiting` says to. Returns how many rectangles were
    /// shown. When the device does not answer in time, it is left with commands it may still
    /// process, and every later command fails with [`Error::TimedOut`] until [`Self::reset`].
    pub fn flush(
        &mut self,
        rects: &[Rect],
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<usize, Error> {
        let mode = self.mode().ok_or(Error::NoScanout)?;
        let resource = self.shown().map_or(RESOURCE_ID, |scanout| scanout.resource);
        let batch = MAX_BATCH.min(usize::from(self.queue.size() / 2));
        let mut shown = 0;
        let mut clipped = rects
            .iter()
            .map(|rect| rect.clipped(mode.width, mode.height))
            .filter(|rect| !rect.is_empty())
            .peekable();
        while clipped.peek().is_some() {
            let mut chunk = [Rect::default(); MAX_BATCH];
            let mut count = 0;
            for (slot, rect) in chunk.iter_mut().take(batch).zip(clipped.by_ref()) {
                *slot = rect;
                count += 1;
            }
            for (index, rect) in chunk[..count].iter().enumerate() {
                let request = self.header(index, command::TRANSFER_TO_HOST_2D);
                self.put_rect(request + 24, *rect);
                let offset = rect.y as u64 * mode.stride() as u64
                    + u64::from(rect.x) * u64::from(BYTES_PER_PIXEL);
                self.put64(request + 40, offset);
                self.put32(request + 48, resource);
                self.put32(request + 52, 0);
            }
            self.run(count, TRANSFER_TO_HOST_2D_BYTES, keep_waiting)?;
            for (index, rect) in chunk[..count].iter().enumerate() {
                let request = self.header(index, command::RESOURCE_FLUSH);
                self.put_rect(request + 24, *rect);
                self.put32(request + 40, resource);
                self.put32(request + 44, 0);
            }
            self.run(count, RESOURCE_FLUSH_BYTES, keep_waiting)?;
            shown += count;
        }
        Ok(shown)
    }

    /// Posts the flush of `rects` without waiting (D414): each rectangle's transfer, then each
    /// one's flush, in one batch, the rectangles past what one batch holds joined into their
    /// bounding box; [`Self::finish_flush`] collects the answers, so the caller may sleep on the
    /// device's interrupt in between. Returns how many rectangles it posted (0 when none shows).
    pub fn start_flush(&mut self, rects: &[Rect]) -> Result<usize, Error> {
        if self.started.is_some() {
            return Err(Error::TimedOut);
        }
        let mode = self.mode().ok_or(Error::NoScanout)?;
        let resource = self.shown().map_or(RESOURCE_ID, |scanout| scanout.resource);
        let room = (MAX_BATCH / 2)
            .min(usize::from(self.queue.size() / 4))
            .max(1);
        let mut chunk = [Rect::default(); MAX_BATCH];
        let mut count = 0;
        for rect in rects
            .iter()
            .map(|rect| rect.clipped(mode.width, mode.height))
            .filter(|rect| !rect.is_empty())
        {
            match count < room {
                true => {
                    chunk[count] = rect;
                    count += 1;
                }
                false => chunk[room - 1] = chunk[room - 1].union(rect),
            }
        }
        if count == 0 {
            return Ok(0);
        }
        for (index, rect) in chunk[..count].iter().enumerate() {
            let request = self.header(index, command::TRANSFER_TO_HOST_2D);
            self.put_rect(request + 24, *rect);
            let offset = rect.y as u64 * mode.stride() as u64
                + u64::from(rect.x) * u64::from(BYTES_PER_PIXEL);
            self.put64(request + 40, offset);
            self.put32(request + 48, resource);
            self.put32(request + 52, 0);
        }
        for (index, rect) in chunk[..count].iter().enumerate() {
            let request = self.header(count + index, command::RESOURCE_FLUSH);
            self.put_rect(request + 24, *rect);
            self.put32(request + 40, resource);
            self.put32(request + 44, 0);
        }
        for index in 0..2 * count {
            let response = RESPONSES + index * RESPONSE_SLOT;
            self.put32(response, 0);
            let bytes = match index < count {
                true => TRANSFER_TO_HOST_2D_BYTES,
                false => RESOURCE_FLUSH_BYTES,
            };
            let posted = self.post(
                REQUESTS + index * REQUEST_SLOT,
                bytes,
                response,
                HEADER_BYTES,
            );
            if let Err(error) = posted {
                self.unanswered |= index > 0;
                return Err(error);
            }
        }
        if self.queue.should_notify() {
            self.transport.notify(CONTROL_QUEUE);
        }
        self.started = Some(StartedFlush {
            commands: 2 * count,
            answered: 0,
            rects: count,
        });
        Ok(count)
    }

    /// Collects the answers of the flush [`Self::start_flush`] posted: `Some` with its count of
    /// rectangles once every command is answered, `None` while some are not.
    pub fn finish_flush(&mut self) -> Result<Option<usize>, Error> {
        if self.started.is_none() {
            return Ok(Some(0));
        }
        self.collect();
        let Some(started) = self.started else {
            return Ok(Some(0));
        };
        if started.answered < started.commands {
            return Ok(None);
        }
        self.started = None;
        for index in 0..started.commands {
            let answer = self.get32(RESPONSES + index * RESPONSE_SLOT);
            if answer != response::OK_NODATA {
                return Err(Error::CommandFailed(answer));
            }
        }
        Ok(Some(started.rects))
    }

    /// Gives up on a started flush that was never answered: the device is treated as wedged,
    /// as when a wait times out.
    pub fn abandon_flush(&mut self) {
        if self.started.take().is_some() {
            self.unanswered = true;
        }
    }

    /// Has the device interrupt when it answers a command on the control queue, once the
    /// kernel connected its interrupt (D414).
    pub fn interrupt_on_answers(&mut self) {
        self.queue.enable_used_notifications();
    }

    /// Whether a flush is posted and not yet collected.
    pub fn flush_started(&self) -> bool {
        self.started.is_some()
    }

    /// Whether the device takes 3D commands (`VIRTIO_GPU_F_VIRGL`).
    pub fn takes_three_d(&self) -> bool {
        self.features & three_d::FEATURE_VIRGL != 0
    }

    /// Whether the device takes blob resources (`VIRTIO_GPU_F_RESOURCE_BLOB`, D439).
    pub fn takes_blobs(&self) -> bool {
        self.features & three_d::FEATURE_RESOURCE_BLOB != 0
    }

    /// Whether a context is created for one capability set (`VIRTIO_GPU_F_CONTEXT_INIT`).
    pub fn takes_context_init(&self) -> bool {
        self.features & three_d::FEATURE_CONTEXT_INIT != 0
    }

    /// How many capability sets the device describes ([`ThreeDCommand::CapsetInfo`]); 0 when it
    /// takes no 3D commands.
    pub fn capset_count(&self) -> u32 {
        match self.takes_three_d() {
            true => self.transport.read_config32(CONFIG_NUM_CAPSETS_OFFSET),
            false => 0,
        }
    }

    /// Gives the driver `memory` ([`THREE_D_BYTES`] or more) for the 3D commands, their answers
    /// and the command streams; handed back when the device takes no 3D commands, memory is
    /// attached already or it is too small. The device reaches it only while a 3D command or a
    /// command stream is in flight; it comes back from [`Self::reset`].
    pub fn attach_three_d(&mut self, memory: M) -> Result<(), (Error, M)> {
        if !self.takes_three_d() || self.three_d.is_some() {
            return Err((Error::NoThreeD, memory));
        }
        if memory.len() < THREE_D_BYTES {
            return Err((Error::MemoryTooSmall, memory));
        }
        self.three_d = Some(memory);
        Ok(())
    }

    /// A stream slot no command stream is in flight in, after collecting the answers that came:
    /// `None` while all [`STREAM_SLOTS`] are, or before [`Self::attach_three_d`].
    pub fn free_stream(&mut self) -> Option<usize> {
        self.three_d.as_ref()?;
        self.collect();
        self.posted_streams.iter().position(Option::is_none)
    }

    /// Where the command stream of `slot` is written before [`Self::start_stream`] posts it,
    /// [`COMMAND_STREAM_BYTES`] long; `None` before [`Self::attach_three_d`], for a slot past
    /// [`STREAM_SLOTS`], and while the slot's stream is in flight.
    pub fn stream(&mut self, slot: usize) -> Option<&mut [u8]> {
        if self.posted_streams.get(slot)?.is_some() {
            return None;
        }
        let memory = self.three_d.as_mut()?;
        // SAFETY: the memory holds at least `THREE_D_BYTES` (checked when attached), owned by
        // this driver while it lives, and slot `slot`'s stream lies inside it; the device reads
        // the slot only while a stream posted from it is in flight, which this borrow excludes
        // (checked above, and posting takes `&mut self`).
        Some(unsafe {
            core::slice::from_raw_parts_mut(
                memory
                    .as_mut_ptr()
                    .wrapping_add(THREE_D_STREAM + slot * COMMAND_STREAM_BYTES),
                COMMAND_STREAM_BYTES,
            )
        })
    }

    /// Posts the first `length` bytes of `slot`'s stream ([`Self::stream`]) as `context`'s
    /// command stream without waiting, and returns its fence: the device answers it once it
    /// finished it, in any order with the other streams ([`Self::oldest_stream`]). Refused
    /// while the slot is in flight and while the device is treated as wedged.
    pub fn start_stream(&mut self, slot: usize, context: u32, length: u32) -> Result<u64, Error> {
        if self.unanswered {
            return Err(Error::TimedOut);
        }
        if length as usize > COMMAND_STREAM_BYTES {
            return Err(Error::MemoryTooSmall);
        }
        match self.posted_streams.get(slot) {
            Some(None) => {}
            Some(Some(_)) => return Err(Error::QueueFull),
            None => return Err(Error::QueueUnavailable),
        }
        let (pointer, base) = match self.three_d.as_ref() {
            Some(memory) => (memory.as_mut_ptr(), memory.dma_addr()),
            None => return Err(Error::NoThreeD),
        };
        self.fence_id = self.fence_id.wrapping_add(1);
        let fence = self.fence_id;
        let mut request = [0u8; three_d::LONGEST_REQUEST];
        let request_bytes = three_d::encode(
            &ThreeDCommand::Submit { context, length },
            fence,
            &mut request,
        );
        let header = STREAM_HEADERS + slot * STREAM_HEADER;
        // SAFETY: the slot's header lies inside the memory, which holds at least
        // `THREE_D_BYTES`, owned by this driver; the device does not own it now, as no stream
        // of this slot is in flight (checked above); the request fits before the answer
        // (asserted).
        unsafe {
            core::ptr::copy_nonoverlapping(
                request.as_ptr(),
                pointer.wrapping_add(header),
                request_bytes,
            )
        };
        // SAFETY: as above: the answer's first word lies inside the same memory.
        unsafe {
            pointer
                .wrapping_add(header + STREAM_ANSWER)
                .cast::<u32>()
                .write_volatile(0)
        };
        let at = |offset: usize| base.checked_add(offset as u64).ok_or(Error::MemoryTooSmall);
        let head = self.queue.submit(&[
            Buffer {
                addr: at(header)?,
                len: request_bytes as u32,
                device_writes: false,
            },
            Buffer {
                addr: at(THREE_D_STREAM + slot * COMMAND_STREAM_BYTES)?,
                len: length,
                device_writes: false,
            },
            Buffer {
                addr: at(header + STREAM_ANSWER)?,
                len: three_d::HEADER_BYTES as u32,
                device_writes: true,
            },
        ])?;
        self.posted_streams[slot] = Some(PostedStream { head, fence });
        if self.queue.should_notify() {
            self.transport.notify(CONTROL_QUEUE);
        }
        Ok(fence)
    }

    /// Collects the answers that came and returns the fence of the oldest command stream still
    /// in flight: every stream with an older fence is finished; `None` when none is in flight.
    pub fn oldest_stream(&mut self) -> Option<u64> {
        self.collect();
        self.posted_streams
            .iter()
            .flatten()
            .map(|posted| posted.fence)
            .min()
    }

    /// How many command streams the device answered with an error: their slots are freed and
    /// their fences pass like the others'.
    pub fn refused_streams(&self) -> u64 {
        self.refused_streams
    }

    /// Gives up on the command streams the device never answered: it is treated as wedged, as
    /// when a wait times out.
    pub fn abandon_streams(&mut self) {
        if self.posted_streams.iter().any(Option::is_some) {
            self.posted_streams = [None; STREAM_SLOTS];
            self.unanswered = true;
        }
    }

    /// Whether a command stream is in flight.
    pub fn streams_in_flight(&self) -> bool {
        self.posted_streams.iter().any(Option::is_some)
    }

    fn collect(&mut self) -> usize {
        let mut others = 0;
        while let Some(used) = self.queue.pop_used() {
            let stream = self
                .posted_streams
                .iter()
                .position(|posted| posted.is_some_and(|posted| posted.head == used.head));
            if let Some(slot) = stream {
                self.posted_streams[slot] = None;
                let answer =
                    self.three_d_word(STREAM_HEADERS + slot * STREAM_HEADER + STREAM_ANSWER);
                if answer != three_d::answer::OK_NODATA {
                    self.refused_streams += 1;
                }
                continue;
            }
            match (self.posted_three_d.as_mut(), self.started.as_mut()) {
                (Some(posted), _) if posted.head == used.head && !posted.answered => {
                    posted.answered = true;
                }
                (_, Some(started)) => started.answered += 1,
                _ => others += 1,
            }
        }
        others
    }

    /// Posts `command` on the control queue without waiting (D438); [`Self::finish_three_d`]
    /// collects the answer, so the caller may sleep on the device's interrupt meanwhile. Refused
    /// while a flush or another 3D command is posted, and for a [`ThreeDCommand::Submit`], which
    /// goes through [`Self::start_stream`] (D460).
    pub fn start_three_d(&mut self, command: &ThreeDCommand<'_>) -> Result<(), Error> {
        if self.unanswered {
            return Err(Error::TimedOut);
        }
        if self.started.is_some() || self.posted_three_d.is_some() {
            return Err(Error::QueueFull);
        }
        let (pointer, base) = match self.three_d.as_ref() {
            Some(memory) => (memory.as_mut_ptr(), memory.dma_addr()),
            None => return Err(Error::NoThreeD),
        };
        if let ThreeDCommand::Submit { .. } = *command {
            return Err(Error::QueueUnavailable);
        }
        let capset_room = match *command {
            ThreeDCommand::Capset { .. } => CAPSET_ROOM,
            _ => 0,
        };
        let answer_bytes = three_d::answer_bytes(command, capset_room);
        self.fence_id = self.fence_id.wrapping_add(1);
        let mut request = [0u8; three_d::LONGEST_REQUEST];
        let request_bytes = three_d::encode(command, self.fence_id, &mut request);
        // SAFETY: the memory holds at least `THREE_D_BYTES`, owned by this driver; the request
        // fits before `THREE_D_ANSWER` (asserted), and the device does not own it now, as no 3D
        // command is in flight between calls.
        unsafe {
            core::ptr::copy_nonoverlapping(
                request.as_ptr(),
                pointer.wrapping_add(THREE_D_REQUEST),
                request_bytes,
            )
        };
        // SAFETY: as above: the answer's first word lies inside the same memory.
        unsafe {
            pointer
                .wrapping_add(THREE_D_ANSWER)
                .cast::<u32>()
                .write_volatile(0)
        };
        let at = |offset: usize| base.checked_add(offset as u64).ok_or(Error::MemoryTooSmall);
        let request = Buffer {
            addr: at(THREE_D_REQUEST)?,
            len: request_bytes as u32,
            device_writes: false,
        };
        let answer = Buffer {
            addr: at(THREE_D_ANSWER)?,
            len: answer_bytes as u32,
            device_writes: true,
        };
        let head = self.queue.submit(&[request, answer])?;
        self.posted_three_d = Some(PostedThreeD {
            head,
            expected: three_d::expected_answer(command),
            answer_bytes,
            answered: false,
        });
        if self.queue.should_notify() {
            self.transport.notify(CONTROL_QUEUE);
        }
        Ok(())
    }

    /// Collects the answer of the command [`Self::start_three_d`] posted: `true` once it is
    /// answered (or when none is posted), `false` while it is not; an error when the device
    /// refused it.
    pub fn finish_three_d(&mut self) -> Result<bool, Error> {
        if self.posted_three_d.is_none() {
            return Ok(true);
        }
        self.collect();
        let Some(posted) = self.posted_three_d else {
            return Ok(true);
        };
        if !posted.answered {
            return Ok(false);
        }
        self.posted_three_d = None;
        self.answered_three_d = posted.answer_bytes;
        let answered = self.three_d_word(THREE_D_ANSWER);
        match answered == posted.expected {
            true => Ok(true),
            false => Err(Error::CommandFailed(answered)),
        }
    }

    /// Gives up on a posted 3D command the device never answered: the device is treated as
    /// wedged, as when a wait times out.
    pub fn abandon_three_d(&mut self) {
        if self.posted_three_d.take().is_some() {
            self.unanswered = true;
        }
    }

    /// Whether a 3D command is posted and not yet collected.
    pub fn three_d_posted(&self) -> bool {
        self.posted_three_d.is_some()
    }

    /// The data past the header of the last 3D command's answer: a capability set's
    /// description or bytes; empty for the others.
    pub fn three_d_answer(&self) -> &[u8] {
        let Some(memory) = self.three_d.as_ref() else {
            return &[];
        };
        let length = self.answered_three_d.saturating_sub(three_d::HEADER_BYTES);
        // SAFETY: the answer's data lies inside the memory (`answer_bytes` is at most
        // `THREE_D_STREAM - THREE_D_ANSWER`), which the device handed back with its answer; the
        // borrow of `self` keeps the next command from overwriting it.
        unsafe {
            core::slice::from_raw_parts(
                memory
                    .as_mut_ptr()
                    .wrapping_add(THREE_D_ANSWER + three_d::HEADER_BYTES),
                length,
            )
        }
    }

    /// Sends `command` and waits for its answer, polling while `keep_waiting` says to (see
    /// [`Self::start_three_d`]); returns [`Self::three_d_answer`]. A device that does not
    /// answer in time is treated as in [`Self::flush`].
    pub fn run_three_d(
        &mut self,
        command: &ThreeDCommand<'_>,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<&[u8], Error> {
        self.start_three_d(command)?;
        while !self.finish_three_d()? {
            if !keep_waiting() {
                self.abandon_three_d();
                return Err(Error::TimedOut);
            }
        }
        Ok(self.three_d_answer())
    }

    fn three_d_word(&self, offset: usize) -> u32 {
        let Some(memory) = self.three_d.as_ref() else {
            return 0;
        };
        let field = memory.as_mut_ptr().wrapping_add(offset).cast::<u32>();
        // SAFETY: a 4-byte aligned field of the 3D memory, which the device has handed back.
        u32::from_le(unsafe { field.read_volatile() })
    }

    /// Resets the device and hands back the transport, the control memory and the framebuffer
    /// if one was attached, with the proof that the device no longer uses them. If the reset
    /// does not read back 0, the memory is leaked and only the transport comes back.
    pub fn reset(
        self,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<Released<T, M>, (Error, T)> {
        let Self {
            transport,
            control,
            scanout,
            cursor,
            three_d,
            ..
        } = self;
        let framebuffer = scanout.map(|scanout| scanout.framebuffer);
        let cursor = cursor.map(|cursor| cursor.image);
        match super::reset(&transport, keep_waiting) {
            Ok(reset) => Ok(Released {
                transport,
                control,
                framebuffer,
                cursor,
                three_d,
                reset,
            }),
            Err(error) => {
                core::mem::forget(control);
                core::mem::forget(framebuffer);
                core::mem::forget(cursor);
                core::mem::forget(three_d);
                Err((error, transport))
            }
        }
    }

    fn query_display(&mut self, keep_waiting: &mut dyn FnMut() -> bool) -> Result<Mode, Error> {
        self.header(0, command::GET_DISPLAY_INFO);
        self.put32(DISPLAY_INFO, 0);
        self.post(REQUESTS, HEADER_BYTES, DISPLAY_INFO, DISPLAY_INFO_BYTES)?;
        self.wait(1, keep_waiting)?;
        if self.get32(DISPLAY_INFO) != response::OK_DISPLAY_INFO {
            return Ok(FALLBACK_MODE);
        }
        let first = DISPLAY_INFO + HEADER_BYTES;
        let width = self.get32(first + 8);
        let height = self.get32(first + 12);
        let enabled = self.get32(first + 16) != 0;
        Ok(match enabled && width > 0 && height > 0 {
            true => Mode::new(width, height),
            false => FALLBACK_MODE,
        })
    }

    fn run(
        &mut self,
        count: usize,
        request_bytes: usize,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        for index in 0..count {
            let response = RESPONSES + index * RESPONSE_SLOT;
            self.put32(response, 0);
            let posted = self.post(
                REQUESTS + index * REQUEST_SLOT,
                request_bytes,
                response,
                HEADER_BYTES,
            );
            if let Err(error) = posted {
                self.unanswered |= index > 0;
                return Err(error);
            }
        }
        self.wait(count, keep_waiting)?;
        for index in 0..count {
            let answer = self.get32(RESPONSES + index * RESPONSE_SLOT);
            if answer != response::OK_NODATA {
                return Err(Error::CommandFailed(answer));
            }
        }
        Ok(())
    }

    fn post(
        &mut self,
        request: usize,
        request_bytes: usize,
        response: usize,
        response_bytes: usize,
    ) -> Result<(), Error> {
        if self.unanswered {
            return Err(Error::TimedOut);
        }
        if self.posted_three_d.is_some() {
            return Err(Error::QueueFull);
        }
        let at = |offset: usize| {
            self.control
                .dma_addr()
                .checked_add(offset as u64)
                .ok_or(Error::MemoryTooSmall)
        };
        let chain = [
            Buffer {
                addr: at(request)?,
                len: request_bytes as u32,
                device_writes: false,
            },
            Buffer {
                addr: at(response)?,
                len: response_bytes as u32,
                device_writes: true,
            },
        ];
        self.queue.submit(&chain).map(|_| ())
    }

    fn wait(&mut self, count: usize, keep_waiting: &mut dyn FnMut() -> bool) -> Result<(), Error> {
        if self.queue.should_notify() {
            self.transport.notify(CONTROL_QUEUE);
        }
        let mut answered = 0;
        while answered < count {
            let collected = self.collect();
            answered += collected;
            if collected == 0 && answered < count && !keep_waiting() {
                self.unanswered = true;
                return Err(Error::TimedOut);
            }
        }
        Ok(())
    }

    fn put_backing(&self, request: usize, memory: &M) -> Option<usize> {
        let mut count = 0usize;
        let mut fits = true;
        memory.each_piece(&mut |address, length| {
            let entry = request + ATTACH_BACKING_ENTRIES + count * BACKING_ENTRY_BYTES;
            let Ok(length) = u32::try_from(length) else {
                fits = false;
                return;
            };
            if entry + BACKING_ENTRY_BYTES > RESPONSES {
                fits = false;
                return;
            }
            self.put64(entry, address.get());
            self.put32(entry + 8, length);
            self.put32(entry + 12, 0);
            count += 1;
        });
        if !fits || count == 0 {
            return None;
        }
        self.put32(request + 28, count as u32);
        Some(ATTACH_BACKING_ENTRIES + count * BACKING_ENTRY_BYTES)
    }

    fn header(&mut self, slot: usize, command: u32) -> usize {
        let request = REQUESTS + slot * REQUEST_SLOT;
        self.fence_id = self.fence_id.wrapping_add(1);
        self.put32(request, command);
        self.put32(request + 4, FLAG_FENCE);
        self.put64(request + 8, self.fence_id);
        self.put32(request + 16, 0);
        self.put32(request + 20, 0);
        request
    }

    fn put_rect(&self, offset: usize, rect: Rect) {
        self.put32(offset, rect.x);
        self.put32(offset + 4, rect.y);
        self.put32(offset + 8, rect.width);
        self.put32(offset + 12, rect.height);
    }

    fn put32(&self, offset: usize, value: u32) {
        let field = self.control.as_mut_ptr().wrapping_add(offset).cast::<u32>();
        // SAFETY: a 4-byte aligned field below `MEMORY_BYTES` of the control memory, past the
        // queue, which only this driver writes while the device does not own it.
        unsafe { field.write_volatile(value.to_le()) };
    }

    fn put64(&self, offset: usize, value: u64) {
        let field = self.control.as_mut_ptr().wrapping_add(offset).cast::<u64>();
        // SAFETY: as in `put32`, 8-byte aligned.
        unsafe { field.write_volatile(value.to_le()) };
    }

    fn get32(&self, offset: usize) -> u32 {
        let field = self.control.as_mut_ptr().wrapping_add(offset).cast::<u32>();
        // SAFETY: a 4-byte aligned response field of the control memory, which the device has
        // handed back.
        u32::from_le(unsafe { field.read_volatile() })
    }
}

fn cursor_queue_of<T: Transport, M: DmaMemory>(transport: &T, control: &M) -> Option<SplitQueue> {
    let max = transport
        .max_queue_size(CURSOR_QUEUE)
        .min(CURSOR_QUEUE_SIZE);
    if max == 0 {
        return None;
    }
    let size = 1 << max.ilog2();
    let memory = control.as_mut_ptr().wrapping_add(CURSOR_RING);
    let dma = control.dma_addr().checked_add(CURSOR_RING as u64)?;
    // SAFETY: the control memory holds at least `MEMORY_BYTES`, so the bytes from `CURSOR_RING`
    // hold the queue's `memory_size(CURSOR_QUEUE_SIZE)` before `CURSOR_REQUEST`, 16-byte aligned
    // (`CURSOR_RING` is a multiple of 16 of 4096-aligned memory); the driver keeps the memory for
    // as long as the queue lives and nothing else uses those bytes.
    let mut queue = unsafe { SplitQueue::new(memory, dma, size) }.ok()?;
    queue.disable_used_notifications();
    transport
        .set_up_queue(CURSOR_QUEUE, size, &queue.addresses())
        .ok()?;
    Some(queue)
}
