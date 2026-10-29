//! The compositor (D140): every client's requests checked against `wayland.xml` and
//! `xdg-shell.xml` and carried out, each `xdg_toplevel` a window on `/dev/window`, the window
//! server's input turned into seat and shell events, and frame callbacks paced at 60 Hz. A
//! client that breaks the protocol gets `wl_display.error` and is disconnected; nothing a
//! client sends can make the compositor panic or hold more than its fixed limits.

use crate::client::{
    Attach, Buffer, ClientSlot, INPUT_CAPACITY, MAX_BUFFERS, MAX_FRAMES, MAX_IN_DESCRIPTORS,
    MAX_OBJECTS, MAX_OUT_DESCRIPTORS, MAX_POOLS, MAX_SURFACES, OUTPUT_CAPACITY, Pool, Surface,
    TITLE_CAPACITY,
};
use crate::system::{Fd, PoolPixels, Received, System, WindowBuffer};
use eggos_element_tree::{
    Bounds, Element, HEADER_BYTES, Outcome, Query, Role, TreeWriter, query_kind, tree_answer,
};
use eggos_runtime::errno::Errno;
use eggos_wayland::blit::Rect;
use eggos_wayland::keymap;
use eggos_wayland::protocol::{
    self, Interface, SERVER_ID_START, buffer, callback, display, keyboard, output, pointer,
    registry, seat, shm, surface, xdg_popup, xdg_surface, xdg_toplevel, xdg_wm_base,
};
use eggos_wayland::wire::{self, Header, MAX_ARGUMENTS, MAX_MESSAGE_SIZE, Value};

/// Wayland clients served at once.
pub const MAX_CLIENTS: usize = 16;
/// Requests handled per client each time it is readable, so one client cannot starve others.
pub const MAX_REQUESTS_PER_WAKE: usize = 64;
/// The least time between two frame callbacks of a surface: 60 Hz.
pub const FRAME_INTERVAL_MS: u64 = 16;
/// The largest pool.
pub const MAX_POOL_BYTES: usize = 32 << 20;
/// The most pool memory one client maps.
pub const MAX_CLIENT_POOL_BYTES: usize = 32 << 20;
/// The most pool memory all clients map together.
pub const MAX_TOTAL_POOL_BYTES: usize = 48 << 20;
/// The largest buffer: `/dev/window`'s largest window buffer.
pub const MAX_BUFFER_BYTES: usize = 4 << 20;
/// The most descriptors one read takes: libwayland's most per message batch.
pub const MAX_RECEIVE_DESCRIPTORS: usize = 28;
const ANSWER_ROOM: usize = 4096;
const ROOT_ELEMENT: u32 = 1;

const FIXED_SIZE: u32 = 1;
/// `/dev/window`'s event record size.
pub const RECORD_SIZE: usize = 32;
const BTN_LEFT: u32 = 0x110;
const WHEEL_STEP: i32 = 10;
const REFRESH_MILLIHERTZ: i32 = 60_000;
const REPEAT_RATE: i32 = 25;
const REPEAT_DELAY_MS: i32 = 600;

mod record {
    pub const POINTER_ENTER: u16 = 1;
    pub const POINTER_MOVE: u16 = 2;
    pub const POINTER_LEAVE: u16 = 3;
    pub const BUTTON: u16 = 4;
    pub const WHEEL: u16 = 5;
    pub const KEY: u16 = 6;
    pub const FOCUS_IN: u16 = 7;
    pub const FOCUS_OUT: u16 = 8;
    pub const CLOSE_REQUEST: u16 = 9;
    pub const RESIZE: u16 = 10;
    pub const STATE_CHANGED: u16 = 11;
}

mod window_state {
    pub const NORMAL: u32 = 0;
    pub const MINIMISED: u32 = 1;
    pub const MAXIMISED: u32 = 2;
}

mod move_or_resize {
    pub const MOVE: u32 = 0;
    pub const RESIZE: u32 = 1;
    pub const LEFT: u32 = 1;
    pub const RIGHT: u32 = 2;
    pub const TOP: u32 = 4;
    pub const BOTTOM: u32 = 8;
}

/// What one client waits for, for `ppoll`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Watch {
    pub socket: Fd,
    /// It may send more requests: watch for input.
    pub wants_input: bool,
    /// Events wait to go: watch for room.
    pub wants_output: bool,
    /// Its window server connection, for input events.
    pub device: Option<Fd>,
}

#[derive(Debug, Default)]
struct Shared {
    serial: u32,
    pool_bytes: usize,
}

impl Shared {
    fn next_serial(&mut self) -> u32 {
        self.serial = self.serial.wrapping_add(1).max(1);
        self.serial
    }
}

/// The compositor: its clients and what they share.
#[derive(Debug, Default)]
pub struct Compositor {
    clients: [Option<&'static mut ClientSlot>; MAX_CLIENTS],
    shared: Shared,
}

type Values<'a> = [Value<'a>; MAX_ARGUMENTS];

struct Message<'a> {
    object: u32,
    interface: Interface,
    version: u32,
    index: u16,
    opcode: u16,
    values: Values<'a>,
    descriptors: [Fd; 2],
}

impl Message<'_> {
    fn uint(&self, at: usize) -> u32 {
        self.values[at].uint()
    }

    fn int(&self, at: usize) -> i32 {
        self.values[at].int()
    }
}

fn clamp_to(value: i32) -> u32 {
    value.max(0) as u32
}

fn rect_of(message: &Message, at: usize) -> Rect {
    Rect::from_signed(
        message.int(at),
        message.int(at + 1),
        message.int(at + 2),
        message.int(at + 3),
    )
}

impl Compositor {
    /// A compositor with no clients.
    pub fn new() -> Self {
        Self::default()
    }

    /// How many clients are connected.
    pub fn client_count(&self) -> usize {
        self.clients.iter().flatten().count()
    }

    /// The pool memory every client maps together.
    pub fn pool_bytes(&self) -> usize {
        self.shared.pool_bytes
    }

    /// Takes the new connection on `socket`; `false` when it was closed at once (too many
    /// clients or no memory).
    pub fn add_client<S: System>(&mut self, system: &mut S, socket: Fd) -> bool {
        let Some(free) = self.clients.iter().position(Option::is_none) else {
            system.close(socket);
            return false;
        };
        let Some(client) = system.new_client(socket) else {
            system.close(socket);
            return false;
        };
        self.clients[free] = Some(client);
        true
    }

    /// What client `index` waits for, if it exists.
    pub fn watch(&self, index: usize) -> Option<Watch> {
        let client = self.clients.get(index)?.as_deref()?;
        Some(Watch {
            socket: client.socket,
            wants_input: !client.failed && !client.blocked,
            wants_output: client.output_length > 0 || client.descriptors_out_count > 0,
            device: client.device,
        })
    }

    /// Client `index`'s socket is readable or hung up: takes its requests.
    pub fn readable<S: System>(&mut self, system: &mut S, index: usize) {
        let Some(client) = self
            .clients
            .get_mut(index)
            .and_then(|slot| slot.as_deref_mut())
        else {
            return;
        };
        let alive = receive(client, &mut self.shared, system);
        if !alive {
            self.disconnect(system, index);
        }
    }

    /// Client `index`'s socket has room: sends what waits.
    pub fn writable<S: System>(&mut self, system: &mut S, index: usize) {
        if let Some(client) = self
            .clients
            .get_mut(index)
            .and_then(|slot| slot.as_deref_mut())
        {
            client.blocked = false;
        }
        self.flush_client(system, index);
        if let Some(client) = self
            .clients
            .get_mut(index)
            .and_then(|slot| slot.as_deref_mut())
            && !client.blocked
        {
            handle_requests(client, &mut self.shared, system);
        }
    }

    /// Whether client `index` holds whole requests it has not handled yet (the per-wake limit
    /// stopped it): handle them without waiting for its socket.
    pub fn has_backlog(&self, index: usize) -> bool {
        let Some(client) = self.clients.get(index).and_then(|slot| slot.as_deref()) else {
            return false;
        };
        !client.failed
            && !client.blocked
            && matches!(
                Header::read(&client.input[..client.input_length]),
                Ok(Some(header)) if header.size <= client.input_length
            )
    }

    /// Client `index`'s window server connection has events.
    pub fn device_readable<S: System>(&mut self, system: &mut S, index: usize) {
        let Some(client) = self
            .clients
            .get_mut(index)
            .and_then(|slot| slot.as_deref_mut())
        else {
            return;
        };
        let Some(device) = client.device else {
            return;
        };
        let mut records = [0u8; RECORD_SIZE * 32];
        for _ in 0..8 {
            let Ok(length) = system.read_events(device, &mut records) else {
                break;
            };
            for record in records[..length].as_chunks::<RECORD_SIZE>().0 {
                if Query::from_record(record).is_some() {
                    answer_query(client, system, device, record);
                    continue;
                }
                window_event(client, &mut self.shared, record);
            }
            if length < records.len() {
                break;
            }
        }
        flush_motion(client, &mut self.shared);
    }

    /// Sends the frame callbacks that are due at `now`, then every client's events; clients
    /// that failed or hung up go.
    pub fn tick<S: System>(&mut self, system: &mut S) {
        let now = system.now_ms();
        for client in self.clients.iter_mut().flatten() {
            for index in 0..MAX_SURFACES {
                let Some(surface) = client.surfaces[index] else {
                    continue;
                };
                if surface.frame_count > 0
                    && !surface.minimised
                    && now >= surface.last_done_ms.saturating_add(FRAME_INTERVAL_MS)
                {
                    fire_frames(client, index as u16, now);
                }
            }
        }
        for index in 0..MAX_CLIENTS {
            self.flush_client(system, index);
        }
    }

    /// How long until a frame callback is due, from `now`; `None` when none waits.
    pub fn timeout_ms(&self, now: u64) -> Option<u64> {
        self.clients
            .iter()
            .flatten()
            .flat_map(|client| client.surfaces.iter().flatten())
            .filter(|surface| surface.frame_count > 0 && !surface.minimised)
            .map(|surface| {
                surface
                    .last_done_ms
                    .saturating_add(FRAME_INTERVAL_MS)
                    .saturating_sub(now)
            })
            .min()
    }

    fn flush_client<S: System>(&mut self, system: &mut S, index: usize) {
        let Some(client) = self
            .clients
            .get_mut(index)
            .and_then(|slot| slot.as_deref_mut())
        else {
            return;
        };
        let sent = flush(client, system);
        if !sent || client.failed {
            self.disconnect(system, index);
        }
    }

    /// Ends client `index`: its socket, window server connection, pools and descriptors are
    /// closed and its memory given back.
    pub fn disconnect<S: System>(&mut self, system: &mut S, index: usize) {
        let Some(client) = self.clients.get_mut(index).and_then(Option::take) else {
            return;
        };
        system.close(client.socket);
        if let Some(device) = client.device {
            for surface in client.surfaces.iter().flatten() {
                if let Some(window) = surface.window {
                    system.destroy_window(device, window);
                }
            }
            system.close(device);
        }
        for pool in client.pools.iter().flatten() {
            system.unmap_pool(pool.address, pool.length);
            system.close(pool.descriptor);
        }
        self.shared.pool_bytes = self.shared.pool_bytes.saturating_sub(client.pool_bytes);
        while let Some(descriptor) = client.take_descriptor() {
            system.close(descriptor);
        }
        for &descriptor in &client.descriptors_out[..client.descriptors_out_count] {
            system.close(descriptor);
        }
        system.free_client(client);
    }
}

fn flush<S: System>(client: &mut ClientSlot, system: &mut S) -> bool {
    if client.output_length == 0 {
        return true;
    }
    let count = client.descriptors_out_count;
    match system.send(
        client.socket,
        &client.output[..client.output_length],
        &client.descriptors_out[..count],
    ) {
        Ok(sent) => {
            if sent > 0 {
                for &descriptor in &client.descriptors_out[..count] {
                    system.close(descriptor);
                }
                client.descriptors_out_count = 0;
            }
            let sent = sent.min(client.output_length);
            client.output.copy_within(sent..client.output_length, 0);
            client.output_length -= sent;
            client.blocked = client.output_length > 0;
            true
        }
        Err(Errno::EAGAIN) => {
            client.blocked = true;
            true
        }
        Err(_) => false,
    }
}

fn receive<S: System>(client: &mut ClientSlot, shared: &mut Shared, system: &mut S) -> bool {
    if client.failed {
        return true;
    }
    handle_requests(client, shared, system);
    if client.failed || client.blocked || client.input_length == INPUT_CAPACITY {
        return true;
    }
    let room = (MAX_IN_DESCRIPTORS - client.descriptors_in_count).min(MAX_RECEIVE_DESCRIPTORS);
    let mut descriptors = [-1; MAX_RECEIVE_DESCRIPTORS];
    let length = client.input_length;
    let received = system.receive(
        client.socket,
        &mut client.input[length..INPUT_CAPACITY],
        &mut descriptors[..room],
    );
    match received {
        Ok(Received {
            bytes: 0,
            descriptors: 0,
            ..
        }) => false,
        Ok(received) => {
            for &descriptor in &descriptors[..received.descriptors.min(room)] {
                client.push_descriptor(descriptor);
            }
            client.input_length += received.bytes.min(INPUT_CAPACITY - length);
            if received.truncated {
                return false;
            }
            handle_requests(client, shared, system);
            true
        }
        Err(Errno::EAGAIN) | Err(Errno::EINTR) => true,
        Err(_) => false,
    }
}

fn handle_requests<S: System>(client: &mut ClientSlot, shared: &mut Shared, system: &mut S) {
    for _ in 0..MAX_REQUESTS_PER_WAKE {
        if client.descriptors_out_count == MAX_OUT_DESCRIPTORS && !flush(client, system) {
            client.failed = true;
        }
        let full = client.output_room() < ANSWER_ROOM
            || client.descriptors_out_count == MAX_OUT_DESCRIPTORS;
        if client.failed || full {
            if !client.failed {
                client.blocked = true;
            }
            return;
        }
        let header = match Header::read(&client.input[..client.input_length]) {
            Ok(Some(header)) => header,
            Ok(None) => return,
            Err(_) => {
                client.protocol_error(1, display::INVALID_METHOD, b"bad message size");
                return;
            }
        };
        if header.size > client.input_length {
            return;
        }
        let mut message = [0u8; MAX_MESSAGE_SIZE];
        message[..header.size].copy_from_slice(&client.input[..header.size]);
        client
            .input
            .copy_within(header.size..client.input_length, 0);
        client.input_length -= header.size;
        dispatch(client, shared, system, header, &message[8..header.size]);
    }
}

fn dispatch<S: System>(
    client: &mut ClientSlot,
    shared: &mut Shared,
    system: &mut S,
    header: Header,
    body: &[u8],
) {
    let Some(object) = client.object(header.object) else {
        client.protocol_error(header.object, display::INVALID_OBJECT, b"no such object");
        return;
    };
    let Some(interface) = object.interface else {
        return;
    };
    let Some(request) = protocol::request_of(interface, object.version, header.opcode) else {
        client.protocol_error(header.object, display::INVALID_METHOD, b"no such request");
        return;
    };
    let mut values = [Value::None; MAX_ARGUMENTS];
    let Ok(wanted) = wire::decode(body, request.signature, &mut values) else {
        client.protocol_error(header.object, display::INVALID_METHOD, b"malformed request");
        return;
    };
    let mut descriptors = [-1; 2];
    for slot in descriptors.iter_mut().take(wanted) {
        match client.take_descriptor() {
            Some(descriptor) => *slot = descriptor,
            None => {
                for &descriptor in descriptors.iter().filter(|&&descriptor| descriptor >= 0) {
                    system.close(descriptor);
                }
                client.protocol_error(header.object, display::INVALID_METHOD, b"missing fd");
                return;
            }
        }
    }
    let message = Message {
        object: header.object,
        interface,
        version: object.version,
        index: object.index,
        opcode: header.opcode,
        values,
        descriptors,
    };
    if object.inert {
        for &descriptor in descriptors.iter().filter(|&&descriptor| descriptor >= 0) {
            system.close(descriptor);
        }
    } else {
        handle(client, shared, system, &message);
    }
    if request.destructor && !client.failed {
        destroy_object(client, header.object);
    }
}

fn destroy_object(client: &mut ClientSlot, id: u32) {
    client.forget(id);
    if id < SERVER_ID_START {
        client.post(1, 1, display::DELETE_ID, &[Value::Uint(id)]);
    }
}

fn new_object(
    client: &mut ClientSlot,
    id: u32,
    interface: Interface,
    version: u32,
    index: u16,
) -> bool {
    if !client.is_free_id(id) {
        let code = match (id as usize) < MAX_OBJECTS {
            true => display::INVALID_OBJECT,
            false => display::NO_MEMORY,
        };
        client.protocol_error(id, code, b"bad new id");
        return false;
    }
    client.create(id, interface, version, index);
    true
}

fn object_index(client: &mut ClientSlot, id: u32, interface: Interface) -> Option<u16> {
    match client.object(id) {
        Some(object) if object.interface == Some(interface) && !object.inert => Some(object.index),
        _ => {
            client.protocol_error(id, display::INVALID_OBJECT, b"wrong object");
            None
        }
    }
}

fn handle<S: System>(
    client: &mut ClientSlot,
    shared: &mut Shared,
    system: &mut S,
    message: &Message,
) {
    match message.interface {
        Interface::Display => display_request(client, shared, message),
        Interface::Registry => bind(client, system, message),
        Interface::Compositor => compositor_request(client, message),
        Interface::Shm => shm_request(client, shared, system, message),
        Interface::ShmPool => pool_request(client, shared, system, message),
        Interface::Buffer => destroy_buffer(client, shared, system, message.index),
        Interface::Surface => surface_request(client, shared, system, message),
        Interface::Seat => seat_request(client, shared, system, message),
        Interface::XdgWmBase => wm_base_request(client, message),
        Interface::XdgSurface => xdg_surface_request(client, message),
        Interface::XdgToplevel => toplevel_request(client, system, message),
        Interface::Region
        | Interface::Callback
        | Interface::Pointer
        | Interface::Keyboard
        | Interface::Touch
        | Interface::Output
        | Interface::XdgPositioner
        | Interface::XdgPopup => {}
    }
}

fn display_request(client: &mut ClientSlot, shared: &mut Shared, message: &Message) {
    let id = message.uint(0);
    match message.opcode {
        0 => {
            if new_object(client, id, Interface::Callback, 1, 0) {
                let serial = shared.next_serial();
                client.post(id, 1, callback::DONE, &[Value::Uint(serial)]);
                destroy_object(client, id);
            }
        }
        _ => {
            if new_object(client, id, Interface::Registry, 1, 0) {
                for global in protocol::GLOBALS {
                    client.post(
                        id,
                        1,
                        registry::GLOBAL,
                        &[
                            Value::Uint(global.name),
                            Value::String(global.interface.name().as_bytes()),
                            Value::Uint(global.version),
                        ],
                    );
                }
            }
        }
    }
}

fn bind<S: System>(client: &mut ClientSlot, system: &mut S, message: &Message) {
    let Value::UntypedNewId {
        interface,
        version,
        id,
    } = message.values[1]
    else {
        return;
    };
    let global = protocol::global(message.uint(0)).filter(|global| {
        global.interface.name().as_bytes() == interface && (1..=global.version).contains(&version)
    });
    let Some(global) = global else {
        client.protocol_error(message.object, display::INVALID_OBJECT, b"bad bind");
        return;
    };
    if !new_object(client, id, global.interface, version, 0) {
        return;
    }
    match global.interface {
        Interface::Shm => {
            for format in [shm::ARGB8888, shm::XRGB8888] {
                client.post(id, version, shm::FORMAT, &[Value::Uint(format)]);
            }
        }
        Interface::Seat => {
            client.post(
                id,
                version,
                seat::CAPABILITIES,
                &[Value::Uint(seat::POINTER | seat::KEYBOARD)],
            );
            client.post(id, version, seat::NAME, &[Value::String(b"seat0")]);
        }
        Interface::Output => {
            let (width, height) = system.work_area();
            send_output(client, id, version, width, height);
            for index in 0..MAX_SURFACES {
                if let Some(surface) =
                    client.surfaces[index].filter(|surface| surface.window.is_some())
                {
                    client.post(surface.id, 1, surface::ENTER, &[Value::Object(id)]);
                }
            }
        }
        _ => {}
    }
}

fn send_output(client: &mut ClientSlot, id: u32, version: u32, width: u32, height: u32) {
    client.post(
        id,
        version,
        output::GEOMETRY,
        &[
            Value::Int(0),
            Value::Int(0),
            Value::Int(0),
            Value::Int(0),
            Value::Int(output::SUBPIXEL_UNKNOWN),
            Value::String(b"EggOS"),
            Value::String(b"display"),
            Value::Int(output::TRANSFORM_NORMAL),
        ],
    );
    client.post(
        id,
        version,
        output::MODE,
        &[
            Value::Uint(output::MODE_CURRENT | output::MODE_PREFERRED),
            Value::Int(width as i32),
            Value::Int(height as i32),
            Value::Int(REFRESH_MILLIHERTZ),
        ],
    );
    client.post(id, version, output::SCALE, &[Value::Int(1)]);
    client.post(id, version, output::DONE, &[]);
}

fn compositor_request(client: &mut ClientSlot, message: &Message) {
    let id = message.uint(0);
    match message.opcode {
        0 => {
            let Some(free) = client.surfaces.iter().position(Option::is_none) else {
                client.protocol_error(id, display::NO_MEMORY, b"too many surfaces");
                return;
            };
            if new_object(client, id, Interface::Surface, message.version, free as u16) {
                client.surfaces[free] = Some(Surface::new(id));
            }
        }
        _ => {
            new_object(client, id, Interface::Region, message.version, 0);
        }
    }
}

fn shm_request<S: System>(
    client: &mut ClientSlot,
    shared: &mut Shared,
    system: &mut S,
    message: &Message,
) {
    if message.opcode != 0 {
        return;
    }
    let (id, descriptor, size) = (message.uint(0), message.descriptors[0], message.int(2));
    let refuse = |client: &mut ClientSlot, system: &mut S, object, code, text: &[u8]| {
        system.close(descriptor);
        client.protocol_error(object, code, text);
    };
    if size <= 0 {
        return refuse(
            client,
            system,
            message.object,
            shm::INVALID_STRIDE,
            b"bad pool size",
        );
    }
    let length = size as usize;
    let Some(free) = client.pools.iter().position(Option::is_none) else {
        return refuse(client, system, id, display::NO_MEMORY, b"too many pools");
    };
    if length > MAX_POOL_BYTES
        || client.pool_bytes + length > MAX_CLIENT_POOL_BYTES
        || shared.pool_bytes + length > MAX_TOTAL_POOL_BYTES
    {
        return refuse(client, system, id, display::NO_MEMORY, b"pool too large");
    }
    if !client.is_free_id(id) {
        system.close(descriptor);
        new_object(client, id, Interface::ShmPool, message.version, 0);
        return;
    }
    match system.map_pool(descriptor, length) {
        Ok(address) => {
            client.pools[free] = Some(Pool {
                descriptor,
                address,
                length,
                buffers: 0,
                destroyed: false,
            });
            client.pool_bytes += length;
            shared.pool_bytes += length;
            new_object(client, id, Interface::ShmPool, message.version, free as u16);
        }
        Err(Errno::ENOMEM) => refuse(client, system, id, display::NO_MEMORY, b"no memory"),
        Err(_) => refuse(
            client,
            system,
            message.object,
            shm::INVALID_FD,
            b"cannot map",
        ),
    }
}

fn release_pool_if_unused<S: System>(
    client: &mut ClientSlot,
    shared: &mut Shared,
    system: &mut S,
    index: usize,
) {
    if let Some(pool) = client.pools[index].filter(|pool| pool.destroyed && pool.buffers == 0) {
        system.unmap_pool(pool.address, pool.length);
        system.close(pool.descriptor);
        client.pool_bytes -= pool.length;
        shared.pool_bytes = shared.pool_bytes.saturating_sub(pool.length);
        client.pools[index] = None;
    }
}

fn pool_request<S: System>(
    client: &mut ClientSlot,
    shared: &mut Shared,
    system: &mut S,
    message: &Message,
) {
    let index = usize::from(message.index);
    let Some(pool) = client.pools[index] else {
        return;
    };
    match message.opcode {
        0 => create_buffer(client, message, pool),
        1 => {
            if let Some(pool) = client.pools[index].as_mut() {
                pool.destroyed = true;
            }
            release_pool_if_unused(client, shared, system, index);
        }
        _ => {
            let size = message.int(0);
            if size < 0 || (size as usize) < pool.length {
                client.protocol_error(message.object, shm::INVALID_STRIDE, b"pool shrinks");
                return;
            }
            let length = size as usize;
            let growth = length - pool.length;
            if growth == 0 {
                return;
            }
            if length > MAX_POOL_BYTES
                || client.pool_bytes + growth > MAX_CLIENT_POOL_BYTES
                || shared.pool_bytes + growth > MAX_TOTAL_POOL_BYTES
            {
                client.protocol_error(message.object, display::NO_MEMORY, b"pool too large");
                return;
            }
            match system.map_pool(pool.descriptor, length) {
                Ok(address) => {
                    system.unmap_pool(pool.address, pool.length);
                    client.pools[index] = Some(Pool {
                        address,
                        length,
                        ..pool
                    });
                    client.pool_bytes += growth;
                    shared.pool_bytes += growth;
                }
                Err(Errno::ENOMEM) => {
                    client.protocol_error(message.object, display::NO_MEMORY, b"no memory");
                }
                Err(_) => client.protocol_error(message.object, shm::INVALID_FD, b"cannot map"),
            }
        }
    }
}

fn create_buffer(client: &mut ClientSlot, message: &Message, pool: Pool) {
    let id = message.uint(0);
    let (offset, width, height, stride, format) = (
        message.int(1),
        message.int(2),
        message.int(3),
        message.int(4),
        message.uint(5),
    );
    if format != shm::ARGB8888 && format != shm::XRGB8888 {
        client.protocol_error(message.object, shm::INVALID_FORMAT, b"bad format");
        return;
    }
    let geometry = (|| {
        if offset < 0 || offset % 4 != 0 || width <= 0 || height <= 0 || stride <= 0 {
            return None;
        }
        let (offset, width, height, stride) = (
            offset as usize,
            width as usize,
            height as usize,
            stride as usize,
        );
        let row = width.checked_mul(4)?;
        if stride < row || !stride.is_multiple_of(4) || row.checked_mul(height)? > MAX_BUFFER_BYTES
        {
            return None;
        }
        let end = offset.checked_add(stride.checked_mul(height)?)?;
        (end <= pool.length).then_some((offset, width as u32, height as u32, stride))
    })();
    let Some((offset, width, height, stride)) = geometry else {
        client.protocol_error(message.object, shm::INVALID_STRIDE, b"bad buffer geometry");
        return;
    };
    let Some(free) = client.buffers.iter().position(Option::is_none) else {
        client.protocol_error(id, display::NO_MEMORY, b"too many buffers");
        return;
    };
    if new_object(client, id, Interface::Buffer, message.version, free as u16) {
        client.buffers[free] = Some(Buffer {
            id,
            pool: message.index as u8,
            offset,
            width,
            height,
            stride,
        });
        if let Some(pool) = client.pools[usize::from(message.index)].as_mut() {
            pool.buffers += 1;
        }
    }
}

fn destroy_buffer<S: System>(
    client: &mut ClientSlot,
    shared: &mut Shared,
    system: &mut S,
    index: u16,
) {
    let Some(buffer) = client.buffers[usize::from(index)].take() else {
        return;
    };
    for surface in client.surfaces.iter_mut().flatten() {
        if surface.attach == Attach::Buffer(index) {
            surface.attach = Attach::Unchanged;
        }
    }
    let pool = usize::from(buffer.pool);
    if let Some(pool) = client.pools[pool].as_mut() {
        pool.buffers = pool.buffers.saturating_sub(1);
    }
    release_pool_if_unused(client, shared, system, pool);
}

fn surface_request<S: System>(
    client: &mut ClientSlot,
    shared: &mut Shared,
    system: &mut S,
    message: &Message,
) {
    let index = message.index;
    match message.opcode {
        0 => destroy_surface(client, system, index),
        1 => {
            let attach = match message.uint(0) {
                0 => Attach::Null,
                id => match object_index(client, id, Interface::Buffer) {
                    Some(buffer) => Attach::Buffer(buffer),
                    None => return,
                },
            };
            if let Some(surface) = client.surface(index) {
                surface.attach = attach;
            }
        }
        2 | 9 => {
            let damage = rect_of(message, 0);
            if let Some(surface) = client.surface(index) {
                surface.damage = surface.damage.union(&damage);
            }
        }
        3 => frame(client, message),
        4 | 5 => {
            let region = message.uint(0);
            if region != 0 {
                object_index(client, region, Interface::Region);
            }
        }
        6 => commit(client, shared, system, index),
        7 if !(0..=surface::LARGEST_TRANSFORM).contains(&message.int(0)) => {
            client.protocol_error(message.object, surface::INVALID_TRANSFORM, b"bad transform");
        }
        8 if message.int(0) < 1 => {
            client.protocol_error(message.object, surface::INVALID_SCALE, b"bad scale");
        }
        _ => {}
    }
}

fn frame(client: &mut ClientSlot, message: &Message) {
    let id = message.uint(0);
    if !new_object(client, id, Interface::Callback, 1, 0) {
        return;
    }
    let Some(surface) = client.surface(message.index) else {
        return;
    };
    if surface.pending_frame_count == MAX_FRAMES {
        let oldest = surface.pending_frames[0];
        surface.pending_frames.copy_within(1.., 0);
        surface.pending_frame_count -= 1;
        client.post(oldest, 1, callback::DONE, &[Value::Uint(0)]);
        destroy_object(client, oldest);
    }
    if let Some(surface) = client.surface(message.index) {
        surface.pending_frames[surface.pending_frame_count] = id;
        surface.pending_frame_count += 1;
    }
}

fn fire_frames(client: &mut ClientSlot, index: u16, now: u64) {
    let Some(surface) = client.surface(index) else {
        return;
    };
    let (frames, count) = (surface.frames, surface.frame_count);
    surface.frame_count = 0;
    surface.last_done_ms = now;
    for &id in &frames[..count] {
        client.post(id, 1, callback::DONE, &[Value::Uint(now as u32)]);
        destroy_object(client, id);
    }
}

fn destroy_surface<S: System>(client: &mut ClientSlot, system: &mut S, index: u16) {
    let Some(surface) = client.surfaces[usize::from(index)].take() else {
        return;
    };
    if let (Some(window), Some(device)) = (surface.window, client.device) {
        system.destroy_window(device, window);
    }
    let pending = &surface.pending_frames[..surface.pending_frame_count];
    for &id in surface.frames[..surface.frame_count].iter().chain(pending) {
        client.post(id, 1, callback::DONE, &[Value::Uint(0)]);
        destroy_object(client, id);
    }
    for object in client.objects.iter_mut() {
        let role = matches!(
            object.interface,
            Some(Interface::XdgSurface | Interface::XdgToplevel | Interface::XdgPopup)
        );
        if role && object.index == index {
            object.inert = true;
        }
    }
    forget_focus(client, index);
}

fn forget_focus(client: &mut ClientSlot, index: u16) {
    if client.pointer_focus == Some(index) {
        client.pointer_focus = None;
    }
    if client.keyboard_focus == Some(index) {
        client.keyboard_focus = None;
    }
    if client
        .held_press
        .is_some_and(|(_, surface)| surface == index)
    {
        client.held_press = None;
    }
    if client
        .pending_motion
        .is_some_and(|motion| motion.0 == index)
    {
        client.pending_motion = None;
    }
}

fn release_buffer(client: &mut ClientSlot, buffer: u16) {
    if let Some(buffer) = client.buffers[usize::from(buffer)] {
        client.post(buffer.id, 1, buffer::RELEASE, &[]);
    }
}

fn commit<S: System>(client: &mut ClientSlot, shared: &mut Shared, system: &mut S, index: u16) {
    let Some(mut surface) = client.surface(index).copied() else {
        return;
    };
    for &id in &surface.pending_frames[..surface.pending_frame_count] {
        if surface.frame_count == MAX_FRAMES {
            let oldest = surface.frames[0];
            surface.frames.copy_within(1.., 0);
            surface.frame_count -= 1;
            client.post(oldest, 1, callback::DONE, &[Value::Uint(0)]);
            destroy_object(client, oldest);
        }
        surface.frames[surface.frame_count] = id;
        surface.frame_count += 1;
    }
    surface.pending_frame_count = 0;
    let attach = surface.attach;
    let damage = surface.damage;
    surface.attach = Attach::Unchanged;
    surface.damage = Rect::default();
    if attach != Attach::Unchanged {
        surface.ever_attached = true;
    }
    if let Some(slot) = client.surface(index) {
        *slot = surface;
    }
    if surface.toplevel == 0 {
        if let Attach::Buffer(buffer) = attach {
            release_buffer(client, buffer);
        }
        return;
    }
    if !surface.initial_configure_sent || !surface.acked {
        if matches!(attach, Attach::Buffer(_)) {
            client.protocol_error(
                surface.xdg_surface,
                xdg_surface::UNCONFIGURED_BUFFER,
                b"buffer before the first configure",
            );
            return;
        }
        if !surface.initial_configure_sent {
            if let Some(slot) = client.surface(index) {
                slot.initial_configure_sent = true;
            }
            configure(client, shared, index, (0, 0));
        }
        return;
    }
    match attach {
        Attach::Unchanged => {}
        Attach::Null => {
            if let (Some(window), Some(device)) = (surface.window, client.device) {
                system.destroy_window(device, window);
            }
            if let Some(slot) = client.surface(index) {
                slot.window = None;
                slot.asked_size = (0, 0);
            }
            forget_focus(client, index);
        }
        Attach::Buffer(buffer) => {
            present(client, shared, system, index, buffer, damage);
            release_buffer(client, buffer);
        }
    }
}

fn present<S: System>(
    client: &mut ClientSlot,
    shared: &mut Shared,
    system: &mut S,
    index: u16,
    buffer: u16,
    damage: Rect,
) {
    let (Some(surface), Some(buffer)) = (
        client.surface(index).copied(),
        client.buffers[usize::from(buffer)],
    ) else {
        return;
    };
    let Some(pool) = client.pools[usize::from(buffer.pool)] else {
        return;
    };
    let device = match client.device {
        Some(device) => device,
        None => match system.open_window_device() {
            Ok(device) => {
                client.device = Some(device);
                device
            }
            Err(_) => {
                client.protocol_error(1, display::IMPLEMENTATION, b"no display");
                return;
            }
        },
    };
    let pixels = PoolPixels {
        address: pool.address,
        length: pool.length,
        offset: buffer.offset,
        stride: buffer.stride,
    };
    let size = (buffer.width, buffer.height);
    let (window, everything) = match surface.window {
        None => {
            let fixed = surface.minimum == surface.maximum
                && surface.minimum.0 > 0
                && surface.minimum.1 > 0;
            let flags = if fixed { FIXED_SIZE } else { 0 };
            let Ok(window) = system.create_window(device, size.0, size.1, flags) else {
                client.protocol_error(1, display::NO_MEMORY, b"no window");
                return;
            };
            if surface.title_length > 0 {
                let _ = system.set_title(device, window.id, &surface.title[..surface.title_length]);
            }
            if surface.minimum != (0, 0) {
                let _ = system.set_minimum_size(
                    device,
                    window.id,
                    surface.minimum.0,
                    surface.minimum.1,
                );
            }
            (window, true)
        }
        Some(mut window) if size != surface.asked_size => {
            if system
                .resize_window(device, &mut window, size.0, size.1)
                .is_err()
            {
                client.protocol_error(1, display::NO_MEMORY, b"no window buffer");
                return;
            }
            (window, true)
        }
        Some(window) => (window, false),
    };
    let visible = (size.0.min(window.width), size.1.min(window.height));
    let rect = match everything {
        true => Rect::new(0, 0, visible.0, visible.1),
        false => damage.clipped(visible.0, visible.1),
    };
    if !rect.is_empty() {
        system.copy_rows(pixels, window, rect);
    }
    let rects: &[Rect] = match everything {
        true => &[],
        false => core::slice::from_ref(&rect),
    };
    if everything || !rect.is_empty() {
        let _ = system.commit(device, window.id, rects);
    }
    let created = surface.window.is_none();
    if let Some(slot) = client.surface(index) {
        slot.window = Some(window);
        slot.asked_size = size;
    }
    if created {
        if surface.wants_maximized {
            let _ = system.set_state(device, window.id, window_state::MAXIMISED);
        }
        let outputs: [u32; 8] = {
            let mut ids = [0; 8];
            for (slot, (id, _)) in ids.iter_mut().zip(client.ids_of(Interface::Output)) {
                *slot = id;
            }
            ids
        };
        for &output in outputs.iter().filter(|&&id| id != 0) {
            client.post(surface.id, 1, surface::ENTER, &[Value::Object(output)]);
        }
        if (window.width, window.height) != size {
            configure(client, shared, index, (window.width, window.height));
        }
    }
}

fn configure(client: &mut ClientSlot, shared: &mut Shared, index: u16, size: (u32, u32)) {
    let Some(surface) = client.surface(index) else {
        return;
    };
    let mut size = size;
    if surface.maximum.0 > 0 && size.0 > 0 {
        size.0 = size.0.min(surface.maximum.0);
    }
    if surface.maximum.1 > 0 && size.1 > 0 {
        size.1 = size.1.min(surface.maximum.1);
    }
    surface.configure_size = size;
    let serial = shared.next_serial();
    surface.last_serial = serial;
    let (toplevel, xdg) = (surface.toplevel, surface.xdg_surface);
    let mut states = [0u8; 8];
    let mut length = 0;
    for (on, state) in [
        (surface.maximized, xdg_toplevel::STATE_MAXIMIZED),
        (surface.activated, xdg_toplevel::STATE_ACTIVATED),
    ] {
        if on {
            states[length..length + 4].copy_from_slice(&state.to_ne_bytes());
            length += 4;
        }
    }
    let version = client.object(toplevel).map_or(1, |object| object.version);
    client.post(
        toplevel,
        version,
        xdg_toplevel::CONFIGURE,
        &[
            Value::Int(size.0 as i32),
            Value::Int(size.1 as i32),
            Value::Array(&states[..length]),
        ],
    );
    client.post(xdg, version, xdg_surface::CONFIGURE, &[Value::Uint(serial)]);
}

fn current_size(surface: &Surface) -> (u32, u32) {
    match surface.configure_size {
        (0, 0) => surface
            .window
            .map_or((0, 0), |window| (window.width, window.height)),
        size => size,
    }
}

fn seat_request<S: System>(
    client: &mut ClientSlot,
    shared: &mut Shared,
    system: &mut S,
    message: &Message,
) {
    let id = message.uint(0);
    match message.opcode {
        0 => {
            if !new_object(client, id, Interface::Pointer, message.version, 0) {
                return;
            }
            if let Some(surface) = client.pointer_focus.and_then(|index| client.surface(index)) {
                let surface = surface.id;
                let (x, y) = client.pointer_at;
                let serial = shared.next_serial();
                client.post(
                    id,
                    message.version,
                    pointer::ENTER,
                    &[
                        Value::Uint(serial),
                        Value::Object(surface),
                        Value::Fixed(wire::fixed(x)),
                        Value::Fixed(wire::fixed(y)),
                    ],
                );
                client.post(id, message.version, pointer::FRAME, &[]);
            }
        }
        1 => {
            if !new_object(client, id, Interface::Keyboard, message.version, 0) {
                return;
            }
            send_keymap(client, system, id, message.version);
            client.post(
                id,
                message.version,
                keyboard::REPEAT_INFO,
                &[Value::Int(REPEAT_RATE), Value::Int(REPEAT_DELAY_MS)],
            );
            if let Some(surface) = client
                .keyboard_focus
                .and_then(|index| client.surface(index))
            {
                let surface = surface.id;
                let (depressed, locked) = client.modifiers;
                let (enter, modifiers) = (shared.next_serial(), shared.next_serial());
                client.post(
                    id,
                    message.version,
                    keyboard::ENTER,
                    &[
                        Value::Uint(enter),
                        Value::Object(surface),
                        Value::Array(&[]),
                    ],
                );
                client.post(
                    id,
                    message.version,
                    keyboard::MODIFIERS,
                    &[
                        Value::Uint(modifiers),
                        Value::Uint(depressed),
                        Value::Uint(0),
                        Value::Uint(locked),
                        Value::Uint(0),
                    ],
                );
            }
        }
        2 => {
            new_object(client, id, Interface::Touch, message.version, 0);
        }
        _ => {}
    }
}

fn send_keymap<S: System>(client: &mut ClientSlot, system: &mut S, id: u32, version: u32) {
    if client.descriptors_out_count == MAX_OUT_DESCRIPTORS {
        flush(client, system);
    }
    if client.descriptors_out_count == MAX_OUT_DESCRIPTORS {
        client.blocked = true;
        client.protocol_error(id, display::NO_MEMORY, b"descriptors not read");
        return;
    }
    let Ok((descriptor, size)) = system.keymap() else {
        client.protocol_error(id, display::NO_MEMORY, b"no keymap");
        return;
    };
    client.descriptors_out[client.descriptors_out_count] = descriptor;
    client.descriptors_out_count += 1;
    client.post(
        id,
        version,
        keyboard::KEYMAP,
        &[Value::Uint(keyboard::XKB_V1), Value::Fd, Value::Uint(size)],
    );
}

fn wm_base_request(client: &mut ClientSlot, message: &Message) {
    match message.opcode {
        0 => {
            if client
                .surfaces
                .iter()
                .flatten()
                .any(|surface| surface.xdg_surface != 0)
            {
                client.protocol_error(
                    message.object,
                    xdg_wm_base::DEFUNCT_SURFACES,
                    b"surfaces left",
                );
            }
        }
        1 => {
            new_object(
                client,
                message.uint(0),
                Interface::XdgPositioner,
                message.version,
                0,
            );
        }
        2 => {
            let (id, surface_id) = (message.uint(0), message.uint(1));
            let Some(index) = object_index(client, surface_id, Interface::Surface) else {
                return;
            };
            let Some(surface) = client.surface(index).copied() else {
                return;
            };
            if surface.xdg_surface != 0 {
                client.protocol_error(message.object, xdg_wm_base::ROLE, b"surface has a role");
                return;
            }
            if surface.ever_attached {
                client.protocol_error(
                    message.object,
                    xdg_wm_base::INVALID_SURFACE_STATE,
                    b"surface has a buffer",
                );
                return;
            }
            if new_object(client, id, Interface::XdgSurface, message.version, index)
                && let Some(surface) = client.surface(index)
            {
                surface.xdg_surface = id;
            }
        }
        _ => {}
    }
}

fn xdg_surface_request(client: &mut ClientSlot, message: &Message) {
    let index = message.index;
    let Some(surface) = client.surface(index).copied() else {
        return;
    };
    match message.opcode {
        0 => {
            if surface.toplevel != 0 {
                client.protocol_error(
                    message.object,
                    xdg_surface::DEFUNCT_ROLE_OBJECT,
                    b"toplevel still alive",
                );
            } else if let Some(surface) = client.surface(index) {
                surface.xdg_surface = 0;
            }
        }
        1 => {
            let id = message.uint(0);
            if surface.toplevel != 0 {
                client.protocol_error(
                    message.object,
                    xdg_surface::ALREADY_CONSTRUCTED,
                    b"has a role",
                );
                return;
            }
            if new_object(client, id, Interface::XdgToplevel, message.version, index)
                && let Some(surface) = client.surface(index)
            {
                surface.toplevel = id;
            }
        }
        2 => {
            let (id, parent, positioner) = (message.uint(0), message.uint(1), message.uint(2));
            if parent != 0 && object_index(client, parent, Interface::XdgSurface).is_none() {
                return;
            }
            if object_index(client, positioner, Interface::XdgPositioner).is_none() {
                return;
            }
            if new_object(client, id, Interface::XdgPopup, message.version, index) {
                client.post(id, message.version, xdg_popup::POPUP_DONE, &[]);
            }
        }
        3 => {
            if message.int(2) <= 0 || message.int(3) <= 0 {
                client.protocol_error(message.object, xdg_surface::INVALID_SIZE, b"bad geometry");
            }
        }
        _ => {
            let serial = message.uint(0);
            if serial == 0 || serial > surface.last_serial {
                client.protocol_error(message.object, xdg_surface::INVALID_SERIAL, b"bad serial");
            } else if let Some(surface) = client.surface(index) {
                surface.acked = true;
            }
        }
    }
}

fn resize_edges(edge: u32) -> Option<u32> {
    Some(match edge {
        xdg_toplevel::EDGE_NONE => 0,
        xdg_toplevel::EDGE_TOP => move_or_resize::TOP,
        xdg_toplevel::EDGE_BOTTOM => move_or_resize::BOTTOM,
        xdg_toplevel::EDGE_LEFT => move_or_resize::LEFT,
        xdg_toplevel::EDGE_TOP_LEFT => move_or_resize::TOP | move_or_resize::LEFT,
        xdg_toplevel::EDGE_BOTTOM_LEFT => move_or_resize::BOTTOM | move_or_resize::LEFT,
        xdg_toplevel::EDGE_RIGHT => move_or_resize::RIGHT,
        xdg_toplevel::EDGE_TOP_RIGHT => move_or_resize::TOP | move_or_resize::RIGHT,
        xdg_toplevel::EDGE_BOTTOM_RIGHT => move_or_resize::BOTTOM | move_or_resize::RIGHT,
        _ => return None,
    })
}

fn title_prefix(title: &[u8]) -> &[u8] {
    let mut end = title.len().min(TITLE_CAPACITY);
    while end > 0 && core::str::from_utf8(&title[..end]).is_err() {
        end -= 1;
    }
    &title[..end]
}

fn toplevel_request<S: System>(client: &mut ClientSlot, system: &mut S, message: &Message) {
    let index = message.index;
    let Some(surface) = client.surface(index).copied() else {
        return;
    };
    let window = surface.window.map(|window| window.id);
    let device = client.device;
    let on_window = |system: &mut S, act: &mut dyn FnMut(&mut S, Fd, u32)| {
        if let (Some(device), Some(window)) = (device, window) {
            act(system, device, window);
        }
    };
    match message.opcode {
        0 => {
            if let (Some(window), Some(device)) = (surface.window, device) {
                system.destroy_window(device, window);
            }
            if let Some(surface) = client.surface(index) {
                surface.reset_toplevel();
            }
            forget_focus(client, index);
        }
        2 => {
            let title = title_prefix(message.values[0].bytes());
            if let Some(surface) = client.surface(index) {
                surface.title[..title.len()].copy_from_slice(title);
                surface.title_length = title.len();
            }
            on_window(system, &mut |system, device, window| {
                let _ = system.set_title(device, window, title);
            });
        }
        3 => {
            let app_id = title_prefix(message.values[0].bytes());
            if let Some(surface) = client.surface(index) {
                surface.app_id[..app_id.len()].copy_from_slice(app_id);
                surface.app_id_length = app_id.len();
            }
        }
        5 | 6 => {
            let (seat, serial) = (message.uint(0), message.uint(1));
            if object_index(client, seat, Interface::Seat).is_none() {
                return;
            }
            let (kind, edges) = match message.opcode {
                5 => (move_or_resize::MOVE, 0),
                _ => match resize_edges(message.uint(2)) {
                    Some(0) => return,
                    Some(edges) => (move_or_resize::RESIZE, edges),
                    None => {
                        client.protocol_error(
                            message.object,
                            xdg_toplevel::INVALID_RESIZE_EDGE,
                            b"bad edge",
                        );
                        return;
                    }
                },
            };
            if client.held_press != Some((serial, index)) {
                return;
            }
            let mut started = false;
            on_window(system, &mut |system, device, window| {
                started = system
                    .start_move_or_resize(device, window, kind, edges)
                    .is_ok();
            });
            if started {
                client.held_press = None;
            }
        }
        7 | 8 => {
            let (width, height) = (message.int(0), message.int(1));
            if width < 0 || height < 0 {
                client.protocol_error(message.object, xdg_toplevel::INVALID_SIZE, b"bad size");
                return;
            }
            let size = (width as u32, height as u32);
            if let Some(surface) = client.surface(index) {
                match message.opcode {
                    7 => surface.maximum = size,
                    _ => surface.minimum = size,
                }
            }
            if message.opcode == 8 {
                on_window(system, &mut |system, device, window| {
                    let _ = system.set_minimum_size(device, window, size.0, size.1);
                });
            }
        }
        9 | 11 => {
            if let Some(surface) = client.surface(index) {
                surface.wants_maximized = true;
            }
            on_window(system, &mut |system, device, window| {
                let _ = system.set_state(device, window, window_state::MAXIMISED);
            });
        }
        10 | 12 => {
            if let Some(surface) = client.surface(index) {
                surface.wants_maximized = false;
            }
            on_window(system, &mut |system, device, window| {
                let _ = system.set_state(device, window, window_state::NORMAL);
            });
        }
        13 => on_window(system, &mut |system, device, window| {
            let _ = system.set_state(device, window, window_state::MINIMISED);
        }),
        _ => {}
    }
}

fn surface_of_window(client: &ClientSlot, window: u16) -> Option<u16> {
    client
        .surfaces
        .iter()
        .position(|surface| {
            surface
                .and_then(|surface| surface.window)
                .is_some_and(|buffer| buffer.id == u32::from(window))
        })
        .map(|index| index as u16)
}

fn word(record: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes([
        record[offset],
        record[offset + 1],
        record[offset + 2],
        record[offset + 3],
    ])
}

fn each_of(
    client: &mut ClientSlot,
    interface: Interface,
    mut post: impl FnMut(&mut ClientSlot, u32, u32),
) {
    let mut ids = [(0u32, 0u32); 8];
    let mut count = 0;
    for (id, version) in client.ids_of(interface) {
        if count < ids.len() {
            ids[count] = (id, version);
            count += 1;
        }
    }
    for &(id, version) in &ids[..count] {
        post(client, id, version);
    }
}

fn pointer_frame(client: &mut ClientSlot) {
    each_of(client, Interface::Pointer, |client, id, version| {
        client.post(id, version, pointer::FRAME, &[]);
    });
}

fn flush_motion(client: &mut ClientSlot, _shared: &mut Shared) {
    let Some((index, time, x, y)) = client.pending_motion.take() else {
        return;
    };
    if client.pointer_focus != Some(index) {
        return;
    }
    each_of(client, Interface::Pointer, |client, id, version| {
        let _ = client.queue(
            id,
            version,
            pointer::MOTION,
            &[
                Value::Uint(time),
                Value::Fixed(wire::fixed(x)),
                Value::Fixed(wire::fixed(y)),
            ],
        );
    });
    pointer_frame(client);
}

fn answer_query<S: System>(
    client: &mut ClientSlot,
    system: &mut S,
    device: Fd,
    record: &[u8; RECORD_SIZE],
) {
    let Some(query) = Query::from_record(record) else {
        return;
    };
    let surface = u16::try_from(query.window)
        .ok()
        .and_then(|window| surface_of_window(client, window))
        .and_then(|index| client.surface(index).copied());
    let Some(surface) = surface.filter(|_| query.kind == query_kind::TREE) else {
        let outcome = match query.kind {
            query_kind::TREE => tree_answer::PIXELS_ONLY,
            _ => Outcome::NotSupported.value(),
        };
        let _ = system.answer_query(device, query.serial, outcome, &[]);
        return;
    };
    let (width, height) = current_size(&surface);
    let mut bytes = [0u8; HEADER_BYTES + 2 * TITLE_CAPACITY];
    let mut tree = TreeWriter::new(&mut bytes);
    let bounds = Bounds::new(0, 0, width as i32, height as i32);
    tree.push(
        &Element::new(ROOT_ELEMENT, 0, Role::Window, bounds)
            .named(&surface.title[..surface.title_length])
            .valued(&surface.app_id[..surface.app_id_length]),
    );
    let _ = system.answer_query(device, query.serial, tree_answer::TREE, tree.bytes());
}

fn window_event(client: &mut ClientSlot, shared: &mut Shared, record: &[u8]) {
    let kind = u16::from_ne_bytes([record[0], record[1]]);
    let window = u16::from_ne_bytes([record[2], record[3]]);
    let (time, a, b, c, d) = (
        word(record, 4),
        word(record, 8) as i32,
        word(record, 12) as i32,
        word(record, 16),
        word(record, 20),
    );
    let Some(index) = surface_of_window(client, window) else {
        return;
    };
    let Some(surface) = client.surface(index).copied() else {
        return;
    };
    if kind != record::POINTER_MOVE {
        flush_motion(client, shared);
    }
    match kind {
        record::POINTER_ENTER => {
            client.pointer_focus = Some(index);
            client.pointer_at = (a, b);
            let serial = shared.next_serial();
            each_of(client, Interface::Pointer, |client, id, version| {
                client.post(
                    id,
                    version,
                    pointer::ENTER,
                    &[
                        Value::Uint(serial),
                        Value::Object(surface.id),
                        Value::Fixed(wire::fixed(a)),
                        Value::Fixed(wire::fixed(b)),
                    ],
                );
            });
            pointer_frame(client);
        }
        record::POINTER_MOVE => {
            if client.pointer_focus == Some(index) {
                client.pending_motion = Some((index, time, a, b));
                client.pointer_at = (a, b);
            }
        }
        record::POINTER_LEAVE => {
            if client.pointer_focus == Some(index) {
                client.pointer_focus = None;
                let serial = shared.next_serial();
                each_of(client, Interface::Pointer, |client, id, version| {
                    client.post(
                        id,
                        version,
                        pointer::LEAVE,
                        &[Value::Uint(serial), Value::Object(surface.id)],
                    );
                });
                pointer_frame(client);
            }
        }
        record::BUTTON => {
            let serial = shared.next_serial();
            let state = if d == 1 {
                pointer::PRESSED
            } else {
                pointer::RELEASED
            };
            each_of(client, Interface::Pointer, |client, id, version| {
                client.post(
                    id,
                    version,
                    pointer::BUTTON,
                    &[
                        Value::Uint(serial),
                        Value::Uint(time),
                        Value::Uint(c),
                        Value::Uint(state),
                    ],
                );
            });
            pointer_frame(client);
            if c == BTN_LEFT {
                client.held_press = (state == pointer::PRESSED).then_some((serial, index));
            }
        }
        record::WHEEL => {
            if client.output_room() < OUTPUT_CAPACITY / 2 {
                return;
            }
            let (axis, steps) = match d {
                0 => (pointer::HORIZONTAL_SCROLL, c as i32),
                vertical => (pointer::VERTICAL_SCROLL, (vertical as i32).saturating_neg()),
            };
            each_of(client, Interface::Pointer, |client, id, version| {
                let _ = client.queue(
                    id,
                    version,
                    pointer::AXIS_DISCRETE,
                    &[Value::Uint(axis), Value::Int(steps)],
                );
                let _ = client.queue(
                    id,
                    version,
                    pointer::AXIS,
                    &[
                        Value::Uint(time),
                        Value::Uint(axis),
                        Value::Fixed(wire::fixed(steps.saturating_mul(WHEEL_STEP))),
                    ],
                );
            });
            pointer_frame(client);
        }
        record::KEY => {
            send_modifiers(client, shared, c);
            let serial = shared.next_serial();
            let state = if b == 1 {
                keyboard::PRESSED
            } else {
                keyboard::RELEASED
            };
            each_of(client, Interface::Keyboard, |client, id, version| {
                client.post(
                    id,
                    version,
                    keyboard::KEY,
                    &[
                        Value::Uint(serial),
                        Value::Uint(time),
                        Value::Uint(a as u32),
                        Value::Uint(state),
                    ],
                );
            });
        }
        record::FOCUS_IN => {
            client.keyboard_focus = Some(index);
            let serial = shared.next_serial();
            each_of(client, Interface::Keyboard, |client, id, version| {
                client.post(
                    id,
                    version,
                    keyboard::ENTER,
                    &[
                        Value::Uint(serial),
                        Value::Object(surface.id),
                        Value::Array(&[]),
                    ],
                );
            });
            let modifiers = client.modifiers;
            post_modifiers(client, shared, modifiers);
            if let Some(slot) = client.surface(index) {
                slot.activated = true;
            }
            configure(client, shared, index, current_size(&surface));
        }
        record::FOCUS_OUT => {
            if client.keyboard_focus == Some(index) {
                client.keyboard_focus = None;
            }
            let serial = shared.next_serial();
            each_of(client, Interface::Keyboard, |client, id, version| {
                client.post(
                    id,
                    version,
                    keyboard::LEAVE,
                    &[Value::Uint(serial), Value::Object(surface.id)],
                );
            });
            if let Some(slot) = client.surface(index) {
                slot.activated = false;
            }
            configure(client, shared, index, current_size(&surface));
        }
        record::CLOSE_REQUEST => {
            let version = client
                .object(surface.toplevel)
                .map_or(1, |object| object.version);
            client.post(surface.toplevel, version, xdg_toplevel::CLOSE, &[]);
        }
        record::RESIZE => configure(client, shared, index, (clamp_to(a), clamp_to(b))),
        record::STATE_CHANGED => {
            let state = a as u32;
            if let Some(slot) = client.surface(index) {
                slot.minimised = state == window_state::MINIMISED;
                if state != window_state::MINIMISED {
                    slot.maximized = state == window_state::MAXIMISED;
                    slot.last_done_ms = 0;
                }
            }
            if state != window_state::MINIMISED
                && let Some(updated) = client.surface(index).copied()
            {
                configure(client, shared, index, current_size(&updated));
            }
        }
        _ => {}
    }
}

fn send_modifiers(client: &mut ClientSlot, shared: &mut Shared, bits: u32) {
    let masks = keymap::modifier_masks(bits);
    if masks != client.modifiers {
        client.modifiers = masks;
        post_modifiers(client, shared, masks);
    }
}

fn post_modifiers(client: &mut ClientSlot, shared: &mut Shared, (depressed, locked): (u32, u32)) {
    let serial = shared.next_serial();
    each_of(client, Interface::Keyboard, |client, id, version| {
        client.post(
            id,
            version,
            keyboard::MODIFIERS,
            &[
                Value::Uint(serial),
                Value::Uint(depressed),
                Value::Uint(0),
                Value::Uint(locked),
                Value::Uint(0),
            ],
        );
    });
}

/// The size of a `WindowBuffer`'s pixels in bytes.
pub fn buffer_bytes(window: &WindowBuffer) -> usize {
    window.width as usize * window.height as usize * 4
}

const _: () = assert!(MAX_POOLS <= u8::MAX as usize && MAX_BUFFERS <= u16::MAX as usize);
