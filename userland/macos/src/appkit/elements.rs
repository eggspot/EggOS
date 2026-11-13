//! An AppKit app's windows as element trees for agents (D161, D162, D175; `wm show`, `wm act`):
//! the window server's `Query` record reaches the event loop, which answers it. A window is the
//! root element, named by its title; its views follow in pre-order from the content view, each
//! numbered from 2 in that order (hidden views and their subviews left out): a push button as a
//! button named by its title (the default button flagged; a toggle valued `on` or `off` and
//! checked when on), a checkbox as a checkbox (checked when on, valued `mixed` when mixed), a
//! radio as a radio (selected when on), an editable text field as a text
//! field whose value is its text, a label as a label, a plain `NSView` as a group and any view
//! of the app's own class as a graphic named by its class, focusable when it accepts the first
//! responder. Each has its frame in the window's content pixels (a control its drawn 28 pixel
//! rectangle). `invoke` clicks a button as `performClick:` does (`select` too for a radio),
//! `focus` makes a focusable view
//! the first responder, `set-text` replaces a text field's text; disabled controls refuse.
//! While an alert runs, trees are answered and actions refused, as the app is modal.

use super::{
    NSBUTTON, NSCLIPVIEW, NSIMAGEVIEW, NSSCROLLVIEW, NSTABLEVIEW, NSTEXTFIELD, NSVIEW, about,
    application, control, image, is, scroll_view, server, table_view, view, window, with_text,
};
use crate::libsystem::heap;
use crate::objc::abi::Id;
use crate::objc::{class, root, sel, send1};
use eggos_element_tree::{
    Action, Bounds, Element, MAX_LABEL_BYTES, MAX_TEXT_BYTES, MAX_TREE_BYTES, Outcome, Query, Role,
    TreeWriter, flags, query_kind, tree_answer,
};
use eggos_macos_gui::button::{self, Family};
use eggos_macos_gui::geometry::{Frame, pixel_rect};
use eggos_macos_gui::paint;
use eggos_toolkit::geometry::Rect;

/// The window's element.
pub const ROOT_ELEMENT: u32 = 1;
/// The first view's element; the others follow in pre-order.
pub const FIRST_VIEW_ELEMENT: u32 = 2;
const MAX_DEPTH: usize = 32;

/// Answers `query` for the app's window (or the About panel) it names.
pub fn answer(query: &Query) {
    answer_with(query, true);
}

/// Answers `query` while an alert runs: trees as usual, actions refused.
pub fn answer_while_modal(query: &Query) {
    answer_with(query, false);
}

fn answer_with(query: &Query, may_act: bool) {
    if about::is_panel(query.window) {
        about::answer(query);
        return;
    }
    let window = application::window_with_server_id(query.window);
    if query.kind == query_kind::TREE {
        match window {
            Some(window) => answer_tree(query.serial, window),
            None => server::answer_query(query.serial, tree_answer::PIXELS_ONLY, &[]),
        }
        return;
    }
    let outcome = match (window, query.action()) {
        (None, _) => Outcome::NoSuchElement,
        (Some(_), None) => Outcome::NotSupported,
        (Some(_), Some(_)) if !may_act => Outcome::Disabled,
        (Some(window), Some(action)) => act(query.serial, window, query.element, action),
    };
    server::answer_query(query.serial, outcome.value(), &[]);
}

fn answer_tree(serial: u32, window: Id) {
    let block = heap::allocate(MAX_TREE_BYTES, false);
    if block.is_null() {
        server::answer_query(serial, tree_answer::PIXELS_ONLY, &[]);
        return;
    }
    // SAFETY: a new block of `MAX_TREE_BYTES` bytes, this function's until it is freed below.
    let bytes = unsafe { core::slice::from_raw_parts_mut(block, MAX_TREE_BYTES) };
    let length = {
        let mut tree = TreeWriter::new(&mut *bytes);
        describe(window, &mut tree);
        tree.bytes().len()
    };
    server::answer_query(serial, tree_answer::TREE, &bytes[..length]);
    heap::release(block);
}

fn copied<'a>(text: &str, out: &'a mut [u8; MAX_LABEL_BYTES]) -> &'a [u8] {
    let mut length = text.len().min(MAX_LABEL_BYTES);
    while !text.is_char_boundary(length) {
        length -= 1;
    }
    out[..length].copy_from_slice(&text.as_bytes()[..length]);
    &out[..length]
}

fn describe(window: Id, tree: &mut TreeWriter<'_>) {
    let (width, height, title_object, content) = {
        let fields = window::fields(window);
        (
            fields.buffer.width as i32,
            fields.buffer.height as i32,
            fields.title,
            fields.content_view,
        )
    };
    let mut title = [0u8; MAX_LABEL_BYTES];
    let length = super::with_text(title_object, |text| {
        let text = if text.is_empty() {
            super::app_name()
        } else {
            text
        };
        copied(text, &mut title).len()
    });
    let root = Element::new(
        ROOT_ELEMENT,
        0,
        Role::Window,
        Bounds::new(0, 0, width, height),
    )
    .named(&title[..length]);
    if !tree.push(&root) || content.is_null() {
        return;
    }
    push_view(window, content, tree);
}

struct Described {
    role: Role,
    flags: u16,
    name: [u8; MAX_LABEL_BYTES],
    name_length: usize,
    value: [u8; MAX_LABEL_BYTES],
    value_length: usize,
}

impl Described {
    fn new(role: Role) -> Self {
        Self {
            role,
            flags: 0,
            name: [0; MAX_LABEL_BYTES],
            name_length: 0,
            value: [0; MAX_LABEL_BYTES],
            value_length: 0,
        }
    }

    fn named(mut self, text: &str) -> Self {
        self.name_length = copied(text, &mut self.name).len();
        self
    }

    fn valued(mut self, text: &str) -> Self {
        self.value_length = copied(text, &mut self.value).len();
        self
    }
}

fn is_focused(window: Id, view: Id) -> bool {
    window::is_key(window) && window::first_responder(window) == view
}

fn classify(window: Id, view: Id) -> Described {
    if is(view, &NSBUTTON) {
        let family = control::family_of(view);
        let on = control::state_of(view) == button::ON;
        let role = match family {
            Family::Checkbox => Role::Checkbox,
            Family::Radio => Role::Radio,
            Family::Push | Family::Toggle => Role::Button,
        };
        let mut described = control::with_title(view, |title| Described::new(role).named(title));
        match family {
            Family::Checkbox if control::state_of(view) == button::MIXED => {
                described = described.valued("mixed");
            }
            Family::Toggle => described = described.valued(if on { "on" } else { "off" }),
            Family::Checkbox | Family::Radio | Family::Push => {}
        }
        if on && family == Family::Radio {
            described.flags |= flags::SELECTED;
        } else if on && matches!(family, Family::Checkbox | Family::Toggle) {
            described.flags |= flags::CHECKED;
        }
        if control::is_default_button(view) {
            described.flags |= flags::DEFAULT;
        }
        if !control::is_enabled_control(view) {
            described.flags |= flags::DISABLED;
        }
        return described;
    }
    if is(view, &NSTEXTFIELD) {
        let editable = control::is_editable(view);
        let mut described = control::with_field_text(view, |text| match editable {
            true => Described::new(Role::TextField).valued(text),
            false => Described::new(Role::Label).named(text),
        });
        if editable {
            described.flags |= flags::FOCUSABLE;
            if is_focused(window, view) {
                described.flags |= flags::FOCUSED;
            }
            if !control::is_enabled_control(view) {
                described.flags |= flags::DISABLED;
            }
        }
        return described;
    }
    if is(view, &NSSCROLLVIEW) {
        let clip = scroll_view::clip_of_scroll_view(view);
        let ((x, y), _) = scroll_view::clip_offset(clip).unwrap_or(((0, 0), (0, 0)));
        let mut value = [0u8; 24];
        let value = pair_text(x, y, &mut value);
        return Described::new(Role::Group).valued(value);
    }
    if is(view, &NSTABLEVIEW) {
        let (selected, rows, disabled) = table_view::summary(view);
        let mut value = [0u8; 48];
        let value = table_value(selected, rows, &mut value);
        let mut described = Described::new(Role::Table).valued(value);
        described.flags |= flags::FOCUSABLE;
        if is_focused(window, view) {
            described.flags |= flags::FOCUSED;
        }
        if disabled {
            described.flags |= flags::DISABLED;
        }
        return described;
    }
    if is(view, &NSIMAGEVIEW) {
        let shown = image::image_of_view(view);
        let mut value = [0u8; 24];
        let value = match image::size_of(shown) {
            Some((width, height)) => size_text(width, height, &mut value),
            None => "none",
        };
        return image::with_image_name(shown, |name| {
            Described::new(Role::Graphic).named(name).valued(value)
        });
    }
    if class::class_of(view) == (&raw const NSVIEW).cast_mut() {
        return Described::new(Role::Group);
    }
    let name = core::str::from_utf8(root::class_name(view)).unwrap_or("");
    let mut described = Described::new(Role::Graphic).named(name);
    if view::accepts_first_responder(view) {
        described.flags |= flags::FOCUSABLE;
        if is_focused(window, view) {
            described.flags |= flags::FOCUSED;
        }
    }
    described
}

fn write_number(out: &mut [u8], at: &mut usize, number: i64) {
    if number < 0 {
        out[*at] = b'-';
        *at += 1;
    }
    let mut digits = [0u8; 20];
    let mut count = 0;
    let mut rest = number.unsigned_abs();
    loop {
        digits[count] = b'0' + (rest % 10) as u8;
        count += 1;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    for &digit in digits[..count].iter().rev() {
        out[*at] = digit;
        *at += 1;
    }
}

fn write_text(out: &mut [u8], at: &mut usize, text: &str) {
    out[*at..*at + text.len()].copy_from_slice(text.as_bytes());
    *at += text.len();
}

fn pair_text(x: i32, y: i32, out: &mut [u8; 24]) -> &str {
    let mut at = 0;
    write_number(out, &mut at, i64::from(x));
    write_text(out, &mut at, ",");
    write_number(out, &mut at, i64::from(y));
    core::str::from_utf8(&out[..at]).unwrap_or("")
}

fn table_value(selected: isize, rows: usize, out: &mut [u8; 48]) -> &str {
    let mut at = 0;
    if rows == 0 {
        write_text(out, &mut at, "0 rows");
    } else if selected < 0 {
        write_text(out, &mut at, "no row of ");
        write_number(out, &mut at, rows as i64);
    } else {
        write_text(out, &mut at, "row ");
        write_number(out, &mut at, selected as i64 + 1);
        write_text(out, &mut at, " of ");
        write_number(out, &mut at, rows as i64);
    }
    core::str::from_utf8(&out[..at]).unwrap_or("")
}

fn size_text(width: u32, height: u32, out: &mut [u8; 24]) -> &str {
    let mut at = 0;
    for (index, number) in [width, height].into_iter().enumerate() {
        if index == 1 {
            out[at] = b'x';
            at += 1;
        }
        let mut digits = [0u8; 10];
        let mut count = 0;
        let mut rest = number;
        loop {
            digits[count] = b'0' + (rest % 10) as u8;
            count += 1;
            rest /= 10;
            if rest == 0 {
                break;
            }
        }
        for &digit in digits[..count].iter().rev() {
            out[at] = digit;
            at += 1;
        }
    }
    core::str::from_utf8(&out[..at]).unwrap_or("")
}

fn bounds_of(view: Id, height: i32) -> Rect {
    let origin = view::origin_in_window(view);
    let frame = view::fields(view).frame;
    let in_window = Frame::new(origin.x, origin.y, frame.width, frame.height);
    match is(view, &NSBUTTON) || (is(view, &NSTEXTFIELD) && control::is_editable(view)) {
        true => paint::control_rect(in_window, height),
        false => pixel_rect(in_window, height),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    View(Id),
    Column(Id, usize),
    Row(Id, usize),
}

fn clipped(rect: Rect, clip: Option<Rect>) -> Rect {
    let Some(clip) = clip else {
        return rect;
    };
    let inside = rect.intersection(&clip);
    if !inside.is_empty() {
        return inside;
    }
    let edge = match rect.bottom() <= clip.y {
        true => clip.y,
        false => clip.bottom(),
    };
    Rect::new(rect.x, edge, rect.width, 0)
}

type Visit<'v> = dyn FnMut(u32, u32, Target, Rect) -> bool + 'v;

fn walk_table(
    table: Id,
    id: u32,
    next: &mut u32,
    clip: Option<Rect>,
    visit: &mut Visit<'_>,
) -> bool {
    let (x, y) = table_view::origin_pixel(table);
    let in_window = |rect: Rect| clipped(rect.offset(x, y), clip);
    let mut going = table_view::header_cells(table, &mut |shown, _, rect| {
        let column = *next;
        *next += 1;
        visit(column, id, Target::Column(table, shown), in_window(rect))
    });
    let (first, last) = table_view::shown_rows(table);
    for row in first..last {
        if !going {
            break;
        }
        let element = *next;
        *next += 1;
        going = visit(
            element,
            id,
            Target::Row(table, row),
            in_window(table_view::row_rect(table, row)),
        );
    }
    going
}

fn walk(
    window: Id,
    view: Id,
    (parent, depth): (u32, usize),
    next: &mut u32,
    clip: Option<Rect>,
    visit: &mut Visit<'_>,
) -> bool {
    if depth > MAX_DEPTH || view::fields(view).hidden {
        return true;
    }
    let height = window::fields(window).buffer.height as i32;
    let rect = clipped(bounds_of(view, height), clip);
    let (id, inner_clip) = match is(view, &NSCLIPVIEW) {
        true => (parent, Some(rect)),
        false => {
            let id = *next;
            *next += 1;
            if !visit(id, parent, Target::View(view), rect) {
                return false;
            }
            (id, clip)
        }
    };
    if is(view, &NSTABLEVIEW) {
        return walk_table(view, id, next, inner_clip, visit);
    }
    let mut index = 0;
    while let Some(&child) = view::fields(view).subviews.as_slice().get(index) {
        if !walk(window, child, (id, depth + 1), next, inner_clip, visit) {
            return false;
        }
        index += 1;
    }
    true
}

fn push_view(window: Id, content: Id, tree: &mut TreeWriter<'_>) {
    let mut next = FIRST_VIEW_ELEMENT;
    let mut joined = [0u8; MAX_LABEL_BYTES];
    let _ = walk(
        window,
        content,
        (ROOT_ELEMENT, 0),
        &mut next,
        None,
        &mut |id, parent, target, rect| {
            let described = match target {
                Target::View(view) => classify(window, view),
                Target::Column(_, _) => Described::new(Role::ColumnHeader),
                Target::Row(table, row) => {
                    table_view::with_row_text(table, row, &mut joined, |name, value| {
                        let mut described = Described::new(Role::Row).named(name).valued(value);
                        if table_view::summary(table).0 == row as isize {
                            described.flags |= flags::SELECTED;
                        }
                        described
                    })
                }
            };
            let described = match target {
                Target::Column(table, shown) => {
                    let mut title = Described::new(Role::ColumnHeader);
                    let _ = table_view::header_cells(table, &mut |index, column, _| {
                        if index == shown {
                            title = with_text(table_view::column_title(column), |text| {
                                Described::new(Role::ColumnHeader).named(text)
                            });
                        }
                        true
                    });
                    title
                }
                _ => described,
            };
            let element = Element::new(
                id,
                parent,
                described.role,
                Bounds::new(rect.x, rect.y, rect.width, rect.height),
            )
            .named(&described.name[..described.name_length])
            .valued(&described.value[..described.value_length])
            .flagged(described.flags);
            tree.push(&element)
        },
    );
}

fn target_with_id(window: Id, element: u32) -> Option<Target> {
    let content = window::fields(window).content_view;
    if content.is_null() || element < FIRST_VIEW_ELEMENT {
        return None;
    }
    let mut next = FIRST_VIEW_ELEMENT;
    let mut found = None;
    let _ = walk(
        window,
        content,
        (ROOT_ELEMENT, 0),
        &mut next,
        None,
        &mut |id, _, target, _| {
            if id == element {
                found = Some(target);
                return false;
            }
            true
        },
    );
    found
}

fn query_number(serial: u32) -> Option<(i64, Option<i64>)> {
    let mut text = [0u8; MAX_TEXT_BYTES];
    let length = server::query_text(serial, &mut text).ok()?;
    let text = core::str::from_utf8(&text[..length]).ok()?.trim();
    let parse = |part: &str| part.trim().parse::<i64>().ok();
    match text.split_once(',') {
        Some((first, second)) => Some((parse(first)?, Some(parse(second)?))),
        None => Some((parse(text)?, None)),
    }
}

fn act_on_extra(serial: u32, window: Id, target: Target, action: Action) -> Outcome {
    match (target, action) {
        (Target::Row(table, row), Action::Select | Action::Invoke) => {
            match table_view::agent_select(table, row, action == Action::Invoke) {
                true => Outcome::Done,
                false => Outcome::Disabled,
            }
        }
        (Target::Column(table, shown), Action::SetText) => match query_number(serial) {
            Some((width, None))
                if table_view::agent_resize(
                    table,
                    shown,
                    width.clamp(0, i64::from(i32::MAX)) as i32,
                ) =>
            {
                Outcome::Done
            }
            _ => Outcome::NotSupported,
        },
        (Target::View(view), Action::SetText) if is(view, &NSSCROLLVIEW) => {
            match query_number(serial) {
                Some((x, Some(y))) => {
                    let clip = scroll_view::clip_of_scroll_view(view);
                    let clamp = |value: i64| value.clamp(0, i64::from(i32::MAX)) as i32;
                    scroll_view::scroll_clip(clip, (clamp(x), clamp(y)));
                    Outcome::Done
                }
                _ => Outcome::NotSupported,
            }
        }
        (Target::View(view), Action::SetText) if is(view, &NSTABLEVIEW) => {
            let (_, rows, disabled) = table_view::summary(view);
            match query_number(serial) {
                _ if disabled => Outcome::Disabled,
                Some((row, None)) if row >= 1 && row as usize <= rows => {
                    let _ = table_view::agent_select(view, row as usize - 1, false);
                    Outcome::Done
                }
                Some((_, None)) => Outcome::Done,
                _ => Outcome::NotSupported,
            }
        }
        (Target::View(view), Action::Focus) if is(view, &NSTABLEVIEW) => {
            if table_view::summary(view).2 {
                return Outcome::Disabled;
            }
            window::make_first_responder(window, view);
            Outcome::Done
        }
        _ => Outcome::NotSupported,
    }
}

fn act(serial: u32, window: Id, element: u32, action: Action) -> Outcome {
    if element == ROOT_ELEMENT {
        return Outcome::NotSupported;
    }
    let target = match target_with_id(window, element) {
        Some(Target::View(view)) if !is(view, &NSSCROLLVIEW) && !is(view, &NSTABLEVIEW) => view,
        Some(target) => return act_on_extra(serial, window, target, action),
        None => return Outcome::NoSuchElement,
    };
    let view = target;
    let is_button = is(view, &NSBUTTON);
    let is_field = is(view, &NSTEXTFIELD) && control::is_editable(view);
    let enabled = !(is_button || is_field) || control::is_enabled_control(view);
    match action {
        Action::Invoke if is_button => {
            if !enabled {
                return Outcome::Disabled;
            }
            control::perform_click(view);
            Outcome::Done
        }
        Action::Select if is_button && control::family_of(view) == Family::Radio => {
            if !enabled {
                return Outcome::Disabled;
            }
            control::perform_click(view);
            Outcome::Done
        }
        Action::Focus if is_field || (!is_button && view::accepts_first_responder(view)) => {
            if !enabled {
                return Outcome::Disabled;
            }
            window::make_first_responder(window, view);
            Outcome::Done
        }
        Action::SetText if is_field => {
            if !enabled {
                return Outcome::Disabled;
            }
            let mut text = [0u8; MAX_TEXT_BYTES];
            let Ok(length) = server::query_text(serial, &mut text) else {
                return Outcome::NotSupported;
            };
            let Ok(text) = core::str::from_utf8(&text[..length]) else {
                return Outcome::NotSupported;
            };
            send1(view, sel!("setStringValue:"), super::string(text) as usize);
            Outcome::Done
        }
        _ => Outcome::NotSupported,
    }
}
