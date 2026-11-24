//! CoreGraphics drawing behind `NSGraphicsContext` (D386): the one `CGContext` a view's
//! `drawRect:` draws through, with its graphics state stack (transform, fill and stroke
//! colours, line width) and its current path, the `CGContext...` functions that build and
//! paint paths and rectangles, and `CGColor`. Drawing goes to the window being displayed, in
//! the view's coordinates, as NSBezierPath's does; each view's drawing starts from a fresh
//! state. Painting is anti-aliased and blended at the colours' and the context's alpha, strokes
//! take caps, joins, a miter limit and dashes, and the clip is a mask the clipping calls narrow
//! and the state stack keeps (D738). `CGPath` objects, linear and radial gradients, blurred
//! shadows, colour spaces, affine transforms and `CGImage`s, made from NSImages or from bytes
//! through a data provider and drawn with `CGContextDrawImage` (D739).

use super::{NSCOLOR, NSGRAPHICSCONTEXT, NSGRAPHICSCONTEXT_META, drawing, string};
use crate::foundation::{class_pointer, new_instance};
use crate::libsystem::{Exclusive, heap};
use crate::objc::abi::{Id, Sel};
use crate::objc::root::NSOBJECT;
use crate::objc::reference_count::objc_retain;
use crate::objc::{Runtime, autorelease, class, methods, sel, send, send1};
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use eggos_macos_gui::bitmap::{self, Layout};
use eggos_macos_gui::colour::{self, Colour};
use eggos_macos_gui::geometry::{Frame, Point, Size};
use eggos_macos_gui::path::{Cap, ClipMask, Join, Path, StrokeStyle, Transform};
use eggos_macos_gui::shading::{Extend, Gradient, Shading, faded, paint_area, premultiply};
use eggos_toolkit::canvas::Canvas;
use eggos_toolkit::geometry::Rect;

const MOST_STATES: usize = 64;
const MOST_COLOURS: usize = 256;
const DRAW_FILL: i32 = 0;
const DRAW_EO_FILL: i32 = 1;
const DRAW_STROKE: i32 = 2;
const DRAW_FILL_STROKE: i32 = 3;
const DRAW_EO_FILL_STROKE: i32 = 4;
const MOST_DASHES: usize = 16;
const DEFAULT_SHADOW_ALPHA: f64 = 1.0 / 3.0;
const COLOUR_SPACE_MODEL_MONOCHROME: i32 = 0;
const COLOUR_SPACE_MODEL_RGB: i32 = 1;
const IMAGE_ALPHA_PREMULTIPLIED_FIRST: u32 = 2;
const IMAGE_BYTE_ORDER_32_LITTLE: u32 = 2 << 12;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Shadow {
    offset: (f64, f64),
    blur: f64,
    colour: [f64; 4],
}

#[derive(Debug, Clone, Copy)]
struct State {
    transform: Transform,
    fill: Colour,
    stroke: Colour,
    fill_alpha: f64,
    stroke_alpha: f64,
    alpha: f64,
    line_width: f64,
    cap: Cap,
    join: Join,
    miter_limit: f64,
    dashes: [f64; MOST_DASHES],
    dash_count: usize,
    dash_phase: f64,
    shadow: Option<Shadow>,
}

const FRESH: State = State {
    transform: Transform::IDENTITY,
    fill: Colour::opaque(0),
    stroke: Colour::opaque(0),
    fill_alpha: 1.0,
    stroke_alpha: 1.0,
    alpha: 1.0,
    line_width: 1.0,
    cap: Cap::Butt,
    join: Join::Miter,
    miter_limit: 10.0,
    dashes: [0.0; MOST_DASHES],
    dash_count: 0,
    dash_phase: 0.0,
    shadow: None,
};

impl State {
    fn stroke_style(&self) -> StrokeStyle<'_> {
        StrokeStyle {
            width: self.line_width,
            cap: self.cap,
            join: self.join,
            miter_limit: self.miter_limit,
            dashes: &self.dashes[..self.dash_count],
            dash_phase: self.dash_phase,
        }
    }
}

struct Context {
    state: State,
    saved: Vec<(State, Option<ClipMask>)>,
    path: Path,
    current: Option<Point>,
    clip: Option<ClipMask>,
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
            clip: None,
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
        context.clip = None;
    });
}

fn context_token() -> usize {
    &raw const CONTEXT_TOKEN as usize
}

extern "C" fn save_state(_: usize) {
    with_context(|context| {
        if context.saved.len() < MOST_STATES {
            let clip = context.clip.clone();
            context.saved.push((context.state, clip));
        }
    });
}

extern "C" fn restore_state(_: usize) {
    with_context(|context| {
        if let Some((state, clip)) = context.saved.pop() {
            context.state = state;
            context.clip = clip;
        }
    });
}

fn set_fill(colour: Colour, alpha: f64) {
    with_context(|context| {
        context.state.fill = colour;
        context.state.fill_alpha = alpha.clamp(0.0, 1.0);
    });
}

fn set_stroke(colour: Colour, alpha: f64) {
    with_context(|context| {
        context.state.stroke = colour;
        context.state.stroke_alpha = alpha.clamp(0.0, 1.0);
    });
}

extern "C" fn set_rgb_fill(_: usize, red: f64, green: f64, blue: f64, alpha: f64) {
    set_fill(colour::rgb(red, green, blue, alpha), alpha);
}

extern "C" fn set_rgb_stroke(_: usize, red: f64, green: f64, blue: f64, alpha: f64) {
    set_stroke(colour::rgb(red, green, blue, alpha), alpha);
}

extern "C" fn set_gray_fill(_: usize, white: f64, alpha: f64) {
    set_fill(colour::grey(white, alpha), alpha);
}

extern "C" fn set_gray_stroke(_: usize, white: f64, alpha: f64) {
    set_stroke(colour::grey(white, alpha), alpha);
}

extern "C" fn set_alpha(_: usize, alpha: f64) {
    with_context(|context| context.state.alpha = alpha.clamp(0.0, 1.0));
}

extern "C" fn set_line_cap(_: usize, cap: i32) {
    let cap = match cap {
        1 => Cap::Round,
        2 => Cap::Square,
        _ => Cap::Butt,
    };
    with_context(|context| context.state.cap = cap);
}

extern "C" fn set_line_join(_: usize, join: i32) {
    let join = match join {
        1 => Join::Round,
        2 => Join::Bevel,
        _ => Join::Miter,
    };
    with_context(|context| context.state.join = join);
}

extern "C" fn set_miter_limit(_: usize, limit: f64) {
    with_context(|context| context.state.miter_limit = limit.max(1.0));
}

extern "C" fn set_line_dash(_: usize, phase: f64, lengths: *const f64, count: usize) {
    let count = match lengths.is_null() {
        true => 0,
        false => count.min(MOST_DASHES),
    };
    let mut dashes = [0.0; MOST_DASHES];
    for (index, slot) in dashes.iter_mut().enumerate().take(count) {
        // SAFETY: the caller's `count` readable lengths.
        *slot = unsafe { lengths.add(index).read() }.max(0.0);
    }
    with_context(|context| {
        context.state.dashes = dashes;
        context.state.dash_count = count;
        context.state.dash_phase = phase;
    });
}

extern "C" fn set_should_antialias(_: usize, _: u8) {}

fn clip_to(path: Path, even_odd: bool) {
    let held = with_context(|context| context.clip.take());
    let mut made = None;
    drawing::with_canvas(|canvas, height, origin, _, _| {
        let mut moved = path.clone();
        moved.offset(origin.x, origin.y);
        let mut mask = held.clone().unwrap_or_else(|| ClipMask::rect(canvas.clip()));
        mask.intersect_path(&moved, height, even_odd);
        made = Some(mask);
    });
    with_context(|context| context.clip = made.or(held));
}

fn clip_current(even_odd: bool) {
    let path = with_context(|context| {
        context.current = None;
        core::mem::take(&mut context.path)
    });
    clip_to(path, even_odd);
}

extern "C" fn clip(_: usize) {
    clip_current(false);
}

extern "C" fn eo_clip(_: usize) {
    clip_current(true);
}

extern "C" fn clip_to_rect(_: usize, rect: Frame) {
    let (path, _) = rect_path(rect, false);
    clip_to(path, false);
}

extern "C" fn clip_to_rects(_: usize, rects: *const Frame, count: usize) {
    if rects.is_null() {
        return;
    }
    let path = with_context(|context| {
        let mut path = Path::new();
        for index in 0..count {
            // SAFETY: the caller's `count` readable rectangles.
            let rect = unsafe { rects.add(index).read() };
            path.rect(&context.state.transform, (rect.x, rect.y, rect.width, rect.height));
        }
        path
    });
    clip_to(path, false);
}

fn colour_of(colour: *const CgColour) -> Option<Colour> {
    // SAFETY: a `CGColorRef` the layer made, interned for the program's lifetime, or null.
    (!colour.is_null()).then(|| unsafe { (*colour).colour })
}

fn alpha_of(colour: *const CgColour) -> f64 {
    match colour.is_null() {
        true => 1.0,
        // SAFETY: a `CGColorRef` the layer made, interned for the program's lifetime.
        false => unsafe { (*colour).components[3] }.clamp(0.0, 1.0),
    }
}

extern "C" fn set_fill_colour(_: usize, colour: *const CgColour) {
    if let Some(made) = colour_of(colour) {
        let alpha = alpha_of(colour);
        with_context(|context| {
            context.state.fill = made;
            context.state.fill_alpha = alpha;
        });
    }
}

extern "C" fn set_stroke_colour(_: usize, colour: *const CgColour) {
    if let Some(made) = colour_of(colour) {
        let alpha = alpha_of(colour);
        with_context(|context| {
            context.state.stroke = made;
            context.state.stroke_alpha = alpha;
        });
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
    let mask = with_context(|context| context.clip.take());
    drawing::with_canvas(|canvas, height, origin, _, _| {
        let mut moved = path.clone();
        moved.offset(origin.x, origin.y);
        let clip = mask.as_ref();
        if let Some(shadow) = state.shadow {
            let (shape, shape_even_odd, alpha) = match fill {
                true => (moved.clone(), even_odd, state.fill_alpha),
                false => (moved.stroke_outline(&state.stroke_style()), false, state.stroke_alpha),
            };
            let cast = (shape, shape_even_odd);
            paint_shadow(canvas, height, cast, shadow, alpha * state.alpha, clip);
        }
        if fill {
            let ink = (state.fill, state.fill_alpha * state.alpha);
            moved.fill_with(canvas, height, ink, even_odd, clip);
        }
        if stroke {
            let ink = (state.stroke, state.stroke_alpha * state.alpha);
            moved.stroke_with(canvas, height, ink, &state.stroke_style(), clip);
        }
    });
    with_context(|context| context.clip = mask);
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
    state.fill_alpha = 1.0;
    state.alpha = 1.0;
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

fn paint_shadow(
    canvas: &mut Canvas<'_>,
    height: i32,
    (mut shape, even_odd): (Path, bool),
    shadow: Shadow,
    alpha: f64,
    clip: Option<&ClipMask>,
) {
    shape.offset(shadow.offset.0, shadow.offset.1);
    let Some(bounds) = shape.pixel_bounds(height) else {
        return;
    };
    let radius = (shadow.blur.max(0.0) / 2.0 + 0.5) as i32;
    let spread = radius + 1;
    let reach = canvas.clip();
    let reach = Rect::new(
        reach.x - spread,
        reach.y - spread,
        reach.width + 2 * spread,
        reach.height + 2 * spread,
    );
    let area = Rect::new(
        bounds.x - spread,
        bounds.y - spread,
        bounds.width + 2 * spread,
        bounds.height + 2 * spread,
    )
    .intersection(&reach);
    let mut cover = ClipMask::rect(area);
    cover.intersect_path(&shape, height, even_odd);
    cover.blur(radius);
    let pixel = premultiply(shadow.colour, alpha);
    let masks: Vec<&ClipMask> = core::iter::once(&cover).chain(clip).collect();
    paint_area(canvas, height, Some(area), &masks, |_| Some(pixel));
}

extern "C" fn set_shadow(_: usize, offset: Size, blur: f64) {
    let shadow = Shadow {
        offset: (offset.width, offset.height),
        blur,
        colour: [0.0, 0.0, 0.0, DEFAULT_SHADOW_ALPHA],
    };
    with_context(|context| context.state.shadow = Some(shadow));
}

extern "C" fn set_shadow_with_colour(_: usize, offset: Size, blur: f64, colour: *const CgColour) {
    let shadow = match colour.is_null() {
        true => None,
        false => Some(Shadow {
            offset: (offset.width, offset.height),
            blur,
            // SAFETY: a `CGColorRef` the layer made, interned for the program's lifetime.
            colour: unsafe { (*colour).components },
        }),
    };
    with_context(|context| context.state.shadow = shadow);
}

/// `CGAffineTransform`, laid out as the ABI passes it.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AffineTransform {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    tx: f64,
    ty: f64,
}

impl From<AffineTransform> for Transform {
    fn from(made: AffineTransform) -> Self {
        Transform {
            a: made.a,
            b: made.b,
            c: made.c,
            d: made.d,
            tx: made.tx,
            ty: made.ty,
        }
    }
}

impl From<Transform> for AffineTransform {
    fn from(made: Transform) -> Self {
        AffineTransform {
            a: made.a,
            b: made.b,
            c: made.c,
            d: made.d,
            tx: made.tx,
            ty: made.ty,
        }
    }
}

static IDENTITY_TRANSFORM: AffineTransform = AffineTransform {
    a: 1.0,
    b: 0.0,
    c: 0.0,
    d: 1.0,
    tx: 0.0,
    ty: 0.0,
};

fn transform_at(transform: *const AffineTransform) -> Transform {
    match transform.is_null() {
        true => Transform::IDENTITY,
        // SAFETY: the caller's readable `CGAffineTransform`.
        false => Transform::from(unsafe { transform.read() }),
    }
}

extern "C" fn concat_ctm(_: usize, transform: AffineTransform) {
    with_context(|context| {
        context.state.transform = context.state.transform.after(&Transform::from(transform));
    });
}

extern "C" fn get_ctm(_: usize) -> AffineTransform {
    with_context(|context| AffineTransform::from(context.state.transform))
}

extern "C" fn make_translation(x: f64, y: f64) -> AffineTransform {
    AffineTransform::from(Transform::IDENTITY.translated(x, y))
}

extern "C" fn make_scale(x: f64, y: f64) -> AffineTransform {
    AffineTransform::from(Transform::IDENTITY.scaled(x, y))
}

extern "C" fn make_rotation(angle: f64) -> AffineTransform {
    AffineTransform::from(Transform::IDENTITY.rotated(angle))
}

extern "C" fn transform_concat(first: AffineTransform, then: AffineTransform) -> AffineTransform {
    AffineTransform::from(Transform::from(then).after(&Transform::from(first)))
}

extern "C" fn transform_invert(transform: AffineTransform) -> AffineTransform {
    Transform::from(transform)
        .inverse()
        .map_or(transform, AffineTransform::from)
}

extern "C" fn transform_translate(transform: AffineTransform, x: f64, y: f64) -> AffineTransform {
    AffineTransform::from(Transform::from(transform).translated(x, y))
}

extern "C" fn transform_scale(transform: AffineTransform, x: f64, y: f64) -> AffineTransform {
    AffineTransform::from(Transform::from(transform).scaled(x, y))
}

extern "C" fn transform_rotate(transform: AffineTransform, angle: f64) -> AffineTransform {
    AffineTransform::from(Transform::from(transform).rotated(angle))
}

extern "C" fn point_apply(point: Point, transform: AffineTransform) -> Point {
    Transform::from(transform).apply(point)
}

/// A `CGPathRef`: a path of points already under each call's transform.
#[derive(Debug)]
pub struct CgPath {
    references: usize,
    path: Path,
    current: Option<Point>,
}

fn new_path(path: Path, current: Option<Point>) -> *mut CgPath {
    Box::into_raw(Box::new(CgPath {
        references: 1,
        path,
        current,
    }))
}

fn path_at<'a>(path: *mut CgPath) -> Option<&'a mut CgPath> {
    // SAFETY: a `CGPathRef` the layer made that the program still holds, or null.
    unsafe { path.as_mut() }
}

extern "C" fn path_create_mutable() -> *mut CgPath {
    new_path(Path::new(), None)
}

extern "C" fn path_create_copy(path: *mut CgPath) -> *mut CgPath {
    match path_at(path) {
        Some(path) => new_path(path.path.clone(), path.current),
        None => core::ptr::null_mut(),
    }
}

extern "C" fn path_retain(path: *mut CgPath) -> *mut CgPath {
    if let Some(held) = path_at(path) {
        held.references += 1;
    }
    path
}

extern "C" fn path_release(path: *mut CgPath) {
    let Some(held) = path_at(path) else {
        return;
    };
    held.references -= 1;
    if held.references == 0 {
        // SAFETY: made by `Box::into_raw` in `new_path`, and its last reference is gone.
        drop(unsafe { Box::from_raw(path) });
    }
}

fn build(
    path: *mut CgPath,
    transform: *const AffineTransform,
    work: impl FnOnce(&mut CgPath, &Transform),
) {
    if let Some(path) = path_at(path) {
        work(path, &transform_at(transform));
    }
}

extern "C" fn path_move_to(path: *mut CgPath, transform: *const AffineTransform, x: f64, y: f64) {
    build(path, transform, |path, transform| {
        path.path.move_to(transform.apply(Point::new(x, y)));
        path.current = Some(Point::new(x, y));
    });
}

extern "C" fn path_line_to(path: *mut CgPath, transform: *const AffineTransform, x: f64, y: f64) {
    build(path, transform, |path, transform| {
        path.path.line_to(transform.apply(Point::new(x, y)));
        path.current = Some(Point::new(x, y));
    });
}

#[expect(clippy::too_many_arguments, reason = "the C function's own parameters")]
extern "C" fn path_curve_to(
    path: *mut CgPath,
    transform: *const AffineTransform,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    x: f64,
    y: f64,
) {
    build(path, transform, |path, transform| {
        path.path.curve_to(
            transform.apply(Point::new(x1, y1)),
            transform.apply(Point::new(x2, y2)),
            transform.apply(Point::new(x, y)),
        );
        path.current = Some(Point::new(x, y));
    });
}

extern "C" fn path_quad_to(
    path: *mut CgPath,
    transform: *const AffineTransform,
    cx: f64,
    cy: f64,
    x: f64,
    y: f64,
) {
    build(path, transform, |path, transform| {
        let control = transform.apply(Point::new(cx, cy));
        path.path.quad_to(control, transform.apply(Point::new(x, y)));
        path.current = Some(Point::new(x, y));
    });
}

extern "C" fn path_add_rect(path: *mut CgPath, transform: *const AffineTransform, rect: Frame) {
    build(path, transform, |path, transform| {
        path.path.rect(transform, (rect.x, rect.y, rect.width, rect.height));
        path.current = Some(Point::new(rect.x, rect.y));
    });
}

extern "C" fn path_add_rects(
    path: *mut CgPath,
    transform: *const AffineTransform,
    rects: *const Frame,
    count: usize,
) {
    if rects.is_null() {
        return;
    }
    for index in 0..count {
        // SAFETY: the caller's `count` readable rectangles.
        path_add_rect(path, transform, unsafe { rects.add(index).read() });
    }
}

extern "C" fn path_add_ellipse(path: *mut CgPath, transform: *const AffineTransform, rect: Frame) {
    build(path, transform, |path, transform| {
        path.path.ellipse(transform, (rect.x, rect.y, rect.width, rect.height));
        path.current = Some(Point::new(rect.x + rect.width, rect.y + rect.height / 2.0));
    });
}

extern "C" fn path_add_rounded_rect(
    path: *mut CgPath,
    transform: *const AffineTransform,
    rect: Frame,
    corner_width: f64,
    corner_height: f64,
) {
    build(path, transform, |path, transform| {
        let shape = (rect.x, rect.y, rect.width, rect.height);
        path.path.rounded_rect(transform, shape, (corner_width, corner_height));
        path.current = Some(Point::new(rect.x + rect.width - corner_width, rect.y));
    });
}

#[expect(clippy::too_many_arguments, reason = "the C function's own parameters")]
extern "C" fn path_add_arc(
    path: *mut CgPath,
    transform: *const AffineTransform,
    x: f64,
    y: f64,
    radius: f64,
    start: f64,
    end: f64,
    clockwise: u8,
) {
    build(path, transform, |path, transform| {
        let centre = Point::new(x, y);
        path.path.arc(transform, centre, radius, (start, end), clockwise != 0);
        let (s, c) = (eggos_math::sin(end), eggos_math::cos(end));
        path.current = Some(Point::new(x + radius * c, y + radius * s));
    });
}

extern "C" fn path_add_arc_to(
    path: *mut CgPath,
    transform: *const AffineTransform,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    radius: f64,
) {
    build(path, transform, |path, transform| {
        let corner = Point::new(x1, y1);
        let from = path.current.unwrap_or(corner);
        path.path.arc_to(transform, from, corner, Point::new(x2, y2), radius);
        path.current = Some(Point::new(x2, y2));
    });
}

extern "C" fn path_add_path(
    path: *mut CgPath,
    transform: *const AffineTransform,
    other: *mut CgPath,
) {
    let Some(other) = path_at(other).map(|other| (other.path.clone(), other.current)) else {
        return;
    };
    build(path, transform, |path, transform| {
        path.path.append(&other.0, transform);
        path.current = other.1.or(path.current);
    });
}

extern "C" fn path_close(path: *mut CgPath) {
    if let Some(path) = path_at(path) {
        path.path.close();
    }
}

extern "C" fn path_create_with_rect(rect: Frame, transform: *const AffineTransform) -> *mut CgPath {
    let made = path_create_mutable();
    path_add_rect(made, transform, rect);
    made
}

extern "C" fn path_create_with_ellipse(
    rect: Frame,
    transform: *const AffineTransform,
) -> *mut CgPath {
    let made = path_create_mutable();
    path_add_ellipse(made, transform, rect);
    made
}

extern "C" fn path_create_with_rounded_rect(
    rect: Frame,
    corner_width: f64,
    corner_height: f64,
    transform: *const AffineTransform,
) -> *mut CgPath {
    let made = path_create_mutable();
    path_add_rounded_rect(made, transform, rect, corner_width, corner_height);
    made
}

extern "C" fn path_is_empty(path: *mut CgPath) -> u8 {
    path_at(path).is_none_or(|path| path.path.is_empty()) as u8
}

fn null_rect() -> Frame {
    Frame::new(f64::INFINITY, f64::INFINITY, 0.0, 0.0)
}

extern "C" fn path_bounding_box(path: *mut CgPath) -> Frame {
    path_at(path)
        .and_then(|path| path.path.bounds())
        .map_or_else(null_rect, |(x, y, width, height)| Frame::new(x, y, width, height))
}

extern "C" fn path_contains(
    path: *mut CgPath,
    transform: *const AffineTransform,
    point: Point,
    even_odd: u8,
) -> u8 {
    let point = transform_at(transform).apply(point);
    path_at(path).is_some_and(|path| path.path.contains(point, even_odd != 0)) as u8
}

extern "C" fn path_current_point(path: *mut CgPath) -> Point {
    path_at(path).and_then(|path| path.current).unwrap_or_default()
}

extern "C" fn context_add_path(_: usize, path: *mut CgPath) {
    let Some(path) = path_at(path) else {
        return;
    };
    with_context(|context| {
        context.path.append(&path.path, &context.state.transform);
        context.current = path.current.or(context.current);
    });
}

extern "C" fn context_is_path_empty(_: usize) -> u8 {
    with_context(|context| context.path.is_empty()) as u8
}

extern "C" fn context_current_point(_: usize) -> Point {
    with_context(|context| context.current.unwrap_or_default())
}

static SRGB_NAME: AtomicUsize = AtomicUsize::new(0);
static GENERIC_RGB_NAME: AtomicUsize = AtomicUsize::new(0);
static LINEAR_SRGB_NAME: AtomicUsize = AtomicUsize::new(0);
static EXTENDED_SRGB_NAME: AtomicUsize = AtomicUsize::new(0);
static DISPLAY_P3_NAME: AtomicUsize = AtomicUsize::new(0);
static GENERIC_GREY_NAME: AtomicUsize = AtomicUsize::new(0);
static GENERIC_GREY_GAMMA_NAME: AtomicUsize = AtomicUsize::new(0);
static LINEAR_GREY_NAME: AtomicUsize = AtomicUsize::new(0);

static SPACE_NAMES: [(&AtomicUsize, &[u8], &str); 8] = [
    (&SRGB_NAME, b"_kCGColorSpaceSRGB", "kCGColorSpaceSRGB"),
    (&GENERIC_RGB_NAME, b"_kCGColorSpaceGenericRGB", "kCGColorSpaceGenericRGB"),
    (&LINEAR_SRGB_NAME, b"_kCGColorSpaceLinearSRGB", "kCGColorSpaceLinearSRGB"),
    (&EXTENDED_SRGB_NAME, b"_kCGColorSpaceExtendedSRGB", "kCGColorSpaceExtendedSRGB"),
    (&DISPLAY_P3_NAME, b"_kCGColorSpaceDisplayP3", "kCGColorSpaceDisplayP3"),
    (&GENERIC_GREY_NAME, b"_kCGColorSpaceGenericGray", "kCGColorSpaceGenericGray"),
    (
        &GENERIC_GREY_GAMMA_NAME,
        b"_kCGColorSpaceGenericGrayGamma2_2",
        "kCGColorSpaceGenericGrayGamma2_2",
    ),
    (&LINEAR_GREY_NAME, b"_kCGColorSpaceLinearGray", "kCGColorSpaceLinearGray"),
];

/// Makes the colour space names' strings; false if there is no memory.
pub fn set_up() -> bool {
    let pool = autorelease::push();
    let mut made = true;
    for &(slot, _, name) in SPACE_NAMES.iter() {
        let object = string(name);
        made &= !object.is_null();
        slot.store(objc_retain(object) as usize, Ordering::Relaxed);
    }
    autorelease::pop(pool);
    made
}

static RGB_SPACE: u64 = 0;
static GREY_SPACE: u64 = 0;

fn rgb_space() -> usize {
    &raw const RGB_SPACE as usize
}

fn grey_space() -> usize {
    &raw const GREY_SPACE as usize
}

fn channels_of(space: usize) -> usize {
    match space == grey_space() {
        true => 1,
        false => 3,
    }
}

fn colour_from(space: usize, components: *const f64) -> [f64; 4] {
    let read = |index: usize| {
        // SAFETY: the caller's colour, one component per channel of `space` and alpha.
        unsafe { components.add(index).read() }
    };
    match channels_of(space) {
        1 => [read(0), read(0), read(0), read(1)],
        _ => [read(0), read(1), read(2), read(3)],
    }
}

extern "C" fn space_create_rgb() -> usize {
    rgb_space()
}

extern "C" fn space_create_grey() -> usize {
    grey_space()
}

extern "C" fn space_create_with_name(name: Id) -> usize {
    let grey = !name.is_null() && {
        let text = send(name, sel!("UTF8String")) as *const u8;
        !text.is_null() && {
            // SAFETY: a NUL-terminated name the string object keeps alive.
            let bytes = unsafe { core::ffi::CStr::from_ptr(text.cast()) }.to_bytes();
            bytes.windows(4).any(|word| word == b"Gray")
        }
    };
    match grey {
        true => grey_space(),
        false => rgb_space(),
    }
}

extern "C" fn space_retain(space: usize) -> usize {
    space
}

extern "C" fn space_release(_: usize) {}

extern "C" fn space_number_of_components(space: usize) -> usize {
    channels_of(space)
}

extern "C" fn space_model(space: usize) -> i32 {
    match channels_of(space) {
        1 => COLOUR_SPACE_MODEL_MONOCHROME,
        _ => COLOUR_SPACE_MODEL_RGB,
    }
}

extern "C" fn colour_create(space: usize, components: *const f64) -> *mut CgColour {
    if components.is_null() {
        return core::ptr::null_mut();
    }
    let [red, green, blue, alpha] = colour_from(space, components);
    intern(colour::rgb(red, green, blue, alpha), [red, green, blue, alpha])
}

/// A `CGGradientRef`.
#[derive(Debug)]
pub struct CgGradient {
    references: usize,
    gradient: Gradient,
}

fn new_gradient(colours: &[[f64; 4]], locations: *const f64) -> *mut CgGradient {
    let places: Option<Vec<f64>> = (!locations.is_null()).then(|| {
        (0..colours.len())
            // SAFETY: the caller's one location per colour.
            .map(|index| unsafe { locations.add(index).read() })
            .collect()
    });
    Box::into_raw(Box::new(CgGradient {
        references: 1,
        gradient: Gradient::new(colours, places.as_deref()),
    }))
}

extern "C" fn gradient_create_with_components(
    space: usize,
    components: *const f64,
    locations: *const f64,
    count: usize,
) -> *mut CgGradient {
    if components.is_null() {
        return core::ptr::null_mut();
    }
    let step = channels_of(space) + 1;
    let colours: Vec<[f64; 4]> = (0..count)
        // SAFETY: the caller's `count` colours of `step` components each.
        .map(|index| colour_from(space, unsafe { components.add(index * step) }))
        .collect();
    new_gradient(&colours, locations)
}

extern "C" fn gradient_create_with_colours(
    _: usize,
    colours: Id,
    locations: *const f64,
) -> *mut CgGradient {
    let count = match colours.is_null() {
        true => 0,
        false => send(colours, sel!("count")) as usize,
    };
    let made: Vec<[f64; 4]> = (0..count)
        .map(|index| send1(colours, sel!("objectAtIndex:"), index) as *const CgColour)
        .filter(|colour| !colour.is_null())
        // SAFETY: the array holds `CGColorRef`s the layer made, interned for good.
        .map(|colour| unsafe { (*colour).components })
        .collect();
    new_gradient(&made, locations)
}

fn gradient_at<'a>(gradient: *mut CgGradient) -> Option<&'a mut CgGradient> {
    // SAFETY: a `CGGradientRef` the layer made that the program still holds, or null.
    unsafe { gradient.as_mut() }
}

extern "C" fn gradient_retain(gradient: *mut CgGradient) -> *mut CgGradient {
    if let Some(held) = gradient_at(gradient) {
        held.references += 1;
    }
    gradient
}

extern "C" fn gradient_release(gradient: *mut CgGradient) {
    let Some(held) = gradient_at(gradient) else {
        return;
    };
    held.references -= 1;
    if held.references == 0 {
        // SAFETY: made by `Box::into_raw` in `new_gradient`, and its last reference is gone.
        drop(unsafe { Box::from_raw(gradient) });
    }
}

fn device_transform(state: &State, origin: Point) -> Transform {
    Transform {
        tx: origin.x,
        ty: origin.y,
        ..Transform::IDENTITY
    }
    .after(&state.transform)
}

fn draw_shading(gradient: *mut CgGradient, shading: Shading, options: u32) {
    let Some(gradient) = gradient_at(gradient) else {
        return;
    };
    let (state, mask) = with_context(|context| (context.state, context.clip.take()));
    drawing::with_canvas(|canvas, height, origin, _, _| {
        let device = device_transform(&state, origin);
        let scale = eggos_math::sqrt((device.a * device.d - device.b * device.c).abs());
        let shading = match shading {
            Shading::Linear { start, end } => Shading::Linear {
                start: device.apply(start),
                end: device.apply(end),
            },
            Shading::Radial {
                start,
                start_radius,
                end,
                end_radius,
            } => Shading::Radial {
                start: device.apply(start),
                start_radius: start_radius * scale,
                end: device.apply(end),
                end_radius: end_radius * scale,
            },
        };
        let extend = Extend::from_options(options);
        let held = mask.as_ref();
        paint_area(canvas, height, None, held.as_slice(), |point| {
            let t = shading.parameter(point, extend)?;
            Some(premultiply(gradient.gradient.colour_at(t), state.alpha))
        });
    });
    with_context(|context| context.clip = mask);
}

extern "C" fn draw_linear_gradient(
    _: usize,
    gradient: *mut CgGradient,
    start: Point,
    end: Point,
    options: u32,
) {
    draw_shading(gradient, Shading::Linear { start, end }, options);
}

extern "C" fn draw_radial_gradient(
    _: usize,
    gradient: *mut CgGradient,
    start: Point,
    start_radius: f64,
    end: Point,
    end_radius: f64,
    options: u32,
) {
    let shading = Shading::Radial {
        start,
        start_radius,
        end,
        end_radius,
    };
    draw_shading(gradient, shading, options);
}

/// A `CGImageRef`: premultiplied pixels, top row first.
#[derive(Debug)]
pub struct CgImage {
    references: usize,
    width: usize,
    height: usize,
    pixels: Vec<u32>,
}

/// A new `CGImageRef` holding a copy of `pixels`, `width` wide, with one reference.
pub fn new_image(pixels: &[u32], width: usize) -> *mut CgImage {
    let height = pixels.len() / width.max(1);
    Box::into_raw(Box::new(CgImage {
        references: 1,
        width,
        height,
        pixels: pixels.to_vec(),
    }))
}

/// Runs `work` on `image`'s premultiplied pixels, width and height; `None` for null.
pub fn with_image_pixels<R>(
    image: *mut CgImage,
    work: impl FnOnce(&[u32], usize, usize) -> R,
) -> Option<R> {
    image_at(image).map(|image| work(&image.pixels, image.width, image.height))
}

fn image_at<'a>(image: *mut CgImage) -> Option<&'a mut CgImage> {
    // SAFETY: a `CGImageRef` the layer made that the program still holds, or null.
    unsafe { image.as_mut() }
}

extern "C" fn image_retain(image: *mut CgImage) -> *mut CgImage {
    if let Some(held) = image_at(image) {
        held.references += 1;
    }
    image
}

/// Gives up one reference to `image`, freeing it with its last.
pub extern "C" fn image_release(image: *mut CgImage) {
    let Some(held) = image_at(image) else {
        return;
    };
    held.references -= 1;
    if held.references == 0 {
        // SAFETY: made by `Box::into_raw` in `new_image`, and its last reference is gone.
        drop(unsafe { Box::from_raw(image) });
    }
}

extern "C" fn image_width(image: *mut CgImage) -> usize {
    image_at(image).map_or(0, |image| image.width)
}

extern "C" fn image_height(image: *mut CgImage) -> usize {
    image_at(image).map_or(0, |image| image.height)
}

extern "C" fn image_bits_per_component(_: *mut CgImage) -> usize {
    8
}

extern "C" fn image_bits_per_pixel(_: *mut CgImage) -> usize {
    32
}

extern "C" fn image_bytes_per_row(image: *mut CgImage) -> usize {
    image_width(image) * 4
}

extern "C" fn image_alpha_info(_: *mut CgImage) -> u32 {
    IMAGE_ALPHA_PREMULTIPLIED_FIRST
}

extern "C" fn image_bitmap_info(_: *mut CgImage) -> u32 {
    IMAGE_ALPHA_PREMULTIPLIED_FIRST | IMAGE_BYTE_ORDER_32_LITTLE
}

extern "C" fn image_colour_space(_: *mut CgImage) -> usize {
    rgb_space()
}

extern "C" fn image_create_with_rect(image: *mut CgImage, rect: Frame) -> *mut CgImage {
    let Some(image) = image_at(image) else {
        return core::ptr::null_mut();
    };
    let whole = |value: f64| value.max(0.0) as usize;
    let part = (whole(rect.x), whole(rect.y), whole(rect.width), whole(rect.height));
    let (pixels, width, _) = bitmap::crop(&image.pixels, image.width, part);
    match pixels.is_empty() {
        true => core::ptr::null_mut(),
        false => new_image(&pixels, width),
    }
}

/// A `CGDataProviderRef`: a copy of the bytes it was made with.
#[derive(Debug)]
pub struct CgDataProvider {
    references: usize,
    bytes: Vec<u8>,
}

type ReleaseData = extern "C" fn(usize, *const u8, usize);

extern "C" fn provider_create_with_data(
    info: usize,
    data: *const u8,
    size: usize,
    release: Option<ReleaseData>,
) -> *mut CgDataProvider {
    let bytes = match data.is_null() {
        true => Vec::new(),
        // SAFETY: the caller's `size` readable bytes, read before `release` gives them back.
        false => unsafe { core::slice::from_raw_parts(data, size) }.to_vec(),
    };
    if let Some(release) = release {
        release(info, data, size);
    }
    Box::into_raw(Box::new(CgDataProvider {
        references: 1,
        bytes,
    }))
}

fn provider_at<'a>(provider: *mut CgDataProvider) -> Option<&'a mut CgDataProvider> {
    // SAFETY: a `CGDataProviderRef` the layer made that the program still holds, or null.
    unsafe { provider.as_mut() }
}

extern "C" fn provider_retain(provider: *mut CgDataProvider) -> *mut CgDataProvider {
    if let Some(held) = provider_at(provider) {
        held.references += 1;
    }
    provider
}

extern "C" fn provider_release(provider: *mut CgDataProvider) {
    let Some(held) = provider_at(provider) else {
        return;
    };
    held.references -= 1;
    if held.references == 0 {
        // SAFETY: made by `Box::into_raw` in `provider_create_with_data`, last reference gone.
        drop(unsafe { Box::from_raw(provider) });
    }
}

#[expect(clippy::too_many_arguments, reason = "the C function's own parameters")]
extern "C" fn image_create(
    width: usize,
    height: usize,
    bits_per_component: usize,
    bits_per_pixel: usize,
    bytes_per_row: usize,
    _: usize,
    info: u32,
    provider: *mut CgDataProvider,
    _: *const f64,
    _: u8,
    _: i32,
) -> *mut CgImage {
    let Some(provider) = provider_at(provider) else {
        return core::ptr::null_mut();
    };
    let layout = Layout {
        width,
        height,
        bits_per_component,
        bits_per_pixel,
        bytes_per_row,
        info,
    };
    match bitmap::premultiplied_pixels(&provider.bytes, layout) {
        Some(pixels) => new_image(&pixels, width),
        None => core::ptr::null_mut(),
    }
}

extern "C" fn draw_image(_: usize, rect: Frame, image: *mut CgImage) {
    let Some(image) = image_at(image) else {
        return;
    };
    if image.width == 0 || image.height == 0 || rect.width == 0.0 || rect.height == 0.0 {
        return;
    }
    let (state, mask) = with_context(|context| (context.state, context.clip.take()));
    drawing::with_canvas(|canvas, height, origin, _, _| {
        let device = device_transform(&state, origin);
        let Some(back) = device.inverse() else {
            return;
        };
        let mut outline = Path::new();
        outline.rect(&device, (rect.x, rect.y, rect.width, rect.height));
        let Some(area) = outline.pixel_bounds(height) else {
            return;
        };
        let share = (state.alpha.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
        let held = mask.as_ref();
        paint_area(canvas, height, Some(area), held.as_slice(), |point| {
            let user = back.apply(point);
            let across = (user.x - rect.x) / rect.width;
            let down = (rect.y + rect.height - user.y) / rect.height;
            if !(0.0..1.0).contains(&across) || !(0.0..1.0).contains(&down) {
                return None;
            }
            let x = ((across * image.width as f64) as usize).min(image.width - 1);
            let y = ((down * image.height as f64) as usize).min(image.height - 1);
            Some(faded(image.pixels[y * image.width + x], share))
        });
    });
    with_context(|context| context.clip = mask);
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

/// The address of the CoreGraphics function or variable `name`, if the layer has it.
pub fn resolve(name: &[u8]) -> Option<u64> {
    if let Some((slot, _, _)) = SPACE_NAMES.iter().find(|(_, symbol, _)| *symbol == name) {
        return Some(*slot as *const AtomicUsize as u64);
    }
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
        b"_CGContextSetLineCap" => set_line_cap as *const () as usize,
        b"_CGContextSetLineJoin" => set_line_join as *const () as usize,
        b"_CGContextSetMiterLimit" => set_miter_limit as *const () as usize,
        b"_CGContextSetLineDash" => set_line_dash as *const () as usize,
        b"_CGContextSetAlpha" => set_alpha as *const () as usize,
        b"_CGContextSetShouldAntialias" | b"_CGContextSetAllowsAntialiasing" => {
            set_should_antialias as *const () as usize
        }
        b"_CGContextClip" => clip as *const () as usize,
        b"_CGContextEOClip" => eo_clip as *const () as usize,
        b"_CGContextClipToRect" => clip_to_rect as *const () as usize,
        b"_CGContextClipToRects" => clip_to_rects as *const () as usize,
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
        b"_CGColorCreate" => colour_create as *const () as usize,
        b"_CGContextSetShadow" => set_shadow as *const () as usize,
        b"_CGContextSetShadowWithColor" => set_shadow_with_colour as *const () as usize,
        b"_CGContextConcatCTM" => concat_ctm as *const () as usize,
        b"_CGContextGetCTM" => get_ctm as *const () as usize,
        b"_CGAffineTransformIdentity" => &raw const IDENTITY_TRANSFORM as usize,
        b"_CGAffineTransformMakeTranslation" => make_translation as *const () as usize,
        b"_CGAffineTransformMakeScale" => make_scale as *const () as usize,
        b"_CGAffineTransformMakeRotation" => make_rotation as *const () as usize,
        b"_CGAffineTransformConcat" => transform_concat as *const () as usize,
        b"_CGAffineTransformInvert" => transform_invert as *const () as usize,
        b"_CGAffineTransformTranslate" => transform_translate as *const () as usize,
        b"_CGAffineTransformScale" => transform_scale as *const () as usize,
        b"_CGAffineTransformRotate" => transform_rotate as *const () as usize,
        b"_CGPointApplyAffineTransform" => point_apply as *const () as usize,
        b"_CGPathCreateMutable" => path_create_mutable as *const () as usize,
        b"_CGPathCreateCopy" | b"_CGPathCreateMutableCopy" => {
            path_create_copy as *const () as usize
        }
        b"_CGPathRetain" => path_retain as *const () as usize,
        b"_CGPathRelease" => path_release as *const () as usize,
        b"_CGPathMoveToPoint" => path_move_to as *const () as usize,
        b"_CGPathAddLineToPoint" => path_line_to as *const () as usize,
        b"_CGPathAddCurveToPoint" => path_curve_to as *const () as usize,
        b"_CGPathAddQuadCurveToPoint" => path_quad_to as *const () as usize,
        b"_CGPathAddRect" => path_add_rect as *const () as usize,
        b"_CGPathAddRects" => path_add_rects as *const () as usize,
        b"_CGPathAddEllipseInRect" => path_add_ellipse as *const () as usize,
        b"_CGPathAddRoundedRect" => path_add_rounded_rect as *const () as usize,
        b"_CGPathAddArc" => path_add_arc as *const () as usize,
        b"_CGPathAddArcToPoint" => path_add_arc_to as *const () as usize,
        b"_CGPathAddPath" => path_add_path as *const () as usize,
        b"_CGPathCloseSubpath" => path_close as *const () as usize,
        b"_CGPathCreateWithRect" => path_create_with_rect as *const () as usize,
        b"_CGPathCreateWithEllipseInRect" => path_create_with_ellipse as *const () as usize,
        b"_CGPathCreateWithRoundedRect" => path_create_with_rounded_rect as *const () as usize,
        b"_CGPathIsEmpty" => path_is_empty as *const () as usize,
        b"_CGPathGetBoundingBox" | b"_CGPathGetPathBoundingBox" => {
            path_bounding_box as *const () as usize
        }
        b"_CGPathContainsPoint" => path_contains as *const () as usize,
        b"_CGPathGetCurrentPoint" => path_current_point as *const () as usize,
        b"_CGContextAddPath" => context_add_path as *const () as usize,
        b"_CGContextIsPathEmpty" => context_is_path_empty as *const () as usize,
        b"_CGContextGetPathCurrentPoint" => context_current_point as *const () as usize,
        b"_CGColorSpaceCreateDeviceRGB" => space_create_rgb as *const () as usize,
        b"_CGColorSpaceCreateDeviceGray" => space_create_grey as *const () as usize,
        b"_CGColorSpaceCreateWithName" => space_create_with_name as *const () as usize,
        b"_CGColorSpaceRetain" => space_retain as *const () as usize,
        b"_CGColorSpaceRelease" => space_release as *const () as usize,
        b"_CGColorSpaceGetNumberOfComponents" => space_number_of_components as *const () as usize,
        b"_CGColorSpaceGetModel" => space_model as *const () as usize,
        b"_CGGradientCreateWithColorComponents" => {
            gradient_create_with_components as *const () as usize
        }
        b"_CGGradientCreateWithColors" => gradient_create_with_colours as *const () as usize,
        b"_CGGradientRetain" => gradient_retain as *const () as usize,
        b"_CGGradientRelease" => gradient_release as *const () as usize,
        b"_CGContextDrawLinearGradient" => draw_linear_gradient as *const () as usize,
        b"_CGContextDrawRadialGradient" => draw_radial_gradient as *const () as usize,
        b"_CGDataProviderCreateWithData" => provider_create_with_data as *const () as usize,
        b"_CGDataProviderRetain" => provider_retain as *const () as usize,
        b"_CGDataProviderRelease" => provider_release as *const () as usize,
        b"_CGImageCreate" => image_create as *const () as usize,
        b"_CGImageCreateWithImageInRect" => image_create_with_rect as *const () as usize,
        b"_CGImageRetain" => image_retain as *const () as usize,
        b"_CGImageRelease" => image_release as *const () as usize,
        b"_CGImageGetWidth" => image_width as *const () as usize,
        b"_CGImageGetHeight" => image_height as *const () as usize,
        b"_CGImageGetBitsPerComponent" => image_bits_per_component as *const () as usize,
        b"_CGImageGetBitsPerPixel" => image_bits_per_pixel as *const () as usize,
        b"_CGImageGetBytesPerRow" => image_bytes_per_row as *const () as usize,
        b"_CGImageGetAlphaInfo" => image_alpha_info as *const () as usize,
        b"_CGImageGetBitmapInfo" => image_bitmap_info as *const () as usize,
        b"_CGImageGetColorSpace" => image_colour_space as *const () as usize,
        b"_CGContextDrawImage" => draw_image as *const () as usize,
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
