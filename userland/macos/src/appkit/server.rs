//! The layer's connection to the window server, `/dev/window` (docs/windows.md, D127): one per
//! process, opened when the app first shows a window or runs, blocking, with the requests
//! AppKit makes and the pixel buffers it maps.

use crate::linux::{self, O_CLOEXEC, O_RDWR};
use core::sync::atomic::{AtomicUsize, Ordering};
use eggos_runtime::errno::Errno;

const CREATE: usize = 0xc020_7701;
const SET_TITLE: usize = 0x4048_7702;
const SET_SIZE: usize = 0xc018_7703;
const SET_POINTER: usize = 0x4008_7707;
const COMMIT: usize = 0x4088_7708;
const DESTROY: usize = 0x4004_7709;
const SET_MENU: usize = 0x4100_7715;
const TITLE_CAPACITY: usize = 64;
const MENU_REQUEST_SIZE: usize = 256;
const PIXEL_BYTES: usize = 4;

/// `CREATE`'s roles.
pub const ROLE_NORMAL: u32 = 0;
pub const ROLE_DIALOG: u32 = 1;
/// `CREATE`'s flag for a window the user cannot resize.
pub const FIXED_SIZE: u32 = 1;
/// `SET_POINTER`'s shapes.
pub const POINTER_ARROW: u32 = 0;
pub const POINTER_I_BEAM: u32 = 1;

static DESCRIPTOR: AtomicUsize = AtomicUsize::new(0);

/// A window the server made: its id, its content size and where its pixels are mapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Buffer {
    pub window: u32,
    pub width: u32,
    pub height: u32,
    pub pixels: usize,
}

impl Buffer {
    /// The buffer's pixels.
    ///
    /// # Safety
    /// The buffer must still be mapped, and nothing else may use its pixels while the slice
    /// lives.
    pub unsafe fn pixels<'a>(&self) -> &'a mut [u32] {
        // SAFETY: the caller's promise; the mapping holds `width * height` pixels.
        unsafe {
            core::slice::from_raw_parts_mut(
                self.pixels as *mut u32,
                self.width as usize * self.height as usize,
            )
        }
    }
}

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_ne_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn put(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_ne_bytes());
}

/// The connection's descriptor, opening it the first time; `ENXIO` without a display.
pub fn descriptor() -> Result<usize, Errno> {
    let open = DESCRIPTOR.load(Ordering::Acquire);
    if open != 0 {
        return Ok(open - 1);
    }
    let fd = linux::open(b"/dev/window\0", O_RDWR | O_CLOEXEC, 0)?;
    match DESCRIPTOR.compare_exchange(0, fd + 1, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => Ok(fd),
        Err(winner) => {
            let _ = linux::close(fd);
            Ok(winner - 1)
        }
    }
}

fn request(number: usize, argument: &mut [u8]) -> Result<(), Errno> {
    linux::device_control(descriptor()?, number, argument).map(|_| ())
}

/// A window of `role` (a dialog of server window `parent`) with content `width` by `height`
/// (the server may clamp it) and `flags`, its buffer mapped.
pub fn create(
    role: u32,
    parent: u32,
    width: u32,
    height: u32,
    flags: u32,
) -> Result<Buffer, Errno> {
    let mut create = [0u8; 32];
    put(&mut create, 0, role);
    put(&mut create, 4, parent);
    put(&mut create, 8, width);
    put(&mut create, 12, height);
    put(&mut create, 16, flags);
    request(CREATE, &mut create)?;
    let window = word(&create, 20);
    let (width, height) = (word(&create, 8), word(&create, 12));
    let mut offset = [0u8; 8];
    offset.copy_from_slice(&create[24..32]);
    let length = width as usize * height as usize * PIXEL_BYTES;
    match linux::map_shared(descriptor()?, length, u64::from_ne_bytes(offset) as usize) {
        Ok(pixels) => Ok(Buffer {
            window,
            width,
            height,
            pixels,
        }),
        Err(errno) => {
            let _ = request(DESTROY, &mut window.to_ne_bytes());
            Err(errno)
        }
    }
}

/// Gives `buffer`'s window a new buffer of `width` by `height` (as the server clamps it), maps
/// it and unmaps the old one.
///
/// # Safety
/// Nothing may use `buffer`'s pixels any more.
pub unsafe fn resize(buffer: &Buffer, width: u32, height: u32) -> Result<Buffer, Errno> {
    let mut set_size = [0u8; 24];
    put(&mut set_size, 0, buffer.window);
    put(&mut set_size, 4, width);
    put(&mut set_size, 8, height);
    request(SET_SIZE, &mut set_size)?;
    let (width, height) = (word(&set_size, 4), word(&set_size, 8));
    let mut offset = [0u8; 8];
    offset.copy_from_slice(&set_size[16..24]);
    let length = width as usize * height as usize * PIXEL_BYTES;
    let pixels = linux::map_shared(descriptor()?, length, u64::from_ne_bytes(offset) as usize)?;
    let old = buffer.width as usize * buffer.height as usize * PIXEL_BYTES;
    // SAFETY: the caller's promise: the old mapping is unused.
    let _ = unsafe { linux::unmap(buffer.pixels, old) };
    Ok(Buffer {
        window: buffer.window,
        width,
        height,
        pixels,
    })
}

/// Sets `window`'s title to `title`, cut to the server's 64 bytes at a character boundary.
pub fn set_title(window: u32, title: &str) {
    let mut length = title.len().min(TITLE_CAPACITY);
    while !title.is_char_boundary(length) {
        length -= 1;
    }
    let mut set_title = [0u8; 8 + TITLE_CAPACITY];
    put(&mut set_title, 0, window);
    put(&mut set_title, 4, length as u32);
    set_title[8..8 + length].copy_from_slice(&title.as_bytes()[..length]);
    let _ = request(SET_TITLE, &mut set_title);
}

/// Gives `window` the main menu `pool` (`SET_MENU`); an empty pool removes it.
pub fn set_menu(window: u32, pool: &[u8]) {
    let mut set_menu = [0u8; MENU_REQUEST_SIZE];
    let length = pool.len().min(MENU_REQUEST_SIZE - 8);
    put(&mut set_menu, 0, window);
    put(&mut set_menu, 4, length as u32);
    set_menu[8..8 + length].copy_from_slice(&pool[..length]);
    let _ = request(SET_MENU, &mut set_menu);
}

/// Shows `shape` while the pointer is over `window`'s content.
pub fn set_pointer(window: u32, shape: u32) {
    let mut set_pointer = [0u8; 8];
    put(&mut set_pointer, 0, window);
    put(&mut set_pointer, 4, shape);
    let _ = request(SET_POINTER, &mut set_pointer);
}

/// Shows the whole of `window`'s buffer; the first commit shows the window.
pub fn commit(window: u32) {
    let mut commit = [0u8; 136];
    put(&mut commit, 0, window);
    let _ = request(COMMIT, &mut commit);
}

/// Destroys `buffer`'s window and unmaps its pixels.
///
/// # Safety
/// Nothing may use the buffer's pixels any more.
pub unsafe fn destroy(buffer: &Buffer) {
    let _ = request(DESTROY, &mut buffer.window.to_ne_bytes());
    let length = buffer.width as usize * buffer.height as usize * PIXEL_BYTES;
    // SAFETY: the caller's promise: the mapping is unused.
    let _ = unsafe { linux::unmap(buffer.pixels, length) };
}

/// Waits for event records and reads as many as fit `records`; how many bytes.
pub fn read_events(records: &mut [u8]) -> Result<usize, Errno> {
    let fd = descriptor()?;
    loop {
        match linux::read(fd, records) {
            Err(Errno::EINTR) => continue,
            result => return result,
        }
    }
}
