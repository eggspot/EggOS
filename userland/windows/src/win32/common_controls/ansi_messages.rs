//! The common controls' `A` messages (D402): a program built for ANSI sends `LVM_INSERTITEMA`,
//! `TVM_GETITEMA`, `SB_SETTEXTA` and the rest with narrow text, which on EggOS is UTF-8. Each
//! becomes its `W` message: a structure is copied with its text widened, and what a `GET`
//! message writes is narrowed back into the program's buffer, cut at a whole character, so the
//! controls keep one implementation.

use super::super::ansi;
use super::super::heap;
use crate::gui::windows::CommonKind;
use crate::text;

const MOST_STRUCTURE: usize = 128;
const LONGEST_TEXT: usize = 0x1_0000;
const TEXT_CALLBACK: usize = usize::MAX;
const RESOURCE_LIMIT: usize = 0x1_0000;
const TOOL_TEXT_UNITS: usize = 80;
const SBT_OWNERDRAW: usize = 0x1000;
const TBBUTTON_SIZE: usize = 32;
const TBBUTTON_STRING: usize = 24;
const MOST_BUTTONS: usize = 256;
const SB_GETTEXTW: u32 = 0x40d;
const SB_GETTEXTLENGTHW: u32 = 0x40c;

#[derive(Debug, Clone, Copy)]
enum Capacity {
    Field(usize),
    Parameter,
}

#[derive(Debug, Clone, Copy)]
struct Structure {
    size: usize,
    base: usize,
    text: usize,
    capacity: Capacity,
    mask: Option<(usize, u32)>,
    out: bool,
    answers_length: bool,
    sized: bool,
}

#[derive(Debug, Clone, Copy)]
enum Rule {
    Renumber,
    Structure(Structure),
    Text,
    StatusText,
    StatusGet,
    StatusLength,
    Strings,
    Buttons { single: bool },
    ButtonText,
}

const fn item(size: usize, text: usize, capacity: usize, mask: Option<(usize, u32)>) -> Structure {
    Structure {
        size,
        base: 0,
        text,
        capacity: Capacity::Field(capacity),
        mask,
        out: false,
        answers_length: false,
        sized: false,
    }
}

const fn read(structure: Structure) -> Structure {
    Structure {
        out: true,
        ..structure
    }
}

const LIST_ITEM: Structure = item(88, 24, 32, Some((0, 0x1)));
const LIST_COLUMN: Structure = item(56, 16, 24, Some((0, 0x4)));
const LIST_FIND: Structure = item(40, 8, 0, Some((0, 0x2 | 0x8)));
const LIST_ITEM_TEXT: Structure = item(88, 24, 32, None);
const TREE_ITEM: Structure = item(80, 24, 32, Some((0, 0x1)));
const TREE_INSERT: Structure = Structure {
    base: 16,
    ..TREE_ITEM
};
const HEADER_ITEM: Structure = item(72, 8, 24, Some((0, 0x2)));
const TAB_ITEM: Structure = item(40, 16, 24, Some((0, 0x1)));
const TOOL_INFO: Structure = Structure {
    sized: true,
    ..item(72, 48, 0, None)
};

fn rule_of(kind: CommonKind, number: u32) -> Option<(u32, Rule)> {
    use Rule::{Renumber, Structure as S};
    Some(match (kind, number) {
        (CommonKind::ListView, 0x1005) => (0x104b, S(read(LIST_ITEM))),
        (CommonKind::ListView, 0x1006) => (0x104c, S(LIST_ITEM)),
        (CommonKind::ListView, 0x1007) => (0x104d, S(LIST_ITEM)),
        (CommonKind::ListView, 0x100d) => (0x1053, S(LIST_FIND)),
        (CommonKind::ListView, 0x1011) => (0x1057, Rule::Text),
        (CommonKind::ListView, 0x1017) => (0x1076, Renumber),
        (CommonKind::ListView, 0x1019) => (0x105f, S(read(LIST_COLUMN))),
        (CommonKind::ListView, 0x101a) => (0x1060, S(LIST_COLUMN)),
        (CommonKind::ListView, 0x101b) => (0x1061, S(LIST_COLUMN)),
        (CommonKind::ListView, 0x102d) => (
            0x1073,
            S(Structure {
                answers_length: true,
                ..read(LIST_ITEM_TEXT)
            }),
        ),
        (CommonKind::ListView, 0x102e) => (0x1074, S(LIST_ITEM_TEXT)),
        (CommonKind::TreeView, 0x1100) => (0x1132, S(TREE_INSERT)),
        (CommonKind::TreeView, 0x110c) => (0x113e, S(read(TREE_ITEM))),
        (CommonKind::TreeView, 0x110d) => (0x113f, S(TREE_ITEM)),
        (CommonKind::TreeView, 0x110e) => (0x1141, Renumber),
        (CommonKind::Header, 0x1201) => (0x120a, S(HEADER_ITEM)),
        (CommonKind::Header, 0x1203) => (0x120b, S(read(HEADER_ITEM))),
        (CommonKind::Header, 0x1204) => (0x120c, S(HEADER_ITEM)),
        (CommonKind::TabControl, 0x1305) => (0x133c, S(read(TAB_ITEM))),
        (CommonKind::TabControl, 0x1306) => (0x133d, S(TAB_ITEM)),
        (CommonKind::TabControl, 0x1307) => (0x133e, S(TAB_ITEM)),
        (CommonKind::StatusBar, 0x401) => (0x40b, Rule::StatusText),
        (CommonKind::StatusBar, 0x402) => (0x40d, Rule::StatusGet),
        (CommonKind::StatusBar, 0x403) => (0x40c, Rule::StatusLength),
        (CommonKind::StatusBar, 0x410) => (0x411, Rule::Text),
        (CommonKind::Toolbar, 0x41c) => (0x44d, Rule::Strings),
        (CommonKind::Toolbar, 0x414) => (0x444, Rule::Buttons { single: false }),
        (CommonKind::Toolbar, 0x415) => (0x443, Rule::Buttons { single: true }),
        (CommonKind::Toolbar, 0x42d) => (0x44b, Rule::ButtonText),
        (CommonKind::Tooltip, 0x404) => (0x432, S(TOOL_INFO)),
        (CommonKind::Tooltip, 0x405) => (0x433, S(TOOL_INFO)),
        (CommonKind::Tooltip, 0x406) => (0x434, S(TOOL_INFO)),
        (CommonKind::Tooltip, 0x409) => (0x436, S(TOOL_INFO)),
        (CommonKind::Tooltip, 0x40b) => (
            0x438,
            S(Structure {
                capacity: Capacity::Parameter,
                ..read(TOOL_INFO)
            }),
        ),
        (CommonKind::Tooltip, 0x40c) => (0x439, S(TOOL_INFO)),
        (CommonKind::Tooltip, 0x420) => (0x421, Rule::Text),
        _ => return None,
    })
}

/// Answers `number` for common control `window` of `kind` when it is an `A` message, by its
/// `W` message; `None` for every other message.
pub(super) fn translate(
    window: usize,
    kind: CommonKind,
    number: u32,
    wparam: usize,
    lparam: usize,
) -> Option<usize> {
    let (wide, rule) = rule_of(kind, number)?;
    let call = |wparam: usize, lparam: usize| super::procedure(window, kind, wide, wparam, lparam);
    let send = |number: u32, wparam: usize, lparam: usize| {
        super::procedure(window, kind, number, wparam, lparam)
    };
    // SAFETY: the message's contract: its `lParam` (and for some its `wParam`) are what the
    // `A` message documents, the program's for the call.
    Some(unsafe {
        match rule {
            Rule::Renumber => call(wparam, lparam),
            Rule::Structure(structure) => through_structure(&call, wparam, lparam, structure),
            Rule::Text => with_wide_text(lparam, |wide| call(wparam, wide)),
            Rule::StatusText if wparam & SBT_OWNERDRAW != 0 => call(wparam, lparam),
            Rule::StatusText => with_wide_text(lparam, |wide| call(wparam, wide)),
            Rule::StatusGet => status_text(&send, wparam, lparam, true),
            Rule::StatusLength => status_text(&send, wparam, 0, false),
            Rule::Strings if wparam != 0 => call(wparam, lparam),
            Rule::Strings => strings(&call, lparam),
            Rule::Buttons { single } => buttons(&call, wparam, lparam, single),
            Rule::ButtonText => button_text(&call, wparam, lparam),
        }
    })
}

unsafe fn narrow_at<'a>(pointer: usize) -> &'a [u8] {
    let start = pointer as *const u8;
    let mut length = 0;
    // SAFETY: the caller's duty: the bytes up to the NUL are readable.
    while length < LONGEST_TEXT && unsafe { start.add(length).read() } != 0 {
        length += 1;
    }
    // SAFETY: as above.
    unsafe { core::slice::from_raw_parts(start, length) }
}

fn is_text_pointer(pointer: usize) -> bool {
    pointer >= RESOURCE_LIMIT && pointer != TEXT_CALLBACK
}

unsafe fn with_wide_text(pointer: usize, send: impl FnOnce(usize) -> usize) -> usize {
    if !is_text_pointer(pointer) {
        return send(pointer);
    }
    // SAFETY: the caller's duty.
    match ansi::widen(unsafe { narrow_at(pointer) }) {
        Some(wide) => send(wide.address()),
        None => 0,
    }
}

unsafe fn narrow_into(units: &[u16], out: usize, capacity: usize) -> usize {
    if out == 0 || capacity == 0 {
        return 0;
    }
    let end = units
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(units.len());
    // SAFETY: the caller's duty.
    let target = unsafe { core::slice::from_raw_parts_mut(out as *mut u8, capacity) };
    let (written, _) = text::write_utf8(&units[..end], &mut target[..capacity - 1]);
    target[written] = 0;
    written
}

fn read_word<T: Copy>(bytes: &[u8], at: usize) -> T {
    // SAFETY: `at` lies inside the copied structure, which holds a `T` there.
    unsafe { bytes.as_ptr().add(at).cast::<T>().read_unaligned() }
}

fn write_word<T: Copy>(bytes: &mut [u8], at: usize, value: T) {
    // SAFETY: as in `read_word`, writable.
    unsafe {
        bytes
            .as_mut_ptr()
            .add(at)
            .cast::<T>()
            .write_unaligned(value)
    }
}

unsafe fn through_structure(
    call: &dyn Fn(usize, usize) -> usize,
    wparam: usize,
    lparam: usize,
    rule: Structure,
) -> usize {
    if lparam == 0 {
        return call(wparam, 0);
    }
    let total = match rule.sized {
        // SAFETY: the caller's duty: a sized structure starts with its size.
        true => (unsafe { (lparam as *const u32).read_unaligned() } as usize).min(rule.size),
        false => rule.base + rule.size,
    };
    if total < rule.base + rule.text + 8 {
        return call(wparam, lparam);
    }
    let mut copy = [0u8; MOST_STRUCTURE];
    // SAFETY: the caller's duty: the program's structure holds `total` bytes.
    unsafe { core::ptr::copy_nonoverlapping(lparam as *const u8, copy.as_mut_ptr(), total) };
    let text_at = rule.base + rule.text;
    let pointer: usize = read_word(&copy, text_at);
    let uses_text = rule
        .mask
        .is_none_or(|(at, bits)| read_word::<u32>(&copy, rule.base + at) & bits != 0);
    if !uses_text || (!rule.out && !is_text_pointer(pointer)) {
        let answer = call(wparam, copy.as_mut_ptr() as usize);
        if rule.out {
            // SAFETY: as above: written back whole, the text pointer unchanged.
            unsafe { core::ptr::copy_nonoverlapping(copy.as_ptr(), lparam as *mut u8, total) };
        }
        return answer;
    }
    if !rule.out {
        // SAFETY: the caller's duty: the text field points at the program's narrow text.
        let Some(wide) = ansi::widen(unsafe { narrow_at(pointer) }) else {
            return 0;
        };
        write_word(&mut copy, text_at, wide.address());
        return call(wparam, copy.as_mut_ptr() as usize);
    }
    let capacity = match rule.capacity {
        Capacity::Field(at) => read_word::<i32>(&copy, rule.base + at).max(0) as usize,
        Capacity::Parameter if wparam > 0 => wparam,
        Capacity::Parameter => TOOL_TEXT_UNITS,
    };
    let units = capacity.clamp(1, LONGEST_TEXT);
    let block = heap::allocate(units * 2).cast::<u16>();
    if block.is_null() {
        return 0;
    }
    write_word(&mut copy, text_at, block as usize);
    if let Capacity::Field(at) = rule.capacity {
        write_word(&mut copy, rule.base + at, units as i32);
    }
    let answer = call(wparam, copy.as_mut_ptr() as usize);
    let given: usize = read_word(&copy, text_at);
    let source = match given {
        0 => &[][..],
        // SAFETY: the control wrote into this call's block of `units` units.
        given if given == block as usize => unsafe { core::slice::from_raw_parts(block, units) },
        // SAFETY: the control pointed the field at its own NUL-terminated text.
        given => unsafe { super::super::file::wide_string(given as *const u16) },
    };
    // SAFETY: the caller's duty: the program's buffer of `capacity` bytes.
    let written = unsafe { narrow_into(source, pointer, capacity.min(units)) };
    // SAFETY: this call's block, read.
    unsafe { heap::free(block.cast()) };
    write_word(&mut copy, text_at, pointer);
    if let Capacity::Field(at) = rule.capacity {
        write_word(&mut copy, rule.base + at, capacity as i32);
    }
    // SAFETY: as above: the structure written back with the program's own buffer in it.
    unsafe { core::ptr::copy_nonoverlapping(copy.as_ptr(), lparam as *mut u8, total) };
    match rule.answers_length {
        true => written,
        false => answer,
    }
}

unsafe fn status_text(
    send: &dyn Fn(u32, usize, usize) -> usize,
    part: usize,
    out: usize,
    writes: bool,
) -> usize {
    let length = send(SB_GETTEXTLENGTHW, part, 0);
    let units = (length & 0xffff) + 1;
    let kind = length & !0xffff;
    let block = heap::allocate(units * 2).cast::<u16>();
    if block.is_null() {
        return 0;
    }
    let _ = send(SB_GETTEXTW, part, block as usize);
    // SAFETY: the control wrote at most its length and a NUL into this call's block.
    let wide = unsafe { core::slice::from_raw_parts(block, units) };
    let end = wide
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(wide.len());
    let bytes = text::utf8_length(&wide[..end]);
    if writes && out != 0 {
        // SAFETY: the caller's duty: room for the length `SB_GETTEXTLENGTHA` gave and a NUL.
        let _ = unsafe { narrow_into(&wide[..end], out, bytes + 1) };
    }
    // SAFETY: this call's block, read.
    unsafe { heap::free(block.cast()) };
    kind | bytes
}

unsafe fn strings(call: &dyn Fn(usize, usize) -> usize, lparam: usize) -> usize {
    if lparam == 0 {
        return call(0, 0);
    }
    let start = lparam as *const u8;
    let mut length = 0;
    // SAFETY: the caller's duty: the list ends with two NULs.
    while length + 1 < LONGEST_TEXT
        && unsafe { (start.add(length).read(), start.add(length + 1).read()) } != (0, 0)
    {
        length += 1;
    }
    // SAFETY: as above: the bytes before the closing pair.
    let bytes = unsafe { core::slice::from_raw_parts(start, length + 1) };
    let units = text::utf16_length(bytes) + 2;
    let block = heap::allocate(units * 2).cast::<u16>();
    if block.is_null() {
        return usize::MAX;
    }
    // SAFETY: a fresh, zeroed block of `units` units; the last two stay NUL.
    let out = unsafe { core::slice::from_raw_parts_mut(block, units) };
    let _ = text::write_utf16(bytes, &mut out[..units - 2]);
    let answer = call(0, block as usize);
    // SAFETY: this call's block, which the control copied.
    unsafe { heap::free(block.cast()) };
    answer
}

unsafe fn buttons(
    call: &dyn Fn(usize, usize) -> usize,
    wparam: usize,
    lparam: usize,
    single: bool,
) -> usize {
    let count = if single { 1 } else { wparam.min(MOST_BUTTONS) };
    if lparam == 0 || count == 0 {
        return call(wparam, lparam);
    }
    let bytes = count * TBBUTTON_SIZE;
    let copy = heap::allocate(bytes);
    if copy.is_null() {
        return 0;
    }
    // SAFETY: the caller's duty: `count` buttons; the copy is a fresh block of their size.
    unsafe { core::ptr::copy_nonoverlapping(lparam as *const u8, copy, bytes) };
    // SAFETY: the copy's bytes, this call's.
    let held = unsafe { core::slice::from_raw_parts_mut(copy, bytes) };
    let mut widened = alloc::vec::Vec::new();
    for index in 0..count {
        let at = index * TBBUTTON_SIZE + TBBUTTON_STRING;
        let pointer: usize = read_word(held, at);
        if !is_text_pointer(pointer) {
            continue;
        }
        // SAFETY: the caller's duty: a pointer `iString` is the program's narrow text.
        if let Some(wide) = ansi::widen(unsafe { narrow_at(pointer) }) {
            write_word(held, at, wide.address());
            widened.push(wide);
        }
    }
    let answer = call(wparam, copy as usize);
    drop(widened);
    // SAFETY: this call's copy, which the control read.
    unsafe { heap::free(copy) };
    answer
}

unsafe fn button_text(call: &dyn Fn(usize, usize) -> usize, id: usize, out: usize) -> usize {
    let length = call(id, 0);
    if length == usize::MAX || length as isize == -1 {
        return length;
    }
    let units = length + 1;
    let block = heap::allocate(units * 2).cast::<u16>();
    if block.is_null() {
        return usize::MAX;
    }
    let _ = call(id, block as usize);
    // SAFETY: the control wrote its text and a NUL into this call's block.
    let wide = unsafe { core::slice::from_raw_parts(block, units) };
    let end = wide
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(wide.len());
    let bytes = text::utf8_length(&wide[..end]);
    if out != 0 {
        // SAFETY: the caller's duty.
        let _ = unsafe { narrow_into(&wide[..end], out, bytes + 1) };
    }
    // SAFETY: this call's block, read.
    unsafe { heap::free(block.cast()) };
    bytes
}
