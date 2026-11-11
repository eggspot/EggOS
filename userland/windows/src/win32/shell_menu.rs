//! `TrackPopupMenu` outside the program's own content (D371,
//! `docs/ux/windows-tooltips-tray-v0.5.md` section 14): a tray program's context menu, whose
//! point is on the bar and whose window is usually hidden, is sent to the window server as a
//! shell menu, and the call waits for the entry the person chose.

use super::SpinLock;
use super::gui::{self, Next, try_with, with};
use crate::gui::menus::{MAX_ITEMS, flag};
use crate::gui::message;
use crate::gui::queue::Filter;
use crate::text;

const SIZE_MINIMIZED: usize = 1;
const ENTRY_GRAYED: u8 = 1;
const ENTRY_CHECKED: u8 = 2;
const ENTRY_RADIO: u8 = 4;
const ENTRY_SEPARATOR: u8 = 8;
const LABEL_CAPACITY: usize = 64;
const ENTRIES_CAPACITY: usize = MAX_ITEMS * (LABEL_CAPACITY + 2);

#[derive(Debug, Clone, Copy)]
struct Waiting {
    ids: [usize; MAX_ITEMS],
    answer: Option<usize>,
}

static WAITING: SpinLock<Option<Waiting>> = SpinLock::new(None);

/// Whether a context menu of top-level window `root` at screen point `point` is the shell's:
/// the window is hidden or minimised, or the point lies outside its content.
pub(super) fn is_shells(root: usize, point: (i32, i32)) -> bool {
    let state = with(|gui| {
        let window = gui.windows.get(root)?;
        Some((
            window.shown && window.size_kind != SIZE_MINIMIZED,
            window.buffer_width as i32,
            window.buffer_height as i32,
        ))
    })
    .flatten();
    let Some((true, width, height)) = state else {
        return true;
    };
    let Ok((left, top)) = super::placement::content_origin(root) else {
        return true;
    };
    let (x, y) = (point.0 - left, point.1 - top);
    x < 0 || y < 0 || x >= width || y >= height
}

/// A menu label as the shell shows it: `&` marks dropped (`&&` kept as one), the tab before
/// an accelerator's text two spaces, at most [`LABEL_CAPACITY`] bytes of UTF-8.
fn shell_label(units: &[u16], out: &mut [u8; LABEL_CAPACITY]) -> usize {
    let mut utf8 = [0u8; LABEL_CAPACITY * 2];
    let (length, _) = text::write_utf8(units, &mut utf8);
    let decoded = core::str::from_utf8(&utf8[..length]).unwrap_or("");
    let mut written = 0;
    let mut characters = decoded.chars().peekable();
    while let Some(character) = characters.next() {
        let shown: &[char] = match character {
            '&' if characters.peek() == Some(&'&') => {
                characters.next();
                &['&']
            }
            '&' => &[],
            '\t' => &[' ', ' '],
            _ => core::slice::from_ref(&character),
        };
        for &shown in shown {
            let mut bytes = [0u8; 4];
            let encoded = shown.encode_utf8(&mut bytes).as_bytes();
            if written + encoded.len() > LABEL_CAPACITY {
                return written;
            }
            out[written..written + encoded.len()].copy_from_slice(encoded);
            written += encoded.len();
        }
    }
    written
}

/// The entries of menu `popup` in the server's encoding, how many bytes they take, and each
/// entry's command id (0 for one that cannot be chosen: a separator, a grayed item or a
/// submenu, which shell menus do not open).
fn entries(popup: usize) -> Result<([u8; ENTRIES_CAPACITY], usize, [usize; MAX_ITEMS]), u32> {
    try_with(|gui| {
        let menu = super::menu::table(gui)
            .and_then(|table| table.get(popup))
            .ok_or(super::error::INVALID_MENU_HANDLE)?;
        let mut bytes = [0u8; ENTRIES_CAPACITY];
        let mut ids = [0usize; MAX_ITEMS];
        let mut at = 0;
        for (index, item) in menu.items().iter().enumerate() {
            let opens = item.popup().is_some();
            let mut flags = 0;
            if item.is_grayed() || opens {
                flags |= ENTRY_GRAYED;
            }
            if item.flags & flag::CHECKED != 0 {
                flags |= ENTRY_CHECKED;
            }
            if item.flags & flag::RADIO_CHECK != 0 {
                flags |= ENTRY_RADIO;
            }
            let mut label = [0u8; LABEL_CAPACITY];
            let length = match item.is_separator() {
                true => {
                    flags |= ENTRY_SEPARATOR;
                    0
                }
                false => shell_label(item.label(), &mut label),
            };
            bytes[at] = flags;
            bytes[at + 1] = length as u8;
            bytes[at + 2..at + 2 + length].copy_from_slice(&label[..length]);
            at += 2 + length;
            if flags & (ENTRY_GRAYED | ENTRY_SEPARATOR) == 0 {
                ids[index] = item.id;
            }
        }
        Ok((bytes, at, ids))
    })
}

/// Shows menu `popup` as a shell menu for top-level window `root` and runs the thread's
/// messages until it closes; the chosen item's command id, 0 when none was chosen.
pub(super) fn track(root: usize, popup: usize) -> Result<usize, u32> {
    let (bytes, length, ids) = entries(popup)?;
    let device = gui::device()?;
    let server = try_with(|gui| gui.window(root).map(|window| window.server))?;
    WAITING.with(|waiting| *waiting = Some(Waiting { ids, answer: None }));
    if device.show_shell_menu(server, &bytes[..length]).is_err() {
        WAITING.with(|waiting| *waiting = None);
        return Err(super::error::INVALID_PARAMETER);
    }
    let finished = |_: &gui::Gui| WAITING.with(|waiting| waiting.is_none_or(|w| w.answer.is_some()));
    loop {
        match gui::wait_for(Filter::default(), true, true, &finished) {
            Next::Done | Next::Nothing => break,
            Next::Message(next) if next.message == message::QUIT => {
                with(|gui| gui.queue.quit(next.thread, next.wparam));
                break;
            }
            Next::Message(next) => {
                super::user32::translate(&next);
                super::user32::dispatch(&next);
            }
        }
    }
    let answer = WAITING.with(|waiting| waiting.take().and_then(|w| w.answer));
    Ok(answer.unwrap_or(0))
}

/// The window server's `SHELL_MENU` event: entry `chosen` (from 1), or 0 for none.
pub(super) fn answered(chosen: u32) {
    WAITING.with(|waiting| {
        if let Some(waiting) = waiting.as_mut() {
            let id = (chosen as usize)
                .checked_sub(1)
                .and_then(|index| waiting.ids.get(index).copied())
                .unwrap_or(0);
            waiting.answer = Some(id);
        }
    });
}
