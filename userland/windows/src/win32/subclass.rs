//! Subclassing (D178): a program replaces a window's procedure with `SetWindowLongPtrW`'s
//! `GWLP_WNDPROC` and passes what it does not handle to the old one with `CallWindowProcW`, on
//! its own windows and on the system's controls alike. A control's procedure, as the program
//! sees it, is a layer entry that runs the control's own; once the program put its procedure
//! in its place, every message to the control goes to the program's first (`gui::send`).

use super::gui::with;
use super::{call_program, controls};
use crate::gui::classes::ClassName;
use crate::gui::windows::Procedure;
use crate::library::Export;

extern "efiapi" fn control_window_procedure(
    window: usize,
    number: u32,
    wparam: usize,
    lparam: usize,
) -> usize {
    let kind = with(|gui| match gui.windows.get(window)?.procedure {
        Procedure::Control(kind, _) => Some(kind),
        _ => None,
    })
    .flatten();
    match kind {
        Some(kind) => controls::procedure(window, kind, number, wparam, lparam),
        None => 0,
    }
}

/// The address `GetWindowLongPtrW(GWLP_WNDPROC)` gives for a system control the program has
/// not subclassed: an entry the program can call as any window procedure.
pub fn entry_address() -> usize {
    let entry: *const () = export_entry!(control_window_procedure);
    entry as usize
}

fn is_ansi_procedure(window: usize, procedure: usize) -> bool {
    with(|gui| {
        let atom = gui.windows.get(window)?.class_atom;
        let (_, class) = gui.classes.find(ClassName::Atom(atom))?;
        Some(class.ansi && class.procedure == procedure)
    })
    .flatten()
    .unwrap_or(false)
}

extern "efiapi" fn call_window_proc_w(
    procedure: usize,
    window: usize,
    number: u32,
    wparam: usize,
    lparam: usize,
) -> usize {
    if procedure == 0 {
        return 0;
    }
    if procedure == entry_address() {
        return control_window_procedure(window, number, wparam, lparam);
    }
    if is_ansi_procedure(window, procedure) {
        return super::ansi::call_procedure(procedure, window, number, wparam, lparam);
    }
    // SAFETY: the program passes a window procedure it took from `GetWindowLongPtrW` or its own,
    // which Windows calls with these four arguments.
    unsafe { call_program(procedure, [window, number as usize, wparam, lparam]) }
}

pub(super) const EXPORTS: &[Export] = exports![
    b"CallWindowProcW" => call_window_proc_w,
];
