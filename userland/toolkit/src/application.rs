//! The event loop of a window built with the toolkit: draw what changed, commit it, wait for
//! input or the next deadline, hand the input to the widgets and the program, again until the
//! user closes the window. Nothing runs while nothing happens: the loop sleeps in `ppoll`.

use crate::canvas::Canvas;
use crate::input::{self, Input, RECORD_SIZE};
use crate::tree::{Response, WidgetTree};
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

fn draw<const N: usize>(
    window: &mut Window,
    tree: &mut WidgetTree<N>,
    everything: bool,
) -> Result<(), Errno> {
    let damage = tree.take_damage();
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
            tree.paint(&mut canvas);
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

/// Shows `window` with `application`'s widgets laid out in it and serves it until the user
/// asks to close it.
pub fn run<const N: usize>(
    window: &mut Window,
    application: &mut impl Application<N>,
) -> Result<(), Errno> {
    let scale = window.scale().max(1);
    let (width, height) = window.size();
    application
        .tree()
        .resize((width / scale) as i32, (height / scale) as i32);
    let mut everything = true;
    let mut records = [0u8; RECORD_SIZE * RECORDS_READ];
    loop {
        draw(window, application.tree(), everything)?;
        everything = false;
        let shape = application.tree().pointer_shape();
        window.set_pointer(shape)?;
        let now = now_ms();
        let deadline = soonest(
            application.tree().next_deadline(now),
            application.deadline(now),
            now,
        );
        let timeout = deadline.map(|deadline| (deadline.wrapping_sub(now) as i32).max(0) as u32);
        let read = window.read_events(&mut records, timeout)?;
        let now = now_ms();
        application.tree().tick(now);
        if application
            .deadline(now)
            .is_some_and(|deadline| deadline.wrapping_sub(now) as i32 <= 0)
        {
            application.tick(now);
        }
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
                    application
                        .tree()
                        .resize((width / scale) as i32, (height / scale) as i32);
                    everything = true;
                }
                other => {
                    let response = application
                        .tree()
                        .handle(in_scale_1(other, scale as i32), event.time_ms);
                    application.respond(response, event.time_ms);
                    if other == Input::FocusIn {
                        application.focused(event.time_ms);
                    }
                }
            }
        }
    }
}
