//! What the compositor needs from the machine: its clients' sockets and memory, their
//! descriptors, the window server's device (`/dev/window`, docs/windows.md) and the clock. The
//! Linux side is `linux.rs`; the host tests and the fuzz target use a fake.

use crate::client::ClientSlot;
use eggos_runtime::errno::Errno;
use eggos_wayland::blit::Rect;

/// A file descriptor.
pub type Fd = i32;

/// What one read from a client's socket brought.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Received {
    /// Bytes written into the buffer; 0 with no descriptors means the client hung up.
    pub bytes: usize,
    /// Descriptors written into the descriptor buffer.
    pub descriptors: usize,
    /// More descriptors came than the buffer had room for; the rest were closed.
    pub truncated: bool,
}

/// A window's pixel buffer as the server mapped it into this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowBuffer {
    /// The window's id on `/dev/window`.
    pub id: u32,
    /// Its content's width in pixels: the buffer's rows.
    pub width: u32,
    /// Its content's height.
    pub height: u32,
    /// Where the buffer is mapped.
    pub address: usize,
}

/// Where a client's pixels are: its pool's mapping, and the buffer's place in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolPixels {
    /// Where the pool is mapped.
    pub address: usize,
    /// The mapping's length in bytes.
    pub length: usize,
    /// The buffer's first pixel's offset in the pool, in bytes.
    pub offset: usize,
    /// The bytes from one of the buffer's rows to the next.
    pub stride: usize,
}

/// The machine, as the compositor uses it.
pub trait System {
    /// The monotonic clock, in milliseconds.
    fn now_ms(&mut self) -> u64;
    /// A new client's state for the connection on `socket`, in memory of its own; `None` when
    /// there is no memory for it.
    fn new_client(&mut self, socket: Fd) -> Option<&'static mut ClientSlot>;
    /// Gives a client's memory back; nothing uses it after. It may be unmapped only after the
    /// call, once no reference to it is alive.
    fn free_client(&mut self, client: &'static mut ClientSlot);
    /// Reads what `socket` holds into `bytes`, and the descriptors that came with it into
    /// `descriptors`, without waiting (`EAGAIN` when nothing is there).
    fn receive(
        &mut self,
        socket: Fd,
        bytes: &mut [u8],
        descriptors: &mut [Fd],
    ) -> Result<Received, Errno>;
    /// Writes as much of `bytes` to `socket` as fits without waiting, with `descriptors`
    /// attached to its first byte; how many bytes went.
    fn send(&mut self, socket: Fd, bytes: &[u8], descriptors: &[Fd]) -> Result<usize, Errno>;
    /// Closes `descriptor`.
    fn close(&mut self, descriptor: Fd);
    /// Maps `length` bytes of the client's memory file `descriptor` readable and shared.
    fn map_pool(&mut self, descriptor: Fd, length: usize) -> Result<usize, Errno>;
    /// Unmaps a pool's mapping.
    fn unmap_pool(&mut self, address: usize, length: usize);
    /// Copies `rect` of a client's buffer into the same place of `window`'s buffer, every
    /// pixel made opaque; the parts outside the pool's mapping or the window are left out.
    fn copy_rows(&mut self, from: PoolPixels, window: WindowBuffer, rect: Rect);
    /// A new memory file holding the keyboard map and its NUL, sealed against changes of size:
    /// its descriptor and size.
    fn keymap(&mut self) -> Result<(Fd, u32), Errno>;
    /// Connects to the window server.
    fn open_window_device(&mut self) -> Result<Fd, Errno>;
    /// A new normal window of `width` by `height` (clamped by the server) with `flags`, its
    /// buffer mapped.
    fn create_window(
        &mut self,
        device: Fd,
        width: u32,
        height: u32,
        flags: u32,
    ) -> Result<WindowBuffer, Errno>;
    /// Gives `window` a new buffer of `width` by `height` (clamped by the server), mapped
    /// before the old one is unmapped.
    fn resize_window(
        &mut self,
        device: Fd,
        window: &mut WindowBuffer,
        width: u32,
        height: u32,
    ) -> Result<(), Errno>;
    /// Destroys `window` and unmaps its buffer.
    fn destroy_window(&mut self, device: Fd, window: WindowBuffer);
    /// Sets `window`'s title, at most 64 bytes of UTF-8.
    fn set_title(&mut self, device: Fd, window: u32, title: &[u8]) -> Result<(), Errno>;
    /// Answers the window server's query `serial` (D161, `ANSWER_QUERY`) with `outcome` and,
    /// for a tree query, the element tree `tree`.
    fn answer_query(
        &mut self,
        device: Fd,
        serial: u32,
        outcome: u32,
        tree: &[u8],
    ) -> Result<(), Errno>;
    /// Sets the smallest content the window may be resized to.
    fn set_minimum_size(
        &mut self,
        device: Fd,
        window: u32,
        width: u32,
        height: u32,
    ) -> Result<(), Errno>;
    /// Asks for state 0 (normal), 1 (minimised) or 2 (maximised).
    fn set_state(&mut self, device: Fd, window: u32, state: u32) -> Result<(), Errno>;
    /// Hands the press `window` holds to the server's move (`kind` 0) or resize by `edges`
    /// (`kind` 1).
    fn start_move_or_resize(
        &mut self,
        device: Fd,
        window: u32,
        kind: u32,
        edges: u32,
    ) -> Result<(), Errno>;
    /// Shows `rects` of `window`'s buffer (all of it when empty), and the window on its first
    /// commit.
    fn commit(&mut self, device: Fd, window: u32, rects: &[Rect]) -> Result<(), Errno>;
    /// The work area's width and height.
    fn work_area(&mut self) -> (u32, u32);
    /// Reads event records from `device` without waiting; how many bytes.
    fn read_events(&mut self, device: Fd, records: &mut [u8]) -> Result<usize, Errno>;
}
