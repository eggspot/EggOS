//! The `/dev/window` contract (D127, docs/windows.md): the limits, the `ioctl` request numbers
//! and the layouts of their arguments, and the event records `read` returns. Every field is a
//! 32-bit or 64-bit integer in the machine's byte order, at the offset docs/windows.md gives.

use crate::display::Rect;

/// Windows the server holds at once, the terminal's included.
pub const MAX_WINDOWS: usize = 64;
/// Windows one client (one open `/dev/window`) holds at once.
pub const MAX_WINDOWS_PER_CLIENT: usize = 16;
/// Clients (open descriptions of `/dev/window`) the server serves at once.
pub const MAX_CLIENTS: usize = 64;
/// Records one client's event queue holds.
pub const QUEUE_CAPACITY: usize = 256;
/// The longest title, in bytes of UTF-8.
pub const TITLE_CAPACITY: usize = 64;
/// The most rectangles one commit names.
pub const MAX_COMMIT_RECTS: usize = 8;
/// The longest pool of labels `SetMenu` carries, in bytes.
pub const MENU_POOL_CAPACITY: usize = 248;
/// The most menus one window's main menu holds.
pub const MAX_MENUS: usize = 6;
/// The most entries, items and separators, one menu holds.
pub const MAX_MENU_ENTRIES: usize = 16;
/// The size of one event record.
pub const EVENT_SIZE: usize = 32;
/// The `ioctl` type byte of every request (`_IOC_TYPE`).
pub const IOCTL_TYPE: u8 = b'w';

const WRITE: u32 = 1;
const READ: u32 = 2;

const fn encode(direction: u32, number: u32, size: usize) -> u32 {
    direction << 30 | (size as u32) << 16 | (IOCTL_TYPE as u32) << 8 | number
}

/// The argument sizes of the requests, in bytes.
pub mod size {
    pub const CREATE: usize = 32;
    pub const SET_TITLE: usize = 72;
    pub const SET_SIZE: usize = 24;
    pub const SET_MINIMUM_SIZE: usize = 12;
    pub const SET_POSITION: usize = 12;
    pub const SET_STATE: usize = 8;
    pub const SET_POINTER: usize = 8;
    pub const COMMIT: usize = 136;
    pub const DESTROY: usize = 4;
    pub const GET_WORK_AREA: usize = 20;
    pub const LIST_WINDOW: usize = 96;
    pub const MANAGE_WINDOW: usize = 16;
    pub const GET_PROFILE: usize = 4;
    pub const SET_PROFILE: usize = 4;
    pub const APPS_CHANGED: usize = 0;
    pub const OPEN_APP: usize = 68;
    pub const TYPE_IN_TERMINAL: usize = 260;
    pub const START_MOVE_OR_RESIZE: usize = 12;
    pub const CAPTURE_WINDOW: usize = 40;
    pub const SEND_INPUT: usize = 24;
    pub const SET_MENU: usize = 256;
    /// The largest argument.
    pub const LARGEST: usize = TYPE_IN_TERMINAL;
}

/// The request numbers, Linux's `_IOC` encoding: direction, argument size, [`IOCTL_TYPE`] and
/// the request's own number.
pub mod request {
    use super::{READ, WRITE, encode, size};

    pub const CREATE: u32 = encode(READ | WRITE, 1, size::CREATE);
    pub const SET_TITLE: u32 = encode(WRITE, 2, size::SET_TITLE);
    pub const SET_SIZE: u32 = encode(READ | WRITE, 3, size::SET_SIZE);
    pub const SET_MINIMUM_SIZE: u32 = encode(WRITE, 4, size::SET_MINIMUM_SIZE);
    pub const SET_POSITION: u32 = encode(WRITE, 5, size::SET_POSITION);
    pub const SET_STATE: u32 = encode(WRITE, 6, size::SET_STATE);
    pub const SET_POINTER: u32 = encode(WRITE, 7, size::SET_POINTER);
    pub const COMMIT: u32 = encode(WRITE, 8, size::COMMIT);
    pub const DESTROY: u32 = encode(WRITE, 9, size::DESTROY);
    pub const GET_WORK_AREA: u32 = encode(READ, 10, size::GET_WORK_AREA);
    pub const LIST_WINDOW: u32 = encode(READ | WRITE, 11, size::LIST_WINDOW);
    pub const MANAGE_WINDOW: u32 = encode(WRITE, 12, size::MANAGE_WINDOW);
    pub const GET_PROFILE: u32 = encode(READ, 13, size::GET_PROFILE);
    pub const SET_PROFILE: u32 = encode(WRITE, 14, size::SET_PROFILE);
    pub const APPS_CHANGED: u32 = encode(0, 15, size::APPS_CHANGED);
    pub const OPEN_APP: u32 = encode(WRITE, 16, size::OPEN_APP);
    pub const TYPE_IN_TERMINAL: u32 = encode(WRITE, 17, size::TYPE_IN_TERMINAL);
    pub const START_MOVE_OR_RESIZE: u32 = encode(WRITE, 18, size::START_MOVE_OR_RESIZE);
    pub const CAPTURE_WINDOW: u32 = encode(READ | WRITE, 19, size::CAPTURE_WINDOW);
    pub const SEND_INPUT: u32 = encode(WRITE, 20, size::SEND_INPUT);
    pub const SET_MENU: u32 = encode(WRITE, 21, size::SET_MENU);
}

/// What `StartMoveOrResize` starts.
pub mod move_or_resize {
    /// The window follows the pointer, as when its title bar is dragged.
    pub const MOVE: u32 = 0;
    /// The window's edges follow the pointer, as when they are dragged.
    pub const RESIZE: u32 = 1;
}

/// What `SendInput` sends to the window it names.
pub mod input_kind {
    /// The pointer moves to (`x`, `y`) in the window's content.
    pub const POINTER_MOVE: u32 = 0;
    /// Button `code` is pressed and released at (`x`, `y`).
    pub const CLICK: u32 = 1;
    /// The wheel turns by `value` notches (up when positive) at (`x`, `y`).
    pub const WHEEL: u32 = 2;
    /// Key `code` is pressed and released with the [`super::modifier`] bits in `value` held.
    pub const KEY: u32 = 3;
    /// The printable ASCII character `code` is typed, as the US keymap types it.
    pub const CHARACTER: u32 = 4;
}

/// The most wheel notches one `SendInput` turns, either way; more are clamped to it.
pub const MAX_WHEEL_NOTCHES: i32 = 16;

/// The longest app name `OpenApp` carries, in bytes.
pub const APP_NAME_CAPACITY: usize = size::OPEN_APP - 4;
/// The longest line `TypeInTerminal` carries, in bytes.
pub const TERMINAL_LINE_CAPACITY: usize = size::TYPE_IN_TERMINAL - 4;

/// The flags `Create` takes.
pub mod flags {
    /// The window keeps its size: no resize zones, no maximise (item 31).
    pub const FIXED_SIZE: u32 = 1;
    /// The window takes every Ctrl key the shell's shortcuts would otherwise take.
    pub const TAKES_CTRL_KEYS: u32 = 2;
    /// Every flag there is.
    pub const KNOWN: u32 = FIXED_SIZE | TAKES_CTRL_KEYS;
}

/// The kinds of event records.
pub mod event {
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
    pub const SCALE: u16 = 12;
    pub const OVERFLOW: u16 = 13;
    pub const MENU_CHOSEN: u16 = 14;
}

/// The modifier bits of a `Key` record's `c`.
pub mod modifier {
    pub const SHIFT: u32 = 1;
    pub const CTRL: u32 = 2;
    pub const ALT: u32 = 4;
    pub const SUPER: u32 = 8;
    pub const CAPS_LOCK: u32 = 16;
}

/// What a window is for (docs/ux/desktop-v0.3-gui.md, section 3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Normal,
    Dialog,
    Panel,
    Popup,
    Background,
}

impl Role {
    /// The role numbered `value`, 0 to 4 in the order above.
    pub const fn from_value(value: u32) -> Option<Self> {
        Some(match value {
            0 => Self::Normal,
            1 => Self::Dialog,
            2 => Self::Panel,
            3 => Self::Popup,
            4 => Self::Background,
            _ => return None,
        })
    }
}

/// A window's state, as `SetState` asks for it and `StateChanged` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowState {
    Normal,
    Minimised,
    Maximised,
}

impl WindowState {
    /// The state numbered `value`: 0 normal, 1 minimised, 2 maximised.
    pub const fn from_value(value: u32) -> Option<Self> {
        Some(match value {
            0 => Self::Normal,
            1 => Self::Minimised,
            2 => Self::Maximised,
            _ => return None,
        })
    }

    /// Its number.
    pub const fn value(self) -> u32 {
        self as u32
    }
}

/// The pointer shapes a window may ask for over its content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PointerShape {
    #[default]
    Arrow,
    IBeam,
    ResizeNorthSouth,
    ResizeEastWest,
    ResizeNorthWestSouthEast,
    ResizeNorthEastSouthWest,
}

impl PointerShape {
    /// The shape numbered `value`, 0 to 5 in the order above.
    pub const fn from_value(value: u32) -> Option<Self> {
        Some(match value {
            0 => Self::Arrow,
            1 => Self::IBeam,
            2 => Self::ResizeNorthSouth,
            3 => Self::ResizeEastWest,
            4 => Self::ResizeNorthWestSouthEast,
            5 => Self::ResizeNorthEastSouthWest,
            _ => return None,
        })
    }
}

/// Why a request was refused, and the error number it fails with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowError {
    /// No such window of the caller's, or an argument out of range (`EINVAL`).
    Invalid,
    /// A role the caller may not create (`EPERM`).
    NotAllowed,
    /// The caller or the server holds as many windows as it may (`ENOSPC`).
    TableFull,
    /// Something this version does not do yet (`EOPNOTSUPP`).
    Unsupported,
    /// No window at that place in the list, or no window of that id (`ENOENT`).
    NoSuchWindow,
    /// A person is using the keyboard or pointer, so injected input waits (`EBUSY`).
    Busy,
    /// The window's client has no room in its queue for the input's events yet (`EAGAIN`).
    QueueFull,
}

/// A request and its arguments, as the caller wrote them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Create {
        role: u32,
        parent: u32,
        width: u32,
        height: u32,
        flags: u32,
    },
    SetTitle {
        window: u32,
        length: u32,
        title: [u8; TITLE_CAPACITY],
    },
    SetSize {
        window: u32,
        width: u32,
        height: u32,
    },
    SetMinimumSize {
        window: u32,
        width: u32,
        height: u32,
    },
    SetPosition {
        window: u32,
        x: i32,
        y: i32,
    },
    SetState {
        window: u32,
        state: u32,
    },
    SetPointer {
        window: u32,
        shape: u32,
    },
    Commit {
        window: u32,
        count: u32,
        rects: [Rect; MAX_COMMIT_RECTS],
    },
    Destroy {
        window: u32,
    },
    GetWorkArea,
    ListWindow {
        index: u32,
    },
    ManageWindow {
        window: u32,
        action: u32,
        a: i32,
        b: i32,
    },
    GetProfile,
    SetProfile {
        profile: u32,
    },
    AppsChanged,
    OpenApp {
        length: u32,
        name: [u8; APP_NAME_CAPACITY],
    },
    TypeInTerminal {
        length: u32,
        line: [u8; TERMINAL_LINE_CAPACITY],
    },
    StartMoveOrResize {
        window: u32,
        kind: u32,
        edges: u32,
    },
    CaptureWindow {
        window: u32,
        first_row: u32,
        row_count: u32,
        buffer: u64,
        length: u64,
    },
    SendInput {
        window: u32,
        kind: u32,
        x: i32,
        y: i32,
        code: u32,
        value: u32,
    },
    SetMenu {
        window: u32,
        length: u32,
        pool: [u8; MENU_POOL_CAPACITY],
    },
}

fn word(bytes: &[u8], offset: usize) -> u32 {
    let mut word = [0; 4];
    word.copy_from_slice(&bytes[offset..offset + 4]);
    u32::from_ne_bytes(word)
}

fn double_word(bytes: &[u8], offset: usize) -> u64 {
    let mut word = [0; 8];
    word.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_ne_bytes(word)
}

/// Writes `value` at `offset` of `bytes`.
pub fn put_word(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
}

/// Writes `value` at `offset` of `bytes`.
pub fn put_double_word(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_ne_bytes());
}

impl Request {
    /// The size of request `number`'s argument; `None` for a number that is no request.
    pub const fn argument_size(number: u32) -> Option<usize> {
        Some(match number {
            request::CREATE => size::CREATE,
            request::SET_TITLE => size::SET_TITLE,
            request::SET_SIZE => size::SET_SIZE,
            request::SET_MINIMUM_SIZE => size::SET_MINIMUM_SIZE,
            request::SET_POSITION => size::SET_POSITION,
            request::SET_STATE => size::SET_STATE,
            request::SET_POINTER => size::SET_POINTER,
            request::COMMIT => size::COMMIT,
            request::DESTROY => size::DESTROY,
            request::GET_WORK_AREA => size::GET_WORK_AREA,
            request::LIST_WINDOW => size::LIST_WINDOW,
            request::MANAGE_WINDOW => size::MANAGE_WINDOW,
            request::GET_PROFILE => size::GET_PROFILE,
            request::SET_PROFILE => size::SET_PROFILE,
            request::APPS_CHANGED => size::APPS_CHANGED,
            request::OPEN_APP => size::OPEN_APP,
            request::TYPE_IN_TERMINAL => size::TYPE_IN_TERMINAL,
            request::START_MOVE_OR_RESIZE => size::START_MOVE_OR_RESIZE,
            request::CAPTURE_WINDOW => size::CAPTURE_WINDOW,
            request::SEND_INPUT => size::SEND_INPUT,
            request::SET_MENU => size::SET_MENU,
            _ => return None,
        })
    }

    /// Request `number` with the argument `bytes` the caller passed, which must be
    /// [`Self::argument_size`] long; `None` for a number that is no request or an argument of
    /// another size.
    pub fn parse(number: u32, bytes: &[u8]) -> Option<Self> {
        if Self::argument_size(number) != Some(bytes.len()) {
            return None;
        }
        let at = |offset| word(bytes, offset);
        Some(match number {
            request::CREATE => Self::Create {
                role: at(0),
                parent: at(4),
                width: at(8),
                height: at(12),
                flags: at(16),
            },
            request::SET_TITLE => {
                let mut title = [0; TITLE_CAPACITY];
                title.copy_from_slice(&bytes[8..8 + TITLE_CAPACITY]);
                Self::SetTitle {
                    window: at(0),
                    length: at(4),
                    title,
                }
            }
            request::SET_SIZE => Self::SetSize {
                window: at(0),
                width: at(4),
                height: at(8),
            },
            request::SET_MINIMUM_SIZE => Self::SetMinimumSize {
                window: at(0),
                width: at(4),
                height: at(8),
            },
            request::SET_POSITION => Self::SetPosition {
                window: at(0),
                x: at(4) as i32,
                y: at(8) as i32,
            },
            request::SET_STATE => Self::SetState {
                window: at(0),
                state: at(4),
            },
            request::SET_POINTER => Self::SetPointer {
                window: at(0),
                shape: at(4),
            },
            request::COMMIT => {
                let mut rects = [Rect::default(); MAX_COMMIT_RECTS];
                for (index, rect) in rects.iter_mut().enumerate() {
                    let base = 8 + index * 16;
                    *rect = Rect::new(at(base), at(base + 4), at(base + 8), at(base + 12));
                }
                Self::Commit {
                    window: at(0),
                    count: at(4),
                    rects,
                }
            }
            request::DESTROY => Self::Destroy { window: at(0) },
            request::LIST_WINDOW => Self::ListWindow { index: at(0) },
            request::MANAGE_WINDOW => Self::ManageWindow {
                window: at(0),
                action: at(4),
                a: at(8) as i32,
                b: at(12) as i32,
            },
            request::GET_PROFILE => Self::GetProfile,
            request::SET_PROFILE => Self::SetProfile { profile: at(0) },
            request::APPS_CHANGED => Self::AppsChanged,
            request::OPEN_APP => {
                let mut name = [0; APP_NAME_CAPACITY];
                name.copy_from_slice(&bytes[4..4 + APP_NAME_CAPACITY]);
                Self::OpenApp {
                    length: at(0),
                    name,
                }
            }
            request::TYPE_IN_TERMINAL => {
                let mut line = [0; TERMINAL_LINE_CAPACITY];
                line.copy_from_slice(&bytes[4..4 + TERMINAL_LINE_CAPACITY]);
                Self::TypeInTerminal {
                    length: at(0),
                    line,
                }
            }
            request::START_MOVE_OR_RESIZE => Self::StartMoveOrResize {
                window: at(0),
                kind: at(4),
                edges: at(8),
            },
            request::CAPTURE_WINDOW => Self::CaptureWindow {
                window: at(0),
                first_row: at(4),
                row_count: at(8),
                buffer: double_word(bytes, 24),
                length: double_word(bytes, 32),
            },
            request::SEND_INPUT => Self::SendInput {
                window: at(0),
                kind: at(4),
                x: at(8) as i32,
                y: at(12) as i32,
                code: at(16),
                value: at(20),
            },
            request::SET_MENU => {
                let mut pool = [0; MENU_POOL_CAPACITY];
                pool.copy_from_slice(&bytes[8..8 + MENU_POOL_CAPACITY]);
                Self::SetMenu {
                    window: at(0),
                    length: at(4),
                    pool,
                }
            }
            _ => Self::GetWorkArea,
        })
    }
}

/// Where `Create` writes the window's id, its content width and height and its buffer's offset
/// back into its argument.
pub mod create_reply {
    pub const WIDTH: usize = 8;
    pub const HEIGHT: usize = 12;
    pub const WINDOW: usize = 20;
    pub const OFFSET: usize = 24;
}

/// Where `SetSize` writes the content width and height and the new buffer's offset back.
pub mod set_size_reply {
    pub const WIDTH: usize = 4;
    pub const HEIGHT: usize = 8;
    pub const OFFSET: usize = 16;
}

/// Where `CaptureWindow` writes the window's content width and height and how many rows it
/// copied back into its argument.
pub mod capture_reply {
    pub const WIDTH: usize = 12;
    pub const HEIGHT: usize = 16;
    pub const ROWS: usize = 20;
}

/// Where `ListWindow` writes the window at the index it was given: its id, state, outside
/// rectangle on the screen, client (`u16::MAX` for the kernel's), and title.
pub mod list_reply {
    pub const WINDOW: usize = 0;
    pub const STATE: usize = 4;
    pub const X: usize = 8;
    pub const Y: usize = 12;
    pub const WIDTH: usize = 16;
    pub const HEIGHT: usize = 20;
    pub const CLIENT: usize = 24;
    pub const TITLE_LENGTH: usize = 28;
    pub const TITLE: usize = 32;
}

/// What `ManageWindow` does to the window it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowAction {
    /// Raise and focus it, restoring it if it is minimised.
    Focus,
    Minimise,
    Maximise,
    /// Restore it from minimised or maximised.
    Restore,
    /// Send it a close request, as its close control does.
    Close,
    /// Move its outside top-left to (`a`, `b`) on the screen, kept in the work area.
    Move,
    /// Give it a content of `a` by `b`, clamped as the user's resizing is.
    Size,
}

impl WindowAction {
    /// The action numbered `value`, 0 to 6 in the order above.
    pub const fn from_value(value: u32) -> Option<Self> {
        Some(match value {
            0 => Self::Focus,
            1 => Self::Minimise,
            2 => Self::Maximise,
            3 => Self::Restore,
            4 => Self::Close,
            5 => Self::Move,
            6 => Self::Size,
            _ => return None,
        })
    }
}

/// One event record: `kind`, `window`, `time_ms` (the monotonic clock), then `a`, `b`, `c`,
/// `d`, whose meaning depends on the kind, and 12 bytes of zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EventRecord {
    pub kind: u16,
    pub window: u16,
    pub time_ms: u32,
    pub a: i32,
    pub b: i32,
    pub c: u32,
    pub d: u32,
}

impl EventRecord {
    /// A record of `kind` for `window` with `a`, `b`, `c` and `d`, its time not yet set.
    pub const fn new(kind: u16, window: u16, a: i32, b: i32, c: u32, d: u32) -> Self {
        Self {
            kind,
            window,
            time_ms: 0,
            a,
            b,
            c,
            d,
        }
    }

    /// The record as `read` returns it.
    pub fn to_bytes(self) -> [u8; EVENT_SIZE] {
        let mut bytes = [0; EVENT_SIZE];
        bytes[0..2].copy_from_slice(&self.kind.to_ne_bytes());
        bytes[2..4].copy_from_slice(&self.window.to_ne_bytes());
        put_word(&mut bytes, 4, self.time_ms);
        put_word(&mut bytes, 8, self.a as u32);
        put_word(&mut bytes, 12, self.b as u32);
        put_word(&mut bytes, 16, self.c);
        put_word(&mut bytes, 20, self.d);
        bytes
    }
}

/// Where a client's events go: the kernel's queues, or nowhere.
pub trait EventSink {
    /// Hands `record` to client `client`'s queue.
    fn send(&mut self, client: u16, record: EventRecord);

    /// How many records client `client`'s queue takes before it must drop one.
    fn room(&self, _client: u16) -> usize {
        usize::MAX
    }
}

/// An [`EventSink`] that drops every event.
#[derive(Debug, Clone, Copy, Default)]
pub struct DiscardEvents;

impl EventSink for DiscardEvents {
    fn send(&mut self, _client: u16, _record: EventRecord) {}
}
