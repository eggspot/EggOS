//! `msctls_progress32` (D178, design section 22): the layer's procedure over
//! `eggos_windows_gui`'s progress bar model, answering the `PBM_*` messages and painting on
//! `WM_PAINT`; it is never focused and sends no notifications.

use super::super::gui::Gui;
use super::{Model, allocate, default_procedure, invalidate, paint_with, read_state, with_state};
use crate::gui::message;
use crate::gui::windows::{CommonKind, Window};
use eggos_element_tree::Role;
use eggos_windows_gui::controls::common::COMMON_LIGHT;
use eggos_windows_gui::controls::progress::{self, Progress};

const WM_USER: u32 = 0x0400;
const PBM_SETRANGE: u32 = WM_USER + 1;
const PBM_SETPOS: u32 = WM_USER + 2;
const PBM_DELTAPOS: u32 = WM_USER + 3;
const PBM_SETSTEP: u32 = WM_USER + 4;
const PBM_STEPIT: u32 = WM_USER + 5;
const PBM_SETRANGE32: u32 = WM_USER + 6;
const PBM_GETRANGE: u32 = WM_USER + 7;
const PBM_GETPOS: u32 = WM_USER + 8;
const PBM_SETBARCOLOR: u32 = WM_USER + 9;
const PBM_SETMARQUEE: u32 = WM_USER + 10;
const PBM_GETSTEP: u32 = WM_USER + 13;
const PBM_GETBKCOLOR: u32 = WM_USER + 14;
const PBM_GETBARCOLOR: u32 = WM_USER + 15;
const PBM_SETSTATE: u32 = WM_USER + 16;
const PBM_GETSTATE: u32 = WM_USER + 17;
const PBM_SETBKCOLOR: u32 = 0x2001;

impl Model for Progress {
    const KIND: CommonKind = CommonKind::Progress;
}

#[repr(C)]
struct ProgressRange {
    low: i32,
    high: i32,
}

/// A new progress bar's state.
pub(super) fn new_state(_style: u32) -> Option<usize> {
    allocate(Progress::new())
}

fn word_range((min, max): (i32, i32)) -> usize {
    message::word_pair(min as u16 as usize, max as u16 as usize)
}

fn signed(value: i32) -> usize {
    value as isize as usize
}

/// The procedure of every progress bar.
pub(super) fn procedure(window: usize, number: u32, wparam: usize, lparam: usize) -> usize {
    let outcome = match number {
        message::PAINT => {
            paint_with::<Progress>(window, |surface, rect, entry, model, _| {
                progress::paint(surface, rect, model, entry.style, &COMMON_LIGHT)
            });
            return 0;
        }
        message::SIZE | message::ENABLE => {
            invalidate(window);
            return 0;
        }
        PBM_SETRANGE..=PBM_GETSTATE | PBM_SETBKCOLOR => {
            with_state::<Progress, _>(window, |_, model| answer(model, number, wparam, lparam))
        }
        _ => return default_procedure(window, number, wparam, lparam),
    };
    let Some((result, changed)) = outcome else {
        return 0;
    };
    if changed {
        invalidate(window);
    }
    result
}

fn answer(model: &mut Progress, number: u32, wparam: usize, lparam: usize) -> (usize, bool) {
    match number {
        PBM_SETRANGE => {
            let (low, high) = (lparam as u16 as i32, (lparam >> 16) as u16 as i32);
            (word_range(model.set_range(low, high)), true)
        }
        PBM_SETRANGE32 => (
            word_range(model.set_range(wparam as i32, lparam as i32)),
            true,
        ),
        PBM_SETPOS => (signed(model.set_position(wparam as i32)), true),
        PBM_DELTAPOS => (signed(model.delta(wparam as i32)), true),
        PBM_SETSTEP => (signed(model.set_step(wparam as i32)), false),
        PBM_STEPIT => (signed(model.step_it()), true),
        PBM_GETRANGE => {
            let (low, high) = model.range();
            if lparam != 0 {
                // SAFETY: `PBM_GETRANGE`'s contract: `lParam` is 0 or a writable `PBRANGE`.
                unsafe { (lparam as *mut ProgressRange).write(ProgressRange { low, high }) };
            }
            let wanted = match wparam {
                0 => high,
                _ => low,
            };
            (signed(wanted), false)
        }
        PBM_GETPOS => (signed(model.position()), false),
        PBM_GETSTEP => (signed(model.step()), false),
        PBM_SETBARCOLOR => (model.set_bar_colour(lparam as u32) as usize, true),
        PBM_SETBKCOLOR => (model.set_back_colour(lparam as u32) as usize, true),
        PBM_GETBARCOLOR => (model.bar_colour() as usize, false),
        PBM_GETBKCOLOR => (model.back_colour() as usize, false),
        PBM_SETMARQUEE => {
            model.marquee = wparam != 0;
            (1, true)
        }
        PBM_SETSTATE => (model.set_state(wparam as u32) as usize, true),
        PBM_GETSTATE => (model.state() as usize, false),
        _ => (0, false),
    }
}

/// A progress bar's role in agents' tree.
pub(super) fn role(_window: &Window) -> (Role, u16) {
    (Role::Progress, 0)
}

/// A progress bar's value in agents' tree: how much is done, in thousandths, as decimal text.
pub(super) fn value(gui: &Gui, window: usize, out: &mut [u8]) -> usize {
    let Some(per_mille) = read_state::<Progress, _>(gui, window, |_, model| model.per_mille())
    else {
        return 0;
    };
    let mut digits = [0u8; 12];
    let mut at = digits.len();
    let mut rest = per_mille.unsigned_abs();
    loop {
        at -= 1;
        digits[at] = b'0' + (rest % 10) as u8;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    if per_mille < 0 {
        at -= 1;
        digits[at] = b'-';
    }
    let text = &digits[at..];
    let length = text.len().min(out.len());
    out[..length].copy_from_slice(&text[..length]);
    length
}
