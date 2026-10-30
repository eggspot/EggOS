//! NSTimer and NSRunLoop (D175): a timer fires at a date, once or every interval, sending its
//! selector to its target (with the timer) or running its block. Scheduled timers are the main
//! run loop's, kept in fire order: AppKit's event loop waits for the window server's records
//! only until the next one is due and fires those that are (`next_due_in_ms`, `fire_due`); a
//! console program runs the loop with `-[NSRunLoop run]`, `runUntilDate:` or
//! `runMode:beforeDate:`, which sleep until each timer is due. A repeating timer that fell
//! behind fires once and moves to its next date after now, as macOS's does.

use super::date::{self, new_date};
use super::{
    Definition, NSDATE, NSRUNLOOP, NSRUNLOOP_META, NSTIMER, NSTIMER_META, autoreleased,
    class_pointer, define as define_class, is_date, new_instance,
};
use crate::libsystem::Exclusive;
use crate::linux;
use crate::objc::abi::{ClassObject, Id, Sel};
use crate::objc::block::invoke_address;
use crate::objc::buffer::Buffer;
use crate::objc::reference_count::{dispose, objc_release, objc_retain};
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, methods, send, send1};
use core::sync::atomic::{AtomicUsize, Ordering};

const SHORTEST_INTERVAL: f64 = 0.0001;
const LONGEST_SLEEP: f64 = 3600.0;

#[repr(C)]
#[derive(Debug)]
struct TimerObject {
    isa: usize,
    fire: f64,
    interval: f64,
    repeats: bool,
    valid: bool,
    scheduled: bool,
    target: Id,
    selector: usize,
    user_info: Id,
    block: Id,
}

fn fields(timer: Id) -> &'static mut TimerObject {
    // SAFETY: the callers pass NSTimer instances, whose memory starts with these fields, used
    // from the thread that runs the main run loop.
    unsafe { &mut *timer.cast::<TimerObject>() }
}

static SCHEDULED: Exclusive<Buffer<Id>> = Exclusive::new(Buffer::new());
static MAIN_LOOP: AtomicUsize = AtomicUsize::new(0);

fn retained(object: Id) -> Id {
    match object.is_null() {
        true => object,
        false => objc_retain(object),
    }
}

fn new_timer(class: *mut ClassObject, interval: f64, repeats: bool) -> Id {
    let timer = new_instance(class);
    if timer.is_null() {
        return timer;
    }
    let fields = fields(timer);
    fields.interval = interval.max(SHORTEST_INTERVAL);
    fields.fire = date::now() + interval.max(0.0);
    fields.repeats = repeats;
    fields.valid = true;
    timer
}

fn with_target(timer: Id, target: Id, selector: Sel, user_info: Id) -> Id {
    if !timer.is_null() {
        let fields = fields(timer);
        fields.target = retained(target);
        fields.selector = selector as usize;
        fields.user_info = retained(user_info);
    }
    timer
}

fn with_block(timer: Id, block: Id) -> Id {
    if !timer.is_null() && !block.is_null() {
        fields(timer).block = send(block, crate::objc::sel!("copy"));
    }
    timer
}

/// Adds `timer` to the main run loop, which keeps a reference to it while it is valid.
pub fn schedule(timer: Id) {
    if timer.is_null() || !fields(timer).valid || fields(timer).scheduled {
        return;
    }
    objc_retain(timer);
    fields(timer).scheduled = true;
    SCHEDULED.with(|timers| timers.push(timer));
}

fn unschedule(timer: Id) {
    let removed = SCHEDULED.with(|timers| {
        let list = timers.as_mut_slice();
        match list.iter().position(|&each| each == timer) {
            Some(at) => {
                list.copy_within(at + 1.., at);
                timers.pop();
                true
            }
            None => false,
        }
    });
    if removed {
        fields(timer).scheduled = false;
        objc_release(timer);
    }
}

fn next_fire() -> Option<f64> {
    SCHEDULED.with(|timers| {
        timers
            .as_slice()
            .iter()
            .map(|&timer| fields(timer).fire)
            .reduce(f64::min)
    })
}

/// How many milliseconds until the next scheduled timer is due (0 when one is), `None` when
/// none is scheduled.
pub fn next_due_in_ms() -> Option<u64> {
    let fire = next_fire()?;
    let wait = (fire - date::now()).clamp(0.0, LONGEST_SLEEP);
    let milliseconds = wait * 1000.0;
    let whole = milliseconds as u64;
    Some(whole + u64::from((whole as f64) < milliseconds))
}

fn fire(timer: Id) {
    let (target, selector, block) = {
        let fields = fields(timer);
        (fields.target, fields.selector, fields.block)
    };
    if !block.is_null() {
        // SAFETY: a timer's block takes the block and the timer.
        let function: extern "C" fn(Id, Id) =
            unsafe { core::mem::transmute(invoke_address(block)) };
        function(block, timer);
    } else if !target.is_null() && selector != 0 {
        send1(target, selector as Sel, timer as usize);
    }
}

/// Fires every scheduled timer that is due, in date order: a repeating one moves to its next
/// date after now, the others end.
pub fn fire_due() {
    let now = date::now();
    let mut due = SCHEDULED.with(|timers| {
        let mut due = Buffer::new();
        for &timer in timers.as_slice() {
            if fields(timer).fire <= now {
                objc_retain(timer);
                due.push(timer);
            }
        }
        due
    });
    due.as_mut_slice()
        .sort_unstable_by(|&left, &right| fields(left).fire.total_cmp(&fields(right).fire));
    for &timer in due.as_slice() {
        if fields(timer).valid && fields(timer).scheduled {
            let fields_now = fields(timer);
            if fields_now.repeats {
                let interval = fields_now.interval;
                let lag = ((now - fields_now.fire) / interval).max(0.0);
                let behind = lag as u64 as f64 + 1.0;
                fields_now.fire += behind * interval;
            }
            fire(timer);
            if !fields(timer).repeats {
                invalidate_timer(timer);
            }
        }
        objc_release(timer);
    }
    due.free();
}

fn invalidate_timer(timer: Id) {
    let fields = fields(timer);
    if !fields.valid {
        return;
    }
    fields.valid = false;
    let (target, user_info, block) = (fields.target, fields.user_info, fields.block);
    fields.target = core::ptr::null_mut();
    fields.user_info = core::ptr::null_mut();
    fields.block = core::ptr::null_mut();
    unschedule(timer);
    for object in [target, user_info, block] {
        if !object.is_null() {
            objc_release(object);
        }
    }
}

fn this_class(this: Id) -> *mut ClassObject {
    this.cast()
}

extern "C" fn scheduled_with_target(
    this: Id,
    _: Sel,
    interval: f64,
    target: Id,
    selector: Sel,
    user_info: Id,
    repeats: u8,
) -> Id {
    let timer = with_target(
        new_timer(this_class(this), interval, repeats != 0),
        target,
        selector,
        user_info,
    );
    schedule(timer);
    autoreleased(timer)
}

extern "C" fn timer_with_target(
    this: Id,
    _: Sel,
    interval: f64,
    target: Id,
    selector: Sel,
    user_info: Id,
    repeats: u8,
) -> Id {
    let timer = with_target(
        new_timer(this_class(this), interval, repeats != 0),
        target,
        selector,
        user_info,
    );
    autoreleased(timer)
}

extern "C" fn scheduled_with_block(this: Id, _: Sel, interval: f64, repeats: u8, block: Id) -> Id {
    let timer = with_block(new_timer(this_class(this), interval, repeats != 0), block);
    schedule(timer);
    autoreleased(timer)
}

extern "C" fn timer_with_block(this: Id, _: Sel, interval: f64, repeats: u8, block: Id) -> Id {
    autoreleased(with_block(
        new_timer(this_class(this), interval, repeats != 0),
        block,
    ))
}

extern "C" fn invalidate(this: Id, _: Sel) {
    invalidate_timer(this);
}

extern "C" fn is_valid(this: Id, _: Sel) -> u8 {
    u8::from(fields(this).valid)
}

extern "C" fn fire_now(this: Id, _: Sel) {
    if !fields(this).valid {
        return;
    }
    fire(this);
    if !fields(this).repeats {
        invalidate_timer(this);
    }
}

extern "C" fn time_interval(this: Id, _: Sel) -> f64 {
    match fields(this).repeats {
        true => fields(this).interval,
        false => 0.0,
    }
}

extern "C" fn user_info(this: Id, _: Sel) -> Id {
    fields(this).user_info
}

extern "C" fn fire_date(this: Id, _: Sel) -> Id {
    autoreleased(new_date(class_pointer(&NSDATE), fields(this).fire))
}

extern "C" fn set_fire_date(this: Id, _: Sel, date: Id) {
    if !date.is_null() && is_date(date) {
        let seconds: extern "C" fn(Id, Sel) -> f64 = crate::objc::message::msg_send_as();
        fields(this).fire = seconds(date, crate::objc::sel!("timeIntervalSinceReferenceDate"));
    }
}

extern "C" fn dealloc(this: Id, _: Sel) {
    invalidate_timer(this);
    dispose(this);
}

extern "C" fn main_run_loop(_: Id, _: Sel) -> Id {
    let existing = MAIN_LOOP.load(Ordering::Acquire);
    if existing != 0 {
        return existing as Id;
    }
    let made = new_instance(class_pointer(&NSRUNLOOP));
    match MAIN_LOOP.compare_exchange(0, made as usize, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => made,
        Err(winner) => winner as Id,
    }
}

extern "C" fn add_timer(_: Id, _: Sel, timer: Id, _: Id) {
    schedule(timer);
}

fn sleep_until(fire: f64) {
    let wait = (fire - date::now()).clamp(0.0, LONGEST_SLEEP);
    if wait > 0.0 {
        let seconds = wait as i64;
        let _ = linux::sleep([seconds, ((wait - seconds as f64) * 1e9) as i64]);
    }
}

fn run_until(limit: f64, once: bool) -> bool {
    loop {
        let Some(fire) = next_fire() else {
            return false;
        };
        if fire > limit {
            sleep_until(limit);
            return false;
        }
        sleep_until(fire);
        fire_due();
        if once || date::now() >= limit {
            return true;
        }
    }
}

fn seconds_of(date: Id) -> f64 {
    if date.is_null() || !is_date(date) {
        return date::now();
    }
    let seconds: extern "C" fn(Id, Sel) -> f64 = crate::objc::message::msg_send_as();
    seconds(date, crate::objc::sel!("timeIntervalSinceReferenceDate"))
}

extern "C" fn run(_: Id, _: Sel) {
    run_until(f64::INFINITY, false);
}

extern "C" fn run_until_date(_: Id, _: Sel, date: Id) {
    run_until(seconds_of(date), false);
}

extern "C" fn run_mode_before_date(_: Id, _: Sel, _: Id, date: Id) -> u8 {
    u8::from(run_until(seconds_of(date), true))
}

extern "C" fn this_object(this: Id, _: Sel) -> Id {
    this
}

extern "C" fn nothing(_: Id, _: Sel) {}

pub fn define(runtime: &mut Runtime) -> bool {
    let timer_instance = methods![
        "invalidate" => invalidate,
        "isValid" => is_valid,
        "fire" => fire_now,
        "timeInterval" => time_interval,
        "userInfo" => user_info,
        "fireDate" => fire_date,
        "setFireDate:" => set_fire_date,
        "dealloc" => dealloc,
    ];
    let timer_class = methods![
        "scheduledTimerWithTimeInterval:target:selector:userInfo:repeats:" => scheduled_with_target,
        "timerWithTimeInterval:target:selector:userInfo:repeats:" => timer_with_target,
        "scheduledTimerWithTimeInterval:repeats:block:" => scheduled_with_block,
        "timerWithTimeInterval:repeats:block:" => timer_with_block,
    ];
    let loop_instance = methods![
        "addTimer:forMode:" => add_timer,
        "run" => run,
        "runUntilDate:" => run_until_date,
        "runMode:beforeDate:" => run_mode_before_date,
        "retain" => this_object,
        "release" => nothing,
        "autorelease" => this_object,
    ];
    let loop_class = methods![
        "currentRunLoop" => main_run_loop,
        "mainRunLoop" => main_run_loop,
    ];
    define_class(
        runtime,
        &Definition {
            name: b"NSTimer\0",
            class: &NSTIMER,
            meta: &NSTIMER_META,
            superclass: &NSOBJECT,
            instance_size: core::mem::size_of::<TimerObject>(),
            flags: 0,
            instance_methods: &timer_instance,
            class_methods: &timer_class,
        },
    ) && define_class(
        runtime,
        &Definition {
            name: b"NSRunLoop\0",
            class: &NSRUNLOOP,
            meta: &NSRUNLOOP_META,
            superclass: &NSOBJECT,
            instance_size: core::mem::size_of::<usize>(),
            flags: 0,
            instance_methods: &loop_instance,
            class_methods: &loop_class,
        },
    )
}
