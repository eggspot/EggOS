//! The event loop of a window built with the toolkit: draw what changed, commit it, wait for
//! input or the next deadline, hand the input to the widgets and the program, again until the
//! user closes the window. Nothing runs while nothing happens: the loop sleeps in `ppoll`.

use crate::canvas::Canvas;
use crate::damage::Damage;
use crate::input::{self, Input, RECORD_SIZE};
use crate::tree::ROOT_ELEMENT;
use crate::tree::{PointerShape, Response, WidgetTree};
use crate::window::{Window, WindowServer, now_ms};
use eggos_element_tree::{
    Action, Bounds, Element, MAX_TEXT_BYTES, MAX_TREE_BYTES, Outcome, Query, Role, TreeWriter,
    query_kind, tree_answer,
};
use eggos_runtime::errno::Errno;

const RECORDS_READ: usize = 16;

/// A program's window: its widgets and what it does with their responses.
pub trait Application<const N: usize> {
    /// The window's widgets.
    fn tree(&mut self) -> &mut WidgetTree<N>;

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
}

fn draw(window: &mut Window, view: &mut impl View, everything: bool) -> Result<(), Errno> {
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
}

struct Tree<'a, A, const N: usize>(&'a mut A, Option<WindowServer>);

/// Answers a text field's clipboard response through the clipboard every app shares (D197,
/// docs/ux/toolkit-v0.4.md section 4.4): a copy or a cut gives [`WidgetTree::copied`] to the
/// server, a paste fetches its text into the field. What the program then sees: `Nothing` for
/// a copy, `Edited` for a cut, [`WidgetTree::paste`]'s answer for a paste; any other response
/// as it was.
pub fn answer_clipboard<const N: usize>(
    tree: &mut WidgetTree<N>,
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
            let mut bytes = [0u8; 255];
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

impl<A: Application<N>, const N: usize> View for Tree<'_, A, N> {
    fn resize(&mut self, width: i32, height: i32) {
        self.0.tree().resize(width, height);
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

    fn tick(&mut self, now_ms: u32) {
        self.0.tree().tick(now_ms);
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
pub fn run<const N: usize>(
    window: &mut Window,
    application: &mut impl Application<N>,
) -> Result<(), Errno> {
    run_view(window, &mut Tree(application, WindowServer::open().ok()))
}

/// Shows `window` with `view` drawn in it and serves it until the user asks to close it or
/// [`View::handle`] says so.
pub fn run_view(window: &mut Window, view: &mut impl View) -> Result<(), Errno> {
    let scale = window.scale().max(1);
    let (width, height) = window.size();
    view.resize((width / scale) as i32, (height / scale) as i32);
    let mut everything = true;
    let mut records = [0u8; RECORD_SIZE * RECORDS_READ];
    loop {
        if let Some(title) = view.take_title() {
            window.set_title(title)?;
        }
        draw(window, view, everything)?;
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
                other => {
                    if !view.handle(in_scale_1(other, scale as i32), event.time_ms) {
                        return Ok(());
                    }
                }
            }
        }
    }
}
