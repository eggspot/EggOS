//! ANSI windows and the user32 functions that take ANSI text (D174). The layer's ANSI code page
//! is UTF-8 (`strings.rs`), so ANSI text is UTF-8 and a conversion is exact. Inside the layer
//! every message carries UTF-16 text; a window whose class `RegisterClassA` registered gets
//! its messages' text as UTF-8 when its procedure is called ([`call_procedure`]), and the
//! ANSI functions (`SendMessageA`, `DefWindowProcA`, `SetWindowTextA`, ...) widen the text
//! they are given and narrow the text they give back ([`from_narrow`]). The functions whose
//! ANSI form takes no text are the wide ones under both names.

use super::gui::{self, try_with};
use super::user32::{CreateStruct, Msg, def_window_proc_w};
use super::{Bool, FALSE, call_program, dialog, error, fail, heap, menu, message_box, user32};
use crate::gui::classes::Class;
use crate::gui::message;
use crate::library::Export;
use crate::text;

const ATOM_LIMIT: usize = 0x1_0000;
const UTF8_BYTES_PER_UNIT: usize = 3;
const MF_BITMAP: u32 = 0x4;
const MF_OWNER_DRAW: u32 = 0x100;
const MF_SEPARATOR: u32 = 0x800;

mod string_message {
    pub const SET_TEXT: u32 = 0x000c;
    pub const EM_REPLACESEL: u32 = 0x00c2;
    pub const CB_ADDSTRING: u32 = 0x0143;
    pub const CB_INSERTSTRING: u32 = 0x014a;
    pub const CB_FINDSTRING: u32 = 0x014c;
    pub const CB_SELECTSTRING: u32 = 0x014d;
    pub const CB_FINDSTRINGEXACT: u32 = 0x0158;
    pub const LB_ADDSTRING: u32 = 0x0180;
    pub const LB_INSERTSTRING: u32 = 0x0181;
    pub const LB_SELECTSTRING: u32 = 0x018c;
    pub const LB_FINDSTRING: u32 = 0x018f;
    pub const LB_FINDSTRINGEXACT: u32 = 0x01a2;
}

mod item_message {
    pub const CB_GETLBTEXT: u32 = 0x0148;
    pub const CB_GETLBTEXTLEN: u32 = 0x0149;
    pub const LB_GETTEXT: u32 = 0x0189;
    pub const LB_GETTEXTLEN: u32 = 0x018a;
}

const DEAD_CHAR: u32 = 0x0103;
const SYS_DEAD_CHAR: u32 = 0x0107;
const NOT_FOUND: usize = usize::MAX;

fn reads_string(number: u32) -> bool {
    use string_message::*;
    matches!(
        number,
        SET_TEXT
            | EM_REPLACESEL
            | CB_ADDSTRING
            | CB_INSERTSTRING
            | CB_FINDSTRING
            | CB_SELECTSTRING
            | CB_FINDSTRINGEXACT
            | LB_ADDSTRING
            | LB_INSERTSTRING
            | LB_SELECTSTRING
            | LB_FINDSTRING
            | LB_FINDSTRINGEXACT
    )
}

fn is_character(number: u32) -> bool {
    matches!(
        number,
        message::CHAR | message::SYS_CHAR | DEAD_CHAR | SYS_DEAD_CHAR
    )
}

/// UTF-16 text in a heap block, NUL-terminated, freed when dropped.
pub struct Wide {
    block: *mut u16,
    length: usize,
}

impl Wide {
    pub fn units(&self) -> &[u16] {
        // SAFETY: the block holds `length` written units and the NUL, alive until dropped.
        unsafe { core::slice::from_raw_parts(self.block, self.length) }
    }

    pub fn pointer(&self) -> *const u16 {
        self.block
    }

    pub fn address(&self) -> usize {
        self.block as usize
    }
}

impl Drop for Wide {
    fn drop(&mut self) {
        // SAFETY: the block `widen` allocated, used by nothing after its owner is dropped.
        unsafe { heap::free(self.block.cast()) };
    }
}

/// UTF-8 text in a heap block, NUL-terminated, freed when dropped.
pub struct Narrow {
    block: *mut u8,
}

impl Narrow {
    pub fn address(&self) -> usize {
        self.block as usize
    }
}

impl Drop for Narrow {
    fn drop(&mut self) {
        // SAFETY: the block `narrow` allocated, used by nothing after its owner is dropped.
        unsafe { heap::free(self.block) };
    }
}

/// `bytes` (UTF-8) as NUL-terminated UTF-16; `None` without memory.
pub fn widen(bytes: &[u8]) -> Option<Wide> {
    let length = text::utf16_length(bytes);
    let block = heap::allocate((length + 1) * 2).cast::<u16>();
    if block.is_null() {
        return None;
    }
    // SAFETY: a new, zeroed block of `length + 1` units, this function's.
    let out = unsafe { core::slice::from_raw_parts_mut(block, length + 1) };
    let written = text::write_utf16(bytes, &mut out[..length]);
    out[written] = 0;
    Some(Wide {
        block,
        length: written,
    })
}

/// `units` (UTF-16) as NUL-terminated UTF-8; `None` without memory.
pub fn narrow(units: &[u16]) -> Option<Narrow> {
    let length = text::utf8_length(units);
    let block = heap::allocate(length + 1);
    if block.is_null() {
        return None;
    }
    // SAFETY: a new, zeroed block of `length + 1` bytes, this function's.
    let out = unsafe { core::slice::from_raw_parts_mut(block, length + 1) };
    let (written, _) = text::write_utf8(units, &mut out[..length]);
    out[written] = 0;
    Some(Narrow { block })
}

/// The bytes of the NUL-terminated string at `text`.
///
/// # Safety
/// `text` is a readable NUL-terminated string.
pub unsafe fn c_text<'a>(text: *const u8) -> &'a [u8] {
    // SAFETY: the caller's duty.
    unsafe { core::ffi::CStr::from_ptr(text.cast()) }.to_bytes()
}

/// The ANSI string at `text` widened, or `None` for a null `text`; `Err` without memory.
///
/// # Safety
/// `text` is null or a readable NUL-terminated string.
pub unsafe fn widen_text(text: *const u8) -> Result<Option<Wide>, u32> {
    if text.is_null() {
        return Ok(None);
    }
    // SAFETY: the caller's duty.
    let bytes = unsafe { c_text(text) };
    widen(bytes).map(Some).ok_or(error::NOT_ENOUGH_MEMORY)
}

fn pointer_of(wide: &Option<Wide>) -> *const u16 {
    wide.as_ref().map_or(core::ptr::null(), Wide::pointer)
}

unsafe fn widen_name(name: *const u8) -> Result<(*const u16, Option<Wide>), u32> {
    if (name as usize) < ATOM_LIMIT {
        return Ok((name.cast(), None));
    }
    // SAFETY: the caller's duty.
    let wide = unsafe { widen_text(name) }?;
    Ok((pointer_of(&wide), wide))
}

/// Calls the ANSI window procedure at `address` with a message whose text is UTF-16, as the
/// layer holds it: strings it reads are narrowed, `WM_GETTEXT`'s answer widened back into the
/// caller's buffer, `WM_CREATE`'s and `WM_NCCREATE`'s names narrowed in a copy of the
/// `CREATESTRUCT`, and a character past ASCII sent as its UTF-8 bytes, one message each.
pub fn call_procedure(
    address: usize,
    window: usize,
    number: u32,
    wparam: usize,
    lparam: usize,
) -> usize {
    let call = |wparam: usize, lparam: usize| {
        // SAFETY: the address is the window procedure the program gave for this window, which
        // Windows calls with these four arguments.
        unsafe { call_program(address, [window, number as usize, wparam, lparam]) }
    };
    if reads_string(number) && lparam != 0 {
        // SAFETY: these messages' `lParam` is a NUL-terminated string.
        let units = unsafe { super::file::wide_string(lparam as *const u16) };
        return match narrow(units) {
            Some(text) => call(wparam, text.address()),
            None => 0,
        };
    }
    match number {
        message::GET_TEXT if lparam != 0 && wparam != 0 => {
            let room = wparam.saturating_mul(UTF8_BYTES_PER_UNIT);
            let block = heap::allocate(room + 1);
            if block.is_null() {
                return 0;
            }
            let length = call(room, block as usize).min(room);
            // SAFETY: the procedure wrote at most `room` bytes into the block.
            let bytes = unsafe { core::slice::from_raw_parts(block, length) };
            // SAFETY: the caller's buffer holds `wparam` writable units.
            let out = unsafe { core::slice::from_raw_parts_mut(lparam as *mut u16, wparam) };
            let copied = text::write_utf16(bytes, &mut out[..wparam - 1]);
            out[copied] = 0;
            // SAFETY: the block this call allocated, used by nothing now.
            unsafe { heap::free(block) };
            copied
        }
        message::CREATE | message::NC_CREATE if lparam != 0 => {
            // SAFETY: these messages' `lParam` is the layer's own `CREATESTRUCT`.
            let mut copy = unsafe { (lparam as *const CreateStruct).read() };
            let name = match copy.name.is_null() {
                true => None,
                // SAFETY: the window name is a NUL-terminated string.
                false => narrow(unsafe { super::file::wide_string(copy.name) }),
            };
            let class = match (copy.class_name as usize) < ATOM_LIMIT {
                true => None,
                // SAFETY: the class name is a NUL-terminated string when it is no atom.
                false => narrow(unsafe { super::file::wide_string(copy.class_name) }),
            };
            if let Some(name) = &name {
                copy.name = name.address() as *const u16;
            }
            if let Some(class) = &class {
                copy.class_name = class.address() as *const u16;
            }
            call(wparam, &raw const copy as usize)
        }
        number if is_character(number) && wparam >= 0x80 => {
            let character = char::from_u32(wparam as u32).unwrap_or(text::REPLACEMENT);
            let mut bytes = [0u8; 4];
            let mut result = 0;
            for &byte in character.encode_utf8(&mut bytes).as_bytes() {
                result = call(usize::from(byte), lparam);
            }
            result
        }
        _ => call(wparam, lparam),
    }
}

fn item_length_message(number: u32) -> Option<u32> {
    match number {
        item_message::CB_GETLBTEXT => Some(item_message::CB_GETLBTEXTLEN),
        item_message::LB_GETTEXT => Some(item_message::LB_GETTEXTLEN),
        _ => None,
    }
}

fn measures_text(number: u32) -> bool {
    matches!(
        number,
        message::GET_TEXT_LENGTH | item_message::CB_GETLBTEXTLEN | item_message::LB_GETTEXTLEN
    )
}

fn text_of(length: usize, copy: impl FnOnce(usize, usize) -> usize) -> Option<(*mut u16, usize)> {
    let block = heap::allocate((length + 1) * 2).cast::<u16>();
    if block.is_null() {
        return None;
    }
    let copied = copy(length + 1, block as usize).min(length);
    Some((block, copied))
}

/// Passes a message given with ANSI text to `deliver`, which takes UTF-16: strings it reads are
/// widened, text it copies out (`WM_GETTEXT`, `CB_GETLBTEXT`, `LB_GETTEXT`) is narrowed into
/// the caller's buffer, and lengths are counted in UTF-8 bytes.
pub fn from_narrow(
    window: usize,
    number: u32,
    wparam: usize,
    lparam: usize,
    deliver: &dyn Fn(usize, u32, usize, usize) -> usize,
) -> usize {
    if reads_string(number) && lparam != 0 {
        // SAFETY: these messages' `lParam` is a NUL-terminated string.
        let bytes = unsafe { c_text(lparam as *const u8) };
        return match widen(bytes) {
            Some(wide) => deliver(window, number, wparam, wide.address()),
            None => 0,
        };
    }
    if let Some(length_message) = item_length_message(number) {
        let length = deliver(window, length_message, wparam, 0);
        if length == NOT_FOUND || lparam == 0 {
            return length;
        }
        let Some((block, copied)) =
            text_of(length, |_, buffer| deliver(window, number, wparam, buffer))
        else {
            return NOT_FOUND;
        };
        // SAFETY: `block` holds `copied` units the control wrote.
        let units = unsafe { core::slice::from_raw_parts(block, copied) };
        let needed = text::utf8_length(units);
        // SAFETY: the caller's buffer has room for the item's text and a NUL, as
        // `CB_GETLBTEXTLEN` and `LB_GETTEXTLEN` measured it.
        let out = unsafe { core::slice::from_raw_parts_mut(lparam as *mut u8, needed + 1) };
        let (written, _) = text::write_utf8(units, &mut out[..needed]);
        out[written] = 0;
        // SAFETY: the block `text_of` allocated, used by nothing now.
        unsafe { heap::free(block.cast()) };
        return written;
    }
    match number {
        message::GET_TEXT => {
            if lparam == 0 || wparam == 0 {
                return 0;
            }
            let Some((block, copied)) =
                text_of(wparam, |room, buffer| deliver(window, number, room, buffer))
            else {
                return 0;
            };
            // SAFETY: `block` holds `copied` units.
            let units = unsafe { core::slice::from_raw_parts(block, copied) };
            // SAFETY: the caller's buffer holds `wparam` bytes.
            let out = unsafe { core::slice::from_raw_parts_mut(lparam as *mut u8, wparam) };
            let (written, _) = text::write_utf8(units, &mut out[..wparam - 1]);
            out[written] = 0;
            // SAFETY: as above.
            unsafe { heap::free(block.cast()) };
            written
        }
        number if measures_text(number) => {
            let length = deliver(window, number, wparam, lparam);
            if length == NOT_FOUND {
                return length;
            }
            let fetch = match number {
                message::GET_TEXT_LENGTH => message::GET_TEXT,
                item_message::CB_GETLBTEXTLEN => item_message::CB_GETLBTEXT,
                _ => item_message::LB_GETTEXT,
            };
            let Some((block, copied)) = text_of(length, |room, buffer| match fetch {
                message::GET_TEXT => deliver(window, fetch, room, buffer),
                _ => deliver(window, fetch, wparam, buffer),
            }) else {
                return length;
            };
            // SAFETY: `block` holds `copied` units.
            let bytes = text::utf8_length(unsafe { core::slice::from_raw_parts(block, copied) });
            // SAFETY: the block `text_of` allocated, used by nothing now.
            unsafe { heap::free(block.cast()) };
            bytes
        }
        _ => deliver(window, number, wparam, lparam),
    }
}

/// `DefWindowProcA`: [`from_narrow`] over `DefWindowProcW`.
pub extern "efiapi" fn def_window_proc_a(
    window: usize,
    number: u32,
    wparam: usize,
    lparam: usize,
) -> usize {
    from_narrow(
        window,
        number,
        wparam,
        lparam,
        &|window, number, wparam, lparam| def_window_proc_w(window, number, wparam, lparam),
    )
}

fn known_window(window: usize) -> Result<(), u32> {
    try_with(|gui| gui.window(window).map(|_| ()))
}

extern "efiapi" fn send_message_a(
    window: usize,
    number: u32,
    wparam: usize,
    lparam: usize,
) -> usize {
    match known_window(window) {
        Ok(()) => from_narrow(window, number, wparam, lparam, &gui::send),
        Err(code) => fail(code, 0),
    }
}

unsafe extern "efiapi" fn set_window_text_a(window: usize, text: *const u8) -> Bool {
    if let Err(code) = known_window(window) {
        return fail(code, FALSE);
    }
    Bool::from(from_narrow(window, message::SET_TEXT, 0, text as usize, &gui::send) != 0)
}

unsafe extern "efiapi" fn get_window_text_a(window: usize, buffer: *mut u8, capacity: i32) -> i32 {
    if let Err(code) = known_window(window) {
        return fail(code, 0);
    }
    let capacity = capacity.max(0) as usize;
    from_narrow(
        window,
        message::GET_TEXT,
        capacity,
        buffer as usize,
        &gui::send,
    ) as i32
}

extern "efiapi" fn get_window_text_length_a(window: usize) -> i32 {
    if let Err(code) = known_window(window) {
        return fail(code, 0);
    }
    from_narrow(window, message::GET_TEXT_LENGTH, 0, 0, &gui::send) as i32
}

extern "efiapi" fn send_dlg_item_message_a(
    dialog: usize,
    id: i32,
    number: u32,
    wparam: usize,
    lparam: usize,
) -> usize {
    match dialog::dialog_item(dialog, id) {
        Some(item) => from_narrow(item, number, wparam, lparam, &gui::send),
        None => fail(error::CONTROL_ID_NOT_FOUND, 0),
    }
}

unsafe extern "efiapi" fn get_dlg_item_text_a(
    dialog: usize,
    id: i32,
    buffer: *mut u8,
    capacity: i32,
) -> u32 {
    match dialog::dialog_item(dialog, id) {
        Some(item) => {
            let capacity = capacity.max(0) as usize;
            from_narrow(
                item,
                message::GET_TEXT,
                capacity,
                buffer as usize,
                &gui::send,
            ) as u32
        }
        None => fail(error::CONTROL_ID_NOT_FOUND, 0),
    }
}

unsafe extern "efiapi" fn set_dlg_item_text_a(dialog: usize, id: i32, text: *const u8) -> Bool {
    match dialog::dialog_item(dialog, id) {
        Some(item) => {
            let set = from_narrow(item, message::SET_TEXT, 0, text as usize, &gui::send);
            Bool::from(set != 0)
        }
        None => fail(error::CONTROL_ID_NOT_FOUND, FALSE),
    }
}

#[repr(C)]
struct WindowClassA {
    style: u32,
    procedure: usize,
    class_extra: i32,
    window_extra: i32,
    instance: usize,
    icon: usize,
    cursor: usize,
    background: usize,
    menu_name: *const u8,
    class_name: *const u8,
}

#[allow(clippy::too_many_arguments)]
fn ansi_class(
    style: u32,
    procedure: usize,
    class_extra: i32,
    window_extra: i32,
    instance: usize,
    icon: usize,
    cursor: usize,
    background: usize,
) -> Class {
    Class {
        style,
        procedure,
        class_extra: class_extra.max(0) as u32,
        window_extra: window_extra.max(0) as u32,
        instance,
        icon,
        cursor,
        background,
        ansi: true,
    }
}

unsafe fn register_narrow(name: *const u8, class: Class) -> u16 {
    if (name as usize) < ATOM_LIMIT {
        return fail(error::INVALID_PARAMETER, 0);
    }
    // SAFETY: the caller's duty: a NUL-terminated class name.
    match unsafe { widen_text(name) } {
        // SAFETY: the widened name is NUL-terminated.
        Ok(Some(wide)) => unsafe { user32::register(wide.pointer(), class) },
        Ok(None) => fail(error::INVALID_PARAMETER, 0),
        Err(code) => fail(code, 0),
    }
}

unsafe extern "efiapi" fn register_class_a(class: *const WindowClassA) -> u16 {
    // SAFETY: the caller passes a `WNDCLASSA`.
    let class = unsafe { &*class };
    let registered = ansi_class(
        class.style,
        class.procedure,
        class.class_extra,
        class.window_extra,
        class.instance,
        class.icon,
        class.cursor,
        class.background,
    );
    // SAFETY: the class's name is the caller's NUL-terminated string.
    unsafe { register_narrow(class.class_name, registered) }
}

#[repr(C)]
struct WindowClassExA {
    size: u32,
    style: u32,
    procedure: usize,
    class_extra: i32,
    window_extra: i32,
    instance: usize,
    icon: usize,
    cursor: usize,
    background: usize,
    menu_name: *const u8,
    class_name: *const u8,
    small_icon: usize,
}

unsafe extern "efiapi" fn register_class_ex_a(class: *const WindowClassExA) -> u16 {
    // SAFETY: the caller passes a `WNDCLASSEXA`.
    let class = unsafe { &*class };
    if class.size as usize != core::mem::size_of::<WindowClassExA>() {
        return fail(error::INVALID_PARAMETER, 0);
    }
    let registered = ansi_class(
        class.style,
        class.procedure,
        class.class_extra,
        class.window_extra,
        class.instance,
        class.icon,
        class.cursor,
        class.background,
    );
    // SAFETY: the class's name is the caller's NUL-terminated string.
    unsafe { register_narrow(class.class_name, registered) }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "efiapi" fn create_window_ex_a(
    ex_style: u32,
    class: *const u8,
    window_name: *const u8,
    style: u32,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    parent: usize,
    menu: usize,
    instance: usize,
    parameter: usize,
) -> usize {
    // SAFETY: `CreateWindowExA`'s contract: an atom or a NUL-terminated class name.
    let class = match unsafe { widen_name(class) } {
        Ok(class) => class,
        Err(code) => return fail(code, 0),
    };
    // SAFETY: and a NUL-terminated window name or null.
    let name = match unsafe { widen_text(window_name) } {
        Ok(name) => name,
        Err(code) => return fail(code, 0),
    };
    // SAFETY: the widened names are NUL-terminated (or an atom) and outlive the call.
    unsafe {
        user32::create_window_ex_w(
            ex_style,
            class.0,
            pointer_of(&name),
            style,
            x,
            y,
            width,
            height,
            parent,
            menu,
            instance,
            parameter,
        )
    }
}

unsafe extern "efiapi" fn message_box_a(
    owner: usize,
    message_text: *const u8,
    caption: *const u8,
    box_type: u32,
) -> i32 {
    // SAFETY: `MessageBoxA`'s contract: a NUL-terminated string or null.
    let text = unsafe { widen_text(message_text) };
    // SAFETY: as above.
    let title = unsafe { widen_text(caption) };
    let (Ok(text), Ok(title)) = (text, title) else {
        return fail(error::NOT_ENOUGH_MEMORY, 0);
    };
    // SAFETY: the widened strings are NUL-terminated and outlive the call.
    unsafe { message_box::message_box_w(owner, pointer_of(&text), pointer_of(&title), box_type) }
}

unsafe extern "efiapi" fn append_menu_a(
    menu: usize,
    flags: u32,
    id: usize,
    label: *const u8,
) -> Bool {
    let has_text = flags & (MF_SEPARATOR | MF_OWNER_DRAW | MF_BITMAP) == 0 && !label.is_null();
    if !has_text {
        // SAFETY: a label that is no string is passed on untouched.
        return unsafe { menu::append_menu_w(menu, flags, id, label.cast()) };
    }
    // SAFETY: an `MF_STRING` item's label is the caller's NUL-terminated string.
    match unsafe { widen_text(label) } {
        // SAFETY: the widened label is NUL-terminated and outlives the call.
        Ok(wide) => unsafe { menu::append_menu_w(menu, flags, id, pointer_of(&wide)) },
        Err(code) => fail(code, FALSE),
    }
}

unsafe extern "efiapi" fn is_dialog_message_a(dialog: usize, message: *const Msg) -> Bool {
    // SAFETY: the caller passes a readable `MSG`.
    let message = unsafe { user32::read_message(message) };
    Bool::from(dialog::dialog_message(dialog, &message))
}

pub(super) const EXPORTS: &[Export] = exports![
    b"DefWindowProcA" => def_window_proc_a,
    b"SendMessageA" => send_message_a,
    b"SetWindowTextA" => set_window_text_a,
    b"GetWindowTextA" => get_window_text_a,
    b"GetWindowTextLengthA" => get_window_text_length_a,
    b"SendDlgItemMessageA" => send_dlg_item_message_a,
    b"GetDlgItemTextA" => get_dlg_item_text_a,
    b"SetDlgItemTextA" => set_dlg_item_text_a,
    b"RegisterClassA" => register_class_a,
    b"RegisterClassExA" => register_class_ex_a,
    b"CreateWindowExA" => create_window_ex_a [four_stack_arguments],
    b"MessageBoxA" => message_box_a,
    b"AppendMenuA" => append_menu_a,
    b"IsDialogMessageA" => is_dialog_message_a,
];
