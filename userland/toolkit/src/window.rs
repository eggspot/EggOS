//! The connection to the window server: `/dev/window` (docs/windows.md, D127), its requests,
//! a window's mapped pixels and its event records.

use crate::damage::Damage;
use crate::geometry::Rect;
use crate::input::RECORD_SIZE;
use crate::tree::PointerShape;
use core::ffi::CStr;
use eggos_runtime::errno::Errno;
use eggos_runtime::syscall::{call, number};

const CREATE: usize = 0xc020_7701;
const SET_TITLE: usize = 0x4048_7702;
const SET_SIZE: usize = 0xc018_7703;
const SET_MINIMUM_SIZE: usize = 0x400c_7704;
const SET_POINTER: usize = 0x4008_7707;
const COMMIT: usize = 0x4088_7708;
const DESTROY: usize = 0x4004_7709;
const GET_WORK_AREA: usize = 0x8014_770a;
const GET_PROFILE: usize = 0x8004_770d;
const SET_PROFILE: usize = 0x4004_770e;

const TITLE_CAPACITY: usize = 64;
const AT_FDCWD: usize = -100isize as usize;
const O_RDWR_CLOEXEC: usize = 0o2 | 0o2_000_000;
const PROT_READ_WRITE: usize = 3;
const MAP_SHARED: usize = 1;
const POLLIN: u16 = 1;
const CLOCK_MONOTONIC: usize = 1;

fn result(value: isize) -> Result<usize, Errno> {
    Errno::result(value)
}

fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn put(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
}

/// The monotonic clock in milliseconds, wrapping as event records' times do.
pub fn now_ms() -> u32 {
    let mut time = [0u64; 2];
    // SAFETY: `clock_gettime` writes one `timespec`, two 64-bit words, into `time`, which this
    // function owns.
    unsafe {
        call(
            number::CLOCK_GETTIME,
            [CLOCK_MONOTONIC, time.as_mut_ptr() as usize, 0, 0, 0, 0],
        )
    };
    (time[0].wrapping_mul(1000) + time[1] / 1_000_000) as u32
}

/// Where windows go on the screen, and the scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkArea {
    pub rect: Rect,
    pub scale: u32,
}

/// One connection to the window server.
#[derive(Debug)]
pub struct WindowServer {
    fd: usize,
}

impl WindowServer {
    /// Connects: `ENXIO` without a display.
    pub fn open() -> Result<Self, Errno> {
        let path: &CStr = c"/dev/window";
        // SAFETY: `openat` reads the path, a C string that lives for the whole program.
        let fd = unsafe {
            call(
                number::OPENAT,
                [AT_FDCWD, path.as_ptr() as usize, O_RDWR_CLOEXEC, 0, 0, 0],
            )
        };
        Ok(Self { fd: result(fd)? })
    }

    fn ioctl(&self, request: usize, argument: &mut [u8]) -> Result<(), Errno> {
        // SAFETY: the request reads and writes `argument`, which this function borrows, and no
        // more than its size, which the request number encodes and every caller matches.
        let answer = unsafe {
            call(
                number::IOCTL,
                [self.fd, request, argument.as_mut_ptr() as usize, 0, 0, 0],
            )
        };
        result(answer).map(|_| ())
    }

    /// Where windows go and the scale.
    pub fn work_area(&self) -> Result<WorkArea, Errno> {
        let mut area = [0u8; 20];
        self.ioctl(GET_WORK_AREA, &mut area)?;
        Ok(WorkArea {
            rect: Rect::new(
                word(&area, 0) as i32,
                word(&area, 4) as i32,
                word(&area, 8) as i32,
                word(&area, 12) as i32,
            ),
            scale: word(&area, 16).max(1),
        })
    }

    /// The desktop's layout profile's number (0 Windows, 1 macOS, 2 Linux).
    pub fn desktop_profile(&self) -> Result<u32, Errno> {
        let mut profile = [0u8; 4];
        self.ioctl(GET_PROFILE, &mut profile)?;
        Ok(word(&profile, 0))
    }

    /// Makes profile `profile` the desktop's, at once.
    pub fn set_desktop_profile(&self, profile: u32) -> Result<(), Errno> {
        self.ioctl(SET_PROFILE, &mut profile.to_ne_bytes())
    }
}

impl Drop for WindowServer {
    fn drop(&mut self) {
        // SAFETY: `close` ends the descriptor this value owns; nothing uses it after.
        unsafe { call(number::CLOSE, [self.fd, 0, 0, 0, 0, 0]) };
    }
}

/// A normal window of the program and its mapped pixels.
#[derive(Debug)]
pub struct Window {
    server: WindowServer,
    id: u32,
    pixels: usize,
    width: u32,
    height: u32,
    scale: u32,
    pointer: PointerShape,
}

impl Window {
    /// Creates a window titled `title` with a content of `width` by `height` at scale 1
    /// (multiplied by the desktop's scale), at least `minimum`; nothing is shown until the
    /// first [`Self::commit`].
    pub fn create(
        title: &str,
        width: u32,
        height: u32,
        minimum: (u32, u32),
    ) -> Result<Self, Errno> {
        let server = WindowServer::open()?;
        let scale = server.work_area()?.scale;
        let mut create = [0u8; 32];
        put(&mut create, 8, width * scale);
        put(&mut create, 12, height * scale);
        server.ioctl(CREATE, &mut create)?;
        let offset = u64::from_ne_bytes(create[24..32].try_into().unwrap_or([0; 8])) as usize;
        let mut window = Self {
            server,
            id: word(&create, 20),
            pixels: 0,
            width: word(&create, 8),
            height: word(&create, 12),
            scale,
            pointer: PointerShape::Arrow,
        };
        window.map(offset)?;
        let mut set_title = [0u8; 72];
        let mut end = title.len().min(TITLE_CAPACITY);
        while !title.is_char_boundary(end) {
            end -= 1;
        }
        let title = &title.as_bytes()[..end];
        put(&mut set_title, 0, window.id);
        put(&mut set_title, 4, title.len() as u32);
        set_title[8..8 + title.len()].copy_from_slice(title);
        window.server.ioctl(SET_TITLE, &mut set_title)?;
        let mut minimum_size = [0u8; 12];
        put(&mut minimum_size, 0, window.id);
        put(&mut minimum_size, 4, minimum.0 * scale);
        put(&mut minimum_size, 8, minimum.1 * scale);
        window.server.ioctl(SET_MINIMUM_SIZE, &mut minimum_size)?;
        Ok(window)
    }

    /// The connection the window was made on.
    pub fn server(&self) -> &WindowServer {
        &self.server
    }

    /// The content's size in buffer pixels.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// The desktop's scale.
    pub fn scale(&self) -> u32 {
        self.scale
    }

    fn length(&self) -> usize {
        self.width as usize * self.height as usize * 4
    }

    fn map(&mut self, offset: usize) -> Result<(), Errno> {
        // SAFETY: a new mapping, of memory nothing else in the program uses.
        let pixels = unsafe {
            call(
                number::MMAP,
                [
                    0,
                    self.length(),
                    PROT_READ_WRITE,
                    MAP_SHARED,
                    self.server.fd,
                    offset,
                ],
            )
        };
        self.pixels = result(pixels)?;
        Ok(())
    }

    fn unmap(&mut self) {
        if self.pixels != 0 {
            // SAFETY: the mapping is this window's, made by `map`; nothing borrows it now
            // (`pixels` borrows `self` mutably) and nothing uses it after.
            unsafe { call(number::MUNMAP, [self.pixels, self.length(), 0, 0, 0, 0]) };
            self.pixels = 0;
        }
    }

    /// The window's pixels, rows of [`Self::size`]'s width.
    pub fn pixels(&mut self) -> &mut [u32] {
        if self.pixels == 0 {
            return &mut [];
        }
        // SAFETY: the mapping is `width * height` writable pixels of this process, made by
        // `map` and kept until `unmap`, which needs `self` mutably, so nothing else borrows
        // it while this slice lives; the window server reads it only during `COMMIT`.
        unsafe {
            core::slice::from_raw_parts_mut(
                self.pixels as *mut u32,
                self.width as usize * self.height as usize,
            )
        }
    }

    /// Shows the rectangles `damage` names (at scale 1), or the whole content when
    /// `everything`; the first commit shows the window.
    pub fn commit(&self, damage: &Damage, everything: bool) -> Result<(), Errno> {
        let mut commit = [0u8; 136];
        put(&mut commit, 0, self.id);
        if !everything {
            let rects = damage.rects();
            put(&mut commit, 4, rects.len() as u32);
            for (index, rect) in rects.iter().enumerate() {
                let rect = rect.scaled(self.scale as i32).intersection(&Rect::new(
                    0,
                    0,
                    self.width as i32,
                    self.height as i32,
                ));
                let base = 8 + index * 16;
                for (field, value) in [rect.x, rect.y, rect.width, rect.height].iter().enumerate() {
                    put(&mut commit, base + field * 4, (*value).max(0) as u32);
                }
            }
        }
        self.server.ioctl(COMMIT, &mut commit)
    }

    /// Takes the content size the server gave (`width` by `height` buffer pixels): a new
    /// buffer, which the next commit shows whole.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), Errno> {
        let mut set_size = [0u8; 24];
        put(&mut set_size, 0, self.id);
        put(&mut set_size, 4, width);
        put(&mut set_size, 8, height);
        self.server.ioctl(SET_SIZE, &mut set_size)?;
        let offset = u64::from_ne_bytes(set_size[16..24].try_into().unwrap_or([0; 8])) as usize;
        self.unmap();
        self.width = word(&set_size, 4);
        self.height = word(&set_size, 8);
        self.map(offset)
    }

    /// Shows `shape` while the pointer is over the content.
    pub fn set_pointer(&mut self, shape: PointerShape) -> Result<(), Errno> {
        if shape == self.pointer {
            return Ok(());
        }
        let mut set_pointer = [0u8; 8];
        put(&mut set_pointer, 0, self.id);
        put(
            &mut set_pointer,
            4,
            match shape {
                PointerShape::Arrow => 0,
                PointerShape::IBeam => 1,
            },
        );
        self.server.ioctl(SET_POINTER, &mut set_pointer)?;
        self.pointer = shape;
        Ok(())
    }

    /// Waits up to `timeout_ms` (for ever when `None`) for event records and reads as many as
    /// fit `records`; how many bytes, 0 when the time ran out.
    pub fn read_events(&self, records: &mut [u8], timeout_ms: Option<u32>) -> Result<usize, Errno> {
        let mut poll = [0u8; 8];
        poll[0..4].copy_from_slice(&(self.server.fd as i32).to_ne_bytes());
        poll[4..6].copy_from_slice(&POLLIN.to_ne_bytes());
        let timeout = timeout_ms.map(|ms| [u64::from(ms / 1000), u64::from(ms % 1000) * 1_000_000]);
        let timeout_pointer = timeout.as_ref().map_or(0, |time| time.as_ptr() as usize);
        // SAFETY: `ppoll` reads the one `pollfd` in `poll` and the `timespec` at
        // `timeout_pointer` (or none), both owned here, and writes `revents` into `poll`.
        let ready = unsafe {
            call(
                number::PPOLL,
                [poll.as_mut_ptr() as usize, 1, timeout_pointer, 0, 8, 0],
            )
        };
        if result(ready)? == 0 {
            return Ok(0);
        }
        let length = records.len() / RECORD_SIZE * RECORD_SIZE;
        // SAFETY: `read` writes at most `length` bytes into `records`, owned by the caller and
        // borrowed here.
        let read = unsafe {
            call(
                number::READ,
                [
                    self.server.fd,
                    records.as_mut_ptr() as usize,
                    length,
                    0,
                    0,
                    0,
                ],
            )
        };
        result(read)
    }

    /// The window's id, as event records name it.
    pub fn id(&self) -> u32 {
        self.id
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        let mut destroy = self.id.to_ne_bytes();
        let _ = self.server.ioctl(DESTROY, &mut destroy);
        self.unmap();
    }
}
