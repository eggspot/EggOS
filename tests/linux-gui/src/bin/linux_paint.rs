//! `linux-paint`: a Wayland client written by hand against the wire protocol (no libwayland),
//! built from our own source with Rust's standard library and linked statically with musl,
//! that the Linux GUI boot test runs from `eggsh` (D140). It connects to
//! `$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY`, binds the globals, checks the keyboard map, opens an
//! `xdg_toplevel` and paints the scene of `eggos_linux_gui_tests` into a `wl_shm` buffer from a
//! memory file. A click on the target recolours it, a key adds a mark, a press on the handle
//! asks the compositor to move the window (and, on the next pointer enter, asks again with
//! that press's now stale serial), a new size from the compositor is painted at that size, and
//! the close request ends the program with 0. It writes one `LINUX-PAINT` line for each step to
//! its standard output, which the test redirects to a file. On other hosts it is an empty
//! program, so the workspace builds everywhere.

#[cfg(target_os = "linux")]
mod program {
    use eggos_linux_gui_tests::{APP_ID, CLIENT_SIZE, Scene, TITLE};
    use eggos_wayland::protocol::{self, Interface};
    use eggos_wayland::wire::{self, Header, MAX_ARGUMENTS, MAX_MESSAGE_SIZE, Value};
    use std::ffi::{c_int, c_void};
    use std::fs::File;
    use std::io::Write;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::os::unix::net::UnixStream;
    use std::process::ExitCode;
    use std::time::{Duration, Instant};

    const PREFIX: &str = "LINUX-PAINT";
    const CONNECT_FOR: Duration = Duration::from_secs(2);
    const CONNECT_PAUSE: Duration = Duration::from_millis(50);
    const PROT_READ: c_int = 1;
    const PROT_WRITE: c_int = 2;
    const MAP_SHARED: c_int = 1;
    const MAP_PRIVATE: c_int = 2;
    const MFD_CLOEXEC: u32 = 1;
    const SOL_SOCKET: c_int = 1;
    const SCM_RIGHTS: c_int = 1;
    const CMSG_HEADER: usize = 16;
    const MSG_CMSG_CLOEXEC: c_int = 0x4000_0000;
    const BTN_LEFT: u32 = 0x110;
    const XRGB8888: u32 = 1;
    const STATE_MAXIMIZED: u32 = 1;

    const DISPLAY: u32 = 1;
    const REGISTRY: u32 = 2;
    const SYNC: u32 = 3;
    const COMPOSITOR: u32 = 4;
    const SHM: u32 = 5;
    const SEAT: u32 = 6;
    const OUTPUT: u32 = 7;
    const WM_BASE: u32 = 8;
    const POINTER: u32 = 9;
    const KEYBOARD: u32 = 10;
    const SURFACE: u32 = 11;
    const XDG_SURFACE: u32 = 12;
    const TOPLEVEL: u32 = 13;
    const FIRST_DYNAMIC: u32 = 20;

    #[repr(C)]
    struct IoVec {
        base: *mut c_void,
        length: usize,
    }

    #[repr(C)]
    struct MessageHeader {
        name: *mut c_void,
        name_length: u32,
        buffers: *mut IoVec,
        buffer_count: c_int,
        padding: c_int,
        control: *mut c_void,
        control_length: u32,
        control_padding: c_int,
        flags: c_int,
    }

    unsafe extern "C" {
        fn sendmsg(fd: c_int, message: *const MessageHeader, flags: c_int) -> isize;
        fn recvmsg(fd: c_int, message: *mut MessageHeader, flags: c_int) -> isize;
        fn memfd_create(name: *const std::ffi::c_char, flags: u32) -> c_int;
        fn mmap(
            address: *mut c_void,
            length: usize,
            protection: c_int,
            flags: c_int,
            fd: c_int,
            offset: i64,
        ) -> *mut c_void;
        fn munmap(address: *mut c_void, length: usize) -> c_int;
    }

    fn say(line: &str) {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{PREFIX} {line}");
        let _ = out.flush();
    }

    struct Mapping {
        address: *mut u32,
        length: usize,
    }

    impl Mapping {
        fn new(
            file: &File,
            length: usize,
            protection: c_int,
            flags: c_int,
        ) -> Result<Self, String> {
            // SAFETY: a new mapping chosen by the kernel, of `file`'s descriptor, which is open.
            let address = unsafe {
                mmap(
                    std::ptr::null_mut(),
                    length,
                    protection,
                    flags,
                    file.as_raw_fd(),
                    0,
                )
            };
            if address as isize == -1 {
                return Err(format!("mmap: {}", std::io::Error::last_os_error()));
            }
            Ok(Self {
                address: address.cast(),
                length,
            })
        }

        fn pixels(&mut self) -> &mut [u32] {
            // SAFETY: the mapping is `length` writable bytes of this process, kept until `self`
            // drops; the compositor reads it only between a commit and the buffer's release,
            // and the program paints only after the release, so nothing else uses it while the
            // slice lives.
            unsafe { std::slice::from_raw_parts_mut(self.address, self.length / 4) }
        }

        fn bytes(&self) -> &[u8] {
            // SAFETY: the mapping is `length` readable bytes, private to this process, kept
            // until `self` drops; nothing writes the file after the compositor sent it.
            unsafe { std::slice::from_raw_parts(self.address.cast::<u8>(), self.length) }
        }
    }

    impl Drop for Mapping {
        fn drop(&mut self) {
            // SAFETY: the mapping came from `mmap` with this length, and nothing uses it any
            // more.
            unsafe { munmap(self.address.cast(), self.length) };
        }
    }

    struct Frame {
        pool: u32,
        buffer: u32,
        size: (u32, u32),
        mapping: Mapping,
    }

    struct Client {
        stream: UnixStream,
        input: Vec<u8>,
        descriptors: Vec<OwnedFd>,
        next_id: u32,
        scene: Scene,
        frame: Option<Frame>,
        busy: bool,
        frame_pending: bool,
        dirty: bool,
        wanted: (u32, u32),
        maximized: bool,
        maximised_said: bool,
        configured: bool,
        shown: bool,
        pointer: (i32, i32),
        move_serial: Option<u32>,
        stale_sent: bool,
        closing: bool,
    }

    fn connect() -> Result<UnixStream, String> {
        let directory = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/run/user/0".into());
        let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
        let path = format!("{directory}/{display}");
        let started = Instant::now();
        loop {
            match UnixStream::connect(&path) {
                Ok(stream) => return Ok(stream),
                Err(error) if started.elapsed() >= CONNECT_FOR => {
                    return Err(format!("connect {path}: {error}"));
                }
                Err(_) => std::thread::sleep(CONNECT_PAUSE),
            }
        }
    }

    impl Client {
        fn send(&mut self, object: u32, opcode: u16, values: &[Value]) -> Result<(), String> {
            let mut out = [0u8; MAX_MESSAGE_SIZE];
            let size = wire::encode(&mut out, object, opcode, values)
                .map_err(|error| format!("encode: {error:?}"))?;
            self.stream
                .write_all(&out[..size])
                .map_err(|error| format!("write: {error}"))
        }

        fn send_with_fd(
            &mut self,
            object: u32,
            opcode: u16,
            values: &[Value],
            fd: RawFd,
        ) -> Result<(), String> {
            let mut out = [0u8; MAX_MESSAGE_SIZE];
            let size = wire::encode(&mut out, object, opcode, values)
                .map_err(|error| format!("encode: {error:?}"))?;
            let mut buffer = IoVec {
                base: out.as_mut_ptr().cast(),
                length: size,
            };
            let mut control = [0u8; 24];
            control[..8].copy_from_slice(&((CMSG_HEADER + 4) as u64).to_ne_bytes());
            control[8..12].copy_from_slice(&SOL_SOCKET.to_ne_bytes());
            control[12..16].copy_from_slice(&SCM_RIGHTS.to_ne_bytes());
            control[16..20].copy_from_slice(&fd.to_ne_bytes());
            let header = MessageHeader {
                name: std::ptr::null_mut(),
                name_length: 0,
                buffers: &mut buffer,
                buffer_count: 1,
                padding: 0,
                control: control.as_mut_ptr().cast(),
                control_length: (CMSG_HEADER + 4) as u32,
                control_padding: 0,
                flags: 0,
            };
            // SAFETY: `header` points to `buffer`, `out` and `control`, all live for the call,
            // with their lengths.
            let sent = unsafe { sendmsg(self.stream.as_raw_fd(), &header, 0) };
            match sent == size as isize {
                true => Ok(()),
                false => Err(format!("sendmsg: {}", std::io::Error::last_os_error())),
            }
        }

        fn receive(&mut self) -> Result<(), String> {
            let mut data = [0u8; 4096];
            let mut buffer = IoVec {
                base: data.as_mut_ptr().cast(),
                length: data.len(),
            };
            let mut control = [0u64; 16];
            let mut header = MessageHeader {
                name: std::ptr::null_mut(),
                name_length: 0,
                buffers: &mut buffer,
                buffer_count: 1,
                padding: 0,
                control: control.as_mut_ptr().cast(),
                control_length: (control.len() * 8) as u32,
                control_padding: 0,
                flags: 0,
            };
            // SAFETY: `header` points to `buffer`, `data` and `control`, all live and writable
            // for the call, with their lengths.
            let received =
                unsafe { recvmsg(self.stream.as_raw_fd(), &mut header, MSG_CMSG_CLOEXEC) };
            if received <= 0 {
                return Err(format!(
                    "recvmsg {received}: {}",
                    std::io::Error::last_os_error()
                ));
            }
            self.input.extend_from_slice(&data[..received as usize]);
            let control: Vec<u8> = control.iter().flat_map(|word| word.to_ne_bytes()).collect();
            let used = (header.control_length as usize).min(control.len());
            let mut at = 0;
            while at + CMSG_HEADER <= used {
                let length = u64::from_ne_bytes(control[at..at + 8].try_into().unwrap()) as usize;
                let level = c_int::from_ne_bytes(control[at + 8..at + 12].try_into().unwrap());
                let kind = c_int::from_ne_bytes(control[at + 12..at + 16].try_into().unwrap());
                if length < CMSG_HEADER || at + length > used {
                    break;
                }
                if level == SOL_SOCKET && kind == SCM_RIGHTS {
                    for chunk in control[at + CMSG_HEADER..at + length].as_chunks::<4>().0 {
                        let fd = RawFd::from_ne_bytes(*chunk);
                        // SAFETY: the kernel just installed `fd` for this message, and nothing
                        // else owns it.
                        self.descriptors.push(unsafe { OwnedFd::from_raw_fd(fd) });
                    }
                }
                at += length.div_ceil(8) * 8;
            }
            Ok(())
        }

        fn id(&mut self) -> u32 {
            self.next_id += 1;
            self.next_id
        }

        fn next_event(&mut self) -> Result<(u32, u16, Vec<u8>), String> {
            loop {
                if let Ok(Some(header)) = Header::read(&self.input)
                    && header.size <= self.input.len()
                {
                    let body = self.input[8..header.size].to_vec();
                    self.input.drain(..header.size);
                    return Ok((header.object, header.opcode, body));
                }
                self.receive()?;
            }
        }

        fn draw(&mut self) -> Result<(), String> {
            if self.busy || self.frame_pending || !self.configured {
                self.dirty = true;
                return Ok(());
            }
            self.dirty = false;
            let size = self.wanted;
            if self.frame.as_ref().is_none_or(|frame| frame.size != size) {
                self.new_frame(size)?;
            }
            self.scene.size = size;
            let scene = self.scene;
            let Some(frame) = self.frame.as_mut() else {
                return Ok(());
            };
            scene.paint(frame.mapping.pixels(), size.0);
            let buffer = frame.buffer;
            self.send(
                SURFACE,
                1,
                &[Value::Object(buffer), Value::Int(0), Value::Int(0)],
            )?;
            self.send(
                SURFACE,
                9,
                &[
                    Value::Int(0),
                    Value::Int(0),
                    Value::Int(size.0 as i32),
                    Value::Int(size.1 as i32),
                ],
            )?;
            let callback = self.id();
            self.send(SURFACE, 3, &[Value::NewId(callback)])?;
            self.send(SURFACE, 6, &[])?;
            self.busy = true;
            self.frame_pending = true;
            Ok(())
        }

        fn new_frame(&mut self, size: (u32, u32)) -> Result<(), String> {
            if let Some(old) = self.frame.take() {
                self.send(old.buffer, 0, &[])?;
                self.send(old.pool, 1, &[])?;
            }
            let length = size.0 as usize * size.1 as usize * 4;
            // SAFETY: `memfd_create` reads the name, a C string that lives for the program.
            let fd = unsafe { memfd_create(c"linux-paint".as_ptr(), MFD_CLOEXEC) };
            if fd < 0 {
                return Err(format!("memfd_create: {}", std::io::Error::last_os_error()));
            }
            // SAFETY: `memfd_create` just returned `fd`, which nothing else owns.
            let file = File::from(unsafe { OwnedFd::from_raw_fd(fd) });
            file.set_len(length as u64)
                .map_err(|error| format!("ftruncate: {error}"))?;
            let mapping = Mapping::new(&file, length, PROT_READ | PROT_WRITE, MAP_SHARED)?;
            let (pool, buffer) = (self.id(), self.id());
            self.send_with_fd(
                SHM,
                0,
                &[Value::NewId(pool), Value::Fd, Value::Int(length as i32)],
                file.as_raw_fd(),
            )?;
            self.send(
                pool,
                0,
                &[
                    Value::NewId(buffer),
                    Value::Int(0),
                    Value::Int(size.0 as i32),
                    Value::Int(size.1 as i32),
                    Value::Int(size.0 as i32 * 4),
                    Value::Uint(XRGB8888),
                ],
            )?;
            self.frame = Some(Frame {
                pool,
                buffer,
                size,
                mapping,
            });
            Ok(())
        }

        fn check_keymap(&mut self, body: &[u8]) -> Result<(), String> {
            let word = |index: usize| {
                u32::from_ne_bytes(body[index * 4..index * 4 + 4].try_into().unwrap())
            };
            let (format, size) = (word(0), word(1) as usize);
            if self.descriptors.is_empty() {
                return Err("keymap without a descriptor".to_owned());
            }
            let fd = self.descriptors.remove(0);
            let file = File::from(fd);
            let mapping = Mapping::new(&file, size, PROT_READ, MAP_PRIVATE)?;
            let text = mapping.bytes();
            if !text.starts_with(b"xkb_keymap {") || text.last() != Some(&0) {
                return Err("the keymap is not xkb text".to_owned());
            }
            say(&format!("keymap xkb_v{format} {size}"));
            Ok(())
        }

        fn event(&mut self, object: u32, opcode: u16, body: &[u8]) -> Result<(), String> {
            let interface = match object {
                DISPLAY => Interface::Display,
                REGISTRY => Interface::Registry,
                SHM => Interface::Shm,
                SEAT => Interface::Seat,
                OUTPUT => Interface::Output,
                WM_BASE => Interface::XdgWmBase,
                POINTER => Interface::Pointer,
                KEYBOARD => Interface::Keyboard,
                SURFACE => Interface::Surface,
                XDG_SURFACE => Interface::XdgSurface,
                TOPLEVEL => Interface::XdgToplevel,
                _ if body.is_empty() => Interface::Buffer,
                _ => Interface::Callback,
            };
            let Some(signature) = protocol::event_signature(interface, opcode) else {
                return Err(format!("unknown event {object}.{opcode}"));
            };
            let mut values = [Value::None; MAX_ARGUMENTS];
            let descriptors = wire::decode(body, signature, &mut values)
                .map_err(|error| format!("decode {object}.{opcode}: {error:?}"))?;
            if descriptors > 0 && self.descriptors.is_empty() {
                self.receive()?;
            }
            match (interface, opcode) {
                (Interface::Display, 0) => {
                    return Err(format!(
                        "protocol error on {} code {}: {}",
                        values[0].uint(),
                        values[1].uint(),
                        String::from_utf8_lossy(values[2].bytes())
                    ));
                }
                (Interface::Keyboard, 0) => self.check_keymap(body)?,
                (Interface::XdgWmBase, 0) => self.send(WM_BASE, 3, &[values[0]])?,
                (Interface::XdgToplevel, 0) => {
                    let (width, height) = (values[0].int(), values[1].int());
                    let states = values[2].bytes();
                    self.maximized = states
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .any(|state| u32::from_ne_bytes(*state) == STATE_MAXIMIZED);
                    if width > 0 && height > 0 {
                        self.wanted = (width as u32, height as u32);
                    }
                }
                (Interface::XdgToplevel, 1) => {
                    say("close");
                    self.closing = true;
                }
                (Interface::XdgSurface, 0) => {
                    self.send(XDG_SURFACE, 4, &[values[0]])?;
                    let first = !self.configured;
                    self.configured = true;
                    if first {
                        say("configured");
                    }
                    let resized = self
                        .frame
                        .as_ref()
                        .is_some_and(|frame| frame.size != self.wanted);
                    if first || resized {
                        self.draw()?;
                    }
                }
                (Interface::Buffer, 0) => {
                    self.busy = false;
                    self.after_frame()?;
                }
                (Interface::Callback, 0) => {
                    self.frame_pending = false;
                    if !self.shown {
                        self.shown = true;
                        say("shown");
                    }
                    self.after_frame()?;
                }
                (Interface::Pointer, 0) => {
                    self.pointer = (
                        wire::fixed_to_int(values[2].int()),
                        wire::fixed_to_int(values[3].int()),
                    );
                    if let Some(serial) = self.move_serial.filter(|_| !self.stale_sent) {
                        self.send(TOPLEVEL, 5, &[Value::Object(SEAT), Value::Uint(serial)])?;
                        self.stale_sent = true;
                        say("stale move sent");
                        self.scene.handle_moving = false;
                        self.draw()?;
                    }
                }
                (Interface::Pointer, 2) => {
                    self.pointer = (
                        wire::fixed_to_int(values[1].int()),
                        wire::fixed_to_int(values[2].int()),
                    );
                }
                (Interface::Pointer, 3) => {
                    let (serial, button, state) =
                        (values[0].uint(), values[2].uint(), values[3].uint());
                    let (x, y) = self.pointer;
                    if button == BTN_LEFT && state == 1 {
                        if Scene::on_target(x, y) && !self.scene.clicked {
                            self.scene.clicked = true;
                            say(&format!("clicked {x} {y}"));
                            self.draw()?;
                        } else if Scene::on_handle(x, y) && self.move_serial.is_none() {
                            self.send(TOPLEVEL, 5, &[Value::Object(SEAT), Value::Uint(serial)])?;
                            self.move_serial = Some(serial);
                            say("move sent");
                            self.scene.handle_moving = true;
                            self.draw()?;
                        }
                    }
                }
                (Interface::Keyboard, 3) => {
                    let (key, state) = (values[2].uint(), values[3].uint());
                    if state == 1 {
                        say(&format!("key {key} pressed"));
                        self.scene.typed += 1;
                        self.draw()?;
                    }
                }
                _ => {}
            }
            Ok(())
        }

        fn after_frame(&mut self) -> Result<(), String> {
            let size_changed = self
                .frame
                .as_ref()
                .is_some_and(|frame| frame.size != self.scene.size);
            if self.dirty || size_changed {
                self.draw()?;
            }
            let maximised =
                self.maximized && self.scene.size == self.wanted && self.wanted != CLIENT_SIZE;
            if !self.busy && !self.frame_pending && maximised && !self.maximised_said {
                say(&format!("maximised {}x{}", self.wanted.0, self.wanted.1));
                self.maximised_said = true;
            }
            Ok(())
        }

        fn globals(&mut self) -> Result<Vec<String>, String> {
            self.send(DISPLAY, 1, &[Value::NewId(REGISTRY)])?;
            self.send(DISPLAY, 0, &[Value::NewId(SYNC)])?;
            let mut globals = Vec::new();
            loop {
                let (object, opcode, body) = self.next_event()?;
                if object == SYNC {
                    return Ok(globals);
                }
                if object == REGISTRY && opcode == 0 {
                    let mut values = [Value::None; MAX_ARGUMENTS];
                    let signature =
                        protocol::event_signature(Interface::Registry, 0).unwrap_or(&[]);
                    wire::decode(&body, signature, &mut values)
                        .map_err(|error| format!("global: {error:?}"))?;
                    let name = values[0].uint();
                    let interface = String::from_utf8_lossy(values[1].bytes()).into_owned();
                    let version = values[2].uint();
                    let wanted = [
                        ("wl_compositor", COMPOSITOR, 4),
                        ("wl_shm", SHM, 1),
                        ("wl_seat", SEAT, 5),
                        ("wl_output", OUTPUT, 3),
                        ("xdg_wm_base", WM_BASE, 3),
                    ];
                    if let Some(&(_, id, ours)) =
                        wanted.iter().find(|(known, ..)| *known == interface)
                    {
                        self.send(
                            REGISTRY,
                            0,
                            &[
                                Value::Uint(name),
                                Value::UntypedNewId {
                                    interface: interface.as_bytes(),
                                    version: version.min(ours),
                                    id,
                                },
                            ],
                        )?;
                        globals.push(interface);
                    }
                } else {
                    self.event(object, opcode, &body)?;
                }
            }
        }

        fn run(&mut self) -> Result<(), String> {
            let globals = self.globals()?;
            say(&format!("globals {}", globals.join(" ")));
            self.send(SEAT, 0, &[Value::NewId(POINTER)])?;
            self.send(SEAT, 1, &[Value::NewId(KEYBOARD)])?;
            self.send(COMPOSITOR, 0, &[Value::NewId(SURFACE)])?;
            self.send(
                WM_BASE,
                2,
                &[Value::NewId(XDG_SURFACE), Value::Object(SURFACE)],
            )?;
            self.send(XDG_SURFACE, 1, &[Value::NewId(TOPLEVEL)])?;
            self.send(TOPLEVEL, 2, &[Value::String(TITLE.as_bytes())])?;
            self.send(TOPLEVEL, 3, &[Value::String(APP_ID.as_bytes())])?;
            self.send(SURFACE, 6, &[])?;
            while !self.closing {
                let (object, opcode, body) = self.next_event()?;
                self.event(object, opcode, &body)?;
            }
            self.send(TOPLEVEL, 0, &[])?;
            self.send(XDG_SURFACE, 0, &[])?;
            self.send(SURFACE, 0, &[])?;
            let sync = self.id();
            self.send(DISPLAY, 0, &[Value::NewId(sync)])?;
            loop {
                let (object, ..) = self.next_event()?;
                if object == sync {
                    return Ok(());
                }
            }
        }
    }

    pub fn main() -> ExitCode {
        let stream = match connect() {
            Ok(stream) => stream,
            Err(error) => {
                say(&format!("failed: {error}"));
                return ExitCode::FAILURE;
            }
        };
        let mut client = Client {
            stream,
            input: Vec::new(),
            descriptors: Vec::new(),
            next_id: FIRST_DYNAMIC,
            scene: Scene::new(),
            frame: None,
            busy: false,
            frame_pending: false,
            dirty: false,
            wanted: CLIENT_SIZE,
            maximized: false,
            maximised_said: false,
            configured: false,
            shown: false,
            pointer: (0, 0),
            move_serial: None,
            stale_sent: false,
            closing: false,
        };
        match client.run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                say(&format!("failed: {error}"));
                ExitCode::FAILURE
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    program::main()
}

#[cfg(not(target_os = "linux"))]
fn main() {}
