//! `DefDlgProcW` and `DefDlgProcA` (D174, `docs/ux/windows-controls-v0.3.md` section 11.2): the
//! window procedure of a dialog a program builds itself, a class whose procedure is it and whose
//! extra bytes hold the dialog procedure (`DWLP_DLGPROC`) and the message result
//! (`DWLP_MSGRESULT`). Each message goes to the dialog procedure first; what it leaves gets the
//! dialog's defaults: the grey background, `WM_CLOSE` as `IDCANCEL`, the font kept, the focus
//! given to the first control that takes it, and the default button's id.

use super::ansi;
use super::gui::{self, with};
use super::user32::def_window_proc_w;
use super::{call_program, dialog, paint};
use crate::gui::message;
use crate::gui::windows;
use crate::library::Export;
use eggos_windows_gui::controls::LIGHT;

const DWLP_MSGRESULT: i32 = 0;
const DWLP_DLGPROC: i32 = 8;
const POINTER_SIZE: usize = 8;
const INIT_DIALOG: u32 = 0x0110;
const NEXT_DIALOG_CONTROL: u32 = 0x0028;
const DM_GETDEFID: u32 = 0x0400;
const DM_SETDEFID: u32 = 0x0401;
const DC_HASDEFID: usize = 0x534b;
const BN_CLICKED: usize = 0;
const COMPARE_ITEM: u32 = 0x0039;
const VIRTUAL_KEY_TO_ITEM: u32 = 0x002e;
const CHARACTER_TO_ITEM: u32 = 0x002f;
const QUERY_DRAG_ICON: u32 = 0x0037;
const FIRST_CONTROL_COLOUR: u32 = 0x0132;
const LAST_CONTROL_COLOUR: u32 = 0x0138;

fn answers_directly(number: u32) -> bool {
    matches!(
        number,
        INIT_DIALOG | COMPARE_ITEM | VIRTUAL_KEY_TO_ITEM | CHARACTER_TO_ITEM | QUERY_DRAG_ICON
    ) || (FIRST_CONTROL_COLOUR..=LAST_CONTROL_COLOUR).contains(&number)
}

fn long(window: usize, index: i32) -> usize {
    with(|gui| gui.windows.get(window)?.long(index, POINTER_SIZE))
        .flatten()
        .unwrap_or(0)
}

fn focus_first_control(window: usize) {
    let first = with(|gui| {
        let root = windows::root_of(&gui.windows, window)?;
        let focus = gui.windows.get(root)?.focus;
        let has_focus = focus != 0 && windows::is_in_tree(&gui.windows, window, focus);
        match has_focus {
            true => None,
            false => windows::next_tab_stop(&gui.windows, window, 0, false),
        }
    })
    .flatten();
    if let Some(first) = first {
        dialog::set_focus(first);
    }
}

fn next_control(window: usize, wparam: usize, lparam: usize) {
    if lparam & 0xffff != 0 {
        dialog::set_focus(wparam);
        return;
    }
    let next = with(|gui| {
        let root = windows::root_of(&gui.windows, window)?;
        let from = gui.key_target(root);
        windows::next_tab_stop(&gui.windows, window, from, wparam != 0)
    })
    .flatten();
    if let Some(next) = next {
        dialog::set_focus(next);
    }
}

fn dialog_default(window: usize, number: u32, wparam: usize, lparam: usize, narrow: bool) -> usize {
    match number {
        message::ERASE_BACKGROUND => usize::from(paint::fill_clip(wparam, LIGHT.surface)),
        message::CLOSE => {
            let cancel = dialog::dialog_item(window, message::ID_CANCEL as i32).unwrap_or(0);
            gui::send(
                window,
                message::COMMAND,
                message::word_pair(message::ID_CANCEL, BN_CLICKED),
                cancel,
            );
            0
        }
        message::SET_FONT => {
            with(|gui| {
                if let Some(entry) = gui.windows.get_mut(window) {
                    entry.font = wparam;
                }
            });
            0
        }
        message::GET_FONT => {
            with(|gui| gui.windows.get(window).map_or(0, |entry| entry.font)).unwrap_or(0)
        }
        message::SET_FOCUS => {
            focus_first_control(window);
            0
        }
        NEXT_DIALOG_CONTROL => {
            next_control(window, wparam, lparam);
            0
        }
        DM_GETDEFID => match dialog::default_button(window) {
            Some((_, id)) => message::word_pair(id, DC_HASDEFID),
            None => 0,
        },
        DM_SETDEFID | INIT_DIALOG => 0,
        _ => match narrow {
            true => ansi::from_narrow(window, number, wparam, lparam, &|w, n, wp, lp| {
                def_window_proc_w(w, n, wp, lp)
            }),
            false => def_window_proc_w(window, number, wparam, lparam),
        },
    }
}

fn default_dialog_procedure(
    window: usize,
    number: u32,
    wparam: usize,
    lparam: usize,
    narrow: bool,
) -> usize {
    let procedure = long(window, DWLP_DLGPROC);
    if procedure != 0 {
        // SAFETY: `DWLP_DLGPROC` holds the dialog procedure the program set, which Windows calls
        // with these four arguments.
        let answer = unsafe { call_program(procedure, [window, number as usize, wparam, lparam]) };
        if answers_directly(number) {
            if answer != 0 {
                return answer;
            }
        } else if answer as u32 != 0 {
            return long(window, DWLP_MSGRESULT);
        }
    }
    dialog_default(window, number, wparam, lparam, narrow)
}

/// `DefDlgProcW`.
pub extern "efiapi" fn def_dlg_proc_w(
    window: usize,
    number: u32,
    wparam: usize,
    lparam: usize,
) -> usize {
    default_dialog_procedure(window, number, wparam, lparam, false)
}

/// `DefDlgProcA`: its messages carry ANSI text, which its dialog procedure gets as it is.
pub extern "efiapi" fn def_dlg_proc_a(
    window: usize,
    number: u32,
    wparam: usize,
    lparam: usize,
) -> usize {
    default_dialog_procedure(window, number, wparam, lparam, true)
}

pub(super) const EXPORTS: &[Export] = exports![
    b"DefDlgProcW" => def_dlg_proc_w,
    b"DefDlgProcA" => def_dlg_proc_a,
];
