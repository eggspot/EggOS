//! comctl32's tooltip control, `tooltips_class32` (D355, `docs/ux/windows-tooltips-tray-v0.5.md`
//! sections 2 to 7): a window with no rect of its own whose tools (rectangles of its owner's
//! windows, or whole child windows) show a tip when the pointer rests on them. The layer tracks
//! the pointer itself (`TTF_SUBCLASS` and `TTM_RELAYEVENT` change nothing), shows the tip after
//! the initial delay, hides it after the autopop delay, on a press, a key or leaving the tool,
//! and draws it over the top-level window's content, as it draws an open drop-down.

use super::super::gui::{self, Gui, with};
use super::{Model, NotifyHeader, allocate, default_procedure, notify_parent_keeping, with_state};
use crate::gui::windows::{self, CommonKind, ControlKind, Procedure, Window};
use eggos_element_tree::Role;
use eggos_windows_gui::controls::tooltip::{self as model, Placement, TipIcon, TipText};
use eggos_windows_gui::controls::LIGHT;
use eggos_windows_gui::surface::{Rect, Surface};

const MOST_TOOLS: usize = 64;
const TEXT_UNITS: usize = 255;
const TITLE_UNITS: usize = 99;
const TOOL_INFO_SIZE: usize = 72;
const CALLBACK_TEXT: usize = usize::MAX;
const TTF_IDISHWND: u32 = 0x0001;
const TTF_CENTERTIP: u32 = 0x0002;
const TTF_TRACK: u32 = 0x0020;
const TTF_ABSOLUTE: u32 = 0x0080;
const TTF_DI_SETITEM: u32 = 0x8000;
const TTS_NOPREFIX: u32 = 0x0002;
const TTS_BALLOON: u32 = 0x0040;
const TTN_GETDISPINFOW: i32 = -530;
const TTN_SHOW: i32 = -521;
const TTN_POP: i32 = -522;
const TTDT_AUTOMATIC: usize = 0;
const TTDT_RESHOW: usize = 1;
const TTDT_AUTOPOP: usize = 2;
const TTDT_INITIAL: usize = 3;
const DEFAULT_DELAYS: [u32; 3] = [500, 100, 5000];

/// The tooltip messages this layer answers (the W codes).
pub mod code {
    pub const TTM_ACTIVATE: u32 = 0x0401;
    pub const TTM_SETDELAYTIME: u32 = 0x0403;
    pub const TTM_RELAYEVENT: u32 = 0x0407;
    pub const TTM_GETTOOLCOUNT: u32 = 0x040d;
    pub const TTM_TRACKACTIVATE: u32 = 0x0411;
    pub const TTM_TRACKPOSITION: u32 = 0x0412;
    pub const TTM_GETDELAYTIME: u32 = 0x0415;
    pub const TTM_SETMAXTIPWIDTH: u32 = 0x0418;
    pub const TTM_GETMAXTIPWIDTH: u32 = 0x0419;
    pub const TTM_POP: u32 = 0x041c;
    pub const TTM_UPDATE: u32 = 0x041d;
    pub const TTM_SETTITLEW: u32 = 0x0421;
    pub const TTM_POPUP: u32 = 0x0422;
    pub const TTM_ADDTOOLW: u32 = 0x0432;
    pub const TTM_DELTOOLW: u32 = 0x0433;
    pub const TTM_NEWTOOLRECTW: u32 = 0x0434;
    pub const TTM_GETTOOLINFOW: u32 = 0x0435;
    pub const TTM_SETTOOLINFOW: u32 = 0x0436;
    pub const TTM_GETTEXTW: u32 = 0x0438;
    pub const TTM_UPDATETIPTEXTW: u32 = 0x0439;
}

#[derive(Clone, Copy)]
struct Tool {
    owner: usize,
    id: usize,
    rect: [i32; 4],
    flags: u32,
    text: [u16; TEXT_UNITS],
    length: usize,
    callback: bool,
    lparam: usize,
}

/// A tooltip control's state.
pub struct TipState {
    tools: [Option<Tool>; MOST_TOOLS],
    active: bool,
    delays: [u32; 3],
    max_width: i32,
    title: [u16; TITLE_UNITS],
    title_length: usize,
    icon: TipIcon,
    track_point: Option<(i32, i32)>,
    shown_text: [u16; TEXT_UNITS],
    shown_length: usize,
}

impl Model for TipState {
    const KIND: CommonKind = CommonKind::Tooltip;
}

/// Where the pointer rests among the tools of a top-level window's tooltip controls, and the
/// tip shown (one per process, as the pointer is in one place).
#[derive(Debug, Clone, Copy, Default)]
pub struct Tracking {
    resting: Option<(usize, usize, u32)>,
    shown: Option<(usize, usize, Rect, u32)>,
    spent: Option<(usize, usize)>,
    hidden_at: Option<u32>,
    root: usize,
    pointer: (i32, i32),
}

impl Tracking {
    pub const fn new() -> Tracking {
        Tracking {
            resting: None,
            shown: None,
            spent: None,
            hidden_at: None,
            root: 0,
            pointer: (0, 0),
        }
    }
}

/// A new tooltip control's state; `None` without memory.
pub(super) fn new_state(_style: u32) -> Option<usize> {
    allocate(TipState {
        tools: [None; MOST_TOOLS],
        active: true,
        delays: DEFAULT_DELAYS,
        max_width: -1,
        title: [0; TITLE_UNITS],
        title_length: 0,
        icon: TipIcon::None,
        track_point: None,
        shown_text: [0; TEXT_UNITS],
        shown_length: 0,
    })
}

/// A tooltip's role in agents' tree: a tooltip that is not focusable.
pub(super) fn role(_window: &Window) -> (Role, u16) {
    (Role::Tooltip, 0)
}

/// `TOOLINFOW`, as far as its `cbSize` says.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ToolInfo {
    size: u32,
    flags: u32,
    owner: usize,
    id: usize,
    rect: [i32; 4],
    instance: usize,
    text: usize,
    lparam: usize,
    reserved: usize,
}

fn read_tool_info(address: usize) -> Option<ToolInfo> {
    if address == 0 {
        return None;
    }
    // SAFETY: the messages' contract: a `TOOLINFOW` whose first word is its size.
    let size = unsafe { (address as *const u32).read_unaligned() } as usize;
    let mut bytes = [0u8; TOOL_INFO_SIZE];
    let length = size.min(TOOL_INFO_SIZE);
    // SAFETY: as above, `size` readable bytes.
    unsafe { core::ptr::copy_nonoverlapping(address as *const u8, bytes.as_mut_ptr(), length) };
    // SAFETY: `bytes` is a whole `ToolInfo`'s worth, zero past what the program gave.
    Some(unsafe { (bytes.as_ptr() as *const ToolInfo).read_unaligned() })
}

fn text_of(info: &ToolInfo, out: &mut [u16; TEXT_UNITS]) -> (usize, bool) {
    match info.text {
        0 => (0, false),
        CALLBACK_TEXT => (0, true),
        address if address < 0x1_0000 => (0, true),
        address => {
            // SAFETY: the messages' contract: a NUL-terminated string.
            let units = unsafe { super::super::file::wide_string(address as *const u16) };
            let length = units.len().min(TEXT_UNITS);
            out[..length].copy_from_slice(&units[..length]);
            (length, false)
        }
    }
}

fn tool_from(info: &ToolInfo) -> Tool {
    let mut text = [0u16; TEXT_UNITS];
    let (length, callback) = text_of(info, &mut text);
    Tool {
        owner: info.owner,
        id: info.id,
        rect: info.rect,
        flags: info.flags,
        text,
        length,
        callback,
        lparam: info.lparam,
    }
}

fn find(state: &TipState, owner: usize, id: usize) -> Option<usize> {
    state
        .tools
        .iter()
        .position(|tool| tool.is_some_and(|tool| tool.owner == owner && tool.id == id))
}

/// A tool's rectangle in its top-level window's buffer, where the pointer is tracked.
fn tool_rect(gui: &Gui, tool: &Tool) -> Option<Rect> {
    match tool.flags & TTF_IDISHWND != 0 {
        true => {
            let placement = windows::client_placement(&gui.windows, tool.id)?;
            let entry = gui.windows.get(tool.id)?;
            let (x, y) = placement.origin;
            Some(Rect::sized(x, y, entry.width as i32, entry.height as i32))
        }
        false => {
            let (x, y) = windows::client_placement(&gui.windows, tool.owner)?.origin;
            let [left, top, right, bottom] = tool.rect;
            Some(Rect::new(x + left, y + top, x + right, y + bottom))
        }
    }
}

fn tips_of(gui: &Gui, root: usize) -> ([usize; windows::MAX_WINDOWS], usize) {
    let (handles, count) = windows::tree(&gui.windows, root);
    let mut tips = [0; windows::MAX_WINDOWS];
    let mut found = 0;
    for &handle in &handles[..count] {
        let is_tip = gui.windows.get(handle).is_some_and(|entry| {
            matches!(
                entry.procedure,
                Procedure::Control(ControlKind::Common(CommonKind::Tooltip), _)
            )
        });
        if is_tip {
            tips[found] = handle;
            found += 1;
        }
    }
    (tips, found)
}

fn state_of(gui: &Gui, control: usize) -> Option<&TipState> {
    super::read_state::<TipState, &TipState>(gui, control, |_, state| state)
}

/// The tool under `point` (buffer coordinates of `root`): the newest match of the newest
/// control, as Win32 picks the topmost tool.
fn tool_under(gui: &Gui, root: usize, point: (i32, i32)) -> Option<(usize, usize)> {
    let (tips, count) = tips_of(gui, root);
    tips[..count].iter().rev().find_map(|&control| {
        let state = state_of(gui, control)?;
        if !state.active {
            return None;
        }
        state.tools.iter().enumerate().rev().find_map(|(index, tool)| {
            let tool = tool.as_ref().filter(|tool| tool.flags & TTF_TRACK == 0)?;
            tool_rect(gui, tool)
                .filter(|rect| rect.contains(point.0, point.1))
                .map(|_| (control, index))
        })
    })
}

/// The pointer moved to `point` in top-level window `root`'s buffer at `time` (D355): the
/// tool under it starts resting, a tip over another tool hides.
pub fn pointer_moved(gui: &mut Gui, root: usize, point: (i32, i32), time: u32) {
    let under = tool_under(gui, root, point);
    let tracking = gui.tip;
    if let Some((control, tool, rect, _)) = tracking.shown
        && Some((control, tool)) != under
        && state_of(gui, control).is_some_and(|state| {
            state.tools[tool].is_some_and(|tool| tool.flags & TTF_TRACK == 0)
        })
    {
        hide(gui, rect, time);
    }
    let tracking = &mut gui.tip;
    tracking.root = root;
    tracking.pointer = point;
    if tracking.spent.is_some() && tracking.spent != under {
        tracking.spent = None;
    }
    tracking.resting = under.map(|(control, tool)| (control, tool, time));
}

/// A press or a key in top-level window `root`: the tip hides and its tool is spent until the
/// pointer leaves it.
pub fn dismiss(gui: &mut Gui, root: usize, time: u32) {
    if gui.tip.root != root {
        return;
    }
    if let Some((control, tool, rect, _)) = gui.tip.shown {
        hide(gui, rect, time);
        gui.tip.spent = Some((control, tool));
    }
    if let Some((control, tool, _)) = gui.tip.resting.take() {
        gui.tip.spent = Some((control, tool));
    }
}

fn hide(gui: &mut Gui, rect: Rect, time: u32) {
    gui.tip.shown = None;
    gui.tip.hidden_at = Some(time);
    let root = gui.tip.root;
    if let Some(entry) = gui.windows.get(root) {
        let content = rect.offset(0, -entry.menu_height);
        windows::invalidate_tree(&mut gui.windows, root, Some(content), true);
    }
}

#[derive(Clone, Copy)]
enum Due {
    Show(usize, usize),
    Hide(usize, usize),
}

fn due(gui: &Gui, now: u32) -> Option<(Due, u32)> {
    let tracking = gui.tip;
    if let Some((control, tool, _, since)) = tracking.shown {
        let state = state_of(gui, control)?;
        let autopop = state.delays[2];
        if state.tools[tool].is_some_and(|tool| tool.flags & TTF_TRACK != 0) {
            return None;
        }
        return Some((Due::Hide(control, tool), (since + autopop).saturating_sub(now)));
    }
    let (control, tool, since) = tracking.resting?;
    if tracking.spent == Some((control, tool)) {
        return None;
    }
    let state = state_of(gui, control)?;
    let hop = tracking
        .hidden_at
        .is_some_and(|hidden| now.wrapping_sub(hidden) <= state.delays[1]);
    let delay = if hop { state.delays[1] } else { state.delays[0] };
    Some((Due::Show(control, tool), (since + delay).saturating_sub(now)))
}

/// Milliseconds until a tip shows or hides, if one waits.
pub fn next_due(gui: &Gui, now: u32) -> Option<u32> {
    due(gui, now).map(|(_, wait)| wait)
}

/// Shows or hides the tip whose time came (called as threads look for messages): a callback
/// tool's text is asked with `TTN_GETDISPINFOW`, then `TTN_SHOW` and the tip is drawn;
/// `TTN_POP` when it hides.
pub fn tick() {
    let now = super::super::user32::now_ms();
    let found = with(|gui| {
        due(gui, now)
            .filter(|(_, wait)| *wait == 0)
            .map(|(due, _)| due)
    })
    .flatten();
    match found {
        Some(Due::Show(control, tool)) => show(control, tool, None),
        Some(Due::Hide(control, tool)) => {
            with(|gui| {
                if let Some((_, _, rect, _)) = gui.tip.shown {
                    hide(gui, rect, now);
                    gui.tip.spent = Some((control, tool));
                }
            });
            notify_tip(control, tool, TTN_POP);
            gui::wake();
        }
        None => {}
    }
}

fn notify_tip(control: usize, tool: usize, code: i32) -> usize {
    let found = with_state::<TipState, _>(control, |_, state| state.tools[tool].map(|tool| tool.id))
        .flatten();
    let Some(id) = found else {
        return 0;
    };
    let (answer, _) = notify_parent_keeping(control, code, |mut header| {
        header.id = id;
        header
    });
    answer
}

/// `NMTTDISPINFOW`.
#[repr(C)]
pub(super) struct DisplayInfo {
    pub(super) header: NotifyHeader,
    pub(super) text: usize,
    pub(super) buffer: [u16; 80],
    pub(super) instance: usize,
    pub(super) flags: u32,
    pub(super) lparam: usize,
}

fn asked_text(control: usize, tool: usize) -> Option<([u16; TEXT_UNITS], usize, bool)> {
    let (owner, id, flags, lparam) = with_state::<TipState, _>(control, |_, state| {
        state.tools[tool].map(|tool| (tool.owner, tool.id, tool.flags, tool.lparam))
    })
    .flatten()?;
    let mut info = DisplayInfo {
        header: NotifyHeader {
            from: control,
            id,
            code: TTN_GETDISPINFOW as u32,
        },
        text: 0,
        buffer: [0; 80],
        instance: 0,
        flags,
        lparam,
    };
    info.text = info.buffer.as_ptr() as usize;
    gui::send(owner, super::WM_NOTIFY, id, &raw mut info as usize);
    let mut out = [0u16; TEXT_UNITS];
    let units = match info.text {
        0 => &[][..],
        address if address == info.buffer.as_ptr() as usize => {
            let length = info.buffer.iter().position(|&unit| unit == 0).unwrap_or(80);
            &info.buffer[..length]
        }
        address if address < 0x1_0000 => &[][..],
        // SAFETY: `TTN_GETDISPINFOW`'s contract: the program points `lpszText` at a
        // NUL-terminated string it keeps.
        address => unsafe { super::super::file::wide_string(address as *const u16) },
    };
    let length = units.len().min(TEXT_UNITS);
    out[..length].copy_from_slice(&units[..length]);
    Some((out, length, info.flags & TTF_DI_SETITEM != 0))
}

/// Shows tool `tool`'s tip of `control` now: at `track` (buffer coordinates) for a tracking
/// tool, else as its kind places it.
fn show(control: usize, tool: usize, track: Option<(i32, i32)>) {
    let callback = with_state::<TipState, _>(control, |_, state| {
        state.tools[tool].is_some_and(|tool| tool.callback)
    })
    .unwrap_or(false);
    if callback && let Some((text, length, keep)) = asked_text(control, tool) {
        with_state::<TipState, _>(control, |_, state| {
            state.shown_text[..length].copy_from_slice(&text[..length]);
            state.shown_length = length;
            if let Some(stored) = state.tools[tool].as_mut().filter(|_| keep) {
                stored.text = text;
                stored.length = length;
                stored.callback = false;
            }
        });
    }
    notify_tip(control, tool, TTN_SHOW);
    let commit = with(|gui| place_shown(gui, control, tool, callback, track)).flatten();
    if let Some((device, server, rect)) = commit {
        let _ = device.commit(server, Some(rect));
    }
    gui::wake();
}

fn place_shown(
    gui: &mut Gui,
    control: usize,
    tool: usize,
    callback: bool,
    track: Option<(i32, i32)>,
) -> Option<(super::super::window_device::Device, u32, Rect)> {
    let root = windows::root_of(&gui.windows, control)?;
    let style = gui.windows.get(control)?.style;
    let pointer = gui.tip.pointer;
    let (rect, root) = {
        let state = state_of(gui, control)?;
        let entry = state.tools[tool]?;
        let mut stripped = [0u16; TEXT_UNITS];
        let text = match callback {
            true => &state.shown_text[..state.shown_length],
            false => &entry.text[..entry.length],
        };
        let text = match style & TTS_NOPREFIX {
            0 => model::without_prefixes(text, &mut stripped),
            _ => text,
        };
        if text.is_empty() && state.title_length == 0 {
            return None;
        }
        let tip = TipText {
            text,
            title: &state.title[..state.title_length],
            icon: state.icon,
            max_width: state.max_width,
        };
        let laid = model::layout(&tip);
        let top = gui.windows.get(root)?;
        let content = (top.buffer_width as i32, top.buffer_height as i32);
        let balloon = style & TTS_BALLOON != 0;
        let placement = match (track, entry.flags) {
            (Some(at), flags) if flags & TTF_ABSOLUTE != 0 => Placement::Absolute(at),
            (Some(at), _) => Placement::Pointer(at),
            (None, flags) if balloon || flags & TTF_CENTERTIP != 0 => Placement::Centred {
                tool: tool_rect(gui, &entry)?,
                above_first: balloon,
            },
            (None, _) => Placement::Pointer(pointer),
        };
        (model::place(placement, (laid.width, laid.height), content), root)
    };
    let now = super::super::user32::now_ms();
    gui.tip.root = root;
    gui.tip.shown = Some((control, tool, rect, now));
    gui.tip.resting = None;
    let painted = paint_shown(gui, root)?;
    let device = gui.device?;
    let top = gui.windows.get(root)?;
    top.committed.then_some((device, top.server, painted))
}

/// Paints the tip shown over top-level window `root`'s buffer, if there is one: the area
/// painted.
pub fn paint_shown(gui: &Gui, root: usize) -> Option<Rect> {
    let (control, tool, rect, _) = gui.tip.shown.filter(|_| gui.tip.root == root)?;
    let style = gui.windows.get(control)?.style;
    let state = state_of(gui, control)?;
    let entry = state.tools[tool]?;
    let mut stripped = [0u16; TEXT_UNITS];
    let text = match entry.callback {
        true => &state.shown_text[..state.shown_length],
        false => &entry.text[..entry.length],
    };
    let text = match style & TTS_NOPREFIX {
        0 => model::without_prefixes(text, &mut stripped),
        _ => text,
    };
    let tip = TipText {
        text,
        title: &state.title[..state.title_length],
        icon: state.icon,
        max_width: state.max_width,
    };
    let laid = model::layout(&tip);
    let top = gui.windows.get(root)?;
    // SAFETY: a top-level window's buffer is mapped, `buffer_width * buffer_height` pixels,
    // while the window is in the table, and the table's lock is held.
    let pixels = unsafe {
        core::slice::from_raw_parts_mut(
            top.pixels as *mut u32,
            top.buffer_width as usize * top.buffer_height as usize,
        )
    };
    let mut surface = Surface::new(pixels, top.buffer_width, top.buffer_height)?;
    model::paint(&mut surface, rect, &tip, &laid, style & TTS_BALLOON != 0, &LIGHT);
    Some(rect.intersection(&Rect::new(0, 0, top.buffer_width as i32, top.buffer_height as i32)))
}

fn add_tool(control: usize, info: &ToolInfo) -> usize {
    let tool = tool_from(info);
    with_state::<TipState, _>(control, |_, state| {
        if let Some(at) = find(state, tool.owner, tool.id) {
            state.tools[at] = Some(tool);
            return 1;
        }
        match state.tools.iter_mut().find(|slot| slot.is_none()) {
            Some(slot) => {
                *slot = Some(tool);
                1
            }
            None => 0,
        }
    })
    .unwrap_or(0)
}

fn pop(control: usize) {
    let now = super::super::user32::now_ms();
    with(|gui| {
        if let Some((shown, _, rect, _)) = gui.tip.shown
            && shown == control
        {
            hide(gui, rect, now);
        }
    });
    gui::wake();
}

fn tool_message(control: usize, number: u32, wparam: usize, lparam: usize) -> usize {
    use code::*;
    let info = read_tool_info(lparam);
    match number {
        TTM_ADDTOOLW | TTM_SETTOOLINFOW => info.map_or(0, |info| add_tool(control, &info)),
        TTM_DELTOOLW => {
            let Some(info) = info else {
                return 0;
            };
            pop(control);
            with_state::<TipState, _>(control, |_, state| {
                if let Some(at) = find(state, info.owner, info.id) {
                    state.tools[at] = None;
                }
            });
            0
        }
        TTM_NEWTOOLRECTW | TTM_UPDATETIPTEXTW => {
            let Some(info) = info else {
                return 0;
            };
            let replacement = tool_from(&info);
            with_state::<TipState, _>(control, |_, state| {
                let at = find(state, info.owner, info.id)?;
                let stored = state.tools[at].as_mut()?;
                match number {
                    TTM_NEWTOOLRECTW => stored.rect = info.rect,
                    _ => {
                        stored.text = replacement.text;
                        stored.length = replacement.length;
                        stored.callback = replacement.callback;
                    }
                }
                Some(())
            });
            0
        }
        TTM_GETTOOLINFOW | TTM_GETTEXTW => {
            let Some(info) = info else {
                return 0;
            };
            let found = with_state::<TipState, _>(control, |_, state| {
                find(state, info.owner, info.id).and_then(|at| state.tools[at])
            })
            .flatten();
            let Some(tool) = found else {
                return 0;
            };
            if number == TTM_GETTOOLINFOW {
                let mut answered = info;
                answered.flags = tool.flags;
                answered.rect = tool.rect;
                answered.lparam = tool.lparam;
                let size = (info.size as usize).min(TOOL_INFO_SIZE);
                // SAFETY: the program's `TOOLINFOW`, `cbSize` writable bytes.
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        (&raw const answered).cast::<u8>(),
                        lparam as *mut u8,
                        size,
                    )
                };
                return 1;
            }
            if info.text != 0 && info.text != CALLBACK_TEXT {
                let room = wparam.clamp(1, TEXT_UNITS + 1);
                let length = tool.length.min(room - 1);
                // SAFETY: `TTM_GETTEXTW`'s contract: `lpszText` holds `wParam` units.
                unsafe {
                    let out = info.text as *mut u16;
                    core::ptr::copy_nonoverlapping(tool.text.as_ptr(), out, length);
                    out.add(length).write(0);
                }
                return length;
            }
            0
        }
        _ => 0,
    }
}

fn set_title(control: usize, icon: usize, title: usize) -> usize {
    let units = match title {
        0 => &[][..],
        // SAFETY: `TTM_SETTITLEW`'s contract: a NUL-terminated title.
        address => unsafe { super::super::file::wide_string(address as *const u16) },
    };
    if units.len() > TITLE_UNITS {
        return 0;
    }
    with_state::<TipState, _>(control, |_, state| {
        state.title[..units.len()].copy_from_slice(units);
        state.title_length = units.len();
        state.icon = TipIcon::of(icon);
    });
    1
}

fn set_delay(control: usize, which: usize, value: usize) {
    let value = value as u16 as i16;
    with_state::<TipState, _>(control, |_, state| {
        let chosen = |default: u32| if value < 0 { default } else { value as u32 };
        match which {
            TTDT_AUTOMATIC => {
                let initial = chosen(DEFAULT_DELAYS[0]);
                state.delays = [initial, initial / 5, initial * 10];
            }
            TTDT_INITIAL => state.delays[0] = chosen(DEFAULT_DELAYS[0]),
            TTDT_RESHOW => state.delays[1] = chosen(DEFAULT_DELAYS[1]),
            TTDT_AUTOPOP => state.delays[2] = chosen(DEFAULT_DELAYS[2]),
            _ => {}
        }
    });
}

fn track_activate(control: usize, on: bool, lparam: usize) {
    let Some(info) = read_tool_info(lparam) else {
        return;
    };
    let found = with_state::<TipState, _>(control, |_, state| {
        (find(state, info.owner, info.id), state.track_point)
    });
    let Some((Some(tool), track)) = found else {
        return;
    };
    match on {
        true => {
            let point = track.or_else(|| {
                with(|gui| {
                    let entry = state_of(gui, control)?.tools[tool]?;
                    let rect = tool_rect(gui, &entry)?;
                    Some((rect.left, rect.bottom))
                })
                .flatten()
            });
            show(control, tool, point);
        }
        false => pop(control),
    }
}

fn track_position(control: usize, lparam: usize) {
    let (x, y) = (lparam as u16 as i16 as i32, (lparam >> 16) as u16 as i16 as i32);
    let root = with(|gui| {
        let root = windows::root_of(&gui.windows, control)?;
        Some((root, gui.windows.get(root)?.menu_height))
    })
    .flatten();
    let point = root.and_then(|(root, menu_height)| {
        let (left, top) = super::super::placement::client_origin(root).ok()?;
        Some((x - left, y - top + menu_height))
    });
    with_state::<TipState, _>(control, |_, state| state.track_point = point);
}

/// The window procedure of every tooltip control.
pub(super) fn procedure(window: usize, number: u32, wparam: usize, lparam: usize) -> usize {
    use code::*;
    match number {
        TTM_ACTIVATE => {
            with_state::<TipState, _>(window, |_, state| state.active = wparam != 0);
            if wparam == 0 {
                pop(window);
            }
            0
        }
        TTM_SETDELAYTIME => {
            set_delay(window, wparam, lparam);
            0
        }
        TTM_GETDELAYTIME => with_state::<TipState, _>(window, |_, state| match wparam {
            TTDT_RESHOW => state.delays[1],
            TTDT_AUTOPOP => state.delays[2],
            _ => state.delays[0],
        } as usize)
        .unwrap_or(0),
        TTM_RELAYEVENT => 0,
        TTM_GETTOOLCOUNT => with_state::<TipState, _>(window, |_, state| {
            state.tools.iter().flatten().count()
        })
        .unwrap_or(0),
        TTM_SETMAXTIPWIDTH => with_state::<TipState, _>(window, |_, state| {
            let previous = state.max_width;
            state.max_width = lparam as i32;
            previous as isize as usize
        })
        .unwrap_or(0),
        TTM_GETMAXTIPWIDTH => with_state::<TipState, _>(window, |_, state| {
            state.max_width as isize as usize
        })
        .unwrap_or(0),
        TTM_SETTITLEW => set_title(window, wparam, lparam),
        TTM_POP => {
            pop(window);
            0
        }
        TTM_POPUP | TTM_UPDATE => {
            let current = with(|gui| {
                let shown = gui.tip.shown.map(|(control, tool, _, _)| (control, tool));
                let resting = gui.tip.resting.map(|(control, tool, _)| (control, tool));
                shown
                    .or(resting)
                    .filter(|(control, _)| *control == window)
                    .map(|(_, tool)| tool)
            })
            .flatten();
            if let Some(tool) = current {
                show(window, tool, None);
            }
            0
        }
        TTM_TRACKACTIVATE => {
            track_activate(window, wparam != 0, lparam);
            0
        }
        TTM_TRACKPOSITION => {
            track_position(window, lparam);
            0
        }
        TTM_ADDTOOLW | TTM_DELTOOLW | TTM_NEWTOOLRECTW | TTM_GETTOOLINFOW | TTM_SETTOOLINFOW
        | TTM_GETTEXTW | TTM_UPDATETIPTEXTW => tool_message(window, number, wparam, lparam),
        _ => default_procedure(window, number, wparam, lparam),
    }
}

/// Replaces `owner`'s tools in tooltip control `control` by one callback tool per rectangle
/// (D356), each named by its id: the tools a toolbar keeps for its buttons.
pub(super) fn replace_tools(control: usize, owner: usize, tools: &[(usize, Rect)]) {
    with_state::<TipState, _>(control, |_, state| {
        for slot in state.tools.iter_mut() {
            if slot.is_some_and(|tool| tool.owner == owner) {
                *slot = None;
            }
        }
        let free = state.tools.iter_mut().filter(|slot| slot.is_none());
        for (slot, &(id, rect)) in free.zip(tools) {
            *slot = Some(Tool {
                owner,
                id,
                rect: [rect.left, rect.top, rect.right, rect.bottom],
                flags: 0,
                text: [0; TEXT_UNITS],
                length: 0,
                callback: true,
                lparam: 0,
            });
        }
    });
}
