//! The layer's connection to the window server, `/dev/window` (docs/windows.md, D127): one per
//! process, opened without blocking when the program first needs it, with the requests the
//! layer makes and the pixel buffers it maps, each filled with the window background until the
//! program paints it.

use crate::linux::{self, O_NONBLOCK, O_RDWR};
use eggos_element_tree::{
    MAX_TEXT_BYTES, answer_query, read_query, request as query_request, size as query_size,
};
use eggos_runtime::errno::Errno;
use eggos_windows_gui::colour::token;
use eggos_windows_gui::surface::Rect;

const CREATE: usize = 0xc020_7701;
const SET_TITLE: usize = 0x4048_7702;
const SET_SIZE: usize = 0xc018_7703;
const SET_POSITION: usize = 0x400c_7705;
const SET_STATE: usize = 0x4008_7706;
const SET_POINTER: usize = 0x4008_7707;
const COMMIT: usize = 0x4088_7708;
const DESTROY: usize = 0x4004_7709;
const GET_WORK_AREA: usize = 0x8014_770a;
const TITLE_CAPACITY: usize = 64;
const TRY_AGAIN: u16 = 11;
const PIXEL_BYTES: usize = 4;

/// `CREATE`'s roles.
pub const ROLE_NORMAL: u32 = 0;
pub const ROLE_DIALOG: u32 = 1;
/// `CREATE`'s flag for a window the user cannot resize or maximise.
pub const FIXED_SIZE: u32 = 1;

/// The open connection: its descriptor and the work area, in screen coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Device {
    pub fd: usize,
    pub work_area: Rect,
}

/// A window the server made, with its content size and the address its pixels are mapped at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Buffer {
    pub window: u32,
    pub width: u32,
    pub height: u32,
    pub pixels: usize,
}

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_ne_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn put(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_ne_bytes());
}

fn offset(bytes: &[u8], at: usize) -> usize {
    let mut value = [0u8; 8];
    value.copy_from_slice(&bytes[at..at + 8]);
    u64::from_ne_bytes(value) as usize
}

fn buffer_length(width: u32, height: u32) -> usize {
    width as usize * height as usize * PIXEL_BYTES
}

impl Device {
    /// Connects to the window server; `ENXIO` without a display.
    pub fn open() -> Result<Device, Errno> {
        let fd = linux::open(b"/dev/window\0", O_RDWR | O_NONBLOCK)?;
        let mut area = [0u8; 20];
        if let Err(errno) = linux::device_control(fd, GET_WORK_AREA, &mut area) {
            let _ = linux::close(fd);
            return Err(errno);
        }
        let (x, y) = (word(&area, 0) as i32, word(&area, 4) as i32);
        let (width, height) = (word(&area, 8) as i32, word(&area, 12) as i32);
        Ok(Device {
            fd,
            work_area: Rect::sized(x, y, width, height),
        })
    }

    fn request(&self, request: usize, argument: &mut [u8]) -> Result<(), Errno> {
        linux::device_control(self.fd, request, argument).map(|_| ())
    }

    fn map(&self, offset: usize, width: u32, height: u32) -> Result<usize, Errno> {
        let pixels = linux::map_shared(self.fd, buffer_length(width, height), offset)?;
        // SAFETY: the new mapping holds `width * height` pixels and nothing else uses it yet.
        unsafe {
            core::slice::from_raw_parts_mut(pixels as *mut u32, width as usize * height as usize)
        }
        .fill(token::WINDOW_BACKGROUND);
        Ok(pixels)
    }

    /// A window of `role` (a dialog of server window `parent`) with content `width` by
    /// `height` (the server may clamp it) and `flags`, its buffer mapped.
    pub fn create(
        &self,
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
        self.request(CREATE, &mut create)?;
        let window = word(&create, 20);
        let (width, height) = (word(&create, 8), word(&create, 12));
        match self.map(offset(&create, 24), width, height) {
            Ok(pixels) => Ok(Buffer {
                window,
                width,
                height,
                pixels,
            }),
            Err(errno) => {
                let _ = self.request(DESTROY, &mut window.to_ne_bytes());
                Err(errno)
            }
        }
    }

    /// Gives `window` a new buffer of `width` by `height` (as the server clamps it) and maps
    /// it; the old buffer stays mapped (the server keeps it valid until it is unmapped) for the
    /// caller to unmap once nothing draws on it.
    pub fn resize(&self, window: u32, width: u32, height: u32) -> Result<Buffer, Errno> {
        let mut set_size = [0u8; 24];
        put(&mut set_size, 0, window);
        put(&mut set_size, 4, width);
        put(&mut set_size, 8, height);
        self.request(SET_SIZE, &mut set_size)?;
        let (width, height) = (word(&set_size, 4), word(&set_size, 8));
        let pixels = self.map(offset(&set_size, 16), width, height)?;
        Ok(Buffer {
            window,
            width,
            height,
            pixels,
        })
    }

    /// Sets `window`'s title to `title`, cut to the server's 64 bytes at a character boundary.
    pub fn set_title(&self, window: u32, title: &str) -> Result<(), Errno> {
        let mut length = title.len().min(TITLE_CAPACITY);
        while !title.is_char_boundary(length) {
            length -= 1;
        }
        let mut set_title = [0u8; 8 + TITLE_CAPACITY];
        put(&mut set_title, 0, window);
        put(&mut set_title, 4, length as u32);
        set_title[8..8 + length].copy_from_slice(&title.as_bytes()[..length]);
        self.request(SET_TITLE, &mut set_title)
    }

    /// Places `window`'s outside top-left at (`x`, `y`) of the work area.
    pub fn set_position(&self, window: u32, x: i32, y: i32) -> Result<(), Errno> {
        let mut position = [0u8; 12];
        put(&mut position, 0, window);
        put(&mut position, 4, x as u32);
        put(&mut position, 8, y as u32);
        self.request(SET_POSITION, &mut position)
    }

    /// Restores (0), minimises (1) or maximises (2) `window`.
    pub fn set_state(&self, window: u32, state: u32) -> Result<(), Errno> {
        let mut request = [0u8; 8];
        put(&mut request, 0, window);
        put(&mut request, 4, state);
        self.request(SET_STATE, &mut request)
    }

    /// Shows pointer `shape` over `window`'s content.
    pub fn set_pointer(&self, window: u32, shape: u32) -> Result<(), Errno> {
        let mut request = [0u8; 8];
        put(&mut request, 0, window);
        put(&mut request, 4, shape);
        self.request(SET_POINTER, &mut request)
    }

    /// Shows `rect` of `window`'s content as its buffer has it (all of it when `None`); the
    /// first commit shows the window.
    pub fn commit(&self, window: u32, rect: Option<Rect>) -> Result<(), Errno> {
        let mut commit = [0u8; 136];
        put(&mut commit, 0, window);
        if let Some(rect) = rect.filter(|rect| !rect.is_empty()) {
            put(&mut commit, 4, 1);
            put(&mut commit, 8, rect.left.max(0) as u32);
            put(&mut commit, 12, rect.top.max(0) as u32);
            put(&mut commit, 16, rect.width() as u32);
            put(&mut commit, 20, rect.height() as u32);
        }
        self.request(COMMIT, &mut commit)
    }

    /// Destroys `buffer`'s window and unmaps its pixels.
    ///
    /// # Safety
    /// As [`unmap`]: no window in the program's table holds the buffer.
    pub unsafe fn destroy(&self, buffer: &Buffer) {
        let _ = self.request(DESTROY, &mut buffer.window.to_ne_bytes());
        // SAFETY: the caller's duty.
        unsafe { unmap(buffer) };
    }

    /// Reads the text of query `serial`, an agent's `set-text` (D161, `READ_QUERY`), into
    /// `text`; its length.
    pub fn query_text(&self, serial: u32, text: &mut [u8; MAX_TEXT_BYTES]) -> Result<usize, Errno> {
        let mut query = [0u8; query_size::READ_QUERY];
        put(&mut query, read_query::SERIAL, serial);
        self.request(query_request::READ_QUERY as usize, &mut query)?;
        let length = (word(&query, read_query::TEXT_LENGTH) as usize).min(MAX_TEXT_BYTES);
        text[..length].copy_from_slice(&query[read_query::TEXT..read_query::TEXT + length]);
        Ok(length)
    }

    /// Answers query `serial` with `outcome` and, for a tree, its bytes (`ANSWER_QUERY`).
    pub fn answer_query(&self, serial: u32, outcome: u32, tree: &[u8]) -> Result<(), Errno> {
        let mut answer = [0u8; query_size::ANSWER_QUERY];
        put(&mut answer, answer_query::SERIAL, serial);
        put(&mut answer, answer_query::OUTCOME, outcome);
        answer[answer_query::BUFFER..answer_query::BUFFER + 8]
            .copy_from_slice(&(tree.as_ptr() as u64).to_ne_bytes());
        answer[answer_query::LENGTH..answer_query::LENGTH + 8]
            .copy_from_slice(&(tree.len() as u64).to_ne_bytes());
        self.request(query_request::ANSWER_QUERY as usize, &mut answer)
    }

    /// Reads the waiting event records into `records`; 0 when none waits.
    pub fn read_events(&self, records: &mut [u8]) -> Result<usize, Errno> {
        match linux::read(self.fd, records) {
            Err(errno) if errno.0 == TRY_AGAIN => Ok(0),
            result => result,
        }
    }
}

/// Unmaps `buffer`'s pixels.
///
/// # Safety
/// No window in the program's table may hold the buffer: every drawing reaches a buffer only
/// through the table, under its lock, so then nothing draws on it any more.
pub unsafe fn unmap(buffer: &Buffer) {
    // SAFETY: the caller's duty.
    let _ = unsafe { linux::unmap(buffer.pixels, buffer_length(buffer.width, buffer.height)) };
}
