//! NSTimer and NSRunLoop (D175, D232): a timer fires at a date, once or every interval, sending
//! its selector to its target (with the timer) or running its block. Every thread has its own
//! run loop (the main thread's is the main run loop), and a timer belongs to the loop it was
//! scheduled on and to the modes it was added for (the default mode unless `addTimer:forMode:`
//! names others; the common modes are the default, modal panel and event tracking modes).
//! AppKit's event loop waits for the window server's records only until the next of the main
//! loop's default-mode timers is due and fires those that are (`next_due_in_ms`, `fire_due`);
//! a program runs a loop with `-[NSRunLoop run]`, `runUntilDate:` or `runMode:beforeDate:`,
//! which sleep until each of that loop's timers in that mode is due. A repeating timer that
//! fell behind fires once and moves to its next date after now, as macOS's does.
//! `performSelector:withObject:afterDelay:` is a one-shot timer that sends the selector with
//! the object, and `cancelPreviousPerformRequestsWithTarget:` invalidates those still waiting.
//! An exception a timer's callout throws goes on to the code that ran the loop once the timer
//! is released (and a one-shot invalidated), D234.

use super::array;
use super::constants::{
    self, COMMON_RUN_LOOP_MODES, DEFAULT_RUN_LOOP_MODE, EVENT_TRACKING_RUN_LOOP_MODE,
    MODAL_PANEL_RUN_LOOP_MODE,
};
use super::date::{self, new_date};
use super::{
    Definition, NSDATE, NSRUNLOOP, NSRUNLOOP_META, NSTIMER, NSTIMER_META, autoreleased,
    class_pointer, define as define_class, is_array, is_date, new_instance,
};
use crate::libsystem::{Exclusive, thread};
use crate::linux;
use crate::objc::abi::{ClassObject, Id, Sel};
use crate::objc::block::invoke_address;
use crate::objc::buffer::Buffer;
use crate::objc::exception::{self, Caught, guard};
use crate::objc::reference_count::{dispose, objc_release, objc_retain};
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, class, methods, send, send_bool, send1};
use core::sync::atomic::{AtomicUsize, Ordering};

const SHORTEST_INTERVAL: f64 = 0.0001;
const LONGEST_SLEEP: f64 = 3600.0;
const DEFAULT_MODE: u8 = 1;
const MODAL_PANEL_MODE: u8 = 2;
const EVENT_TRACKING_MODE: u8 = 4;
const OTHER_MODE: u8 = 8;
const COMMON_MODES: u8 = DEFAULT_MODE | MODAL_PANEL_MODE | EVENT_TRACKING_MODE;

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
    argument: Id,
    performs: bool,
    modes: u8,
    run_loop: Id,
}

#[repr(C)]
#[derive(Debug)]
struct RunLoopObject {
    isa: usize,
    thread: u32,
}

fn fields(timer: Id) -> &'static mut TimerObject {
    // SAFETY: the callers pass NSTimer instances, whose memory starts with these fields, used
    // from the thread that runs the main run loop.
    unsafe { &mut *timer.cast::<TimerObject>() }
}

static SCHEDULED: Exclusive<Buffer<Id>> = Exclusive::new(Buffer::new());
static MAIN_LOOP: AtomicUsize = AtomicUsize::new(0);
static THREAD_LOOPS: Exclusive<Buffer<Id>> = Exclusive::new(Buffer::new());

fn is_main_thread() -> bool {
    thread::current_id() as usize == linux::process_id()
}

fn new_run_loop(thread: u32) -> Id {
    let made = new_instance(class_pointer(&NSRUNLOOP));
    if !made.is_null() {
        // SAFETY: a new run loop object, whose memory starts with these fields.
        unsafe { (*made.cast::<RunLoopObject>()).thread = thread };
    }
    made
}

fn loop_thread(run_loop: Id) -> u32 {
    // SAFETY: the callers pass NSRunLoop instances.
    unsafe { (*run_loop.cast::<RunLoopObject>()).thread }
}

fn main_loop() -> Id {
    let existing = MAIN_LOOP.load(Ordering::Acquire);
    if existing != 0 {
        return existing as Id;
    }
    let made = new_run_loop(linux::process_id() as u32);
    match MAIN_LOOP.compare_exchange(0, made as usize, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => made,
        Err(winner) => {
            dispose(made);
            winner as Id
        }
    }
}

fn current_loop() -> Id {
    if is_main_thread() {
        return main_loop();
    }
    let me = thread::current_id();
    let found = THREAD_LOOPS.with(|loops| {
        loops
            .as_slice()
            .iter()
            .copied()
            .find(|&each| loop_thread(each) == me)
    });
    if let Some(found) = found {
        return found;
    }
    let made = new_run_loop(me);
    if made.is_null() || !THREAD_LOOPS.with(|loops| loops.push(made)) {
        return main_loop();
    }
    made
}

fn mode_bits(mode: Id) -> u8 {
    let named = |index: usize| {
        let constant = constants::string(index);
        mode == constant || send_bool(mode, crate::objc::sel!("isEqual:"), constant as usize)
    };
    match mode.is_null() {
        true => DEFAULT_MODE,
        false if named(DEFAULT_RUN_LOOP_MODE) => DEFAULT_MODE,
        false if named(COMMON_RUN_LOOP_MODES) => COMMON_MODES,
        false if named(MODAL_PANEL_RUN_LOOP_MODE) => MODAL_PANEL_MODE,
        false if named(EVENT_TRACKING_RUN_LOOP_MODE) => EVENT_TRACKING_MODE,
        false => OTHER_MODE,
    }
}

fn modes_of_array(modes: Id) -> u8 {
    if modes.is_null() || !is_array(modes) {
        return DEFAULT_MODE;
    }
    (0..array::count_of(modes)).fold(0, |bits, index| bits | mode_bits(array::item(modes, index)))
}

fn running_mode(mode: Id) -> u8 {
    match mode_bits(mode) {
        COMMON_MODES => DEFAULT_MODE,
        bits => bits,
    }
}

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

fn schedule(timer: Id, run_loop: Id, modes: u8) {
    if timer.is_null() || !fields(timer).valid {
        return;
    }
    let fields = fields(timer);
    if fields.scheduled {
        if fields.run_loop == run_loop {
            fields.modes |= modes;
        }
        return;
    }
    if !SCHEDULED.with(|timers| timers.push(timer)) {
        return;
    }
    objc_retain(timer);
    fields.scheduled = true;
    fields.run_loop = run_loop;
    fields.modes = modes;
}

fn schedule_here(timer: Id) {
    schedule(timer, current_loop(), DEFAULT_MODE);
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

fn runs_in(timer: Id, run_loop: Id, mode: u8) -> bool {
    let fields = fields(timer);
    fields.run_loop == run_loop && fields.modes & mode != 0
}

fn next_fire(run_loop: Id, mode: u8) -> Option<f64> {
    SCHEDULED.with(|timers| {
        timers
            .as_slice()
            .iter()
            .filter(|&&timer| runs_in(timer, run_loop, mode))
            .map(|&timer| fields(timer).fire)
            .reduce(f64::min)
    })
}

/// How many milliseconds until the main run loop's next default-mode timer is due (0 when one
/// is), `None` when none is scheduled.
pub fn next_due_in_ms() -> Option<u64> {
    let fire = next_fire(main_loop(), DEFAULT_MODE)?;
    let wait = (fire - date::now()).clamp(0.0, LONGEST_SLEEP);
    let milliseconds = wait * 1000.0;
    let whole = milliseconds as u64;
    Some(whole + u64::from((whole as f64) < milliseconds))
}

fn fire(timer: Id) -> Option<Caught> {
    let (target, selector, user_info, block, argument, performs) = {
        let fields = fields(timer);
        (
            retained(fields.target),
            fields.selector,
            retained(fields.user_info),
            retained(fields.block),
            retained(fields.argument),
            fields.performs,
        )
    };
    let failure = exception::catching(&mut || {
        if performs {
            send1(target, selector as Sel, argument as usize);
        } else if !block.is_null() {
            // SAFETY: a timer's block takes the block and the timer.
            let function: extern "C" fn(Id, Id) =
                unsafe { core::mem::transmute(invoke_address(block)) };
            function(block, timer);
        } else if !target.is_null() && selector != 0 {
            send1(target, selector as Sel, timer as usize);
        }
    });
    for object in [target, user_info, block, argument] {
        if !object.is_null() {
            objc_release(object);
        }
    }
    failure
}

/// Fires every timer of the main run loop's default mode that is due.
pub fn fire_due() {
    fire_due_in(main_loop(), DEFAULT_MODE);
}

fn fire_due_in(run_loop: Id, mode: u8) {
    if run_loop == main_loop() {
        crate::libsystem::dispatch::drain_main();
    }
    let now = date::now();
    let mut due = SCHEDULED.with(|timers| {
        let mut due = Buffer::new();
        for &timer in timers.as_slice() {
            if fields(timer).fire <= now && runs_in(timer, run_loop, mode) {
                objc_retain(timer);
                due.push(timer);
            }
        }
        due
    });
    due.as_mut_slice()
        .sort_unstable_by(|&left, &right| fields(left).fire.total_cmp(&fields(right).fire));
    let mut failure = None;
    let mut released = 0;
    for &timer in due.as_slice() {
        released += 1;
        if fields(timer).valid && fields(timer).scheduled {
            let fields_now = fields(timer);
            if fields_now.repeats {
                let interval = fields_now.interval;
                let lag = ((now - fields_now.fire) / interval).max(0.0);
                let behind = lag as u64 as f64 + 1.0;
                fields_now.fire += behind * interval;
            }
            failure = fire(timer);
            if !fields(timer).repeats {
                invalidate_timer(timer);
            }
        }
        objc_release(timer);
        if failure.is_some() {
            break;
        }
    }
    for &rest in &due.as_slice()[released..] {
        objc_release(rest);
    }
    due.free();
    if let Some(caught) = failure {
        caught.rethrow();
    }
}

fn invalidate_timer(timer: Id) {
    let fields = fields(timer);
    if !fields.valid {
        return;
    }
    fields.valid = false;
    let (target, user_info, block) = (fields.target, fields.user_info, fields.block);
    let argument = fields.argument;
    fields.target = core::ptr::null_mut();
    fields.user_info = core::ptr::null_mut();
    fields.block = core::ptr::null_mut();
    fields.argument = core::ptr::null_mut();
    unschedule(timer);
    for object in [target, user_info, block, argument] {
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
    schedule_here(timer);
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
    schedule_here(timer);
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
    let failure = fire(this);
    if !fields(this).repeats {
        invalidate_timer(this);
    }
    if let Some(caught) = failure {
        caught.rethrow();
    }
}

guard!(eggos_guarded_fire_now => fire_now);
guard!(eggos_guarded_run => run);
guard!(eggos_guarded_run_until_date => run_until_date);
guard!(eggos_guarded_run_mode_before_date => run_mode_before_date);

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
    main_loop()
}

extern "C" fn current_run_loop(_: Id, _: Sel) -> Id {
    current_loop()
}

extern "C" fn add_timer(this: Id, _: Sel, timer: Id, mode: Id) {
    schedule(timer, this, mode_bits(mode));
}

fn sleep_until(fire: f64) {
    let wait = (fire - date::now()).clamp(0.0, LONGEST_SLEEP);
    if wait > 0.0 {
        let seconds = wait as i64;
        let _ = linux::sleep([seconds, ((wait - seconds as f64) * 1e9) as i64]);
    }
}

fn run_until(run_loop: Id, mode: u8, limit: f64, once: bool) -> bool {
    loop {
        let Some(fire) = next_fire(run_loop, mode) else {
            return false;
        };
        if fire > limit {
            sleep_until(limit);
            return false;
        }
        sleep_until(fire);
        fire_due_in(run_loop, mode);
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

extern "C" fn run(this: Id, _: Sel) {
    run_until(this, DEFAULT_MODE, f64::INFINITY, false);
}

extern "C" fn run_until_date(this: Id, _: Sel, date: Id) {
    run_until(this, DEFAULT_MODE, seconds_of(date), false);
}

extern "C" fn run_mode_before_date(this: Id, _: Sel, mode: Id, date: Id) -> u8 {
    u8::from(run_until(this, running_mode(mode), seconds_of(date), true))
}

extern "C" fn current_mode(_: Id, _: Sel) -> Id {
    constants::string(DEFAULT_RUN_LOOP_MODE)
}

fn perform_later(target: Id, selector: Sel, argument: Id, delay: f64, modes: u8) {
    if target.is_null() || selector.is_null() {
        return;
    }
    let timer = new_timer(class_pointer(&NSTIMER), delay, false);
    if timer.is_null() {
        return;
    }
    let fields = fields(timer);
    fields.target = objc_retain(target);
    fields.selector = selector as usize;
    fields.argument = retained(argument);
    fields.performs = true;
    schedule(timer, current_loop(), modes);
    objc_release(timer);
}

extern "C" fn perform_after_delay(this: Id, _: Sel, selector: Sel, argument: Id, delay: f64) {
    perform_later(this, selector, argument, delay, DEFAULT_MODE);
}

extern "C" fn perform_after_delay_in_modes(
    this: Id,
    _: Sel,
    selector: Sel,
    argument: Id,
    delay: f64,
    modes: Id,
) {
    perform_later(this, selector, argument, delay, modes_of_array(modes));
}

fn same_argument(waiting: Id, wanted: Id) -> bool {
    waiting == wanted
        || (!waiting.is_null()
            && !wanted.is_null()
            && send_bool(waiting, crate::objc::sel!("isEqual:"), wanted as usize))
}

fn cancel_performs(target: Id, wanted: Option<(usize, Id)>) {
    let run_loop = current_loop();
    let mut waiting = SCHEDULED.with(|timers| {
        let mut waiting = Buffer::new();
        for &timer in timers.as_slice() {
            let fields = fields(timer);
            if fields.performs && fields.run_loop == run_loop && fields.target == target {
                objc_retain(timer);
                if !waiting.push(timer) {
                    objc_release(timer);
                }
            }
        }
        waiting
    });
    for &timer in waiting.as_slice() {
        let fields = fields(timer);
        let matches = wanted.is_none_or(|(selector, argument)| {
            fields.selector == selector && same_argument(fields.argument, argument)
        });
        if matches {
            invalidate_timer(timer);
        }
        objc_release(timer);
    }
    waiting.free();
}

extern "C" fn cancel_previous_requests(_: Id, _: Sel, target: Id) {
    cancel_performs(target, None);
}

extern "C" fn cancel_previous_requests_for(_: Id, _: Sel, target: Id, selector: Sel, argument: Id) {
    cancel_performs(target, Some((selector as usize, argument)));
}

extern "C" fn this_object(this: Id, _: Sel) -> Id {
    this
}

extern "C" fn nothing(_: Id, _: Sel) {}

pub fn define(runtime: &mut Runtime) -> bool {
    let timer_instance = methods![
        "invalidate" => invalidate,
        "isValid" => is_valid,
        "fire" => eggos_guarded_fire_now,
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
        "run" => eggos_guarded_run,
        "runUntilDate:" => eggos_guarded_run_until_date,
        "runMode:beforeDate:" => eggos_guarded_run_mode_before_date,
        "currentMode" => current_mode,
        "retain" => this_object,
        "release" => nothing,
        "autorelease" => this_object,
    ];
    let loop_class = methods![
        "currentRunLoop" => current_run_loop,
        "mainRunLoop" => main_run_loop,
    ];
    let object_instance = methods![
        "performSelector:withObject:afterDelay:" => perform_after_delay,
        "performSelector:withObject:afterDelay:inModes:" => perform_after_delay_in_modes,
    ];
    let object_class = methods![
        "cancelPreviousPerformRequestsWithTarget:" => cancel_previous_requests,
        "cancelPreviousPerformRequestsWithTarget:selector:object:" => cancel_previous_requests_for,
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
            instance_size: core::mem::size_of::<RunLoopObject>(),
            flags: 0,
            instance_methods: &loop_instance,
            class_methods: &loop_class,
        },
    ) && class::extend(runtime, &NSOBJECT, &object_instance, &object_class)
}
