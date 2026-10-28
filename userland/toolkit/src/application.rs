//! The event loop of a window built with the toolkit: draw what changed, commit it, wait for
//! input or the next deadline, hand the input to the widgets and the program, again until the
//! user closes the window. Nothing runs while nothing happens: the loop sleeps in `ppoll`.

use crate::canvas::Canvas;
use crate::input::{self, Input, RECORD_SIZE};
use crate::damage::Damage;
use crate::tree::{PointerShape, Response, WidgetTree};
use crate::window::{Window, now_ms};
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
}

struct Tree<'a, A, const N: usize>(&'a mut A);

impl<A: Application<N>, const N: usize> View for Tree<'_, A, N> {
    fn resize(&mut self, width: i32, height: i32) {
        self.0.tree().resize(width, height);
    }

    fn handle(&mut self, input: Input, time_ms: u32) -> bool {
        let response = self.0.tree().handle(input, time_ms);
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
    run_view(window, &mut Tree(application))
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
        draw(window, view, everything)?;
        everything = false;
        let shape = view.pointer_shape();
        window.set_pointer(shape)?;
        let now = now_ms();
        let deadline = view.deadline(now);
        let timeout = deadline.map(|deadline| (deadline.wrapping_sub(now) as i32).max(0) as u32);
        let read = window.read_events(&mut records, timeout)?;
        view.tick(now_ms());
        for record in records[..read].as_chunks::<RECORD_SIZE>().0 {
            let Some(event) = input::decode(record) else {
                continue;
            };
            if u32::from(event.window) != window.id() {
                continue;
            }
            match event.input {
                Input::CloseRequest => return Ok(()),
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
