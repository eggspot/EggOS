//! Accelerated rendering (D415): rendering contexts on a GPU that takes 3D commands (virtio
//! GPU with `VIRTIO_GPU_F_VIRGL`), for user-space drivers that encode their own command
//! streams (Mesa's virgl for OpenGL, Venus for Vulkan). Each opener of the render node gets a
//! [`Context`]; its buffers are 3D resources backed by DMA memory the program may map. A
//! command stream is posted in one of the device's stream slots and the call returns its fence
//! (D460): several streams are in flight at once, and a fence passes once every stream posted
//! up to it is finished. Each buffer keeps the fence of the last stream that used it, which
//! [`Buffer::wait`] and the buffer's release wait for; [`Fence`] is a fence a program holds as
//! a file. Waits sleep on the console's input key, which the GPU's interrupt wakes, so any
//! number of threads wait at once. Every other command waits for its own answer. The GPU is
//! the display's device, reached through [`crate::display::with_renderer`].

use eggos_hal::atomic::AtomicU64;
use crate::devices::Deadline;
use crate::devices::dma::DmaRegion;
use crate::devices::interrupts::DeviceEvents;
use crate::linux::Errno;
use crate::process::LentFrames;
use crate::sync::SpinLock;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::mem::ManuallyDrop;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};
pub use eggos_drivers::virtio::gpu::three_d::{
    CapsetInfo, ResourceShape, TexelBox, ThreeDCommand, Transfer, capset_info,
};
use eggos_drivers::dma::DmaMemory;
use eggos_hal::Platform;
use eggos_hal::memory::FRAME_SIZE;

/// The most capability sets a device is asked about.
pub const MOST_CAPSETS: u32 = 16;
/// The most bytes of capability set one call reads.
pub const CAPSET_ROOM: usize = eggos_drivers::virtio::gpu::CAPSET_ROOM;
/// The most bytes of command stream one submission carries.
pub const COMMAND_STREAM_BYTES: usize = eggos_drivers::virtio::gpu::COMMAND_STREAM_BYTES;
const FIRST_RESOURCE: u32 = 0x100;
const DEBUG_NAME: &[u8] = b"eggos";
const FENCE_TIMEOUT_MS: u64 = 5000;
const NO_STREAM_IN_FLIGHT: u64 = u64::MAX;

/// A GPU that takes 3D commands, as the display's driver offers it.
pub trait Renderer {
    /// How many capability sets the device describes.
    fn capset_count(&self) -> u32;
    /// Whether a context is created for one capability set.
    fn takes_context_init(&self) -> bool;
    /// Whether the device takes blob resources (D439).
    fn takes_blobs(&self) -> bool;
    /// A stream slot no command stream is in flight in, after collecting the answers that came
    /// (D460); `None` while every slot is in flight.
    fn free_stream(&mut self) -> Option<usize>;
    /// Where `slot`'s command stream is written before [`Self::start_stream`]; empty for a slot
    /// in flight.
    fn stream(&mut self, slot: usize) -> &mut [u8];
    /// Posts the first `length` bytes of `slot`'s stream as `context`'s command stream without
    /// waiting, and returns its fence.
    fn start_stream(
        &mut self,
        slot: usize,
        context: u32,
        length: u32,
    ) -> Result<u64, &'static str>;
    /// Collects the answers that came: the fence of the oldest stream still in flight.
    fn oldest_stream(&mut self) -> Option<u64>;
    /// Gives up on the streams the device never answered.
    fn abandon_streams(&mut self);
    /// Sends `command` and waits for its answer until `deadline`; copies the answer's data
    /// into `answer` and says how many bytes it copied.
    fn run(
        &mut self,
        command: &ThreeDCommand<'_>,
        answer: &mut [u8],
        deadline: &mut Deadline,
    ) -> Result<usize, &'static str>;
    /// The events the device's interrupt raises, when it has one connected.
    fn events(&self) -> Option<&'static DeviceEvents>;
}

/// Why a rendering call failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderError {
    /// No GPU that takes 3D commands.
    NoDevice,
    /// No memory for the buffer or the copy.
    NoMemory,
    /// The device refused the command or did not answer.
    Device,
    /// The call names no buffer of the context, or asks what cannot be.
    Invalid,
    /// The context was created already.
    AlreadyCreated,
    /// The GPU is still running what was posted.
    Busy,
}

static PRESENT: AtomicBool = AtomicBool::new(false);
static NEXT_CONTEXT: AtomicU32 = AtomicU32::new(1);
static NEXT_RESOURCE: AtomicU32 = AtomicU32::new(FIRST_RESOURCE);
static OLDEST_IN_FLIGHT: AtomicU64 = AtomicU64::new(NO_STREAM_IN_FLIGHT);
static COLLECTED_AT: AtomicU64 = AtomicU64::new(0);
static EVENTS: AtomicPtr<DeviceEvents> = AtomicPtr::new(core::ptr::null_mut());

/// Says a GPU that takes 3D commands is the display's device, so the render node is listed.
pub fn set_present() {
    PRESENT.store(true, Ordering::Release);
}

/// Whether a GPU takes 3D commands.
pub fn is_present() -> bool {
    PRESENT.load(Ordering::Acquire)
}

fn run<P: Platform>(command: &ThreeDCommand<'_>, answer: &mut [u8]) -> Result<usize, RenderError> {
    crate::display::with_renderer::<P, _>(|renderer, deadline| {
        renderer.run(command, answer, deadline)
    })
    .map_err(|_| RenderError::NoDevice)?
    .map_err(|_| RenderError::Device)
}

fn command<P: Platform>(command: &ThreeDCommand<'_>) -> Result<(), RenderError> {
    run::<P>(command, &mut []).map(|_| ())
}

fn send_quietly<P: Platform>(sent: &ThreeDCommand<'_>) -> bool {
    command::<P>(sent).is_ok()
}

/// The capability sets the device describes.
pub fn capsets<P: Platform>() -> Result<Vec<CapsetInfo>, RenderError> {
    let count = crate::display::with_renderer::<P, _>(|renderer, _| renderer.capset_count())
        .map_err(|_| RenderError::NoDevice)?;
    let mut found = Vec::new();
    for index in 0..count.min(MOST_CAPSETS) {
        let mut data = [0u8; 16];
        let length = run::<P>(&ThreeDCommand::CapsetInfo { index }, &mut data)?;
        if let Some(info) = capset_info(&data[..length]) {
            found.push(info);
        }
    }
    Ok(found)
}

/// Whether the GPU takes blob resources (D439).
pub fn takes_blobs<P: Platform>() -> bool {
    crate::display::with_renderer::<P, _>(|renderer, _| renderer.takes_blobs()).unwrap_or(false)
}

/// Whether contexts are created for one capability set.
pub fn takes_context_init<P: Platform>() -> bool {
    crate::display::with_renderer::<P, _>(|renderer, _| renderer.takes_context_init())
        .unwrap_or(false)
}

fn events() -> Option<&'static DeviceEvents> {
    let events = EVENTS.load(Ordering::Acquire);
    // SAFETY: only ever set from a `&'static DeviceEvents` in `note_collected`.
    unsafe { events.cast_const().as_ref() }
}

fn note_collected(renderer: &mut dyn Renderer) -> Option<u64> {
    let events = renderer.events();
    if let Some(events) = events {
        EVENTS.store(core::ptr::from_ref(events).cast_mut(), Ordering::Release);
        COLLECTED_AT.store(events.raised(), Ordering::SeqCst);
    }
    let oldest = renderer.oldest_stream();
    OLDEST_IN_FLIGHT.store(oldest.unwrap_or(NO_STREAM_IN_FLIGHT), Ordering::SeqCst);
    oldest
}

/// Whether the command streams posted up to `fence` are finished, as last collected (D460);
/// fence 0 stands for no stream and has always passed.
pub fn fence_passed(fence: u64) -> bool {
    fence < OLDEST_IN_FLIGHT.load(Ordering::SeqCst)
}

/// Whether `fence` passed or the GPU answered something since its answers were last
/// collected, so a waiter looks again: what a wait checks under the futex bucket's lock, so it
/// only reads atomics.
pub fn fence_may_have_passed(fence: u64) -> bool {
    fence_passed(fence)
        || events().is_some_and(|events| events.raised() != COLLECTED_AT.load(Ordering::SeqCst))
}

/// Whether the GPU's interrupt wakes fence waiters; without one, a wait polls.
pub fn wakes_fence_waiters() -> bool {
    events().is_some()
}

/// Collects the GPU's answers, so [`fence_passed`] is current; the display's lock is taken
/// briefly and nothing is waited for.
pub fn collect<P: Platform>() {
    let looked = crate::display::with_renderer_now(|renderer| {
        note_collected(renderer);
    });
    if looked.is_ok()
        && let Some(events) = events()
    {
        events.looked_at::<P>();
    }
}

/// Waits until `fence` passed (D460): sleeps until the GPU's interrupt when it has one, polls
/// otherwise, and gives the streams up when the GPU does not answer in time. `nowait` only
/// looks: `Busy` while it has not passed.
pub fn wait_fence<P: Platform>(fence: u64, nowait: bool) -> Result<(), RenderError> {
    let ticks = eggos_hal::time::mul_div_ceil(FENCE_TIMEOUT_MS, P::clock_frequency(), 1000);
    let end = P::clock_ticks().saturating_add(ticks);
    loop {
        if fence_passed(fence) {
            return Ok(());
        }
        collect::<P>();
        if fence_passed(fence) {
            return Ok(());
        }
        if nowait {
            return Err(RenderError::Busy);
        }
        if P::clock_ticks() >= end {
            let _ = crate::display::with_renderer_now(|renderer| {
                renderer.abandon_streams();
                note_collected(renderer);
            });
            return Err(RenderError::Device);
        }
        match events() {
            Some(events) => {
                events.wake_input_waiters_too();
                let _ = crate::linux::futex::wait::<P>(
                    crate::console::INPUT_KEY,
                    u32::MAX,
                    end,
                    || match fence_may_have_passed(fence) {
                        true => Err(Errno::EAGAIN),
                        false => Ok(()),
                    },
                );
            }
            None => crate::thread::yield_now::<P>(),
        }
    }
}

/// A fence a program holds as a file (`VIRTGPU_EXECBUF_FENCE_FD_OUT`, a Linux sync file):
/// readable once every command stream posted up to it is finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fence {
    sequence: u64,
}

impl Fence {
    /// The fence of the stream that returned `sequence`.
    pub fn new(sequence: u64) -> Self {
        Self { sequence }
    }

    /// The fence that passes when both `self` and `other` passed.
    pub fn merged(self, other: Fence) -> Self {
        Self::new(self.sequence.max(other.sequence))
    }

    /// Its place in the GPU's single timeline.
    pub fn sequence(self) -> u64 {
        self.sequence
    }

    /// Whether it passed, after collecting the GPU's answers.
    pub fn has_passed<P: Platform>(self) -> bool {
        if !fence_passed(self.sequence) {
            collect::<P>();
        }
        fence_passed(self.sequence)
    }

    /// Whether it passed or may have, reading atomics only (see [`fence_may_have_passed`]).
    pub fn may_have_passed(self) -> bool {
        fence_may_have_passed(self.sequence)
    }

    /// Waits until it passed, as [`wait_fence`] does.
    pub fn wait<P: Platform>(self, nowait: bool) -> Result<(), RenderError> {
        wait_fence::<P>(self.sequence, nowait)
    }
}

/// Capability set `id` at `version` into `out`: how many bytes were written.
pub fn capset<P: Platform>(id: u32, version: u32, out: &mut [u8]) -> Result<usize, RenderError> {
    run::<P>(&ThreeDCommand::Capset { id, version }, out)
}

/// A 3D resource and the DMA memory behind it, which the program that made it may map.
#[derive(Debug)]
pub struct Buffer {
    resource: u32,
    physical: u64,
    length: u64,
    backed: bool,
    memory: Option<ManuallyDrop<DmaRegion>>,
    window_offset: Option<u64>,
    last_fence: AtomicU64,
    release: fn(u32, bool, u64) -> bool,
}

impl Buffer {
    /// The device's name for it.
    pub fn resource(&self) -> u32 {
        self.resource
    }

    /// Its size in bytes, whole frames.
    pub fn size(&self) -> u64 {
        self.length
    }

    /// Whether its memory is guest memory the kernel reaches (not a host blob's).
    pub fn is_in_guest_memory(&self) -> bool {
        self.memory.is_some()
    }

    /// The fence of the last command stream that used it (0 for none).
    pub fn last_fence(&self) -> u64 {
        self.last_fence.load(Ordering::Acquire)
    }

    /// Waits until the last command stream that used it is finished, as [`wait_fence`] does.
    pub fn wait<P: Platform>(&self, nowait: bool) -> Result<(), RenderError> {
        wait_fence::<P>(self.last_fence(), nowait)
    }

    fn used_by(&self, fence: u64) {
        self.last_fence.fetch_max(fence, Ordering::AcqRel);
    }
}

impl Buffer {
    /// Reads `out.len()` pixels from byte `offset` of the buffer's guest memory into `out`
    /// (D442); `false` for a host blob, whose memory is the device's, or a range past its end.
    pub fn read_pixels(&self, offset: u64, out: &mut [u32]) -> bool {
        let Some(memory) = self.memory.as_ref() else {
            return false;
        };
        let bytes = (out.len() as u64).saturating_mul(4);
        if offset.checked_add(bytes).is_none_or(|end| end > self.length) || offset % 4 != 0 {
            return false;
        }
        let from = memory.as_mut_ptr().wrapping_add(offset as usize).cast::<u32>();
        // SAFETY: the range lies inside the buffer's DMA memory (checked), which lives as long
        // as `self`, reached through the direct map and 4-byte aligned; the GPU or a program
        // may write it meanwhile, so the copy goes through raw pointers only.
        unsafe { core::ptr::copy_nonoverlapping(from, out.as_mut_ptr(), out.len()) };
        true
    }
}

impl LentFrames for Buffer {
    fn frame_at(&self, offset: u64) -> u64 {
        self.physical + offset
    }

    fn length(&self) -> u64 {
        self.length
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        let released = (self.release)(self.resource, self.backed, self.last_fence());
        if let Some(offset) = self.window_offset
            && released
        {
            give_back_window(offset, self.length);
        }
        let Some(memory) = self.memory.as_mut() else {
            return;
        };
        // SAFETY: taken once, here, as the buffer goes.
        let memory = unsafe { ManuallyDrop::take(memory) };
        match released {
            // SAFETY: the device detached the backing and destroyed the resource (or never had
            // the memory), so it no longer reaches it; no process maps it, as each mapping
            // holds the buffer.
            true => unsafe { memory.free() },
            false => core::mem::forget(memory),
        }
    }
}

fn release_host_blob<P: Platform>(resource: u32, _backed: bool, fence: u64) -> bool {
    if wait_fence::<P>(fence, false).is_err() {
        return false;
    }
    let unmapped = send_quietly::<P>(&ThreeDCommand::UnmapBlob {
        context: 0,
        resource,
    });
    send_quietly::<P>(&ThreeDCommand::Unref { resource }) && unmapped
}

/// The device's host-visible shared memory (D440): where it is in physical memory, how long,
/// and its free ranges.
#[derive(Debug)]
struct HostWindow {
    base: u64,
    free: Vec<(u64, u64)>,
}

static HOST_WINDOW: SpinLock<Option<HostWindow>> = SpinLock::new(None);

/// Says the GPU's host-visible shared memory is `length` bytes at physical `base`, so host
/// blobs can be mapped.
pub fn set_host_window(base: u64, length: u64) {
    *HOST_WINDOW.lock() = Some(HostWindow {
        base,
        free: alloc::vec![(0, length)],
    });
}

/// Whether host blobs can be mapped.
pub fn has_host_window() -> bool {
    HOST_WINDOW.lock().is_some()
}

fn take_window(size: u64) -> Option<(u64, u64)> {
    let mut window = HOST_WINDOW.lock();
    let window = window.as_mut()?;
    let at = window.free.iter().position(|&(_, length)| length >= size)?;
    let (offset, length) = window.free[at];
    match length == size {
        true => {
            window.free.remove(at);
        }
        false => window.free[at] = (offset + size, length - size),
    }
    Some((offset, window.base + offset))
}

fn give_back_window(offset: u64, size: u64) {
    if let Some(window) = HOST_WINDOW.lock().as_mut() {
        let at = window.free.partition_point(|&(start, _)| start < offset);
        window.free.insert(at, (offset, size));
        let mut index = 0;
        while index + 1 < window.free.len() {
            let (start, length) = window.free[index];
            let (next, next_length) = window.free[index + 1];
            if start + length == next {
                window.free[index] = (start, length + next_length);
                window.free.remove(index + 1);
            } else {
                index += 1;
            }
        }
    }
}

fn release_resource<P: Platform>(resource: u32, backed: bool, fence: u64) -> bool {
    if wait_fence::<P>(fence, false).is_err() {
        return false;
    }
    let detached = !backed || send_quietly::<P>(&ThreeDCommand::DetachBacking { resource });
    send_quietly::<P>(&ThreeDCommand::Unref { resource }) && detached
}

#[derive(Debug, Default)]
struct ContextState {
    created: bool,
    buffers: BTreeMap<u32, Arc<Buffer>>,
    next_handle: u32,
}

/// One opener's rendering context: its device context, made on first use or by
/// [`Context::initialize`], and its buffers by handle.
#[derive(Debug)]
pub struct Context {
    id: u32,
    state: SpinLock<ContextState>,
    end: fn(u32, &[u32], bool),
}

impl PartialEq for Context {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for Context {}

fn end_context<P: Platform>(context: u32, resources: &[u32], created: bool) {
    if !created {
        return;
    }
    for &resource in resources {
        send_quietly::<P>(&ThreeDCommand::DetachResource { context, resource });
    }
    send_quietly::<P>(&ThreeDCommand::DestroyContext { context });
}

impl Context {
    /// A new context, made on the device on first use; `NoDevice` without a GPU that takes
    /// 3D commands.
    pub fn open<P: Platform>() -> Result<Arc<Self>, RenderError> {
        if !is_present() {
            return Err(RenderError::NoDevice);
        }
        Ok(Arc::new(Self {
            id: NEXT_CONTEXT.fetch_add(1, Ordering::Relaxed),
            state: SpinLock::new(ContextState {
                next_handle: 1,
                ..ContextState::default()
            }),
            end: end_context::<P>,
        }))
    }

    /// Makes the device context for capability set `capset` (0 for the device's default),
    /// named `name`; `AlreadyCreated` once it exists.
    pub fn initialize<P: Platform>(&self, capset: u32, name: &[u8]) -> Result<(), RenderError> {
        if self.state.lock().created {
            return Err(RenderError::AlreadyCreated);
        }
        command::<P>(&ThreeDCommand::CreateContext {
            context: self.id,
            capset,
            name,
        })?;
        self.state.lock().created = true;
        Ok(())
    }

    fn ensure_created<P: Platform>(&self) -> Result<(), RenderError> {
        match self.initialize::<P>(0, DEBUG_NAME) {
            Ok(()) | Err(RenderError::AlreadyCreated) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// A new buffer of `size` bytes (rounded up to whole frames, one at least) shaped as
    /// `shape`, attached to this context: its handle.
    pub fn create_buffer<P: Platform>(
        &self,
        shape: ResourceShape,
        size: u64,
    ) -> Result<(u32, Arc<Buffer>), RenderError> {
        self.ensure_created::<P>()?;
        let size = size.max(1).checked_next_multiple_of(FRAME_SIZE).ok_or(RenderError::NoMemory)?;
        let length = u32::try_from(size).map_err(|_| RenderError::NoMemory)?;
        let memory =
            DmaRegion::allocate(length as usize).map_err(|_| RenderError::NoMemory)?;
        let address = memory.dma_addr().get();
        let physical = memory.physical();
        let resource = NEXT_RESOURCE.fetch_add(1, Ordering::Relaxed);
        if let Err(error) = command::<P>(&ThreeDCommand::CreateResource { resource, shape }) {
            // SAFETY: the device was never given the memory.
            unsafe { memory.free() };
            return Err(error);
        }
        let backed = command::<P>(&ThreeDCommand::AttachBacking {
            resource,
            address,
            length,
        });
        let buffer = Arc::new(Buffer {
            resource,
            physical,
            length: size,
            backed: true,
            memory: Some(ManuallyDrop::new(memory)),
            window_offset: None,
            last_fence: AtomicU64::new(0),
            release: release_resource::<P>,
        });
        backed?;
        command::<P>(&ThreeDCommand::AttachResource {
            context: self.id,
            resource,
        })?;
        let mut state = self.state.lock();
        let handle = state.next_handle;
        state.next_handle = handle.checked_add(1).ok_or(RenderError::NoMemory)?;
        state.buffers.insert(handle, Arc::clone(&buffer));
        Ok((handle, buffer))
    }

    /// A new blob of `size` bytes (whole frames, one at least) whose only copy is DMA memory
    /// the program may map (`VIRTIO_GPU_BLOB_MEM_GUEST`, D439), for this context, with `flags`
    /// (mappable, shareable): its handle.
    pub fn create_guest_blob<P: Platform>(
        &self,
        size: u64,
        flags: u32,
    ) -> Result<(u32, Arc<Buffer>), RenderError> {
        self.ensure_created::<P>()?;
        let size = size.max(1).checked_next_multiple_of(FRAME_SIZE).ok_or(RenderError::NoMemory)?;
        let length = usize::try_from(size).map_err(|_| RenderError::NoMemory)?;
        let memory = DmaRegion::allocate(length).map_err(|_| RenderError::NoMemory)?;
        let address = memory.dma_addr().get();
        let physical = memory.physical();
        let resource = NEXT_RESOURCE.fetch_add(1, Ordering::Relaxed);
        let created = command::<P>(&ThreeDCommand::CreateGuestBlob {
            context: self.id,
            resource,
            flags,
            address,
            size,
        });
        let buffer = Arc::new(Buffer {
            resource,
            physical,
            length: size,
            backed: false,
            memory: Some(ManuallyDrop::new(memory)),
            window_offset: None,
            last_fence: AtomicU64::new(0),
            release: release_resource::<P>,
        });
        created?;
        let mut state = self.state.lock();
        let handle = state.next_handle;
        state.next_handle = handle.checked_add(1).ok_or(RenderError::NoMemory)?;
        state.buffers.insert(handle, Arc::clone(&buffer));
        Ok((handle, buffer))
    }

    /// A host blob (D440): runs `stream` first when it is not empty (the command that makes
    /// the blob on the host under `blob_id`), creates the resource, and maps it in the GPU's
    /// host-visible shared memory, which the program may then map: its handle.
    pub fn create_host_blob<P: Platform>(
        &self,
        blob_id: u64,
        size: u64,
        flags: u32,
        stream: &[u8],
    ) -> Result<(u32, Arc<Buffer>), RenderError> {
        if !has_host_window() {
            return Err(RenderError::Invalid);
        }
        let size = size.max(1).checked_next_multiple_of(FRAME_SIZE).ok_or(RenderError::NoMemory)?;
        if !stream.is_empty() {
            let fence = self.submit::<P>(stream, &[])?;
            wait_fence::<P>(fence, false)?;
        }
        let resource = NEXT_RESOURCE.fetch_add(1, Ordering::Relaxed);
        command::<P>(&ThreeDCommand::CreateHostBlob {
            context: self.id,
            resource,
            flags,
            blob_id,
            size,
        })?;
        let Some((offset, physical)) = take_window(size) else {
            send_quietly::<P>(&ThreeDCommand::Unref { resource });
            return Err(RenderError::NoMemory);
        };
        let buffer = Arc::new(Buffer {
            resource,
            physical,
            length: size,
            backed: false,
            memory: None,
            window_offset: Some(offset),
            last_fence: AtomicU64::new(0),
            release: release_host_blob::<P>,
        });
        command::<P>(&ThreeDCommand::MapBlob {
            context: self.id,
            resource,
            offset,
        })?;
        let mut state = self.state.lock();
        let handle = state.next_handle;
        state.next_handle = handle.checked_add(1).ok_or(RenderError::NoMemory)?;
        state.buffers.insert(handle, Arc::clone(&buffer));
        Ok((handle, buffer))
    }

    /// Takes `buffer`, handed over as a file from another context or program (PRIME, D441):
    /// the handle it already has here, or a new one with its resource attached to this
    /// context.
    pub fn import<P: Platform>(&self, buffer: Arc<Buffer>) -> Result<u32, RenderError> {
        let known = self
            .state
            .lock()
            .buffers
            .iter()
            .find(|(_, held)| Arc::ptr_eq(held, &buffer))
            .map(|(handle, _)| *handle);
        if let Some(handle) = known {
            return Ok(handle);
        }
        self.ensure_created::<P>()?;
        command::<P>(&ThreeDCommand::AttachResource {
            context: self.id,
            resource: buffer.resource,
        })?;
        let mut state = self.state.lock();
        let handle = state.next_handle;
        state.next_handle = handle.checked_add(1).ok_or(RenderError::NoMemory)?;
        state.buffers.insert(handle, buffer);
        Ok(handle)
    }

    /// The buffer with `handle`.
    pub fn buffer(&self, handle: u32) -> Option<Arc<Buffer>> {
        self.state.lock().buffers.get(&handle).cloned()
    }

    /// Lets go of the buffer with `handle`: detached from the context, destroyed once no
    /// mapping holds it. `false` for a handle it does not have.
    pub fn close_buffer<P: Platform>(&self, handle: u32) -> bool {
        let removed = self.state.lock().buffers.remove(&handle);
        let Some(buffer) = removed else {
            return false;
        };
        send_quietly::<P>(&ThreeDCommand::DetachResource {
            context: self.id,
            resource: buffer.resource,
        });
        drop(buffer);
        true
    }

    /// Posts `stream`, a command stream in the context's encoding, that uses `buffers`, and
    /// returns its fence without waiting for it (D460); waits only while every stream slot is
    /// in flight. Each of `buffers` keeps the fence as its last.
    pub fn submit<P: Platform>(
        &self,
        stream: &[u8],
        buffers: &[Arc<Buffer>],
    ) -> Result<u64, RenderError> {
        self.ensure_created::<P>()?;
        let length = u32::try_from(stream.len()).map_err(|_| RenderError::Invalid)?;
        if stream.len() > COMMAND_STREAM_BYTES {
            return Err(RenderError::Invalid);
        }
        let context = self.id;
        loop {
            let posted = crate::display::with_renderer_now(|renderer| {
                let Some(slot) = renderer.free_stream() else {
                    return Err(note_collected(renderer).unwrap_or(0));
                };
                let Some(target) = renderer.stream(slot).get_mut(..stream.len()) else {
                    return Ok(Err("the GPU has no room for the command stream"));
                };
                target.copy_from_slice(stream);
                Ok(renderer
                    .start_stream(slot, context, length)
                    .inspect(|&fence| {
                        OLDEST_IN_FLIGHT.fetch_min(fence, Ordering::SeqCst);
                    }))
            })
            .map_err(|_| RenderError::NoDevice)?;
            match posted {
                Ok(fence) => {
                    let fence = fence.map_err(|_| RenderError::Device)?;
                    for buffer in buffers {
                        buffer.used_by(fence);
                    }
                    return Ok(fence);
                }
                Err(oldest) => wait_fence::<P>(oldest, false)?,
            }
        }
    }

    /// Copies `transfer`'s box of the buffer with `handle` from its backing to the host
    /// (`to_host`) or back, and waits until it is done; a copy back first waits for the last
    /// command stream that used the buffer.
    pub fn transfer<P: Platform>(
        &self,
        handle: u32,
        to_host: bool,
        transfer: Transfer,
    ) -> Result<(), RenderError> {
        let buffer = self.buffer(handle).ok_or(RenderError::Invalid)?;
        if !to_host {
            buffer.wait::<P>(false)?;
        }
        let transfer = Transfer {
            resource: buffer.resource,
            ..transfer
        };
        let context = self.id;
        command::<P>(&match to_host {
            true => ThreeDCommand::TransferToHost { context, transfer },
            false => ThreeDCommand::TransferFromHost { context, transfer },
        })
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        let state = core::mem::take(self.state.get_mut());
        let resources: Vec<u32> = state.buffers.values().map(|buffer| buffer.resource).collect();
        (self.end)(self.id, &resources, state.created);
        drop(state);
    }
}
