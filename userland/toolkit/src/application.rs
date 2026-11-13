//! The event loop of a window built with the toolkit: draw what changed, commit it, wait for
//! input or the next deadline, hand the input to the widgets and the program, again until the
//! user closes the window. Nothing runs while nothing happens: the loop sleeps in `ppoll`.

use crate::canvas::Canvas;
use crate::damage::Damage;
use crate::input::{self, Input, RECORD_SIZE};
use crate::menu::POOL_CAPACITY;
use crate::theme;
use crate::tree::{COPIED_BYTES, PointerShape, ROOT_ELEMENT, Response, WidgetTree};
use crate::widget::Kind;
use crate::window::{Window, WindowServer, now_ms};
use eggos_element_tree::{
    Action, Bounds, Element, MAX_TEXT_BYTES, MAX_TREE_BYTES, Outcome, Query, Role, TreeWriter,
    query_kind, tree_answer,
};
use eggos_runtime::errno::Errno;

const RECORDS_READ: usize = 16;
const MENU_CHOSEN: u16 = 14;
const PALETTE_CHANGED: u16 = 16;
const MACOS_PROFILE: u32 = 1;

fn record_kind(record: &[u8; RECORD_SIZE]) -> (u16, u16) {
    (
        u16::from_ne_bytes([record[0], record[1]]),
        u16::from_ne_bytes([record[2], record[3]]),
    )
}

fn record_word(record: &[u8; RECORD_SIZE], offset: usize) -> u32 {
    u32::from_ne_bytes([
        record[offset],
        record[offset + 1],
        record[offset + 2],
        record[offset + 3],
    ])
}

/// A program's window: its widgets (with `A` text stores) and what it does with their
/// responses.
pub trait Application<const N: usize, const A: usize = 0> {
    /// The window's widgets.
    fn tree(&mut self) -> &mut WidgetTree<N, A>;

    /// Acts on what an input did to the widgets.
    fn respond(&mut self, response: Response, now_ms: u32);

    /// When the program next wants [`Self::tick`], if ever.
    fn deadline(&self, _now_ms: u32) -> Option<u32> {
        None
    }

    /// Called at or after the time [`Self::deadline`] gave, once the window shows what the
    /// last input changed.
    fn tick(&mut self, _now_ms: u32) {}

    /// Called when the window gets the keyboard focus, after its widgets saw it: the moment to
    /// pick up what changed outside the program meanwhile.
    fn focused(&mut self, _now_ms: u32) {}

    /// `Back` reached the window and no widget used it as Esc: whether the program used it
    /// (closed a pane of its own); if not, the shell goes back past the window (D305).
    fn back(&mut self, _now_ms: u32) -> bool {
        false
    }

    /// The window's content became `width` by `height`, after its widgets were laid out
    /// again: the moment to change the window's shape for its new size.
    fn resized(&mut self, _width: i32, _height: i32) {}

    /// Whether the window shows something for the person's eyes only now, such as a pairing
    /// code: while it does, no agent may capture its pixels (`PERSON_VIEW_ONLY`, D193).
    fn person_view_only(&self) -> bool {
        false
    }
}

fn draw(
    window: &mut Window,
    view: &mut impl View,
    everything: bool,
    palette: u32,
) -> Result<(), Errno> {
    let damage = view.take_damage();
    if damage.is_empty() && !everything {
        return Ok(());
    }
    let (width, height) = window.size();
    let scale = window.scale();
    {
        let Some(mut canvas) = Canvas::new(window.pixels(), width, height, scale) else {
            return Err(Errno::EINVAL);
        };
        canvas.set_palette(theme::palette_at(palette));
        for rect in damage.rects() {
            let previous = canvas.narrow(*rect);
            view.paint(&mut canvas);
            canvas.restore(previous);
        }
    }
    window.commit(&damage, everything)
}

fn answer(window: &Window, view: &mut impl View, query: &Query) {
    let server = window.server();
    if query.kind == query_kind::TREE {
        let mut bytes = [0u8; MAX_TREE_BYTES];
        let mut tree = TreeWriter::new(&mut bytes);
        let scale = window.scale().max(1);
        let (width, height) = window.size();
        let content = Bounds::new(0, 0, (width / scale) as i32, (height / scale) as i32);
        tree.push(&Element::new(ROOT_ELEMENT, 0, Role::Window, content));
        let (outcome, tree) = match view.describe(&mut tree) {
            true => (tree_answer::TREE, tree.bytes()),
            false => (tree_answer::PIXELS_ONLY, &[][..]),
        };
        let _ = server.answer_query(query.serial, outcome, tree);
        return;
    }
    let outcome = match query.action() {
        Some(action) => {
            let mut text = [0u8; MAX_TEXT_BYTES];
            let length = match action {
                Action::SetText => server.query_text(query.serial, &mut text).unwrap_or(0),
                _ => 0,
            };
            let text = core::str::from_utf8(&text[..length]).unwrap_or("");
            view.act(query.element, action, text, now_ms())
        }
        None => Outcome::NotSupported,
    };
    let _ = server.answer_query(query.serial, outcome.value(), &[]);
}

fn soonest(first: Option<u32>, second: Option<u32>, now: u32) -> Option<u32> {
    let wait = |deadline: u32| deadline.wrapping_sub(now) as i32;
    match (first, second) {
        (Some(a), Some(b)) => Some(if wait(a) <= wait(b) { a } else { b }),
        (deadline, None) | (None, deadline) => deadline,
    }
}

fn in_scale_1(input: Input, scale: i32) -> Input {
    match input {
        Input::PointerMove { x, y } => Input::PointerMove {
            x: x.div_euclid(scale),
            y: y.div_euclid(scale),
        },
        Input::Button {
            x,
            y,
            button,
            pressed,
        } => Input::Button {
            x: x.div_euclid(scale),
            y: y.div_euclid(scale),
            button,
            pressed,
        },
        Input::Wheel { x, y, notches } => Input::Wheel {
            x: x.div_euclid(scale),
            y: y.div_euclid(scale),
            notches,
        },
        Input::SidewaysWheel { x, y, notches } => Input::SidewaysWheel {
            x: x.div_euclid(scale),
            y: y.div_euclid(scale),
            notches,
        },
        other => other,
    }
}

/// A window's content a program draws and answers itself, with the toolkit's pieces
/// ([`crate::canvas`], [`crate::widget`], [`crate::menu`], [`crate::panel`]) rather than a
/// widget tree; [`run_view`] serves it.
pub trait View {
    /// The content is now `width` by `height` (scale 1).
    fn resize(&mut self, width: i32, height: i32);

    /// Acts on `input`, which happened at `time_ms`; `false` once the window should close.
    fn handle(&mut self, input: Input, time_ms: u32) -> bool;

    /// What changed since the last call.
    fn take_damage(&mut self) -> Damage;

    /// Draws the whole content; the canvas is clipped to what is redrawn.
    fn paint(&mut self, canvas: &mut Canvas<'_>);

    /// The pointer shape over the content now.
    fn pointer_shape(&mut self) -> PointerShape {
        PointerShape::Arrow
    }

    /// When the view next wants [`Self::tick`], if ever.
    fn deadline(&mut self, _now_ms: u32) -> Option<u32> {
        None
    }

    /// Called whenever the loop wakes, with the time.
    fn tick(&mut self, _now_ms: u32) {}

    /// The user asked to close the window; `true` closes it now, `false` keeps it open (the
    /// view asks first, and [`Self::handle`] says when to close).
    fn close_requested(&mut self, _time_ms: u32) -> bool {
        true
    }

    /// `Back` (D305): whether the view used it, as Esc first; `false` lets the shell go back.
    fn back(&mut self, _time_ms: u32) -> bool {
        false
    }

    /// What has the text focus, as the window tells the shell for the on-screen keyboard
    /// (`SetTextFocus`, D314): the field's kind (0 none, 1 text, 2 multiline, 3 masked,
    /// 8 terminal) and its Enter word (0 `Enter`).
    fn text_focus(&mut self) -> (u32, u32) {
        (0, 0)
    }

    /// The window's new title, once after it changes.
    fn take_title(&mut self) -> Option<&str> {
        None
    }

    /// Writes the content's elements below [`ROOT_ELEMENT`] for an agent (D161, `wm show`);
    /// `false` when the view describes none, so the agent sees pixels only.
    fn describe(&mut self, _tree: &mut TreeWriter<'_>) -> bool {
        false
    }

    /// Does `action` to element `element` for an agent (D161, `wm act`), with `text` for
    /// [`Action::SetText`], at `time_ms`.
    fn act(&mut self, _element: u32, _action: Action, _text: &str, _time_ms: u32) -> Outcome {
        Outcome::NoSuchElement
    }

    /// A descriptor the loop also waits on, such as a terminal's pseudo-terminal (D187).
    fn watched_descriptor(&self) -> Option<usize> {
        None
    }

    /// The watched descriptor has something to read or hung up, at `time_ms`; `false` once the
    /// window should close.
    fn descriptor_ready(&mut self, _time_ms: u32) -> bool {
        true
    }

    /// The desktop's palette is now index `index` ([`theme::palette_at`]); the whole window is
    /// drawn again after it.
    fn palette_changed(&mut self, _index: u32) {}

    /// Whether nothing blinks now (`accessibility.reduce_motion`, D224): a caret or a cursor
    /// is drawn steady while it is.
    fn motion_changed(&mut self, _reduce: bool) {}

    /// Whether the menu bar goes to the top bar (the macOS profile) rather than the window.
    fn set_global_menu(&mut self, _global: bool) {}

    /// The view's main menu as `SET_MENU`'s pool, written into `pool`: its length, or `None`
    /// for none.
    fn main_menu(&mut self, _pool: &mut [u8; POOL_CAPACITY]) -> Option<usize> {
        None
    }

    /// The top bar's item `entry` of menu `menu` was chosen (`MenuChosen`).
    fn menu_chosen(&mut self, _menu: usize, _entry: usize, _time_ms: u32) {}

    /// Whether the content shows something for the person's eyes only now: the loop keeps
    /// the window's pixels from agents from before it is drawn until after it is gone
    /// (`PERSON_VIEW_ONLY`, D193).
    fn person_view_only(&mut self) -> bool {
        false
    }
}

struct Tree<'a, P, const N: usize, const A: usize>(&'a mut P, Option<WindowServer>);

/// Answers a text field's clipboard response through the clipboard every app shares (D197,
/// docs/ux/toolkit-v0.4.md section 4.4): a copy or a cut gives [`WidgetTree::copied`] to the
/// server, a paste fetches its text into the field. What the program then sees: `Nothing` for
/// a copy, `Edited` for a cut, [`WidgetTree::paste`]'s answer for a paste; any other response
/// as it was.
pub fn answer_clipboard<const N: usize, const A: usize>(
    tree: &mut WidgetTree<N, A>,
    server: Option<&WindowServer>,
    response: Response,
    now_ms: u32,
) -> Response {
    match response {
        Response::Copied(_) | Response::Cut(_) => {
            if let Some(server) = server {
                let _ = server.set_clipboard(tree.copied());
            }
            match response {
                Response::Cut(id) => Response::Edited(id),
                _ => Response::Nothing,
            }
        }
        Response::PasteWanted(id) => {
            let mut bytes = [0u8; COPIED_BYTES];
            let length = server
                .and_then(|server| server.clipboard(0, &mut bytes).ok())
                .map_or(0, |(length, _)| length.min(bytes.len()));
            let text = match core::str::from_utf8(&bytes[..length]) {
                Ok(text) => text,
                Err(error) => core::str::from_utf8(&bytes[..error.valid_up_to()]).unwrap_or(""),
            };
            tree.paste(id, text, now_ms)
        }
        other => other,
    }
}

impl<P: Application<N, A>, const N: usize, const A: usize> View for Tree<'_, P, N, A> {
    fn resize(&mut self, width: i32, height: i32) {
        self.0.tree().resize(width, height);
        self.0.resized(width, height);
    }

    fn handle(&mut self, input: Input, time_ms: u32) -> bool {
        let response = self.0.tree().handle(input, time_ms);
        let response = answer_clipboard(self.0.tree(), self.1.as_ref(), response, time_ms);
        self.0.respond(response, time_ms);
        if input == Input::FocusIn {
            self.0.focused(time_ms);
        }
        true
    }

    fn text_focus(&mut self) -> (u32, u32) {
        let tree = self.0.tree();
        let kind = match tree.focus().and_then(|id| tree.kind(id)) {
            Some(Kind::TextField(field)) if field.masked => TEXT_FOCUS_MASKED,
            Some(Kind::TextField(_)) => TEXT_FOCUS_TEXT,
            Some(Kind::TextArea(_)) => TEXT_FOCUS_MULTILINE,
            _ => TEXT_FOCUS_NONE,
        };
        (kind, 0)
    }

    fn back(&mut self, time_ms: u32) -> bool {
        let escape = Input::Key {
            code: crate::input::key::ESC,
            pressed: true,
            modifiers: crate::input::Modifiers(0),
            character: 0,
        };
        let response = self.0.tree().handle(escape, time_ms);
        match response {
            Response::Unused {
                code: crate::input::key::ESC,
                ..
            }
            | Response::Nothing => self.0.back(time_ms),
            other => {
                self.0.respond(other, time_ms);
                true
            }
        }
    }

    fn take_damage(&mut self) -> Damage {
        self.0.tree().take_damage()
    }

    fn paint(&mut self, canvas: &mut Canvas<'_>) {
        self.0.tree().paint(canvas);
    }

    fn pointer_shape(&mut self) -> PointerShape {
        self.0.tree().pointer_shape()
    }

    fn describe(&mut self, tree: &mut TreeWriter<'_>) -> bool {
        self.0.tree().describe(tree);
        true
    }

    fn act(&mut self, element: u32, action: Action, text: &str, time_ms: u32) -> Outcome {
        let (outcome, response) = self.0.tree().act(element, action, text, time_ms);
        let response = answer_clipboard(self.0.tree(), self.1.as_ref(), response, time_ms);
        self.0.respond(response, time_ms);
        outcome
    }

    fn deadline(&mut self, now_ms: u32) -> Option<u32> {
        let tree = self.0.tree().next_deadline(now_ms);
        soonest(tree, self.0.deadline(now_ms), now_ms)
    }

    fn person_view_only(&mut self) -> bool {
        self.0.person_view_only()
    }

    fn palette_changed(&mut self, index: u32) {
        self.0.tree().set_palette(theme::palette_at(index));
    }

    fn motion_changed(&mut self, reduce: bool) {
        self.0.tree().set_reduce_motion(reduce);
    }

    fn set_global_menu(&mut self, global: bool) {
        self.0.tree().set_global_menu(global);
    }

    fn main_menu(&mut self, pool: &mut [u8; POOL_CAPACITY]) -> Option<usize> {
        self.0.tree().main_menu(pool)
    }

    fn menu_chosen(&mut self, menu: usize, entry: usize, time_ms: u32) {
        let response = self.0.tree().menu_chosen(menu, entry);
        if response != Response::Nothing {
            self.0.respond(response, time_ms);
        }
    }

    fn tick(&mut self, now_ms: u32) {
        let response = self.0.tree().tick(now_ms);
        if response != Response::Nothing {
            self.0.respond(response, now_ms);
        }
        if self
            .0
            .deadline(now_ms)
            .is_some_and(|deadline| deadline.wrapping_sub(now_ms) as i32 <= 0)
        {
            self.0.tick(now_ms);
        }
    }
}

/// Shows `window` with `application`'s widgets laid out in it and serves it until the user
/// asks to close it.
pub fn run<const N: usize, const A: usize>(
    window: &mut Window,
    application: &mut impl Application<N, A>,
) -> Result<(), Errno> {
    run_view(window, &mut Tree(application, WindowServer::open().ok()))
}

fn sync_main_menu(
    window: &Window,
    view: &mut impl View,
    global: bool,
    sent: &mut Option<([u8; POOL_CAPACITY], usize)>,
) {
    let mut pool = [0u8; POOL_CAPACITY];
    let length = match global {
        true => view.main_menu(&mut pool),
        false => None,
    };
    let unchanged = match (*sent, length) {
        (None, None) => true,
        (Some((held, held_length)), Some(length)) => held[..held_length] == pool[..length],
        _ => false,
    };
    if unchanged {
        return;
    }
    let length = length.unwrap_or(0);
    if window
        .server()
        .set_menu(window.id(), &pool[..length])
        .is_ok()
    {
        *sent = (length > 0).then_some((pool, length));
    }
}

/// Shows `window` with `view` drawn in it and serves it until the user asks to close it or
/// [`View::handle`] says so.
pub fn run_view(window: &mut Window, view: &mut impl View) -> Result<(), Errno> {
    let scale = window.scale().max(1);
    let (width, height) = window.size();
    view.resize((width / scale) as i32, (height / scale) as i32);
    let mut everything = true;
    let mut records = [0u8; RECORD_SIZE * RECORDS_READ];
    let mut view_only = false;
    let mut palette = window.server().palette().unwrap_or(0);
    view.palette_changed(palette);
    view.motion_changed(window.server().reduce_motion().unwrap_or(false));
    let mut global = window.server().desktop_profile() == Ok(MACOS_PROFILE);
    view.set_global_menu(global);
    let mut sent_menu: Option<([u8; POOL_CAPACITY], usize)> = None;
    let mut sent_text_focus = (TEXT_FOCUS_NONE, 0);
    loop {
        sync_main_menu(window, view, global, &mut sent_menu);
        if let Some(title) = view.take_title() {
            window.set_title(title)?;
        }
        let wanted = view.person_view_only();
        if wanted && !view_only {
            window.set_person_view_only(true)?;
            view_only = true;
        }
        draw(window, view, everything, palette)?;
        if view_only && !wanted && window.set_person_view_only(false).is_ok() {
            view_only = false;
        }
        everything = false;
        let shape = view.pointer_shape();
        window.set_pointer(shape)?;
        let now = now_ms();
        let deadline = view.deadline(now);
        let timeout = deadline.map(|deadline| (deadline.wrapping_sub(now) as i32).max(0) as u32);
        let watched = view.watched_descriptor();
        let (read, watched_ready) = window.read_events_or(&mut records, timeout, watched)?;
        view.tick(now_ms());
        if watched_ready && !view.descriptor_ready(now_ms()) {
            return Ok(());
        }
        for record in records[..read].as_chunks::<RECORD_SIZE>().0 {
            if let Some(query) = Query::from_record(record) {
                answer(window, view, &query);
                continue;
            }
            let (kind, for_window) = record_kind(record);
            if u32::from(for_window) == window.id() && kind == PALETTE_CHANGED {
                palette = record_word(record, 8);
                view.palette_changed(palette);
                view.motion_changed(record_word(record, 12) == 1);
                everything = true;
                continue;
            }
            if u32::from(for_window) == window.id() && kind == MENU_CHOSEN {
                let (menu, entry) = (record_word(record, 8), record_word(record, 12));
                view.menu_chosen(menu as usize, entry as usize, record_word(record, 4));
                continue;
            }
            let Some(event) = input::decode(record) else {
                continue;
            };
            if u32::from(event.window) != window.id() {
                continue;
            }
            match event.input {
                Input::CloseRequest => {
                    if view.close_requested(event.time_ms) {
                        return Ok(());
                    }
                }
                Input::Resize { width, height } => {
                    window.resize(width, height)?;
                    let (width, height) = window.size();
                    view.resize((width / scale) as i32, (height / scale) as i32);
                    everything = true;
                }
                Input::Key {
                    code: crate::input::key::BACK,
                    pressed,
                    ..
                } => {
                    if pressed && !view.back(event.time_ms) {
                        window.back_unused()?;
                    }
                }
                other => {
                    if other == Input::FocusIn {
                        let now_global = window.server().desktop_profile() == Ok(MACOS_PROFILE);
                        if now_global != global {
                            global = now_global;
                            view.set_global_menu(global);
                            everything = true;
                        }
                    }
                    if !view.handle(in_scale_1(other, scale as i32), event.time_ms) {
                        return Ok(());
                    }
                    let focus = view.text_focus();
                    let cause = match other {
                        Input::Button { pressed: true, .. } => Some(FOCUS_BY_TAP),
                        Input::Key {
                            code: crate::input::key::ENTER,
                            pressed: true,
                            ..
                        } => Some(FOCUS_BY_ACTIVATE),
                        _ => None,
                    };
                    let asked = cause.is_some() && focus.0 != TEXT_FOCUS_NONE;
                    if focus != sent_text_focus || asked {
                        let _ = window.set_text_focus(
                            focus.0,
                            focus.1,
                            cause.unwrap_or(FOCUS_OTHERWISE),
                        );
                        sent_text_focus = focus;
                    }
                }
            }
        }
    }
}

const TEXT_FOCUS_NONE: u32 = 0;
const TEXT_FOCUS_TEXT: u32 = 1;
const TEXT_FOCUS_MULTILINE: u32 = 2;
const TEXT_FOCUS_MASKED: u32 = 3;
const FOCUS_BY_TAP: u32 = 0;
const FOCUS_BY_ACTIVATE: u32 = 1;
const FOCUS_OTHERWISE: u32 = 2;
