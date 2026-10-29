//! Drawing inside `drawRect:` (`docs/ux/macos-gui-v0.3.md` sections 0 and 1): the current
//! drawing context (the window's pixels, the view's origin and clip, the fill and stroke
//! colours), NSColor, NSFont, NSBezierPath, `NSRectFill` and its kin, NSString's drawing
//! additions and the attribute names they read. Every primitive draws through
//! `eggos_macos_gui::paint` into the window being displayed; outside a display it draws nothing.

use super::{
    NSBEZIERPATH, NSBEZIERPATH_META, NSCOLOR, NSCOLOR_META, NSFONT, NSFONT_META, is, server,
    string, with_text,
};
use crate::foundation::{NSSTRING, autoreleased, class_pointer, new_instance};
use crate::libsystem::Exclusive;
use crate::objc::abi::{Id, Sel};
use crate::objc::buffer::Buffer;
use crate::objc::reference_count::{dispose, objc_retain};
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, autorelease, class, methods, sel, send1};
use core::sync::atomic::{AtomicUsize, Ordering};
use eggos_macos_gui::colour::{self, Colour};
use eggos_macos_gui::geometry::{Frame, Point, Size};
use eggos_macos_gui::paint;
use eggos_toolkit::canvas::Canvas;
use eggos_toolkit::geometry::Rect;

const BLACK: Colour = Colour::opaque(0);
const WHITE: Colour = Colour::opaque(0x00ff_ffff);
const SYSTEM_FONT_SIZE: f64 = 13.0;
const SMALL_FONT_SIZE: f64 = 11.0;
const LABEL_FONT_SIZE: f64 = 10.0;
const ASCENDER: f64 = 12.0;
const DESCENDER: f64 = -4.0;
const X_HEIGHT: f64 = 7.0;
const CAP_HEIGHT: f64 = 10.0;

#[derive(Debug, Clone, Copy)]
struct Context {
    buffer: Option<server::Buffer>,
    origin: Point,
    clip: Rect,
    fill: Colour,
    stroke: Colour,
}

static CONTEXT: Exclusive<Context> = Exclusive::new(Context {
    buffer: None,
    origin: Point::new(0.0, 0.0),
    clip: Rect::new(0, 0, 0, 0),
    fill: BLACK,
    stroke: BLACK,
});

static FOREGROUND_COLOUR_NAME: AtomicUsize = AtomicUsize::new(0);
static FONT_NAME: AtomicUsize = AtomicUsize::new(0);
static BACKGROUND_COLOUR_NAME: AtomicUsize = AtomicUsize::new(0);

/// Starts drawing into `buffer` for a view whose bounds' origin is `origin` in the window,
/// clipped to `clip`; the colours start black, as each view's do on macOS.
pub fn begin(buffer: server::Buffer, origin: Point, clip: Rect) {
    CONTEXT.with(|context| {
        *context = Context {
            buffer: Some(buffer),
            origin,
            clip,
            fill: BLACK,
            stroke: BLACK,
        };
    });
}

/// Ends drawing: primitives draw nothing until the next [`begin`].
pub fn end() {
    CONTEXT.with(|context| context.buffer = None);
}

/// Runs `work` on a canvas over the window being drawn, clipped, with the content's height, the
/// view's origin and the fill and stroke colours; nothing outside a display.
pub fn with_canvas(work: impl FnOnce(&mut Canvas<'_>, i32, Point, Colour, Colour)) {
    CONTEXT.with(|context| {
        let Some(buffer) = context.buffer else {
            return;
        };
        // SAFETY: the window's buffer stays mapped while it is displayed, and only the thread
        // that displays it draws, inside this lock.
        let pixels = unsafe { buffer.pixels() };
        let Some(mut canvas) = Canvas::new(pixels, buffer.width, buffer.height, 1) else {
            return;
        };
        canvas.narrow(context.clip);
        work(
            &mut canvas,
            buffer.height as i32,
            context.origin,
            context.fill,
            context.stroke,
        );
    });
}

fn in_window(frame: Frame, origin: Point) -> Frame {
    frame.offset(origin.x, origin.y)
}

fn point_in_window(point: Point, origin: Point) -> Point {
    Point::new(point.x + origin.x, point.y + origin.y)
}

#[repr(C)]
struct ColourFields {
    isa: usize,
    colour: Colour,
    alpha: f64,
}

fn colour_fields(object: Id) -> &'static mut ColourFields {
    // SAFETY: the callers pass NSColor instances.
    unsafe { &mut *object.cast::<ColourFields>() }
}

/// A new autoreleased NSColor of `colour`.
pub fn new_colour(colour: Colour, alpha: f64) -> Id {
    let object = new_instance(class_pointer(&NSCOLOR));
    if object.is_null() {
        return object;
    }
    let fields = colour_fields(object);
    fields.colour = colour;
    fields.alpha = alpha;
    autoreleased(object)
}

/// The colour of `object`, if it is an NSColor.
pub fn colour_of(object: Id) -> Option<Colour> {
    is(object, &NSCOLOR).then(|| colour_fields(object).colour)
}

extern "C" fn named_colour(_: Id, sel: Sel) -> Id {
    // SAFETY: a selector is its canonical, permanent name.
    let name = unsafe { class::abi_name(sel) };
    let made = colour::named(name).unwrap_or(BLACK);
    new_colour(made, if made.visible { 1.0 } else { 0.0 })
}

extern "C" fn colour_with_rgba(_: Id, _: Sel, red: f64, green: f64, blue: f64, alpha: f64) -> Id {
    new_colour(colour::rgb(red, green, blue, alpha), alpha)
}

extern "C" fn colour_with_white(_: Id, _: Sel, white: f64, alpha: f64) -> Id {
    new_colour(colour::grey(white, alpha), alpha)
}

extern "C" fn set_colour(this: Id, _: Sel) {
    let colour = colour_fields(this).colour;
    CONTEXT.with(|context| {
        context.fill = colour;
        context.stroke = colour;
    });
}

extern "C" fn set_fill_colour(this: Id, _: Sel) {
    let colour = colour_fields(this).colour;
    CONTEXT.with(|context| context.fill = colour);
}

extern "C" fn set_stroke_colour(this: Id, _: Sel) {
    let colour = colour_fields(this).colour;
    CONTEXT.with(|context| context.stroke = colour);
}

extern "C" fn with_alpha(this: Id, _: Sel, alpha: f64) -> Id {
    let colour = colour_fields(this).colour;
    new_colour(
        Colour {
            visible: alpha > 0.0,
            ..colour
        },
        alpha,
    )
}

extern "C" fn alpha_component(this: Id, _: Sel) -> f64 {
    colour_fields(this).alpha
}

fn component(this: Id, shift: u32) -> f64 {
    f64::from((colour_fields(this).colour.pixel >> shift) & 0xff) / 255.0
}

extern "C" fn red_component(this: Id, _: Sel) -> f64 {
    component(this, 16)
}

extern "C" fn green_component(this: Id, _: Sel) -> f64 {
    component(this, 8)
}

extern "C" fn blue_component(this: Id, _: Sel) -> f64 {
    component(this, 0)
}

extern "C" fn this_with(this: Id, _: Sel, _: Id) -> Id {
    this
}

#[repr(C)]
struct FontFields {
    isa: usize,
    size: f64,
}

fn new_font(size: f64) -> Id {
    let object = new_instance(class_pointer(&NSFONT));
    if object.is_null() {
        return object;
    }
    // SAFETY: a new NSFont instance.
    unsafe {
        (*object.cast::<FontFields>()).size = if size > 0.0 { size } else { SYSTEM_FONT_SIZE }
    };
    autoreleased(object)
}

fn font_size(object: Id) -> f64 {
    if !is(object, &NSFONT) {
        return SYSTEM_FONT_SIZE;
    }
    // SAFETY: an NSFont instance.
    unsafe { (*object.cast::<FontFields>()).size }
}

/// The glyph scale text in `font` (an NSFont, or nil for the system font) draws at.
pub fn font_scale(font: Id) -> i32 {
    paint::font_scale(font_size(font))
}

extern "C" fn font_of_size(_: Id, _: Sel, size: f64) -> Id {
    new_font(size)
}

extern "C" fn font_of_size_weight(_: Id, _: Sel, size: f64, _: f64) -> Id {
    new_font(size)
}

extern "C" fn font_named(_: Id, _: Sel, _: Id, size: f64) -> Id {
    new_font(size)
}

extern "C" fn system_font_size(_: Id, _: Sel) -> f64 {
    SYSTEM_FONT_SIZE
}

extern "C" fn small_font_size(_: Id, _: Sel) -> f64 {
    SMALL_FONT_SIZE
}

extern "C" fn label_font_size(_: Id, _: Sel) -> f64 {
    LABEL_FONT_SIZE
}

extern "C" fn point_size(this: Id, _: Sel) -> f64 {
    font_size(this)
}

fn scaled(this: Id, value: f64) -> f64 {
    value * f64::from(font_scale(this))
}

extern "C" fn ascender(this: Id, _: Sel) -> f64 {
    scaled(this, ASCENDER)
}

extern "C" fn descender(this: Id, _: Sel) -> f64 {
    scaled(this, DESCENDER)
}

extern "C" fn leading(_: Id, _: Sel) -> f64 {
    0.0
}

extern "C" fn x_height(this: Id, _: Sel) -> f64 {
    scaled(this, X_HEIGHT)
}

extern "C" fn cap_height(this: Id, _: Sel) -> f64 {
    scaled(this, CAP_HEIGHT)
}

extern "C" fn font_name(_: Id, _: Sel) -> Id {
    string("Egg Sans")
}

#[derive(Debug, Clone, Copy)]
enum Element {
    Move(Point),
    Line(Point),
    Close,
    Rect(Frame),
    Oval(Frame),
}

#[repr(C)]
struct PathFields {
    isa: usize,
    elements: Buffer<Element>,
    line_width: f64,
}

fn path_fields(object: Id) -> &'static mut PathFields {
    // SAFETY: the callers pass NSBezierPath instances.
    unsafe { &mut *object.cast::<PathFields>() }
}

fn new_path(first: Option<Element>) -> Id {
    let object = new_instance(class_pointer(&NSBEZIERPATH));
    if object.is_null() {
        return object;
    }
    let fields = path_fields(object);
    fields.line_width = 1.0;
    if let Some(element) = first {
        fields.elements.push(element);
    }
    autoreleased(object)
}

extern "C" fn path(_: Id, _: Sel) -> Id {
    new_path(None)
}

extern "C" fn path_with_rect(_: Id, _: Sel, rect: Frame) -> Id {
    new_path(Some(Element::Rect(rect)))
}

extern "C" fn path_with_oval(_: Id, _: Sel, rect: Frame) -> Id {
    new_path(Some(Element::Oval(rect)))
}

fn push(this: Id, element: Element) {
    path_fields(this).elements.push(element);
}

extern "C" fn move_to(this: Id, _: Sel, point: Point) {
    push(this, Element::Move(point));
}

extern "C" fn line_to(this: Id, _: Sel, point: Point) {
    push(this, Element::Line(point));
}

extern "C" fn close_path(this: Id, _: Sel) {
    push(this, Element::Close);
}

extern "C" fn append_rect(this: Id, _: Sel, rect: Frame) {
    push(this, Element::Rect(rect));
}

extern "C" fn append_oval(this: Id, _: Sel, rect: Frame) {
    push(this, Element::Oval(rect));
}

extern "C" fn remove_all_points(this: Id, _: Sel) {
    path_fields(this).elements.free();
}

extern "C" fn set_line_width(this: Id, _: Sel, width: f64) {
    path_fields(this).line_width = width;
}

extern "C" fn line_width(this: Id, _: Sel) -> f64 {
    path_fields(this).line_width
}

extern "C" fn is_empty(this: Id, _: Sel) -> u8 {
    u8::from(path_fields(this).elements.len() == 0)
}

fn draw_elements(elements: &[Element], filled: bool) {
    with_canvas(|canvas, height, origin, fill, stroke| {
        let ink = if filled { fill } else { stroke };
        let mut start = Point::default();
        let mut current = Point::default();
        for element in elements {
            match *element {
                Element::Rect(rect) if filled => {
                    paint::fill(canvas, height, in_window(rect, origin), ink);
                }
                Element::Rect(rect) => {
                    paint::outline(canvas, height, in_window(rect, origin), ink);
                }
                Element::Oval(rect) => {
                    paint::oval(canvas, height, in_window(rect, origin), ink, filled);
                }
                Element::Move(point) => {
                    start = point;
                    current = point;
                }
                Element::Line(point) => {
                    if !filled {
                        paint::line(
                            canvas,
                            height,
                            point_in_window(current, origin),
                            point_in_window(point, origin),
                            ink,
                        );
                    }
                    current = point;
                }
                Element::Close => {
                    if !filled {
                        paint::line(
                            canvas,
                            height,
                            point_in_window(current, origin),
                            point_in_window(start, origin),
                            ink,
                        );
                    }
                    current = start;
                }
            }
        }
    });
}

extern "C" fn fill_path(this: Id, _: Sel) {
    draw_elements(path_fields(this).elements.as_slice(), true);
}

extern "C" fn stroke_path(this: Id, _: Sel) {
    draw_elements(path_fields(this).elements.as_slice(), false);
}

extern "C" fn path_dealloc(this: Id, _: Sel) {
    path_fields(this).elements.free();
    dispose(this);
}

extern "C" fn fill_rect_class(_: Id, _: Sel, rect: Frame) {
    draw_elements(&[Element::Rect(rect)], true);
}

extern "C" fn stroke_rect_class(_: Id, _: Sel, rect: Frame) {
    draw_elements(&[Element::Rect(rect)], false);
}

extern "C" fn stroke_line_class(_: Id, _: Sel, from: Point, to: Point) {
    draw_elements(&[Element::Move(from), Element::Line(to)], false);
}

extern "C" fn rect_fill(rect: Frame) {
    draw_elements(&[Element::Rect(rect)], true);
}

extern "C" fn rect_fill_using_operation(rect: Frame, _: usize) {
    rect_fill(rect);
}

extern "C" fn rect_fill_list(rects: *const Frame, count: isize) {
    if rects.is_null() || count <= 0 {
        return;
    }
    // SAFETY: the program passes `count` rects.
    let rects = unsafe { core::slice::from_raw_parts(rects, count as usize) };
    for &rect in rects {
        rect_fill(rect);
    }
}

extern "C" fn frame_rect(rect: Frame) {
    draw_elements(&[Element::Rect(rect)], false);
}

extern "C" fn frame_rect_with_width(rect: Frame, _: f64) {
    frame_rect(rect);
}

extern "C" fn erase_rect(rect: Frame) {
    with_canvas(|canvas, height, origin, _, _| {
        paint::fill(canvas, height, in_window(rect, origin), WHITE);
    });
}

fn attribute(attributes: Id, name: &AtomicUsize) -> Id {
    let key = name.load(Ordering::Relaxed);
    if attributes.is_null() || key == 0 {
        return core::ptr::null_mut();
    }
    send1(attributes, sel!("objectForKey:"), key)
}

struct TextStyle {
    colour: Colour,
    scale: i32,
}

fn style_of(attributes: Id) -> TextStyle {
    TextStyle {
        colour: colour_of(attribute(attributes, &FOREGROUND_COLOUR_NAME))
            .unwrap_or(colour::DEFAULT_TEXT),
        scale: font_scale(attribute(attributes, &FONT_NAME)),
    }
}

extern "C" fn draw_at_point(this: Id, _: Sel, at: Point, attributes: Id) {
    let style = style_of(attributes);
    with_text(this, |text| {
        with_canvas(|canvas, height, origin, _, _| {
            paint::text(
                canvas,
                height,
                point_in_window(at, origin),
                text,
                style.colour,
                style.scale,
            );
        });
    });
}

extern "C" fn draw_in_rect(this: Id, _: Sel, rect: Frame, attributes: Id) {
    let style = style_of(attributes);
    with_text(this, |text| {
        with_canvas(|canvas, height, origin, _, _| {
            let frame = in_window(rect, origin);
            let previous = canvas.narrow(eggos_macos_gui::geometry::pixel_rect(frame, height));
            paint::text(
                canvas,
                height,
                Point::new(frame.x, frame.y),
                text,
                style.colour,
                style.scale,
            );
            canvas.restore(previous);
        });
    });
}

extern "C" fn size_with_attributes(this: Id, _: Sel, attributes: Id) -> Size {
    let scale = font_scale(attribute(attributes, &FONT_NAME));
    let width = with_text(this, |text| paint::text_width(text, scale));
    Size {
        width: f64::from(width),
        height: f64::from(paint::LINE_HEIGHT * scale),
    }
}

/// Makes the attribute names' strings; false if there is no memory.
pub fn set_up() -> bool {
    let pool = autorelease::push();
    let mut made = true;
    for (slot, name) in [
        (&FOREGROUND_COLOUR_NAME, "NSColor"),
        (&FONT_NAME, "NSFont"),
        (&BACKGROUND_COLOUR_NAME, "NSBackgroundColor"),
    ] {
        let object = string(name);
        made &= !object.is_null();
        slot.store(objc_retain(object) as usize, Ordering::Relaxed);
    }
    autorelease::pop(pool);
    made
}

pub fn resolve(name: &[u8]) -> Option<u64> {
    let address = match name {
        b"_NSRectFill" => rect_fill as *const () as usize,
        b"_NSRectFillUsingOperation" => rect_fill_using_operation as *const () as usize,
        b"_NSRectFillList" => rect_fill_list as *const () as usize,
        b"_NSFrameRect" => frame_rect as *const () as usize,
        b"_NSFrameRectWithWidth" => frame_rect_with_width as *const () as usize,
        b"_NSEraseRect" => erase_rect as *const () as usize,
        b"_NSForegroundColorAttributeName" => &raw const FOREGROUND_COLOUR_NAME as usize,
        b"_NSFontAttributeName" => &raw const FONT_NAME as usize,
        b"_NSBackgroundColorAttributeName" => &raw const BACKGROUND_COLOUR_NAME as usize,
        _ => return None,
    };
    Some(address as u64)
}

pub fn define(runtime: &mut Runtime) -> bool {
    let colour_class = methods![
        "blackColor" => named_colour,
        "whiteColor" => named_colour,
        "grayColor" => named_colour,
        "darkGrayColor" => named_colour,
        "lightGrayColor" => named_colour,
        "redColor" => named_colour,
        "greenColor" => named_colour,
        "blueColor" => named_colour,
        "cyanColor" => named_colour,
        "yellowColor" => named_colour,
        "magentaColor" => named_colour,
        "orangeColor" => named_colour,
        "purpleColor" => named_colour,
        "brownColor" => named_colour,
        "clearColor" => named_colour,
        "windowBackgroundColor" => named_colour,
        "controlBackgroundColor" => named_colour,
        "textBackgroundColor" => named_colour,
        "labelColor" => named_colour,
        "textColor" => named_colour,
        "controlTextColor" => named_colour,
        "headerTextColor" => named_colour,
        "selectedTextColor" => named_colour,
        "selectedMenuItemTextColor" => named_colour,
        "alternateSelectedControlTextColor" => named_colour,
        "secondaryLabelColor" => named_colour,
        "tertiaryLabelColor" => named_colour,
        "quaternaryLabelColor" => named_colour,
        "placeholderTextColor" => named_colour,
        "disabledControlTextColor" => named_colour,
        "systemGrayColor" => named_colour,
        "controlColor" => named_colour,
        "selectedControlColor" => named_colour,
        "selectedContentBackgroundColor" => named_colour,
        "selectedTextBackgroundColor" => named_colour,
        "controlAccentColor" => named_colour,
        "keyboardFocusIndicatorColor" => named_colour,
        "systemOrangeColor" => named_colour,
        "separatorColor" => named_colour,
        "gridColor" => named_colour,
        "linkColor" => named_colour,
        "systemBlueColor" => named_colour,
        "systemRedColor" => named_colour,
        "systemGreenColor" => named_colour,
        "systemYellowColor" => named_colour,
        "systemPurpleColor" => named_colour,
        "systemTealColor" => named_colour,
        "colorWithRed:green:blue:alpha:" => colour_with_rgba,
        "colorWithCalibratedRed:green:blue:alpha:" => colour_with_rgba,
        "colorWithDeviceRed:green:blue:alpha:" => colour_with_rgba,
        "colorWithSRGBRed:green:blue:alpha:" => colour_with_rgba,
        "colorWithWhite:alpha:" => colour_with_white,
        "colorWithCalibratedWhite:alpha:" => colour_with_white,
    ];
    let colour_instance = methods![
        "set" => set_colour,
        "setFill" => set_fill_colour,
        "setStroke" => set_stroke_colour,
        "colorWithAlphaComponent:" => with_alpha,
        "alphaComponent" => alpha_component,
        "redComponent" => red_component,
        "greenComponent" => green_component,
        "blueComponent" => blue_component,
        "colorUsingColorSpace:" => this_with,
    ];
    let font_class = methods![
        "systemFontOfSize:" => font_of_size,
        "boldSystemFontOfSize:" => font_of_size,
        "userFontOfSize:" => font_of_size,
        "userFixedPitchFontOfSize:" => font_of_size,
        "labelFontOfSize:" => font_of_size,
        "menuFontOfSize:" => font_of_size,
        "messageFontOfSize:" => font_of_size,
        "systemFontOfSize:weight:" => font_of_size_weight,
        "monospacedSystemFontOfSize:weight:" => font_of_size_weight,
        "monospacedDigitSystemFontOfSize:weight:" => font_of_size_weight,
        "fontWithName:size:" => font_named,
        "systemFontSize" => system_font_size,
        "smallSystemFontSize" => small_font_size,
        "labelFontSize" => label_font_size,
    ];
    let font_instance = methods![
        "pointSize" => point_size,
        "ascender" => ascender,
        "descender" => descender,
        "leading" => leading,
        "xHeight" => x_height,
        "capHeight" => cap_height,
        "fontName" => font_name,
        "familyName" => font_name,
    ];
    let path_class = methods![
        "bezierPath" => path,
        "bezierPathWithRect:" => path_with_rect,
        "bezierPathWithOvalInRect:" => path_with_oval,
        "fillRect:" => fill_rect_class,
        "strokeRect:" => stroke_rect_class,
        "strokeLineFromPoint:toPoint:" => stroke_line_class,
    ];
    let path_instance = methods![
        "moveToPoint:" => move_to,
        "lineToPoint:" => line_to,
        "closePath" => close_path,
        "appendBezierPathWithRect:" => append_rect,
        "appendBezierPathWithOvalInRect:" => append_oval,
        "removeAllPoints" => remove_all_points,
        "setLineWidth:" => set_line_width,
        "lineWidth" => line_width,
        "isEmpty" => is_empty,
        "fill" => fill_path,
        "stroke" => stroke_path,
        "dealloc" => path_dealloc,
    ];
    let string_drawing = methods![
        "drawAtPoint:withAttributes:" => draw_at_point,
        "drawInRect:withAttributes:" => draw_in_rect,
        "sizeWithAttributes:" => size_with_attributes,
    ];
    super::define(
        runtime,
        b"NSColor\0",
        (&NSCOLOR, &NSCOLOR_META),
        &NSOBJECT,
        core::mem::size_of::<ColourFields>(),
        &colour_instance,
        &colour_class,
    ) && super::define(
        runtime,
        b"NSFont\0",
        (&NSFONT, &NSFONT_META),
        &NSOBJECT,
        core::mem::size_of::<FontFields>(),
        &font_instance,
        &font_class,
    ) && super::define(
        runtime,
        b"NSBezierPath\0",
        (&NSBEZIERPATH, &NSBEZIERPATH_META),
        &NSOBJECT,
        core::mem::size_of::<PathFields>(),
        &path_instance,
        &path_class,
    ) && class::extend(runtime, &NSSTRING, &string_drawing, &[])
}
