//! NSScreen (D382, D729): one screen for each display the window server shows (`GET_SCREENS`),
//! the first the main one, or a single screen from the work area when the server lists none.
//! A screen's `visibleFrame` is its work area in AppKit's coordinates (bottom-left origin), its
//! `frame` reaches from the screen's origin to the work area's far edges, its
//! `backingScaleFactor` is the display's scale and its `deviceDescription` names it by
//! `NSScreenNumber`.

use super::server::{self, Display, MOST_SCREENS};
use super::{NSSCREEN, NSSCREEN_META};
use crate::foundation::{NSARRAY, NSDICTIONARY, NSNUMBER, class_pointer, new_instance};
use crate::libsystem::Exclusive;
use crate::objc::abi::{Id, Sel};
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, methods, sel, send1, send2};
use eggos_macos_gui::geometry::Frame;

#[repr(C)]
#[derive(Debug)]
struct ScreenFields {
    isa: usize,
    index: usize,
}

static SCREENS: Exclusive<[usize; MOST_SCREENS]> = Exclusive::new([0; MOST_SCREENS]);

fn fields(object: Id) -> &'static mut ScreenFields {
    // SAFETY: the callers pass NSScreen instances, whose memory starts with these fields.
    unsafe { &mut *object.cast::<ScreenFields>() }
}

fn displays() -> ([Display; MOST_SCREENS], usize) {
    let mut found = [Display::default(); MOST_SCREENS];
    let count = server::screens(&mut found);
    if count > 0 {
        return (found, count);
    }
    found[0] = Display {
        number: 0,
        area: server::work_area().unwrap_or((0, 0, 0, 0)),
        scale: 1,
    };
    (found, 1)
}

fn screen_at(index: usize) -> Id {
    SCREENS.with(|screens| {
        if screens[index] == 0 {
            let made = new_instance(class_pointer(&NSSCREEN));
            if !made.is_null() {
                fields(made).index = index;
            }
            screens[index] = made as usize;
        }
        screens[index] as Id
    })
}

fn display_of(this: Id) -> Display {
    let (found, count) = displays();
    found[fields(this).index.min(count - 1)]
}

fn frames(display: Display) -> (Frame, Frame) {
    let (x, y, width, height) = display.area;
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
    screen_at(0)
}

extern "C" fn screens(_: Id, _: Sel) -> Id {
    let (_, count) = displays();
    let mut objects = [core::ptr::null_mut(); MOST_SCREENS];
    let mut made = 0;
    for index in 0..count {
        let screen = screen_at(index);
        if !screen.is_null() {
            objects[made] = screen;
            made += 1;
        }
    }
    send2(
        class_pointer(&NSARRAY).cast(),
        sel!("arrayWithObjects:count:"),
        objects.as_ptr() as usize,
        made,
    )
}

extern "C" fn frame(this: Id, _: Sel) -> Frame {
    frames(display_of(this)).0
}

extern "C" fn visible_frame(this: Id, _: Sel) -> Frame {
    frames(display_of(this)).1
}

extern "C" fn backing_scale_factor(this: Id, _: Sel) -> f64 {
    f64::from(display_of(this).scale)
}

extern "C" fn localized_name(_: Id, _: Sel) -> Id {
    super::string("EggOS Display")
}

extern "C" fn device_description(this: Id, _: Sel) -> Id {
    let number = send1(
        class_pointer(&NSNUMBER).cast(),
        sel!("numberWithUnsignedInt:"),
        display_of(this).number as usize,
    );
    let key = super::string("NSScreenNumber");
    send2(
        class_pointer(&NSDICTIONARY).cast(),
        sel!("dictionaryWithObject:forKey:"),
        number as usize,
        key as usize,
    )
}

pub fn define(runtime: &mut Runtime) -> bool {
    let instance = methods![
        "frame" => frame,
        "visibleFrame" => visible_frame,
        "backingScaleFactor" => backing_scale_factor,
        "localizedName" => localized_name,
        "deviceDescription" => device_description,
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
