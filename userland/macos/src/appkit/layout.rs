//! Auto Layout, the part apps use most (D506): a view's anchors (`leadingAnchor`, `topAnchor`,
//! `widthAnchor`, `centerXAnchor`, ...) make constraints (`constraintEqualToAnchor:constant:`,
//! `constraintEqualToConstant:`, and the inequalities, kept as equalities), as does
//! `constraintWithItem:attribute:relatedBy:toItem:attribute:multiplier:constant:`; active
//! constraints (`active`, `activateConstraints:`) are applied before a window draws. Each pass
//! reads every constrained view's wanted edges, sizes and centres from the others' frames (any
//! two items, through their places in the window) and sets its frame from them, keeping its
//! current size along an axis no constraint sizes, or its intrinsic size when it answers one;
//! three passes settle chains of views placed by their neighbours. Constraints are kept in a
//! table of 512, the first come.

use super::view::fields;
use super::{
    NSLAYOUTANCHOR, NSLAYOUTANCHOR_META, NSLAYOUTCONSTRAINT, NSLAYOUTCONSTRAINT_META, NSVIEW,
};
use crate::foundation::{class_pointer, new_instance};
use crate::objc::abi::{Id, Sel};
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, class, message, methods, sel, send, send1, send_bool};
use core::sync::atomic::{AtomicUsize, Ordering};
use eggos_macos_gui::geometry::{Frame, Point, Size};

/// `NSLayoutAttribute`.
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
}

const MOST_CONSTRAINTS: usize = 512;
const PASSES: usize = 3;

static ACTIVE: [AtomicUsize; MOST_CONSTRAINTS] =
    [const { AtomicUsize::new(0) }; MOST_CONSTRAINTS];

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
}

fn anchor(object: Id) -> &'static mut AnchorFields {
    // SAFETY: the callers pass NSLayoutAnchor instances, whose memory starts with these fields.
    unsafe { &mut *object.cast::<AnchorFields>() }
}

fn constraint(object: Id) -> &'static mut ConstraintFields {
    // SAFETY: the callers pass NSLayoutConstraint instances, whose memory starts with these
    // fields.
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
}

fn to_anchor(this: Id, other: Id, multiplier: f64, constant: f64) -> Id {
    let own = anchor(this);
    let (view, kind) = (own.view, own.attribute);
    match other.is_null() {
        true => new_constraint((view, kind), (core::ptr::null_mut(), 0), 0.0, constant),
        false => {
            let other = anchor(other);
            new_constraint((view, kind), (other.view, other.attribute), multiplier, constant)
        }
    }
}

extern "C" fn equal_to_anchor(this: Id, _: Sel, other: Id) -> Id {
    to_anchor(this, other, 1.0, 0.0)
}

extern "C" fn equal_to_anchor_constant(this: Id, _: Sel, other: Id, constant: f64) -> Id {
    to_anchor(this, other, 1.0, constant)
}

extern "C" fn equal_to_anchor_multiplier(this: Id, _: Sel, other: Id, multiplier: f64) -> Id {
    to_anchor(this, other, multiplier, 0.0)
}

extern "C" fn equal_to_anchor_multiplier_constant(
    this: Id,
    _: Sel,
    other: Id,
    multiplier: f64,
    constant: f64,
) -> Id {
    to_anchor(this, other, multiplier, constant)
}

extern "C" fn equal_to_constant(this: Id, _: Sel, constant: f64) -> Id {
    to_anchor(this, core::ptr::null_mut(), 0.0, constant)
}

#[allow(clippy::too_many_arguments)]
extern "C" fn with_item(
    _: Id,
    _: Sel,
    first: Id,
    first_attribute: u64,
    _relation: i64,
    second: Id,
    second_attribute: u64,
    multiplier: f64,
    constant: f64,
) -> Id {
    new_constraint((first, first_attribute), (second, second_attribute), multiplier, constant)
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
                let _ = slot.compare_exchange(
                    object as usize,
                    0,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
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

fn read(frame: Frame, kind: u64) -> Option<f64> {
    Some(match kind {
        attribute::LEFT | attribute::LEADING => frame.x,
        attribute::RIGHT | attribute::TRAILING => frame.x + frame.width,
        attribute::BOTTOM => frame.y,
        attribute::TOP => frame.y + frame.height,
        attribute::WIDTH => frame.width,
        attribute::HEIGHT => frame.height,
        attribute::CENTER_X => frame.x + frame.width / 2.0,
        attribute::CENTER_Y => frame.y + frame.height / 2.0,
        _ => return None,
    })
}

/// What one pass wants of a view along one axis: its low edge, high edge, size and centre.
#[derive(Default, Clone, Copy)]
struct Axis {
    low: Option<f64>,
    high: Option<f64>,
    size: Option<f64>,
    centre: Option<f64>,
}

impl Axis {
    fn solve(self, origin: f64, size: f64) -> (f64, f64) {
        let size = match (self.size, self.low, self.high) {
            (Some(size), ..) => size,
            (None, Some(low), Some(high)) => high - low,
            _ => size,
        };
        let origin = match (self.low, self.high, self.centre) {
            (Some(low), ..) => low,
            (None, Some(high), _) => high - size,
            (None, None, Some(centre)) => centre - size / 2.0,
            (None, None, None) => origin,
        };
        (origin, size.max(0.0))
    }
}

fn intrinsic_size(view: Id) -> Option<Size> {
    if !send_bool(view, sel!("respondsToSelector:"), sel!("intrinsicContentSize") as usize) {
        return None;
    }
    let size: extern "C" fn(Id, Sel) -> Size = message::msg_send_as();
    let size = size(view, sel!("intrinsicContentSize"));
    (size.width > 0.0 || size.height > 0.0).then_some(size)
}

fn lay_out(view: Id) {
    let superview = fields(view).superview;
    let mut horizontal = Axis::default();
    let mut vertical = Axis::default();
    for slot in &ACTIVE {
        let object = slot.load(Ordering::Acquire) as Id;
        if object.is_null() || constraint(object).first != view {
            continue;
        }
        let rule = constraint(object);
        let source = match rule.second.is_null() {
            true => Some(0.0),
            false => read(frame_in(rule.second, superview), rule.second_attribute),
        };
        let Some(source) = source else {
            continue;
        };
        let wanted = source * rule.multiplier + rule.constant;
        match rule.first_attribute {
            attribute::LEFT | attribute::LEADING => horizontal.low = Some(wanted),
            attribute::RIGHT | attribute::TRAILING => horizontal.high = Some(wanted),
            attribute::WIDTH => horizontal.size = Some(wanted),
            attribute::CENTER_X => horizontal.centre = Some(wanted),
            attribute::BOTTOM => vertical.low = Some(wanted),
            attribute::TOP => vertical.high = Some(wanted),
            attribute::HEIGHT => vertical.size = Some(wanted),
            attribute::CENTER_Y => vertical.centre = Some(wanted),
            _ => {}
        }
    }
    let frame = fields(view).frame;
    let intrinsic = intrinsic_size(view);
    let width = match (frame.width, intrinsic) {
        (width, Some(size)) if width <= 0.0 => size.width,
        (width, _) => width,
    };
    let height = match (frame.height, intrinsic) {
        (height, Some(size)) if height <= 0.0 => size.height,
        (height, _) => height,
    };
    let (x, width) = horizontal.solve(frame.x, width);
    let (y, height) = vertical.solve(frame.y, height);
    fields(view).frame = Frame {
        x,
        y,
        width,
        height,
    };
}

/// Applies the active constraints to the views they place (before a window draws).
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
    }
}

/// Gives every view its anchors and `translatesAutoresizingMaskIntoConstraints`.
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
        "translatesAutoresizingMaskIntoConstraints" => yes,
        "setTranslatesAutoresizingMaskIntoConstraints:" => ignore_flag,
    ];
    class::extend(runtime, &NSVIEW, &view_methods, &[])
}

pub fn define(runtime: &mut Runtime) -> bool {
    let anchor_methods = methods![
        "constraintEqualToAnchor:" => equal_to_anchor,
        "constraintEqualToAnchor:constant:" => equal_to_anchor_constant,
        "constraintGreaterThanOrEqualToAnchor:" => equal_to_anchor,
        "constraintGreaterThanOrEqualToAnchor:constant:" => equal_to_anchor_constant,
        "constraintLessThanOrEqualToAnchor:" => equal_to_anchor,
        "constraintLessThanOrEqualToAnchor:constant:" => equal_to_anchor_constant,
        "constraintEqualToAnchor:multiplier:" => equal_to_anchor_multiplier,
        "constraintEqualToAnchor:multiplier:constant:" => equal_to_anchor_multiplier_constant,
        "constraintEqualToConstant:" => equal_to_constant,
        "constraintGreaterThanOrEqualToConstant:" => equal_to_constant,
        "constraintLessThanOrEqualToConstant:" => equal_to_constant,
    ];
    let constraint_methods = methods![
        "isActive" => constraint_is_active,
        "setActive:" => constraint_set_active,
        "constant" => constraint_constant,
        "setConstant:" => constraint_set_constant,
    ];
    let constraint_class_methods = methods![
        "activateConstraints:" => activate_constraints,
        "deactivateConstraints:" => deactivate_constraints,
        "constraintWithItem:attribute:relatedBy:toItem:attribute:multiplier:constant:" => with_item,
    ];
    super::define(
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
