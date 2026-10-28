//! `eggway`'s system calls (D140): the listening socket at `/run/user/0/wayland-0`, the loop
//! that waits in `ppoll` for clients, their window server connections and the next frame
//! callback (for ever when none is due), and the machine the compositor uses: sockets with
//! descriptors in their control data, client memory mapped read-only, windows on `/dev/window`
//! and the keyboard map in a sealed memory file. `eggway --status` says whether a server is
//! listening.

use crate::client::ClientSlot;
use crate::compositor::{Compositor, MAX_CLIENTS, MAX_RECEIVE_DESCRIPTORS, buffer_bytes};
use crate::system::{Fd, PoolPixels, Received, System, WindowBuffer};
use core::ffi::CStr;
use eggos_runtime::errno::Errno;
use eggos_runtime::startup::Startup;
use eggos_runtime::syscall::{self, number};
use eggos_wayland::blit::{OPAQUE_MASK, Rect};
use eggos_wayland::keymap::US_KEYMAP;

/// The folder of every user's runtime directory.
pub const RUNTIME_PARENT: &CStr = c"/run/user";
/// The runtime directory of root, the account every process runs as.
pub const RUNTIME_DIRECTORY: &CStr = c"/run/user/0";
/// The socket clients connect to: `$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY`.
pub const SOCKET_PATH: &CStr = c"/run/user/0/wayland-0";

const STATUS: &[u8] = b"--status";
const CONSOLE: usize = 2;
const STANDARD_OUTPUT: usize = 1;
const AT_FDCWD: usize = -100isize as usize;
const AF_UNIX: usize = 1;
const SOCK_STREAM: usize = 1;
const SOCK_NONBLOCK: usize = 0o4000;
const SOCK_CLOEXEC: usize = 0o2_000_000;
const O_RDWR: usize = 2;
const O_NONBLOCK: usize = 0o4000;
const O_CLOEXEC: usize = 0o2_000_000;
const LISTEN_BACKLOG: usize = 16;
const MSG_DONTWAIT: usize = 0x40;
const MSG_NOSIGNAL: usize = 0x4000;
const MSG_CMSG_CLOEXEC: usize = 0x4000_0000;
const MSG_CTRUNC: i32 = 0x8;
const SOL_SOCKET: i32 = 1;
const SCM_RIGHTS: i32 = 1;
const CMSG_HEADER: usize = 16;
const PROT_READ: usize = 1;
const PROT_WRITE: usize = 2;
const MAP_SHARED: usize = 1;
const MAP_PRIVATE: usize = 2;
const MAP_ANONYMOUS: usize = 0x20;
const PAGE: usize = 4096;
const MFD_CLOEXEC: usize = 1;
const MFD_ALLOW_SEALING: usize = 2;
const F_ADD_SEALS: usize = 1033;
const SEALS: usize = 1 | 2 | 4;
const POLLIN: i16 = 1;
const POLLOUT: i16 = 4;
const POLLERR: i16 = 8;
const POLLHUP: i16 = 0x10;
const SIGPIPE: usize = 13;
const SIG_IGN: u64 = 1;
const SIGNAL_SET_BYTES: usize = 8;
const CLOCK_MONOTONIC: usize = 1;
const RUNTIME_PARENT_MODE: usize = 0o755;
const RUNTIME_DIRECTORY_MODE: usize = 0o700;
const ACCEPTS_PER_WAKE: usize = 4;

mod request {
    pub const CREATE: usize = 0xc020_7701;
    pub const SET_TITLE: usize = 0x4048_7702;
    pub const SET_SIZE: usize = 0xc018_7703;
    pub const SET_MINIMUM_SIZE: usize = 0x400c_7704;
    pub const SET_STATE: usize = 0x4008_7706;
    pub const COMMIT: usize = 0x4088_7708;
    pub const DESTROY: usize = 0x4004_7709;
    pub const GET_WORK_AREA: usize = 0x8014_770a;
    pub const START_MOVE_OR_RESIZE: usize = 0x400c_7712;
}

const WINDOW_DEVICE: &CStr = c"/dev/window";
const KEYMAP_NAME: &CStr = c"eggway-keymap";
const TITLE_BYTES: usize = 64;
const MAX_COMMIT_RECTS: usize = 8;

#[repr(C)]
struct SocketAddress {
    family: u16,
    path: [u8; 108],
}

#[repr(C)]
struct IoVector {
    base: usize,
    length: usize,
}

#[repr(C)]
struct MessageHeader {
    name: usize,
    name_length: u32,
    name_padding: u32,
    buffers: usize,
    buffer_count: usize,
    control: usize,
    control_length: usize,
    flags: i32,
    flags_padding: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct PollEntry {
    descriptor: i32,
    events: i16,
    returned: i16,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Watched {
    Listener,
    Socket(usize),
    Device(usize),
}

fn call(number: usize, arguments: [usize; 6]) -> Result<usize, Errno> {
    // SAFETY: every call made through here passes only memory this program owns and borrows
    // for the call (paths, socket addresses, message headers with their buffers and control
    // data, poll entries, ioctl arguments sized as their request numbers say, clocks), sized as
    // the call reads or writes it; none maps, unmaps or protects memory.
    Errno::result(unsafe { syscall::call(number, arguments) })
}

fn say(parts: &[&[u8]]) {
    for part in parts {
        syscall::write_all(CONSOLE, part);
    }
}

fn now_ms() -> u64 {
    let mut time = [0i64; 2];
    let _ = call(
        number::CLOCK_GETTIME,
        [CLOCK_MONOTONIC, time.as_mut_ptr() as usize, 0, 0, 0, 0],
    );
    time[0] as u64 * 1000 + time[1] as u64 / 1_000_000
}

fn close(descriptor: Fd) {
    let _ = call(number::CLOSE, [descriptor as usize, 0, 0, 0, 0, 0]);
}

fn put(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
}

fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn address_of(path: &CStr) -> (SocketAddress, usize) {
    let mut address = SocketAddress {
        family: AF_UNIX as u16,
        path: [0; 108],
    };
    let bytes = path.to_bytes();
    let length = bytes.len().min(address.path.len() - 1);
    address.path[..length].copy_from_slice(&bytes[..length]);
    (address, 2 + length + 1)
}

fn new_socket(flags: usize) -> Result<Fd, Errno> {
    call(
        number::SOCKET,
        [AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC | flags, 0, 0, 0, 0],
    )
    .map(|descriptor| descriptor as Fd)
}

fn someone_listens() -> bool {
    let Ok(socket) = new_socket(0) else {
        return false;
    };
    let (address, length) = address_of(SOCKET_PATH);
    let connected = call(
        number::CONNECT,
        [
            socket as usize,
            &raw const address as usize,
            length,
            0,
            0,
            0,
        ],
    )
    .is_ok();
    close(socket);
    connected
}

fn listen() -> Result<Fd, Errno> {
    for (folder, mode) in [
        (RUNTIME_PARENT, RUNTIME_PARENT_MODE),
        (RUNTIME_DIRECTORY, RUNTIME_DIRECTORY_MODE),
    ] {
        match call(
            number::MKDIRAT,
            [AT_FDCWD, folder.as_ptr() as usize, mode, 0, 0, 0],
        ) {
            Ok(_) | Err(Errno::EEXIST) => {}
            Err(error) => return Err(error),
        }
    }
    let _ = call(
        number::UNLINKAT,
        [AT_FDCWD, SOCKET_PATH.as_ptr() as usize, 0, 0, 0, 0],
    );
    let socket = new_socket(SOCK_NONBLOCK)?;
    let (address, length) = address_of(SOCKET_PATH);
    let bound = call(
        number::BIND,
        [
            socket as usize,
            &raw const address as usize,
            length,
            0,
            0,
            0,
        ],
    )
    .and_then(|_| {
        call(
            number::LISTEN,
            [socket as usize, LISTEN_BACKLOG, 0, 0, 0, 0],
        )
    });
    if let Err(error) = bound {
        close(socket);
        return Err(error);
    }
    Ok(socket)
}

fn ignore_broken_pipes() {
    let action = [SIG_IGN, 0, 0, 0];
    let _ = call(
        number::RT_SIGACTION,
        [SIGPIPE, action.as_ptr() as usize, 0, SIGNAL_SET_BYTES, 0, 0],
    );
}

/// `eggway`'s main function: with `--status`, says whether a server listens (status 0) or
/// not (1); otherwise serves Wayland clients until it fails.
pub fn run(startup: &Startup) -> i32 {
    ignore_broken_pipes();
    let status = startup
        .arguments()
        .skip(1)
        .any(|argument| argument.to_bytes() == STATUS);
    let running = someone_listens();
    if status {
        let line: &[u8] = match running {
            true => b"eggway: running on /run/user/0/wayland-0\n",
            false => b"eggway: not running\n",
        };
        syscall::write_all(STANDARD_OUTPUT, line);
        return i32::from(!running);
    }
    if running {
        say(&[b"eggway: a Wayland server already runs on /run/user/0/wayland-0\n"]);
        return 1;
    }
    let listener = match listen() {
        Ok(listener) => listener,
        Err(error) => {
            say(&[
                b"eggway: cannot listen on /run/user/0/wayland-0: ",
                error.message(),
                b"\n",
            ]);
            return 1;
        }
    };
    serve(listener)
}

fn serve(listener: Fd) -> ! {
    let mut system = Linux;
    let mut compositor = Compositor::new();
    let mut entries = [PollEntry {
        descriptor: -1,
        events: 0,
        returned: 0,
    }; 1 + 2 * MAX_CLIENTS];
    let mut watched = [Watched::Listener; 1 + 2 * MAX_CLIENTS];
    loop {
        let mut count = 0;
        let mut add = |descriptor: Fd, events: i16, what: Watched| {
            entries[count] = PollEntry {
                descriptor,
                events,
                returned: 0,
            };
            watched[count] = what;
            count += 1;
        };
        add(listener, POLLIN, Watched::Listener);
        let mut backlog = false;
        for index in 0..MAX_CLIENTS {
            let Some(watch) = compositor.watch(index) else {
                continue;
            };
            let mut events = 0;
            if watch.wants_input {
                events |= POLLIN;
            }
            if watch.wants_output {
                events |= POLLOUT;
            }
            add(watch.socket, events, Watched::Socket(index));
            if let Some(device) = watch.device {
                add(device, POLLIN, Watched::Device(index));
            }
            backlog |= compositor.has_backlog(index);
        }
        let timeout = match backlog {
            true => Some(0),
            false => compositor.timeout_ms(now_ms()),
        };
        let time = timeout.map(|ms| [(ms / 1000) as i64, (ms % 1000 * 1_000_000) as i64]);
        let time_pointer = time.as_ref().map_or(0, |time| time.as_ptr() as usize);
        let _ = call(
            number::PPOLL,
            [entries.as_mut_ptr() as usize, count, time_pointer, 0, 8, 0],
        );
        for at in 0..count {
            let returned = entries[at].returned;
            if returned == 0 {
                continue;
            }
            match watched[at] {
                Watched::Listener => accept(&mut compositor, &mut system, listener),
                Watched::Socket(index) => {
                    if returned & (POLLIN | POLLHUP | POLLERR) != 0 {
                        compositor.readable(&mut system, index);
                    }
                    if returned & POLLOUT != 0 {
                        compositor.writable(&mut system, index);
                    }
                }
                Watched::Device(index) => compositor.device_readable(&mut system, index),
            }
        }
        for index in 0..MAX_CLIENTS {
            if compositor.has_backlog(index) {
                compositor.readable(&mut system, index);
            }
        }
        compositor.tick(&mut system);
    }
}

fn accept(compositor: &mut Compositor, system: &mut Linux, listener: Fd) {
    for _ in 0..ACCEPTS_PER_WAKE {
        let Ok(socket) = call(
            number::ACCEPT4,
            [listener as usize, 0, 0, SOCK_NONBLOCK | SOCK_CLOEXEC, 0, 0],
        ) else {
            return;
        };
        compositor.add_client(system, socket as Fd);
    }
}

/// The machine as Linux system calls reach it.
#[derive(Debug, Clone, Copy, Default)]
pub struct Linux;

fn map(
    length: usize,
    protection: usize,
    flags: usize,
    descriptor: usize,
    offset: usize,
) -> Result<usize, Errno> {
    // SAFETY: a new mapping at an address the kernel chooses, so it replaces nothing the
    // program uses; nothing refers to it until this returns.
    Errno::result(unsafe {
        syscall::call(
            number::MMAP,
            [0, length, protection, flags, descriptor, offset],
        )
    })
}

fn unmap(address: usize, length: usize) {
    // SAFETY: callers unmap only a mapping this program made with `map` and no longer uses:
    // no reference into it is alive.
    unsafe { syscall::call(number::MUNMAP, [address, length, 0, 0, 0, 0]) };
}

const fn pages(length: usize) -> usize {
    length.div_ceil(PAGE) * PAGE
}

const CONTROL_BYTES: usize = CMSG_HEADER + MAX_RECEIVE_DESCRIPTORS * 4;
const CONTROL_WORDS: usize = CONTROL_BYTES.div_ceil(8);

impl System for Linux {
    fn now_ms(&mut self) -> u64 {
        now_ms()
    }

    fn new_client(&mut self, socket: Fd) -> Option<&'static mut ClientSlot> {
        let length = pages(size_of::<ClientSlot>());
        let address = map(
            length,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS,
            usize::MAX,
            0,
        )
        .ok()?;
        let slot = address as *mut ClientSlot;
        // SAFETY: `slot` is a new private mapping of at least `size_of::<ClientSlot>()`
        // writable bytes, page-aligned (enough for `ClientSlot`'s alignment), that nothing else
        // refers to; writing a whole value there needs no old value dropped.
        unsafe { slot.write(ClientSlot::new(socket)) };
        // SAFETY: the mapping now holds a valid `ClientSlot`; this is the only reference made to
        // it, handed to the compositor, which gives it back to `free_client` before the mapping
        // goes.
        Some(unsafe { &mut *slot })
    }

    fn free_client(&mut self, client: &'static mut ClientSlot) {
        let address = client as *mut ClientSlot as usize;
        unmap(address, pages(size_of::<ClientSlot>()));
    }

    fn receive(
        &mut self,
        socket: Fd,
        bytes: &mut [u8],
        descriptors: &mut [Fd],
    ) -> Result<Received, Errno> {
        let mut vector = IoVector {
            base: bytes.as_mut_ptr() as usize,
            length: bytes.len(),
        };
        let mut control = [0u64; CONTROL_WORDS];
        let room = descriptors.len().min(MAX_RECEIVE_DESCRIPTORS);
        let control_length = match room {
            0 => 0,
            room => CMSG_HEADER + (room * 4).div_ceil(8) * 8,
        };
        let mut header = MessageHeader {
            name: 0,
            name_length: 0,
            name_padding: 0,
            buffers: &raw mut vector as usize,
            buffer_count: 1,
            control: control.as_mut_ptr() as usize,
            control_length,
            flags: 0,
            flags_padding: 0,
        };
        let received = call(
            number::RECVMSG,
            [
                socket as usize,
                &raw mut header as usize,
                MSG_DONTWAIT | MSG_CMSG_CLOEXEC,
                0,
                0,
                0,
            ],
        )?;
        let mut control_bytes = [0u8; CONTROL_WORDS * 8];
        for (chunk, word) in control_bytes.as_chunks_mut::<8>().0.iter_mut().zip(control) {
            *chunk = word.to_ne_bytes();
        }
        let control = &control_bytes[..header.control_length.min(CONTROL_BYTES)];
        let mut taken = 0;
        let mut at = 0;
        while at + CMSG_HEADER <= control.len() {
            let length =
                u64::from_ne_bytes(control[at..at + 8].try_into().unwrap_or([0; 8])) as usize;
            let level = word(control, at + 8) as i32;
            let kind = word(control, at + 12) as i32;
            if length < CMSG_HEADER || at + length > control.len() {
                break;
            }
            if level == SOL_SOCKET && kind == SCM_RIGHTS {
                for chunk in control[at + CMSG_HEADER..at + length].as_chunks::<4>().0 {
                    let descriptor = i32::from_ne_bytes(*chunk);
                    match descriptors.get_mut(taken) {
                        Some(slot) => {
                            *slot = descriptor;
                            taken += 1;
                        }
                        None => close(descriptor),
                    }
                }
            }
            at += length.div_ceil(8) * 8;
        }
        Ok(Received {
            bytes: received,
            descriptors: taken,
            truncated: header.flags & MSG_CTRUNC != 0,
        })
    }

    fn send(&mut self, socket: Fd, bytes: &[u8], descriptors: &[Fd]) -> Result<usize, Errno> {
        let mut vector = IoVector {
            base: bytes.as_ptr() as usize,
            length: bytes.len(),
        };
        let mut control = [0u8; CMSG_HEADER + 4 * 4];
        let count = descriptors.len().min(4);
        let control_length = match count {
            0 => 0,
            count => {
                let length = CMSG_HEADER + count * 4;
                control[..8].copy_from_slice(&(length as u64).to_ne_bytes());
                put(&mut control, 8, SOL_SOCKET as u32);
                put(&mut control, 12, SCM_RIGHTS as u32);
                for (index, &descriptor) in descriptors[..count].iter().enumerate() {
                    put(&mut control, CMSG_HEADER + index * 4, descriptor as u32);
                }
                (length).div_ceil(8) * 8
            }
        };
        let mut aligned = [0u64; (CMSG_HEADER + 4 * 4) / 8];
        for (word, chunk) in aligned.iter_mut().zip(control.as_chunks::<8>().0) {
            *word = u64::from_ne_bytes(*chunk);
        }
        let mut header = MessageHeader {
            name: 0,
            name_length: 0,
            name_padding: 0,
            buffers: &raw mut vector as usize,
            buffer_count: 1,
            control: match control_length {
                0 => 0,
                _ => aligned.as_mut_ptr() as usize,
            },
            control_length,
            flags: 0,
            flags_padding: 0,
        };
        call(
            number::SENDMSG,
            [
                socket as usize,
                &raw mut header as usize,
                MSG_DONTWAIT | MSG_NOSIGNAL,
                0,
                0,
                0,
            ],
        )
    }

    fn close(&mut self, descriptor: Fd) {
        close(descriptor);
    }

    fn map_pool(&mut self, descriptor: Fd, length: usize) -> Result<usize, Errno> {
        map(length, PROT_READ, MAP_SHARED, descriptor as usize, 0)
    }

    fn unmap_pool(&mut self, address: usize, length: usize) {
        unmap(address, length);
    }

    fn copy_rows(&mut self, from: PoolPixels, window: WindowBuffer, rect: Rect) {
        let rect = rect.clipped(window.width, window.height);
        let (left, width) = (rect.x as usize, rect.width as usize);
        for row in rect.y as usize..rect.bottom() as usize {
            let Some(start) = row
                .checked_mul(from.stride)
                .and_then(|bytes| bytes.checked_add(from.offset))
                .and_then(|bytes| bytes.checked_add(left * 4))
            else {
                return;
            };
            if start.saturating_add(width * 4) > from.length {
                return;
            }
            let source = (from.address + start) as *const u32;
            let destination =
                (window.address + (row * window.width as usize + left) * 4) as *mut u32;
            for x in 0..width {
                // SAFETY: the pixel is inside the pool's mapping (`start + 4 * width` is at most
                // its length) and 4-aligned (the mapping is page-aligned, the buffer's offset and
                // stride multiples of 4). The client may change that memory at any moment from
                // its own process, so it is read with one volatile load and never through a
                // reference; any 32 bits are a pixel, and a torn picture is accepted.
                let pixel = unsafe { source.wrapping_add(x).read_volatile() };
                // SAFETY: the pixel is inside the window's buffer (the rectangle is clipped to
                // its width and height) and 4-aligned; this thread is the only one that writes the
                // mapping and the window server reads it only during `COMMIT`, which is not in
                // flight; no reference to it exists.
                unsafe { destination.wrapping_add(x).write(pixel & OPAQUE_MASK) };
            }
        }
    }

    fn keymap(&mut self) -> Result<(Fd, u32), Errno> {
        let descriptor = call(
            number::MEMFD_CREATE,
            [
                KEYMAP_NAME.as_ptr() as usize,
                MFD_CLOEXEC | MFD_ALLOW_SEALING,
                0,
                0,
                0,
                0,
            ],
        )?;
        let text = US_KEYMAP.as_bytes();
        let written = syscall::write_all(descriptor, text) && syscall::write_all(descriptor, &[0]);
        let sealed = call(number::FCNTL, [descriptor, F_ADD_SEALS, SEALS, 0, 0, 0]);
        if !written || sealed.is_err() {
            close(descriptor as Fd);
            return Err(Errno::ENOMEM);
        }
        Ok((descriptor as Fd, (text.len() + 1) as u32))
    }

    fn open_window_device(&mut self) -> Result<Fd, Errno> {
        call(
            number::OPENAT,
            [
                AT_FDCWD,
                WINDOW_DEVICE.as_ptr() as usize,
                O_RDWR | O_NONBLOCK | O_CLOEXEC,
                0,
                0,
                0,
            ],
        )
        .map(|descriptor| descriptor as Fd)
    }

    fn create_window(
        &mut self,
        device: Fd,
        width: u32,
        height: u32,
        flags: u32,
    ) -> Result<WindowBuffer, Errno> {
        let mut create = [0u8; 32];
        put(&mut create, 8, width);
        put(&mut create, 12, height);
        put(&mut create, 16, flags);
        ioctl(device, request::CREATE, &mut create)?;
        let mut window = WindowBuffer {
            id: word(&create, 20),
            width: word(&create, 8),
            height: word(&create, 12),
            address: 0,
        };
        let offset = u64::from_ne_bytes(create[24..32].try_into().unwrap_or([0; 8])) as usize;
        match map_window(device, &window, offset) {
            Ok(address) => window.address = address,
            Err(error) => {
                let _ = ioctl(device, request::DESTROY, &mut window.id.to_ne_bytes());
                return Err(error);
            }
        }
        Ok(window)
    }

    fn resize_window(
        &mut self,
        device: Fd,
        window: &mut WindowBuffer,
        width: u32,
        height: u32,
    ) -> Result<(), Errno> {
        let mut set_size = [0u8; 24];
        put(&mut set_size, 0, window.id);
        put(&mut set_size, 4, width);
        put(&mut set_size, 8, height);
        ioctl(device, request::SET_SIZE, &mut set_size)?;
        let resized = WindowBuffer {
            id: window.id,
            width: word(&set_size, 4),
            height: word(&set_size, 8),
            address: 0,
        };
        let offset = u64::from_ne_bytes(set_size[16..24].try_into().unwrap_or([0; 8])) as usize;
        let address = map_window(device, &resized, offset)?;
        unmap(window.address, buffer_bytes(window));
        *window = WindowBuffer { address, ..resized };
        Ok(())
    }

    fn destroy_window(&mut self, device: Fd, window: WindowBuffer) {
        let _ = ioctl(device, request::DESTROY, &mut window.id.to_ne_bytes());
        unmap(window.address, buffer_bytes(&window));
    }

    fn set_title(&mut self, device: Fd, window: u32, title: &[u8]) -> Result<(), Errno> {
        let mut set_title = [0u8; 8 + TITLE_BYTES];
        let length = title.len().min(TITLE_BYTES);
        put(&mut set_title, 0, window);
        put(&mut set_title, 4, length as u32);
        set_title[8..8 + length].copy_from_slice(&title[..length]);
        ioctl(device, request::SET_TITLE, &mut set_title)
    }

    fn set_minimum_size(
        &mut self,
        device: Fd,
        window: u32,
        width: u32,
        height: u32,
    ) -> Result<(), Errno> {
        let mut minimum = [0u8; 12];
        for (index, value) in [window, width, height].into_iter().enumerate() {
            put(&mut minimum, index * 4, value);
        }
        ioctl(device, request::SET_MINIMUM_SIZE, &mut minimum)
    }

    fn set_state(&mut self, device: Fd, window: u32, state: u32) -> Result<(), Errno> {
        let mut argument = [0u8; 8];
        put(&mut argument, 0, window);
        put(&mut argument, 4, state);
        ioctl(device, request::SET_STATE, &mut argument)
    }

    fn start_move_or_resize(
        &mut self,
        device: Fd,
        window: u32,
        kind: u32,
        edges: u32,
    ) -> Result<(), Errno> {
        let mut argument = [0u8; 12];
        for (index, value) in [window, kind, edges].into_iter().enumerate() {
            put(&mut argument, index * 4, value);
        }
        ioctl(device, request::START_MOVE_OR_RESIZE, &mut argument)
    }

    fn commit(&mut self, device: Fd, window: u32, rects: &[Rect]) -> Result<(), Errno> {
        let mut commit = [0u8; 8 + 16 * MAX_COMMIT_RECTS];
        let rects = &rects[..rects.len().min(MAX_COMMIT_RECTS)];
        put(&mut commit, 0, window);
        put(&mut commit, 4, rects.len() as u32);
        for (index, rect) in rects.iter().enumerate() {
            let base = 8 + index * 16;
            for (field, value) in [rect.x, rect.y, rect.width, rect.height]
                .into_iter()
                .enumerate()
            {
                put(&mut commit, base + field * 4, value);
            }
        }
        ioctl(device, request::COMMIT, &mut commit)
    }

    fn work_area(&mut self) -> (u32, u32) {
        let Ok(device) = self.open_window_device() else {
            return (0, 0);
        };
        let mut area = [0u8; 20];
        let size = match ioctl(device, request::GET_WORK_AREA, &mut area) {
            Ok(()) => (word(&area, 8), word(&area, 12)),
            Err(_) => (0, 0),
        };
        close(device);
        size
    }

    fn read_events(&mut self, device: Fd, records: &mut [u8]) -> Result<usize, Errno> {
        match call(
            number::READ,
            [
                device as usize,
                records.as_mut_ptr() as usize,
                records.len(),
                0,
                0,
                0,
            ],
        ) {
            Err(Errno::EAGAIN) => Ok(0),
            read => read,
        }
    }
}

fn ioctl(device: Fd, request: usize, argument: &mut [u8]) -> Result<(), Errno> {
    call(
        number::IOCTL,
        [
            device as usize,
            request,
            argument.as_mut_ptr() as usize,
            0,
            0,
            0,
        ],
    )
    .map(|_| ())
}

fn map_window(device: Fd, window: &WindowBuffer, offset: usize) -> Result<usize, Errno> {
    map(
        buffer_bytes(window),
        PROT_READ | PROT_WRITE,
        MAP_SHARED,
        device as usize,
        offset,
    )
}
