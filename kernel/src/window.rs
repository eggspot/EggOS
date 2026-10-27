//! The window server (D127): the windows programs open through `/dev/window` (docs/windows.md),
//! on the desktop the kernel draws ([`crate::terminal`]), whose state and drawing are
//! [`eggos_drivers::desktop::Desktop`]'s. In the kernel for v0.3.0 (D126); the contract is what
//! stays when it moves to user space.
//!
//! Each open of `/dev/window` is a client ([`Connection`]) with an event queue
//! ([`EventQueue`]); closing its last descriptor destroys its windows. A window's pixels are a
//! [`PixelBuffer`]: physically contiguous frames the server owns, zeroed, which the client maps
//! with `mmap` at the offset `Create` and `SetSize` return ([`Process::map_lent_frames`]) and
//! the compositor reads through the direct map. A client's buffers, those its windows show and
//! those its process still maps, hold at most [`CLIENT_PIXEL_LIMIT`] bytes together. `Commit`
//! composes the rectangles it names into the framebuffer during the call, under the display's
//! and the screen's locks, and has the drawing thread flush them. Requests, reads and polls
//! never allocate but for a window's buffer and a client's queue; composing never does.
//!
//! [`Process::map_lent_frames`]: crate::process::Process::map_lent_frames

use crate::memory::FRAMES;
use crate::process::LentFrames;
use crate::sync::LeafSpinLock;
use alloc::sync::Arc;
use core::ptr;
use core::sync::atomic::{AtomicU16, AtomicU64, Ordering};
use eggos_drivers::desktop::protocol::{
    EventRecord, EventSink, MAX_CLIENTS, MAX_WINDOWS, WindowError,
};
use eggos_drivers::desktop::{Desktop, EventQueue, KERNEL_CLIENT, WindowId, colour, slot_of};
use eggos_hal::Platform;
use eggos_hal::memory::FRAME_SIZE;

/// The most bytes of pixel buffers one client holds at once: those its windows show and those
/// its process still maps after a resize or a destroy (32 MiB, eight buffers of a maximised
/// window on a 1280x800 screen).
pub const CLIENT_PIXEL_LIMIT: u64 = 32 << 20;

/// The pixels of one window: `width` by `height` `0x00RRGGBB` pixels, rows packed, in
/// physically contiguous frames the server owns, freed when the last holder lets go (the
/// window and every process that maps them), counted against its client's
/// [`CLIENT_PIXEL_LIMIT`] until then.
pub struct PixelBuffer {
    physical: u64,
    order: u32,
    width: u32,
    height: u32,
    frame_offset: usize,
    owner: Arc<Client>,
}

impl core::fmt::Debug for PixelBuffer {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("PixelBuffer")
            .field("physical", &self.physical)
            .field("width", &self.width)
            .field("height", &self.height)
            .finish()
    }
}

impl PixelBuffer {
    /// A zeroed buffer of `width` by `height` pixels for `owner`; `None` without contiguous
    /// frames for it (at most 4 MiB, the frame allocator's largest block) or when it would take
    /// `owner` past [`CLIENT_PIXEL_LIMIT`].
    pub fn new(width: u32, height: u32, owner: &Arc<Client>) -> Option<Arc<Self>> {
        let bytes = u64::from(width) * u64::from(height) * 4;
        let frames = bytes.div_ceil(FRAME_SIZE).max(1);
        let order = frames.next_power_of_two().trailing_zeros();
        let block = FRAME_SIZE << order;
        owner
            .pinned
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |pinned| {
                pinned
                    .checked_add(block)
                    .filter(|&total| total <= CLIENT_PIXEL_LIMIT)
            })
            .ok()?;
        let allocated = {
            let mut allocator = FRAMES.lock();
            allocator.as_mut().and_then(|allocator| {
                Some((
                    allocator.allocate_contiguous(order)?,
                    allocator.physical_memory_offset(),
                ))
            })
        };
        let Some((physical, frame_offset)) = allocated else {
            owner.pinned.fetch_sub(block, Ordering::Relaxed);
            return None;
        };
        let buffer = Self {
            physical,
            order,
            width,
            height,
            frame_offset,
            owner: Arc::clone(owner),
        };
        // SAFETY: the block was just allocated, so nothing else uses it, and the allocator
        // reaches it through `frame_offset`, the direct map's offset.
        unsafe { eggos_hal::memory::zero(buffer.base().cast(), (FRAME_SIZE << order) as usize) };
        Some(Arc::new(buffer))
    }

    fn base(&self) -> *mut u32 {
        (self.physical as usize).wrapping_add(self.frame_offset) as *mut u32
    }

    /// Its size in pixels.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Copies pixels `x..x + out.len()` of row `y` into `out`; what lies outside the buffer is
    /// the window background.
    pub fn copy_row(&self, x: u32, y: u32, out: &mut [u32]) {
        if y >= self.height || x >= self.width {
            out.fill(colour::WINDOW_BACKGROUND);
            return;
        }
        let count = out.len().min((self.width - x) as usize);
        out[count..].fill(colour::WINDOW_BACKGROUND);
        let at = y as usize * self.width as usize + x as usize;
        // SAFETY: row `y`, columns `x..x + count`, are inside the buffer's frames, which stay
        // allocated while `self` lives. A program that maps them may write them meanwhile:
        // the copy goes through raw pointers only, as kernel copies of user memory do (D94),
        // into `out`, which is not in the buffer.
        unsafe { ptr::copy_nonoverlapping(self.base().wrapping_add(at), out.as_mut_ptr(), count) };
    }
}

impl LentFrames for PixelBuffer {
    fn physical(&self) -> u64 {
        self.physical
    }

    fn length(&self) -> u64 {
        (u64::from(self.width) * u64::from(self.height) * 4).next_multiple_of(FRAME_SIZE)
    }
}

impl Drop for PixelBuffer {
    fn drop(&mut self) {
        let mut allocator = FRAMES.lock();
        let allocator = allocator
            .as_mut()
            .expect("the buffer came from the frame allocator");
        // SAFETY: the block came from `allocate_contiguous` with this order on this allocator
        // (`new`), and this is the last holder: no window shows it and no process maps it.
        unsafe { allocator.free_contiguous(self.physical, self.order) };
        self.owner
            .pinned
            .fetch_sub(FRAME_SIZE << self.order, Ordering::Relaxed);
    }
}

/// One client of the server: an open description of `/dev/window`, its number, its event
/// queue and the bytes of pixel buffers it holds.
#[derive(Debug)]
pub struct Client {
    id: u16,
    queue: LeafSpinLock<EventQueue>,
    pinned: AtomicU64,
}

impl Client {
    /// Its number, the one the desktop knows it by: its slot in the low bits, and above them a
    /// generation that grows with every client the kernel connects, so no later client, even
    /// after the desktop restarts, has the number of one still connected.
    pub fn id(&self) -> u16 {
        self.id
    }

    /// The bytes of pixel buffers it holds.
    pub fn pinned_bytes(&self) -> u64 {
        self.pinned.load(Ordering::Relaxed)
    }

    /// Whether an event waits to be read.
    pub fn has_events(&self) -> bool {
        !self.queue.lock().is_empty()
    }

    /// Takes the oldest records, as many as `records` holds; returns how many.
    pub fn take_events(&self, records: &mut [EventRecord]) -> usize {
        let mut queue = self.queue.lock();
        let mut taken = 0;
        while taken < records.len()
            && let Some(record) = queue.pop()
        {
            records[taken] = record;
            taken += 1;
        }
        taken
    }
}

/// What `/dev/window`'s open file description holds: the client, which closing the last
/// descriptor on it disconnects.
pub struct Connection {
    client: Arc<Client>,
    disconnect: fn(&Arc<Client>),
}

impl Connection {
    /// The client.
    pub fn client(&self) -> &Arc<Client> {
        &self.client
    }
}

impl core::fmt::Debug for Connection {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_tuple("Connection")
            .field(&self.client.id)
            .finish()
    }
}

impl PartialEq for Connection {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.client, &other.client)
    }
}

impl Eq for Connection {}

impl Drop for Connection {
    fn drop(&mut self) {
        (self.disconnect)(&self.client);
    }
}

#[derive(Debug)]
struct Attached {
    window: WindowId,
    serial: u32,
    buffer: Arc<PixelBuffer>,
}

/// The buffer each window shows, by slot, and the serial numbers that make their offsets.
#[derive(Debug)]
pub struct Buffers {
    slots: [Option<Attached>; MAX_WINDOWS],
    next_serial: u32,
}

const OFFSET_SLOT_SHIFT: u32 = 32;

impl Buffers {
    /// None.
    pub const fn new() -> Self {
        Self {
            slots: [const { None }; MAX_WINDOWS],
            next_serial: 0,
        }
    }

    /// Makes `buffer` window `window`'s, in place of the one it had, and returns the offset
    /// that maps it.
    pub fn attach(&mut self, window: WindowId, buffer: Arc<PixelBuffer>) -> u64 {
        let serial = self.next_serial;
        self.next_serial = self.next_serial.wrapping_add(1);
        let slot = slot_of(window);
        self.slots[slot] = Some(Attached {
            window,
            serial,
            buffer,
        });
        ((slot as u64) << OFFSET_SLOT_SHIFT | u64::from(serial)) * FRAME_SIZE
    }

    /// Window `window`'s buffer, if it has one.
    pub fn of(&self, window: WindowId) -> Option<&PixelBuffer> {
        self.slots[slot_of(window)]
            .as_ref()
            .filter(|attached| attached.window == window)
            .map(|attached| &*attached.buffer)
    }

    /// The buffer `offset` maps, and its window.
    pub fn at_offset(&self, offset: u64) -> Option<(WindowId, Arc<PixelBuffer>)> {
        if !offset.is_multiple_of(FRAME_SIZE) {
            return None;
        }
        let index = offset / FRAME_SIZE;
        let slot = usize::try_from(index >> OFFSET_SLOT_SHIFT).ok()?;
        let attached = self.slots.get(slot)?.as_ref()?;
        (u64::from(attached.serial) == index & u64::from(u32::MAX))
            .then(|| (attached.window, Arc::clone(&attached.buffer)))
    }

    /// Lets go of the buffers of windows the desktop no longer has.
    pub fn release_unused(&mut self, desktop: &Desktop) {
        for slot in &mut self.slots {
            if slot
                .as_ref()
                .is_some_and(|attached| desktop.window_rect(attached.window).is_none())
            {
                *slot = None;
            }
        }
    }
}

impl Default for Buffers {
    fn default() -> Self {
        Self::new()
    }
}

/// The server's clients, by number.
#[derive(Debug)]
pub struct Clients {
    slots: [Option<Arc<Client>>; MAX_CLIENTS],
}

const CLIENT_SLOT_BITS: u32 = MAX_CLIENTS.trailing_zeros();
const CLIENT_GENERATIONS: u16 = (KERNEL_CLIENT >> CLIENT_SLOT_BITS) + 1;

static NEXT_CLIENT_GENERATION: AtomicU16 = AtomicU16::new(0);

fn client_number(slot: usize) -> u16 {
    let generation =
        NEXT_CLIENT_GENERATION.fetch_add(1, Ordering::Relaxed) % (CLIENT_GENERATIONS - 1);
    generation << CLIENT_SLOT_BITS | slot as u16
}

fn client_slot(id: u16) -> usize {
    usize::from(id) & (MAX_CLIENTS - 1)
}

impl Clients {
    /// None.
    pub const fn new() -> Self {
        Self {
            slots: [const { None }; MAX_CLIENTS],
        }
    }

    fn add(&mut self, queue: EventQueue) -> Option<Arc<Client>> {
        let slot = self.slots.iter().position(Option::is_none)?;
        let client = Arc::new(Client {
            id: client_number(slot),
            queue: LeafSpinLock::new_leaf(queue),
            pinned: AtomicU64::new(0),
        });
        self.slots[slot] = Some(Arc::clone(&client));
        Some(client)
    }

    fn get(&self, id: u16) -> Option<&Arc<Client>> {
        self.slots[client_slot(id)]
            .as_ref()
            .filter(|client| client.id == id)
    }

    /// Whether `client` is one of these clients (not one of a desktop that stopped).
    pub fn has(&self, client: &Arc<Client>) -> bool {
        self.get(client.id)
            .is_some_and(|known| Arc::ptr_eq(known, client))
    }

    fn remove(&mut self, client: &Arc<Client>) {
        if self.has(client) {
            self.slots[client_slot(client.id)] = None;
        }
    }
}

impl Default for Clients {
    fn default() -> Self {
        Self::new()
    }
}

/// The [`EventSink`] that stamps events with the time and queues them for their clients.
#[derive(Debug)]
pub struct Queues<'a> {
    clients: &'a Clients,
    time_ms: u32,
    sent: bool,
}

impl<'a> Queues<'a> {
    /// Queues for `clients`' events at `time_ms` on the monotonic clock.
    pub fn new(clients: &'a Clients, time_ms: u32) -> Self {
        Self {
            clients,
            time_ms,
            sent: false,
        }
    }

    /// Whether an event was queued.
    pub fn sent(&self) -> bool {
        self.sent
    }
}

impl EventSink for Queues<'_> {
    fn send(&mut self, client: u16, mut record: EventRecord) {
        if let Some(target) = self.clients.get(client) {
            record.time_ms = self.time_ms;
            target.queue.lock().push(record);
            self.sent = true;
        }
    }
}

/// The monotonic clock in milliseconds, wrapping at 2^32 as the records carry it.
pub fn time_ms<P: Platform>() -> u32 {
    (u128::from(P::clock_ticks()) * 1000 / u128::from(P::clock_frequency().max(1))) as u32
}

/// Why a request failed, as the window server sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerError {
    /// A request the desktop refused.
    Refused(WindowError),
    /// No display, so no server (`ENXIO`).
    NoServer,
    /// Every client slot is taken (`ENFILE`).
    NoClientSlot,
    /// No memory for a buffer or a queue (`ENOMEM`).
    OutOfMemory,
}

impl From<WindowError> for ServerError {
    fn from(error: WindowError) -> Self {
        Self::Refused(error)
    }
}

/// Connects a new client; `disconnect` is called with it when its last descriptor closes.
pub fn connect(disconnect: fn(&Arc<Client>)) -> Result<Connection, ServerError> {
    let queue = EventQueue::new().ok_or(ServerError::OutOfMemory)?;
    let client = crate::terminal::with_clients(|clients| clients.add(queue))
        .ok_or(ServerError::NoServer)?
        .ok_or(ServerError::NoClientSlot)?;
    Ok(Connection { client, disconnect })
}

/// Destroys `client`'s windows and forgets it; nothing if it was a client of a desktop that
/// stopped since, whose windows went with it.
pub fn disconnect<P: Platform>(client: &Arc<Client>) {
    let current = crate::terminal::with_clients(|clients| clients.has(client)).unwrap_or(false);
    if !current {
        return;
    }
    crate::terminal::with_windows::<P, _>(|desktop, buffers, damage, events| {
        desktop.destroy_client(client.id, damage, events);
        buffers.release_unused(desktop);
    });
    crate::terminal::with_clients(|clients| clients.remove(client));
}
