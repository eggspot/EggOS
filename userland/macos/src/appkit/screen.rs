//! NSScreen (D382): one screen, the window server's, whose `visibleFrame` is the server's
//! work area in AppKit's coordinates (bottom-left origin) and whose `frame` reaches from the
//! screen's origin to the work area's far edges, at the server's scale.

use super::{NSSCREEN, NSSCREEN_META, server};
use crate::foundation::{class_pointer, new_instance};
use crate::libsystem::Exclusive;
use crate::objc::abi::{Id, Sel};
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, methods, send2, sel};
use eggos_macos_gui::geometry::Frame;

#[repr(C)]
#[derive(Debug)]
struct ScreenFields {
    isa: usize,
}

static MAIN: Exclusive<usize> = Exclusive::new(0);

fn the_screen() -> Id {
    MAIN.with(|main| {
        if *main == 0 {
            *main = new_instance(class_pointer(&NSSCREEN)) as usize;
        }
        *main as Id
    })
}

fn frames() -> (Frame, Frame) {
    let (x, y, width, height) = server::work_area().unwrap_or((0, 0, 0, 0));
    let (screen_width, screen_height) = (x + width, y + height);
    let frame = Frame::new(0.0, 0.0, f64::from(screen_width), f64::from(screen_height));
    let visible = Frame::new(
        f64::from(x),
        f64::from(screen_height - (y + height)),
        f64::from(width),
        f64::from(height),
    );
    (frame, visible)
}

extern "C" fn main_screen(_: Id, _: Sel) -> Id {
    the_screen()
}

extern "C" fn screens(_: Id, _: Sel) -> Id {
    let screen = the_screen();
    send2(
        class_pointer(&crate::foundation::NSARRAY).cast(),
        sel!("arrayWithObjects:count:"),
        (&raw const screen) as usize,
        usize::from(!screen.is_null()),
    )
}

extern "C" fn frame(_: Id, _: Sel) -> Frame {
    frames().0
}

extern "C" fn visible_frame(_: Id, _: Sel) -> Frame {
    frames().1
}

extern "C" fn backing_scale_factor(_: Id, _: Sel) -> f64 {
    1.0
}

extern "C" fn localized_name(_: Id, _: Sel) -> Id {
    super::string("EggOS Display")
}

pub fn define(runtime: &mut Runtime) -> bool {
    let instance = methods![
        "frame" => frame,
        "visibleFrame" => visible_frame,
        "backingScaleFactor" => backing_scale_factor,
        "localizedName" => localized_name,
    ];
    let class = methods![
        "mainScreen" => main_screen,
        "deepestScreen" => main_screen,
        "screens" => screens,
    ];
    super::define(
        runtime,
        b"NSScreen\0",
        (&NSSCREEN, &NSSCREEN_META),
        &NSOBJECT,
        core::mem::size_of::<ScreenFields>(),
        &instance,
        &class,
    )
}
