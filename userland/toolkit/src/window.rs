//! The connection to the window server: `/dev/window` (docs/windows.md, D127), its requests,
//! a window's mapped pixels and its event records.

use crate::damage::Damage;
use crate::geometry::Rect;
use crate::input::RECORD_SIZE;
use crate::tree::PointerShape;
use core::ffi::CStr;
use eggos_element_tree::{MAX_TEXT_BYTES, answer_query, read_query, request, size};
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
const GET_KEYBOARD_LAYOUT: usize = 0x8008_771d;
const SET_KEYBOARD_LAYOUT: usize = 0x4004_771e;
const SET_INPUT_PREFERENCES: usize = 0x4014_771f;
const SET_CLIPBOARD: usize = 0x4010_7720;
const SET_KEY_ACCESSIBILITY: usize = 0x400c_7724;
const GET_CLIPBOARD: usize = 0xc020_7721;
const CAPS_LOCK_FLAG: u32 = 1;
const KEYBOARD_FLAG: u32 = 2;

/// The window flag that has every Ctrl key reach the window, as a terminal needs (`CREATE`'s
/// `TAKES_CTRL_KEYS`, docs/windows.md).
pub const TAKES_CTRL_KEYS: u32 = 2;
/// The window flag that keeps its size: no resize zones, no maximise (`CREATE`'s `FIXED_SIZE`).
pub const FIXED_SIZE: u32 = 1;
/// The window flag that refuses injected input (`wm click`, `wm type`, `wm key`, a menu
/// chosen for an agent): only the person's own pointer and keys reach it (`CREATE`'s
/// `PERSON_INPUT_ONLY`, D193's grant prompt).
pub const PERSON_INPUT_ONLY: u32 = 4;

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

/// The keyboard, as the window server reports it (D185).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Keyboard {
    /// The layout's number in the settings file's `KEYBOARD_LAYOUTS`.
    pub layout: u32,
    /// Whether the machine has a keyboard.
    pub present: bool,
    /// Whether Caps Lock is on.
    pub caps_lock: bool,
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

    fn ioctl_with_buffer(
        &self,
        request: usize,
        buffer: *const u8,
        length: usize,
        offset: usize,
    ) -> Result<[u8; 32], Errno> {
        let mut argument = [0u8; 32];
        argument[..8].copy_from_slice(&(buffer as u64).to_ne_bytes());
        argument[8..16].copy_from_slice(&(length as u64).to_ne_bytes());
        argument[16..24].copy_from_slice(&(offset as u64).to_ne_bytes());
        // SAFETY: the request reads its 16- or 32-byte argument, which this function owns, and
        // reads or writes at most `length` bytes at `buffer`, which every caller borrows for the
        // call: shared for a request that only reads it, exclusively for one that writes it.
        let answer = unsafe {
            call(
                number::IOCTL,
                [self.fd, request, argument.as_mut_ptr() as usize, 0, 0, 0],
            )
        };
        result(answer).map(|_| argument)
    }

    /// Turns sticky keys and slow keys on or off, slow keys counting a key once held
    /// `slow_keys_ms` (`SET_KEY_ACCESSIBILITY`, D205).
    pub fn set_key_accessibility(
        &self,
        sticky_keys: bool,
        slow_keys: bool,
        slow_keys_ms: u32,
    ) -> Result<(), Errno> {
        let mut bytes = [0u8; 12];
        for (place, word) in bytes.as_chunks_mut::<4>().0.iter_mut().zip([
            u32::from(sticky_keys),
            u32::from(slow_keys),
            slow_keys_ms,
        ]) {
            *place = word.to_ne_bytes();
        }
        self.ioctl(SET_KEY_ACCESSIBILITY, &mut bytes)
    }

    /// Makes `text` the clipboard every app shares (`SET_CLIPBOARD`, D197).
    pub fn set_clipboard(&self, text: &str) -> Result<(), Errno> {
        self.ioctl_with_buffer(SET_CLIPBOARD, text.as_ptr(), text.len(), 0)
            .map(|_| ())
    }

    /// Copies the shared clipboard's text from byte `offset` on into `into`, as much as fits
    /// (`GET_CLIPBOARD`, D197): its whole length, which may be more than `into` holds, and its
    /// serial, which changes with every copy.
    pub fn clipboard(&self, offset: usize, into: &mut [u8]) -> Result<(usize, u32), Errno> {
        let reply = self.ioctl_with_buffer(GET_CLIPBOARD, into.as_mut_ptr(), into.len(), offset)?;
        let word = |at: usize| {
            u32::from_ne_bytes([reply[at], reply[at + 1], reply[at + 2], reply[at + 3]])
        };
        Ok((word(24) as usize, word(28)))
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

    /// The keyboard (`GET_KEYBOARD_LAYOUT`, D185): its layout's number in the settings file's
    /// `KEYBOARD_LAYOUTS`, whether the machine has a keyboard, and whether Caps Lock is on.
    pub fn keyboard(&self) -> Result<Keyboard, Errno> {
        let mut reply = [0u8; 8];
        self.ioctl(GET_KEYBOARD_LAYOUT, &mut reply)?;
        let flags = word(&reply, 4);
        Ok(Keyboard {
            layout: word(&reply, 0),
            present: flags & KEYBOARD_FLAG != 0,
            caps_lock: flags & CAPS_LOCK_FLAG != 0,
        })
    }

    /// Makes keyboard layout `layout` (its number in the settings file's `KEYBOARD_LAYOUTS`)
    /// the keyboard's from the next key on (`SET_KEYBOARD_LAYOUT`, D185).
    pub fn set_keyboard_layout(&self, layout: u32) -> Result<(), Errno> {
        self.ioctl(SET_KEYBOARD_LAYOUT, &mut layout.to_ne_bytes())
    }

    /// Uses the mouse and key preferences `words` holds from the next event on
    /// (`SET_INPUT_PREFERENCES`, D195): left-handed, natural scrolling, pointer speed, repeat
    /// delay in milliseconds and repeats a second, each a 32-bit word.
    pub fn set_input_preferences(&self, words: [u32; 5]) -> Result<(), Errno> {
        let mut bytes = [0u8; 20];
        for (place, word) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(words) {
            *place = word.to_ne_bytes();
        }
        self.ioctl(SET_INPUT_PREFERENCES, &mut bytes)
    }

    /// The text of the action query `serial` (`READ_QUERY`, D161): `set-text`'s new text,
    /// copied into `text`; returns its length. `ENOENT` once the query is answered or its
    /// asker stopped waiting.
    pub fn query_text(&self, serial: u32, text: &mut [u8; MAX_TEXT_BYTES]) -> Result<usize, Errno> {
        let mut query = [0u8; size::READ_QUERY];
        put(&mut query, read_query::SERIAL, serial);
        self.ioctl(request::READ_QUERY as usize, &mut query)?;
        let length = (word(&query, read_query::TEXT_LENGTH) as usize).min(MAX_TEXT_BYTES);
        text[..length].copy_from_slice(&query[read_query::TEXT..read_query::TEXT + length]);
        Ok(length)
    }

    /// Answers query `serial` with `outcome` and, for a tree, the tree's bytes
    /// (`ANSWER_QUERY`, D161).
    pub fn answer_query(&self, serial: u32, outcome: u32, tree: &[u8]) -> Result<(), Errno> {
        let mut answer = [0u8; size::ANSWER_QUERY];
        put(&mut answer, answer_query::SERIAL, serial);
        put(&mut answer, answer_query::OUTCOME, outcome);
        answer[answer_query::BUFFER..answer_query::BUFFER + 8]
            .copy_from_slice(&(tree.as_ptr() as u64).to_ne_bytes());
        answer[answer_query::LENGTH..answer_query::LENGTH + 8]
            .copy_from_slice(&(tree.len() as u64).to_ne_bytes());
        self.ioctl(request::ANSWER_QUERY as usize, &mut answer)
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
        Self::create_with_flags(title, width, height, minimum, 0)
    }

    /// [`Self::create`] with `CREATE`'s `flags` ([`TAKES_CTRL_KEYS`]).
    pub fn create_with_flags(
        title: &str,
        width: u32,
        height: u32,
        minimum: (u32, u32),
        flags: u32,
    ) -> Result<Self, Errno> {
        let server = WindowServer::open()?;
        let scale = server.work_area()?.scale;
        let mut create = [0u8; 32];
        put(&mut create, 8, width * scale);
        put(&mut create, 12, height * scale);
        put(&mut create, 16, flags);
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
        window.set_title(title)?;
        let mut minimum_size = [0u8; 12];
        put(&mut minimum_size, 0, window.id);
        put(&mut minimum_size, 4, minimum.0 * scale);
        put(&mut minimum_size, 8, minimum.1 * scale);
        window.server.ioctl(SET_MINIMUM_SIZE, &mut minimum_size)?;
        Ok(window)
    }

    /// Gives the window the title `title`, cut at 64 bytes on a character boundary.
    pub fn set_title(&self, title: &str) -> Result<(), Errno> {
        let mut set_title = [0u8; 72];
        let mut end = title.len().min(TITLE_CAPACITY);
        while !title.is_char_boundary(end) {
            end -= 1;
        }
        let title = &title.as_bytes()[..end];
        put(&mut set_title, 0, self.id);
        put(&mut set_title, 4, title.len() as u32);
        set_title[8..8 + title.len()].copy_from_slice(title);
        self.server.ioctl(SET_TITLE, &mut set_title)
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
    /// fit `records`; how many bytes, 0 when the time ran out or a signal interrupted the wait.
    pub fn read_events(&self, records: &mut [u8], timeout_ms: Option<u32>) -> Result<usize, Errno> {
        self.read_events_or(records, timeout_ms, None)
            .map(|(read, _)| read)
    }

    /// [`Self::read_events`], also waking when descriptor `also`, if any, has something to
    /// read or hung up, as a terminal's pseudo-terminal does (D187); returns how many bytes of
    /// records it read, and whether `also` is ready.
    pub fn read_events_or(
        &self,
        records: &mut [u8],
        timeout_ms: Option<u32>,
        also: Option<usize>,
    ) -> Result<(usize, bool), Errno> {
        let mut poll = [0u8; 16];
        poll[0..4].copy_from_slice(&(self.server.fd as i32).to_ne_bytes());
        poll[4..6].copy_from_slice(&POLLIN.to_ne_bytes());
        let count = match also {
            Some(descriptor) => {
                poll[8..12].copy_from_slice(&(descriptor as i32).to_ne_bytes());
                poll[12..14].copy_from_slice(&POLLIN.to_ne_bytes());
                2
            }
            None => 1,
        };
        let timeout = timeout_ms.map(|ms| [u64::from(ms / 1000), u64::from(ms % 1000) * 1_000_000]);
        let timeout_pointer = timeout.as_ref().map_or(0, |time| time.as_ptr() as usize);
        // SAFETY: `ppoll` reads the `count` `pollfd`s in `poll` and the `timespec` at
        // `timeout_pointer` (or none), all owned here, and writes their `revents` into `poll`.
        let ready = unsafe {
            call(
                number::PPOLL,
                [poll.as_mut_ptr() as usize, count, timeout_pointer, 0, 8, 0],
            )
        };
        match result(ready) {
            Ok(0) | Err(Errno::EINTR) => return Ok((0, false)),
            Ok(_) => {}
            Err(error) => return Err(error),
        }
        let returned = |at: usize| u16::from_ne_bytes([poll[at], poll[at + 1]]) != 0;
        let also_ready = count == 2 && returned(14);
        if !returned(6) {
            return Ok((0, also_ready));
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
        match result(read) {
            Err(Errno::EINTR) => Ok((0, also_ready)),
            read => read.map(|read| (read, also_ready)),
        }
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
