//! Kernel threads and the scheduler (D35).
//!
//! First step of M4: threads that run kernel code on their own stacks, scheduled round-robin
//! on one CPU, cooperatively. A thread runs until it calls [`yield_now`] or its function
//! returns; [`spawn`] starts one and [`JoinHandle::join`] waits for it. The boot code becomes a
//! thread too, the first time it uses the scheduler (its stack is the boot stack). Preemption
//! from the timer interrupt, blocking and per-CPU run queues follow in the next steps.
//!
//! The context switch itself is the HAL's ([`Platform::switch_thread`]): it saves only the
//! registers a function call must preserve, on the thread's own stack, so a switch is a few
//! dozen instructions and never allocates. Neither does anything else on the [`yield_now`] path:
//! the run queue's capacity grows in [`spawn`], never in a switch.
//!
//! Each thread has a 16 KiB stack ([`STACK_SIZE`]) from the buddy allocator, reached through
//! the direct map, with a canary in its lowest word that every switch away from the thread
//! checks: an overflow that reached it stops the kernel with the thread's name instead of
//! silently corrupting the memory below. Guard pages need a kernel virtual area and exception
//! stacks first (roadmap, paging follow-ups).
//!
//! Locking: the scheduler's lock is taken before the heap's and the frame allocator's, never
//! after; an exited thread's stack is freed after it is released. It is never held across a
//! switch: on one CPU, with
//! interrupts masked outside [`Platform::wait_for_interrupt`] and no interrupt handler using the
//! scheduler, nothing can run between releasing it and the switch. The SMP step (M4) hands the
//! lock over across the switch instead.

use crate::heap::{FrameSource, GlobalFrames};
use crate::sync::SpinLock;
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::fmt;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, Ordering};
use eggos_hal::Platform;
use eggos_hal::memory::FRAME_SIZE;

/// The buddy order of a thread's stack: 2^2 frames.
pub const STACK_ORDER: u32 = 2;
/// The size of a thread's stack, in bytes.
pub const STACK_SIZE: usize = (FRAME_SIZE as usize) << STACK_ORDER;
/// The value of a stack's lowest word as long as the thread has not overflowed it.
const STACK_CANARY: u64 = 0x4567_4753_7461_636b;

/// Identifies a thread for as long as the kernel runs; never reused. The boot thread is 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ThreadId(u64);

impl fmt::Display for ThreadId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Why [`spawn`] failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnError {
    /// No frames for the stack, or no heap for the thread's bookkeeping.
    OutOfMemory,
}

/// A thread's stack: [`STACK_SIZE`] bytes from the frame allocator, freed when dropped.
#[derive(Debug)]
struct Stack {
    /// Direct-map address of the lowest byte, which holds the canary.
    base: usize,
}

impl Stack {
    fn allocate() -> Option<Self> {
        let base = GlobalFrames.allocate_frames(STACK_ORDER)?;
        let stack = Self { base };
        // SAFETY: `base` is the first, 16 KiB-aligned word of the frames just allocated, which
        // this stack owns and which the direct map makes writable.
        unsafe { stack.canary().write(STACK_CANARY) };
        Some(stack)
    }

    fn canary(&self) -> *mut u64 {
        self.base as *mut u64
    }

    fn top(&self) -> usize {
        self.base + STACK_SIZE
    }

    /// Whether the thread's writes have not reached the stack's lowest word. Volatile: the
    /// compiler cannot see the thread's writes to its stack through this pointer.
    fn canary_intact(&self) -> bool {
        // SAFETY: the stack's frames stay allocated and mapped until it is dropped; the canary
        // is its aligned first word.
        unsafe { self.canary().read_volatile() == STACK_CANARY }
    }
}

impl Drop for Stack {
    fn drop(&mut self) {
        // SAFETY: the frames came from `GlobalFrames` with this order in `allocate`, and the
        // thread that ran on them has switched away for the last time (see `finish_switch`).
        unsafe { GlobalFrames.free_frames(self.base, STACK_ORDER) };
    }
}

/// A thread the scheduler knows about.
#[derive(Debug)]
struct Thread {
    id: ThreadId,
    /// The saved context while the thread is not running ([`Platform::switch_thread`]).
    context: usize,
    /// `None` for the boot thread, which runs on the boot stack.
    stack: Option<Stack>,
    /// Set once the thread has exited and its stack is freed.
    finished: Arc<AtomicBool>,
}

impl Thread {
    /// Stops the kernel if the thread overflowed its stack: whatever lies below it may already
    /// be corrupted, so carrying on is not safe.
    fn check_stack(&self) {
        if self
            .stack
            .as_ref()
            .is_some_and(|stack| !stack.canary_intact())
        {
            panic!("kernel thread {} overflowed its stack", self.id);
        }
    }
}

impl Drop for Thread {
    fn drop(&mut self) {
        // Free the stack before saying so.
        self.stack = None;
        self.finished.store(true, Ordering::Release);
    }
}

/// An owned, heap-allocated [`Thread`] with a stable address. A raw pointer rather than a
/// `Box`, because the switch writes the thread's context through a pointer to it while the
/// scheduler moves the owner between the queue and `current`, which a `Box`'s uniqueness
/// does not allow.
#[derive(Debug)]
struct ThreadBox(NonNull<Thread>);

// SAFETY: a `ThreadBox` owns its `Thread` as a `Box` would, and `Thread` is `Send`.
unsafe impl Send for ThreadBox {}

impl ThreadBox {
    fn new(thread: Thread) -> Self {
        Self(NonNull::from(Box::leak(Box::new(thread))))
    }

    /// Where the switch saves the thread's context.
    fn context(&self) -> *mut usize {
        // SAFETY: the thread is allocated for as long as `self` lives; no reference is created.
        unsafe { &raw mut (*self.0.as_ptr()).context }
    }

    fn get(&self) -> &Thread {
        // SAFETY: owned by `self`, and only the switch writes it without a reference, while it
        // is not running and the scheduler's lock is released, which no `get` overlaps.
        unsafe { self.0.as_ref() }
    }
}

impl Drop for ThreadBox {
    fn drop(&mut self) {
        // SAFETY: allocated by `Box` in `new`, owned only by `self`.
        drop(unsafe { Box::from_raw(self.0.as_ptr()) });
    }
}

/// The scheduler's state for the one CPU.
#[derive(Debug)]
struct Scheduler {
    /// The running thread; `None` until the boot thread first uses the scheduler.
    current: Option<ThreadBox>,
    /// Threads ready to run, in the order they will. Its capacity is always at least the
    /// number of live threads, so a switch never allocates.
    ready: VecDeque<ThreadBox>,
    /// A thread that has exited, until the next thread to run frees it (no thread can free the
    /// stack it is running on).
    exited: Option<ThreadBox>,
    /// Live threads, including the running one.
    live: usize,
    next_id: u64,
}

static SCHEDULER: SpinLock<Scheduler> = SpinLock::new(Scheduler {
    current: None,
    ready: VecDeque::new(),
    exited: None,
    live: 0,
    next_id: 0,
});

impl Scheduler {
    /// Makes the code running now the boot thread, if no thread runs yet.
    fn adopt_boot_thread(&mut self) -> Result<(), SpawnError> {
        if self.current.is_none() {
            let thread = self.new_thread(0, None)?;
            self.current = Some(thread);
        }
        Ok(())
    }

    fn new_thread(
        &mut self,
        context: usize,
        stack: Option<Stack>,
    ) -> Result<ThreadBox, SpawnError> {
        // One more live thread: room for all of them in the queue first.
        self.ready
            .try_reserve(self.live + 1 - self.ready.len())
            .map_err(|_| SpawnError::OutOfMemory)?;
        let id = ThreadId(self.next_id);
        self.next_id += 1;
        self.live += 1;
        Ok(ThreadBox::new(Thread {
            id,
            context,
            stack,
            finished: Arc::new(AtomicBool::new(false)),
        }))
    }

    /// Makes `next` the running thread and returns the previous one, whose stack it checks.
    fn replace_current(&mut self, next: ThreadBox) -> ThreadBox {
        let previous = self
            .current
            .replace(next)
            .expect("a thread is ready, so one runs");
        previous.get().check_stack();
        previous
    }
}

/// A started thread. Dropping the handle lets the thread run on, unjoined.
#[derive(Debug)]
pub struct JoinHandle {
    id: ThreadId,
    finished: Arc<AtomicBool>,
}

impl JoinHandle {
    pub fn id(&self) -> ThreadId {
        self.id
    }

    /// Whether the thread has exited and its stack is freed.
    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    /// Runs other threads until this one has exited and its stack is freed.
    pub fn join<P: Platform>(self) {
        while !self.is_finished() {
            yield_now::<P>();
        }
    }
}

/// Starts a thread that runs `function` on a stack of its own, after the threads already
/// ready to run. The caller keeps running.
pub fn spawn<P: Platform, F: FnOnce() + Send + 'static>(
    function: F,
) -> Result<JoinHandle, SpawnError> {
    let stack = Stack::allocate().ok_or(SpawnError::OutOfMemory)?;
    let function: Box<dyn FnOnce() + Send> = Box::new(function);
    // A thin pointer, so it fits the one-word argument.
    let start = Box::into_raw(Box::new(function));
    // SAFETY: the stack's frames are allocated, writable through the direct map and used by
    // nothing else; they stay allocated until the thread has exited (`finish_switch`).
    let context = unsafe { P::prepare_thread(stack.top(), thread_start::<P>, start as usize) };
    let mut scheduler = SCHEDULER.lock();
    let thread = scheduler
        .adopt_boot_thread()
        .and_then(|()| scheduler.new_thread(context, Some(stack)));
    let thread = match thread {
        Ok(thread) => thread,
        Err(error) => {
            drop(scheduler);
            // SAFETY: from `Box::into_raw` above; the thread that would have owned it never
            // existed. (The stack, if any, was freed with the failed `Thread`.)
            drop(unsafe { Box::from_raw(start) });
            return Err(error);
        }
    };
    let handle = JoinHandle {
        id: thread.get().id,
        finished: thread.get().finished.clone(),
    };
    scheduler.ready.push_back(thread);
    Ok(handle)
}

/// Lets the next ready thread run, if there is one; returns when this thread's turn comes
/// again.
pub fn yield_now<P: Platform>() {
    let mut scheduler = SCHEDULER.lock();
    let Some(next) = scheduler.ready.pop_front() else {
        return;
    };
    let resume = next.get().context;
    let previous = scheduler.replace_current(next);
    let save = previous.context();
    // Within capacity (see `Scheduler::ready`): the push never allocates.
    scheduler.ready.push_back(previous);
    drop(scheduler);
    // SAFETY: interrupts are masked, as the kernel runs. `save` points into the previous
    // thread, which stays allocated in the queue and is not resumed before this switch has
    // saved it (nothing else runs in between, see the module documentation). `resume` was
    // saved by the switch away from `next` (or prepared by `spawn`) and not resumed since: a
    // thread is in the queue only while it is not running.
    unsafe { P::switch_thread(save, resume) };
    finish_switch();
}

/// Ends the running thread: switches to the next one, which frees this one's stack.
fn exit<P: Platform>() -> ! {
    let mut scheduler = SCHEDULER.lock();
    // The boot thread never exits, and it is not the one running here, so it is ready.
    let next = scheduler
        .ready
        .pop_front()
        .expect("the boot thread is ready to run");
    let resume = next.get().context;
    let previous = scheduler.replace_current(next);
    let save = previous.context();
    scheduler.live -= 1;
    debug_assert!(
        scheduler.exited.is_none(),
        "every switch frees the exited thread"
    );
    scheduler.exited = Some(previous);
    drop(scheduler);
    // SAFETY: as in `yield_now`; `save` is in the exited thread, which `finish_switch` frees
    // only once the switch has left its stack, and nothing resumes it.
    unsafe { P::switch_thread(save, resume) };
    unreachable!("an exited thread was resumed");
}

/// Runs in every thread right after a switch to it: frees the thread that exited, if the
/// switch came from one. Its stack is no longer in use, and the scheduler's lock is not held
/// while it is freed.
fn finish_switch() {
    let exited = SCHEDULER.lock().exited.take();
    drop(exited);
}

/// Where every spawned thread starts, on its own stack: `argument` is the function `spawn`
/// boxed.
extern "C" fn thread_start<P: Platform>(argument: usize) -> ! {
    finish_switch();
    // SAFETY: `spawn` passed a pointer from `Box::into_raw` to this thread alone, which starts
    // once.
    let function = unsafe { Box::from_raw(argument as *mut Box<dyn FnOnce() + Send>) };
    function();
    exit::<P>()
}

/// Host stand-ins for the HAL's context switch: each kernel thread runs on a host thread of its
/// own, and a switch hands a baton from one to the other, so exactly one of them runs at a time,
/// as on one CPU. A thread's context is the address of its baton.
#[cfg(test)]
pub(crate) mod host {
    use std::cell::Cell;
    use std::sync::{Condvar, Mutex};

    #[derive(Default)]
    struct Baton {
        held: Mutex<bool>,
        handed: Condvar,
    }

    impl Baton {
        fn hand_over(&self) {
            *self.held.lock().unwrap() = true;
            self.handed.notify_one();
        }

        fn wait(&self) {
            let mut held = self.held.lock().unwrap();
            while !*held {
                held = self.handed.wait(held).unwrap();
            }
            *held = false;
        }
    }

    std::thread_local! {
        static BATON: Cell<Option<&'static Baton>> = const { Cell::new(None) };
    }

    fn my_baton() -> &'static Baton {
        BATON.with(|baton| {
            *baton.get().get_or_insert_with(|| {
                let new = Box::leak(Box::default());
                baton.set(Some(new));
                new
            })
        })
    }

    pub(crate) fn prepare_thread(entry: extern "C" fn(usize) -> !, argument: usize) -> usize {
        let baton: &'static Baton = Box::leak(Box::default());
        std::thread::spawn(move || {
            BATON.with(|mine| mine.set(Some(baton)));
            baton.wait();
            entry(argument)
        });
        baton as *const Baton as usize
    }

    /// # Safety
    /// `save` must be valid for a write, and `resume` a context from [`prepare_thread`] or
    /// saved by this function.
    pub(crate) unsafe fn switch_thread(save: *mut usize, resume: usize) {
        let me = my_baton();
        // SAFETY: the caller guarantees `save` is writable.
        unsafe { save.write(me as *const Baton as usize) };
        // SAFETY: the caller guarantees `resume` is the address of a leaked `Baton`.
        unsafe { &*(resume as *const Baton) }.hand_over();
        me.wait();
    }

    /// Held by every host test that uses the scheduler, boots the kernel or uses the kernel's
    /// frame allocator ([`crate::memory::FRAMES`]), whose results depend on what other tests
    /// take from it: there is one of each, and tests run in parallel. Also makes sure the frame
    /// allocator exists, over the test platform's RAM, as boot leaves one.
    pub(crate) fn lock_scheduler_and_frames_for_test() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        // A test that panicked while holding the lock may leave the scheduler unusable, but
        // reporting that test's failure is what matters.
        let guard = LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        use eggos_hal::Platform;
        type P = crate::tests::TestPlatform;
        crate::memory::FRAMES.lock().get_or_insert_with(|| {
            crate::memory::FrameAllocator::new(&P::memory_map(), P::physical_memory_offset())
        });
        guard
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::SpinLock;
    use crate::tests::TestPlatform as P;
    use alloc::vec::Vec;

    fn free_frames() -> u64 {
        crate::memory::FRAMES
            .lock()
            .as_ref()
            .map_or(0, |frames| frames.free_frames())
    }

    #[test]
    fn threads_take_turns_in_spawn_order() {
        let _lock = host::lock_scheduler_and_frames_for_test();
        static LOG: SpinLock<Vec<(u32, u32)>> = SpinLock::new(Vec::new());
        LOG.lock().clear();
        let handles: Vec<JoinHandle> = (0..3)
            .map(|thread| {
                spawn::<P, _>(move || {
                    for turn in 0..3 {
                        LOG.lock().push((thread, turn));
                        yield_now::<P>();
                    }
                })
                .unwrap()
            })
            .collect();
        assert!(handles.windows(2).all(|pair| pair[0].id() < pair[1].id()));
        for handle in handles {
            handle.join::<P>();
        }
        let expected: Vec<(u32, u32)> = (0..3)
            .flat_map(|turn| (0..3).map(move |thread| (thread, turn)))
            .collect();
        assert_eq!(*LOG.lock(), expected);
    }

    #[test]
    fn a_thread_that_exited_has_its_stack_freed() {
        let _lock = host::lock_scheduler_and_frames_for_test();
        let before = free_frames();
        let handle = spawn::<P, _>(|| {}).unwrap();
        assert!(free_frames() < before);
        assert!(!handle.is_finished());
        handle.join::<P>();
        assert_eq!(free_frames(), before);
        assert!(SCHEDULER.lock().exited.is_none());
    }

    #[test]
    fn yield_without_other_threads_returns_at_once() {
        let _lock = host::lock_scheduler_and_frames_for_test();
        yield_now::<P>();
        yield_now::<P>();
    }

    #[test]
    #[should_panic(expected = "overflowed its stack")]
    fn an_overflowed_stack_stops_the_kernel_at_the_next_switch() {
        // No scheduler and no frame allocator: the thread never runs, and its stack is leaked
        // host memory, which must not reach the frame allocator, so the thread is never dropped.
        let memory = Box::leak(Box::new([0u64; STACK_SIZE / 8]));
        let stack = Stack {
            base: memory.as_mut_ptr() as usize,
        };
        // SAFETY: the first word of `memory`, which the stack owns.
        unsafe { stack.canary().write(STACK_CANARY) };
        let thread = core::mem::ManuallyDrop::new(Thread {
            id: ThreadId(u64::MAX),
            context: 0,
            stack: Some(stack),
            finished: Arc::new(AtomicBool::new(false)),
        });
        thread.check_stack();
        memory[0] = 0;
        thread.check_stack();
    }
}
