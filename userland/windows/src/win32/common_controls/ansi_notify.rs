//! Notifications to parents that want ANSI (D747): before a control's `WM_NOTIFY` goes to its
//! parent, the parent is asked `WM_NOTIFYFORMAT`; when it answers `NFR_ANSI` (its window
//! class is an ANSI one), each notification that has an `A` form goes with that code and its
//! text narrowed to the layer's narrow text (UTF-8, D178): text the control gives is copied
//! narrow, a buffer the parent fills is lent narrow and widened back into the control's
//! buffer, and a string the parent points at instead is widened into it too. Tooltips'
//! `TTN_GETDISPINFO`, whose `A` structure has a narrower inline buffer, is rebuilt in its own
//! layout. Codes without text (selection and expansion changes, header tracking) only change
//! their number.

use super::super::ansi;
use super::super::gui;
use crate::text;
use alloc::vec::Vec;

const WM_NOTIFY: u32 = super::WM_NOTIFY;
const NF_QUERY: usize = 3;
const NFR_ANSI: usize = 1;
const CODE_OFFSET: usize = 16;
const TEXT_CALLBACK: usize = usize::MAX;
const TTN_GETDISPINFOW: i32 = -530;
const TTN_GETDISPINFOA: i32 = -520;
const TIP_UNITS: usize = 80;
const TIP_NARROW_BYTES: usize = 136;
const TIP_TEXT: usize = 24;
const TIP_INLINE: usize = 32;
const TIP_INSTANCE_WIDE: usize = 192;
const TIP_INSTANCE_NARROW: usize = 112;
const TIP_TAIL_BYTES: usize = 24;
const LONGEST_NARROW: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Text {
    None,
    In { text: usize },
    Out { text: usize, room: usize },
}

const FORMS: [(i32, i32, Text); 30] = [
    (-177, -150, Text::Out { text: 48, room: 56 }),
    (-178, -151, Text::In { text: 48 }),
    (-175, -105, Text::In { text: 48 }),
    (-176, -106, Text::In { text: 48 }),
    (-158, -157, Text::Out { text: 32, room: 40 }),
    (-179, -152, Text::In { text: 40 }),
    (-450, -401, Text::None),
    (-451, -402, Text::None),
    (-452, -403, Text::Out { text: 48, room: 56 }),
    (-453, -404, Text::In { text: 48 }),
    (-454, -405, Text::None),
    (-455, -406, Text::None),
    (-456, -407, Text::None),
    (-457, -408, Text::None),
    (-458, -409, Text::None),
    (-459, -410, Text::In { text: 48 }),
    (-460, -411, Text::In { text: 48 }),
    (-414, -413, Text::Out { text: 24, room: 32 }),
    (-320, -300, Text::None),
    (-321, -301, Text::None),
    (-322, -302, Text::None),
    (-323, -303, Text::None),
    (-325, -305, Text::None),
    (-326, -306, Text::None),
    (-327, -307, Text::None),
    (-328, -308, Text::None),
    (-329, -309, Text::None),
    (-717, -716, Text::Out { text: 48, room: 56 }),
    (-719, -718, Text::Out { text: 24, room: 32 }),
    (-720, -700, Text::None),
];

fn wants_ansi(parent: usize) -> bool {
    gui::send(parent, super::WM_NOTIFYFORMAT, 0, NF_QUERY) == NFR_ANSI
}

fn read_word(data: *mut u8, offset: usize) -> usize {
    // SAFETY: the callers pass a notification structure holding a word at `offset`.
    unsafe { data.add(offset).cast::<usize>().read_unaligned() }
}

fn write_word(data: *mut u8, offset: usize, value: usize) {
    // SAFETY: as in `read_word`.
    unsafe { data.add(offset).cast::<usize>().write_unaligned(value) }
}

fn read_i32(data: *mut u8, offset: usize) -> i32 {
    // SAFETY: as in `read_word`, for a 32-bit field.
    unsafe { data.add(offset).cast::<i32>().read_unaligned() }
}

fn write_i32(data: *mut u8, offset: usize, value: i32) {
    // SAFETY: as in `read_i32`.
    unsafe { data.add(offset).cast::<i32>().write_unaligned(value) }
}

fn wide_text<'a>(pointer: usize) -> &'a [u16] {
    // SAFETY: a control's text field points at a NUL-terminated wide string.
    unsafe { super::list_view::text_at(pointer) }
}

fn narrow_text<'a>(pointer: usize) -> &'a [u8] {
    // SAFETY: the parent's text field points at a NUL-terminated narrow string.
    let bytes = unsafe { ansi::c_text(pointer as *const u8) };
    &bytes[..bytes.len().min(LONGEST_NARROW)]
}

fn widen_into(bytes: &[u8], target: usize, room: usize) {
    if target == 0 || room == 0 {
        return;
    }
    // SAFETY: the control's buffer of `room` units.
    let out = unsafe { core::slice::from_raw_parts_mut(target as *mut u16, room) };
    let written = text::write_utf16(bytes, &mut out[..room - 1]);
    out[written] = 0;
}

fn send_tip(parent: usize, id: usize, data: *mut u8) -> usize {
    let mut narrow = [0u8; TIP_NARROW_BYTES];
    // SAFETY: a `NMTTDISPINFOW`, 216 bytes long; the copies stay inside both structures.
    unsafe {
        core::ptr::copy_nonoverlapping(data, narrow.as_mut_ptr(), TIP_TEXT);
        core::ptr::copy_nonoverlapping(
            data.add(TIP_INSTANCE_WIDE),
            narrow.as_mut_ptr().add(TIP_INSTANCE_NARROW),
            TIP_TAIL_BYTES,
        );
    }
    let place = narrow.as_mut_ptr();
    write_i32(place, CODE_OFFSET, TTN_GETDISPINFOA);
    write_word(place, TIP_TEXT, 0);
    let answer = gui::send(parent, WM_NOTIFY, id, place as usize);
    let given = read_word(place, TIP_TEXT);
    let inline = place as usize + TIP_INLINE;
    let wide_inline = data as usize + TIP_INLINE;
    match given {
        0 => write_word(data, TIP_TEXT, 0),
        resource if resource < 0x1_0000 => write_word(data, TIP_TEXT, resource),
        pointer => {
            let bytes = match pointer == inline {
                true => {
                    let end = narrow[TIP_INLINE..TIP_INLINE + TIP_UNITS]
                        .iter()
                        .position(|&byte| byte == 0)
                        .unwrap_or(TIP_UNITS);
                    narrow[TIP_INLINE..TIP_INLINE + end].to_vec()
                }
                false => narrow_text(pointer).to_vec(),
            };
            widen_into(&bytes, wide_inline, TIP_UNITS);
            write_word(data, TIP_TEXT, wide_inline);
        }
    }
    // SAFETY: as above, the instance and flags the parent may have set.
    unsafe {
        core::ptr::copy_nonoverlapping(
            narrow.as_ptr().add(TIP_INSTANCE_NARROW),
            data.add(TIP_INSTANCE_WIDE),
            TIP_TAIL_BYTES,
        );
    }
    answer
}

/// Sends `data`, a notification whose header names `code`, from a control numbered `id` to
/// `parent`, in the `A` form when the parent wants ANSI.
pub(super) fn send(parent: usize, id: usize, data: *mut u8, code: i32) -> usize {
    let form = FORMS.iter().find(|(wide, _, _)| *wide == code).copied();
    let tip = code == TTN_GETDISPINFOW;
    if (form.is_none() && !tip) || !wants_ansi(parent) {
        return gui::send(parent, WM_NOTIFY, id, data as usize);
    }
    if tip {
        return send_tip(parent, id, data);
    }
    let Some((_, narrow_code, text)) = form else {
        return gui::send(parent, WM_NOTIFY, id, data as usize);
    };
    write_i32(data, CODE_OFFSET, narrow_code);
    let answer = match text {
        Text::None => gui::send(parent, WM_NOTIFY, id, data as usize),
        Text::In { text } => {
            let original = read_word(data, text);
            let made = match original {
                0 | TEXT_CALLBACK => None,
                pointer => ansi::narrow(wide_text(pointer)),
            };
            if let Some(made) = &made {
                write_word(data, text, made.address());
            }
            let answer = gui::send(parent, WM_NOTIFY, id, data as usize);
            write_word(data, text, original);
            drop(made);
            answer
        }
        Text::Out { text, room } => {
            let original = read_word(data, text);
            let units = read_i32(data, room).max(0) as usize;
            let mut lent: Vec<u8> = alloc::vec![0u8; (units * 3).max(1)];
            let lent_address = lent.as_mut_ptr() as usize;
            if original != 0 && original != TEXT_CALLBACK {
                write_word(data, text, lent_address);
                write_i32(data, room, lent.len() as i32);
            }
            let answer = gui::send(parent, WM_NOTIFY, id, data as usize);
            let given = read_word(data, text);
            if original != 0 && original != TEXT_CALLBACK {
                let bytes = match given {
                    0 => Vec::new(),
                    pointer if pointer == lent_address => {
                        let end = lent.iter().position(|&byte| byte == 0).unwrap_or(lent.len());
                        lent[..end].to_vec()
                    }
                    pointer => narrow_text(pointer).to_vec(),
                };
                widen_into(&bytes, original, units);
                write_word(data, text, original);
                write_i32(data, room, units as i32);
            }
            answer
        }
    };
    write_i32(data, CODE_OFFSET, code);
    answer
}
