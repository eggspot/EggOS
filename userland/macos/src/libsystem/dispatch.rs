//! libdispatch, the part programs use most (D388): the main queue, whose work runs in order on
//! the main thread's run loop (AppKit's, `NSRunLoop`'s or `dispatch_main`), the global and
//! private queues, `dispatch_async`, `dispatch_sync`, `dispatch_after` and `dispatch_time`,
//! `dispatch_once`, groups and semaphores. The layer runs one thread of the program's work: a
//! global or private queue's asynchronous work runs at once on the calling thread, which keeps
//! its order and finishes before the caller waits on it; delayed work always waits for the
//! main thread's loop.

use crate::libsystem::Exclusive;
use crate::objc::block::{block_copy, block_release, invoke_address};
use alloc::collections::VecDeque;

const NOW: u64 = 0;
const FOREVER: u64 = !0;
const ONCE_DONE: isize = -1;
const MOST_QUEUES: usize = 64;
const MAIN: usize = 0;
const GLOBAL: usize = 1;

/// A queue as programs hold it; its first word is zero, its second says which queue it is.
#[repr(C)]
#[derive(Debug)]
pub struct Queue {
    isa: usize,
    kind: usize,
    label: [u8; 64],
}

const fn label(text: &[u8]) -> [u8; 64] {
    let mut out = [0u8; 64];
    let mut index = 0;
    while index < text.len() && index < 63 {
        out[index] = text[index];
        index += 1;
    }
    out
}

/// The main queue: `dispatch_get_main_queue()` is `&_dispatch_main_q`.
pub static MAIN_QUEUE: Queue = Queue {
    isa: 0,
    kind: MAIN,
    label: label(b"com.apple.main-thread"),
};

static GLOBAL_QUEUE: Queue = Queue {
    isa: 0,
    kind: GLOBAL,
    label: [0; 64],
};

#[derive(Debug, Clone, Copy)]
enum Work {
    Block(usize),
    Function(usize, usize),
}

struct Pending {
    due: u64,
    work: Work,
}

static MAIN_WORK: Exclusive<VecDeque<Work>> = Exclusive::new(VecDeque::new());
static DELAYED: Exclusive<alloc::vec::Vec<Pending>> = Exclusive::new(alloc::vec::Vec::new());
static QUEUES_MADE: Exclusive<usize> = Exclusive::new(0);

fn now_ns() -> u64 {
    super::process::clock_gettime_nsec_np(6)
}

fn run(work: Work) {
    match work {
        Work::Block(block) => {
            let invoke = invoke_address(block as crate::objc::abi::Id);
            // SAFETY: a copied block of the program's taking only itself.
            let invoke: extern "C" fn(usize) = unsafe { core::mem::transmute(invoke) };
            invoke(block);
            block_release(block as *const u8);
        }
        Work::Function(context, function) => {
            let function = crate::callback::native_callable(function);
            // SAFETY: a `dispatch_function_t`, which takes its context.
            let function: extern "C" fn(usize) = unsafe { core::mem::transmute(function) };
            function(context);
        }
    }
}

fn is_main(queue: *const Queue) -> bool {
    // SAFETY: a queue the layer made (or the main queue), as programs pass them back.
    queue.is_null() || unsafe { (*queue).kind } == MAIN
}

fn submit(queue: *const Queue, work: Work) {
    match is_main(queue) {
        true => MAIN_WORK.with(|pending| pending.push_back(work)),
        false => run(work),
    }
}

fn copied(block: usize) -> Work {
    Work::Block(block_copy(block as *const u8) as usize)
}

/// Runs the main queue's work that is ready and the delayed work that is due, in order; what
/// that work queues runs on a later turn.
pub fn drain_main() {
    let now = now_ns();
    let due: alloc::vec::Vec<Work> = DELAYED.with(|delayed| {
        let mut ready = alloc::vec::Vec::new();
        delayed.retain(|pending| {
            let is_due = pending.due <= now;
            if is_due {
                ready.push(pending.work);
            }
            !is_due
        });
        ready
    });
    MAIN_WORK.with(|pending| pending.extend(due));
    let count = MAIN_WORK.with(|pending| pending.len());
    for _ in 0..count {
        let Some(work) = MAIN_WORK.with(|pending| pending.pop_front()) else {
            break;
        };
        run(work);
    }
}

/// How many milliseconds until main-queue work is ready (0 when some is), `None` when none is
/// waiting.
pub fn next_due_in_ms() -> Option<u64> {
    if MAIN_WORK.with(|pending| !pending.is_empty()) {
        return Some(0);
    }
    let earliest = DELAYED.with(|delayed| delayed.iter().map(|pending| pending.due).min())?;
    Some(earliest.saturating_sub(now_ns()).div_ceil(1_000_000))
}

extern "C" fn dispatch_async(queue: *const Queue, block: usize) {
    if block != 0 {
        submit(queue, copied(block));
    }
}

extern "C" fn dispatch_async_f(queue: *const Queue, context: usize, function: usize) {
    if function != 0 {
        submit(queue, Work::Function(context, function));
    }
}

extern "C" fn dispatch_sync(_: *const Queue, block: usize) {
    if block != 0 {
        run(copied(block));
    }
}

extern "C" fn dispatch_sync_f(_: *const Queue, context: usize, function: usize) {
    if function != 0 {
        run(Work::Function(context, function));
    }
}

extern "C" fn dispatch_time(base: u64, delta: i64) -> u64 {
    if base == FOREVER {
        return FOREVER;
    }
    let start = match base {
        NOW => now_ns(),
        base => base,
    };
    start.saturating_add_signed(delta)
}

extern "C" fn dispatch_walltime(_: usize, delta: i64) -> u64 {
    dispatch_time(NOW, delta)
}

fn delay(when: u64, work: Work) {
    if when == FOREVER {
        return;
    }
    DELAYED.with(|delayed| delayed.push(Pending { due: when, work }));
}

extern "C" fn dispatch_after(when: u64, _: *const Queue, block: usize) {
    if block != 0 {
        delay(when, copied(block));
    }
}

extern "C" fn dispatch_after_f(when: u64, _: *const Queue, context: usize, function: usize) {
    if function != 0 {
        delay(when, Work::Function(context, function));
    }
}

extern "C" fn dispatch_once(predicate: *mut isize, block: usize) {
    // SAFETY: the program's `dispatch_once_t`.
    if predicate.is_null() || unsafe { predicate.read() } == ONCE_DONE {
        return;
    }
    // SAFETY: as above.
    unsafe { predicate.write(ONCE_DONE) };
    let invoke = invoke_address(block as crate::objc::abi::Id);
    // SAFETY: the program's block, which takes only itself.
    let invoke: extern "C" fn(usize) = unsafe { core::mem::transmute(invoke) };
    invoke(block);
}

extern "C" fn dispatch_once_f(predicate: *mut isize, context: usize, function: usize) {
    // SAFETY: the program's `dispatch_once_t`.
    if predicate.is_null() || unsafe { predicate.read() } == ONCE_DONE {
        return;
    }
    // SAFETY: as above.
    unsafe { predicate.write(ONCE_DONE) };
    run(Work::Function(context, function));
}

extern "C" fn get_global_queue(_: isize, _: usize) -> *const Queue {
    &raw const GLOBAL_QUEUE
}

extern "C" fn queue_create(label: *const u8, _: usize) -> *const Queue {
    let room = QUEUES_MADE.with(|made| {
        let room = *made < MOST_QUEUES;
        *made += usize::from(room);
        room
    });
    if !room {
        return &raw const GLOBAL_QUEUE;
    }
    let queue = super::heap::allocate(core::mem::size_of::<Queue>(), true).cast::<Queue>();
    if queue.is_null() {
        return &raw const GLOBAL_QUEUE;
    }
    let mut copied_label = [0u8; 64];
    if !label.is_null() {
        for (index, slot) in copied_label.iter_mut().enumerate().take(63) {
            // SAFETY: the program's NUL-terminated label.
            let byte = unsafe { label.add(index).read() };
            if byte == 0 {
                break;
            }
            *slot = byte;
        }
    }
    // SAFETY: a new block of the queue's size.
    unsafe {
        queue.write(Queue {
            isa: 0,
            kind: GLOBAL,
            label: copied_label,
        })
    };
    queue
}

extern "C" fn queue_get_label(queue: *const Queue) -> *const u8 {
    let queue = match queue.is_null() {
        true => &raw const MAIN_QUEUE,
        false => queue,
    };
    // SAFETY: a queue the layer made, which lives as long as the program.
    unsafe { (*queue).label.as_ptr() }
}

extern "C" fn retain_or_release(_: usize) {}

#[repr(C)]
#[derive(Debug)]
struct Counter {
    isa: usize,
    count: isize,
    waiting: usize,
}

fn new_counter(count: isize) -> *mut Counter {
    let counter = super::heap::allocate(core::mem::size_of::<Counter>(), true).cast::<Counter>();
    if !counter.is_null() {
        // SAFETY: a new block of the counter's size.
        unsafe {
            counter.write(Counter {
                isa: 0,
                count,
                waiting: 0,
            })
        };
    }
    counter
}

fn counter<'a>(object: *mut Counter) -> Option<&'a mut Counter> {
    // SAFETY: a group or semaphore the layer made, or null.
    (!object.is_null()).then(|| unsafe { &mut *object })
}

extern "C" fn semaphore_create(value: isize) -> *mut Counter {
    match value < 0 {
        true => core::ptr::null_mut(),
        false => new_counter(value),
    }
}

extern "C" fn semaphore_signal(semaphore: *mut Counter) -> isize {
    let Some(semaphore) = counter(semaphore) else {
        return 0;
    };
    semaphore.count += 1;
    0
}

extern "C" fn semaphore_wait(semaphore: *mut Counter, _: u64) -> isize {
    let Some(semaphore) = counter(semaphore) else {
        return 1;
    };
    if semaphore.count > 0 {
        semaphore.count -= 1;
        return 0;
    }
    1
}

extern "C" fn group_create() -> *mut Counter {
    new_counter(0)
}

extern "C" fn group_enter(group: *mut Counter) {
    if let Some(group) = counter(group) {
        group.count += 1;
    }
}

fn notify_if_done(group: &mut Counter) {
    if group.count <= 0 && group.waiting != 0 {
        let work = group.waiting;
        group.waiting = 0;
        MAIN_WORK.with(|pending| pending.push_back(Work::Block(work)));
    }
}

extern "C" fn group_leave(group: *mut Counter) {
    if let Some(group) = counter(group) {
        group.count -= 1;
        notify_if_done(group);
    }
}

extern "C" fn group_async(group: *mut Counter, queue: *const Queue, block: usize) {
    group_enter(group);
    dispatch_async(queue, block);
    if !is_main(queue) {
        group_leave(group);
    }
}

extern "C" fn group_notify(group: *mut Counter, queue: *const Queue, block: usize) {
    let Some(group) = counter(group) else {
        return;
    };
    let copy = block_copy(block as *const u8) as usize;
    match group.count <= 0 {
        true => submit(queue, Work::Block(copy)),
        false => group.waiting = copy,
    }
}

extern "C" fn group_wait(group: *mut Counter, _: u64) -> isize {
    counter(group).map_or(0, |group| isize::from(group.count > 0))
}

extern "C" fn dispatch_main() -> ! {
    loop {
        drain_main();
        let wait = next_due_in_ms().unwrap_or(1000).min(1000);
        let _ = crate::linux::sleep([0, (wait.max(1) * 1_000_000) as i64]);
    }
}

/// The address of the libdispatch function or variable `name`, if the layer has it.
pub fn resolve(name: &[u8]) -> Option<usize> {
    let address = match name {
        b"__dispatch_main_q" => &raw const MAIN_QUEUE as usize,
        b"_dispatch_async" => dispatch_async as *const () as usize,
        b"_dispatch_async_f" => dispatch_async_f as *const () as usize,
        b"_dispatch_sync" | b"_dispatch_barrier_sync" => dispatch_sync as *const () as usize,
        b"_dispatch_sync_f" => dispatch_sync_f as *const () as usize,
        b"_dispatch_barrier_async" => dispatch_async as *const () as usize,
        b"_dispatch_time" => dispatch_time as *const () as usize,
        b"_dispatch_walltime" => dispatch_walltime as *const () as usize,
        b"_dispatch_after" => dispatch_after as *const () as usize,
        b"_dispatch_after_f" => dispatch_after_f as *const () as usize,
        b"_dispatch_once" => dispatch_once as *const () as usize,
        b"_dispatch_once_f" => dispatch_once_f as *const () as usize,
        b"_dispatch_get_global_queue" => get_global_queue as *const () as usize,
        b"_dispatch_queue_create" => queue_create as *const () as usize,
        b"_dispatch_queue_get_label" => queue_get_label as *const () as usize,
        b"_dispatch_retain" | b"_dispatch_release" => retain_or_release as *const () as usize,
        b"_dispatch_semaphore_create" => semaphore_create as *const () as usize,
        b"_dispatch_semaphore_signal" => semaphore_signal as *const () as usize,
        b"_dispatch_semaphore_wait" => semaphore_wait as *const () as usize,
        b"_dispatch_group_create" => group_create as *const () as usize,
        b"_dispatch_group_enter" => group_enter as *const () as usize,
        b"_dispatch_group_leave" => group_leave as *const () as usize,
        b"_dispatch_group_async" => group_async as *const () as usize,
        b"_dispatch_group_notify" => group_notify as *const () as usize,
        b"_dispatch_group_wait" => group_wait as *const () as usize,
        b"_dispatch_main" => dispatch_main as *const () as usize,
        _ => return None,
    };
    Some(address)
}
