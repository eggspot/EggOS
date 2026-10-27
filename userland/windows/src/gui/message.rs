//! Win32's message numbers and the values some of them carry.

pub const CREATE: u32 = 0x0001;
pub const DESTROY: u32 = 0x0002;
pub const SIZE: u32 = 0x0005;
pub const SET_FOCUS: u32 = 0x0007;
pub const KILL_FOCUS: u32 = 0x0008;
pub const PAINT: u32 = 0x000f;
pub const CLOSE: u32 = 0x0010;
pub const QUIT: u32 = 0x0012;
pub const ERASE_BACKGROUND: u32 = 0x0014;
pub const SHOW_WINDOW: u32 = 0x0018;
pub const HELP: u32 = 0x0053;
pub const NC_CREATE: u32 = 0x0081;
pub const NC_DESTROY: u32 = 0x0082;
pub const KEY_DOWN: u32 = 0x0100;
pub const KEY_UP: u32 = 0x0101;
pub const CHAR: u32 = 0x0102;
pub const SYS_KEY_DOWN: u32 = 0x0104;
pub const SYS_KEY_UP: u32 = 0x0105;
pub const SYS_CHAR: u32 = 0x0106;
pub const SYS_COMMAND: u32 = 0x0112;
pub const MOUSE_MOVE: u32 = 0x0200;
pub const LEFT_BUTTON_DOWN: u32 = 0x0201;
pub const LEFT_BUTTON_UP: u32 = 0x0202;
pub const RIGHT_BUTTON_DOWN: u32 = 0x0204;
pub const RIGHT_BUTTON_UP: u32 = 0x0205;
pub const MIDDLE_BUTTON_DOWN: u32 = 0x0207;
pub const MIDDLE_BUTTON_UP: u32 = 0x0208;
pub const MOUSE_WHEEL: u32 = 0x020a;

pub const SIZE_RESTORED: usize = 0;
pub const SIZE_MINIMIZED: usize = 1;
pub const SIZE_MAXIMIZED: usize = 2;
pub const SC_CLOSE: usize = 0xf060;

/// `lParam` holding the 16-bit coordinates `x` and `y`, as `MAKELPARAM` packs them.
pub fn point_parameter(x: i32, y: i32) -> usize {
    (usize::from(y as i16 as u16) << 16) | usize::from(x as i16 as u16)
}
