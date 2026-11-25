//! Auto Layout (D506, D769): a view's anchors (`leadingAnchor`, `topAnchor`, `widthAnchor`,
//! `firstBaselineAnchor`, ...) make constraints (`constraintEqualToAnchor:constant:`, the
//! inequalities, `constraintEqualToConstant:`, the system-spacing ones), as does
//! `constraintWithItem:attribute:relatedBy:toItem:attribute:multiplier:constant:`, each with its
//! relation and priority; active constraints (`active`, `activateConstraints:`) are applied
//! before a window draws. Each pass gathers what every constraint on a view wants of each axis
//! from the other items' frames (through their places in the superview, the vertical axis
//! measured down from the top as Auto Layout does, leading and trailing mirrored when the
//! user's language reads right to left), adds its intrinsic size at its hugging and
//! compression-resistance priorities, and solves each axis strongest first; three passes
//! settle chains of views placed by their neighbours. Constraints are kept in a table of 512,
//! the first come.
//!
//! Stack views (D510, D769): `NSStackView` and UIKit's `UIStackView` keep up to 32 arranged
//! subviews, an axis, a spacing, a distribution and an alignment (read as UIKit's in an iOS
//! app, as an `NSLayoutAttribute` in a Mac app), and each pass places the shown ones from the
//! stack's leading edge or top. A view's `safeAreaLayoutGuide`, `layoutMarginsGuide` and
//! `readableContentGuide` are the view itself, whose anchors stand for theirs.

use super::view::fields;
use super::{
    NSAPPLICATION, NSFONT, NSLAYOUTANCHOR, NSLAYOUTANCHOR_META, NSLAYOUTCONSTRAINT,
    NSLAYOUTCONSTRAINT_META, NSSTACKVIEW, NSSTACKVIEW_META, NSVIEW,
};
use crate::foundation::{class_pointer, new_instance};
use crate::objc::abi::{Id, Sel};
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, class, message, methods, sel, send, send_bool, send1};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};
use eggos_macos_gui::auto_layout::{
    self, Alignment, Arranged, DEFAULT_COMPRESSION_RESISTANCE, DEFAULT_HUGGING, Distribution,
    Part, REQUIRED, SYSTEM_SPACING, Want, relation,
};
use eggos_macos_gui::geometry::{Frame, Point, Size};

mod attribute {
    pub const LEFT: u64 = 1;
    pub const RIGHT: u64 = 2;
    pub const TOP: u64 = 3;
    pub const BOTTOM: u64 = 4;
    pub const LEADING: u64 = 5;
    pub const TRAILING: u64 = 6;
    pub const WIDTH: u64 = 7;
    pub const HEIGHT: u64 = 8;
    pub const CENTER_X: u64 = 9;
    pub const CENTER_Y: u64 = 10;
    pub const LAST_BASELINE: u64 = 11;
    pub const FIRST_BASELINE: u64 = 12;
    pub const LEFT_MARGIN: u64 = 13;
    pub const RIGHT_MARGIN: u64 = 14;
    pub const TOP_MARGIN: u64 = 15;
    pub const BOTTOM_MARGIN: u64 = 16;
    pub const LEADING_MARGIN: u64 = 17;
    pub const TRAILING_MARGIN: u64 = 18;
    pub const CENTER_X_WITHIN_MARGINS: u64 = 19;
    pub const CENTER_Y_WITHIN_MARGINS: u64 = 20;
}

const LAYOUT_MARGIN: f64 = 8.0;

const MOST_CONSTRAINTS: usize = 512;
const PASSES: usize = 3;

static ACTIVE: [AtomicUsize; MOST_CONSTRAINTS] = [const { AtomicUsize::new(0) }; MOST_CONSTRAINTS];

#[repr(C)]
struct AnchorFields {
    isa: usize,
    view: Id,
    attribute: u64,
}

#[repr(C)]
struct ConstraintFields {
    isa: usize,
    first: Id,
    first_attribute: u64,
    second: Id,
    second_attribute: u64,
    multiplier: f64,
    constant: f64,
    active: u8,
    relation: i64,
    priority: f32,
}

fn anchor(object: Id) -> &'static mut AnchorFields {
    // SAFETY: the callers pass NSLayoutAnchor instances, whose memory starts with these fields.
    unsafe { &mut *object.cast::<AnchorFields>() }
}

fn constraint(object: Id) -> &'static mut ConstraintFields {
    // SAFETY: the callers pass NSLayoutConstraint instances, which start with these fields.
    unsafe { &mut *object.cast::<ConstraintFields>() }
}

fn new_anchor(view: Id, kind: u64) -> Id {
    let object = new_instance(class_pointer(&NSLAYOUTANCHOR));
    if !object.is_null() {
        let fields = anchor(object);
        fields.view = view;
        fields.attribute = kind;
    }
    object
}

fn new_constraint(
    (first, first_attribute): (Id, u64),
    (second, second_attribute): (Id, u64),
    relation: i64,
    multiplier: f64,
    constant: f64,
) -> Id {
    let object = new_instance(class_pointer(&NSLAYOUTCONSTRAINT));
    if !object.is_null() {
        let fields = constraint(object);
        fields.first = first;
        fields.first_attribute = first_attribute;
        fields.second = second;
        fields.second_attribute = second_attribute;
        fields.multiplier = multiplier;
        fields.constant = constant;
        fields.relation = relation.clamp(relation::LESS_OR_EQUAL, relation::GREATER_OR_EQUAL);
        fields.priority = REQUIRED;
    }
    object
}

macro_rules! anchors {
    ($($name:ident => $kind:expr),* $(,)?) => {
        $(
            extern "C" fn $name(this: Id, _: Sel) -> Id {
                new_anchor(this, $kind)
            }
        )*
    };
}

anchors! {
    left_anchor => attribute::LEFT,
    right_anchor => attribute::RIGHT,
    top_anchor => attribute::TOP,
    bottom_anchor => attribute::BOTTOM,
    leading_anchor => attribute::LEADING,
    trailing_anchor => attribute::TRAILING,
    width_anchor => attribute::WIDTH,
    height_anchor => attribute::HEIGHT,
    center_x_anchor => attribute::CENTER_X,
    center_y_anchor => attribute::CENTER_Y,
    first_baseline_anchor => attribute::FIRST_BASELINE,
    last_baseline_anchor => attribute::LAST_BASELINE,
}

fn to_anchor(this: Id, other: Id, relation: i64, multiplier: f64, constant: f64) -> Id {
    let own = anchor(this);
    let (view, kind) = (own.view, own.attribute);
    match other.is_null() {
        true => new_constraint(
            (view, kind),
            (core::ptr::null_mut(), 0),
            relation,
            0.0,
            constant,
        ),
        false => {
            let other = anchor(other);
            new_constraint(
                (view, kind),
                (other.view, other.attribute),
                relation,
                multiplier,
                constant,
            )
        }
    }
}

macro_rules! anchor_constraints {
    ($($relation:expr => $to:ident, $to_constant:ident, $to_multiplier:ident,
        $to_multiplier_constant:ident, $constant:ident, $spacing:ident;)*) => {
        $(
            extern "C" fn $to(this: Id, _: Sel, other: Id) -> Id {
                to_anchor(this, other, $relation, 1.0, 0.0)
            }

            extern "C" fn $to_constant(this: Id, _: Sel, other: Id, constant: f64) -> Id {
                to_anchor(this, other, $relation, 1.0, constant)
            }

            extern "C" fn $to_multiplier(this: Id, _: Sel, other: Id, multiplier: f64) -> Id {
                to_anchor(this, other, $relation, multiplier, 0.0)
            }

            extern "C" fn $to_multiplier_constant(
                this: Id,
                _: Sel,
                other: Id,
                multiplier: f64,
                constant: f64,
            ) -> Id {
                to_anchor(this, other, $relation, multiplier, constant)
            }

            extern "C" fn $constant(this: Id, _: Sel, constant: f64) -> Id {
                to_anchor(this, core::ptr::null_mut(), $relation, 0.0, constant)
            }

            extern "C" fn $spacing(this: Id, _: Sel, other: Id, multiplier: f64) -> Id {
                to_anchor(this, other, $relation, 1.0, SYSTEM_SPACING * multiplier)
            }
        )*
    };
}

anchor_constraints! {
    relation::EQUAL => equal_to_anchor, equal_to_anchor_constant, equal_to_anchor_multiplier,
        equal_to_anchor_multiplier_constant, equal_to_constant, equal_spacing;
    relation::GREATER_OR_EQUAL => at_least_anchor, at_least_anchor_constant,
        at_least_anchor_multiplier, at_least_anchor_multiplier_constant, at_least_constant,
        at_least_spacing;
    relation::LESS_OR_EQUAL => at_most_anchor, at_most_anchor_constant,
        at_most_anchor_multiplier, at_most_anchor_multiplier_constant, at_most_constant,
        at_most_spacing;
}

#[allow(clippy::too_many_arguments)]
extern "C" fn with_item(
    _: Id,
    _: Sel,
    first: Id,
    first_attribute: u64,
    relation: i64,
    second: Id,
    second_attribute: u64,
    multiplier: f64,
    constant: f64,
) -> Id {
    new_constraint(
        (first, first_attribute),
        (second, second_attribute),
        relation,
        multiplier,
        constant,
    )
}

fn set_active(object: Id, active: bool) {
    let fields = constraint(object);
    if active == (fields.active != 0) {
        return;
    }
    fields.active = u8::from(active);
    match active {
        true => {
            let found = ACTIVE.iter().any(|slot| {
                slot.compare_exchange(0, object as usize, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
            });
            if !found {
                fields.active = 0;
            }
        }
        false => {
            for slot in &ACTIVE {
                let _ =
                    slot.compare_exchange(object as usize, 0, Ordering::AcqRel, Ordering::Acquire);
            }
        }
    }
}

extern "C" fn constraint_set_active(this: Id, _: Sel, active: u8) {
    set_active(this, active != 0);
}

extern "C" fn constraint_is_active(this: Id, _: Sel) -> u8 {
    constraint(this).active
}

extern "C" fn constraint_constant(this: Id, _: Sel) -> f64 {
    constraint(this).constant
}

extern "C" fn constraint_set_constant(this: Id, _: Sel, constant: f64) {
    constraint(this).constant = constant;
}

extern "C" fn constraint_priority(this: Id, _: Sel) -> f32 {
    constraint(this).priority
}

extern "C" fn constraint_set_priority(this: Id, _: Sel, priority: f32) {
    constraint(this).priority = priority.clamp(1.0, REQUIRED);
}

extern "C" fn constraint_relation(this: Id, _: Sel) -> i64 {
    constraint(this).relation
}

extern "C" fn constraint_multiplier(this: Id, _: Sel) -> f64 {
    constraint(this).multiplier
}

extern "C" fn constraint_first_item(this: Id, _: Sel) -> Id {
    constraint(this).first
}

extern "C" fn constraint_second_item(this: Id, _: Sel) -> Id {
    constraint(this).second
}

extern "C" fn constraint_first_attribute(this: Id, _: Sel) -> u64 {
    constraint(this).first_attribute
}

extern "C" fn constraint_second_attribute(this: Id, _: Sel) -> u64 {
    constraint(this).second_attribute
}

extern "C" fn ignore_identifier(_: Id, _: Sel, _: Id) {}

extern "C" fn no_identifier(_: Id, _: Sel) -> Id {
    core::ptr::null_mut()
}

fn each_in(array: Id, mut each: impl FnMut(Id)) {
    if array.is_null() {
        return;
    }
    let count = send(array, sel!("count")) as usize;
    for index in 0..count {
        each(send1(array, sel!("objectAtIndex:"), index));
    }
}

extern "C" fn activate_constraints(_: Id, _: Sel, array: Id) {
    each_in(array, |object| set_active(object, true));
}

extern "C" fn deactivate_constraints(_: Id, _: Sel, array: Id) {
    each_in(array, |object| set_active(object, false));
}

extern "C" fn ignore_flag(_: Id, _: Sel, _: u8) {}

extern "C" fn yes(_: Id, _: Sel) -> u8 {
    1
}

fn origin_in_window(view: Id) -> Point {
    let mut origin = Point::default();
    let mut current = view;
    let mut depth = 0;
    while !current.is_null() && depth < 64 {
        let frame = fields(current).frame;
        origin = Point::new(origin.x + frame.x, origin.y + frame.y);
        current = fields(current).superview;
        depth += 1;
    }
    origin
}

fn frame_in(view: Id, space: Id) -> Frame {
    let frame = fields(view).frame;
    let own = origin_in_window(view);
    let base = match space.is_null() {
        true => Point::default(),
        false => origin_in_window(space),
    };
    Frame {
        x: own.x - base.x,
        y: own.y - base.y,
        width: frame.width,
        height: frame.height,
    }
}

static DIRECTION: AtomicU8 = AtomicU8::new(0);
const LEFT_TO_RIGHT: u8 = 1;
const RIGHT_TO_LEFT: u8 = 2;

fn right_to_left() -> bool {
    match DIRECTION.load(Ordering::Relaxed) {
        LEFT_TO_RIGHT => false,
        RIGHT_TO_LEFT => true,
        _ => {
            let locale = crate::libsystem::locale::messages_locale();
            let backwards = auto_layout::is_right_to_left(locale);
            let direction = if backwards { RIGHT_TO_LEFT } else { LEFT_TO_RIGHT };
            DIRECTION.store(direction, Ordering::Relaxed);
            backwards
        }
    }
}

extern "C" fn layout_direction(_: Id, _: Sel) -> i64 {
    i64::from(right_to_left())
}

extern "C" fn layout_direction_for_attribute(_: Id, _: Sel, attribute: i64) -> i64 {
    match attribute {
        1..=3 => 0,
        4 => 1,
        _ => i64::from(right_to_left()),
    }
}

extern "C" fn semantic_content_attribute(_: Id, _: Sel) -> i64 {
    0
}

extern "C" fn ignore_number(_: Id, _: Sel, _: i64) {}

const MOST_PRIORITISED: usize = 256;
const HUGGING: usize = 0;
const RESISTANCE: usize = 2;

static VIEW_PRIORITIES: [(AtomicUsize, AtomicU64); MOST_PRIORITISED] =
    [const { (AtomicUsize::new(0), AtomicU64::new(0)) }; MOST_PRIORITISED];

fn packed_defaults() -> u64 {
    let hugging = DEFAULT_HUGGING as u64;
    let resistance = DEFAULT_COMPRESSION_RESISTANCE as u64;
    hugging | hugging << 16 | resistance << 32 | resistance << 48
}

fn view_priority(view: Id, kind: usize, vertical: bool) -> f32 {
    let shift = (kind + usize::from(vertical)) * 16;
    let packed = VIEW_PRIORITIES
        .iter()
        .find(|(owner, _)| owner.load(Ordering::Acquire) == view as usize)
        .map_or_else(packed_defaults, |(_, packed)| packed.load(Ordering::Acquire));
    (packed >> shift & 0xffff) as f32
}

fn set_view_priority(view: Id, kind: usize, orientation: i64, priority: f32) {
    let owned = |(owner, _): &&(AtomicUsize, AtomicU64)| {
        owner.load(Ordering::Acquire) == view as usize
    };
    let slot = VIEW_PRIORITIES.iter().find(owned).or_else(|| {
        VIEW_PRIORITIES.iter().find(|(owner, packed)| {
            let claimed = owner
                .compare_exchange(0, view as usize, Ordering::AcqRel, Ordering::Acquire)
                .is_ok();
            if claimed {
                packed.store(packed_defaults(), Ordering::Release);
            }
            claimed
        })
    });
    let Some((_, packed)) = slot else {
        return;
    };
    let shift = (kind + usize::from(orientation == 1)) * 16;
    let value = priority.clamp(1.0, REQUIRED) as u64;
    let old = packed.load(Ordering::Acquire);
    packed.store(old & !(0xffff << shift) | value << shift, Ordering::Release);
}

extern "C" fn set_hugging(this: Id, _: Sel, priority: f32, orientation: i64) {
    set_view_priority(this, HUGGING, orientation, priority);
}

extern "C" fn hugging(this: Id, _: Sel, orientation: i64) -> f32 {
    view_priority(this, HUGGING, orientation == 1)
}

extern "C" fn set_resistance(this: Id, _: Sel, priority: f32, orientation: i64) {
    set_view_priority(this, RESISTANCE, orientation, priority);
}

extern "C" fn resistance(this: Id, _: Sel, orientation: i64) -> f32 {
    view_priority(this, RESISTANCE, orientation == 1)
}

fn font_metrics(view: Id) -> Option<(f64, f64)> {
    if !send_bool(view, sel!("respondsToSelector:"), sel!("font") as usize) {
        return None;
    }
    let mut font = send(view, sel!("font"));
    if font.is_null() {
        let system: extern "C" fn(Id, Sel, f64) -> Id = message::msg_send_as();
        font = system(class_pointer(&NSFONT).cast(), sel!("systemFontOfSize:"), 0.0);
    }
    if font.is_null() {
        return None;
    }
    let metric: extern "C" fn(Id, Sel) -> f64 = message::msg_send_as();
    Some((metric(font, sel!("ascender")), metric(font, sel!("descender"))))
}

extern "C" fn first_baseline_from_top(this: Id, _: Sel) -> f64 {
    let height = fields(this).frame.height;
    font_metrics(this).map_or(height, |(ascender, descender)| {
        auto_layout::baseline_offsets(height, ascender, descender).0
    })
}

extern "C" fn last_baseline_from_bottom(this: Id, _: Sel) -> f64 {
    let height = fields(this).frame.height;
    font_metrics(this).map_or(0.0, |(ascender, descender)| {
        auto_layout::baseline_offsets(height, ascender, descender).1
    })
}

fn baselines(view: Id) -> (f64, f64) {
    let offset: extern "C" fn(Id, Sel) -> f64 = message::msg_send_as();
    (
        offset(view, sel!("firstBaselineOffsetFromTop")),
        offset(view, sel!("lastBaselineOffsetFromBottom")),
    )
}

#[derive(Clone, Copy)]
struct Place {
    vertical: bool,
    part: Part,
    leading: bool,
    offset: f64,
}

fn place_of(view: Id, kind: u64) -> Option<Place> {
    let place = |vertical, part, leading, offset| {
        Some(Place {
            vertical,
            part,
            leading,
            offset,
        })
    };
    match kind {
        attribute::LEFT => place(false, Part::Start, false, 0.0),
        attribute::RIGHT => place(false, Part::End, false, 0.0),
        attribute::LEADING => place(false, Part::Start, true, 0.0),
        attribute::TRAILING => place(false, Part::End, true, 0.0),
        attribute::WIDTH => place(false, Part::Length, false, 0.0),
        attribute::CENTER_X | attribute::CENTER_X_WITHIN_MARGINS => {
            place(false, Part::Centre, false, 0.0)
        }
        attribute::TOP => place(true, Part::Start, false, 0.0),
        attribute::BOTTOM => place(true, Part::End, false, 0.0),
        attribute::HEIGHT => place(true, Part::Length, false, 0.0),
        attribute::CENTER_Y | attribute::CENTER_Y_WITHIN_MARGINS => {
            place(true, Part::Centre, false, 0.0)
        }
        attribute::FIRST_BASELINE => place(true, Part::Start, false, baselines(view).0),
        attribute::LAST_BASELINE => place(true, Part::End, false, -baselines(view).1),
        attribute::LEFT_MARGIN => place(false, Part::Start, false, LAYOUT_MARGIN),
        attribute::RIGHT_MARGIN => place(false, Part::End, false, -LAYOUT_MARGIN),
        attribute::LEADING_MARGIN => place(false, Part::Start, true, LAYOUT_MARGIN),
        attribute::TRAILING_MARGIN => place(false, Part::End, true, -LAYOUT_MARGIN),
        attribute::TOP_MARGIN => place(true, Part::Start, false, LAYOUT_MARGIN),
        attribute::BOTTOM_MARGIN => place(true, Part::End, false, -LAYOUT_MARGIN),
        _ => None,
    }
}

fn space_of(superview: Id) -> Size {
    match superview.is_null() {
        true => Size::default(),
        false => {
            let frame = fields(superview).frame;
            Size {
                width: frame.width,
                height: frame.height,
            }
        }
    }
}

fn axis_of(frame: Frame, vertical: bool, mirrored: bool, space: Size) -> (f64, f64) {
    match (vertical, mirrored) {
        (true, _) => (space.height - (frame.y + frame.height), frame.height),
        (false, true) => (space.width - (frame.x + frame.width), frame.width),
        (false, false) => (frame.x, frame.width),
    }
}

fn unmirrored(want: Want, width: f64) -> Want {
    let part = match want.part {
        Part::Start => Part::End,
        Part::End => Part::Start,
        other => other,
    };
    match want.part {
        Part::Length => want,
        _ => Want {
            part,
            value: width - want.value,
            relation: -want.relation,
            priority: want.priority,
        },
    }
}

fn want_of(view: Id, rule: &ConstraintFields, superview: Id, space: Size) -> Option<(bool, Want)> {
    let own = place_of(view, rule.first_attribute)?;
    let other = match rule.second.is_null() {
        true => None,
        false => Some(place_of(rule.second, rule.second_attribute)?),
    };
    let mirrored = right_to_left() && (own.leading || other.is_some_and(|other| other.leading));
    let source = match other {
        None => 0.0,
        Some(other) => {
            let frame = frame_in(rule.second, superview);
            let (start, length) = axis_of(frame, other.vertical, mirrored, space);
            other.part.value_at(start, length) + other.offset
        }
    };
    let want = Want {
        part: own.part,
        value: source * rule.multiplier + rule.constant - own.offset,
        relation: rule.relation,
        priority: rule.priority,
    };
    Some(match !own.vertical && mirrored {
        true => (false, unmirrored(want, space.width)),
        false => (own.vertical, want),
    })
}

fn intrinsic_size(view: Id) -> Option<Size> {
    if !send_bool(
        view,
        sel!("respondsToSelector:"),
        sel!("intrinsicContentSize") as usize,
    ) {
        return None;
    }
    let size: extern "C" fn(Id, Sel) -> Size = message::msg_send_as();
    let size = size(view, sel!("intrinsicContentSize"));
    (size.width > 0.0 || size.height > 0.0).then_some(size)
}

fn lay_out(view: Id) {
    let superview = fields(view).superview;
    let space = space_of(superview);
    let mut horizontal: Vec<Want> = Vec::new();
    let mut vertical: Vec<Want> = Vec::new();
    for slot in &ACTIVE {
        let object = slot.load(Ordering::Acquire) as Id;
        if object.is_null() || constraint(object).first != view {
            continue;
        }
        match want_of(view, constraint(object), superview, space) {
            Some((true, want)) => vertical.push(want),
            Some((false, want)) => horizontal.push(want),
            None => {}
        }
    }
    let frame = fields(view).frame;
    let intrinsic = intrinsic_size(view);
    let mut width = frame.width;
    let mut height = frame.height;
    if let Some(size) = intrinsic {
        for (wants, length, is_vertical) in [
            (&mut horizontal, size.width, false),
            (&mut vertical, size.height, true),
        ] {
            if length > 0.0 {
                wants.extend(auto_layout::intrinsic_wants(
                    length,
                    view_priority(view, HUGGING, is_vertical),
                    view_priority(view, RESISTANCE, is_vertical),
                ));
            }
        }
        if width <= 0.0 {
            width = size.width;
        }
        if height <= 0.0 {
            height = size.height;
        }
    }
    let (x, width) = auto_layout::solve(&horizontal, frame.x, width);
    let top = space.height - (frame.y + frame.height);
    let (top, height) = auto_layout::solve(&vertical, top, height);
    fields(view).frame = Frame {
        x,
        y: space.height - (top + height),
        width,
        height,
    };
}

/// Applies the active constraints to the views they place, and the stack views to their
/// arranged subviews (before a window draws).
pub fn apply() {
    for _ in 0..PASSES {
        for slot in &ACTIVE {
            let object = slot.load(Ordering::Acquire) as Id;
            if object.is_null() {
                continue;
            }
            let first = constraint(object).first;
            if !first.is_null() {
                lay_out(first);
            }
        }
        for slot in &STACKS {
            let stack = slot.load(Ordering::Acquire) as Id;
            if !stack.is_null() {
                stack_lay_out(stack);
            }
        }
    }
}

const MOST_ARRANGED: usize = 32;
const MOST_STACKS: usize = 128;

static STACKS: [AtomicUsize; MOST_STACKS] = [const { AtomicUsize::new(0) }; MOST_STACKS];

#[repr(C)]
struct StackFields {
    base: super::view::ViewFields,
    vertical: u8,
    spacing: f64,
    count: usize,
    arranged: [Id; MOST_ARRANGED],
    distribution: i64,
    alignment: i64,
}

fn stack(object: Id) -> &'static mut StackFields {
    // SAFETY: the callers pass stack views, which start with a view's fields, then these.
    unsafe { &mut *object.cast::<StackFields>() }
}

fn register_stack(object: Id) {
    let known = STACKS
        .iter()
        .any(|slot| slot.load(Ordering::Acquire) == object as usize);
    if !known {
        let _ = STACKS.iter().any(|slot| {
            slot.compare_exchange(0, object as usize, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        });
    }
}

fn insert_arranged(this: Id, view: Id, index: usize) {
    if view.is_null() {
        return;
    }
    register_stack(this);
    let fields = stack(this);
    if fields.count >= MOST_ARRANGED {
        return;
    }
    let index = index.min(fields.count);
    fields.arranged.copy_within(index..fields.count, index + 1);
    fields.arranged[index] = view;
    fields.count += 1;
    send1(this, sel!("addSubview:"), view as usize);
}

extern "C" fn stack_add_arranged(this: Id, _: Sel, view: Id) {
    insert_arranged(this, view, MOST_ARRANGED);
}

extern "C" fn stack_add_in_gravity(this: Id, _: Sel, view: Id, _: i64) {
    insert_arranged(this, view, MOST_ARRANGED);
}

extern "C" fn stack_insert_arranged(this: Id, _: Sel, view: Id, index: usize) {
    insert_arranged(this, view, index);
}

extern "C" fn stack_remove_arranged(this: Id, _: Sel, view: Id) {
    let fields = stack(this);
    let count = fields.count;
    if let Some(index) = fields.arranged[..count].iter().position(|&each| each == view) {
        fields.arranged.copy_within(index + 1..count, index);
        fields.count -= 1;
        fields.arranged[fields.count] = core::ptr::null_mut();
    }
}

extern "C" fn stack_with_views(class: Id, _: Sel, views: Id) -> Id {
    let this = send(send(class, sel!("alloc")), sel!("init"));
    if !this.is_null() {
        each_in(views, |view| insert_arranged(this, view, MOST_ARRANGED));
    }
    this
}

extern "C" fn stack_init_with_arranged(this: Id, _: Sel, views: Id) -> Id {
    let this = send(this, sel!("init"));
    if !this.is_null() {
        each_in(views, |view| insert_arranged(this, view, MOST_ARRANGED));
    }
    this
}

extern "C" fn stack_set_axis(this: Id, _: Sel, axis: i64) {
    register_stack(this);
    stack(this).vertical = u8::from(axis == 1);
}

extern "C" fn stack_axis(this: Id, _: Sel) -> i64 {
    i64::from(stack(this).vertical)
}

extern "C" fn stack_set_spacing(this: Id, _: Sel, spacing: f64) {
    stack(this).spacing = spacing;
}

extern "C" fn stack_spacing(this: Id, _: Sel) -> f64 {
    stack(this).spacing
}

extern "C" fn stack_set_distribution(this: Id, _: Sel, distribution: i64) {
    stack(this).distribution = distribution;
}

extern "C" fn stack_distribution(this: Id, _: Sel) -> i64 {
    stack(this).distribution
}

extern "C" fn stack_set_alignment(this: Id, _: Sel, alignment: i64) {
    stack(this).alignment = alignment;
}

extern "C" fn stack_alignment(this: Id, _: Sel) -> i64 {
    stack(this).alignment
}

fn arranged_of(view: Id, vertical: bool) -> Arranged {
    let frame = fields(view).frame;
    let intrinsic = intrinsic_size(view).unwrap_or_default();
    let width = if intrinsic.width > 0.0 { intrinsic.width } else { frame.width };
    let height = if intrinsic.height > 0.0 { intrinsic.height } else { frame.height };
    let (length, cross) = match vertical {
        true => (height, width),
        false => (width, height),
    };
    let (first_baseline, last_baseline) = baselines(view);
    Arranged {
        length,
        cross,
        hugging: view_priority(view, HUGGING, vertical),
        first_baseline,
        last_baseline,
    }
}

fn stack_lay_out(this: Id) {
    let fields = stack(this);
    let bounds = fields.base.frame;
    let vertical = fields.vertical != 0;
    let backwards = right_to_left();
    let shown: Vec<Id> = fields.arranged[..fields.count]
        .iter()
        .copied()
        .filter(|&view| !view.is_null() && !super::view::fields(view).hidden)
        .collect();
    let views: Vec<Arranged> = shown.iter().map(|&view| arranged_of(view, vertical)).collect();
    let alignment = match crate::uikit::is_running() {
        true => Alignment::from_uikit(fields.alignment),
        false => Alignment::from_appkit(fields.alignment),
    };
    let (length, cross) = match vertical {
        true => (bounds.height, bounds.width),
        false => (bounds.width, bounds.height),
    };
    let slots = auto_layout::stack(
        &views,
        (length, cross),
        fields.spacing,
        Distribution::from_raw(fields.distribution),
        alignment,
        !vertical,
    );
    let across = |start: f64, length: f64, room: f64| match backwards {
        true => room - (start + length),
        false => start,
    };
    for (&view, slot) in shown.iter().zip(slots) {
        super::view::fields(view).frame = match vertical {
            true => Frame {
                x: across(slot.cross_start, slot.cross_length, bounds.width),
                y: bounds.height - (slot.start + slot.length),
                width: slot.cross_length,
                height: slot.length,
            },
            false => Frame {
                x: across(slot.start, slot.length, bounds.width),
                y: bounds.height - (slot.cross_start + slot.cross_length),
                width: slot.length,
                height: slot.cross_length,
            },
        };
    }
}

extern "C" fn guide_is_view(this: Id, _: Sel) -> Id {
    this
}

/// Gives every view its anchors, `translatesAutoresizingMaskIntoConstraints`, its hugging and
/// compression-resistance priorities, its baselines and the layout direction, and the
/// application its layout direction.
pub fn extend_views(runtime: &mut Runtime) -> bool {
    let view_methods = methods![
        "leftAnchor" => left_anchor,
        "rightAnchor" => right_anchor,
        "topAnchor" => top_anchor,
        "bottomAnchor" => bottom_anchor,
        "leadingAnchor" => leading_anchor,
        "trailingAnchor" => trailing_anchor,
        "widthAnchor" => width_anchor,
        "heightAnchor" => height_anchor,
        "centerXAnchor" => center_x_anchor,
        "centerYAnchor" => center_y_anchor,
        "firstBaselineAnchor" => first_baseline_anchor,
        "lastBaselineAnchor" => last_baseline_anchor,
        "translatesAutoresizingMaskIntoConstraints" => yes,
        "setTranslatesAutoresizingMaskIntoConstraints:" => ignore_flag,
        "safeAreaLayoutGuide" => guide_is_view,
        "layoutMarginsGuide" => guide_is_view,
        "readableContentGuide" => guide_is_view,
        "setContentHuggingPriority:forOrientation:" => set_hugging,
        "setContentHuggingPriority:forAxis:" => set_hugging,
        "contentHuggingPriorityForOrientation:" => hugging,
        "contentHuggingPriorityForAxis:" => hugging,
        "setContentCompressionResistancePriority:forOrientation:" => set_resistance,
        "setContentCompressionResistancePriority:forAxis:" => set_resistance,
        "contentCompressionResistancePriorityForOrientation:" => resistance,
        "contentCompressionResistancePriorityForAxis:" => resistance,
        "firstBaselineOffsetFromTop" => first_baseline_from_top,
        "lastBaselineOffsetFromBottom" => last_baseline_from_bottom,
        "baselineOffsetFromBottom" => last_baseline_from_bottom,
        "userInterfaceLayoutDirection" => layout_direction,
        "effectiveUserInterfaceLayoutDirection" => layout_direction,
        "semanticContentAttribute" => semantic_content_attribute,
        "setSemanticContentAttribute:" => ignore_number,
        "setUserInterfaceLayoutDirection:" => ignore_number,
    ];
    let view_class_methods = methods![
        "userInterfaceLayoutDirectionForSemanticContentAttribute:" =>
            layout_direction_for_attribute,
    ];
    let application_methods = methods![
        "userInterfaceLayoutDirection" => layout_direction,
    ];
    class::extend(runtime, &NSVIEW, &view_methods, &view_class_methods)
        && class::extend(runtime, &NSAPPLICATION, &application_methods, &[])
}

pub fn define(runtime: &mut Runtime) -> bool {
    let anchor_methods = methods![
        "constraintEqualToAnchor:" => equal_to_anchor,
        "constraintEqualToAnchor:constant:" => equal_to_anchor_constant,
        "constraintEqualToAnchor:multiplier:" => equal_to_anchor_multiplier,
        "constraintEqualToAnchor:multiplier:constant:" => equal_to_anchor_multiplier_constant,
        "constraintEqualToConstant:" => equal_to_constant,
        "constraintEqualToSystemSpacingAfterAnchor:multiplier:" => equal_spacing,
        "constraintEqualToSystemSpacingBelowAnchor:multiplier:" => equal_spacing,
        "constraintGreaterThanOrEqualToAnchor:" => at_least_anchor,
        "constraintGreaterThanOrEqualToAnchor:constant:" => at_least_anchor_constant,
        "constraintGreaterThanOrEqualToAnchor:multiplier:" => at_least_anchor_multiplier,
        "constraintGreaterThanOrEqualToAnchor:multiplier:constant:" =>
            at_least_anchor_multiplier_constant,
        "constraintGreaterThanOrEqualToConstant:" => at_least_constant,
        "constraintGreaterThanOrEqualToSystemSpacingAfterAnchor:multiplier:" => at_least_spacing,
        "constraintGreaterThanOrEqualToSystemSpacingBelowAnchor:multiplier:" => at_least_spacing,
        "constraintLessThanOrEqualToAnchor:" => at_most_anchor,
        "constraintLessThanOrEqualToAnchor:constant:" => at_most_anchor_constant,
        "constraintLessThanOrEqualToAnchor:multiplier:" => at_most_anchor_multiplier,
        "constraintLessThanOrEqualToAnchor:multiplier:constant:" =>
            at_most_anchor_multiplier_constant,
        "constraintLessThanOrEqualToConstant:" => at_most_constant,
        "constraintLessThanOrEqualToSystemSpacingAfterAnchor:multiplier:" => at_most_spacing,
        "constraintLessThanOrEqualToSystemSpacingBelowAnchor:multiplier:" => at_most_spacing,
    ];
    let constraint_methods = methods![
        "isActive" => constraint_is_active,
        "setActive:" => constraint_set_active,
        "constant" => constraint_constant,
        "setConstant:" => constraint_set_constant,
        "priority" => constraint_priority,
        "setPriority:" => constraint_set_priority,
        "relation" => constraint_relation,
        "multiplier" => constraint_multiplier,
        "firstItem" => constraint_first_item,
        "secondItem" => constraint_second_item,
        "firstAttribute" => constraint_first_attribute,
        "secondAttribute" => constraint_second_attribute,
        "identifier" => no_identifier,
        "setIdentifier:" => ignore_identifier,
    ];
    let constraint_class_methods = methods![
        "activateConstraints:" => activate_constraints,
        "deactivateConstraints:" => deactivate_constraints,
        "constraintWithItem:attribute:relatedBy:toItem:attribute:multiplier:constant:" => with_item,
    ];
    let stack_methods = methods![
        "addArrangedSubview:" => stack_add_arranged,
        "addView:inGravity:" => stack_add_in_gravity,
        "insertArrangedSubview:atIndex:" => stack_insert_arranged,
        "removeArrangedSubview:" => stack_remove_arranged,
        "initWithArrangedSubviews:" => stack_init_with_arranged,
        "setAxis:" => stack_set_axis,
        "axis" => stack_axis,
        "setOrientation:" => stack_set_axis,
        "orientation" => stack_axis,
        "setSpacing:" => stack_set_spacing,
        "spacing" => stack_spacing,
        "setAlignment:" => stack_set_alignment,
        "alignment" => stack_alignment,
        "setDistribution:" => stack_set_distribution,
        "distribution" => stack_distribution,
    ];
    let stack_class_methods = methods![
        "stackViewWithViews:" => stack_with_views,
    ];
    super::define(
        runtime,
        b"NSStackView\0",
        (&NSSTACKVIEW, &NSSTACKVIEW_META),
        &NSVIEW,
        core::mem::size_of::<StackFields>(),
        &stack_methods,
        &stack_class_methods,
    ) && super::define(
        runtime,
        b"NSLayoutAnchor\0",
        (&NSLAYOUTANCHOR, &NSLAYOUTANCHOR_META),
        &NSOBJECT,
        core::mem::size_of::<AnchorFields>(),
        &anchor_methods,
        &[],
    ) && super::define(
        runtime,
        b"NSLayoutConstraint\0",
        (&NSLAYOUTCONSTRAINT, &NSLAYOUTCONSTRAINT_META),
        &NSOBJECT,
        core::mem::size_of::<ConstraintFields>(),
        &constraint_methods,
        &constraint_class_methods,
    )
}
