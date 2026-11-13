//! NSAttributedString and NSMutableAttributedString (D384): a string and the attribute
//! dictionaries of its runs, which cover it end to end in UTF-16 units, and AppKit's drawing
//! of it, each run drawn with its own font and colour after the one before.

use super::{
    NSATTRIBUTEDSTRING, NSATTRIBUTEDSTRING_META, NSMUTABLEATTRIBUTEDSTRING,
    NSMUTABLEATTRIBUTEDSTRING_META, store, string,
};
use crate::foundation::{NSDICTIONARY, autoreleased, class_pointer};
use crate::objc::abi::{Id, Sel};
use crate::objc::buffer::Buffer;
use crate::objc::reference_count::{dispose, objc_release, objc_retain};
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, message, methods, sel, send, send1, send2};
use eggos_macos_gui::geometry::{Frame, Point, Size};

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

fn each_run(this: Id, work: &mut dyn FnMut(Id, Id)) {
    let count = fields(this).runs.len();
    for index in 0..count {
        let run = fields(this).runs.as_slice()[index];
        let end = run_end(this, index);
        if end <= run.start {
            continue;
        }
        let range = Range {
            location: run.start,
            length: end - run.start,
        };
        let piece: extern "C" fn(Id, Sel, Range) -> Id = message::msg_send_as();
        let text = piece(fields(this).string, sel!("substringWithRange:"), range);
        work(text, run.attributes);
    }
}

fn size_of_piece(text: Id, attributes: Id) -> Size {
    let measure: extern "C" fn(Id, Sel, Id) -> Size = message::msg_send_as();
    measure(text, sel!("sizeWithAttributes:"), attributes)
}

extern "C" fn draw_at_point(this: Id, _: Sel, at: Point) {
    let mut x = at.x;
    each_run(this, &mut |text, attributes| {
        let draw: extern "C" fn(Id, Sel, Point, Id) = message::msg_send_as();
        draw(
            text,
            sel!("drawAtPoint:withAttributes:"),
            Point::new(x, at.y),
            attributes,
        );
        x += size_of_piece(text, attributes).width;
    });
}

extern "C" fn draw_in_rect(this: Id, _: Sel, rect: Frame) {
    let mut x = rect.x;
    each_run(this, &mut |text, attributes| {
        let width = size_of_piece(text, attributes).width;
        let draw: extern "C" fn(Id, Sel, Frame, Id) = message::msg_send_as();
        let room = (rect.x + rect.width - x).max(0.0);
        draw(
            text,
            sel!("drawInRect:withAttributes:"),
            Frame::new(x, rect.y, room, rect.height),
            attributes,
        );
        x += width;
    });
}

extern "C" fn size(this: Id, _: Sel) -> Size {
    let mut total = Size {
        width: 0.0,
        height: 0.0,
    };
    each_run(this, &mut |text, attributes| {
        let piece = size_of_piece(text, attributes);
        total.width += piece.width;
        total.height = total.height.max(piece.height);
    });
    total
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
