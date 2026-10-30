//! The virtio GPU device (virtio 1.2, 5.7), 2D and polled: the control queue, one scanout
//! showing one resource whose backing is the driver's framebuffer in guest memory, and the cursor
//! queue, which shows a 64x64 cursor image over the scanout and moves it without any redraw
//! (D186).
//!
//! Every command is fenced (`VIRTIO_GPU_FLAG_FENCE`), so its answer means the device finished
//! it: the spec lets a device answer an unfenced command before processing it and does not
//! promise to process commands in order. A flush therefore sends the damaged rectangles'
//! transfers, waits for their answers, then sends their flushes.

use super::queue::{Buffer, SplitQueue};
use super::{Error, ResetDone, Transport, device_type};
use crate::display::{BYTES_PER_PIXEL, Mode, Rect};
use crate::dma::DmaMemory;

#[cfg(test)]
mod tests;

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
const SCANOUT_ID: u32 = 0;
const CONFIG_NUM_SCANOUTS_OFFSET: usize = 8;
const FLAG_FENCE: u32 = 1;

const HEADER_BYTES: usize = 24;
const DISPLAY_INFO_BYTES: usize = HEADER_BYTES + 16 * 24;
const CREATE_2D_BYTES: usize = 40;
const ATTACH_BACKING_BYTES: usize = 48;
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

mod command {
    pub const GET_DISPLAY_INFO: u32 = 0x0100;
    pub const RESOURCE_CREATE_2D: u32 = 0x0101;
    pub const SET_SCANOUT: u32 = 0x0103;
    pub const RESOURCE_FLUSH: u32 = 0x0104;
    pub const TRANSFER_TO_HOST_2D: u32 = 0x0105;
    pub const RESOURCE_ATTACH_BACKING: u32 = 0x0106;
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
    pub reset: ResetDone,
}

#[derive(Debug)]
struct Scanout<M> {
    framebuffer: M,
    mode: Mode,
    shown: bool,
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
        if let Err(error) = super::initialize(&transport, 0, keep_waiting) {
            return Err((error, transport, Some(control)));
        }
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
        let Ok(length) = u32::try_from(mode.bytes()) else {
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
        self.put32(request + 28, 1);
        self.put64(request + 32, framebuffer.dma_addr().get());
        self.put32(request + 40, length);
        self.put32(request + 44, 0);
        self.scanout = Some(Scanout {
            framebuffer,
            mode,
            shown: false,
        });
        self.run(1, ATTACH_BACKING_BYTES, keep_waiting)
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
                self.put32(request + 48, RESOURCE_ID);
                self.put32(request + 52, 0);
            }
            self.run(count, TRANSFER_TO_HOST_2D_BYTES, keep_waiting)?;
            for (index, rect) in chunk[..count].iter().enumerate() {
                let request = self.header(index, command::RESOURCE_FLUSH);
                self.put_rect(request + 24, *rect);
                self.put32(request + 40, RESOURCE_ID);
                self.put32(request + 44, 0);
            }
            self.run(count, RESOURCE_FLUSH_BYTES, keep_waiting)?;
            shown += count;
        }
        Ok(shown)
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
                reset,
            }),
            Err(error) => {
                core::mem::forget(control);
                core::mem::forget(framebuffer);
                core::mem::forget(cursor);
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
            if self.queue.pop_used().is_some() {
                answered += 1;
            } else if !keep_waiting() {
                self.unanswered = true;
                return Err(Error::TimedOut);
            }
        }
        Ok(())
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
