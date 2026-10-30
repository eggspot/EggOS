//! One Wayland client's state (D140, D172), all of it fixed in size: what it sent and what
//! waits to go to it, the descriptors in both directions, its objects by id, and its pools,
//! buffers, surfaces and positioners. The compositor keeps each client in memory of its own,
//! given back when it goes.

use crate::system::{Fd, WindowBuffer};
use eggos_wayland::blit::Rect;
use eggos_wayland::positioner::Positioner;
use eggos_wayland::protocol::{Event, Interface, display};
use eggos_wayland::wire::{self, Value};

/// The highest object id a client may use, plus one.
pub const MAX_OBJECTS: usize = 1024;
/// The bytes of requests held until they are whole.
pub const INPUT_CAPACITY: usize = 8192;
/// The bytes of events held until the client reads them.
pub const OUTPUT_CAPACITY: usize = 16384;
/// The part of the events' room kept for the one `wl_display.error` before a disconnect.
pub const ERROR_RESERVE: usize = 512;
/// The descriptors received and not yet taken by a request.
pub const MAX_IN_DESCRIPTORS: usize = 64;
/// The descriptors waiting to go with the next send.
pub const MAX_OUT_DESCRIPTORS: usize = 4;
/// Surfaces per client.
pub const MAX_SURFACES: usize = 16;
/// Buffers per client.
pub const MAX_BUFFERS: usize = 64;
/// Pools per client.
pub const MAX_POOLS: usize = 8;
/// Frame callbacks waiting per surface.
pub const MAX_FRAMES: usize = 8;
/// The longest title kept, in bytes: `/dev/window`'s.
pub const TITLE_CAPACITY: usize = 64;
/// Positioners per client.
pub const MAX_POSITIONERS: usize = 8;

/// One entry of the object table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Object {
    /// Its interface; `None` when the id is free.
    pub interface: Option<Interface>,
    /// The version it was bound or created at.
    pub version: u32,
    /// The pool, buffer or surface it stands for.
    pub index: u16,
    /// Its surface is gone: its requests do nothing.
    pub inert: bool,
}

/// A `wl_shm_pool`: the client's memory file and its mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pool {
    pub descriptor: Fd,
    pub address: usize,
    pub length: usize,
    /// Buffers still made from it.
    pub buffers: u16,
    /// The client destroyed the pool; the mapping stays while buffers use it.
    pub destroyed: bool,
}

/// A `wl_buffer`: a rectangle of pixels in a pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Buffer {
    pub id: u32,
    pub pool: u8,
    pub offset: usize,
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    /// `ARGB8888`: its alpha counts where it is drawn over another surface.
    pub alpha: bool,
}

/// What the next commit does with the surface's buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Attach {
    /// Nothing attached since the last commit.
    #[default]
    Unchanged,
    /// A null buffer: the window goes.
    Null,
    /// This buffer.
    Buffer(u16),
}

/// A role a surface takes besides the `xdg_surface` ones, which [`Surface::xdg_surface`] marks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Role {
    #[default]
    None,
    /// A `wl_subsurface`'s, drawn over its parent.
    Subsurface,
    /// The pointer's image (`wl_pointer.set_cursor`).
    Cursor,
}

/// A `wp_viewport`'s state: the part of the buffer shown and the size it is shown at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ViewportState {
    /// The source rectangle in buffer pixels, 24.8 fixed point: x, y, width, height.
    pub source: Option<[i32; 4]>,
    /// The surface's size.
    pub destination: Option<(i32, i32)>,
}

impl ViewportState {
    /// Whether it changes how the buffer is shown.
    pub fn is_active(&self) -> bool {
        self.source.is_some() || self.destination.is_some()
    }
}

/// A `wl_surface` and, when it has the role, its `xdg_surface` and `xdg_toplevel` or
/// `xdg_popup`, or its `wl_subsurface`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Surface {
    pub id: u32,
    pub attach: Attach,
    pub damage: Rect,
    pub pending_frames: [u32; MAX_FRAMES],
    pub pending_frame_count: usize,
    pub frames: [u32; MAX_FRAMES],
    pub frame_count: usize,
    pub last_done_ms: u64,
    pub ever_attached: bool,
    pub xdg_surface: u32,
    pub toplevel: u32,
    pub initial_configure_sent: bool,
    pub acked: bool,
    pub last_serial: u32,
    pub configure_size: (u32, u32),
    pub asked_size: (u32, u32),
    pub window: Option<WindowBuffer>,
    pub title: [u8; TITLE_CAPACITY],
    pub title_length: usize,
    pub app_id: [u8; TITLE_CAPACITY],
    pub app_id_length: usize,
    pub minimum: (u32, u32),
    pub maximum: (u32, u32),
    pub wants_maximized: bool,
    pub maximized: bool,
    pub minimised: bool,
    pub activated: bool,
    pub role: Role,
    /// The surface a subsurface or a popup is drawn on.
    pub parent: Option<u16>,
    /// A subsurface's or a popup's place relative to its parent, in surface pixels.
    pub position: (i32, i32),
    /// A subsurface's `set_position`, taken on its parent's next commit.
    pub pending_position: Option<(i32, i32)>,
    /// A subsurface waits for its parent's commit to show what it commits.
    pub synced: bool,
    /// What a synchronized subsurface committed, shown on its parent's next commit.
    pub cached: Option<Attach>,
    /// The buffer kept to draw again when anything in its window changes.
    pub held: Option<u16>,
    /// A subsurface's place among its siblings: above its parent when positive.
    pub order: i32,
    /// The `xdg_popup`, 0 when it is none.
    pub popup: u32,
    /// The popup took the keyboard and dismisses on a click elsewhere.
    pub grabbed: bool,
    /// The popup was dismissed: what it commits is not shown.
    pub dismissed: bool,
    /// The popup's size, as placed.
    pub popup_size: (u32, u32),
    /// The window geometry (`set_window_geometry`): x, y, width, height.
    pub geometry: Option<(i32, i32, i32, i32)>,
    /// The `zxdg_toplevel_decoration_v1`, 0 when it is none.
    pub decoration: u32,
    /// The `wp_viewport`, 0 when it is none.
    pub viewport: u32,
    pub viewport_pending: ViewportState,
    pub viewport_current: ViewportState,
}

impl Surface {
    /// A new surface with object id `id`, nothing attached and no role.
    pub const fn new(id: u32) -> Self {
        Self {
            id,
            attach: Attach::Unchanged,
            damage: Rect::new(0, 0, 0, 0),
            pending_frames: [0; MAX_FRAMES],
            pending_frame_count: 0,
            frames: [0; MAX_FRAMES],
            frame_count: 0,
            last_done_ms: 0,
            ever_attached: false,
            xdg_surface: 0,
            toplevel: 0,
            initial_configure_sent: false,
            acked: false,
            last_serial: 0,
            configure_size: (0, 0),
            asked_size: (0, 0),
            window: None,
            title: [0; TITLE_CAPACITY],
            title_length: 0,
            app_id: [0; TITLE_CAPACITY],
            app_id_length: 0,
            minimum: (0, 0),
            maximum: (0, 0),
            wants_maximized: false,
            maximized: false,
            minimised: false,
            activated: false,
            role: Role::None,
            parent: None,
            position: (0, 0),
            pending_position: None,
            synced: true,
            cached: None,
            held: None,
            order: 0,
            popup: 0,
            grabbed: false,
            dismissed: false,
            popup_size: (0, 0),
            geometry: None,
            decoration: 0,
            viewport: 0,
            viewport_pending: ViewportState {
                source: None,
                destination: None,
            },
            viewport_current: ViewportState {
                source: None,
                destination: None,
            },
        }
    }

    /// Forgets the toplevel or popup role's state, as when the toplevel or popup is destroyed;
    /// what it holds and its viewport stay.
    pub fn reset_toplevel(&mut self) {
        let kept = *self;
        *self = Self::new(kept.id);
        self.xdg_surface = kept.xdg_surface;
        self.frames = kept.frames;
        self.frame_count = kept.frame_count;
        self.last_done_ms = kept.last_done_ms;
        self.held = kept.held;
        self.viewport = kept.viewport;
        self.viewport_pending = kept.viewport_pending;
        self.viewport_current = kept.viewport_current;
        self.ever_attached = true;
    }
}

/// The events didn't fit what the client has not read yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Overflow;

/// One client.
#[derive(Debug)]
pub struct ClientSlot {
    pub socket: Fd,
    pub device: Option<Fd>,
    pub input: [u8; INPUT_CAPACITY],
    pub input_length: usize,
    pub output: [u8; OUTPUT_CAPACITY],
    pub output_length: usize,
    pub descriptors_in: [Fd; MAX_IN_DESCRIPTORS],
    pub descriptors_in_start: usize,
    pub descriptors_in_count: usize,
    pub descriptors_out: [Fd; MAX_OUT_DESCRIPTORS],
    pub descriptors_out_count: usize,
    pub objects: [Object; MAX_OBJECTS],
    pub pools: [Option<Pool>; MAX_POOLS],
    pub buffers: [Option<Buffer>; MAX_BUFFERS],
    pub surfaces: [Option<Surface>; MAX_SURFACES],
    pub positioners: [Option<Positioner>; MAX_POSITIONERS],
    pub pool_bytes: usize,
    /// It bound `wl_subcompositor` or `wp_viewporter`, or made a positioner: its windows are
    /// drawn from the buffers they hold, so subsurfaces and popups can be drawn over them.
    pub composes: bool,
    /// The last order given to a subsurface placed on top.
    pub next_order: i32,
    pub pointer_focus: Option<u16>,
    pub keyboard_focus: Option<u16>,
    pub held_press: Option<(u32, u16)>,
    pub pending_motion: Option<(u16, u32, i32, i32)>,
    pub pointer_at: (i32, i32),
    pub modifiers: (u32, u32),
    /// A protocol error is queued: the client goes once it is sent.
    pub failed: bool,
    /// The last send could not take everything: wait for the socket to drain.
    pub blocked: bool,
}

impl ClientSlot {
    /// A new client on `socket`, with only `wl_display` (id 1).
    pub fn new(socket: Fd) -> Self {
        let mut objects = [Object::default(); MAX_OBJECTS];
        objects[1] = Object {
            interface: Some(Interface::Display),
            version: 1,
            index: 0,
            inert: false,
        };
        Self {
            socket,
            device: None,
            input: [0; INPUT_CAPACITY],
            input_length: 0,
            output: [0; OUTPUT_CAPACITY],
            output_length: 0,
            descriptors_in: [-1; MAX_IN_DESCRIPTORS],
            descriptors_in_start: 0,
            descriptors_in_count: 0,
            descriptors_out: [-1; MAX_OUT_DESCRIPTORS],
            descriptors_out_count: 0,
            objects,
            pools: [None; MAX_POOLS],
            buffers: [None; MAX_BUFFERS],
            surfaces: [None; MAX_SURFACES],
            positioners: [None; MAX_POSITIONERS],
            pool_bytes: 0,
            composes: false,
            next_order: 0,
            pointer_focus: None,
            keyboard_focus: None,
            held_press: None,
            pending_motion: None,
            pointer_at: (0, 0),
            modifiers: (0, 0),
            failed: false,
            blocked: false,
        }
    }

    /// Object `id`, when it exists.
    pub fn object(&self, id: u32) -> Option<Object> {
        self.objects
            .get(id as usize)
            .copied()
            .filter(|object| object.interface.is_some())
    }

    /// Whether `id` is free for a new object of the client's.
    pub fn is_free_id(&self, id: u32) -> bool {
        id != 0 && (id as usize) < MAX_OBJECTS && self.object(id).is_none()
    }

    /// Makes `id` an object of `interface` at `version`, standing for `index`.
    pub fn create(&mut self, id: u32, interface: Interface, version: u32, index: u16) {
        self.objects[id as usize] = Object {
            interface: Some(interface),
            version,
            index,
            inert: false,
        };
    }

    /// Frees `id`.
    pub fn forget(&mut self, id: u32) {
        if let Some(object) = self.objects.get_mut(id as usize) {
            *object = Object::default();
        }
    }

    /// The ids of the objects of `interface`.
    pub fn ids_of(&self, interface: Interface) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.objects
            .iter()
            .enumerate()
            .filter(move |(_, object)| object.interface == Some(interface) && !object.inert)
            .map(|(id, object)| (id as u32, object.version))
    }

    /// The room events may use: all of it but the error's reserve.
    pub fn output_room(&self) -> usize {
        (OUTPUT_CAPACITY - ERROR_RESERVE).saturating_sub(self.output_length)
    }

    /// Queues `event` of object `object` bound at `version` with `values`; nothing when the
    /// object's version is older than the event.
    pub fn queue(
        &mut self,
        object: u32,
        version: u32,
        event: Event,
        values: &[Value],
    ) -> Result<(), Overflow> {
        if event.since > version || self.failed {
            return Ok(());
        }
        let end = OUTPUT_CAPACITY - ERROR_RESERVE;
        let room = self
            .output
            .get_mut(self.output_length..end)
            .ok_or(Overflow)?;
        let size = wire::encode(room, object, event.opcode, values).map_err(|_| Overflow)?;
        self.output_length += size;
        Ok(())
    }

    /// Queues `event` and marks the client failed if it does not fit: it is disconnected.
    pub fn post(&mut self, object: u32, version: u32, event: Event, values: &[Value]) {
        if self.queue(object, version, event, values).is_err() {
            self.failed = true;
        }
    }

    /// Queues `wl_display.error` about `object` with `code` and `message`, in the reserve if
    /// need be, and marks the client failed. Only the first error is sent.
    pub fn protocol_error(&mut self, object: u32, code: u32, message: &[u8]) {
        if self.failed {
            return;
        }
        let room = &mut self.output[self.output_length..];
        if let Ok(size) = wire::encode(
            room,
            1,
            display::ERROR.opcode,
            &[
                Value::Object(object),
                Value::Uint(code),
                Value::String(message),
            ],
        ) {
            self.output_length += size;
        }
        self.failed = true;
    }

    /// Takes the oldest received descriptor.
    pub fn take_descriptor(&mut self) -> Option<Fd> {
        if self.descriptors_in_count == 0 {
            return None;
        }
        let descriptor = self.descriptors_in[self.descriptors_in_start];
        self.descriptors_in_start = (self.descriptors_in_start + 1) % MAX_IN_DESCRIPTORS;
        self.descriptors_in_count -= 1;
        Some(descriptor)
    }

    /// Queues a received descriptor; `false` when the queue is full.
    pub fn push_descriptor(&mut self, descriptor: Fd) -> bool {
        if self.descriptors_in_count == MAX_IN_DESCRIPTORS {
            return false;
        }
        let at = (self.descriptors_in_start + self.descriptors_in_count) % MAX_IN_DESCRIPTORS;
        self.descriptors_in[at] = descriptor;
        self.descriptors_in_count += 1;
        true
    }

    /// The surface `index`, when it exists.
    pub fn surface(&mut self, index: u16) -> Option<&mut Surface> {
        self.surfaces.get_mut(usize::from(index))?.as_mut()
    }
}
