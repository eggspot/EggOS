//! A machine in memory for the host tests and the fuzz target: sockets whose chunks carry
//! descriptors with their first byte as `AF_UNIX` sockets do (D138), memory files of pixels,
//! windows that record what the compositor asks of them, and a clock the test sets. It counts
//! every descriptor, so a test can check that nothing leaks.

use crate::client::ClientSlot;
use crate::system::{Fd, PoolPixels, Received, System, WindowBuffer};
use eggos_runtime::errno::Errno;
use eggos_wayland::blit::{OPAQUE_MASK, Placement, Rect, WindowPixels};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Bytes a client sent in one `sendmsg`, with its descriptors.
#[derive(Debug, Clone, Default)]
pub struct Chunk {
    pub bytes: Vec<u8>,
    pub descriptors: Vec<Fd>,
    taken: usize,
}

impl Chunk {
    /// `bytes` sent with `descriptors`.
    pub fn new(bytes: Vec<u8>, descriptors: Vec<Fd>) -> Self {
        Self {
            bytes,
            descriptors,
            taken: 0,
        }
    }
}

/// One end of a connection, as the compositor holds it.
#[derive(Debug, Default)]
pub struct Socket {
    /// What the client sent and the compositor has not read.
    pub incoming: VecDeque<Chunk>,
    /// What the compositor sent.
    pub outgoing: Vec<u8>,
    /// How many descriptors came with what the compositor sent.
    pub outgoing_descriptors: usize,
    /// The most bytes the next sends take; `Some(0)` makes them `EAGAIN`.
    pub send_limit: Option<usize>,
    /// The client closed its end.
    pub hung_up: bool,
}

/// A window the compositor made.
#[derive(Debug, Clone, Default)]
pub struct FakeWindow {
    pub device: Fd,
    pub width: u32,
    pub height: u32,
    pub flags: u32,
    pub pixels: Vec<u32>,
    pub title: Vec<u8>,
    pub minimum: (u32, u32),
    pub states: Vec<u32>,
    pub commits: Vec<Vec<Rect>>,
    pub drags: Vec<(u32, u32)>,
    pub pointer_shapes: Vec<u32>,
    pub destroyed: bool,
}

struct FakePixels<'a> {
    pixels: &'a mut [u32],
    width: u32,
}

impl WindowPixels for FakePixels<'_> {
    fn get(&mut self, x: u32, y: u32) -> u32 {
        self.pixels[(y * self.width + x) as usize]
    }

    fn set(&mut self, x: u32, y: u32, pixel: u32) {
        self.pixels[(y * self.width + x) as usize] = pixel;
    }
}

/// The machine.
#[derive(Debug)]
pub struct Fake {
    pub now: u64,
    next_descriptor: Fd,
    /// Every descriptor open in the compositor's process.
    pub open: BTreeSet<Fd>,
    pub sockets: BTreeMap<Fd, Socket>,
    /// The memory files' contents, in pixels.
    pub memory: BTreeMap<Fd, Vec<u32>>,
    mappings: BTreeMap<usize, (Fd, usize)>,
    next_address: usize,
    pub windows: BTreeMap<u32, FakeWindow>,
    next_window: u32,
    /// The records each window server connection has for the compositor.
    pub records: BTreeMap<Fd, VecDeque<[u8; 32]>>,
    /// What the next pool mappings fail with.
    pub map_error: Option<Errno>,
    /// What the window server answers a move or resize with.
    pub drag_answer: Result<(), Errno>,
    /// The size the window server clamps new windows to, at least.
    pub minimum_window: (u32, u32),
    pub work_area: (u32, u32),
    pub clients: usize,
    pub keymaps: usize,
    /// The queries answered: serial, outcome and tree.
    pub answers: Vec<(u32, u32, Vec<u8>)>,
    /// The windows raised and focused, in order (D176).
    pub focused: Vec<u32>,
}

std::thread_local! {
    static SPARE_CLIENTS: std::cell::RefCell<Vec<&'static mut ClientSlot>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

impl Default for Fake {
    fn default() -> Self {
        Self {
            now: 1000,
            next_descriptor: 10,
            open: BTreeSet::new(),
            sockets: BTreeMap::new(),
            memory: BTreeMap::new(),
            mappings: BTreeMap::new(),
            next_address: 0x1000_0000,
            windows: BTreeMap::new(),
            next_window: 64,
            records: BTreeMap::new(),
            map_error: None,
            drag_answer: Ok(()),
            minimum_window: (158, 62),
            work_area: (1280, 732),
            clients: 0,
            keymaps: 0,
            answers: Vec::new(),
            focused: Vec::new(),
        }
    }
}

impl Fake {
    fn descriptor(&mut self) -> Fd {
        let descriptor = self.next_descriptor;
        self.next_descriptor += 1;
        self.open.insert(descriptor);
        descriptor
    }

    /// A new connection: the compositor's end.
    pub fn connect(&mut self) -> Fd {
        let socket = self.descriptor();
        self.sockets.insert(socket, Socket::default());
        socket
    }

    /// A memory file of `pixels`, as a client makes it; its descriptor, which the client passes
    /// (the compositor's copy).
    pub fn memory_file(&mut self, pixels: Vec<u32>) -> Fd {
        let descriptor = self.descriptor();
        self.memory.insert(descriptor, pixels);
        descriptor
    }

    /// A descriptor of no memory file (a pipe, say).
    pub fn other_file(&mut self) -> Fd {
        self.descriptor()
    }

    /// Queues an event record for the window server connection that holds `window`.
    pub fn record(&mut self, window: u32, kind: u16, values: [u32; 4]) {
        let Some(device) = self.windows.get(&window).map(|window| window.device) else {
            return;
        };
        let mut record = [0u8; 32];
        record[0..2].copy_from_slice(&kind.to_ne_bytes());
        record[2..4].copy_from_slice(&(window as u16).to_ne_bytes());
        record[4..8].copy_from_slice(&(self.now as u32).to_ne_bytes());
        for (index, value) in values.into_iter().enumerate() {
            record[8 + index * 4..12 + index * 4].copy_from_slice(&value.to_ne_bytes());
        }
        self.records.entry(device).or_default().push_back(record);
    }

    /// The live window, if there is one.
    pub fn window(&self) -> Option<(u32, &FakeWindow)> {
        self.windows
            .iter()
            .find(|(_, window)| !window.destroyed)
            .map(|(id, window)| (*id, window))
    }
}

impl System for Fake {
    fn now_ms(&mut self) -> u64 {
        self.now
    }

    fn new_client(&mut self, socket: Fd) -> Option<&'static mut ClientSlot> {
        self.clients += 1;
        match SPARE_CLIENTS.with_borrow_mut(Vec::pop) {
            Some(slot) => {
                *slot = ClientSlot::new(socket);
                Some(slot)
            }
            None => Some(Box::leak(Box::new(ClientSlot::new(socket)))),
        }
    }

    fn free_client(&mut self, client: &'static mut ClientSlot) {
        self.clients -= 1;
        SPARE_CLIENTS.with_borrow_mut(|spare| spare.push(client));
    }

    fn receive(
        &mut self,
        socket: Fd,
        bytes: &mut [u8],
        descriptors: &mut [Fd],
    ) -> Result<Received, Errno> {
        let Some(end) = self.sockets.get_mut(&socket) else {
            return Err(Errno(9));
        };
        let Some(chunk) = end.incoming.front_mut() else {
            return match end.hung_up {
                true => Ok(Received::default()),
                false => Err(Errno::EAGAIN),
            };
        };
        let mut received = Received::default();
        let mut closed = Vec::new();
        if chunk.taken == 0 {
            for (index, &descriptor) in chunk.descriptors.iter().enumerate() {
                match descriptors.get_mut(index) {
                    Some(slot) => {
                        *slot = descriptor;
                        received.descriptors += 1;
                    }
                    None => {
                        received.truncated = true;
                        closed.push(descriptor);
                    }
                }
            }
        }
        let count = bytes.len().min(chunk.bytes.len() - chunk.taken);
        bytes[..count].copy_from_slice(&chunk.bytes[chunk.taken..chunk.taken + count]);
        chunk.taken += count;
        received.bytes = count;
        if chunk.taken == chunk.bytes.len() {
            end.incoming.pop_front();
        }
        for descriptor in closed {
            self.open.remove(&descriptor);
        }
        Ok(received)
    }

    fn send(&mut self, socket: Fd, bytes: &[u8], descriptors: &[Fd]) -> Result<usize, Errno> {
        let Some(end) = self.sockets.get_mut(&socket) else {
            return Err(Errno(9));
        };
        if end.hung_up {
            return Err(Errno::EPIPE);
        }
        let count = bytes.len().min(end.send_limit.unwrap_or(usize::MAX));
        if count == 0 {
            return Err(Errno::EAGAIN);
        }
        end.outgoing.extend_from_slice(&bytes[..count]);
        end.outgoing_descriptors += descriptors.len();
        Ok(count)
    }

    fn close(&mut self, descriptor: Fd) {
        assert!(self.open.remove(&descriptor), "closed {descriptor} twice");
        if let Some(end) = self.sockets.get_mut(&descriptor) {
            let in_flight: Vec<Fd> = end
                .incoming
                .drain(..)
                .filter(|chunk| chunk.taken == 0)
                .flat_map(|chunk| chunk.descriptors)
                .collect();
            for descriptor in in_flight {
                self.open.remove(&descriptor);
            }
        }
    }

    fn map_pool(&mut self, descriptor: Fd, length: usize) -> Result<usize, Errno> {
        if let Some(error) = self.map_error {
            return Err(error);
        }
        if !self.memory.contains_key(&descriptor) {
            return Err(Errno::ENODEV);
        }
        let address = self.next_address;
        self.next_address += length.div_ceil(4096) * 4096 + 4096;
        self.mappings.insert(address, (descriptor, length));
        Ok(address)
    }

    fn unmap_pool(&mut self, address: usize, length: usize) {
        let mapping = self.mappings.remove(&address);
        assert_eq!(
            mapping.map(|(_, mapped)| mapped),
            Some(length),
            "unmapped {address:#x}"
        );
    }

    fn copy_rows(&mut self, from: PoolPixels, window: WindowBuffer, rect: Rect) {
        let Some(&(descriptor, length)) = self.mappings.get(&from.address) else {
            panic!("copy from {:#x}, which is not mapped", from.address);
        };
        assert_eq!(length, from.length);
        let source = self.memory.get(&descriptor).cloned().unwrap_or_default();
        let target = self.windows.get_mut(&window.id).expect("a window");
        assert_eq!((target.width, target.height), (window.width, window.height));
        let rect = rect.clipped(window.width, window.height);
        for row in rect.y..rect.bottom() as u32 {
            for column in rect.x..rect.right() as u32 {
                let at = from.offset + row as usize * from.stride + column as usize * 4;
                assert!(at + 4 <= from.length, "copy past the pool");
                let pixel = source.get(at / 4).copied().unwrap_or(0);
                target.pixels[(row * window.width + column) as usize] = pixel & OPAQUE_MASK;
            }
        }
    }

    fn place(
        &mut self,
        from: PoolPixels,
        size: (u32, u32),
        window: WindowBuffer,
        placement: Placement,
    ) {
        let Some(&(descriptor, length)) = self.mappings.get(&from.address) else {
            panic!("draw from {:#x}, which is not mapped", from.address);
        };
        assert_eq!(length, from.length);
        let source = self.memory.get(&descriptor).cloned().unwrap_or_default();
        let target = self.windows.get_mut(&window.id).expect("a window");
        assert_eq!((target.width, target.height), (window.width, window.height));
        let placement = Placement {
            clip: placement.clip.clipped(window.width, window.height),
            ..placement
        };
        let read = |column: u32, row: u32| -> Option<u32> {
            if column >= size.0 || row >= size.1 {
                return None;
            }
            let at = from.offset + row as usize * from.stride + column as usize * 4;
            assert!(at + 4 <= from.length, "draw past the pool");
            source.get(at / 4).copied()
        };
        let mut pixels = FakePixels {
            pixels: &mut target.pixels,
            width: window.width,
        };
        placement.draw(read, &mut pixels);
    }

    fn keymap(&mut self) -> Result<(Fd, u32), Errno> {
        self.keymaps += 1;
        let size = eggos_wayland::keymap::US_KEYMAP.len() as u32 + 1;
        Ok((self.descriptor(), size))
    }

    fn open_window_device(&mut self) -> Result<Fd, Errno> {
        Ok(self.descriptor())
    }

    fn create_window(
        &mut self,
        device: Fd,
        width: u32,
        height: u32,
        flags: u32,
    ) -> Result<WindowBuffer, Errno> {
        let id = self.next_window;
        self.next_window += 1;
        let (width, height) = (
            width.max(self.minimum_window.0),
            height.max(self.minimum_window.1),
        );
        self.windows.insert(
            id,
            FakeWindow {
                device,
                width,
                height,
                flags,
                pixels: vec![0; (width * height) as usize],
                ..FakeWindow::default()
            },
        );
        Ok(WindowBuffer {
            id,
            width,
            height,
            address: id as usize,
        })
    }

    fn resize_window(
        &mut self,
        _device: Fd,
        window: &mut WindowBuffer,
        width: u32,
        height: u32,
    ) -> Result<(), Errno> {
        let target = self.windows.get_mut(&window.id).ok_or(Errno::EINVAL)?;
        let (width, height) = (
            width.max(self.minimum_window.0),
            height.max(self.minimum_window.1),
        );
        target.width = width;
        target.height = height;
        target.pixels = vec![0; (width * height) as usize];
        window.width = width;
        window.height = height;
        Ok(())
    }

    fn destroy_window(&mut self, _device: Fd, window: WindowBuffer) {
        if let Some(target) = self.windows.get_mut(&window.id) {
            target.destroyed = true;
        }
    }

    fn set_title(&mut self, _device: Fd, window: u32, title: &[u8]) -> Result<(), Errno> {
        let target = self.windows.get_mut(&window).ok_or(Errno::EINVAL)?;
        target.title = title.to_vec();
        Ok(())
    }

    fn answer_query(
        &mut self,
        _device: Fd,
        serial: u32,
        outcome: u32,
        tree: &[u8],
    ) -> Result<(), Errno> {
        self.answers.push((serial, outcome, tree.to_vec()));
        Ok(())
    }

    fn set_minimum_size(
        &mut self,
        _device: Fd,
        window: u32,
        width: u32,
        height: u32,
    ) -> Result<(), Errno> {
        let target = self.windows.get_mut(&window).ok_or(Errno::EINVAL)?;
        target.minimum = (width, height);
        Ok(())
    }

    fn set_pointer(&mut self, _device: Fd, window: u32, shape: u32) -> Result<(), Errno> {
        let target = self.windows.get_mut(&window).ok_or(Errno::EINVAL)?;
        target.pointer_shapes.push(shape);
        Ok(())
    }

    fn set_state(&mut self, _device: Fd, window: u32, state: u32) -> Result<(), Errno> {
        let target = self.windows.get_mut(&window).ok_or(Errno::EINVAL)?;
        target.states.push(state);
        Ok(())
    }

    fn focus_window(&mut self, _device: Fd, window: u32) -> Result<(), Errno> {
        if !self.windows.contains_key(&window) {
            return Err(Errno::EINVAL);
        }
        self.focused.push(window);
        Ok(())
    }

    fn start_move_or_resize(
        &mut self,
        _device: Fd,
        window: u32,
        kind: u32,
        edges: u32,
    ) -> Result<(), Errno> {
        let target = self.windows.get_mut(&window).ok_or(Errno::EINVAL)?;
        target.drags.push((kind, edges));
        self.drag_answer
    }

    fn commit(&mut self, _device: Fd, window: u32, rects: &[Rect]) -> Result<(), Errno> {
        let target = self.windows.get_mut(&window).ok_or(Errno::EINVAL)?;
        target.commits.push(rects.to_vec());
        Ok(())
    }

    fn work_area(&mut self) -> (u32, u32) {
        self.work_area
    }

    fn read_events(&mut self, device: Fd, records: &mut [u8]) -> Result<usize, Errno> {
        let queue = self.records.entry(device).or_default();
        let mut length = 0;
        while length + 32 <= records.len() {
            let Some(record) = queue.pop_front() else {
                break;
            };
            records[length..length + 32].copy_from_slice(&record);
            length += 32;
        }
        Ok(length)
    }
}

/// One message a client sends: `object`'s request `opcode` with `values`.
pub fn message(object: u32, opcode: u16, values: &[eggos_wayland::wire::Value]) -> Vec<u8> {
    let mut out = [0u8; eggos_wayland::wire::MAX_MESSAGE_SIZE];
    let size = eggos_wayland::wire::encode(&mut out, object, opcode, values).expect("it fits");
    out[..size].to_vec()
}

/// One event the compositor sent: its object, opcode and the bytes after its header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentEvent {
    pub object: u32,
    pub opcode: u16,
    pub body: Vec<u8>,
}

impl SentEvent {
    /// The body's 32-bit word at `index`.
    pub fn word(&self, index: usize) -> u32 {
        let at = index * 4;
        u32::from_ne_bytes(self.body[at..at + 4].try_into().expect("a word"))
    }
}

impl Fake {
    /// Queues what a client sent on its connection `socket`.
    pub fn push(&mut self, socket: Fd, bytes: Vec<u8>, descriptors: Vec<Fd>) {
        match self.sockets.get_mut(&socket) {
            Some(end) if self.open.contains(&socket) => {
                end.incoming.push_back(Chunk::new(bytes, descriptors));
            }
            _ => {
                for descriptor in descriptors {
                    self.open.remove(&descriptor);
                }
            }
        }
    }

    /// The client closed its end of `socket`.
    pub fn hang_up(&mut self, socket: Fd) {
        if let Some(end) = self.sockets.get_mut(&socket) {
            end.hung_up = true;
        }
    }

    /// The events the compositor sent on `socket` from byte `from` on, moving `from` past them.
    pub fn events(&self, socket: Fd, from: &mut usize) -> Vec<SentEvent> {
        let Some(end) = self.sockets.get(&socket) else {
            return Vec::new();
        };
        let mut events = Vec::new();
        while let Ok(Some(header)) = eggos_wayland::wire::Header::read(&end.outgoing[*from..]) {
            if *from + header.size > end.outgoing.len() {
                break;
            }
            events.push(SentEvent {
                object: header.object,
                opcode: header.opcode,
                body: end.outgoing[*from + 8..*from + header.size].to_vec(),
            });
            *from += header.size;
        }
        events
    }
}
