//! The interfaces EggOS's Wayland server serves, with their requests' signatures and their
//! events' opcodes, errors and enums, from `wayland.xml`, `xdg-shell.xml`,
//! `xdg-decoration-unstable-v1.xml`, `cursor-shape-v1.xml`, `viewporter.xml`,
//! `primary-selection-unstable-v1.xml`, `xdg-activation-v1.xml`, `text-input-unstable-v3.xml`
//! and KDE's `server-decoration.xml`. Request tables are indexed by opcode; a request whose
//! `since` is above the object's version is no request of that object's.

use crate::wire::Arg::{
    self, Array, Fd, Fixed, Int, NewId, NullableObject, NullableString, Object, String, Uint,
};

/// The interfaces the server knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interface {
    Display,
    Registry,
    Callback,
    Compositor,
    Region,
    Shm,
    ShmPool,
    Buffer,
    Surface,
    Seat,
    Pointer,
    Keyboard,
    Touch,
    Output,
    XdgWmBase,
    XdgPositioner,
    XdgSurface,
    XdgToplevel,
    XdgPopup,
    Subcompositor,
    Subsurface,
    DecorationManager,
    ToplevelDecoration,
    CursorShapeManager,
    CursorShapeDevice,
    Viewporter,
    Viewport,
    DataDeviceManager,
    DataSource,
    DataDevice,
    DataOffer,
    PrimarySelectionDeviceManager,
    PrimarySelectionDevice,
    PrimarySelectionOffer,
    PrimarySelectionSource,
    Activation,
    ActivationToken,
    TextInputManager,
    TextInput,
    ServerDecorationManager,
    ServerDecoration,
}

impl Interface {
    /// Its name on the wire.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Display => "wl_display",
            Self::Registry => "wl_registry",
            Self::Callback => "wl_callback",
            Self::Compositor => "wl_compositor",
            Self::Region => "wl_region",
            Self::Shm => "wl_shm",
            Self::ShmPool => "wl_shm_pool",
            Self::Buffer => "wl_buffer",
            Self::Surface => "wl_surface",
            Self::Seat => "wl_seat",
            Self::Pointer => "wl_pointer",
            Self::Keyboard => "wl_keyboard",
            Self::Touch => "wl_touch",
            Self::Output => "wl_output",
            Self::XdgWmBase => "xdg_wm_base",
            Self::XdgPositioner => "xdg_positioner",
            Self::XdgSurface => "xdg_surface",
            Self::XdgToplevel => "xdg_toplevel",
            Self::XdgPopup => "xdg_popup",
            Self::Subcompositor => "wl_subcompositor",
            Self::Subsurface => "wl_subsurface",
            Self::DecorationManager => "zxdg_decoration_manager_v1",
            Self::ToplevelDecoration => "zxdg_toplevel_decoration_v1",
            Self::CursorShapeManager => "wp_cursor_shape_manager_v1",
            Self::CursorShapeDevice => "wp_cursor_shape_device_v1",
            Self::Viewporter => "wp_viewporter",
            Self::Viewport => "wp_viewport",
            Self::DataDeviceManager => "wl_data_device_manager",
            Self::DataSource => "wl_data_source",
            Self::DataDevice => "wl_data_device",
            Self::DataOffer => "wl_data_offer",
            Self::PrimarySelectionDeviceManager => "zwp_primary_selection_device_manager_v1",
            Self::PrimarySelectionDevice => "zwp_primary_selection_device_v1",
            Self::PrimarySelectionOffer => "zwp_primary_selection_offer_v1",
            Self::PrimarySelectionSource => "zwp_primary_selection_source_v1",
            Self::Activation => "xdg_activation_v1",
            Self::ActivationToken => "xdg_activation_token_v1",
            Self::TextInputManager => "zwp_text_input_manager_v3",
            Self::TextInput => "zwp_text_input_v3",
            Self::ServerDecorationManager => "org_kde_kwin_server_decoration_manager",
            Self::ServerDecoration => "org_kde_kwin_server_decoration",
        }
    }
}

/// One request: its name, the version it came in, its arguments, and whether it destroys its
/// object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request {
    pub name: &'static str,
    pub since: u32,
    pub signature: &'static [Arg],
    pub destructor: bool,
}

const fn request(name: &'static str, since: u32, signature: &'static [Arg]) -> Request {
    Request {
        name,
        since,
        signature,
        destructor: false,
    }
}

const fn destructor(name: &'static str, since: u32) -> Request {
    Request {
        name,
        since,
        signature: &[],
        destructor: true,
    }
}

const RECT: &[Arg] = &[Int, Int, Int, Int];

const DISPLAY: &[Request] = &[
    request("sync", 1, &[NewId]),
    request("get_registry", 1, &[NewId]),
];
const REGISTRY: &[Request] = &[request("bind", 1, &[Uint, Arg::UntypedNewId])];
const COMPOSITOR: &[Request] = &[
    request("create_surface", 1, &[NewId]),
    request("create_region", 1, &[NewId]),
];
const REGION: &[Request] = &[
    destructor("destroy", 1),
    request("add", 1, RECT),
    request("subtract", 1, RECT),
];
const SHM: &[Request] = &[
    request("create_pool", 1, &[NewId, Fd, Int]),
    destructor("release", 2),
];
const SHM_POOL: &[Request] = &[
    request("create_buffer", 1, &[NewId, Int, Int, Int, Int, Uint]),
    destructor("destroy", 1),
    request("resize", 1, &[Int]),
];
const BUFFER: &[Request] = &[destructor("destroy", 1)];
const SURFACE: &[Request] = &[
    destructor("destroy", 1),
    request("attach", 1, &[NullableObject, Int, Int]),
    request("damage", 1, RECT),
    request("frame", 1, &[NewId]),
    request("set_opaque_region", 1, &[NullableObject]),
    request("set_input_region", 1, &[NullableObject]),
    request("commit", 1, &[]),
    request("set_buffer_transform", 2, &[Int]),
    request("set_buffer_scale", 3, &[Int]),
    request("damage_buffer", 4, RECT),
    request("offset", 5, &[Int, Int]),
];
const SEAT: &[Request] = &[
    request("get_pointer", 1, &[NewId]),
    request("get_keyboard", 1, &[NewId]),
    request("get_touch", 1, &[NewId]),
    destructor("release", 5),
];
const POINTER: &[Request] = &[
    request("set_cursor", 1, &[Uint, NullableObject, Int, Int]),
    destructor("release", 3),
];
const KEYBOARD: &[Request] = &[destructor("release", 3)];
const TOUCH: &[Request] = &[destructor("release", 3)];
const OUTPUT: &[Request] = &[destructor("release", 3)];
const XDG_WM_BASE: &[Request] = &[
    destructor("destroy", 1),
    request("create_positioner", 1, &[NewId]),
    request("get_xdg_surface", 1, &[NewId, Object]),
    request("pong", 1, &[Uint]),
];
const XDG_POSITIONER: &[Request] = &[
    destructor("destroy", 1),
    request("set_size", 1, &[Int, Int]),
    request("set_anchor_rect", 1, RECT),
    request("set_anchor", 1, &[Uint]),
    request("set_gravity", 1, &[Uint]),
    request("set_constraint_adjustment", 1, &[Uint]),
    request("set_offset", 1, &[Int, Int]),
    request("set_reactive", 3, &[]),
    request("set_parent_size", 3, &[Int, Int]),
    request("set_parent_configure", 3, &[Uint]),
];
const XDG_SURFACE: &[Request] = &[
    destructor("destroy", 1),
    request("get_toplevel", 1, &[NewId]),
    request("get_popup", 1, &[NewId, NullableObject, Object]),
    request("set_window_geometry", 1, RECT),
    request("ack_configure", 1, &[Uint]),
];
const XDG_TOPLEVEL: &[Request] = &[
    destructor("destroy", 1),
    request("set_parent", 1, &[NullableObject]),
    request("set_title", 1, &[String]),
    request("set_app_id", 1, &[String]),
    request("show_window_menu", 1, &[Object, Uint, Int, Int]),
    request("move", 1, &[Object, Uint]),
    request("resize", 1, &[Object, Uint, Uint]),
    request("set_max_size", 1, &[Int, Int]),
    request("set_min_size", 1, &[Int, Int]),
    request("set_maximized", 1, &[]),
    request("unset_maximized", 1, &[]),
    request("set_fullscreen", 1, &[NullableObject]),
    request("unset_fullscreen", 1, &[]),
    request("set_minimized", 1, &[]),
];
const XDG_POPUP: &[Request] = &[
    destructor("destroy", 1),
    request("grab", 1, &[Object, Uint]),
    request("reposition", 3, &[Object, Uint]),
];
const SUBCOMPOSITOR: &[Request] = &[
    destructor("destroy", 1),
    request("get_subsurface", 1, &[NewId, Object, Object]),
];
const SUBSURFACE: &[Request] = &[
    destructor("destroy", 1),
    request("set_position", 1, &[Int, Int]),
    request("place_above", 1, &[Object]),
    request("place_below", 1, &[Object]),
    request("set_sync", 1, &[]),
    request("set_desync", 1, &[]),
];
const DECORATION_MANAGER: &[Request] = &[
    destructor("destroy", 1),
    request("get_toplevel_decoration", 1, &[NewId, Object]),
];
const TOPLEVEL_DECORATION: &[Request] = &[
    destructor("destroy", 1),
    request("set_mode", 1, &[Uint]),
    request("unset_mode", 1, &[]),
];
const CURSOR_SHAPE_MANAGER: &[Request] = &[
    destructor("destroy", 1),
    request("get_pointer", 1, &[NewId, Object]),
    request("get_tablet_tool_v2", 1, &[NewId, Object]),
];
const CURSOR_SHAPE_DEVICE: &[Request] = &[
    destructor("destroy", 1),
    request("set_shape", 1, &[Uint, Uint]),
];
const VIEWPORTER: &[Request] = &[
    destructor("destroy", 1),
    request("get_viewport", 1, &[NewId, Object]),
];
const VIEWPORT: &[Request] = &[
    destructor("destroy", 1),
    request("set_source", 1, &[Fixed, Fixed, Fixed, Fixed]),
    request("set_destination", 1, &[Int, Int]),
];
const DATA_DEVICE_MANAGER: &[Request] = &[
    request("create_data_source", 1, &[NewId]),
    request("get_data_device", 1, &[NewId, Object]),
];
const DATA_SOURCE: &[Request] = &[
    request("offer", 1, &[String]),
    destructor("destroy", 1),
    request("set_actions", 3, &[Uint]),
];
const DATA_DEVICE: &[Request] = &[
    request(
        "start_drag",
        1,
        &[NullableObject, Object, NullableObject, Uint],
    ),
    request("set_selection", 1, &[NullableObject, Uint]),
    destructor("release", 2),
];
const DATA_OFFER: &[Request] = &[
    request("accept", 1, &[Uint, NullableString]),
    request("receive", 1, &[String, Fd]),
    destructor("destroy", 1),
    request("finish", 3, &[]),
    request("set_actions", 3, &[Uint, Uint]),
];
const PRIMARY_SELECTION_DEVICE_MANAGER: &[Request] = &[
    request("create_source", 1, &[NewId]),
    request("get_device", 1, &[NewId, Object]),
    destructor("destroy", 1),
];
const PRIMARY_SELECTION_DEVICE: &[Request] = &[
    request("set_selection", 1, &[NullableObject, Uint]),
    destructor("destroy", 1),
];
const PRIMARY_SELECTION_OFFER: &[Request] = &[
    request("receive", 1, &[String, Fd]),
    destructor("destroy", 1),
];
const PRIMARY_SELECTION_SOURCE: &[Request] =
    &[request("offer", 1, &[String]), destructor("destroy", 1)];
const ACTIVATION: &[Request] = &[
    destructor("destroy", 1),
    request("get_activation_token", 1, &[NewId]),
    request("activate", 1, &[String, Object]),
];
const ACTIVATION_TOKEN: &[Request] = &[
    request("set_serial", 1, &[Uint, Object]),
    request("set_app_id", 1, &[String]),
    request("set_surface", 1, &[Object]),
    request("commit", 1, &[]),
    destructor("destroy", 1),
];
const TEXT_INPUT_MANAGER: &[Request] = &[
    destructor("destroy", 1),
    request("get_text_input", 1, &[NewId, Object]),
];
const TEXT_INPUT: &[Request] = &[
    destructor("destroy", 1),
    request("enable", 1, &[]),
    request("disable", 1, &[]),
    request("set_surrounding_text", 1, &[String, Int, Int]),
    request("set_text_change_cause", 1, &[Uint]),
    request("set_content_type", 1, &[Uint, Uint]),
    request("set_cursor_rectangle", 1, RECT),
    request("commit", 1, &[]),
];
const SERVER_DECORATION_MANAGER: &[Request] = &[request("create", 1, &[NewId, Object])];
const SERVER_DECORATION: &[Request] = &[
    destructor("release", 1),
    request("request_mode", 1, &[Uint]),
];

/// `interface`'s requests, indexed by opcode.
pub const fn requests(interface: Interface) -> &'static [Request] {
    match interface {
        Interface::Display => DISPLAY,
        Interface::Registry => REGISTRY,
        Interface::Callback => &[],
        Interface::Compositor => COMPOSITOR,
        Interface::Region => REGION,
        Interface::Shm => SHM,
        Interface::ShmPool => SHM_POOL,
        Interface::Buffer => BUFFER,
        Interface::Surface => SURFACE,
        Interface::Seat => SEAT,
        Interface::Pointer => POINTER,
        Interface::Keyboard => KEYBOARD,
        Interface::Touch => TOUCH,
        Interface::Output => OUTPUT,
        Interface::XdgWmBase => XDG_WM_BASE,
        Interface::XdgPositioner => XDG_POSITIONER,
        Interface::XdgSurface => XDG_SURFACE,
        Interface::XdgToplevel => XDG_TOPLEVEL,
        Interface::XdgPopup => XDG_POPUP,
        Interface::Subcompositor => SUBCOMPOSITOR,
        Interface::Subsurface => SUBSURFACE,
        Interface::DecorationManager => DECORATION_MANAGER,
        Interface::ToplevelDecoration => TOPLEVEL_DECORATION,
        Interface::CursorShapeManager => CURSOR_SHAPE_MANAGER,
        Interface::CursorShapeDevice => CURSOR_SHAPE_DEVICE,
        Interface::Viewporter => VIEWPORTER,
        Interface::Viewport => VIEWPORT,
        Interface::DataDeviceManager => DATA_DEVICE_MANAGER,
        Interface::DataSource => DATA_SOURCE,
        Interface::DataDevice => DATA_DEVICE,
        Interface::DataOffer => DATA_OFFER,
        Interface::PrimarySelectionDeviceManager => PRIMARY_SELECTION_DEVICE_MANAGER,
        Interface::PrimarySelectionDevice => PRIMARY_SELECTION_DEVICE,
        Interface::PrimarySelectionOffer => PRIMARY_SELECTION_OFFER,
        Interface::PrimarySelectionSource => PRIMARY_SELECTION_SOURCE,
        Interface::Activation => ACTIVATION,
        Interface::ActivationToken => ACTIVATION_TOKEN,
        Interface::TextInputManager => TEXT_INPUT_MANAGER,
        Interface::TextInput => TEXT_INPUT,
        Interface::ServerDecorationManager => SERVER_DECORATION_MANAGER,
        Interface::ServerDecoration => SERVER_DECORATION,
    }
}

/// Request `opcode` of an object of `interface` bound at `version`; `None` when it has none.
pub fn request_of(interface: Interface, version: u32, opcode: u16) -> Option<&'static Request> {
    requests(interface)
        .get(usize::from(opcode))
        .filter(|request| request.since <= version)
}

/// A global the registry announces: its name (number), interface and the version we serve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Global {
    pub name: u32,
    pub interface: Interface,
    pub version: u32,
}

/// Every global, in the order the registry announces them.
pub const GLOBALS: [Global; 14] = [
    Global {
        name: 1,
        interface: Interface::Compositor,
        version: 4,
    },
    Global {
        name: 2,
        interface: Interface::Shm,
        version: 1,
    },
    Global {
        name: 3,
        interface: Interface::Seat,
        version: 5,
    },
    Global {
        name: 4,
        interface: Interface::Output,
        version: 3,
    },
    Global {
        name: 5,
        interface: Interface::XdgWmBase,
        version: 3,
    },
    Global {
        name: 6,
        interface: Interface::Subcompositor,
        version: 1,
    },
    Global {
        name: 7,
        interface: Interface::DecorationManager,
        version: 1,
    },
    Global {
        name: 8,
        interface: Interface::CursorShapeManager,
        version: 1,
    },
    Global {
        name: 9,
        interface: Interface::Viewporter,
        version: 1,
    },
    Global {
        name: 10,
        interface: Interface::DataDeviceManager,
        version: 3,
    },
    Global {
        name: 11,
        interface: Interface::PrimarySelectionDeviceManager,
        version: 1,
    },
    Global {
        name: 12,
        interface: Interface::Activation,
        version: 1,
    },
    Global {
        name: 13,
        interface: Interface::TextInputManager,
        version: 1,
    },
    Global {
        name: 14,
        interface: Interface::ServerDecorationManager,
        version: 1,
    },
];

/// The global named `name`.
pub fn global(name: u32) -> Option<Global> {
    GLOBALS.into_iter().find(|global| global.name == name)
}

/// The first id of the server's range; clients' ids are below it.
pub const SERVER_ID_START: u32 = 0xff00_0000;

/// An event: its opcode and the version it came in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event {
    pub opcode: u16,
    pub since: u32,
}

const fn event(opcode: u16, since: u32) -> Event {
    Event { opcode, since }
}

/// `wl_display`'s events and errors.
pub mod display {
    use super::{Event, event};
    pub const ERROR: Event = event(0, 1);
    pub const DELETE_ID: Event = event(1, 1);
    pub const INVALID_OBJECT: u32 = 0;
    pub const INVALID_METHOD: u32 = 1;
    pub const NO_MEMORY: u32 = 2;
    pub const IMPLEMENTATION: u32 = 3;
}

/// `wl_registry`'s events.
pub mod registry {
    use super::{Event, event};
    pub const GLOBAL: Event = event(0, 1);
}

/// `wl_callback`'s events.
pub mod callback {
    use super::{Event, event};
    pub const DONE: Event = event(0, 1);
}

/// `wl_shm`'s events, errors and formats.
pub mod shm {
    use super::{Event, event};
    pub const FORMAT: Event = event(0, 1);
    pub const INVALID_FORMAT: u32 = 0;
    pub const INVALID_STRIDE: u32 = 1;
    pub const INVALID_FD: u32 = 2;
    pub const ARGB8888: u32 = 0;
    pub const XRGB8888: u32 = 1;
}

/// `wl_buffer`'s events.
pub mod buffer {
    use super::{Event, event};
    pub const RELEASE: Event = event(0, 1);
}

/// `wl_surface`'s events and errors.
pub mod surface {
    use super::{Event, event};
    pub const ENTER: Event = event(0, 1);
    pub const LEAVE: Event = event(1, 1);
    pub const INVALID_SCALE: u32 = 0;
    pub const INVALID_TRANSFORM: u32 = 1;
    pub const INVALID_SIZE: u32 = 2;
    pub const INVALID_OFFSET: u32 = 3;
    pub const DEFUNCT_ROLE_OBJECT: u32 = 4;
    /// `wl_output.transform`'s largest value (`flipped_270`).
    pub const LARGEST_TRANSFORM: i32 = 7;
}

/// `wl_seat`'s events, capabilities and errors.
pub mod seat {
    use super::{Event, event};
    pub const CAPABILITIES: Event = event(0, 1);
    pub const NAME: Event = event(1, 2);
    pub const POINTER: u32 = 1;
    pub const KEYBOARD: u32 = 2;
    pub const MISSING_CAPABILITY: u32 = 0;
}

/// `wl_pointer`'s events and enums.
pub mod pointer {
    use super::{Event, event};
    pub const ENTER: Event = event(0, 1);
    pub const LEAVE: Event = event(1, 1);
    pub const MOTION: Event = event(2, 1);
    pub const BUTTON: Event = event(3, 1);
    pub const AXIS: Event = event(4, 1);
    pub const FRAME: Event = event(5, 5);
    pub const AXIS_DISCRETE: Event = event(8, 5);
    pub const RELEASED: u32 = 0;
    pub const PRESSED: u32 = 1;
    pub const VERTICAL_SCROLL: u32 = 0;
    pub const HORIZONTAL_SCROLL: u32 = 1;
    pub const ROLE: u32 = 0;
}

/// `wl_keyboard`'s events and enums.
pub mod keyboard {
    use super::{Event, event};
    pub const KEYMAP: Event = event(0, 1);
    pub const ENTER: Event = event(1, 1);
    pub const LEAVE: Event = event(2, 1);
    pub const KEY: Event = event(3, 1);
    pub const MODIFIERS: Event = event(4, 1);
    pub const REPEAT_INFO: Event = event(5, 4);
    pub const XKB_V1: u32 = 1;
    pub const RELEASED: u32 = 0;
    pub const PRESSED: u32 = 1;
}

/// `wl_output`'s events and enums.
pub mod output {
    use super::{Event, event};
    pub const GEOMETRY: Event = event(0, 1);
    pub const MODE: Event = event(1, 1);
    pub const DONE: Event = event(2, 2);
    pub const SCALE: Event = event(3, 2);
    pub const SUBPIXEL_UNKNOWN: i32 = 0;
    pub const TRANSFORM_NORMAL: i32 = 0;
    pub const MODE_CURRENT: u32 = 1;
    pub const MODE_PREFERRED: u32 = 2;
}

/// `xdg_wm_base`'s events and errors.
pub mod xdg_wm_base {
    use super::{Event, event};
    pub const PING: Event = event(0, 1);
    pub const ROLE: u32 = 0;
    pub const DEFUNCT_SURFACES: u32 = 1;
    pub const NOT_THE_TOPMOST_POPUP: u32 = 2;
    pub const INVALID_POPUP_PARENT: u32 = 3;
    pub const INVALID_SURFACE_STATE: u32 = 4;
    pub const INVALID_POSITIONER: u32 = 5;
}

/// `xdg_surface`'s events and errors.
pub mod xdg_surface {
    use super::{Event, event};
    pub const CONFIGURE: Event = event(0, 1);
    pub const NOT_CONSTRUCTED: u32 = 1;
    pub const ALREADY_CONSTRUCTED: u32 = 2;
    pub const UNCONFIGURED_BUFFER: u32 = 3;
    pub const INVALID_SERIAL: u32 = 4;
    pub const INVALID_SIZE: u32 = 5;
    pub const DEFUNCT_ROLE_OBJECT: u32 = 6;
}

/// `xdg_toplevel`'s events, errors, states and resize edges.
pub mod xdg_toplevel {
    use super::{Event, event};
    pub const CONFIGURE: Event = event(0, 1);
    pub const CLOSE: Event = event(1, 1);
    pub const INVALID_RESIZE_EDGE: u32 = 0;
    pub const INVALID_PARENT: u32 = 1;
    pub const INVALID_SIZE: u32 = 2;
    pub const STATE_MAXIMIZED: u32 = 1;
    pub const STATE_FULLSCREEN: u32 = 2;
    pub const STATE_RESIZING: u32 = 3;
    pub const STATE_ACTIVATED: u32 = 4;
    pub const EDGE_NONE: u32 = 0;
    pub const EDGE_TOP: u32 = 1;
    pub const EDGE_BOTTOM: u32 = 2;
    pub const EDGE_LEFT: u32 = 4;
    pub const EDGE_TOP_LEFT: u32 = 5;
    pub const EDGE_BOTTOM_LEFT: u32 = 6;
    pub const EDGE_RIGHT: u32 = 8;
    pub const EDGE_TOP_RIGHT: u32 = 9;
    pub const EDGE_BOTTOM_RIGHT: u32 = 10;
}

/// `xdg_popup`'s events and errors.
pub mod xdg_popup {
    use super::{Event, event};
    pub const CONFIGURE: Event = event(0, 1);
    pub const POPUP_DONE: Event = event(1, 1);
    pub const REPOSITIONED: Event = event(2, 3);
    pub const INVALID_GRAB: u32 = 0;
}

/// `xdg_positioner`'s errors and enums: anchors and gravities share their numbers.
pub mod xdg_positioner {
    pub const INVALID_INPUT: u32 = 0;
    pub const NONE: u32 = 0;
    pub const TOP: u32 = 1;
    pub const BOTTOM: u32 = 2;
    pub const LEFT: u32 = 3;
    pub const RIGHT: u32 = 4;
    pub const TOP_LEFT: u32 = 5;
    pub const BOTTOM_LEFT: u32 = 6;
    pub const TOP_RIGHT: u32 = 7;
    pub const BOTTOM_RIGHT: u32 = 8;
    pub const SLIDE_X: u32 = 1;
    pub const SLIDE_Y: u32 = 2;
    pub const FLIP_X: u32 = 4;
    pub const FLIP_Y: u32 = 8;
    pub const RESIZE_X: u32 = 16;
    pub const RESIZE_Y: u32 = 32;
}

/// `wl_subcompositor`'s and `wl_subsurface`'s errors.
pub mod subsurface {
    pub const BAD_SURFACE: u32 = 0;
    pub const BAD_PARENT: u32 = 1;
}

/// `zxdg_toplevel_decoration_v1`'s events, errors and modes.
pub mod decoration {
    use super::{Event, event};
    pub const CONFIGURE: Event = event(0, 1);
    pub const UNCONFIGURED_BUFFER: u32 = 0;
    pub const ALREADY_CONSTRUCTED: u32 = 1;
    pub const ORPHANED: u32 = 2;
    pub const CLIENT_SIDE: u32 = 1;
    pub const SERVER_SIDE: u32 = 2;
}

/// `wp_cursor_shape_device_v1`'s errors and shapes (version 1's, 1 to 34).
pub mod cursor_shape {
    pub const INVALID_SHAPE: u32 = 1;
    pub const DEFAULT: u32 = 1;
    pub const TEXT: u32 = 9;
    pub const VERTICAL_TEXT: u32 = 10;
    pub const E_RESIZE: u32 = 18;
    pub const N_RESIZE: u32 = 19;
    pub const NE_RESIZE: u32 = 20;
    pub const NW_RESIZE: u32 = 21;
    pub const S_RESIZE: u32 = 22;
    pub const SE_RESIZE: u32 = 23;
    pub const SW_RESIZE: u32 = 24;
    pub const W_RESIZE: u32 = 25;
    pub const EW_RESIZE: u32 = 26;
    pub const NS_RESIZE: u32 = 27;
    pub const NESW_RESIZE: u32 = 28;
    pub const NWSE_RESIZE: u32 = 29;
    pub const COL_RESIZE: u32 = 30;
    pub const ROW_RESIZE: u32 = 31;
    pub const LAST: u32 = 34;
}

/// `wp_viewporter`'s and `wp_viewport`'s errors.
pub mod viewport {
    pub const VIEWPORT_EXISTS: u32 = 0;
    pub const BAD_VALUE: u32 = 0;
    pub const BAD_SIZE: u32 = 1;
    pub const OUT_OF_BUFFER: u32 = 2;
    pub const NO_SURFACE: u32 = 3;
}

/// `wl_data_source`'s events and errors.
pub mod data_source {
    use super::{Event, event};
    pub const TARGET: Event = event(0, 1);
    pub const SEND: Event = event(1, 1);
    pub const CANCELLED: Event = event(2, 1);
    pub const INVALID_ACTION_MASK: u32 = 0;
    pub const INVALID_SOURCE: u32 = 1;
}

/// `wl_data_device`'s events and errors.
pub mod data_device {
    use super::{Event, event};
    pub const DATA_OFFER: Event = event(0, 1);
    pub const SELECTION: Event = event(5, 1);
    pub const ROLE: u32 = 0;
    pub const USED_SOURCE: u32 = 1;
}

/// `wl_data_offer`'s events and errors.
pub mod data_offer {
    use super::{Event, event};
    pub const OFFER: Event = event(0, 1);
    pub const INVALID_FINISH: u32 = 0;
    pub const INVALID_OFFER: u32 = 3;
}

/// `zwp_primary_selection_device_v1`'s events.
pub mod primary_selection_device {
    use super::{Event, event};
    pub const DATA_OFFER: Event = event(0, 1);
    pub const SELECTION: Event = event(1, 1);
}

/// `zwp_primary_selection_offer_v1`'s events.
pub mod primary_selection_offer {
    use super::{Event, event};
    pub const OFFER: Event = event(0, 1);
}

/// `zwp_primary_selection_source_v1`'s events.
pub mod primary_selection_source {
    use super::{Event, event};
    pub const SEND: Event = event(0, 1);
    pub const CANCELLED: Event = event(1, 1);
}

/// `xdg_activation_token_v1`'s events and errors.
pub mod activation_token {
    use super::{Event, event};
    pub const DONE: Event = event(0, 1);
    pub const ALREADY_USED: u32 = 0;
}

/// `zwp_text_input_v3`'s events and enums.
pub mod text_input {
    use super::{Event, event};
    pub const ENTER: Event = event(0, 1);
    pub const LEAVE: Event = event(1, 1);
    pub const PREEDIT_STRING: Event = event(2, 1);
    pub const COMMIT_STRING: Event = event(3, 1);
    pub const DELETE_SURROUNDING_TEXT: Event = event(4, 1);
    pub const DONE: Event = event(5, 1);
    /// `content_hint`: the text is hidden as typed (a password field's dots).
    pub const HINT_HIDDEN_TEXT: u32 = 0x40;
    /// `content_hint`: the text is sensitive and must not be kept or shown elsewhere.
    pub const HINT_SENSITIVE_DATA: u32 = 0x80;
    /// `content_hint`: the field takes several lines.
    pub const HINT_MULTILINE: u32 = 0x200;
    /// `content_purpose`: a password.
    pub const PURPOSE_PASSWORD: u32 = 8;
    /// `content_purpose`: a PIN.
    pub const PURPOSE_PIN: u32 = 9;
    /// Every `content_purpose`'s name, by its number.
    pub const PURPOSES: [&str; 14] = [
        "normal", "alpha", "digits", "number", "phone", "url", "email", "name", "password", "pin",
        "date", "time", "datetime", "terminal",
    ];
}

/// `org_kde_kwin_server_decoration_manager`'s and `org_kde_kwin_server_decoration`'s events
/// and modes.
pub mod server_decoration {
    use super::{Event, event};
    pub const DEFAULT_MODE: Event = event(0, 1);
    pub const MODE: Event = event(0, 1);
    pub const NONE: u32 = 0;
    pub const CLIENT: u32 = 1;
    pub const SERVER: u32 = 2;
}

/// The signatures of the events the test client decodes, by the interface and opcode.
pub const fn event_signature(interface: Interface, opcode: u16) -> Option<&'static [Arg]> {
    Some(match (interface, opcode) {
        (Interface::Display, 0) => &[Object, Uint, String],
        (Interface::Display, 1) => &[Uint],
        (Interface::Registry, 0) => &[Uint, String, Uint],
        (Interface::Registry, 1) => &[Uint],
        (Interface::Callback, 0) => &[Uint],
        (Interface::Shm, 0) => &[Uint],
        (Interface::Buffer, 0) => &[],
        (Interface::Surface, 0 | 1) => &[Object],
        (Interface::Seat, 0) => &[Uint],
        (Interface::Seat, 1) => &[String],
        (Interface::Pointer, 0) => &[Uint, Object, Fixed, Fixed],
        (Interface::Pointer, 1) => &[Uint, Object],
        (Interface::Pointer, 2) => &[Uint, Fixed, Fixed],
        (Interface::Pointer, 3) => &[Uint, Uint, Uint, Uint],
        (Interface::Pointer, 4) => &[Uint, Uint, Fixed],
        (Interface::Pointer, 5) => &[],
        (Interface::Pointer, 8) => &[Uint, Int],
        (Interface::Keyboard, 0) => &[Uint, Fd, Uint],
        (Interface::Keyboard, 1) => &[Uint, Object, Array],
        (Interface::Keyboard, 2) => &[Uint, Object],
        (Interface::Keyboard, 3) => &[Uint, Uint, Uint, Uint],
        (Interface::Keyboard, 4) => &[Uint, Uint, Uint, Uint, Uint],
        (Interface::Keyboard, 5) => &[Int, Int],
        (Interface::Output, 0) => &[Int, Int, Int, Int, Int, String, String, Int],
        (Interface::Output, 1) => &[Uint, Int, Int, Int],
        (Interface::Output, 2) => &[],
        (Interface::Output, 3) => &[Int],
        (Interface::XdgWmBase, 0) => &[Uint],
        (Interface::XdgSurface, 0) => &[Uint],
        (Interface::XdgToplevel, 0) => &[Int, Int, Array],
        (Interface::XdgToplevel, 1) => &[],
        (Interface::XdgPopup, 0) => &[Int, Int, Int, Int],
        (Interface::XdgPopup, 1) => &[],
        (Interface::XdgPopup, 2) => &[Uint],
        (Interface::ToplevelDecoration, 0) => &[Uint],
        (Interface::DataSource, 0) => &[NullableString],
        (Interface::DataSource, 1) => &[String, Fd],
        (Interface::DataSource, 2) => &[],
        (Interface::DataDevice, 0) => &[NewId],
        (Interface::DataDevice, 5) => &[NullableObject],
        (Interface::DataOffer, 0) => &[String],
        (Interface::PrimarySelectionDevice, 0) => &[NewId],
        (Interface::PrimarySelectionDevice, 1) => &[NullableObject],
        (Interface::PrimarySelectionOffer, 0) => &[String],
        (Interface::PrimarySelectionSource, 0) => &[String, Fd],
        (Interface::PrimarySelectionSource, 1) => &[],
        (Interface::ActivationToken, 0) => &[String],
        (Interface::TextInput, 0 | 1) => &[Object],
        (Interface::TextInput, 2) => &[NullableString, Int, Int],
        (Interface::TextInput, 3) => &[NullableString],
        (Interface::TextInput, 4) => &[Uint, Uint],
        (Interface::TextInput, 5) => &[Uint],
        (Interface::ServerDecorationManager, 0) => &[Uint],
        (Interface::ServerDecoration, 0) => &[Uint],
        _ => return None,
    })
}
