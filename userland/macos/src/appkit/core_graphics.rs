//! CoreGraphics drawing behind `NSGraphicsContext` (D386): the one `CGContext` a view's
//! `drawRect:` draws through, with its graphics state stack (transform, fill and stroke
//! colours, line width) and its current path, the `CGContext...` functions that build and
//! paint paths and rectangles, and `CGColor`. Drawing goes to the window being displayed, in
//! the view's coordinates, as NSBezierPath's does; each view's drawing starts from a fresh
//! state.

use super::{NSCOLOR, NSGRAPHICSCONTEXT, NSGRAPHICSCONTEXT_META, drawing};
use crate::foundation::{class_pointer, new_instance};
use crate::libsystem::{Exclusive, heap};
use crate::objc::abi::{Id, Sel};
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, class, methods};
use alloc::vec::Vec;
use eggos_macos_gui::colour::{self, Colour};
use eggos_macos_gui::geometry::{Frame, Point};
use eggos_macos_gui::path::{Path, Transform};

const MOST_STATES: usize = 64;
const MOST_COLOURS: usize = 256;
const DRAW_FILL: i32 = 0;
const DRAW_EO_FILL: i32 = 1;
const DRAW_STROKE: i32 = 2;
const DRAW_FILL_STROKE: i32 = 3;
const DRAW_EO_FILL_STROKE: i32 = 4;

#[derive(Debug, Clone, Copy)]
struct State {
    transform: Transform,
    fill: Colour,
    stroke: Colour,
    line_width: f64,
}

const FRESH: State = State {
    transform: Transform::IDENTITY,
    fill: Colour::opaque(0),
    stroke: Colour::opaque(0),
    line_width: 1.0,
};

struct Context {
    state: State,
    saved: Vec<State>,
    path: Path,
    current: Option<Point>,
}

static CONTEXT: Exclusive<Option<Context>> = Exclusive::new(None);

static CONTEXT_TOKEN: u64 = 0;

/// A `CGColorRef`: an interned colour.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CgColour {
    colour: Colour,
    components: [f64; 4],
}

static COLOURS: Exclusive<Vec<usize>> = Exclusive::new(Vec::new());

fn with_context<R>(work: impl FnOnce(&mut Context) -> R) -> R {
    CONTEXT.with(|context| {
        let context = context.get_or_insert_with(|| Context {
            state: FRESH,
            saved: Vec::new(),
            path: Path::new(),
            current: None,
        });
        work(context)
    })
}

/// Starts a view's drawing from a fresh state: called as each view's `drawRect:` begins.
pub fn reset() {
    with_context(|context| {
        context.state = FRESH;
        context.saved.clear();
        context.path = Path::new();
        context.current = None;
    });
}

fn context_token() -> usize {
    &raw const CONTEXT_TOKEN as usize
}

extern "C" fn save_state(_: usize) {
    with_context(|context| {
        if context.saved.len() < MOST_STATES {
            context.saved.push(context.state);
        }
    });
}

extern "C" fn restore_state(_: usize) {
    with_context(|context| {
        if let Some(state) = context.saved.pop() {
            context.state = state;
        }
    });
}

extern "C" fn set_rgb_fill(_: usize, red: f64, green: f64, blue: f64, alpha: f64) {
    with_context(|context| context.state.fill = colour::rgb(red, green, blue, alpha));
}

extern "C" fn set_rgb_stroke(_: usize, red: f64, green: f64, blue: f64, alpha: f64) {
    with_context(|context| context.state.stroke = colour::rgb(red, green, blue, alpha));
}

extern "C" fn set_gray_fill(_: usize, white: f64, alpha: f64) {
    with_context(|context| context.state.fill = colour::grey(white, alpha));
}

extern "C" fn set_gray_stroke(_: usize, white: f64, alpha: f64) {
    with_context(|context| context.state.stroke = colour::grey(white, alpha));
}

fn colour_of(colour: *const CgColour) -> Option<Colour> {
    // SAFETY: a `CGColorRef` the layer made, interned for the program's lifetime, or null.
    (!colour.is_null()).then(|| unsafe { (*colour).colour })
}

extern "C" fn set_fill_colour(_: usize, colour: *const CgColour) {
    if let Some(colour) = colour_of(colour) {
        with_context(|context| context.state.fill = colour);
    }
}

extern "C" fn set_stroke_colour(_: usize, colour: *const CgColour) {
    if let Some(colour) = colour_of(colour) {
        with_context(|context| context.state.stroke = colour);
    }
}

extern "C" fn set_line_width(_: usize, width: f64) {
    with_context(|context| context.state.line_width = width.max(0.0));
}

extern "C" fn translate(_: usize, x: f64, y: f64) {
    with_context(|context| context.state.transform = context.state.transform.translated(x, y));
}

extern "C" fn scale(_: usize, x: f64, y: f64) {
    with_context(|context| context.state.transform = context.state.transform.scaled(x, y));
}

extern "C" fn rotate(_: usize, angle: f64) {
    with_context(|context| context.state.transform = context.state.transform.rotated(angle));
}

extern "C" fn begin_path(_: usize) {
    with_context(|context| {
        context.path = Path::new();
        context.current = None;
    });
}

extern "C" fn move_to(_: usize, x: f64, y: f64) {
    with_context(|context| {
        let point = Point::new(x, y);
        context.path.move_to(context.state.transform.apply(point));
        context.current = Some(point);
    });
}

extern "C" fn line_to(_: usize, x: f64, y: f64) {
    with_context(|context| {
        let point = Point::new(x, y);
        context.path.line_to(context.state.transform.apply(point));
        context.current = Some(point);
    });
}

extern "C" fn curve_to(_: usize, x1: f64, y1: f64, x2: f64, y2: f64, x: f64, y: f64) {
    with_context(|context| {
        let transform = context.state.transform;
        context.path.curve_to(
            transform.apply(Point::new(x1, y1)),
            transform.apply(Point::new(x2, y2)),
            transform.apply(Point::new(x, y)),
        );
        context.current = Some(Point::new(x, y));
    });
}

extern "C" fn add_arc(_: usize, x: f64, y: f64, radius: f64, start: f64, end: f64, clockwise: i32) {
    with_context(|context| {
        let transform = context.state.transform;
        context.path.arc(
            &transform,
            Point::new(x, y),
            radius,
            (start, end),
            clockwise != 0,
        );
        let (s, c) = (eggos_math::sin(end), eggos_math::cos(end));
        context.current = Some(Point::new(x + radius * c, y + radius * s));
    });
}

extern "C" fn add_arc_to(_: usize, x1: f64, y1: f64, x2: f64, y2: f64, radius: f64) {
    with_context(|context| {
        let transform = context.state.transform;
        let from = context.current.unwrap_or(Point::new(x1, y1));
        context.path.arc_to(
            &transform,
            from,
            Point::new(x1, y1),
            Point::new(x2, y2),
            radius,
        );
        context.current = Some(Point::new(x2, y2));
    });
}

extern "C" fn close_path(_: usize) {
    with_context(|context| context.path.close());
}

extern "C" fn add_rect(_: usize, rect: Frame) {
    with_context(|context| {
        let transform = context.state.transform;
        context
            .path
            .rect(&transform, (rect.x, rect.y, rect.width, rect.height));
    });
}

extern "C" fn add_ellipse(_: usize, rect: Frame) {
    with_context(|context| {
        let transform = context.state.transform;
        context
            .path
            .ellipse(&transform, (rect.x, rect.y, rect.width, rect.height));
    });
}

fn paint(path: &Path, state: State, mode: i32) {
    let (fill, even_odd, stroke) = match mode {
        DRAW_FILL => (true, false, false),
        DRAW_EO_FILL => (true, true, false),
        DRAW_STROKE => (false, false, true),
        DRAW_FILL_STROKE => (true, false, true),
        DRAW_EO_FILL_STROKE => (true, true, true),
        _ => return,
    };
    drawing::with_canvas(|canvas, height, origin, _, _| {
        let mut moved = path.clone();
        moved.offset(origin.x, origin.y);
        if fill {
            moved.fill(canvas, height, state.fill, even_odd);
        }
        if stroke {
            moved.stroke(canvas, height, state.stroke, state.line_width);
        }
    });
}

fn draw_current(mode: i32) {
    let (path, state) = with_context(|context| {
        let path = core::mem::take(&mut context.path);
        context.current = None;
        (path, context.state)
    });
    paint(&path, state, mode);
}

extern "C" fn fill_path(_: usize) {
    draw_current(DRAW_FILL);
}

extern "C" fn eo_fill_path(_: usize) {
    draw_current(DRAW_EO_FILL);
}

extern "C" fn stroke_path(_: usize) {
    draw_current(DRAW_STROKE);
}

extern "C" fn draw_path(_: usize, mode: i32) {
    draw_current(mode);
}

fn rect_path(rect: Frame, ellipse: bool) -> (Path, State) {
    with_context(|context| {
        let mut path = Path::new();
        let shape = (rect.x, rect.y, rect.width, rect.height);
        match ellipse {
            true => path.ellipse(&context.state.transform, shape),
            false => path.rect(&context.state.transform, shape),
        }
        (path, context.state)
    })
}

extern "C" fn fill_rect(_: usize, rect: Frame) {
    let (path, state) = rect_path(rect, false);
    paint(&path, state, DRAW_FILL);
}

extern "C" fn stroke_rect(_: usize, rect: Frame) {
    let (path, state) = rect_path(rect, false);
    paint(&path, state, DRAW_STROKE);
}

extern "C" fn clear_rect(_: usize, rect: Frame) {
    let (path, mut state) = rect_path(rect, false);
    state.fill = Colour::opaque(eggos_toolkit::colour::WINDOW_BACKGROUND);
    paint(&path, state, DRAW_FILL);
}

extern "C" fn fill_ellipse(_: usize, rect: Frame) {
    let (path, state) = rect_path(rect, true);
    paint(&path, state, DRAW_FILL);
}

extern "C" fn stroke_ellipse(_: usize, rect: Frame) {
    let (path, state) = rect_path(rect, true);
    paint(&path, state, DRAW_STROKE);
}

/// An interned `CGColorRef` for `colour` with its `components`.
pub fn intern(colour: Colour, components: [f64; 4]) -> *mut CgColour {
    COLOURS.with(|colours| {
        if let Some(&found) = colours.iter().find(|&&each| {
            // SAFETY: interned colours live for the program's lifetime.
            let each = unsafe { &*(each as *const CgColour) };
            each.colour == colour
        }) {
            return found as *mut CgColour;
        }
        if colours.len() == MOST_COLOURS {
            return colours[0] as *mut CgColour;
        }
        let made = heap::allocate(core::mem::size_of::<CgColour>(), true).cast::<CgColour>();
        if made.is_null() {
            return made;
        }
        // SAFETY: a new block of the record's size.
        unsafe { made.write(CgColour { colour, components }) };
        colours.push(made as usize);
        made
    })
}

extern "C" fn create_generic_rgb(red: f64, green: f64, blue: f64, alpha: f64) -> *mut CgColour {
    intern(
        colour::rgb(red, green, blue, alpha),
        [red, green, blue, alpha],
    )
}

extern "C" fn create_generic_gray(white: f64, alpha: f64) -> *mut CgColour {
    intern(colour::grey(white, alpha), [white, white, white, alpha])
}

extern "C" fn colour_retain(colour: *mut CgColour) -> *mut CgColour {
    colour
}

extern "C" fn colour_release(_: *mut CgColour) {}

extern "C" fn colour_alpha(colour: *const CgColour) -> f64 {
    match colour.is_null() {
        true => 0.0,
        // SAFETY: an interned colour.
        false => unsafe { (*colour).components[3] },
    }
}

extern "C" fn colour_components(colour: *const CgColour) -> *const f64 {
    match colour.is_null() {
        true => core::ptr::null(),
        // SAFETY: an interned colour, which lives as long as the program.
        false => unsafe { (*colour).components.as_ptr() },
    }
}

extern "C" fn colour_number_of_components(_: *const CgColour) -> usize {
    4
}

extern "C" fn ns_colour_cg_colour(this: Id, _: Sel) -> *mut CgColour {
    let colour = drawing::colour_of(this).unwrap_or(Colour::opaque(0));
    let channel = |shift: u32| f64::from(colour.pixel >> shift & 0xff) / 255.0;
    let alpha = match colour.visible {
        true => 1.0,
        false => 0.0,
    };
    intern(colour, [channel(16), channel(8), channel(0), alpha])
}

#[repr(C)]
#[derive(Debug)]
struct GraphicsContextFields {
    isa: usize,
}

static CURRENT: Exclusive<usize> = Exclusive::new(0);

extern "C" fn current_context(_: Id, _: Sel) -> Id {
    CURRENT.with(|current| {
        if *current == 0 {
            *current = new_instance(class_pointer(&NSGRAPHICSCONTEXT)) as usize;
        }
        *current as Id
    })
}

extern "C" fn cg_context(_: Id, _: Sel) -> usize {
    context_token()
}

extern "C" fn class_save(_: Id, _: Sel) {
    save_state(0);
}

extern "C" fn class_restore(_: Id, _: Sel) {
    restore_state(0);
}

extern "C" fn ignore_object(_: Id, _: Sel, _: Id) {}

extern "C" fn flush(_: Id, _: Sel) {}

/// The address of the CoreGraphics function `name`, if the layer has it.
pub fn resolve(name: &[u8]) -> Option<u64> {
    let address = match name {
        b"_CGContextSaveGState" => save_state as *const () as usize,
        b"_CGContextRestoreGState" => restore_state as *const () as usize,
        b"_CGContextSetRGBFillColor" => set_rgb_fill as *const () as usize,
        b"_CGContextSetRGBStrokeColor" => set_rgb_stroke as *const () as usize,
        b"_CGContextSetGrayFillColor" => set_gray_fill as *const () as usize,
        b"_CGContextSetGrayStrokeColor" => set_gray_stroke as *const () as usize,
        b"_CGContextSetFillColorWithColor" => set_fill_colour as *const () as usize,
        b"_CGContextSetStrokeColorWithColor" => set_stroke_colour as *const () as usize,
        b"_CGContextSetLineWidth" => set_line_width as *const () as usize,
        b"_CGContextTranslateCTM" => translate as *const () as usize,
        b"_CGContextScaleCTM" => scale as *const () as usize,
        b"_CGContextRotateCTM" => rotate as *const () as usize,
        b"_CGContextBeginPath" => begin_path as *const () as usize,
        b"_CGContextMoveToPoint" => move_to as *const () as usize,
        b"_CGContextAddLineToPoint" => line_to as *const () as usize,
        b"_CGContextAddCurveToPoint" => curve_to as *const () as usize,
        b"_CGContextAddArc" => add_arc as *const () as usize,
        b"_CGContextAddArcToPoint" => add_arc_to as *const () as usize,
        b"_CGContextClosePath" => close_path as *const () as usize,
        b"_CGContextAddRect" => add_rect as *const () as usize,
        b"_CGContextAddEllipseInRect" => add_ellipse as *const () as usize,
        b"_CGContextFillPath" => fill_path as *const () as usize,
        b"_CGContextEOFillPath" => eo_fill_path as *const () as usize,
        b"_CGContextStrokePath" => stroke_path as *const () as usize,
        b"_CGContextDrawPath" => draw_path as *const () as usize,
        b"_CGContextFillRect" => fill_rect as *const () as usize,
        b"_CGContextStrokeRect" => stroke_rect as *const () as usize,
        b"_CGContextClearRect" => clear_rect as *const () as usize,
        b"_CGContextFillEllipseInRect" => fill_ellipse as *const () as usize,
        b"_CGContextStrokeEllipseInRect" => stroke_ellipse as *const () as usize,
        b"_CGColorCreateGenericRGB" | b"_CGColorCreateSRGB" => {
            create_generic_rgb as *const () as usize
        }
        b"_CGColorCreateGenericGray" => create_generic_gray as *const () as usize,
        b"_CGColorRetain" => colour_retain as *const () as usize,
        b"_CGColorRelease" => colour_release as *const () as usize,
        b"_CGColorGetAlpha" => colour_alpha as *const () as usize,
        b"_CGColorGetComponents" => colour_components as *const () as usize,
        b"_CGColorGetNumberOfComponents" => colour_number_of_components as *const () as usize,
        _ => return None,
    };
    Some(address as u64)
}

pub fn define(runtime: &mut Runtime) -> bool {
    let instance = methods![
        "CGContext" => cg_context,
        "graphicsPort" => cg_context,
        "saveGraphicsState" => class_save,
        "restoreGraphicsState" => class_restore,
        "flushGraphics" => flush,
    ];
    let class_methods = methods![
        "currentContext" => current_context,
        "saveGraphicsState" => class_save,
        "restoreGraphicsState" => class_restore,
        "setCurrentContext:" => ignore_object,
    ];
    let colour_methods = methods![
        "CGColor" => ns_colour_cg_colour,
    ];
    super::define(
        runtime,
        b"NSGraphicsContext\0",
        (&NSGRAPHICSCONTEXT, &NSGRAPHICSCONTEXT_META),
        &NSOBJECT,
        core::mem::size_of::<GraphicsContextFields>(),
        &instance,
        &class_methods,
    ) && class::extend(runtime, &NSCOLOR, &colour_methods, &[])
}
