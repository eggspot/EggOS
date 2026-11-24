//! NSAttributedString and NSMutableAttributedString (D384, D730): a string and the attribute
//! dictionaries of its runs, which cover it end to end in UTF-16 units, and AppKit's drawing
//! of it: each paragraph (up to a line feed) on its own line, placed by the paragraph style of
//! its first character, each run drawn with its own font and colour after the one before, and
//! underlined or struck through when its attributes say so. RTF documents are read into one
//! (D731) through `eggos-macos-gui`'s reader: each style's run gets its font, colour,
//! underline and strikethrough, each aligned paragraph a paragraph style.

use super::paragraph_style::{self, Layout, alignment};
use super::{
    NSATTRIBUTEDSTRING, NSATTRIBUTEDSTRING_META, NSMUTABLEATTRIBUTEDSTRING,
    NSMUTABLEATTRIBUTEDSTRING_META, drawing, store, string,
};
use crate::objc::block::invoke_address;
use eggos_macos_gui::geometry::pixel_rect;
use eggos_macos_gui::rtf::{self, Alignment, Sink, Style};
use crate::foundation::{NSDICTIONARY, autoreleased, class_pointer};
use crate::objc::abi::{Id, Sel};
use crate::objc::buffer::Buffer;
use crate::objc::reference_count::{dispose, objc_release, objc_retain};
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, message, methods, sel, send, send1, send2};
use eggos_macos_gui::geometry::{Frame, Point, Size};

const REVERSE: usize = 1 << 1;
const LINE_FEED: usize = 0x0a;
const PARAGRAPH_SEPARATOR: usize = 0x2029;
const GLYPH_HEIGHT: f64 = 16.0;
const UNDERLINE_ABOVE_BOTTOM: f64 = 2.0;
const STRIKE_ABOVE_BOTTOM: f64 = 8.0;
const UNDERLINE_THICK: isize = 0x02;
const UNDERLINE_DOUBLE: isize = 0x09;
const DOUBLE_GAP: f64 = 2.0;
const UTF8_ENCODING: usize = 4;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Range {
    location: usize,
    length: usize,
}

impl Range {
    fn end(&self) -> usize {
        self.location + self.length
    }
}

#[derive(Debug, Clone, Copy)]
struct Run {
    start: usize,
    attributes: Id,
}

#[repr(C)]
#[derive(Debug)]
struct Fields {
    isa: usize,
    string: Id,
    runs: Buffer<Run>,
}

fn fields(object: Id) -> &'static mut Fields {
    // SAFETY: the callers pass NSAttributedString instances, whose memory starts with these
    // fields; the program's single event thread is the only one that reaches them.
    unsafe { &mut *object.cast::<Fields>() }
}

fn length_of(this: Id) -> usize {
    send(fields(this).string, sel!("length")) as usize
}

fn empty_attributes() -> Id {
    send(class_pointer(&NSDICTIONARY).cast(), sel!("dictionary"))
}

fn copied_attributes(attributes: Id) -> Id {
    let source = match attributes.is_null() {
        true => empty_attributes(),
        false => attributes,
    };
    send(source, sel!("copy"))
}

fn run_end(this: Id, index: usize) -> usize {
    let runs = fields(this).runs.as_slice();
    runs.get(index + 1)
        .map_or(length_of(this), |next| next.start)
}

fn run_at(this: Id, at: usize) -> Option<usize> {
    let runs = fields(this).runs.as_slice();
    if runs.is_empty() {
        return None;
    }
    let found = runs.iter().rposition(|run| run.start <= at).unwrap_or(0);
    Some(found)
}

fn release_runs(this: Id) {
    let mut runs = core::mem::replace(&mut fields(this).runs, Buffer::new());
    for run in runs.as_slice() {
        objc_release(run.attributes);
    }
    runs.free();
}

fn set_up(this: Id, text: Id, attributes: Id) -> Id {
    if this.is_null() {
        return this;
    }
    let copy = match text.is_null() {
        true => string(""),
        false => text,
    };
    let mutable = send(copy, sel!("mutableCopy"));
    fields(this).string = mutable;
    let attributes = copied_attributes(attributes);
    fields(this).runs.push(Run {
        start: 0,
        attributes,
    });
    this
}

extern "C" fn init_with_string(this: Id, _: Sel, text: Id) -> Id {
    set_up(this, text, core::ptr::null_mut())
}

extern "C" fn init_with_string_attributes(this: Id, _: Sel, text: Id, attributes: Id) -> Id {
    set_up(this, text, attributes)
}

extern "C" fn init(this: Id, _: Sel) -> Id {
    set_up(this, core::ptr::null_mut(), core::ptr::null_mut())
}

fn copy_runs(from: Id, to: Id) {
    release_runs(to);
    for run in fields(from).runs.as_slice() {
        fields(to).runs.push(Run {
            start: run.start,
            attributes: objc_retain(run.attributes),
        });
    }
}

extern "C" fn init_with_attributed_string(this: Id, _: Sel, other: Id) -> Id {
    if this.is_null() || other.is_null() {
        return init(this, sel!("init"));
    }
    let made = set_up(this, fields(other).string, core::ptr::null_mut());
    if !made.is_null() {
        copy_runs(other, made);
    }
    made
}

extern "C" fn dealloc(this: Id, _: Sel) {
    release_runs(this);
    store(&mut fields(this).string, core::ptr::null_mut());
    dispose(this);
}

extern "C" fn string_method(this: Id, _: Sel) -> Id {
    fields(this).string
}

extern "C" fn length(this: Id, _: Sel) -> usize {
    length_of(this)
}

fn write_range(out: *mut Range, range: Range) {
    if !out.is_null() {
        // SAFETY: the caller's `NSRange` to fill, or null.
        unsafe { out.write_unaligned(range) };
    }
}

extern "C" fn attributes_at_index(this: Id, _: Sel, at: usize, effective: *mut Range) -> Id {
    let Some(index) = run_at(this, at).filter(|_| at < length_of(this).max(1)) else {
        write_range(
            effective,
            Range {
                location: 0,
                length: 0,
            },
        );
        return empty_attributes();
    };
    let run = fields(this).runs.as_slice()[index];
    let end = run_end(this, index);
    write_range(
        effective,
        Range {
            location: run.start,
            length: end - run.start,
        },
    );
    run.attributes
}

extern "C" fn attribute_at_index(
    this: Id,
    _: Sel,
    name: Id,
    at: usize,
    effective: *mut Range,
) -> Id {
    let attributes = attributes_at_index(
        this,
        sel!("attributesAtIndex:effectiveRange:"),
        at,
        effective,
    );
    send1(attributes, sel!("objectForKey:"), name as usize)
}

fn clipped(range: Range, limit: Range) -> Range {
    let start = range.location.max(limit.location);
    let end = range.end().min(limit.end()).max(start);
    Range {
        location: start,
        length: end - start,
    }
}

fn run_range(this: Id, index: usize) -> Range {
    let start = fields(this).runs.as_slice()[index].start;
    Range {
        location: start,
        length: run_end(this, index).saturating_sub(start),
    }
}

fn same(first: Id, second: Id) -> bool {
    first == second
        || (!first.is_null() && crate::objc::send_bool(first, sel!("isEqual:"), second as usize))
}

fn longest(this: Id, at: usize, limit: Range, value: &dyn Fn(Id) -> Id) -> Range {
    let Some(index) = run_at(this, at).filter(|_| at < length_of(this)) else {
        return Range {
            location: at,
            length: 0,
        };
    };
    let runs = fields(this).runs.as_slice();
    let wanted = value(runs[index].attributes);
    let mut first = index;
    while first > 0
        && run_range(this, first).location > limit.location
        && same(value(runs[first - 1].attributes), wanted)
    {
        first -= 1;
    }
    let mut last = index;
    while last + 1 < runs.len()
        && run_range(this, last).end() < limit.end()
        && same(value(runs[last + 1].attributes), wanted)
    {
        last += 1;
    }
    let whole = Range {
        location: run_range(this, first).location,
        length: run_range(this, last).end() - run_range(this, first).location,
    };
    clipped(whole, limit)
}

extern "C" fn attributes_longest(
    this: Id,
    _: Sel,
    at: usize,
    effective: *mut Range,
    limit: Range,
) -> Id {
    let asking = sel!("attributesAtIndex:effectiveRange:");
    let attributes = attributes_at_index(this, asking, at, core::ptr::null_mut());
    write_range(effective, longest(this, at, limit, &|attributes| attributes));
    attributes
}

extern "C" fn attribute_longest(
    this: Id,
    _: Sel,
    name: Id,
    at: usize,
    effective: *mut Range,
    limit: Range,
) -> Id {
    let value_of = |attributes: Id| send1(attributes, sel!("objectForKey:"), name as usize);
    let asking = sel!("attributesAtIndex:effectiveRange:");
    let attributes = attributes_at_index(this, asking, at, core::ptr::null_mut());
    write_range(effective, longest(this, at, limit, &value_of));
    value_of(attributes)
}

fn each_run_in(this: Id, limit: Range, reverse: bool, work: &mut dyn FnMut(Id, Range) -> bool) {
    let count = fields(this).runs.len();
    for step in 0..count {
        let index = match reverse {
            true => count - 1 - step,
            false => step,
        };
        let range = clipped(run_range(this, index), limit);
        if range.length == 0 {
            continue;
        }
        let attributes = fields(this).runs.as_slice()[index].attributes;
        if !work(attributes, range) {
            return;
        }
    }
}

extern "C" fn enumerate_attributes(this: Id, _: Sel, limit: Range, options: usize, block: Id) {
    if block.is_null() {
        return;
    }
    // SAFETY: the program's block takes the block, the attributes, an `NSRange` and a `BOOL *`.
    let function: extern "C" fn(Id, Id, Range, *mut u8) =
        unsafe { core::mem::transmute(invoke_address(block)) };
    let mut stop = 0u8;
    each_run_in(this, limit, options & REVERSE != 0, &mut |attributes, range| {
        function(block, attributes, range, &mut stop);
        stop == 0
    });
}

extern "C" fn enumerate_attribute(
    this: Id,
    _: Sel,
    name: Id,
    limit: Range,
    options: usize,
    block: Id,
) {
    if block.is_null() {
        return;
    }
    // SAFETY: the program's block takes the block, the value, an `NSRange` and a `BOOL *`.
    let function: extern "C" fn(Id, Id, Range, *mut u8) =
        unsafe { core::mem::transmute(invoke_address(block)) };
    let reverse = options & REVERSE != 0;
    let mut pending: Option<(Id, Range)> = None;
    let mut stop = 0u8;
    each_run_in(this, limit, reverse, &mut |attributes, range| {
        let value = send1(attributes, sel!("objectForKey:"), name as usize);
        match pending {
            Some((held, held_range)) if same(held, value) => {
                let start = held_range.location.min(range.location);
                let end = held_range.end().max(range.end());
                pending = Some((
                    held,
                    Range {
                        location: start,
                        length: end - start,
                    },
                ));
                true
            }
            Some((held, held_range)) => {
                function(block, held, held_range, &mut stop);
                pending = Some((value, range));
                stop == 0
            }
            None => {
                pending = Some((value, range));
                true
            }
        }
    });
    if stop == 0
        && let Some((held, held_range)) = pending
    {
        function(block, held, held_range, &mut stop);
    }
}

extern "C" fn copy_with_zone(this: Id, _: Sel, _: usize) -> Id {
    objc_retain(this)
}

extern "C" fn immutable_copy_with_zone(this: Id, _: Sel, _: usize) -> Id {
    let made = send(class_pointer(&NSATTRIBUTEDSTRING).cast(), sel!("alloc"));
    let init: extern "C" fn(Id, Sel, Id) -> Id = message::msg_send_as();
    init(made, sel!("initWithAttributedString:"), this)
}

extern "C" fn mutable_copy_with_zone(this: Id, _: Sel, _: usize) -> Id {
    let made = send(
        class_pointer(&NSMUTABLEATTRIBUTEDSTRING).cast(),
        sel!("alloc"),
    );
    let init: extern "C" fn(Id, Sel, Id) -> Id = message::msg_send_as();
    init(made, sel!("initWithAttributedString:"), this)
}

extern "C" fn is_equal(this: Id, _: Sel, other: Id) -> u8 {
    if this == other {
        return 1;
    }
    if other.is_null() || !super::is(other, &NSATTRIBUTEDSTRING) {
        return 0;
    }
    let same_text = send1(
        fields(this).string,
        sel!("isEqualToString:"),
        fields(other).string as usize,
    ) as usize
        & 0xff
        != 0;
    let (mine, theirs) = (fields(this).runs.as_slice(), fields(other).runs.as_slice());
    let same_runs = mine.len() == theirs.len()
        && mine.iter().zip(theirs).all(|(a, b)| {
            a.start == b.start
                && send1(
                    a.attributes,
                    sel!("isEqualToDictionary:"),
                    b.attributes as usize,
                ) as usize
                    & 0xff
                    != 0
        });
    u8::from(same_text && same_runs)
}

fn split_at(this: Id, at: usize) {
    if at == 0 || at >= length_of(this) {
        return;
    }
    let Some(index) = run_at(this, at) else {
        return;
    };
    let run = fields(this).runs.as_slice()[index];
    if run.start == at {
        return;
    }
    let mut runs = core::mem::replace(&mut fields(this).runs, Buffer::new());
    for (position, existing) in runs.as_slice().iter().enumerate() {
        fields(this).runs.push(*existing);
        if position == index {
            fields(this).runs.push(Run {
                start: at,
                attributes: objc_retain(run.attributes),
            });
        }
    }
    runs.free();
}

fn edit_runs(this: Id, range: Range, edit: &mut dyn FnMut(Id) -> Id) {
    let end = range.end().min(length_of(this));
    if range.location >= end {
        return;
    }
    split_at(this, range.location);
    split_at(this, end);
    let count = fields(this).runs.len();
    for index in 0..count {
        let run = fields(this).runs.as_slice()[index];
        if run.start < range.location || run.start >= end {
            continue;
        }
        let replaced = edit(run.attributes);
        objc_release(run.attributes);
        fields(this).runs.as_mut_slice()[index].attributes = replaced;
    }
}

fn mutable_attributes(attributes: Id) -> Id {
    send(attributes, sel!("mutableCopy"))
}

extern "C" fn set_attributes(this: Id, _: Sel, attributes: Id, range: Range) {
    edit_runs(this, range, &mut |_| copied_attributes(attributes));
}

extern "C" fn add_attribute(this: Id, _: Sel, name: Id, value: Id, range: Range) {
    if name.is_null() || value.is_null() {
        return;
    }
    edit_runs(this, range, &mut |old| {
        let changed = mutable_attributes(old);
        send2(
            changed,
            sel!("setObject:forKey:"),
            value as usize,
            name as usize,
        );
        changed
    });
}

extern "C" fn add_attributes(this: Id, _: Sel, attributes: Id, range: Range) {
    if attributes.is_null() {
        return;
    }
    edit_runs(this, range, &mut |old| {
        let changed = mutable_attributes(old);
        send1(
            changed,
            sel!("addEntriesFromDictionary:"),
            attributes as usize,
        );
        changed
    });
}

extern "C" fn remove_attribute(this: Id, _: Sel, name: Id, range: Range) {
    edit_runs(this, range, &mut |old| {
        let changed = mutable_attributes(old);
        send1(changed, sel!("removeObjectForKey:"), name as usize);
        changed
    });
}

extern "C" fn replace_characters(this: Id, _: Sel, range: Range, text: Id) {
    let total = length_of(this);
    let range = Range {
        location: range.location.min(total),
        length: range.length.min(total - range.location.min(total)),
    };
    let added = match text.is_null() {
        true => 0,
        false => send(text, sel!("length")) as usize,
    };
    let replacing: extern "C" fn(Id, Sel, Range, Id) = message::msg_send_as();
    replacing(
        fields(this).string,
        sel!("replaceCharactersInRange:withString:"),
        range,
        if text.is_null() { string("") } else { text },
    );
    let shift = added as isize - range.length as isize;
    let mut kept: Buffer<Run> = Buffer::new();
    let mut runs = core::mem::replace(&mut fields(this).runs, Buffer::new());
    for run in runs.as_slice() {
        let start = if run.start <= range.location {
            run.start
        } else if run.start >= range.end() {
            (run.start as isize + shift) as usize
        } else {
            objc_release(run.attributes);
            continue;
        };
        let duplicate = kept
            .as_slice()
            .last()
            .is_some_and(|last: &Run| last.start == start);
        if duplicate && start != 0 {
            let last = kept.len() - 1;
            objc_release(kept.as_slice()[last].attributes);
            kept.as_mut_slice()[last].attributes = run.attributes;
            continue;
        }
        kept.push(Run {
            start,
            attributes: run.attributes,
        });
    }
    runs.free();
    if kept.len() == 0 {
        kept.push(Run {
            start: 0,
            attributes: copied_attributes(core::ptr::null_mut()),
        });
    }
    fields(this).runs = kept;
}

extern "C" fn append_attributed_string(this: Id, _: Sel, other: Id) {
    if other.is_null() {
        return;
    }
    let start = length_of(this);
    send1(
        fields(this).string,
        sel!("appendString:"),
        fields(other).string as usize,
    );
    let other_length = send(fields(other).string, sel!("length")) as usize;
    if other_length == 0 {
        return;
    }
    let first_empty = start == 0;
    if first_empty {
        release_runs(this);
    }
    for run in fields(other).runs.as_slice() {
        fields(this).runs.push(Run {
            start: start + run.start,
            attributes: objc_retain(run.attributes),
        });
    }
}

extern "C" fn set_attributed_string(this: Id, _: Sel, other: Id) {
    let replacing: extern "C" fn(Id, Sel, Range, Id) = message::msg_send_as();
    let text = match other.is_null() {
        true => string(""),
        false => fields(other).string,
    };
    replacing(
        fields(this).string,
        sel!("replaceCharactersInRange:withString:"),
        Range {
            location: 0,
            length: length_of(this),
        },
        text,
    );
    match other.is_null() {
        true => {
            release_runs(this);
            fields(this).runs.push(Run {
                start: 0,
                attributes: copied_attributes(core::ptr::null_mut()),
            });
        }
        false => copy_runs(other, this),
    }
}

fn size_of_piece(text: Id, attributes: Id) -> Size {
    let measure: extern "C" fn(Id, Sel, Id) -> Size = message::msg_send_as();
    measure(text, sel!("sizeWithAttributes:"), attributes)
}

fn attribute(attributes: Id, name: &str) -> Id {
    send1(attributes, sel!("objectForKey:"), string(name) as usize)
}

fn integer_attribute(attributes: Id, name: &str) -> isize {
    let value = attribute(attributes, name);
    match value.is_null() {
        true => 0,
        false => send(value, sel!("integerValue")) as isize,
    }
}

fn line_colour(attributes: Id, own: &str) -> u32 {
    let colour = [own, "NSColor"]
        .iter()
        .find_map(|name| drawing::colour_of(attribute(attributes, name)));
    colour.map_or(0, |colour| colour.pixel)
}

fn decorate(frame: Frame, attributes: Id) {
    let underline = match integer_attribute(attributes, "NSUnderline") {
        0 if !attribute(attributes, "NSLink").is_null() => 1,
        style => style,
    };
    let strike = integer_attribute(attributes, "NSStrikethrough");
    if underline == 0 && strike == 0 {
        return;
    }
    let scale = (frame.height / GLYPH_HEIGHT).max(1.0);
    let mut lines: [(f64, f64, u32); 4] = [(0.0, 0.0, 0); 4];
    let mut count = 0;
    let mut add = |above: f64, style: isize, colour: u32| {
        let thick = match style & UNDERLINE_THICK != 0 {
            true => 2.0,
            false => 1.0,
        };
        lines[count] = (frame.y + above * scale, thick * scale, colour);
        count += 1;
        if style & UNDERLINE_DOUBLE == UNDERLINE_DOUBLE {
            lines[count] = (frame.y + (above - DOUBLE_GAP) * scale, thick * scale, colour);
            count += 1;
        }
    };
    if underline != 0 {
        add(UNDERLINE_ABOVE_BOTTOM, underline, line_colour(attributes, "NSUnderlineColor"));
    }
    if strike != 0 {
        add(STRIKE_ABOVE_BOTTOM, strike, line_colour(attributes, "NSStrikethroughColor"));
    }
    drawing::with_canvas(|canvas, height, origin, _, _| {
        for &(y, thickness, colour) in &lines[..count] {
            let line = Frame::new(origin.x + frame.x, origin.y + y, frame.width, thickness);
            canvas.fill(pixel_rect(line, height), colour);
        }
    });
}

fn draw_piece(text: Id, attributes: Id, at: Point) -> f64 {
    let draw: extern "C" fn(Id, Sel, Point, Id) = message::msg_send_as();
    draw(text, sel!("drawAtPoint:withAttributes:"), at, attributes);
    let size = size_of_piece(text, attributes);
    decorate(Frame::new(at.x, at.y, size.width, size.height), attributes);
    size.width
}

fn character_at(this: Id, at: usize) -> usize {
    send1(fields(this).string, sel!("characterAtIndex:"), at) as usize & 0xffff
}

#[derive(Debug, Clone, Copy)]
struct Line {
    range: Range,
    width: f64,
    height: f64,
    layout: Layout,
}

fn piece_text(this: Id, range: Range) -> Id {
    let piece: extern "C" fn(Id, Sel, Range) -> Id = message::msg_send_as();
    piece(fields(this).string, sel!("substringWithRange:"), range)
}

fn measure_line(this: Id, range: Range) -> Line {
    let layout = match run_at(this, range.location) {
        Some(index) if range.location < length_of(this) => paragraph_style::layout_of(attribute(
            fields(this).runs.as_slice()[index].attributes,
            "NSParagraphStyle",
        )),
        _ => paragraph_style::layout_of(core::ptr::null_mut()),
    };
    let mut width = 0.0;
    let mut height: f64 = GLYPH_HEIGHT;
    each_run_in(this, range, false, &mut |attributes, piece| {
        let size = size_of_piece(piece_text(this, piece), attributes);
        width += size.width;
        height = height.max(size.height);
        true
    });
    if layout.line_height_multiple > 0.0 {
        height *= layout.line_height_multiple;
    }
    if layout.minimum_line_height > 0.0 {
        height = height.max(layout.minimum_line_height);
    }
    if layout.maximum_line_height > 0.0 {
        height = height.min(layout.maximum_line_height);
    }
    Line {
        range,
        width,
        height,
        layout,
    }
}

fn each_line(this: Id, work: &mut dyn FnMut(Line)) {
    let length = length_of(this);
    let mut start = 0;
    for at in 0..=length {
        let ends =
            at == length || matches!(character_at(this, at), LINE_FEED | PARAGRAPH_SEPARATOR);
        if !ends {
            continue;
        }
        work(measure_line(
            this,
            Range {
                location: start,
                length: at - start,
            },
        ));
        start = at + 1;
        if at == length {
            break;
        }
    }
}

fn line_x(line: &Line, left: f64, width: f64) -> f64 {
    let start = left + line.layout.first_line_head_indent;
    let right = match line.layout.tail_indent {
        tail if tail > 0.0 => left + tail,
        tail => left + width + tail,
    };
    match line.layout.alignment {
        alignment::CENTER => start + ((right - start) - line.width).max(0.0) / 2.0,
        alignment::RIGHT => (right - line.width).max(start),
        _ => start,
    }
}

fn draw_line(this: Id, line: &Line, x: f64, bottom: f64) {
    let mut x = x;
    each_run_in(this, line.range, false, &mut |attributes, piece| {
        x += draw_piece(piece_text(this, piece), attributes, Point::new(x, bottom));
        true
    });
}

fn total_size(this: Id) -> Size {
    let mut total = Size {
        width: 0.0,
        height: 0.0,
    };
    let mut first = true;
    each_line(this, &mut |line| {
        if !first {
            total.height += line.layout.paragraph_spacing_before;
        }
        total.width = total.width.max(line.width + line.layout.first_line_head_indent);
        total.height += line.height + line.layout.line_spacing + line.layout.paragraph_spacing;
        first = false;
    });
    total
}

extern "C" fn draw_at_point(this: Id, _: Sel, at: Point) {
    let size = total_size(this);
    draw_in_rect(
        this,
        sel!("drawInRect:"),
        Frame::new(at.x, at.y, size.width, size.height),
    );
}

extern "C" fn draw_in_rect(this: Id, _: Sel, rect: Frame) {
    let mut top = rect.y + rect.height;
    let mut first = true;
    each_line(this, &mut |line| {
        if !first {
            top -= line.layout.paragraph_spacing_before;
        }
        first = false;
        let bottom = top - line.height;
        if bottom + line.height >= rect.y {
            draw_line(this, &line, line_x(&line, rect.x, rect.width), bottom);
        }
        top = bottom - line.layout.line_spacing - line.layout.paragraph_spacing;
    });
}

extern "C" fn size(this: Id, _: Sel) -> Size {
    total_size(this)
}

struct Collected {
    units: Buffer<u16>,
    runs: Buffer<(usize, Style)>,
    paragraphs: Buffer<(usize, usize, Alignment)>,
    paragraph_start: usize,
}

impl Sink for Collected {
    fn unit(&mut self, unit: u16, style: &Style) {
        let at = self.units.len();
        if self.runs.as_slice().last().is_none_or(|&(_, last)| last != *style) {
            self.runs.push((at, *style));
        }
        self.units.push(unit);
    }

    fn paragraph(&mut self, alignment: Alignment) {
        let end = self.units.len();
        self.paragraphs.push((self.paragraph_start, end, alignment));
        self.paragraph_start = end;
    }
}

impl Collected {
    fn free(&mut self) {
        self.units.free();
        self.runs.free();
        self.paragraphs.free();
    }
}

fn class_named(name: &[u8]) -> Id {
    crate::objc::class_named(name).cast()
}

fn style_attributes(style: &Style) -> Id {
    let attributes = send(
        send(class_named(b"NSMutableDictionary"), sel!("alloc")),
        sel!("init"),
    );
    if attributes.is_null() {
        return attributes;
    }
    let put = |name: &str, value: Id| {
        if !value.is_null() {
            send2(attributes, sel!("setObject:forKey:"), value as usize, string(name) as usize);
        }
    };
    let size = f64::from(style.half_points) / 2.0;
    let font_of: extern "C" fn(Id, Sel, f64) -> Id = message::msg_send_as();
    let font_sel = match style.bold {
        true => sel!("boldSystemFontOfSize:"),
        false => sel!("systemFontOfSize:"),
    };
    put("NSFont", font_of(class_named(b"NSFont"), font_sel, size));
    if let Some(colour) = style.colour {
        let channel = |shift: u32| f64::from((colour >> shift) & 0xff) / 255.0;
        let make: extern "C" fn(Id, Sel, f64, f64, f64, f64) -> Id = message::msg_send_as();
        let colour = make(
            class_named(b"NSColor"),
            sel!("colorWithSRGBRed:green:blue:alpha:"),
            channel(16),
            channel(8),
            channel(0),
            1.0,
        );
        put("NSColor", colour);
    }
    let one = || send1(class_named(b"NSNumber"), sel!("numberWithInteger:"), 1);
    if style.underline {
        put("NSUnderline", one());
    }
    if style.strikethrough {
        put("NSStrikethrough", one());
    }
    attributes
}

fn alignment_style(alignment: Alignment) -> Id {
    let value = match alignment {
        Alignment::Left => return core::ptr::null_mut(),
        Alignment::Center => alignment::CENTER,
        Alignment::Right => alignment::RIGHT,
        Alignment::Justified => alignment::JUSTIFIED,
    };
    let style = send(
        send(class_named(b"NSMutableParagraphStyle"), sel!("alloc")),
        sel!("init"),
    );
    if !style.is_null() {
        send1(style, sel!("setAlignment:"), value);
    }
    style
}

fn from_rtf(this: Id, bytes: &[u8]) -> Id {
    if this.is_null() {
        return this;
    }
    let mut collected = Collected {
        units: Buffer::new(),
        runs: Buffer::new(),
        paragraphs: Buffer::new(),
        paragraph_start: 0,
    };
    if rtf::read(bytes, &mut collected).is_err() {
        collected.free();
        objc_release(this);
        return core::ptr::null_mut();
    }
    let units = collected.units.as_slice();
    let text = send2(
        class_named(b"NSString"),
        sel!("stringWithCharacters:length:"),
        units.as_ptr() as usize,
        units.len(),
    );
    let made = set_up(this, text, core::ptr::null_mut());
    if made.is_null() {
        collected.free();
        return made;
    }
    let total = units.len();
    let runs = collected.runs.as_slice();
    for (index, &(start, style)) in runs.iter().enumerate() {
        let end = runs.get(index + 1).map_or(total, |&(next, _)| next);
        let attributes = style_attributes(&style);
        let range = Range {
            location: start,
            length: end - start,
        };
        set_attributes(made, sel!("setAttributes:range:"), attributes, range);
        if !attributes.is_null() {
            objc_release(attributes);
        }
    }
    for &(start, end, alignment) in collected.paragraphs.as_slice() {
        let style = alignment_style(alignment);
        if style.is_null() {
            continue;
        }
        let range = Range {
            location: start,
            length: end - start,
        };
        let name = string("NSParagraphStyle");
        add_attribute(made, sel!("addAttribute:value:range:"), name, style, range);
        objc_release(style);
    }
    collected.free();
    made
}

fn no_document_attributes(out: *mut Id) {
    if !out.is_null() {
        let empty = send(class_named(b"NSDictionary"), sel!("dictionary"));
        // SAFETY: the caller's `NSDictionary **`.
        unsafe { out.write(empty) };
    }
}

fn data_bytes(data: Id) -> &'static [u8] {
    match data.is_null() {
        true => &[],
        false => crate::foundation::data_bytes(data),
    }
}

extern "C" fn init_with_rtf(this: Id, _: Sel, data: Id, document: *mut Id) -> Id {
    let made = from_rtf(this, data_bytes(data));
    if !made.is_null() {
        no_document_attributes(document);
    }
    made
}

extern "C" fn init_with_data(
    this: Id,
    _: Sel,
    data: Id,
    _: Id,
    document: *mut Id,
    error: *mut Id,
) -> Id {
    let bytes = data_bytes(data);
    let made = match bytes.starts_with(b"{\\rtf") {
        true => from_rtf(this, bytes),
        false => {
            let text = send2(
                send(class_named(b"NSString"), sel!("alloc")),
                sel!("initWithData:encoding:"),
                data as usize,
                UTF8_ENCODING,
            );
            let made = set_up(this, text, core::ptr::null_mut());
            if !text.is_null() {
                objc_release(text);
            }
            made
        }
    };
    if !error.is_null() {
        // SAFETY: the caller's `NSError **`.
        unsafe { error.write(core::ptr::null_mut()) };
    }
    if !made.is_null() {
        no_document_attributes(document);
    }
    made
}

extern "C" fn mutable_string(this: Id, _: Sel) -> Id {
    fields(this).string
}

extern "C" fn description(this: Id, _: Sel) -> Id {
    fields(this).string
}

extern "C" fn attributed_with_string(class: Id, _: Sel, text: Id) -> Id {
    let made = send(class, sel!("alloc"));
    autoreleased(init_with_string(made, sel!("initWithString:"), text))
}

fn plain(value: Id) -> Id {
    match !value.is_null() && super::is(value, &NSATTRIBUTEDSTRING) {
        true => fields(value).string,
        false => value,
    }
}

extern "C" fn set_attributed_string_value(this: Id, _: Sel, value: Id) {
    send1(this, sel!("setStringValue:"), plain(value) as usize);
}

extern "C" fn attributed_string_value(this: Id, _: Sel) -> Id {
    let text = send(this, sel!("stringValue"));
    attributed_with_string(
        class_pointer(&NSATTRIBUTEDSTRING).cast(),
        sel!("attributedStringWithString:"),
        text,
    )
}

extern "C" fn set_attributed_title(this: Id, _: Sel, value: Id) {
    send1(this, sel!("setTitle:"), plain(value) as usize);
}

extern "C" fn attributed_title(this: Id, _: Sel) -> Id {
    let text = send(this, sel!("title"));
    attributed_with_string(
        class_pointer(&NSATTRIBUTEDSTRING).cast(),
        sel!("attributedStringWithString:"),
        text,
    )
}

pub fn define(runtime: &mut Runtime) -> bool {
    let instance = methods![
        "init" => init,
        "initWithString:" => init_with_string,
        "initWithString:attributes:" => init_with_string_attributes,
        "initWithAttributedString:" => init_with_attributed_string,
        "dealloc" => dealloc,
        "string" => string_method,
        "length" => length,
        "attributesAtIndex:effectiveRange:" => attributes_at_index,
        "attribute:atIndex:effectiveRange:" => attribute_at_index,
        "attributesAtIndex:longestEffectiveRange:inRange:" => attributes_longest,
        "attribute:atIndex:longestEffectiveRange:inRange:" => attribute_longest,
        "enumerateAttributesInRange:options:usingBlock:" => enumerate_attributes,
        "enumerateAttribute:inRange:options:usingBlock:" => enumerate_attribute,
        "initWithRTF:documentAttributes:" => init_with_rtf,
        "initWithData:options:documentAttributes:error:" => init_with_data,
        "copyWithZone:" => copy_with_zone,
        "mutableCopyWithZone:" => mutable_copy_with_zone,
        "isEqual:" => is_equal,
        "isEqualToAttributedString:" => is_equal,
        "drawAtPoint:" => draw_at_point,
        "drawInRect:" => draw_in_rect,
        "size" => size,
        "description" => description,
    ];
    let mutable = methods![
        "setAttributes:range:" => set_attributes,
        "addAttribute:value:range:" => add_attribute,
        "addAttributes:range:" => add_attributes,
        "removeAttribute:range:" => remove_attribute,
        "replaceCharactersInRange:withString:" => replace_characters,
        "appendAttributedString:" => append_attributed_string,
        "setAttributedString:" => set_attributed_string,
        "mutableString" => mutable_string,
        "copyWithZone:" => immutable_copy_with_zone,
    ];
    let class = methods![
        "attributedStringWithString:" => attributed_with_string,
    ];
    let field = methods![
        "setAttributedStringValue:" => set_attributed_string_value,
        "attributedStringValue" => attributed_string_value,
    ];
    let button = methods![
        "setAttributedTitle:" => set_attributed_title,
        "attributedTitle" => attributed_title,
    ];
    super::define(
        runtime,
        b"NSAttributedString\0",
        (&NSATTRIBUTEDSTRING, &NSATTRIBUTEDSTRING_META),
        &NSOBJECT,
        core::mem::size_of::<Fields>(),
        &instance,
        &class,
    ) && super::define(
        runtime,
        b"NSMutableAttributedString\0",
        (&NSMUTABLEATTRIBUTEDSTRING, &NSMUTABLEATTRIBUTEDSTRING_META),
        &NSATTRIBUTEDSTRING,
        core::mem::size_of::<Fields>(),
        &mutable,
        &[],
    ) && crate::objc::class::extend(runtime, &super::NSTEXTFIELD, &field, &[])
        && crate::objc::class::extend(runtime, &super::NSBUTTON, &button, &[])
}

const _: () = assert!(core::mem::size_of::<Range>() == 16);
