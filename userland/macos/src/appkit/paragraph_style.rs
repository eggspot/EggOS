//! NSParagraphStyle and NSMutableParagraphStyle (D730): a paragraph's alignment, indents,
//! spacing and line heights, which NSAttributedString's drawing reads from each paragraph's
//! first character (`NSParagraphStyleAttributeName`).

use super::{
    NSMUTABLEPARAGRAPHSTYLE, NSMUTABLEPARAGRAPHSTYLE_META, NSPARAGRAPHSTYLE, NSPARAGRAPHSTYLE_META,
};
use crate::foundation::{class_pointer, new_instance};
use crate::libsystem::Exclusive;
use crate::objc::abi::{ClassObject, Id, Sel};
use crate::objc::reference_count::objc_retain;
use crate::objc::root::{self, NSOBJECT};
use crate::objc::{Runtime, methods};

/// `NSTextAlignment`, as macOS numbers it.
pub mod alignment {
    pub const LEFT: usize = 0;
    pub const CENTER: usize = 1;
    pub const RIGHT: usize = 2;
    pub const JUSTIFIED: usize = 3;
    pub const NATURAL: usize = 4;
}

/// What a paragraph style says about laying a paragraph out.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Layout {
    pub alignment: usize,
    pub line_spacing: f64,
    pub paragraph_spacing: f64,
    pub paragraph_spacing_before: f64,
    pub first_line_head_indent: f64,
    pub head_indent: f64,
    pub tail_indent: f64,
    pub minimum_line_height: f64,
    pub maximum_line_height: f64,
    pub line_height_multiple: f64,
    pub line_break_mode: usize,
}

impl Layout {
    const DEFAULT: Layout = Layout {
        alignment: alignment::NATURAL,
        line_spacing: 0.0,
        paragraph_spacing: 0.0,
        paragraph_spacing_before: 0.0,
        first_line_head_indent: 0.0,
        head_indent: 0.0,
        tail_indent: 0.0,
        minimum_line_height: 0.0,
        maximum_line_height: 0.0,
        line_height_multiple: 0.0,
        line_break_mode: 0,
    };
}

#[repr(C)]
#[derive(Debug)]
struct StyleFields {
    isa: usize,
    layout: Layout,
}

static DEFAULT_STYLE: Exclusive<usize> = Exclusive::new(0);

fn fields(object: Id) -> &'static mut StyleFields {
    // SAFETY: the callers pass NSParagraphStyle instances, whose memory starts with these fields.
    unsafe { &mut *object.cast::<StyleFields>() }
}

/// The layout `style` says, or the default one when it is nil or no paragraph style.
pub fn layout_of(style: Id) -> Layout {
    match !style.is_null() && root::is_kind_of(style, &NSPARAGRAPHSTYLE) {
        true => fields(style).layout,
        false => Layout::DEFAULT,
    }
}

fn made(class: &'static ClassObject, layout: Layout) -> Id {
    let style = new_instance(class_pointer(class));
    if !style.is_null() {
        fields(style).layout = layout;
    }
    style
}

extern "C" fn default_paragraph_style(_: Id, _: Sel) -> Id {
    DEFAULT_STYLE.with(|style| {
        if *style == 0 {
            *style = made(&NSPARAGRAPHSTYLE, Layout::DEFAULT) as usize;
        }
        *style as Id
    })
}

extern "C" fn init(this: Id, _: Sel) -> Id {
    if !this.is_null() {
        fields(this).layout = Layout::DEFAULT;
    }
    this
}

extern "C" fn copy_with_zone(this: Id, _: Sel, _: usize) -> Id {
    match root::is_kind_of(this, &NSMUTABLEPARAGRAPHSTYLE) {
        true => made(&NSPARAGRAPHSTYLE, fields(this).layout),
        false => objc_retain(this),
    }
}

extern "C" fn mutable_copy_with_zone(this: Id, _: Sel, _: usize) -> Id {
    made(&NSMUTABLEPARAGRAPHSTYLE, fields(this).layout)
}

extern "C" fn set_paragraph_style(this: Id, _: Sel, other: Id) {
    fields(this).layout = layout_of(other);
}

extern "C" fn is_equal(this: Id, _: Sel, other: Id) -> u8 {
    let same = other == this
        || (!other.is_null()
            && root::is_kind_of(other, &NSPARAGRAPHSTYLE)
            && fields(other).layout == fields(this).layout);
    u8::from(same)
}

extern "C" fn default_writing_direction(_: Id, _: Sel, _: Id) -> isize {
    0
}

macro_rules! layout_accessors {
    ($($getter:ident, $setter:ident, $field:ident: $kind:ty;)*) => {
        $(
            extern "C" fn $getter(this: Id, _: Sel) -> $kind {
                fields(this).layout.$field
            }

            extern "C" fn $setter(this: Id, _: Sel, value: $kind) {
                fields(this).layout.$field = value;
            }
        )*
    };
}

layout_accessors! {
    get_alignment, set_alignment, alignment: usize;
    get_line_spacing, set_line_spacing, line_spacing: f64;
    get_paragraph_spacing, set_paragraph_spacing, paragraph_spacing: f64;
    get_spacing_before, set_spacing_before, paragraph_spacing_before: f64;
    get_first_indent, set_first_indent, first_line_head_indent: f64;
    get_head_indent, set_head_indent, head_indent: f64;
    get_tail_indent, set_tail_indent, tail_indent: f64;
    get_minimum_height, set_minimum_height, minimum_line_height: f64;
    get_maximum_height, set_maximum_height, maximum_line_height: f64;
    get_height_multiple, set_height_multiple, line_height_multiple: f64;
    get_break_mode, set_break_mode, line_break_mode: usize;
}

pub fn define(runtime: &mut Runtime) -> bool {
    let instance = methods![
        "init" => init,
        "copyWithZone:" => copy_with_zone,
        "mutableCopyWithZone:" => mutable_copy_with_zone,
        "isEqual:" => is_equal,
        "alignment" => get_alignment,
        "lineSpacing" => get_line_spacing,
        "paragraphSpacing" => get_paragraph_spacing,
        "paragraphSpacingBefore" => get_spacing_before,
        "firstLineHeadIndent" => get_first_indent,
        "headIndent" => get_head_indent,
        "tailIndent" => get_tail_indent,
        "minimumLineHeight" => get_minimum_height,
        "maximumLineHeight" => get_maximum_height,
        "lineHeightMultiple" => get_height_multiple,
        "lineBreakMode" => get_break_mode,
    ];
    let class = methods![
        "defaultParagraphStyle" => default_paragraph_style,
        "defaultWritingDirectionForLanguage:" => default_writing_direction,
    ];
    let mutable = methods![
        "setParagraphStyle:" => set_paragraph_style,
        "setAlignment:" => set_alignment,
        "setLineSpacing:" => set_line_spacing,
        "setParagraphSpacing:" => set_paragraph_spacing,
        "setParagraphSpacingBefore:" => set_spacing_before,
        "setFirstLineHeadIndent:" => set_first_indent,
        "setHeadIndent:" => set_head_indent,
        "setTailIndent:" => set_tail_indent,
        "setMinimumLineHeight:" => set_minimum_height,
        "setMaximumLineHeight:" => set_maximum_height,
        "setLineHeightMultiple:" => set_height_multiple,
        "setLineBreakMode:" => set_break_mode,
    ];
    super::define(
        runtime,
        b"NSParagraphStyle\0",
        (&NSPARAGRAPHSTYLE, &NSPARAGRAPHSTYLE_META),
        &NSOBJECT,
        core::mem::size_of::<StyleFields>(),
        &instance,
        &class,
    ) && super::define(
        runtime,
        b"NSMutableParagraphStyle\0",
        (&NSMUTABLEPARAGRAPHSTYLE, &NSMUTABLEPARAGRAPHSTYLE_META),
        &NSPARAGRAPHSTYLE,
        core::mem::size_of::<StyleFields>(),
        &mutable,
        &[],
    )
}
