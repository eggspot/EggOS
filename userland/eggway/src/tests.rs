use crate::compositor::{self, Compositor, MAX_CLIENTS};
use crate::fake::{Fake, SentEvent, message};
use crate::system::{Fd, System};
use eggos_wayland::protocol::{
    Interface, buffer, callback, display, keyboard, pointer, registry, shm, surface, xdg_surface,
    xdg_toplevel,
};
use eggos_wayland::wire::Value;

const REGISTRY: u32 = 2;
const COMPOSITOR: u32 = 3;
const SHM: u32 = 4;
const SEAT: u32 = 5;
const OUTPUT: u32 = 6;
const WM_BASE: u32 = 7;
const POINTER: u32 = 8;
const KEYBOARD: u32 = 9;
const SURFACE: u32 = 10;
const XDG_SURFACE: u32 = 11;
const TOPLEVEL: u32 = 12;
const POOL: u32 = 13;
const BUFFER: u32 = 14;
const WIDTH: u32 = 200;
const HEIGHT: u32 = 100;

mod record {
    pub const POINTER_ENTER: u16 = 1;
    pub const POINTER_MOVE: u16 = 2;
    pub const POINTER_LEAVE: u16 = 3;
    pub const BUTTON: u16 = 4;
    pub const KEY: u16 = 6;
    pub const FOCUS_IN: u16 = 7;
    pub const CLOSE_REQUEST: u16 = 9;
    pub const RESIZE: u16 = 10;
    pub const STATE_CHANGED: u16 = 11;
}

struct Session {
    compositor: Compositor,
    fake: Fake,
    socket: Fd,
    read: usize,
    pending: Vec<u8>,
    events: Vec<SentEvent>,
}

impl Session {
    fn new() -> Self {
        let mut fake = Fake::default();
        let mut compositor = Compositor::new();
        let socket = fake.connect();
        assert!(compositor.add_client(&mut fake, socket));
        Self {
            compositor,
            fake,
            socket,
            read: 0,
            pending: Vec::new(),
            events: Vec::new(),
        }
    }

    fn send(&mut self, object: u32, opcode: u16, values: &[Value]) {
        self.pending.extend(message(object, opcode, values));
    }

    fn send_with(&mut self, object: u32, opcode: u16, values: &[Value], descriptors: Vec<Fd>) {
        self.flush_requests();
        self.fake
            .push(self.socket, message(object, opcode, values), descriptors);
    }

    fn flush_requests(&mut self) {
        if !self.pending.is_empty() {
            let bytes = core::mem::take(&mut self.pending);
            self.fake.push(self.socket, bytes, Vec::new());
        }
    }

    fn pump(&mut self) -> Vec<SentEvent> {
        self.flush_requests();
        for _ in 0..8 {
            if self.compositor.client_count() == 0 {
                break;
            }
            self.compositor.readable(&mut self.fake, 0);
            self.compositor.device_readable(&mut self.fake, 0);
            self.compositor.tick(&mut self.fake);
        }
        let events = self.fake.events(self.socket, &mut self.read);
        self.events.extend(events.iter().cloned());
        events
    }

    fn alive(&self) -> bool {
        self.compositor.client_count() == 1
    }

    fn error(&self) -> Option<(u32, u32)> {
        self.events
            .iter()
            .find(|event| event.object == 1 && event.opcode == display::ERROR.opcode)
            .map(|event| (event.word(0), event.word(1)))
    }

    fn globals(&mut self) {
        self.send(1, 1, &[Value::NewId(REGISTRY)]);
        for (name, interface, version, id) in [
            (1, "wl_compositor", 4, COMPOSITOR),
            (2, "wl_shm", 1, SHM),
            (3, "wl_seat", 5, SEAT),
            (4, "wl_output", 3, OUTPUT),
            (5, "xdg_wm_base", 3, WM_BASE),
        ] {
            self.bind(name, interface, version, id);
        }
        self.send(SEAT, 0, &[Value::NewId(POINTER)]);
        self.send(SEAT, 1, &[Value::NewId(KEYBOARD)]);
    }

    fn bind(&mut self, name: u32, interface: &str, version: u32, id: u32) {
        self.send(
            REGISTRY,
            0,
            &[
                Value::Uint(name),
                Value::UntypedNewId {
                    interface: interface.as_bytes(),
                    version,
                    id,
                },
            ],
        );
    }

    fn configured_toplevel(&mut self) -> u32 {
        self.send(COMPOSITOR, 0, &[Value::NewId(SURFACE)]);
        self.send(
            WM_BASE,
            2,
            &[Value::NewId(XDG_SURFACE), Value::Object(SURFACE)],
        );
        self.send(XDG_SURFACE, 1, &[Value::NewId(TOPLEVEL)]);
        self.send(TOPLEVEL, 2, &[Value::String(b"linux-paint")]);
        self.send(SURFACE, 6, &[]);
        let events = self.pump();
        let serial = events
            .iter()
            .find(|event| event.object == XDG_SURFACE)
            .map(|event| event.word(0))
            .expect("a configure");
        self.send(XDG_SURFACE, 4, &[Value::Uint(serial)]);
        serial
    }

    fn pool(&mut self, pixels: Vec<u32>) -> Fd {
        let bytes = pixels.len() as i32 * 4;
        let memory = self.fake.memory_file(pixels);
        self.send_with(
            SHM,
            0,
            &[Value::NewId(POOL), Value::Fd, Value::Int(bytes)],
            vec![memory],
        );
        memory
    }

    fn buffer(&mut self, id: u32, width: u32, height: u32) {
        self.send(
            POOL,
            0,
            &[
                Value::NewId(id),
                Value::Int(0),
                Value::Int(width as i32),
                Value::Int(height as i32),
                Value::Int(width as i32 * 4),
                Value::Uint(shm::XRGB8888),
            ],
        );
    }

    fn show(&mut self, id: u32, damage: (i32, i32, i32, i32)) {
        self.send(
            SURFACE,
            1,
            &[Value::Object(id), Value::Int(0), Value::Int(0)],
        );
        self.send(
            SURFACE,
            9,
            &[
                Value::Int(damage.0),
                Value::Int(damage.1),
                Value::Int(damage.2),
                Value::Int(damage.3),
            ],
        );
        self.send(SURFACE, 6, &[]);
    }

    fn shown(&mut self) -> u32 {
        self.globals();
        self.configured_toplevel();
        let pixels = (0..WIDTH * HEIGHT)
            .map(|pixel| 0xff00_0000 | pixel)
            .collect();
        self.pool(pixels);
        self.buffer(BUFFER, WIDTH, HEIGHT);
        self.show(BUFFER, (0, 0, WIDTH as i32, HEIGHT as i32));
        self.pump();
        self.fake.window().expect("a window").0
    }

    fn of(&self, object: u32, opcode: u16) -> Vec<&SentEvent> {
        self.events
            .iter()
            .filter(|event| event.object == object && event.opcode == opcode)
            .collect()
    }

    fn record(&mut self, kind: u16, values: [u32; 4]) -> Vec<SentEvent> {
        let window = self.fake.window().expect("a window").0;
        self.fake.record(window, kind, values);
        self.pump()
    }

    fn end(mut self) {
        self.fake.hang_up(self.socket);
        self.pump();
        assert_eq!(self.compositor.client_count(), 0);
        assert!(self.fake.open.is_empty(), "left open: {:?}", self.fake.open);
        assert!(
            self.fake.windows.values().all(|window| window.destroyed),
            "a window's buffer left mapped"
        );
        assert_eq!(self.compositor.pool_bytes(), 0);
    }
}

#[test]
fn the_registry_announces_every_global_and_refuses_a_newer_bind() {
    let mut session = Session::new();
    session.send(1, 1, &[Value::NewId(REGISTRY)]);
    let events = session.pump();
    let globals: Vec<_> = events
        .iter()
        .filter(|event| event.object == REGISTRY && event.opcode == registry::GLOBAL.opcode)
        .collect();
    assert_eq!(globals.len(), 5);
    assert_eq!(globals[0].word(0), 1);
    session.bind(1, "wl_compositor", 5, COMPOSITOR);
    session.pump();
    assert_eq!(session.error(), Some((REGISTRY, display::INVALID_OBJECT)));
    assert!(!session.alive());
    assert!(session.fake.open.is_empty());
}

#[test]
fn binding_announces_formats_capabilities_and_the_output() {
    let mut session = Session::new();
    session.globals();
    session.pump();
    let formats: Vec<_> = session
        .of(SHM, shm::FORMAT.opcode)
        .iter()
        .map(|event| event.word(0))
        .collect();
    assert_eq!(formats, [shm::ARGB8888, shm::XRGB8888]);
    assert_eq!(session.of(SEAT, 0)[0].word(0), 3);
    let mode = session.of(OUTPUT, 1)[0];
    assert_eq!((mode.word(1), mode.word(2)), (1280, 732));
    let keymap = session.of(KEYBOARD, keyboard::KEYMAP.opcode)[0];
    assert_eq!(keymap.word(0), keyboard::XKB_V1);
    assert_eq!(
        keymap.word(1) as usize,
        eggos_wayland::keymap::US_KEYMAP.len() + 1
    );
    assert_eq!(
        session.fake.sockets[&session.socket].outgoing_descriptors,
        1
    );
    assert_eq!(session.fake.keymaps, 1);
    session.end();
}

#[test]
fn a_toplevel_s_buffer_becomes_a_window_with_its_pixels_opaque() {
    let mut session = Session::new();
    let id = session.shown();
    let window = &session.fake.windows[&id];
    assert_eq!((window.width, window.height), (WIDTH, HEIGHT));
    assert_eq!(window.title, b"linux-paint");
    assert_eq!(window.pixels[5], 5);
    assert_eq!(
        window.pixels[(WIDTH * HEIGHT - 1) as usize],
        WIDTH * HEIGHT - 1
    );
    assert_eq!(window.commits, vec![Vec::new()]);
    assert_eq!(session.of(BUFFER, buffer::RELEASE.opcode).len(), 1);
    assert_eq!(session.of(SURFACE, surface::ENTER.opcode).len(), 1);
    session.end();
}

#[test]
fn a_buffer_before_the_first_configure_is_an_error() {
    let mut session = Session::new();
    session.globals();
    session.send(COMPOSITOR, 0, &[Value::NewId(SURFACE)]);
    session.send(
        WM_BASE,
        2,
        &[Value::NewId(XDG_SURFACE), Value::Object(SURFACE)],
    );
    session.send(XDG_SURFACE, 1, &[Value::NewId(TOPLEVEL)]);
    session.pool(vec![0; 64 * 64]);
    session.buffer(BUFFER, 64, 64);
    session.show(BUFFER, (0, 0, 64, 64));
    session.pump();
    assert_eq!(
        session.error(),
        Some((XDG_SURFACE, xdg_surface::UNCONFIGURED_BUFFER))
    );
    assert!(!session.alive());
    assert!(session.fake.open.is_empty());
}

#[test]
fn damage_copies_only_its_rectangle() {
    let mut session = Session::new();
    let id = session.shown();
    let memory = *session.fake.memory.keys().next().unwrap();
    for pixel in session.fake.memory.get_mut(&memory).unwrap().iter_mut() {
        *pixel = 0x00ab_cdef;
    }
    session.show(BUFFER, (10, 20, 4, 2));
    session.pump();
    let window = &session.fake.windows[&id];
    assert_eq!(window.pixels[(20 * WIDTH + 10) as usize], 0x00ab_cdef);
    assert_eq!(window.pixels[(20 * WIDTH + 14) as usize], 20 * WIDTH + 14);
    assert_eq!(
        window.commits.last().unwrap(),
        &vec![eggos_wayland::blit::Rect::new(10, 20, 4, 2)]
    );
    session.end();
}

#[test]
fn a_resize_configures_and_the_new_buffer_resizes_the_window() {
    let mut session = Session::new();
    let id = session.shown();
    session.record(record::RESIZE, [300, 150, 0, 0]);
    let configure = session.of(TOPLEVEL, xdg_toplevel::CONFIGURE.opcode);
    let last = configure.last().unwrap();
    assert_eq!((last.word(0), last.word(1)), (300, 150));
    let serial = session.of(XDG_SURFACE, 0).last().unwrap().word(0);
    session.send(XDG_SURFACE, 4, &[Value::Uint(serial)]);
    session.send(POOL, 2, &[Value::Int(300 * 150 * 4)]);
    session.buffer(BUFFER + 1, 300, 150);
    session.show(BUFFER + 1, (0, 0, 1, 1));
    session.pump();
    let window = &session.fake.windows[&id];
    assert_eq!((window.width, window.height), (300, 150));
    assert_eq!(window.commits.last().unwrap(), &Vec::new());
    assert!(session.alive());
    session.end();
}

#[test]
fn maximise_minimise_and_states_never_say_resizing() {
    let mut session = Session::new();
    let id = session.shown();
    session.send(TOPLEVEL, 9, &[]);
    session.send(TOPLEVEL, 13, &[]);
    session.send(TOPLEVEL, 10, &[]);
    session.send(TOPLEVEL, 11, &[Value::Object(0)]);
    session.pump();
    assert_eq!(session.fake.windows[&id].states, [2, 1, 0, 2]);
    session.record(record::RESIZE, [1280, 732, 0, 0]);
    session.record(record::STATE_CHANGED, [2, 0, 0, 0]);
    let last = *session
        .of(TOPLEVEL, xdg_toplevel::CONFIGURE.opcode)
        .last()
        .unwrap();
    assert_eq!((last.word(0), last.word(1)), (1280, 732));
    assert_eq!(last.word(2), 4);
    assert_eq!(last.word(3), xdg_toplevel::STATE_MAXIMIZED);
    for configure in session.of(TOPLEVEL, xdg_toplevel::CONFIGURE.opcode) {
        let count = configure.word(2) as usize / 4;
        assert!((0..count).all(|index| configure.word(3 + index) != xdg_toplevel::STATE_RESIZING));
    }
    session.end();
}

#[test]
fn focus_keys_and_close_reach_the_client() {
    let mut session = Session::new();
    session.shown();
    session.record(record::FOCUS_IN, [0; 4]);
    assert_eq!(session.of(KEYBOARD, keyboard::ENTER.opcode).len(), 1);
    let configure = *session
        .of(TOPLEVEL, xdg_toplevel::CONFIGURE.opcode)
        .last()
        .unwrap();
    assert_eq!(configure.word(3), xdg_toplevel::STATE_ACTIVATED);
    session.record(record::KEY, [30, 1, 1, 0]);
    let key = *session.of(KEYBOARD, keyboard::KEY.opcode).last().unwrap();
    assert_eq!((key.word(2), key.word(3)), (30, keyboard::PRESSED));
    let modifiers = *session
        .of(KEYBOARD, keyboard::MODIFIERS.opcode)
        .last()
        .unwrap();
    assert_eq!(modifiers.word(1), eggos_wayland::keymap::SHIFT);
    session.record(record::CLOSE_REQUEST, [0; 4]);
    assert_eq!(session.of(TOPLEVEL, xdg_toplevel::CLOSE.opcode).len(), 1);
    session.end();
}

#[test]
fn a_move_needs_the_held_press_s_serial() {
    let mut session = Session::new();
    let id = session.shown();
    session.record(record::POINTER_ENTER, [20, 20, 0, 0]);
    session.record(record::POINTER_MOVE, [25, 22, 1, 0]);
    session.record(record::BUTTON, [25, 22, 0x110, 1]);
    assert_eq!(session.of(POINTER, pointer::MOTION.opcode).len(), 1);
    let press = *session.of(POINTER, pointer::BUTTON.opcode).last().unwrap();
    let serial = press.word(0);
    session.send(
        TOPLEVEL,
        5,
        &[Value::Object(SEAT), Value::Uint(serial + 99)],
    );
    session.pump();
    assert!(session.fake.windows[&id].drags.is_empty());
    session.send(TOPLEVEL, 5, &[Value::Object(SEAT), Value::Uint(serial)]);
    session.pump();
    assert_eq!(session.fake.windows[&id].drags, [(0, 0)]);
    session.record(record::POINTER_LEAVE, [0; 4]);
    assert_eq!(session.of(POINTER, pointer::LEAVE.opcode).len(), 1);
    session.record(record::POINTER_LEAVE, [0; 4]);
    assert_eq!(session.of(POINTER, pointer::LEAVE.opcode).len(), 1);
    session.record(record::POINTER_ENTER, [5, 5, 0, 0]);
    session.send(TOPLEVEL, 5, &[Value::Object(SEAT), Value::Uint(serial)]);
    session.pump();
    assert_eq!(session.fake.windows[&id].drags.len(), 1);
    session.record(record::BUTTON, [5, 5, 0x111, 1]);
    let right = session
        .of(POINTER, pointer::BUTTON.opcode)
        .last()
        .unwrap()
        .word(0);
    session.send(TOPLEVEL, 5, &[Value::Object(SEAT), Value::Uint(right)]);
    session.pump();
    assert_eq!(session.fake.windows[&id].drags.len(), 1);
    session.end();
}

#[test]
fn a_resize_maps_xdg_edges_and_refuses_unknown_ones() {
    let mut session = Session::new();
    let id = session.shown();
    session.record(record::BUTTON, [5, 5, 0x110, 1]);
    let serial = session
        .of(POINTER, pointer::BUTTON.opcode)
        .last()
        .unwrap()
        .word(0);
    session.send(
        TOPLEVEL,
        6,
        &[Value::Object(SEAT), Value::Uint(serial), Value::Uint(10)],
    );
    session.pump();
    assert_eq!(session.fake.windows[&id].drags, [(1, 2 | 8)]);
    for edge in [3, 7, 11] {
        let mut session = Session::new();
        session.shown();
        session.send(
            TOPLEVEL,
            6,
            &[Value::Object(SEAT), Value::Uint(1), Value::Uint(edge)],
        );
        session.pump();
        assert_eq!(
            session.error(),
            Some((TOPLEVEL, xdg_toplevel::INVALID_RESIZE_EDGE)),
            "{edge}"
        );
        assert!(session.fake.open.is_empty());
    }
    session.end();
}

#[test]
fn frame_callbacks_come_at_most_every_16_ms_and_wait_while_minimised() {
    let mut session = Session::new();
    session.shown();
    assert_eq!(session.compositor.timeout_ms(session.fake.now), None);
    session.send(SURFACE, 3, &[Value::NewId(20)]);
    session.send(SURFACE, 6, &[]);
    let events = session.pump();
    assert!(
        events
            .iter()
            .any(|event| event.object == 20 && event.opcode == callback::DONE.opcode)
    );
    session.send(SURFACE, 3, &[Value::NewId(21)]);
    session.send(SURFACE, 6, &[]);
    session.fake.now += 5;
    assert!(session.pump().iter().all(|event| event.object != 21));
    assert_eq!(session.compositor.timeout_ms(session.fake.now), Some(11));
    session.fake.now += 11;
    assert!(session.pump().iter().any(|event| event.object == 21));
    session.record(record::STATE_CHANGED, [1, 0, 0, 0]);
    session.send(SURFACE, 3, &[Value::NewId(22)]);
    session.send(SURFACE, 6, &[]);
    session.fake.now += 100;
    assert!(session.pump().iter().all(|event| event.object != 22));
    assert_eq!(session.compositor.timeout_ms(session.fake.now), None);
    session.record(record::STATE_CHANGED, [0, 0, 0, 0]);
    assert!(session.events.iter().any(|event| event.object == 22));
    for id in 30..39 {
        session.send(SURFACE, 3, &[Value::NewId(id)]);
    }
    let events = session.pump();
    assert!(events.iter().any(|event| event.object == 30));
    assert!(session.alive());
    session.end();
}

#[test]
fn descriptors_queue_across_reads_and_a_truncated_batch_disconnects() {
    let mut session = Session::new();
    session.globals();
    let memory = session.fake.memory_file(vec![0; 64]);
    let request = message(SHM, 0, &[Value::NewId(POOL), Value::Fd, Value::Int(256)]);
    session.flush_requests();
    session
        .fake
        .push(session.socket, request[..8].to_vec(), vec![memory]);
    session
        .fake
        .push(session.socket, request[8..].to_vec(), Vec::new());
    session.pump();
    assert!(session.alive());
    assert_eq!(session.compositor.pool_bytes(), 256);
    let many: Vec<Fd> = (0..28).map(|_| session.fake.other_file()).collect();
    session
        .fake
        .push(session.socket, message(1, 0, &[Value::NewId(40)]), many);
    session.pump();
    assert!(session.alive());
    let more: Vec<Fd> = (0..40).map(|_| session.fake.other_file()).collect();
    session
        .fake
        .push(session.socket, message(1, 0, &[Value::NewId(41)]), more);
    session.pump();
    assert!(!session.alive());
    assert!(session.fake.open.is_empty(), "{:?}", session.fake.open);
    assert_eq!(session.compositor.pool_bytes(), 0);
}

#[test]
fn a_keymap_s_descriptor_waits_for_a_send_that_takes_bytes() {
    let mut session = Session::new();
    session
        .fake
        .sockets
        .get_mut(&session.socket)
        .unwrap()
        .send_limit = Some(0);
    session.globals();
    session.pump();
    assert!(session.alive());
    assert_eq!(session.fake.open.len(), 2);
    assert_eq!(
        session.fake.sockets[&session.socket].outgoing_descriptors,
        0
    );
    session
        .fake
        .sockets
        .get_mut(&session.socket)
        .unwrap()
        .send_limit = Some(10);
    session.compositor.writable(&mut session.fake, 0);
    assert_eq!(
        session.fake.sockets[&session.socket].outgoing_descriptors,
        1
    );
    assert_eq!(session.fake.open.len(), 1);
    session
        .fake
        .sockets
        .get_mut(&session.socket)
        .unwrap()
        .send_limit = None;
    for id in 20..26 {
        session.send(SEAT, 1, &[Value::NewId(id)]);
    }
    session.pump();
    session.compositor.writable(&mut session.fake, 0);
    assert!(session.alive());
    assert_eq!(session.fake.keymaps, 7);
    assert_eq!(
        session.fake.sockets[&session.socket].outgoing_descriptors,
        7
    );
    session.end();
}

#[test]
fn a_seventeenth_client_is_closed_at_once() {
    let mut fake = Fake::default();
    let mut compositor = Compositor::new();
    for _ in 0..MAX_CLIENTS {
        let socket = fake.connect();
        assert!(compositor.add_client(&mut fake, socket));
    }
    let socket = fake.connect();
    assert!(!compositor.add_client(&mut fake, socket));
    assert!(!fake.open.contains(&socket));
    assert_eq!(compositor.client_count(), MAX_CLIENTS);
    for index in 0..MAX_CLIENTS {
        compositor.disconnect(&mut fake, index);
    }
    assert!(fake.open.is_empty());
}

#[test]
fn a_client_that_reads_nothing_stops_being_read() {
    let mut session = Session::new();
    session
        .fake
        .sockets
        .get_mut(&session.socket)
        .unwrap()
        .send_limit = Some(0);
    session.send(1, 1, &[Value::NewId(REGISTRY)]);
    for id in 0..800 {
        session.send(1, 0, &[Value::NewId(100 + id)]);
    }
    session.pump();
    assert!(session.alive());
    let watch = session.compositor.watch(0).unwrap();
    assert!(!watch.wants_input && watch.wants_output);
    session
        .fake
        .sockets
        .get_mut(&session.socket)
        .unwrap()
        .send_limit = None;
    for _ in 0..20 {
        session.compositor.writable(&mut session.fake, 0);
        session.compositor.readable(&mut session.fake, 0);
    }
    session.pump();
    let done = session
        .events
        .iter()
        .filter(|event| event.opcode == callback::DONE.opcode && event.object >= 100)
        .count();
    assert_eq!(done, 800);
    session.end();
}

#[test]
fn protocol_errors_disconnect_and_close_everything() {
    type Case = Box<dyn Fn(&mut Session)>;
    let cases: Vec<(&str, Case, (u32, u32))> = vec![
        (
            "unknown object",
            Box::new(|session| session.send(77, 0, &[])),
            (77, display::INVALID_OBJECT),
        ),
        (
            "unknown request",
            Box::new(|session| session.send(1, 9, &[])),
            (1, display::INVALID_METHOD),
        ),
        (
            "bad format",
            Box::new(|session| {
                session.pool(vec![0; 64]);
                session.send(
                    POOL,
                    0,
                    &[
                        Value::NewId(BUFFER),
                        Value::Int(0),
                        Value::Int(4),
                        Value::Int(4),
                        Value::Int(16),
                        Value::Uint(7),
                    ],
                );
            }),
            (POOL, shm::INVALID_FORMAT),
        ),
        (
            "bad stride",
            Box::new(|session| {
                session.pool(vec![0; 64]);
                session.send(
                    POOL,
                    0,
                    &[
                        Value::NewId(BUFFER),
                        Value::Int(0),
                        Value::Int(4),
                        Value::Int(4),
                        Value::Int(8),
                        Value::Uint(0),
                    ],
                );
            }),
            (POOL, shm::INVALID_STRIDE),
        ),
        (
            "buffer past the pool",
            Box::new(|session| {
                session.pool(vec![0; 64]);
                session.buffer(BUFFER, 8, 9);
            }),
            (POOL, shm::INVALID_STRIDE),
        ),
        (
            "empty pool",
            Box::new(|session| {
                let memory = session.fake.memory_file(Vec::new());
                session.send_with(
                    SHM,
                    0,
                    &[Value::NewId(POOL), Value::Fd, Value::Int(0)],
                    vec![memory],
                );
            }),
            (SHM, shm::INVALID_STRIDE),
        ),
        (
            "pool of no memory file",
            Box::new(|session| {
                let pipe = session.fake.other_file();
                session.send_with(
                    SHM,
                    0,
                    &[Value::NewId(POOL), Value::Fd, Value::Int(64)],
                    vec![pipe],
                );
            }),
            (SHM, shm::INVALID_FD),
        ),
        (
            "shrinking pool",
            Box::new(|session| {
                session.pool(vec![0; 64]);
                session.send(POOL, 2, &[Value::Int(128)]);
            }),
            (POOL, shm::INVALID_STRIDE),
        ),
        (
            "missing descriptor",
            Box::new(|session| {
                session.send(SHM, 0, &[Value::NewId(POOL), Value::Fd, Value::Int(64)]);
            }),
            (SHM, display::INVALID_METHOD),
        ),
        (
            "transform",
            Box::new(|session| {
                session.send(COMPOSITOR, 0, &[Value::NewId(SURFACE)]);
                session.send(SURFACE, 7, &[Value::Int(8)]);
            }),
            (SURFACE, surface::INVALID_TRANSFORM),
        ),
        (
            "scale",
            Box::new(|session| {
                session.send(COMPOSITOR, 0, &[Value::NewId(SURFACE)]);
                session.send(SURFACE, 8, &[Value::Int(0)]);
            }),
            (SURFACE, surface::INVALID_SCALE),
        ),
        (
            "maximum size",
            Box::new(|session| {
                session.configured_toplevel();
                session.send(TOPLEVEL, 7, &[Value::Int(-1), Value::Int(5)]);
            }),
            (TOPLEVEL, xdg_toplevel::INVALID_SIZE),
        ),
        (
            "serial",
            Box::new(|session| {
                let serial = session.configured_toplevel();
                session.send(XDG_SURFACE, 4, &[Value::Uint(serial + 50)]);
            }),
            (XDG_SURFACE, xdg_surface::INVALID_SERIAL),
        ),
        (
            "id in use",
            Box::new(|session| session.send(COMPOSITOR, 0, &[Value::NewId(SHM)])),
            (SHM, display::INVALID_OBJECT),
        ),
        (
            "id past the table",
            Box::new(|session| session.send(COMPOSITOR, 0, &[Value::NewId(5000)])),
            (5000, display::NO_MEMORY),
        ),
    ];
    for (name, case, expected) in cases {
        let mut session = Session::new();
        session.globals();
        case(&mut session);
        session.pump();
        assert_eq!(session.error(), Some(expected), "{name}");
        assert!(!session.alive(), "{name}");
        assert!(
            session.fake.open.is_empty(),
            "{name}: {:?}",
            session.fake.open
        );
        assert_eq!(session.compositor.pool_bytes(), 0, "{name}");
    }
}

#[test]
fn valid_transforms_and_scales_are_kept_and_destroyed_ids_are_freed() {
    let mut session = Session::new();
    session.shown();
    session.send(SURFACE, 7, &[Value::Int(7)]);
    session.send(SURFACE, 8, &[Value::Int(2)]);
    session.send(BUFFER, 0, &[]);
    session.pump();
    assert!(session.alive());
    let deleted = session
        .of(1, display::DELETE_ID.opcode)
        .iter()
        .any(|event| event.word(0) == BUFFER);
    assert!(deleted);
    session.buffer(BUFFER, 4, 4);
    session.pump();
    assert!(session.alive());
    session.end();
}

#[test]
fn pools_are_capped_per_client_and_for_everyone() {
    let mut session = Session::new();
    session.globals();
    session.fake.map_error = Some(eggos_runtime::errno::Errno::ENOMEM);
    session.pool(vec![0; 16]);
    session.pump();
    assert_eq!(session.error(), Some((POOL, display::NO_MEMORY)));
    assert!(session.fake.open.is_empty());
    let mut fake = Fake::default();
    let mut compositor = Compositor::new();
    let pools = |fake: &mut Fake, socket: Fd, sizes: &[i32]| {
        fake.push(socket, message(1, 1, &[Value::NewId(REGISTRY)]), Vec::new());
        let bind = message(
            REGISTRY,
            0,
            &[
                Value::Uint(2),
                Value::UntypedNewId {
                    interface: b"wl_shm",
                    version: 1,
                    id: SHM,
                },
            ],
        );
        fake.push(socket, bind, Vec::new());
        for (pool, &size) in sizes.iter().enumerate() {
            let memory = fake.memory_file(Vec::new());
            let request = message(
                SHM,
                0,
                &[
                    Value::NewId(POOL + pool as u32),
                    Value::Fd,
                    Value::Int(size),
                ],
            );
            fake.push(socket, request, vec![memory]);
        }
    };
    let thirty = 30 << 20;
    for (index, sizes) in [[thirty, 0], [thirty, 0]].iter().enumerate() {
        let socket = fake.connect();
        compositor.add_client(&mut fake, socket);
        pools(&mut fake, socket, &sizes[..1]);
        for _ in 0..4 {
            compositor.readable(&mut fake, index);
            compositor.tick(&mut fake);
        }
    }
    assert_eq!(
        compositor.client_count(),
        1,
        "the second 30 MiB pass the 48 MiB of all"
    );
    assert_eq!(compositor.pool_bytes(), thirty as usize);
    compositor.disconnect(&mut fake, 0);
    assert_eq!(compositor.pool_bytes(), 0);
    let socket = fake.connect();
    compositor.add_client(&mut fake, socket);
    pools(&mut fake, socket, &[20 << 20, 20 << 20]);
    for _ in 0..5 {
        compositor.readable(&mut fake, 0);
        compositor.tick(&mut fake);
    }
    assert_eq!(
        compositor.client_count(),
        0,
        "40 MiB pass one client's 32 MiB"
    );
    assert_eq!(compositor.pool_bytes(), 0);
    assert!(fake.open.is_empty());
}

#[test]
fn a_pool_grows_by_mapping_anew_and_a_shrunken_file_reads_as_zeros() {
    let mut session = Session::new();
    let id = session.shown();
    let memory = *session.fake.memory.keys().next().unwrap();
    session.send(POOL, 2, &[Value::Int((WIDTH * HEIGHT * 4 * 2) as i32)]);
    session.pump();
    assert!(session.alive());
    assert_eq!(
        session.compositor.pool_bytes(),
        (WIDTH * HEIGHT * 4 * 2) as usize
    );
    session.fake.memory.get_mut(&memory).unwrap().truncate(10);
    session.show(BUFFER, (0, 0, WIDTH as i32, HEIGHT as i32));
    session.pump();
    let window = &session.fake.windows[&id];
    assert_eq!(window.pixels[5], 5);
    assert_eq!(window.pixels[50], 0);
    session.end();
}

#[test]
fn an_output_bound_after_the_window_shows_is_entered() {
    let mut session = Session::new();
    session.shown();
    let before = session.of(SURFACE, surface::ENTER.opcode).len();
    session.bind(4, "wl_output", 3, 40);
    session.pump();
    let entered = session.of(SURFACE, surface::ENTER.opcode);
    assert_eq!(entered.len(), before + 1);
    assert_eq!(entered.last().unwrap().word(0), 40);
    session.end();
}

#[test]
fn objects_inherit_their_parent_s_version() {
    let mut session = Session::new();
    session.send(1, 1, &[Value::NewId(REGISTRY)]);
    session.bind(3, "wl_seat", 4, SEAT);
    session.send(SEAT, 0, &[Value::NewId(POINTER)]);
    session.send(SEAT, 3, &[]);
    session.pump();
    assert_eq!(session.error(), Some((SEAT, display::INVALID_METHOD)));
    let mut session = Session::new();
    session.shown();
    session.send(SEAT, 0, &[Value::NewId(30)]);
    session.pump();
    session.record(record::POINTER_ENTER, [1, 1, 0, 0]);
    assert!(
        session
            .events
            .iter()
            .any(|event| event.object == 30 && event.opcode == pointer::FRAME.opcode)
    );
    session.end();
}

#[test]
fn the_fuzz_target_finds_nothing_in_a_short_run() {
    let windows = crate::fuzz::run(0x5eed, 300).expect("no fault");
    assert!(windows > 0, "the fuzzed sessions never made a window");
}

#[test]
fn interface_names_on_the_wire_match_the_specification() {
    assert_eq!(Interface::XdgWmBase.name(), "xdg_wm_base");
    assert_eq!(compositor::FRAME_INTERVAL_MS, 16);
    let mut fake = Fake::default();
    assert_eq!(fake.work_area(), (1280, 732));
}

#[test]
fn a_keyboard_made_while_a_window_has_focus_is_entered_and_windows_go_with_the_client() {
    let mut session = Session::new();
    session.shown();
    session.record(record::FOCUS_IN, [0; 4]);
    session.send(SEAT, 1, &[Value::NewId(40)]);
    session.pump();
    assert_eq!(session.of(40, keyboard::ENTER.opcode).len(), 1);
    assert_eq!(session.of(40, keyboard::MODIFIERS.opcode).len(), 1);
    session.end();
}

#[test]
fn a_pool_grows_up_to_the_client_s_cap_and_no_further() {
    let mut session = Session::new();
    session.globals();
    let twenty = 20 << 20;
    let memory = session.fake.memory_file(Vec::new());
    session.send_with(
        SHM,
        0,
        &[Value::NewId(POOL), Value::Fd, Value::Int(twenty)],
        vec![memory],
    );
    session.send(POOL, 2, &[Value::Int(24 << 20)]);
    session.pump();
    assert!(session.alive(), "{:?}", session.error());
    assert_eq!(session.compositor.pool_bytes(), 24 << 20);
    session.send(POOL, 2, &[Value::Int(32 << 20)]);
    session.pump();
    assert!(session.alive(), "{:?}", session.error());
    assert_eq!(session.compositor.pool_bytes(), 32 << 20);
    session.send(POOL, 2, &[Value::Int((32 << 20) + 4096)]);
    session.pump();
    assert_eq!(session.error(), Some((POOL, display::NO_MEMORY)));
    assert!(!session.alive());
    assert!(session.fake.open.is_empty());
}

#[test]
fn a_pointer_made_over_a_window_is_entered_and_a_gone_surface_s_callbacks_end() {
    let mut session = Session::new();
    session.shown();
    session.record(record::POINTER_ENTER, [30, 40, 0, 0]);
    session.record(record::POINTER_MOVE, [31, 42, 0, 0]);
    session.send(SEAT, 0, &[Value::NewId(41)]);
    session.pump();
    let enter = session.of(41, pointer::ENTER.opcode);
    assert_eq!(enter.len(), 1);
    assert_eq!((enter[0].word(2) >> 8, enter[0].word(3) >> 8), (31, 42));
    session.send(SURFACE, 3, &[Value::NewId(50)]);
    session.send(SURFACE, 0, &[]);
    session.pump();
    assert_eq!(session.of(50, callback::DONE.opcode).len(), 1);
    let deleted = session
        .of(1, display::DELETE_ID.opcode)
        .iter()
        .any(|event| event.word(0) == 50);
    assert!(deleted);
    session.end();
}
