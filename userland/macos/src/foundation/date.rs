//! NSDate (D175): a moment as seconds since the reference date, 2001-01-01 00:00:00 UTC, read
//! from the real-time clock; created now, relative to now, to 1970 or to the reference date;
//! compared, offset and described as macOS describes it, `2026-10-30 12:34:56 +0000`.

use super::{
    Definition, NSDATE, NSDATE_META, autoreleased, define as define_class, is_date, new_instance,
    string_from_utf8,
};
use crate::calendar;
use crate::linux;
use crate::objc::abi::{ClassObject, Id, Sel};
use crate::objc::reference_count::objc_retain;
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, methods};

/// Seconds from 1970-01-01 to the reference date, 2001-01-01.
pub const REFERENCE_FROM_1970: f64 = 978_307_200.0;
const DISTANT: f64 = 63_113_904_000.0;
const DESCRIPTION_FORMAT: &[u8] = b"%Y-%m-%d %H:%M:%S +0000";
const ASCENDING: isize = -1;
const SAME: isize = 0;
const DESCENDING: isize = 1;

#[repr(C)]
#[derive(Debug)]
struct DateObject {
    isa: usize,
    seconds: f64,
}

fn seconds_of(date: Id) -> f64 {
    // SAFETY: the callers pass NSDate instances, whose memory starts with these fields.
    unsafe { (*date.cast::<DateObject>()).seconds }
}

/// Now, in seconds since the reference date.
pub fn now() -> f64 {
    let [seconds, nanoseconds] = linux::clock_time(0).unwrap_or([0, 0]);
    seconds as f64 + nanoseconds as f64 / 1e9 - REFERENCE_FROM_1970
}

/// A new date of `class` (counted once) at `seconds` since the reference date.
pub fn new_date(class: *mut ClassObject, seconds: f64) -> Id {
    let date = new_instance(class);
    if !date.is_null() {
        // SAFETY: a new NSDate instance.
        unsafe { (*date.cast::<DateObject>()).seconds = seconds };
    }
    date
}

fn this_class(this: Id) -> *mut ClassObject {
    this.cast()
}

extern "C" fn date(this: Id, _: Sel) -> Id {
    autoreleased(new_date(this_class(this), now()))
}

extern "C" fn date_since_now(this: Id, _: Sel, interval: f64) -> Id {
    autoreleased(new_date(this_class(this), now() + interval))
}

extern "C" fn date_since_1970(this: Id, _: Sel, interval: f64) -> Id {
    autoreleased(new_date(this_class(this), interval - REFERENCE_FROM_1970))
}

extern "C" fn date_since_reference(this: Id, _: Sel, interval: f64) -> Id {
    autoreleased(new_date(this_class(this), interval))
}

extern "C" fn distant_future(this: Id, _: Sel) -> Id {
    autoreleased(new_date(this_class(this), DISTANT))
}

extern "C" fn distant_past(this: Id, _: Sel) -> Id {
    autoreleased(new_date(this_class(this), -DISTANT))
}

extern "C" fn class_since_reference(_: Id, _: Sel) -> f64 {
    now()
}

fn set(this: Id, seconds: f64) -> Id {
    if !this.is_null() {
        // SAFETY: an NSDate instance being initialized.
        unsafe { (*this.cast::<DateObject>()).seconds = seconds };
    }
    this
}

extern "C" fn init(this: Id, _: Sel) -> Id {
    set(this, now())
}

extern "C" fn init_since_now(this: Id, _: Sel, interval: f64) -> Id {
    set(this, now() + interval)
}

extern "C" fn init_since_1970(this: Id, _: Sel, interval: f64) -> Id {
    set(this, interval - REFERENCE_FROM_1970)
}

extern "C" fn init_since_reference(this: Id, _: Sel, interval: f64) -> Id {
    set(this, interval)
}

extern "C" fn since_reference(this: Id, _: Sel) -> f64 {
    seconds_of(this)
}

extern "C" fn since_1970(this: Id, _: Sel) -> f64 {
    seconds_of(this) + REFERENCE_FROM_1970
}

extern "C" fn since_now(this: Id, _: Sel) -> f64 {
    seconds_of(this) - now()
}

extern "C" fn since_date(this: Id, _: Sel, other: Id) -> f64 {
    if other.is_null() {
        return seconds_of(this);
    }
    seconds_of(this) - seconds_of(other)
}

extern "C" fn adding(this: Id, _: Sel, interval: f64) -> Id {
    let class = crate::objc::class::class_of(this);
    autoreleased(new_date(class, seconds_of(this) + interval))
}

fn order(this: Id, other: Id) -> isize {
    let (left, right) = (seconds_of(this), seconds_of(other));
    if left < right {
        ASCENDING
    } else if left > right {
        DESCENDING
    } else {
        SAME
    }
}

extern "C" fn compare(this: Id, _: Sel, other: Id) -> isize {
    if other.is_null() || !is_date(other) {
        return DESCENDING;
    }
    order(this, other)
}

extern "C" fn is_equal(this: Id, _: Sel, other: Id) -> u8 {
    u8::from(!other.is_null() && is_date(other) && order(this, other) == SAME)
}

extern "C" fn earlier(this: Id, _: Sel, other: Id) -> Id {
    match !other.is_null() && is_date(other) && order(other, this) == ASCENDING {
        true => other,
        false => this,
    }
}

extern "C" fn later(this: Id, _: Sel, other: Id) -> Id {
    match !other.is_null() && is_date(other) && order(other, this) == DESCENDING {
        true => other,
        false => this,
    }
}

extern "C" fn hash(this: Id, _: Sel) -> usize {
    seconds_of(this) as i64 as usize
}

/// The date `seconds` after the reference date as macOS describes one, into `buffer`.
pub fn describe(seconds: f64, buffer: &mut [u8; 32]) -> &[u8] {
    let since_1970 = seconds + REFERENCE_FROM_1970;
    let truncated = since_1970 as i64;
    let unix = truncated - i64::from((truncated as f64) > since_1970);
    let Some(broken) = calendar::from_seconds(unix) else {
        return b"";
    };
    match calendar::format(DESCRIPTION_FORMAT, &broken, buffer) {
        Some(length) => &buffer[..length],
        None => b"",
    }
}

extern "C" fn description(this: Id, _: Sel) -> Id {
    let mut buffer = [0u8; 32];
    string_from_utf8(describe(seconds_of(this), &mut buffer))
}

extern "C" fn copy_with_zone(this: Id, _: Sel, _: usize) -> Id {
    objc_retain(this)
}

pub fn define(runtime: &mut Runtime) -> bool {
    let instance = methods![
        "init" => init,
        "initWithTimeIntervalSinceNow:" => init_since_now,
        "initWithTimeIntervalSince1970:" => init_since_1970,
        "initWithTimeIntervalSinceReferenceDate:" => init_since_reference,
        "timeIntervalSinceReferenceDate" => since_reference,
        "timeIntervalSince1970" => since_1970,
        "timeIntervalSinceNow" => since_now,
        "timeIntervalSinceDate:" => since_date,
        "dateByAddingTimeInterval:" => adding,
        "compare:" => compare,
        "isEqual:" => is_equal,
        "isEqualToDate:" => is_equal,
        "earlierDate:" => earlier,
        "laterDate:" => later,
        "hash" => hash,
        "description" => description,
        "copyWithZone:" => copy_with_zone,
    ];
    let class = methods![
        "date" => date,
        "now" => date,
        "dateWithTimeIntervalSinceNow:" => date_since_now,
        "dateWithTimeIntervalSince1970:" => date_since_1970,
        "dateWithTimeIntervalSinceReferenceDate:" => date_since_reference,
        "distantFuture" => distant_future,
        "distantPast" => distant_past,
        "timeIntervalSinceReferenceDate" => class_since_reference,
    ];
    define_class(
        runtime,
        &Definition {
            name: b"NSDate\0",
            class: &NSDATE,
            meta: &NSDATE_META,
            superclass: &NSOBJECT,
            instance_size: core::mem::size_of::<DateObject>(),
            flags: 0,
            instance_methods: &instance,
            class_methods: &class,
        },
    )
}
