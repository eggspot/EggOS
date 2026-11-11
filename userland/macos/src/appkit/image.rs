//! NSImage and NSImageView (D374, docs/ux/macos-gui-v0.3.md section 12): an image decoded once
//! by our own PNG reader into premultiplied pixels, found by name in the bundle's resources or
//! read from a file or data, drawn from `drawRect:` unscaled, cropped or scaled by nearest
//! neighbour with a fraction of its alpha; and a view that shows one image in its frame by
//! `imageScaling`, `imageAlignment` and `imageFrameStyle`, keeping the scaled pixels until
//! the image or the drawn size changes.

use super::{
    NSIMAGE, NSIMAGE_META, NSIMAGEVIEW, NSIMAGEVIEW_META, NSVIEW, drawing, store, store_copy,
    string, view, with_text,
};
use crate::foundation::{NSBUNDLE, NSDATA, autoreleased, class_pointer, new_instance};
use crate::libsystem::Exclusive;
use crate::objc::abi::{Id, Sel};
use crate::objc::buffer::Buffer;
use crate::objc::reference_count::{dispose, objc_release, objc_retain};
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, message, methods, sel, send, send1};
use alloc::boxed::Box;
use eggos_macos_gui::geometry::{Frame, Point, Size, pixel_rect, round};
use eggos_macos_gui::image_view::{self as geometry, faded, fits, frame_style, premultiplied};
use eggos_png::inflate::SliceSource;
use eggos_png::rows::{RowDecoder, RowWorkspace};
use eggos_toolkit::colour;
use eggos_toolkit::geometry::Rect;

const EXTENSIONS: [&str; 10] = [
    "", ".png", ".icns", ".jpg", ".jpeg", ".gif", ".bmp", ".ico", ".tif", ".tiff",
];
const LONGEST_PATH: usize = 1024;
const COMPOSITE_COPY: usize = 1;

#[repr(C)]
#[derive(Debug)]
struct ImageFields {
    isa: usize,
    pixels: Buffer<u32>,
    width: u32,
    height: u32,
    name: Id,
    file_name: Id,
}

#[repr(C)]
#[derive(Debug)]
struct ImageViewFields {
    view: view::ViewFields,
    image: Id,
    scaling: usize,
    alignment: usize,
    frame_style: usize,
    cached: Buffer<u32>,
    cached_for: Id,
    cached_size: (i32, i32),
}

static NAMED: Exclusive<Buffer<(Id, Id)>> = Exclusive::new(Buffer::new());

fn image(object: Id) -> &'static mut ImageFields {
    // SAFETY: the callers pass NSImage instances, whose memory starts with these fields; the
    // program's single event thread is the only one that reaches them.
    unsafe { &mut *object.cast::<ImageFields>() }
}

fn image_view(object: Id) -> &'static mut ImageViewFields {
    // SAFETY: the callers pass NSImageView instances, whose memory starts with these fields.
    unsafe { &mut *object.cast::<ImageViewFields>() }
}

fn decode_png(bytes: &[u8], out: &mut Buffer<u32>) -> Option<(u32, u32)> {
    let mut source = SliceSource::new(bytes);
    let decoder = RowDecoder::open(&mut source).ok()?;
    let header = decoder.header();
    if !fits(header.width, header.height) {
        return None;
    }
    let count = header.width as usize * header.height as usize;
    if !out.reserve(count) {
        return None;
    }
    let mut work = Box::new(RowWorkspace::new());
    let mut complete = true;
    decoder
        .read_rows(&mut work, |_, row| {
            for &pixel in row {
                complete &= out.push(premultiplied(pixel));
            }
        })
        .ok()?;
    (complete && out.len() == count).then_some((header.width, header.height))
}

fn load(this: Id, bytes: &[u8]) -> Id {
    let fields = image(this);
    match decode_png(bytes, &mut fields.pixels) {
        Some((width, height)) => {
            fields.width = width;
            fields.height = height;
            this
        }
        None => {
            objc_release(this);
            core::ptr::null_mut()
        }
    }
}

fn with_data_bytes<R>(data: Id, work: impl FnOnce(&[u8]) -> R) -> Option<R> {
    if data.is_null() {
        return None;
    }
    let bytes = send(data, sel!("bytes")) as *const u8;
    let length = send(data, sel!("length")) as usize;
    if bytes.is_null() || length == 0 {
        return None;
    }
    // SAFETY: NSData's `bytes` points at its `length` bytes, which live as long as `data`,
    // held by the caller for the call.
    Some(work(unsafe { core::slice::from_raw_parts(bytes, length) }))
}

fn file_data(path: Id) -> Id {
    send1(
        class_pointer(&NSDATA).cast(),
        sel!("dataWithContentsOfFile:"),
        path as usize,
    )
}

extern "C" fn init_with_contents_of_file(this: Id, _: Sel, path: Id) -> Id {
    if this.is_null() {
        return this;
    }
    let loaded = with_data_bytes(file_data(path), |bytes| load(this, bytes));
    let Some(made) = loaded.filter(|made| !made.is_null()) else {
        if loaded.is_none() {
            objc_release(this);
        }
        return core::ptr::null_mut();
    };
    store_copy(&mut image(made).file_name, path);
    made
}

extern "C" fn init_with_contents_of_url(this: Id, _: Sel, url: Id) -> Id {
    let path = match url.is_null() {
        true => url,
        false => send(url, sel!("path")),
    };
    init_with_contents_of_file(this, sel!("initWithContentsOfFile:"), path)
}

extern "C" fn init_with_data(this: Id, _: Sel, data: Id) -> Id {
    if this.is_null() {
        return this;
    }
    match with_data_bytes(data, |bytes| load(this, bytes)) {
        Some(made) => made,
        None => {
            objc_release(this);
            core::ptr::null_mut()
        }
    }
}

extern "C" fn init_with_size(this: Id, _: Sel, size: Size) -> Id {
    if this.is_null() {
        return this;
    }
    let (width, height) = (round(size.width).max(0) as u32, round(size.height).max(0) as u32);
    if !fits(width, height) {
        return this;
    }
    let fields = image(this);
    let count = width as usize * height as usize;
    if fields.pixels.reserve(count) {
        for _ in 0..count {
            fields.pixels.push(0);
        }
        fields.width = width;
        fields.height = height;
    }
    this
}

extern "C" fn dealloc(this: Id, _: Sel) {
    let fields = image(this);
    fields.pixels.free();
    store(&mut fields.name, core::ptr::null_mut());
    store(&mut fields.file_name, core::ptr::null_mut());
    dispose(this);
}

fn same_text(first: Id, second: Id) -> bool {
    with_text(first, |first| with_text(second, |second| first == second))
}

fn registered(name: Id) -> Id {
    NAMED.with(|named| {
        named
            .as_slice()
            .iter()
            .find(|(each, _)| same_text(*each, name))
            .map_or(core::ptr::null_mut(), |&(_, image)| image)
    })
}

fn register(name: Id, made: Id) {
    let kept = objc_retain(send(name, sel!("copy")));
    objc_retain(made);
    NAMED.with(|named| {
        named.push((kept, made));
    });
}

fn resource_path() -> Id {
    let bundle = send(class_pointer(&NSBUNDLE).cast(), sel!("mainBundle"));
    match bundle.is_null() {
        true => bundle,
        false => send(bundle, sel!("resourcePath")),
    }
}

fn image_from_resources(name: Id) -> Id {
    let folder = resource_path();
    let mut path = [0u8; LONGEST_PATH];
    with_text(folder, |folder| {
        with_text(name, |name| {
            for extension in EXTENSIONS {
                let parts = [folder, "/", name, extension];
                let length: usize = parts.iter().map(|part| part.len()).sum();
                if length > LONGEST_PATH || name.is_empty() {
                    continue;
                }
                let mut at = 0;
                for part in parts {
                    path[at..at + part.len()].copy_from_slice(part.as_bytes());
                    at += part.len();
                }
                let Ok(text) = core::str::from_utf8(&path[..length]) else {
                    continue;
                };
                let object = new_instance(class_pointer(&NSIMAGE));
                if object.is_null() {
                    return core::ptr::null_mut();
                }
                let made = init_with_contents_of_file(object, sel!("initWithContentsOfFile:"), string(text));
                if !made.is_null() {
                    return made;
                }
            }
            core::ptr::null_mut()
        })
    })
}

extern "C" fn image_named(_: Id, _: Sel, name: Id) -> Id {
    if name.is_null() {
        return name;
    }
    let found = registered(name);
    if !found.is_null() {
        return found;
    }
    let made = image_from_resources(name);
    if made.is_null() {
        return made;
    }
    store_copy(&mut image(made).name, name);
    register(name, made);
    autoreleased(made)
}

extern "C" fn size(this: Id, _: Sel) -> Size {
    let fields = image(this);
    Size {
        width: f64::from(fields.width),
        height: f64::from(fields.height),
    }
}

extern "C" fn ignore_size(_: Id, _: Sel, _: Size) {}

extern "C" fn is_valid(this: Id, _: Sel) -> u8 {
    u8::from(image(this).width > 0)
}

extern "C" fn name(this: Id, _: Sel) -> Id {
    image(this).name
}

extern "C" fn set_name(this: Id, _: Sel, name: Id) -> u8 {
    if name.is_null() || !registered(name).is_null() {
        return 0;
    }
    store_copy(&mut image(this).name, name);
    register(name, this);
    1
}

fn crop_of(this: Id, from: Frame) -> Rect {
    let fields = image(this);
    let (width, height) = (fields.width as i32, fields.height as i32);
    if from.width <= 0.0 || from.height <= 0.0 {
        return Rect::new(0, 0, width, height);
    }
    let crop = pixel_rect(from, height);
    crop.intersection(&Rect::new(0, 0, width, height))
}

fn draw_scaled(this: Id, rect: Frame, from: Frame, operation: usize, fraction: f64) {
    let crop = crop_of(this, from);
    if crop.is_empty() {
        return;
    }
    let size = (round(rect.width), round(rect.height));
    let fields = image(this);
    let pixels = fields.pixels.as_slice();
    let width = fields.width as usize;
    let mut drawn = match size == (crop.width, crop.height) && crop.x == 0 && crop.y == 0 && crop.width as usize == width {
        true => {
            let rows = crop.height as usize;
            pixels[..rows * width].to_vec()
        }
        false => geometry::scaled(pixels, width, crop, size),
    };
    if drawn.is_empty() {
        return;
    }
    if fraction < 1.0 {
        for pixel in drawn.iter_mut() {
            *pixel = faded(*pixel, fraction);
        }
    }
    let copy = operation == COMPOSITE_COPY;
    drawing::with_canvas(|canvas, height, origin, _, _| {
        let at = pixel_rect(rect.offset(origin.x, origin.y), height);
        canvas.composite((at.x, at.y), &drawn, size.0.max(1) as usize, copy);
    });
}

extern "C" fn draw_at_point(this: Id, _: Sel, point: Point, from: Frame, operation: usize, fraction: f64) {
    let crop = crop_of(this, from);
    let rect = Frame::new(point.x, point.y, f64::from(crop.width), f64::from(crop.height));
    draw_scaled(this, rect, from, operation, fraction);
}

extern "C" fn draw_in_rect_from(this: Id, _: Sel, rect: Frame, from: Frame, operation: usize, fraction: f64) {
    draw_scaled(this, rect, from, operation, fraction);
}

extern "C" fn draw_in_rect(this: Id, _: Sel, rect: Frame) {
    draw_scaled(this, rect, Frame::default(), 0, 1.0);
}

/// The size of image `object`, or `None` for nil or an image with no pixels.
pub fn size_of(object: Id) -> Option<(u32, u32)> {
    if object.is_null() {
        return None;
    }
    let fields = image(object);
    (fields.width > 0).then_some((fields.width, fields.height))
}

/// What agents call image `object`: its name, else its file's name without folder or
/// extension, else nothing.
pub fn with_image_name<R>(object: Id, work: impl FnOnce(&str) -> R) -> R {
    if object.is_null() {
        return work("");
    }
    let fields = image(object);
    if !fields.name.is_null() {
        return with_text(fields.name, work);
    }
    with_text(fields.file_name, |path| {
        let file = path.rsplit('/').next().unwrap_or(path);
        let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
        work(stem)
    })
}

/// The image image view `object` shows, nil for none.
pub fn image_of_view(object: Id) -> Id {
    image_view(object).image
}

extern "C" fn view_init_with_frame(this: Id, _: Sel, frame: Frame) -> Id {
    if this.is_null() {
        return this;
    }
    view::fields(this).frame = frame;
    this
}

extern "C" fn image_view_with_image(class: Id, _: Sel, shown: Id) -> Id {
    let object = send(class, sel!("alloc"));
    if object.is_null() {
        return object;
    }
    let size = size_of(shown).unwrap_or((0, 0));
    let frame = Frame::new(0.0, 0.0, f64::from(size.0), f64::from(size.1));
    let init: extern "C" fn(Id, Sel, Frame) -> Id = message::msg_send_as();
    let made = init(object, sel!("initWithFrame:"), frame);
    if !made.is_null() {
        store(&mut image_view(made).image, shown);
    }
    autoreleased(made)
}

extern "C" fn view_dealloc(this: Id, _: Sel) {
    let fields = image_view(this);
    store(&mut fields.image, core::ptr::null_mut());
    fields.cached.free();
    view::dispose(this);
}

fn view_changed(this: Id) {
    send1(this, sel!("setNeedsDisplay:"), 1);
}

extern "C" fn view_image(this: Id, _: Sel) -> Id {
    image_view(this).image
}

extern "C" fn view_set_image(this: Id, _: Sel, shown: Id) {
    store(&mut image_view(this).image, shown);
    view_changed(this);
}

macro_rules! word_accessors {
    ($get:ident, $set:ident, $field:ident) => {
        extern "C" fn $get(this: Id, _: Sel) -> usize {
            image_view(this).$field
        }

        extern "C" fn $set(this: Id, _: Sel, value: usize) {
            image_view(this).$field = value;
            view_changed(this);
        }
    };
}

word_accessors!(view_scaling, view_set_scaling, scaling);
word_accessors!(view_alignment, view_set_alignment, alignment);
word_accessors!(view_frame_style, view_set_frame_style, frame_style);

extern "C" fn ignore_word(_: Id, _: Sel, _: usize) {}

fn cached_pixels(this: Id, size: (i32, i32)) -> bool {
    let fields = image_view(this);
    if fields.cached_for == fields.image && fields.cached_size == size && fields.cached.len() > 0 {
        return true;
    }
    let source = image(fields.image);
    let crop = Rect::new(0, 0, source.width as i32, source.height as i32);
    let scaled = geometry::scaled(source.pixels.as_slice(), source.width as usize, crop, size);
    fields.cached.free();
    fields.cached = Buffer::new();
    if !fields.cached.reserve(scaled.len()) {
        return false;
    }
    for pixel in scaled {
        fields.cached.push(pixel);
    }
    fields.cached_for = fields.image;
    fields.cached_size = size;
    true
}

extern "C" fn view_draw_rect(this: Id, _: Sel, _: Frame) {
    let frame = view::fields(this).frame;
    let (style, scaling_mode, alignment_mode) = {
        let fields = image_view(this);
        (fields.frame_style, fields.scaling, fields.alignment)
    };
    let shown = image_view(this).image;
    let mut placed = None;
    drawing::with_canvas(|canvas, height, origin, _, _| {
        let outer = pixel_rect(Frame::new(origin.x, origin.y, frame.width, frame.height), height);
        if style != frame_style::NONE {
            let line = match geometry::is_filled(style) {
                true => colour::BUTTON_BORDER,
                false => colour::MENU_BORDER,
            };
            canvas.fill(outer, line);
            let inside = outer.inset(1);
            match geometry::is_filled(style) {
                true => canvas.fill(inside, colour::BUTTON),
                false => canvas.fill(inside, colour::WINDOW_BACKGROUND),
            }
        }
        let area = geometry::image_area(outer, style);
        if let Some(own) = size_of(shown) {
            let size = geometry::drawn_size(
                (own.0 as i32, own.1 as i32),
                (area.width, area.height),
                scaling_mode,
            );
            placed = Some((area, size, geometry::place(area, size, alignment_mode)));
        }
    });
    let Some((area, size, at)) = placed else {
        return;
    };
    if size.0 <= 0 || size.1 <= 0 || !cached_pixels(this, size) {
        return;
    }
    let pixels = image_view(this).cached.as_slice();
    drawing::with_canvas(|canvas, _, _, _, _| {
        let kept = canvas.narrow(area);
        canvas.composite(at, pixels, size.0 as usize, false);
        canvas.restore(kept);
    });
}

pub fn define(runtime: &mut Runtime) -> bool {
    let image_methods = methods![
        "initWithContentsOfFile:" => init_with_contents_of_file,
        "initByReferencingFile:" => init_with_contents_of_file,
        "initWithContentsOfURL:" => init_with_contents_of_url,
        "initByReferencingURL:" => init_with_contents_of_url,
        "initWithData:" => init_with_data,
        "initWithSize:" => init_with_size,
        "dealloc" => dealloc,
        "size" => size,
        "setSize:" => ignore_size,
        "isValid" => is_valid,
        "name" => name,
        "setName:" => set_name,
        "drawAtPoint:fromRect:operation:fraction:" => draw_at_point,
        "drawInRect:fromRect:operation:fraction:" => draw_in_rect_from,
        "drawInRect:" => draw_in_rect,
        "setTemplate:" => ignore_word,
    ];
    let image_class_methods = methods![
        "imageNamed:" => image_named,
    ];
    let view_methods = methods![
        "initWithFrame:" => view_init_with_frame,
        "dealloc" => view_dealloc,
        "image" => view_image,
        "setImage:" => view_set_image,
        "imageScaling" => view_scaling,
        "setImageScaling:" => view_set_scaling,
        "imageAlignment" => view_alignment,
        "setImageAlignment:" => view_set_alignment,
        "imageFrameStyle" => view_frame_style,
        "setImageFrameStyle:" => view_set_frame_style,
        "setEditable:" => ignore_word,
        "setAnimates:" => ignore_word,
        "setAllowsCutCopyPaste:" => ignore_word,
        "setContentTintColor:" => ignore_word,
        "drawRect:" => view_draw_rect,
    ];
    let view_class_methods = methods![
        "imageViewWithImage:" => image_view_with_image,
    ];
    super::define(
        runtime,
        b"NSImage\0",
        (&NSIMAGE, &NSIMAGE_META),
        &NSOBJECT,
        core::mem::size_of::<ImageFields>(),
        &image_methods,
        &image_class_methods,
    ) && super::define(
        runtime,
        b"NSImageView\0",
        (&NSIMAGEVIEW, &NSIMAGEVIEW_META),
        &NSVIEW,
        core::mem::size_of::<ImageViewFields>(),
        &view_methods,
        &view_class_methods,
    )
}
