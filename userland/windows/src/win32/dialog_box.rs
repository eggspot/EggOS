//! Dialogs from templates (D178, `docs/ux/windows-controls-v0.3.md` section 11.4):
//! `DialogBoxParamW`, `DialogBoxIndirectParamW`, `CreateDialogParamW` and
//! `CreateDialogIndirectParamW` read a `DLGTEMPLATE` or `DLGTEMPLATEEX` (an `RT_DIALOG` resource
//! of the module, or the program's memory), create the dialog as an owned window of the dialog
//! class `#32770` (`DefDlgProcW` with its dialog procedure in `DWLP_DLGPROC`) and its controls,
//! dialog units becoming pixels by the base units (`MapDialogRect`), send `WM_INITDIALOG` and
//! show it; `DialogBox*` then runs the modal loop with its owner disabled until `EndDialog`.
//! Template menus and child-style templates are not supported.

use super::dialog::{dialog_item, dialog_message, enable, set_focus};
use super::gui::{self, Next, try_with, with};
use super::resource::{self, Name, kind, name_from};
use super::user32::{
    CAPTION, FRAME, Win32Rect, create_window_ex_w, destroy_tree, dispatch, show_window, translate,
};
use super::window_class::DIALOG_CLASS;
use super::{Bool, FALSE, TRUE, error, fail};
use crate::gui::classes::{Class, ClassName};
use crate::gui::dialog_template::{self as template, Field, TemplateBytes};
use crate::gui::message;
use crate::gui::objects::{self, cursor};
use crate::gui::queue::Filter;
use crate::gui::windows;
use crate::library::Export;
use eggos_windows_gui::text::{CELL_HEIGHT, CELL_WIDTH};

const WS_POPUP: u32 = 0x8000_0000;
const WS_CHILD: u32 = 0x4000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_CAPTION: u32 = 0x00c0_0000;
const WS_SYSMENU: u32 = 0x0008_0000;
const DS_ABSALIGN: u32 = 0x1;
const DS_CENTER: u32 = 0x800;
const DS_CENTERMOUSE: u32 = 0x1000;
const CW_USE_DEFAULT: i32 = i32::MIN;
const SW_HIDE: i32 = 0;
const SW_SHOW: i32 = 5;
const CS_DBLCLKS: u32 = 0x8;
const CS_SAVEBITS: u32 = 0x800;
const DLGWINDOWEXTRA: u32 = 30;
const DWLP_DLGPROC: i32 = 8;
const POINTER_SIZE: usize = 8;
const BM_SETCHECK: u32 = 0x00f1;
const NAME_UNITS: usize = 256;
const ORDINAL_CLASSES: [(u16, &str); 6] = [
    (0x80, "BUTTON"),
    (0x81, "EDIT"),
    (0x82, "STATIC"),
    (0x83, "LISTBOX"),
    (0x84, "SCROLLBAR"),
    (0x85, "COMBOBOX"),
];

/// A template in the program's memory (its own or a resource of its image), read field by
/// field as far as the template goes.
struct ProgramTemplate(usize);

impl TemplateBytes for ProgramTemplate {
    fn byte(&self, at: usize) -> Option<u8> {
        let address = self.0.checked_add(at)?;
        // SAFETY: a `ProgramTemplate` is made only over a template the program passed or one of
        // its resources, and the parser reads each field after the one before it, never past
        // the template's last field, as Windows reads it.
        Some(unsafe { (address as *const u8).read() })
    }
}

fn dialog_units_x(units: i16) -> i32 {
    i32::from(units) * CELL_WIDTH / 4
}

fn dialog_units_y(units: i16) -> i32 {
    i32::from(units) * CELL_HEIGHT / 8
}

fn ascii_name(name: &str, out: &mut [u16; NAME_UNITS + 1]) -> *const u16 {
    let mut length = 0;
    for (slot, byte) in out.iter_mut().zip(name.bytes()) {
        *slot = u16::from(byte);
        length += 1;
    }
    out[length] = 0;
    out.as_ptr()
}

fn text_field(
    bytes: &ProgramTemplate,
    field: Field,
    out: &mut [u16; NAME_UNITS + 1],
) -> *const u16 {
    let length = template::copy_text(bytes, field, &mut out[..NAME_UNITS]);
    out[length] = 0;
    out.as_ptr()
}

/// The atom of the dialog class `#32770`, registered on its first use with `DefDlgProcW` as
/// its procedure and room for the dialog's values.
pub(super) fn dialog_class() -> Option<u16> {
    let mut units = [0u16; NAME_UNITS + 1];
    ascii_name(DIALOG_CLASS, &mut units);
    let name = &units[..DIALOG_CLASS.len()];
    if let Some((atom, _)) = with(|gui| gui.classes.find(ClassName::Name(name))).flatten() {
        return Some(atom);
    }
    let procedure = super::resolve(b"user32", b"DefDlgProcW")? as usize;
    let class = Class {
        style: CS_DBLCLKS | CS_SAVEBITS,
        procedure,
        window_extra: DLGWINDOWEXTRA,
        cursor: objects::cursor_handle(true, cursor::ARROW),
        ..Class::default()
    };
    with(|gui| {
        gui.classes.register(name, class).ok().or_else(|| {
            gui.classes
                .find(ClassName::Name(name))
                .map(|(atom, _)| atom)
        })
    })
    .flatten()
}

fn position(dialog: &template::Dialog, owner: usize) -> (i32, i32) {
    if dialog.style & (DS_CENTER | DS_CENTERMOUSE) != 0 {
        return (CW_USE_DEFAULT, CW_USE_DEFAULT);
    }
    let (x, y) = (dialog_units_x(dialog.x), dialog_units_y(dialog.y));
    let origin = match owner != 0 && dialog.style & DS_ABSALIGN == 0 {
        true => super::placement::client_origin(owner).unwrap_or((0, 0)),
        false => (0, 0),
    };
    (origin.0.saturating_add(x), origin.1.saturating_add(y))
}

fn create_controls(
    bytes: &ProgramTemplate,
    dialog: &template::Dialog,
    window: usize,
    instance: usize,
) -> Result<(), Option<u32>> {
    let mut at = dialog.first_item;
    for _ in 0..dialog.count {
        let item = template::item(bytes, at, dialog.extended).ok_or(Some(error::INVALID_DATA))?;
        at = item.next;
        let mut class_units = [0u16; NAME_UNITS + 1];
        let class = match item.class {
            Field::Ordinal(ordinal) => {
                match ORDINAL_CLASSES.iter().find(|entry| entry.0 == ordinal) {
                    Some(&(_, name)) => ascii_name(name, &mut class_units),
                    None => return Err(Some(error::CANNOT_FIND_WINDOW_CLASS)),
                }
            }
            Field::Text { .. } => text_field(bytes, item.class, &mut class_units),
            Field::Absent => return Err(Some(error::INVALID_DATA)),
        };
        let mut title_units = [0u16; NAME_UNITS + 1];
        let title = text_field(bytes, item.title, &mut title_units);
        let data = item.data.map_or(0, |(offset, _)| bytes.0 + offset);
        // SAFETY: the class name and title are NUL-terminated strings on this stack frame, and
        // the creation data is the template's own, as `CreateWindowExW` takes them.
        let control = unsafe {
            create_window_ex_w(
                item.ex_style,
                class,
                title,
                item.style | WS_CHILD,
                dialog_units_x(item.x),
                dialog_units_y(item.y),
                dialog_units_x(item.width),
                dialog_units_y(item.height),
                window,
                item.id as usize,
                instance,
                data,
            )
        };
        if control == 0 {
            return Err(None);
        }
    }
    Ok(())
}

/// Creates the dialog of the template at `base` owned by `owner`, with dialog procedure
/// `procedure` and `WM_INITDIALOG`'s `parameter`; 0, with the error set, when it cannot.
fn create(base: usize, instance: usize, owner: usize, procedure: usize, parameter: usize) -> usize {
    let bytes = ProgramTemplate(base);
    let Some(dialog) = template::dialog(&bytes) else {
        return fail(error::INVALID_DATA, 0);
    };
    if dialog.menu != Field::Absent || dialog.style & WS_CHILD != 0 {
        return fail(error::NOT_SUPPORTED, 0);
    }
    let mut class_units = [0u16; NAME_UNITS + 1];
    let class = match dialog.class {
        Field::Absent => match dialog_class() {
            Some(atom) => usize::from(atom) as *const u16,
            None => return fail(error::NOT_ENOUGH_MEMORY, 0),
        },
        Field::Ordinal(atom) => usize::from(atom) as *const u16,
        Field::Text { .. } => text_field(&bytes, dialog.class, &mut class_units),
    };
    let mut title_units = [0u16; NAME_UNITS + 1];
    let title = text_field(&bytes, dialog.title, &mut title_units);
    let style = (dialog.style | WS_POPUP | WS_CAPTION | WS_SYSMENU) & !WS_VISIBLE;
    let (x, y) = position(&dialog, owner);
    let width = dialog_units_x(dialog.width).saturating_add(2 * FRAME);
    let height = dialog_units_y(dialog.height).saturating_add(CAPTION + 2 * FRAME);
    // SAFETY: the class is an atom or a NUL-terminated name on this stack frame and the title a
    // NUL-terminated string on it.
    let window = unsafe {
        create_window_ex_w(
            dialog.ex_style,
            class,
            title,
            style,
            x,
            y,
            width,
            height,
            owner,
            0,
            instance,
            parameter,
        )
    };
    if window == 0 {
        return 0;
    }
    with(|gui| {
        if let Some(entry) = gui.windows.get_mut(window) {
            let _ = entry.set_long(DWLP_DLGPROC, POINTER_SIZE, procedure);
        }
    });
    if let Err(code) = create_controls(&bytes, &dialog, window, instance) {
        destroy_tree(window);
        return match code {
            Some(code) => fail(code, 0),
            None => 0,
        };
    }
    let first = with(|gui| windows::next_tab_stop(&gui.windows, window, 0, false))
        .flatten()
        .unwrap_or(0);
    let focus_first = gui::send(window, message::INIT_DIALOG, first, parameter) != 0;
    let alive = with(|gui| {
        gui.windows
            .get(window)
            .is_some_and(|entry| !entry.destroying)
    });
    if alive != Some(true) {
        return 0;
    }
    if focus_first && first != 0 {
        set_focus(first);
    }
    if dialog.style & WS_VISIBLE != 0 {
        show_window(window, SW_SHOW);
    }
    window
}

fn resource_template(module: usize, name: *const u16) -> Option<usize> {
    // SAFETY: the dialog functions' contract: a resource id or a NUL-terminated name.
    let name = unsafe { name_from(name as usize) };
    resource::find(module, Name::Id(kind::DIALOG), name).map(|(address, _)| address)
}

fn run_modal(window: usize, owner: usize) -> isize {
    let owner_root = with(|gui| windows::root_of(&gui.windows, owner)).flatten();
    let reenable = owner_root.is_some_and(|root| enable(root, false) == Ok(false));
    show_window(window, SW_SHOW);
    let ended = |gui: &gui::Gui| {
        gui.windows
            .get(window)
            .is_none_or(|entry| entry.dialog_end.is_some() || entry.destroying)
    };
    loop {
        match gui::wait_for(Filter::default(), true, true, &ended) {
            Next::Done | Next::Nothing => break,
            Next::Message(next) if next.message == message::QUIT => {
                with(|gui| gui.queue.quit(next.thread, next.wparam));
                gui::wake();
                break;
            }
            Next::Message(next) => {
                if !dialog_message(window, &next) {
                    translate(&next);
                    dispatch(&next);
                }
            }
        }
    }
    let result = with(|gui| gui.windows.get(window).and_then(|entry| entry.dialog_end))
        .flatten()
        .unwrap_or(0);
    if let (true, Some(root)) = (reenable, owner_root) {
        let _ = enable(root, true);
    }
    destroy_tree(window);
    result
}

fn dialog_box(
    base: Option<usize>,
    instance: usize,
    owner: usize,
    procedure: usize,
    parameter: usize,
) -> isize {
    let Some(base) = base else {
        return fail(error::RESOURCE_NAME_NOT_FOUND, -1);
    };
    if owner != 0 && try_with(|gui| gui.window(owner).map(|_| ())).is_err() {
        return fail(error::INVALID_WINDOW_HANDLE, 0);
    }
    match create(base, instance, owner, procedure, parameter) {
        0 => -1,
        window => run_modal(window, owner),
    }
}

unsafe extern "efiapi" fn dialog_box_param_w(
    instance: usize,
    name: *const u16,
    owner: usize,
    procedure: usize,
    parameter: usize,
) -> isize {
    dialog_box(
        resource_template(instance, name),
        instance,
        owner,
        procedure,
        parameter,
    )
}

extern "efiapi" fn dialog_box_indirect_param_w(
    instance: usize,
    template: usize,
    owner: usize,
    procedure: usize,
    parameter: usize,
) -> isize {
    dialog_box(
        (template != 0).then_some(template),
        instance,
        owner,
        procedure,
        parameter,
    )
}

unsafe extern "efiapi" fn create_dialog_param_w(
    instance: usize,
    name: *const u16,
    owner: usize,
    procedure: usize,
    parameter: usize,
) -> usize {
    match resource_template(instance, name) {
        Some(base) => create(base, instance, owner, procedure, parameter),
        None => fail(error::RESOURCE_NAME_NOT_FOUND, 0),
    }
}

extern "efiapi" fn create_dialog_indirect_param_w(
    instance: usize,
    template: usize,
    owner: usize,
    procedure: usize,
    parameter: usize,
) -> usize {
    match template {
        0 => fail(error::INVALID_PARAMETER, 0),
        template => create(template, instance, owner, procedure, parameter),
    }
}

extern "efiapi" fn end_dialog(window: usize, result: isize) -> Bool {
    let ended = try_with(|gui| {
        gui.window_mut(window)?.dialog_end = Some(result);
        Ok(())
    });
    match ended {
        Ok(()) => {
            show_window(window, SW_HIDE);
            gui::wake();
            TRUE
        }
        Err(code) => fail(code, FALSE),
    }
}

unsafe extern "efiapi" fn map_dialog_rect(window: usize, rect: *mut Win32Rect) -> Bool {
    if rect.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    if let Err(code) = try_with(|gui| gui.window(window).map(|_| ())) {
        return fail(code, FALSE);
    }
    // SAFETY: the caller passes a readable and writable `RECT`.
    let rect = unsafe { &mut *rect };
    let x = |units: i32| units.saturating_mul(CELL_WIDTH) / 4;
    let y = |units: i32| units.saturating_mul(CELL_HEIGHT) / 8;
    rect.left = x(rect.left);
    rect.right = x(rect.right);
    rect.top = y(rect.top);
    rect.bottom = y(rect.bottom);
    TRUE
}

extern "efiapi" fn check_radio_button(dialog: usize, first: i32, last: i32, checked: i32) -> Bool {
    if let Err(code) = try_with(|gui| gui.window(dialog).map(|_| ())) {
        return fail(code, FALSE);
    }
    for id in first..=last {
        if let Some(item) = dialog_item(dialog, id) {
            gui::send(item, BM_SETCHECK, usize::from(id == checked), 0);
        }
    }
    TRUE
}

pub(super) const EXPORTS: &[Export] = exports![
    b"DialogBoxParamW" => dialog_box_param_w,
    b"DialogBoxIndirectParamW" => dialog_box_indirect_param_w,
    b"CreateDialogParamW" => create_dialog_param_w,
    b"CreateDialogIndirectParamW" => create_dialog_indirect_param_w,
    b"EndDialog" => end_dialog,
    b"MapDialogRect" => map_dialog_rect,
    b"CheckRadioButton" => check_radio_button,
];
