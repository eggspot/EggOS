//! The system's control classes (D145, `docs/ux/windows-controls-v0.3.md`): `BUTTON`, `EDIT`,
//! `STATIC` and `LISTBOX` are child windows whose procedure is the layer's, over the models of
//! `eggos_windows_gui::controls`. Each control's state is a heap block its window's entry
//! points to, reached only under the GUI state's lock and looked up again by handle after every
//! call into the program, so a control the program destroys from a notification is never
//! touched again. Notifications are `WM_COMMAND`s sent to the parent. comctl32's common
//! controls (D178) are system classes too, answered by `common_controls`. A `STATIC` with
//! `SS_BITMAP` or `SS_ICON` (D178, design section 28) keeps the image `STM_SETIMAGE` gave it and
//! draws its pixels.

use super::dialog::{has_focus, release_capture, set_capture, set_focus};
use super::gui::{self, Gui, with};
use super::paint;
use super::user32::def_window_proc_w;
use super::{heap, window_text};
use crate::gui::message;
use crate::gui::objects::{self, Object};
use crate::gui::windows::{self, ControlKind, Procedure, Window};
use crate::text::equal_ignoring_ascii_case;
use eggos_element_tree::Action;
use eggos_windows_gui::colour::{colour_ref, pixel, system_colour};
use eggos_windows_gui::controls::button::{self, Button, ButtonKind, ButtonLook, ButtonOutcome};
use eggos_windows_gui::controls::edit::{
    self, Edit, EditLook, EditOutcome, Modifiers,
};
use eggos_windows_gui::controls::image::{self, Picture};
use eggos_windows_gui::controls::label::{self, LabelLook};
use eggos_windows_gui::controls::list_box::{self, ITEM_HEIGHT, ListBox, ListLook, visible_rows};
use eggos_windows_gui::controls::scroll_bar::{self as bar_model, BarLook, ScrollBar};
use eggos_windows_gui::controls::{DARK, LIGHT, Palette};
use eggos_windows_gui::surface::{Rect, Surface};

const VK_SPACE: usize = 0x20;
const VK_SHIFT: i32 = 0x10;
const VK_CONTROL: i32 = 0x11;
const LB_ERR: usize = usize::MAX;
const SCROLL_REPEAT_TIMER: usize = 0xffff_0227;
const COLOR_WINDOW: u32 = 5;
const COLOR_WINDOWTEXT: u32 = 8;
const COLOR_BTNFACE: u32 = 15;

/// `WM_CTLCOLOR*`, sent to a control's parent before it paints (design section 1).
pub mod control_colour {
    pub const MESSAGE_BOX: u32 = 0x0132;
    pub const EDIT: u32 = 0x0133;
    pub const LIST_BOX: u32 = 0x0134;
    pub const BUTTON: u32 = 0x0135;
    pub const STATIC: u32 = 0x0138;

    /// Whether `number` is one of the `WM_CTLCOLOR*` messages.
    pub fn is(number: u32) -> bool {
        (MESSAGE_BOX..=STATIC).contains(&number)
    }
}
/// The layer's own message that does an agent's action on a list box's item on the list's
/// thread (D227): `wParam` the item, `lParam` the [`Action`]'s number.
pub const ACT_ON_LIST_ITEM: u32 = 0x03f3;

/// The class messages and notification codes the controls answer and send.
pub mod code {
    pub const BM_GETCHECK: u32 = 0x00f0;
    pub const BM_SETCHECK: u32 = 0x00f1;
    pub const BM_GETSTATE: u32 = 0x00f2;
    pub const BM_SETSTATE: u32 = 0x00f3;
    pub const BM_SETSTYLE: u32 = 0x00f4;
    pub const BM_CLICK: u32 = 0x00f5;
    pub const EM_GETSEL: u32 = 0x00b0;
    pub const EM_SETSEL: u32 = 0x00b1;
    pub const EM_SCROLLCARET: u32 = 0x00b7;
    pub const EM_LINESCROLL: u32 = 0x00b6;
    pub const EM_GETLINECOUNT: u32 = 0x00ba;
    pub const EM_LINEINDEX: u32 = 0x00bb;
    pub const EM_LINELENGTH: u32 = 0x00c1;
    pub const EM_GETLINE: u32 = 0x00c4;
    pub const EM_LINEFROMCHAR: u32 = 0x00c9;
    pub const EM_GETFIRSTVISIBLELINE: u32 = 0x00ce;
    pub const EM_GETMODIFY: u32 = 0x00b8;
    pub const EM_SETMODIFY: u32 = 0x00b9;
    pub const EM_REPLACESEL: u32 = 0x00c2;
    pub const EM_LIMITTEXT: u32 = 0x00c5;
    pub const EM_SETPASSWORDCHAR: u32 = 0x00cc;
    pub const EM_SETREADONLY: u32 = 0x00cf;
    pub const EM_GETPASSWORDCHAR: u32 = 0x00d2;
    pub const EM_GETLIMITTEXT: u32 = 0x00d5;
    pub const LB_ADDSTRING: u32 = 0x0180;
    pub const LB_INSERTSTRING: u32 = 0x0181;
    pub const LB_DELETESTRING: u32 = 0x0182;
    pub const LB_RESETCONTENT: u32 = 0x0184;
    pub const LB_SETCURSEL: u32 = 0x0186;
    pub const LB_GETCURSEL: u32 = 0x0188;
    pub const LB_GETTEXT: u32 = 0x0189;
    pub const LB_GETTEXTLEN: u32 = 0x018a;
    pub const LB_GETCOUNT: u32 = 0x018b;
    pub const LB_GETITEMDATA: u32 = 0x0199;
    pub const LB_SETITEMDATA: u32 = 0x019a;
    pub const LB_GETITEMHEIGHT: u32 = 0x01a1;
    pub const BN_CLICKED: usize = 0;
    pub const STN_CLICKED: usize = 0;
    pub const STN_DBLCLK: usize = 1;
    pub const EN_SETFOCUS: usize = 0x0100;
    pub const EN_KILLFOCUS: usize = 0x0200;
    pub const EN_CHANGE: usize = 0x0300;
    pub const EN_UPDATE: usize = 0x0400;
    pub const LBN_SELCHANGE: usize = 1;
    pub const LBN_DBLCLK: usize = 2;
    pub const LBN_SETFOCUS: usize = 4;
    pub const LBN_KILLFOCUS: usize = 5;
}

const BST_CHECKED: usize = 1;
const BST_PUSHED: usize = 4;
const BST_FOCUS: usize = 8;

mod static_image {
    pub const TYPE: u32 = 0x1f;
    pub const ICON: u32 = 0x3;
    pub const BITMAP: u32 = 0xe;
    pub const REAL_SIZE_CONTROL: u32 = 0x40;
    pub const CENTER_IMAGE: u32 = 0x200;
    pub const STM_SETICON: u32 = 0x0170;
    pub const STM_GETICON: u32 = 0x0171;
    pub const STM_SETIMAGE: u32 = 0x0172;
    pub const STM_GETIMAGE: u32 = 0x0173;
    pub const IMAGE_BITMAP: usize = 0;
    pub const IMAGE_ICON: usize = 1;
    pub const IMAGE_CURSOR: usize = 2;
}

#[derive(Debug, Clone, Copy, Default)]
struct StaticPicture {
    image: usize,
    colour: usize,
    mask: usize,
}

fn is_picture(style: u32) -> bool {
    matches!(
        style & static_image::TYPE,
        static_image::ICON | static_image::BITMAP
    )
}

/// Bitmap `handle`'s pixels while the lock is held through `gui`; `None` for no such bitmap or
/// one that draws nowhere.
pub(super) fn picture_of(gui: &Gui, handle: usize) -> Option<Picture<'_>> {
    super::gdi::load_section(handle);
    loaded_picture(gui, handle)
}

/// Bitmaps `colour`'s and `mask`'s pixels, as `picture_of` gives them, both loaded from
/// their DIB sections before either is viewed: the two may be one section.
pub(super) fn pictures_of(
    gui: &Gui,
    colour: usize,
    mask: usize,
) -> (Option<Picture<'_>>, Option<Picture<'_>>) {
    super::gdi::load_section(colour);
    super::gdi::load_section(mask);
    (loaded_picture(gui, colour), loaded_picture(gui, mask))
}

fn loaded_picture(gui: &Gui, handle: usize) -> Option<Picture<'_>> {
    let Object::Bitmap(bitmap) = objects::object(&gui.objects, handle)? else {
        return None;
    };
    if bitmap.address == 0 {
        return None;
    }
    // SAFETY: a bitmap's pixels are a heap block of `width * height` pixels, freed only after
    // the bitmap left the object table, which cannot happen while the lock is held through `gui`.
    let pixels = unsafe {
        core::slice::from_raw_parts(
            bitmap.address as *const u32,
            bitmap.width as usize * bitmap.height as usize,
        )
    };
    Picture::new(pixels, bitmap.width as i32, bitmap.height as i32)
}

/// Draws the colour bitmap `colour`, masked by `mask` (0 for none), with its top-left at (`x`,
/// `y`) by the design's section 27 rule, as an icon is drawn.
pub(super) fn draw_bitmaps(
    gui: &Gui,
    surface: &mut Surface<'_>,
    (colour, mask): (usize, usize),
    x: i32,
    y: i32,
) {
    if let (Some(picture), mask) = pictures_of(gui, colour, mask) {
        image::draw_picture(surface, &picture, mask.as_ref(), x, y);
    }
}

/// The system class a class name names, compared ignoring case.
pub fn system_class(name: &[u16]) -> Option<ControlKind> {
    let is = |wanted: &str| {
        name.len() == wanted.len()
            && equal_ignoring_ascii_case(name.iter().map(|unit| *unit as u8), wanted.bytes())
            && name.iter().all(|unit| *unit < 0x80)
    };
    match () {
        () if is("BUTTON") => Some(ControlKind::Button),
        () if is("EDIT") => Some(ControlKind::Edit),
        () if is("STATIC") => Some(ControlKind::Static),
        () if is("LISTBOX") => Some(ControlKind::ListBox),
        () if is("COMBOBOX") => Some(ControlKind::ComboBox),
        () if is("SCROLLBAR") => Some(ControlKind::ScrollBar),
        () => super::common_controls::class_of(name).map(ControlKind::Common),
    }
}

fn allocate<T>(value: T) -> Option<usize> {
    let block = heap::allocate(core::mem::size_of::<T>()).cast::<T>();
    if block.is_null() {
        return None;
    }
    // SAFETY: a new, 16-byte aligned heap block the size of `T`.
    unsafe { block.write(value) };
    Some(block as usize)
}

/// A new control's state for a window of `style`; `None` without memory (0 for a static
/// without a picture, which keeps none).
pub fn new_state(kind: ControlKind, style: u32) -> Option<usize> {
    match kind {
        ControlKind::Static if is_picture(style) => allocate(StaticPicture::default()),
        ControlKind::Static => Some(0),
        ControlKind::Button => allocate(Button::new(ButtonKind::of(style))),
        ControlKind::Edit => {
            let address = heap::allocate(core::mem::size_of::<Edit>()).cast::<Edit>();
            if address.is_null() {
                return None;
            }
            // SAFETY: a new, 16-byte aligned heap block the size of an `Edit`.
            unsafe { address.write(Edit::with_style(style)) };
            Some(address as usize)
        }
        ControlKind::ScrollBar => allocate(ScrollBar::new(style & bar_model::style::VERTICAL != 0)),
        ControlKind::ListBox => {
            let address = heap::allocate(core::mem::size_of::<ListBox>()).cast::<ListBox>();
            if address.is_null() {
                return None;
            }
            // SAFETY: as for the edit.
            unsafe { address.write(ListBox::new()) };
            Some(address as usize)
        }
        ControlKind::ComboBox => super::combo_box::new_state(0, style),
        ControlKind::Common(kind) => super::common_controls::new_state(kind, style),
    }
}

/// Frees the state of a control whose window left the table.
pub fn free_state(state: usize) {
    if state != 0 {
        // SAFETY: the block is the one `new_state` made; its window is gone, so nothing
        // reaches it any more.
        unsafe { heap::free(state as *mut u8) };
    }
}

enum State<'a> {
    Button(&'a mut Button),
    Edit(&'a mut Edit),
    ListBox(&'a mut ListBox),
    ScrollBar(&'a mut ScrollBar),
    Picture(&'a mut StaticPicture),
    Static,
}

fn state_of(gui: &mut Gui, window: usize) -> Option<(&mut Window, State<'_>)> {
    let entry = gui.windows.get_mut(window)?;
    let Procedure::Control(kind, address) = entry.procedure else {
        return None;
    };
    let state = match kind {
        // SAFETY: a control's state is the heap block `new_state` made for its kind, freed
        // only after its window left the table; the caller holds the lock through `gui`, and
        // every access goes through here.
        ControlKind::Button => State::Button(unsafe { &mut *(address as *mut Button) }),
        // SAFETY: as for the button.
        ControlKind::Edit => State::Edit(unsafe { &mut *(address as *mut Edit) }),
        // SAFETY: as for the button.
        ControlKind::ListBox => State::ListBox(unsafe { &mut *(address as *mut ListBox) }),
        // SAFETY: as for the button.
        ControlKind::ScrollBar => State::ScrollBar(unsafe { &mut *(address as *mut ScrollBar) }),
        ControlKind::Static if address != 0 => {
            // SAFETY: as for the button: a picture static's state is a `StaticPicture`.
            State::Picture(unsafe { &mut *(address as *mut StaticPicture) })
        }
        ControlKind::Static => State::Static,
        ControlKind::ComboBox | ControlKind::Common(_) => return None,
    };
    Some((entry, state))
}

fn with_state<R>(window: usize, work: impl FnOnce(&mut Window, State<'_>) -> R) -> Option<R> {
    with(|gui| {
        let (entry, state) = state_of(gui, window)?;
        Some(work(entry, state))
    })
    .flatten()
}

/// Runs `work` on list box `window`'s entry and items, read under the lock `gui` holds;
/// `None` when `window` is not a list box.
pub(super) fn read_list<R>(
    gui: &Gui,
    window: usize,
    work: impl FnOnce(&Window, &ListBox) -> R,
) -> Option<R> {
    let entry = gui.windows.get(window)?;
    let Procedure::Control(ControlKind::ListBox, address) = entry.procedure else {
        return None;
    };
    // SAFETY: as `state_of`: the block is this list's, alive while its window is in the table,
    // and the lock is held through `gui`; only a shared reference is made here.
    let list = unsafe { &*(address as *const ListBox) };
    Some(work(entry, list))
}

/// Runs `work` on scroll bar control `window`'s model; `None` when it is not one.
pub(super) fn with_scroll_bar<R>(
    window: usize,
    work: impl FnOnce(&mut ScrollBar) -> R,
) -> Option<R> {
    with_state(window, |_, state| match state {
        State::ScrollBar(bar) => Some(work(bar)),
        _ => None,
    })
    .flatten()
}

/// Runs `work` on scroll bar control `window`'s model, read under the lock `gui` holds.
pub(super) fn read_scroll_bar<R>(
    gui: &Gui,
    window: usize,
    work: impl FnOnce(&ScrollBar) -> R,
) -> Option<R> {
    let entry = gui.windows.get(window)?;
    let Procedure::Control(ControlKind::ScrollBar, address) = entry.procedure else {
        return None;
    };
    // SAFETY: as `state_of`: the block is this scroll bar's, alive while its window is in the
    // table, and the lock is held through `gui`; only a shared reference is made here.
    Some(work(unsafe { &*(address as *const ScrollBar) }))
}

/// Whether button `window`, a check box or radio button, is checked.
pub(super) fn is_checked(window: &Window) -> bool {
    match window.procedure {
        Procedure::Control(ControlKind::Button, address) if address != 0 => {
            // SAFETY: as `state_of`: a button's state is the `Button` `new_state` made, alive
            // while its window is in the table, which the caller reads under the lock; only a
            // shared reference is made here.
            unsafe { &*(address as *const Button) }.checked
        }
        _ => false,
    }
}

/// Whether `edit` hides its text behind a password character (`ES_PASSWORD`, or
/// `EM_SETPASSWORDCHAR` with a character other than 0).
pub(super) fn conceals(edit: &Edit) -> bool {
    edit.password != 0
}

/// Whether edit control `window` hides its text behind a password character.
pub(super) fn conceals_text(window: &Window) -> bool {
    match window.procedure {
        Procedure::Control(ControlKind::Edit, address) if address != 0 => {
            // SAFETY: as `state_of`: an edit's state is the `Edit` `new_state` made, alive
            // while its window is in the table, which the caller reads under the lock; only a
            // shared reference is made here.
            conceals(unsafe { &*(address as *const Edit) })
        }
        _ => false,
    }
}

fn act_on_list_item(window: usize, item: usize, action: Action) {
    let enabled = with(|gui| gui.windows.get(window).is_some_and(|entry| entry.enabled()));
    if enabled != Some(true) {
        return;
    }
    let exists = with_state(window, |_, state| match state {
        State::ListBox(list) => item < list.count(),
        _ => false,
    });
    if exists != Some(true) {
        return;
    }
    set_focus(window);
    if action == Action::Focus {
        return;
    }
    let changed = with_state(window, |entry, state| match state {
        State::ListBox(list) if item < list.count() => {
            let changed = list.selected() != Some(item);
            list.select(Some(item), visible_rows(entry.height as i32));
            Some(changed)
        }
        _ => None,
    })
    .flatten();
    list_changed(window, changed);
    let notifies = with(|gui| {
        gui.windows
            .get(window)
            .is_some_and(|entry| entry.style & list_box::style::NOTIFY != 0)
    });
    if action == Action::Invoke && changed.is_some() && notifies == Some(true) {
        notify(window, code::LBN_DBLCLK);
    }
}

pub(super) fn invalidate(window: usize) {
    with(|gui| {
        if let Some(entry) = gui.windows.get_mut(window) {
            entry.invalidate(None, true);
        }
    });
    gui::wake();
}

pub(super) fn notify(window: usize, notification: usize) {
    let target = with(|gui| {
        let entry = gui.windows.get(window)?;
        Some((entry.parent, entry.id))
    })
    .flatten();
    if let Some((parent, id)) = target
        && parent != 0
    {
        gui::send(
            parent,
            message::COMMAND,
            message::word_pair(id, notification),
            window,
        );
    }
}

fn in_layer_dialog(gui: &Gui, window: usize) -> bool {
    crate::gui::windows::root_of(&gui.windows, window)
        .and_then(|root| gui.windows.get(root))
        .is_some_and(|root| matches!(root.procedure, Procedure::FilePicker(_)))
}

pub(super) fn palette_of(gui: &Gui, window: usize) -> &'static Palette {
    match in_layer_dialog(gui, window) {
        true => &DARK,
        false => &LIGHT,
    }
}

fn is_default_button(gui: &Gui, window: usize, kind: ButtonKind) -> bool {
    let Some(root) = crate::gui::windows::root_of(&gui.windows, window) else {
        return false;
    };
    let focus = gui.key_target(root);
    let focus_is_push = gui.windows.get(focus).is_some_and(|entry| {
        matches!(entry.procedure, Procedure::Control(ControlKind::Button, _))
            && ButtonKind::of(entry.style).is_push()
    });
    match kind {
        ButtonKind::DefaultPush => !focus_is_push || focus == window,
        ButtonKind::Push => focus == window,
        _ => false,
    }
}

fn colour_message(gui: &mut Gui, window: usize) -> Option<(usize, u32)> {
    if in_layer_dialog(gui, window) {
        return None;
    }
    let (entry, state) = state_of(gui, window)?;
    let parent = entry.parent;
    let enabled = entry.enabled();
    let number = match state {
        State::Button(button) if button.kind.is_push() => control_colour::BUTTON,
        State::Edit(edit) if enabled && !edit.read_only => control_colour::EDIT,
        State::ListBox(_) => control_colour::LIST_BOX,
        State::ScrollBar(_) | State::Picture(_) => return None,
        State::Button(_) | State::Edit(_) | State::Static => control_colour::STATIC,
    };
    (parent != 0).then_some((parent, number))
}

fn program_colours(window: usize, hdc: usize) -> Option<(u32, Option<u32>, u32)> {
    let (parent, number) = with(|gui| colour_message(gui, window)).flatten()?;
    let brush = gui::send(parent, number, hdc, window);
    with(|gui| {
        let fill = objects::brush_colour(&gui.objects, brush);
        let text = gui.contexts.get(hdc)?.text_colour;
        Some((number, fill, pixel(text)))
    })
    .flatten()
}

fn answered_palette(base: &Palette, (number, fill, text): (u32, Option<u32>, u32)) -> Palette {
    let mut palette = *base;
    match number {
        control_colour::BUTTON => {
            palette.text = text;
            palette.default_text = text;
        }
        control_colour::EDIT | control_colour::LIST_BOX => {
            palette.text = text;
            palette.field_fill = fill.unwrap_or(palette.field_fill);
        }
        _ => {
            palette.text = text;
            palette.surface = fill.unwrap_or(palette.surface);
            palette.field_read_only_fill = fill.unwrap_or(palette.field_read_only_fill);
        }
    }
    palette
}

/// `DefWindowProc`'s answer to `WM_CTLCOLOR*`: `COLOR_WINDOW` for edits and lists,
/// `COLOR_BTNFACE` for the rest, with `COLOR_WINDOWTEXT` set in `hdc`.
pub(super) fn default_control_colour(number: u32, hdc: usize) -> usize {
    with(|gui| {
        if let Some(context) = gui.contexts.get_mut(hdc) {
            context.text_colour = system_colour(COLOR_WINDOWTEXT).map_or(0, colour_ref);
        }
    });
    let index = match number {
        control_colour::EDIT | control_colour::LIST_BOX => COLOR_WINDOW,
        _ => COLOR_BTNFACE,
    };
    objects::system_brush_handle(index).unwrap_or(0)
}

fn paint_control(window: usize) {
    let Ok((hdc, _, _)) = paint::begin(window) else {
        return;
    };
    let answered = program_colours(window, hdc);
    let _ = paint::draw_in(hdc, |surface, _, gui| {
        let Some(surface) = surface else {
            return;
        };
        let Some(entry) = gui.windows.get(window) else {
            return;
        };
        let Procedure::Control(kind, address) = entry.procedure else {
            return;
        };
        let rect = entry.client();
        let custom = answered.map(|answer| answered_palette(palette_of(gui, window), answer));
        let palette = custom.as_ref().unwrap_or(palette_of(gui, window));
        let focused = has_focus(gui, window);
        let hovered = gui.hover.under() == window;
        let text = window_text::of(gui, window);
        match kind {
            ControlKind::Button => {
                // SAFETY: as `state_of`: the block is this control's, alive while its window is
                // in the table, and the lock is held; only a shared reference is made here.
                let model = unsafe { &*(address as *const Button) };
                let look = ButtonLook {
                    kind: model.kind,
                    checked: model.checked,
                    pressed: model.pressed,
                    focused,
                    default: is_default_button(gui, window, model.kind),
                    enabled: entry.enabled(),
                    hovered,
                };
                button::paint(surface, rect, text, look, palette);
            }
            ControlKind::Edit => {
                // SAFETY: as above.
                let model = unsafe { &*(address as *const Edit) };
                let look = EditLook {
                    focused,
                    enabled: entry.enabled(),
                    hovered,
                };
                edit::paint(surface, rect, model, look, palette);
            }
            ControlKind::ListBox => {
                // SAFETY: as above.
                let model = unsafe { &*(address as *const ListBox) };
                let look = ListLook {
                    focused,
                    enabled: entry.enabled(),
                };
                list_box::paint(surface, rect, model, look, palette);
            }
            ControlKind::ScrollBar => {
                // SAFETY: as above.
                let model = unsafe { &*(address as *const ScrollBar) };
                let look = BarLook {
                    enabled: entry.enabled(),
                    focused,
                    hovered,
                };
                bar_model::paint(surface, rect, model, look, palette);
            }
            ControlKind::Static if address != 0 => {
                // SAFETY: as above; a picture static's state is a `StaticPicture`.
                let picture = unsafe { &*(address as *const StaticPicture) };
                paint_picture(surface, rect, entry.style, picture, (palette, gui));
            }
            ControlKind::Static => {
                let look = LabelLook {
                    style: entry.style,
                    enabled: entry.enabled(),
                };
                label::paint(surface, rect, text, look, palette);
            }
            ControlKind::ComboBox | ControlKind::Common(_) => {}
        }
    });
    paint::finish(hdc);
}

fn paint_picture(
    surface: &mut Surface<'_>,
    rect: Rect,
    style: u32,
    picture: &StaticPicture,
    (palette, gui): (&Palette, &Gui),
) {
    let saved = surface.clip();
    surface.clip_to(rect);
    surface.fill(rect, palette.surface);
    if let (Some(colour), mask) = pictures_of(gui, picture.colour, picture.mask) {
        let bitmap = style & static_image::TYPE == static_image::BITMAP;
        match () {
            () if bitmap && style & static_image::REAL_SIZE_CONTROL != 0 => {
                image::draw_stretched(surface, &colour, rect);
            }
            () if style & static_image::CENTER_IMAGE != 0 => image::draw_picture(
                surface,
                &colour,
                mask.as_ref(),
                rect.left + (rect.width() - colour.width) / 2,
                rect.top + (rect.height() - colour.height) / 2,
            ),
            () => image::draw_picture(surface, &colour, mask.as_ref(), rect.left, rect.top),
        }
    }
    surface.set_clip(saved);
}

fn give_picture_state(window: usize) -> bool {
    const PLAIN_STATIC: Procedure = Procedure::Control(ControlKind::Static, 0);
    let plain = |gui: &mut Gui| {
        gui.windows
            .get(window)
            .is_some_and(|entry| entry.procedure == PLAIN_STATIC)
    };
    if with(plain) != Some(true) {
        return true;
    }
    let Some(address) = allocate(StaticPicture::default()) else {
        return false;
    };
    let placed = with(|gui| {
        if !plain(gui) {
            return false;
        }
        let Some(entry) = gui.windows.get_mut(window) else {
            return false;
        };
        entry.procedure = Procedure::Control(ControlKind::Static, address);
        true
    });
    if placed != Some(true) {
        free_state(address);
    }
    true
}

fn picture_message(window: usize, number: u32, wparam: usize, lparam: usize) -> usize {
    let (setting, kind, handle) = match number {
        static_image::STM_SETICON => (true, static_image::IMAGE_ICON, wparam),
        static_image::STM_GETICON => (false, static_image::IMAGE_ICON, 0),
        static_image::STM_GETIMAGE => (false, wparam, 0),
        static_image::STM_SETIMAGE => (true, wparam, lparam),
        _ => return 0,
    };
    let style = with(|gui| gui.windows.get(window).map(|entry| entry.style))
        .flatten()
        .unwrap_or(0);
    let fits = match style & static_image::TYPE {
        static_image::BITMAP => kind == static_image::IMAGE_BITMAP,
        static_image::ICON => matches!(kind, static_image::IMAGE_ICON | static_image::IMAGE_CURSOR),
        _ => false,
    };
    if !fits {
        return 0;
    }
    if !setting {
        return with_state(window, |_, state| match state {
            State::Picture(picture) => picture.image,
            _ => 0,
        })
        .unwrap_or(0);
    }
    if !give_picture_state(window) {
        return 0;
    }
    let (colour, mask) = match kind {
        static_image::IMAGE_BITMAP => (handle, 0),
        _ if handle == 0 => (0, 0),
        _ => super::icon::icon_bitmaps(handle).unwrap_or((0, 0)),
    };
    let resizes = style & (static_image::CENTER_IMAGE | static_image::REAL_SIZE_CONTROL) == 0;
    let previous = with(|gui| {
        let size = picture_of(gui, colour).map(|picture| (picture.width, picture.height));
        let (entry, state) = state_of(gui, window)?;
        let State::Picture(picture) = state else {
            return None;
        };
        let new = StaticPicture {
            image: handle,
            colour,
            mask,
        };
        let previous = core::mem::replace(picture, new).image;
        let old = entry.bounds;
        if resizes && let Some((width, height)) = size {
            entry.bounds = Rect::sized(old.left, old.top, width, height);
            entry.width = width as u32;
            entry.height = height as u32;
        }
        let (parent, changed) = (entry.parent, old.union(&entry.bounds));
        windows::invalidate_tree(&mut gui.windows, parent, Some(changed), true);
        Some(previous)
    })
    .flatten();
    gui::wake();
    previous.unwrap_or(0)
}

pub(super) fn point(lparam: usize) -> (i32, i32) {
    (
        lparam as u16 as i16 as i32,
        (lparam >> 16) as u16 as i16 as i32,
    )
}

fn button_outcome(window: usize, outcome: ButtonOutcome) {
    match outcome {
        ButtonOutcome::Unchanged => {}
        ButtonOutcome::Repaint => invalidate(window),
        ButtonOutcome::Clicked => {
            invalidate(window);
            clear_radio_group(window);
            notify(window, code::BN_CLICKED);
        }
    }
}

fn clear_radio_group(window: usize) {
    let cleared = with(|gui| {
        let kind = ButtonKind::of(gui.windows.get(window)?.style);
        if kind != ButtonKind::AutoRadio {
            return None;
        }
        let (members, count) = windows::group_of(&gui.windows, window);
        let mut cleared = [0usize; windows::MAX_WINDOWS];
        let mut length = 0;
        for &member in members[..count].iter().filter(|&&member| member != window) {
            let Some(entry) = gui.windows.get(member) else {
                continue;
            };
            let Procedure::Control(ControlKind::Button, address) = entry.procedure else {
                continue;
            };
            if address == 0 || !ButtonKind::of(entry.style).is_radio() {
                continue;
            }
            // SAFETY: as `is_checked`: a button's state is the `Button` `new_state` made, alive
            // while its window is in the table, whose lock `with` holds.
            let button = unsafe { &mut *(address as *mut Button) };
            if button.checked {
                button.checked = false;
                cleared[length] = member;
                length += 1;
            }
        }
        Some((cleared, length))
    })
    .flatten();
    if let Some((cleared, length)) = cleared {
        for &member in &cleared[..length] {
            invalidate(member);
        }
    }
}

fn edit_outcome(window: usize, outcome: Option<EditOutcome>) {
    match outcome {
        Some(EditOutcome::Changed) => {
            invalidate(window);
            notify(window, code::EN_UPDATE);
            notify(window, code::EN_CHANGE);
        }
        Some(EditOutcome::Moved) => invalidate(window),
        _ => {}
    }
}

fn list_changed(window: usize, changed: Option<bool>) {
    if changed.is_none() {
        return;
    }
    invalidate(window);
    let notifies = with(|gui| {
        gui.windows
            .get(window)
            .is_some_and(|entry| entry.style & list_box::style::NOTIFY != 0)
    })
    .unwrap_or(false);
    if changed == Some(true) && notifies {
        notify(window, code::LBN_SELCHANGE);
    }
}

/// The window procedure of every control of `kind`.
pub fn procedure(
    window: usize,
    kind: ControlKind,
    number: u32,
    wparam: usize,
    lparam: usize,
) -> usize {
    match kind {
        ControlKind::ComboBox => {
            return super::combo_box::procedure(window, number, wparam, lparam);
        }
        ControlKind::Common(kind) => {
            return super::common_controls::procedure(window, kind, number, wparam, lparam);
        }
        _ => {}
    }
    match number {
        message::NC_CREATE => 1,
        message::CREATE => 0,
        message::PAINT => {
            paint_control(window);
            0
        }
        message::ERASE_BACKGROUND => 1,
        message::SET_FONT | message::GET_FONT => 0,
        message::ENABLE => {
            invalidate(window);
            0
        }
        message::SET_TEXT => set_text(window, kind, lparam),
        message::GET_TEXT => get_text(window, kind, wparam, lparam),
        message::GET_TEXT_LENGTH => match kind {
            ControlKind::Edit => with_state(window, |_, state| match state {
                State::Edit(edit) => edit.text().len(),
                _ => 0,
            })
            .unwrap_or(0),
            _ => window_text::length_default(window),
        },
        message::SET_FOCUS | message::KILL_FOCUS => {
            let gained = number == message::SET_FOCUS;
            if !gained {
                with_state(window, |_, state| {
                    if let State::Button(button) = state {
                        button.cancel();
                    }
                });
            }
            invalidate(window);
            match (kind, gained) {
                (ControlKind::Edit, true) => notify(window, code::EN_SETFOCUS),
                (ControlKind::Edit, false) => notify(window, code::EN_KILLFOCUS),
                (ControlKind::ListBox, focus) => {
                    let notifies = with(|gui| {
                        gui.windows
                            .get(window)
                            .is_some_and(|entry| entry.style & list_box::style::NOTIFY != 0)
                    });
                    if notifies == Some(true) {
                        notify(
                            window,
                            if focus {
                                code::LBN_SETFOCUS
                            } else {
                                code::LBN_KILLFOCUS
                            },
                        );
                    }
                }
                _ => {}
            }
            0
        }
        message::LEFT_BUTTON_DOWN => {
            left_button_down(window, kind, point(lparam));
            0
        }
        message::LEFT_BUTTON_DOUBLE_CLICK => {
            double_click(window, kind, point(lparam));
            0
        }
        message::MOUSE_MOVE => {
            pointer_moved(window, point(lparam));
            0
        }
        message::LEFT_BUTTON_UP => {
            left_button_up(window, kind, point(lparam));
            0
        }
        message::KEY_DOWN | message::KEY_UP => {
            key(window, number == message::KEY_DOWN, wparam);
            0
        }
        message::CHAR => {
            let outcome = with_state(window, |entry, state| match state {
                State::Edit(edit) => {
                    Some(edit.character(wparam as u32, edit_view(entry)))
                }
                _ => None,
            })
            .flatten();
            edit_outcome(window, outcome);
            0
        }
        number if (code::BM_GETCHECK..=code::BM_CLICK).contains(&number) => {
            button_message(window, number, wparam)
        }
        code::EM_GETSEL
        | code::EM_SETSEL
        | code::EM_REPLACESEL
        | code::EM_SETPASSWORDCHAR
        | code::EM_GETPASSWORDCHAR
            if kind == ControlKind::Edit =>
        {
            selection_message(window, number, wparam, lparam)
        }
        code::EM_LINESCROLL
        | code::EM_SCROLLCARET
        | code::EM_GETLINECOUNT
        | code::EM_LINEINDEX
        | code::EM_LINELENGTH
        | code::EM_GETLINE
        | code::EM_LINEFROMCHAR
        | code::EM_GETFIRSTVISIBLELINE
            if kind == ControlKind::Edit =>
        {
            line_message(window, number, wparam, lparam)
        }
        code::EM_LIMITTEXT
        | code::EM_GETLIMITTEXT
        | code::EM_SETREADONLY
        | code::EM_GETMODIFY
        | code::EM_SETMODIFY => {
            with_state(window, |_, state| match (state, number) {
                (State::Edit(edit), code::EM_GETMODIFY) => usize::from(edit.modified),
                (State::Edit(edit), code::EM_SETMODIFY) => {
                    edit.modified = wparam != 0;
                    0
                }
                (State::Edit(edit), code::EM_LIMITTEXT) => {
                    edit.set_limit(wparam);
                    0
                }
                (State::Edit(edit), code::EM_GETLIMITTEXT) => edit.limit(),
                (State::Edit(edit), _) => {
                    edit.read_only = wparam != 0;
                    1
                }
                _ => 0,
            })
            .inspect(|_| invalidate(window))
            .unwrap_or(0)
        }
        number
            if (code::LB_ADDSTRING..=code::LB_GETITEMHEIGHT).contains(&number)
                && kind == ControlKind::ListBox =>
        {
            list_message(window, number, wparam, lparam)
        }
        static_image::STM_SETICON..=static_image::STM_GETIMAGE if kind == ControlKind::Static => {
            picture_message(window, number, wparam, lparam)
        }
        number
            if (super::scroll_bar::SBM_SETPOS..=super::scroll_bar::SBM_GETSCROLLINFO)
                .contains(&number)
                && kind == ControlKind::ScrollBar =>
        {
            super::scroll_bar::control_message(window, number, wparam, lparam)
        }
        message::TIMER if kind == ControlKind::ScrollBar && wparam == SCROLL_REPEAT_TIMER => {
            repeat_scroll(window);
            0
        }
        ACT_ON_LIST_ITEM if kind == ControlKind::ListBox => {
            if let Some(action) = Action::from_value(lparam as u32) {
                act_on_list_item(window, wparam, action);
            }
            0
        }
        _ => def_window_proc_w(window, number, wparam, lparam),
    }
}

fn set_text(window: usize, kind: ControlKind, text: usize) -> usize {
    if kind != ControlKind::Edit {
        // SAFETY: `WM_SETTEXT`'s `lParam` is 0 or a NUL-terminated string.
        let result = unsafe { window_text::set_default(window, text) };
        invalidate(window);
        return result;
    }
    let units = match text {
        0 => &[][..],
        // SAFETY: as above.
        text => unsafe { super::file::wide_string(text as *const u16) },
    };
    let set = with_state(window, |_, state| {
        if let State::Edit(edit) = state {
            edit.set_text(units);
        }
    });
    if set.is_some() {
        edit_outcome(window, Some(EditOutcome::Changed));
    }
    1
}

fn get_text(window: usize, kind: ControlKind, capacity: usize, buffer: usize) -> usize {
    if kind != ControlKind::Edit {
        // SAFETY: `WM_GETTEXT`'s contract: `buffer` holds `capacity` writable units.
        return unsafe { window_text::get_default(window, capacity, buffer) };
    }
    with_state(window, |_, state| match state {
        // SAFETY: as above.
        State::Edit(edit) => unsafe { window_text::copy_out(edit.text(), buffer, capacity) },
        _ => 0,
    })
    .unwrap_or(0)
}

fn left_button_down(window: usize, kind: ControlKind, (x, y): (i32, i32)) {
    let takes = with(|gui| {
        gui.windows.get(window).is_some_and(|entry| {
            entry.enabled()
                && !(kind == ControlKind::Button
                    && ButtonKind::of(entry.style) == ButtonKind::GroupBox)
        })
    });
    if takes != Some(true) {
        return;
    }
    if kind != ControlKind::Static {
        set_focus(window);
    }
    match kind {
        ControlKind::Button => {
            set_capture(window);
            let outcome = with_state(window, |entry, state| match state {
                State::Button(button) => button.left_button(entry.client().contains(x, y), true),
                _ => ButtonOutcome::Unchanged,
            });
            button_outcome(window, outcome.unwrap_or(ButtonOutcome::Unchanged));
        }
        ControlKind::Edit => {
            set_capture(window);
            let extend = held(VK_SHIFT);
            let outcome = with_state(window, |entry, state| match state {
                State::Edit(edit) => Some(edit.click((x, y), extend, edit_view(entry))),
                _ => None,
            })
            .flatten();
            edit_outcome(window, outcome);
        }
        ControlKind::ScrollBar => {
            set_capture(window);
            let scroll = with_state(window, |entry, state| match state {
                State::ScrollBar(bar) => {
                    let (offset, length) = along(bar, entry, (x, y));
                    bar.button_down(offset, length)
                }
                _ => None,
            })
            .flatten();
            invalidate(window);
            if scroll.is_some() {
                repeat_after(window, bar_model::REPEAT_DELAY_MS);
            }
            super::scroll_bar::send_scroll(window, scroll);
        }
        ControlKind::ListBox => {
            let changed = with_state(window, |entry, state| match state {
                State::ListBox(list) => {
                    let height = entry.height as i32;
                    list_box::row_at(y, height).map(|row| list.click_row(row, visible_rows(height)))
                }
                _ => None,
            })
            .flatten();
            list_changed(window, changed);
        }
        ControlKind::Static | ControlKind::ComboBox | ControlKind::Common(_) => {}
    }
}

fn repeat_after(window: usize, interval: u32) {
    with(|gui| {
        let thread = gui.windows.get(window)?.thread;
        let now = super::user32::now_ms();
        gui.timers
            .set(window, SCROLL_REPEAT_TIMER, interval, 0, thread, now)
    });
    gui::wake();
}

fn stop_repeat(window: usize) {
    with(|gui| {
        let thread = gui.windows.get(window).map_or(0, |entry| entry.thread);
        gui.timers.kill(window, SCROLL_REPEAT_TIMER, thread)
    });
}

fn repeat_scroll(window: usize) {
    let repeated = with_state(window, |entry, state| match state {
        State::ScrollBar(bar) if bar.pressed().is_some() => {
            let length = match bar.vertical {
                true => entry.height as i32,
                false => entry.width as i32,
            };
            Some(bar.repeat(length))
        }
        _ => None,
    })
    .flatten();
    match repeated {
        Some(scroll) => {
            repeat_after(window, bar_model::REPEAT_INTERVAL_MS);
            super::scroll_bar::send_scroll(window, scroll);
        }
        None => stop_repeat(window),
    }
}

fn held(virtual_key: i32) -> bool {
    super::placement::key_state(virtual_key) < 0
}

fn along(bar: &ScrollBar, entry: &Window, (x, y): (i32, i32)) -> (i32, i32) {
    match bar.vertical {
        true => (y, entry.height as i32),
        false => (x, entry.width as i32),
    }
}

fn double_click(window: usize, kind: ControlKind, (x, y): (i32, i32)) {
    let notifies = |style_bit: u32| {
        with(|gui| {
            gui.windows
                .get(window)
                .is_some_and(|entry| entry.style & style_bit != 0 && entry.enabled())
        })
        .unwrap_or(false)
    };
    match kind {
        ControlKind::Button | ControlKind::ScrollBar => left_button_down(window, kind, (x, y)),
        ControlKind::ListBox => {
            left_button_down(window, kind, (x, y));
            let on_row = with(|gui| {
                gui.windows
                    .get(window)
                    .and_then(|entry| list_box::row_at(y, entry.height as i32))
            })
            .flatten()
            .is_some();
            if on_row && notifies(list_box::style::NOTIFY) {
                notify(window, code::LBN_DBLCLK);
            }
        }
        ControlKind::Edit => {
            let enabled = with(|gui| gui.windows.get(window).is_some_and(|entry| entry.enabled()));
            if enabled != Some(true) {
                return;
            }
            set_focus(window);
            let outcome = with_state(window, |entry, state| match state {
                State::Edit(edit) => Some(edit.select_word((x, y), edit_view(entry))),
                _ => None,
            })
            .flatten();
            edit_outcome(window, outcome);
        }
        ControlKind::Static => {
            if notifies(label::style::NOTIFY) {
                notify(window, code::STN_DBLCLK);
            }
        }
        ControlKind::ComboBox | ControlKind::Common(_) => {}
    }
}

fn pointer_moved(window: usize, (x, y): (i32, i32)) {
    let captured = with(|gui| {
        crate::gui::windows::root_of(&gui.windows, window)
            .and_then(|root| gui.windows.get(root))
            .is_some_and(|root| root.capture == window)
    })
    .unwrap_or(false);
    enum Effect {
        Button(ButtonOutcome),
        Edit(EditOutcome),
        Scroll(Option<bar_model::Scroll>),
        Hover(bool),
    }
    let effect = with_state(window, |entry, state| match state {
        State::Button(button) => {
            Effect::Button(button.pointer_moved(entry.client().contains(x, y)))
        }
        State::Edit(edit) if captured => {
            Effect::Edit(edit.drag((x, y), edit_view(entry)))
        }
        State::ScrollBar(bar) if captured => {
            let (offset, length) = along(bar, entry, (x, y));
            Effect::Scroll(bar.pointer_moved(offset, length))
        }
        State::ScrollBar(bar) => {
            let (offset, length) = along(bar, entry, (x, y));
            Effect::Hover(bar.hover(Some(offset), length))
        }
        _ => Effect::Button(ButtonOutcome::Unchanged),
    });
    match effect {
        Some(Effect::Button(outcome)) => button_outcome(window, outcome),
        Some(Effect::Edit(outcome)) => edit_outcome(window, Some(outcome)),
        Some(Effect::Scroll(scroll)) => {
            if scroll.is_some() {
                invalidate(window);
            }
            super::scroll_bar::send_scroll(window, scroll);
        }
        Some(Effect::Hover(true)) => invalidate(window),
        Some(Effect::Hover(false)) | None => {}
    }
}

fn selection_message(window: usize, number: u32, wparam: usize, lparam: usize) -> usize {
    let replacement = match (number, lparam) {
        (code::EM_REPLACESEL, 0) => &[][..],
        // SAFETY: `EM_REPLACESEL`'s `lParam` is a NUL-terminated string.
        (code::EM_REPLACESEL, text) => unsafe { super::file::wide_string(text as *const u16) },
        _ => &[][..],
    };
    let answer = with_state(window, |entry, state| {
        let State::Edit(edit) = state else {
            return (0, None);
        };
        let visible = edit_view(entry);
        match number {
            code::EM_GETSEL => {
                let (start, end) = edit.selection();
                for (out, value) in [(wparam, start), (lparam, end)] {
                    if out != 0 {
                        // SAFETY: `EM_GETSEL`'s contract: each non-null parameter points to a
                        // writable `DWORD`.
                        unsafe { (out as *mut u32).write_unaligned(value as u32) };
                    }
                }
                (message::word_pair(start, end), None)
            }
            code::EM_SETSEL => {
                edit.set_selection(wparam as i32 as isize, lparam as i32 as isize, visible);
                (0, Some(EditOutcome::Moved))
            }
            code::EM_REPLACESEL => (0, Some(edit.replace_selection(replacement, visible))),
            code::EM_SETPASSWORDCHAR => {
                edit.password = wparam as u16;
                (0, Some(EditOutcome::Moved))
            }
            _ => (usize::from(edit.password), None),
        }
    });
    let (result, outcome) = answer.unwrap_or((0, None));
    edit_outcome(window, outcome);
    result
}


fn edit_view(entry: &Window) -> edit::View {
    edit::view(entry.width as i32, entry.height as i32)
}

/// The edit line messages (D341); a single-line edit is one line. A `wParam` of -1 names the
/// caret's line, or for `EM_LINELENGTH` the unselected characters of the selection's lines.
fn line_message(window: usize, number: u32, wparam: usize, lparam: usize) -> usize {
    let given = match wparam as i32 {
        -1 => None,
        value => Some(value as u32 as usize),
    };
    let answer = with_state(window, |entry, state| {
        let State::Edit(edit) = state else {
            return (0, None);
        };
        let (start, end) = edit.selection();
        match number {
            code::EM_GETLINECOUNT => (edit.line_count(), None),
            code::EM_LINEINDEX => {
                let line = given.unwrap_or_else(|| edit.line_of(edit.caret()));
                (edit.line_index(line).unwrap_or(usize::MAX), None)
            }
            code::EM_LINELENGTH => match given {
                Some(index) => (edit.line_end_at(index) - edit.line_start_at(index), None),
                None => (
                    start - edit.line_start_at(start) + edit.line_end_at(end) - end,
                    None,
                ),
            },
            code::EM_LINEFROMCHAR => (edit.line_of(given.unwrap_or(start)), None),
            code::EM_GETLINE => (copy_line(edit, wparam, lparam), None),
            code::EM_GETFIRSTVISIBLELINE => (edit.first_visible_line(), None),
            code::EM_LINESCROLL => {
                let moved = edit.scroll_by(wparam as i32 as isize, lparam as i32 as isize);
                (usize::from(edit.multiline), moved.then_some(EditOutcome::Moved))
            }
            _ => {
                edit.scroll_to_caret(edit_view(entry));
                (0, Some(EditOutcome::Moved))
            }
        }
    });
    let (result, outcome) = answer.unwrap_or((0, None));
    edit_outcome(window, outcome);
    result
}

fn copy_line(edit: &Edit, line: usize, buffer: usize) -> usize {
    let line = match edit.multiline {
        true => line,
        false => 0,
    };
    let Some(units) = edit.line(line) else {
        return 0;
    };
    if buffer == 0 {
        return 0;
    }
    // SAFETY: `EM_GETLINE`'s contract: `lParam` points to a buffer whose first `WORD` is the
    // characters it holds.
    let room = usize::from(unsafe { (buffer as *const u16).read_unaligned() });
    let copied = units.len().min(room);
    // SAFETY: as above, the buffer holds `room` characters, and the edit's text is not in it.
    unsafe {
        core::ptr::copy_nonoverlapping(units.as_ptr().cast::<u8>(), buffer as *mut u8, copied * 2)
    };
    copied
}
fn left_button_up(window: usize, kind: ControlKind, (x, y): (i32, i32)) {
    match kind {
        ControlKind::Edit => {
            release_capture();
        }
        ControlKind::ScrollBar => {
            release_capture();
            stop_repeat(window);
            let scrolls = with_state(window, |_, state| match state {
                State::ScrollBar(bar) => bar.button_up(),
                _ => [None, None],
            });
            invalidate(window);
            for scroll in scrolls.unwrap_or([None, None]) {
                super::scroll_bar::send_scroll(window, scroll);
            }
        }
        ControlKind::Button => {
            release_capture();
            let outcome = with_state(window, |entry, state| match state {
                State::Button(button) => button.left_button(entry.client().contains(x, y), false),
                _ => ButtonOutcome::Unchanged,
            });
            button_outcome(window, outcome.unwrap_or(ButtonOutcome::Unchanged));
        }
        ControlKind::Static => {
            let notifies = with(|gui| {
                gui.windows
                    .get(window)
                    .is_some_and(|entry| entry.style & label::style::NOTIFY != 0)
            });
            if notifies == Some(true) {
                notify(window, code::STN_CLICKED);
            }
        }
        _ => {}
    }
}

fn key(window: usize, down: bool, virtual_key: usize) {
    enum Effect {
        Button(ButtonOutcome),
        Edit(Option<EditOutcome>),
        List(Option<bool>),
        Scroll(Option<bar_model::Scroll>),
        None,
    }
    let modifiers = Modifiers {
        shift: held(VK_SHIFT),
        control: held(VK_CONTROL),
    };
    let effect = with_state(window, |entry, state| match state {
        State::Button(button) if virtual_key == VK_SPACE => Effect::Button(button.space(down)),
        State::Edit(edit) if down => Effect::Edit(Some(edit.key_with(
            virtual_key as u32,
            modifiers,
            edit_view(entry),
        ))),
        State::ScrollBar(bar) if down => Effect::Scroll(bar.key(virtual_key as u32)),
        State::ListBox(list) if down => {
            let changed = list.key(virtual_key as u32, visible_rows(entry.height as i32));
            Effect::List(Some(changed))
        }
        _ => Effect::None,
    });
    match effect {
        Some(Effect::Button(outcome)) => button_outcome(window, outcome),
        Some(Effect::Edit(outcome)) => edit_outcome(window, outcome),
        Some(Effect::List(Some(true))) => list_changed(window, Some(true)),
        Some(Effect::Scroll(scroll)) => super::scroll_bar::send_scroll(window, scroll),
        _ => {}
    }
}

fn button_message(window: usize, number: u32, wparam: usize) -> usize {
    let focused = with(|gui| has_focus(gui, window)).unwrap_or(false);
    let answer = with_state(window, |entry, state| {
        let State::Button(button) = state else {
            return (0, ButtonOutcome::Unchanged);
        };
        match number {
            code::BM_GETCHECK => (usize::from(button.checked), ButtonOutcome::Unchanged),
            code::BM_SETCHECK => {
                button.checked = wparam == BST_CHECKED;
                (0, ButtonOutcome::Repaint)
            }
            code::BM_GETSTATE => {
                let mut state = usize::from(button.checked);
                if button.pressed {
                    state |= BST_PUSHED;
                }
                if focused {
                    state |= BST_FOCUS;
                }
                (state, ButtonOutcome::Unchanged)
            }
            code::BM_SETSTATE => {
                button.pressed = wparam != 0;
                (0, ButtonOutcome::Repaint)
            }
            code::BM_SETSTYLE => {
                entry.style =
                    (entry.style & !button::style::TYPE) | (wparam as u32 & button::style::TYPE);
                button.kind = ButtonKind::of(entry.style);
                (0, ButtonOutcome::Repaint)
            }
            _ => (0, button.click()),
        }
    });
    let (result, outcome) = answer.unwrap_or((0, ButtonOutcome::Unchanged));
    button_outcome(window, outcome);
    result
}

fn list_message(window: usize, number: u32, wparam: usize, lparam: usize) -> usize {
    let text = |lparam: usize| match lparam {
        0 => &[][..],
        // SAFETY: `LB_ADDSTRING`'s and `LB_INSERTSTRING`'s `lParam` is a NUL-terminated string.
        lparam => unsafe { super::file::wide_string(lparam as *const u16) },
    };
    let answer = with_state(window, |entry, state| {
        let State::ListBox(list) = state else {
            return (LB_ERR, false);
        };
        let visible = visible_rows(entry.height as i32);
        let sorted = entry.style & list_box::style::SORT != 0;
        let index = |value: Option<usize>| value.unwrap_or(LB_ERR);
        match number {
            code::LB_ADDSTRING => (index(list.add(text(lparam), sorted)), true),
            code::LB_INSERTSTRING => {
                let at = match wparam as isize {
                    -1 => Some(list.count()),
                    at if (0..=list.count() as isize).contains(&at) => Some(at as usize),
                    _ => None,
                };
                (index(at.and_then(|at| list.insert(at, text(lparam)))), true)
            }
            code::LB_DELETESTRING => (index(list.delete(wparam)), true),
            code::LB_RESETCONTENT => {
                list.clear();
                (0, true)
            }
            code::LB_SETCURSEL => {
                let wanted = match wparam as isize {
                    -1 => None,
                    at => Some(at as usize),
                };
                let valid = list.select(wanted, visible);
                (
                    if valid && wanted.is_some() {
                        wparam
                    } else {
                        LB_ERR
                    },
                    true,
                )
            }
            code::LB_GETCURSEL => (index(list.selected()), false),
            code::LB_GETTEXT => match list.text(wparam) {
                Some(item) => {
                    // SAFETY: `LB_GETTEXT`'s contract: `lParam` has room for the item and a NUL.
                    unsafe {
                        let out =
                            core::slice::from_raw_parts_mut(lparam as *mut u16, item.len() + 1);
                        out[..item.len()].copy_from_slice(item);
                        out[item.len()] = 0;
                    }
                    (item.len(), false)
                }
                None => (LB_ERR, false),
            },
            code::LB_GETTEXTLEN => (index(list.text(wparam).map(<[u16]>::len)), false),
            code::LB_GETCOUNT => (list.count(), false),
            code::LB_GETITEMDATA => (index(list.data(wparam)), false),
            code::LB_SETITEMDATA => match list.set_data(wparam, lparam) {
                true => (1, false),
                false => (LB_ERR, false),
            },
            code::LB_GETITEMHEIGHT => (ITEM_HEIGHT as usize, false),
            _ => (LB_ERR, false),
        }
    });
    let (result, changed) = answer.unwrap_or((LB_ERR, false));
    if changed {
        invalidate(window);
    }
    result
}

/// Fills list `window` with `entries` (the folder mark on those that are folders), replacing
/// what it held, as the file picker shows a folder.
pub fn fill_list(window: usize, entries: &mut dyn Iterator<Item = (&[u16], bool)>) {
    with_state(window, |entry, state| {
        if let State::ListBox(list) = state {
            list.clear();
            list.marks = true;
            for (name, folder) in entries {
                if let Some(index) = list.add(name, false)
                    && folder
                {
                    list.mark(index);
                }
            }
            entry.invalidate(None, true);
        }
    });
    gui::wake();
}

/// The text of `window`'s selected item and whether it is marked as a folder.
pub fn selected_item(window: usize, out: &mut [u16]) -> Option<(usize, bool)> {
    with_state(window, |_, state| {
        let State::ListBox(list) = state else {
            return None;
        };
        let index = list.selected()?;
        let text = list.text(index)?;
        let length = text.len().min(out.len());
        out[..length].copy_from_slice(&text[..length]);
        Some((length, list.is_marked(index)))
    })
    .flatten()
}

/// Gives a new edit `window` the text it was created with, without notifications.
pub fn start_edit_text(window: usize, text: &[u16]) {
    with_state(window, |_, state| {
        if let State::Edit(edit) = state {
            edit.set_text(text);
        }
    });
}

/// Puts the caret of edit `window` after its text.
pub fn caret_to_end(window: usize) {
    with_state(window, |entry, state| {
        if let State::Edit(edit) = state {
            edit.caret_to_end(edit_view(entry));
        }
    });
    invalidate(window);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_parents_colours_replace_only_the_fill_and_text_the_message_names() {
        let label = answered_palette(&LIGHT, (control_colour::STATIC, Some(0x0011_2233), 0xff));
        assert_eq!((label.surface, label.text), (0x0011_2233, 0xff));
        assert_eq!(label.field_read_only_fill, 0x0011_2233);
        assert_eq!(label.text_disabled, LIGHT.text_disabled);
        let edit = answered_palette(&LIGHT, (control_colour::EDIT, None, 0xff));
        assert_eq!((edit.field_fill, edit.text), (LIGHT.field_fill, 0xff));
        assert_eq!(edit.field_border, LIGHT.field_border);
        let button = answered_palette(&LIGHT, (control_colour::BUTTON, Some(0x0011_2233), 0xff));
        assert_eq!((button.button_fill, button.text), (LIGHT.button_fill, 0xff));
        assert_eq!(button.default_text, 0xff);
        assert!(control_colour::is(0x0136) && !control_colour::is(0x0139));
    }

    #[test]
    fn a_password_edit_conceals_its_text_until_its_password_character_is_cleared() {
        let mut edit = Edit::with_style(edit::style::PASSWORD);
        assert!(conceals(&edit));
        edit.password = 0;
        assert!(!conceals(&edit));
        assert!(!conceals(&Edit::with_style(0)));
    }
}
