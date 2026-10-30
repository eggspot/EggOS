//! What a program learns of window classes (D178): `GetClassNameW`, `GetClassLongW`,
//! `GetClassLongPtrW` and `GetClassInfoExW` for its own classes and the system's (the controls,
//! comctl32's common controls and the dialog class `#32770`), and `RegisterWindowMessageW`'s
//! messages from the registered names.

use super::gui::{Gui, try_with, with};
use super::{Bool, FALSE, TRUE, error, fail};
use crate::gui::classes::{Class, ClassName};
use crate::gui::objects::{self, cursor};
use crate::gui::windows::{CommonKind, ControlKind, Procedure, Window};
use crate::library::Export;
use crate::text;

const ATOM_LIMIT: usize = 0x1_0000;
const CS_VREDRAW: u32 = 0x1;
const CS_HREDRAW: u32 = 0x2;
const CS_DBLCLKS: u32 = 0x8;
const CS_PARENTDC: u32 = 0x80;
const CS_SAVEBITS: u32 = 0x800;
const CS_GLOBALCLASS: u32 = 0x4000;
const FIRST_COMMON_ATOM: u16 = 0xc100;

/// The name of the dialog class, the class of message boxes, pickers and template dialogs.
pub const DIALOG_CLASS: &str = "#32770";

/// `GetClassLongPtrW`'s indices below 0.
mod index {
    pub const MENU_NAME: i32 = -8;
    pub const BACKGROUND: i32 = -10;
    pub const CURSOR: i32 = -12;
    pub const ICON: i32 = -14;
    pub const MODULE: i32 = -16;
    pub const WINDOW_EXTRA: i32 = -18;
    pub const CLASS_EXTRA: i32 = -20;
    pub const PROCEDURE: i32 = -24;
    pub const STYLE: i32 = -26;
    pub const ATOM: i32 = -32;
    pub const SMALL_ICON: i32 = -34;
}

/// A window's class name: a program class's own, or a system class's.
#[derive(Debug, Clone, Copy)]
pub enum ClassLabel<'a> {
    Program(&'a [u16]),
    System(&'static str),
}

impl ClassLabel<'_> {
    /// Whether `name` names it, compared ignoring case.
    pub fn matches(&self, name: &[u16]) -> bool {
        match self {
            ClassLabel::Program(own) => text::compare_ordinal(own, name, true).is_eq(),
            ClassLabel::System(system) => {
                name.len() == system.len()
                    && name.iter().zip(system.bytes()).all(|(&unit, byte)| {
                        text::upper(unit) == u16::from(byte.to_ascii_uppercase())
                    })
            }
        }
    }

    /// Writes it into `out`, cut to fit; the units written.
    pub fn write(&self, out: &mut [u16]) -> usize {
        let mut count = 0;
        let mut put = |unit: u16| {
            if let Some(slot) = out.get_mut(count) {
                *slot = unit;
                count += 1;
            }
        };
        match self {
            ClassLabel::Program(own) => own.iter().for_each(|&unit| put(unit)),
            ClassLabel::System(system) => system.bytes().for_each(|byte| put(u16::from(byte))),
        }
        count
    }
}

fn common_name(kind: CommonKind) -> &'static str {
    match kind {
        CommonKind::StatusBar => "msctls_statusbar32",
        CommonKind::Progress => "msctls_progress32",
        CommonKind::TabControl => "SysTabControl32",
        CommonKind::ListView => "SysListView32",
        CommonKind::Header => "SysHeader32",
        CommonKind::TreeView => "SysTreeView32",
        CommonKind::Toolbar => "ToolbarWindow32",
    }
}

fn system_name(kind: ControlKind) -> &'static str {
    match kind {
        ControlKind::Button => "Button",
        ControlKind::Edit => "Edit",
        ControlKind::Static => "Static",
        ControlKind::ListBox => "ListBox",
        ControlKind::ComboBox => "ComboBox",
        ControlKind::Common(kind) => common_name(kind),
    }
}

/// The class name of `window`.
pub fn class_label<'a>(gui: &'a Gui, window: &Window) -> ClassLabel<'a> {
    match window.procedure {
        Procedure::Program(_) => {
            ClassLabel::Program(gui.classes.name(window.class_atom).unwrap_or(&[]))
        }
        Procedure::Control(kind, _) => ClassLabel::System(system_name(kind)),
        Procedure::MessageBox(_) | Procedure::FilePicker(_) => ClassLabel::System(DIALOG_CLASS),
    }
}

fn system_atom(kind: ControlKind) -> u16 {
    match kind {
        ControlKind::Button => 0x80,
        ControlKind::Edit => 0x81,
        ControlKind::Static => 0x82,
        ControlKind::ListBox => 0x83,
        ControlKind::ComboBox => 0x85,
        ControlKind::Common(kind) => FIRST_COMMON_ATOM + kind as u16,
    }
}

/// What a system class gives its windows, with its atom.
fn system_class(kind: ControlKind) -> (u16, Class) {
    let style = match kind {
        ControlKind::Common(_) => CS_DBLCLKS | CS_GLOBALCLASS | CS_HREDRAW | CS_VREDRAW,
        _ => CS_DBLCLKS | CS_PARENTDC,
    };
    let shape = match kind {
        ControlKind::Edit => cursor::I_BEAM,
        _ => cursor::ARROW,
    };
    let class = Class {
        style,
        procedure: super::subclass::entry_address(),
        cursor: objects::cursor_handle(true, shape),
        ..Class::default()
    };
    (system_atom(kind), class)
}

fn class_of(gui: &Gui, window: &Window) -> Option<(u16, Class)> {
    match window.procedure {
        Procedure::Program(_) => gui.classes.find(ClassName::Atom(window.class_atom)),
        Procedure::Control(kind, _) => Some(system_class(kind)),
        Procedure::MessageBox(_) | Procedure::FilePicker(_) => Some((
            0,
            Class {
                style: CS_DBLCLKS | CS_SAVEBITS,
                cursor: objects::cursor_handle(true, cursor::ARROW),
                ..Class::default()
            },
        )),
    }
}

unsafe extern "efiapi" fn get_class_name_w(window: usize, buffer: *mut u16, capacity: i32) -> i32 {
    let Ok(capacity) = usize::try_from(capacity) else {
        return fail(error::INVALID_PARAMETER, 0);
    };
    if buffer.is_null() || capacity == 0 {
        return fail(error::INSUFFICIENT_BUFFER, 0);
    }
    // SAFETY: `GetClassNameW`'s contract: `buffer` holds `capacity` writable units.
    let out = unsafe { core::slice::from_raw_parts_mut(buffer, capacity) };
    let written = try_with(|gui| {
        let entry = gui.window(window)?;
        let label = class_label(gui, entry);
        let written = label.write(&mut out[..capacity - 1]);
        out[written] = 0;
        Ok(written)
    });
    match written {
        Ok(written) => written as i32,
        Err(code) => fail(code, 0),
    }
}

fn class_long(window: usize, index: i32, size: usize) -> usize {
    let value = try_with(|gui| {
        let entry = gui.window(window)?;
        let (atom, class) = class_of(gui, entry).ok_or(error::INVALID_WINDOW_HANDLE)?;
        Ok(match index {
            index::MENU_NAME => 0,
            index::BACKGROUND => class.background,
            index::CURSOR => class.cursor,
            index::ICON | index::SMALL_ICON => class.icon,
            index::MODULE => class.instance,
            index::WINDOW_EXTRA => class.window_extra as usize,
            index::CLASS_EXTRA => class.class_extra as usize,
            index::PROCEDURE => match entry.procedure {
                Procedure::Program(_) => class.procedure,
                _ => super::subclass::entry_address(),
            },
            index::STYLE => class.style as usize,
            index::ATOM => usize::from(atom),
            offset => {
                let start = usize::try_from(offset).map_err(|_| error::INVALID_INDEX)?;
                if start + size > class.class_extra as usize {
                    return Err(error::INVALID_INDEX);
                }
                0
            }
        })
    });
    match value {
        Ok(value) => {
            super::thread_environment::set_last_error(error::SUCCESS);
            match size {
                4 => value as u32 as usize,
                _ => value,
            }
        }
        Err(code) => fail(code, 0),
    }
}

extern "efiapi" fn get_class_long_ptr_w(window: usize, index: i32) -> usize {
    class_long(window, index, 8)
}

extern "efiapi" fn get_class_long_w(window: usize, index: i32) -> u32 {
    class_long(window, index, 4) as u32
}

/// Win32's `WNDCLASSEXW`, which `GetClassInfoExW` fills.
#[repr(C)]
struct ClassInfo {
    size: u32,
    style: u32,
    procedure: usize,
    class_extra: i32,
    window_extra: i32,
    instance: usize,
    icon: usize,
    cursor: usize,
    background: usize,
    menu_name: usize,
    class_name: *const u16,
    small_icon: usize,
}

fn same_ascii(name: &[u16], wanted: &str) -> bool {
    ClassLabel::System(wanted).matches(name)
}

unsafe extern "efiapi" fn get_class_info_ex_w(
    _instance: usize,
    name: *const u16,
    out: *mut ClassInfo,
) -> Bool {
    if name.is_null() || out.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    let name = match (name as usize) < ATOM_LIMIT {
        true => ClassName::Atom(name as usize as u16),
        // SAFETY: the caller's duty: a NUL-terminated class name.
        false => ClassName::Name(unsafe { super::file::wide_string(name) }),
    };
    if let ClassName::Name(units) = name
        && same_ascii(units, DIALOG_CLASS)
    {
        super::dialog_box::dialog_class();
    }
    let found = with(|gui| gui.classes.find(name))
        .flatten()
        .map(|(atom, class)| (Bool::from(atom), class))
        .or_else(|| match name {
            ClassName::Name(units) => {
                super::controls::system_class(units).map(|kind| (TRUE, system_class(kind).1))
            }
            ClassName::Atom(_) => None,
        });
    let Some((answer, class)) = found else {
        return fail(error::CLASS_DOES_NOT_EXIST, FALSE);
    };
    // SAFETY: the caller passes a writable `WNDCLASSEXW`; its size and name are left as given.
    let info = unsafe { &mut *out };
    info.style = class.style;
    info.procedure = class.procedure;
    info.class_extra = class.class_extra as i32;
    info.window_extra = class.window_extra as i32;
    info.instance = class.instance;
    info.icon = class.icon;
    info.cursor = class.cursor;
    info.background = class.background;
    info.menu_name = 0;
    info.small_icon = class.icon;
    answer
}

unsafe extern "efiapi" fn register_window_message_w(name: *const u16) -> u32 {
    if name.is_null() {
        return fail(error::INVALID_PARAMETER, 0);
    }
    // SAFETY: the caller passes a NUL-terminated name.
    let name = unsafe { super::file::wide_string(name) };
    if name.is_empty() {
        return fail(error::INVALID_PARAMETER, 0);
    }
    let atom = with(|gui| gui.extras().and_then(|extras| extras.atoms.add(name))).flatten();
    match atom {
        Some(atom) => u32::from(atom),
        None => fail(error::NOT_ENOUGH_MEMORY, 0),
    }
}

pub(super) const EXPORTS: &[Export] = exports![
    b"GetClassNameW" => get_class_name_w,
    b"GetClassLongPtrW" => get_class_long_ptr_w,
    b"GetClassLongW" => get_class_long_w,
    b"GetClassInfoExW" => get_class_info_ex_w,
    b"RegisterWindowMessageW" => register_window_message_w,
];
