//! Kernel threads and the scheduler (D35, D37).
//!
//! M4: threads that run kernel code on their own stacks, scheduled round-robin on one CPU. A
//! thread runs until it calls [`yield_now`], its function returns, or its time slice is over
//! at a [`preemption_point`]; [`spawn`] starts one and [`JoinHandle::join`] waits for it. The
//! boot code becomes a thread too, the first time it uses the scheduler (its stack is the boot
//! stack). Blocking and per-CPU run queues follow in the next steps.
//!
//! Preemption is voluntary inside the kernel (D37): kernel code runs with interrupts masked,
//! and while [`enable_preemption`] has the timer tick every [`TIME_SLICE_MS`], a thread whose
//! slice is over lets the next one run at its next [`preemption_point`], which opens a window
//! for the tick's interrupt. Nothing is preempted while it holds a lock, so locks cost nothing
//! extra and an interrupt handler never finds one held. User code will be preempted on its
//! return to user mode.
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
//! after; an exited thread's stack is freed after it is released. A switch is made holding it,
//! and the lock is handed over to the thread the switch resumes, which releases it
//! ([`finish_switch`]): from choosing the next thread until the previous one's context is
//! saved, nothing else can take the scheduler, on this CPU or another.

use crate::heap::{FrameSource, GlobalFrames};
use crate::interrupt;
use crate::sync::{SpinLock, SpinLockGuard};
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::fmt;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use eggos_hal::Platform;
use eggos_hal::memory::FRAME_SIZE;

/// The buddy order of a thread's stack: 2^2 frames.
pub const STACK_ORDER: u32 = 2;
/// The size of a thread's stack, in bytes.
pub const STACK_SIZE: usize = (FRAME_SIZE as usize) << STACK_ORDER;
/// The value of a stack's lowest word as long as the thread has not overflowed it.
const STACK_CANARY: u64 = 0x4567_4753_7461_636b;

/// How long a thread runs, while preemption is on, before it lets the next one run.
pub const TIME_SLICE_MS: u64 = 10;

/// The timer tick's interval in clock ticks while preemption is on ([`enable_preemption`]);
/// 0 while it is off. Per-CPU with SMP, like [`SLICE_START`].
static TICK_INTERVAL: AtomicU64 = AtomicU64::new(0);
/// [`interrupt::timer_interrupts`] when the running thread's time slice started: the slice is
/// over once a timer interrupt has been taken since. The interrupt handler itself does
/// nothing for preemption, so the interrupt path costs no more with it.
static SLICE_START: AtomicU64 = AtomicU64::new(0);

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
    /// No frames for the stack, or no heap to grow the run queue. (The thread's own small
    /// allocations are infallible, like every `Box`: the heap running out there stops the
    /// kernel.)
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
        // SAFETY: owned by `self`, and only the switch writes it without a reference, holding
        // the scheduler's lock (handed over across the switch), which no `get` overlaps.
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
        debug_assert!(
            SCHEDULER
                .lock()
                .current
                .as_ref()
                .is_none_or(|thread| thread.get().id != self.id),
            "a thread joined itself, which never returns"
        );
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
            // existed. (The stack was freed when `new_thread` failed or was never called.)
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
    // Held across the switch, for the thread it resumes to release.
    SpinLockGuard::leak(scheduler);
    // SAFETY: interrupts are masked, as the kernel runs. `save` points into the previous
    // thread, which stays allocated in the queue and is not resumed before this switch has
    // saved it: that takes the scheduler's lock, which stays held until then. `resume` was
    // saved by the switch away from `next` (or prepared by `spawn`) and not resumed since: a
    // thread is in the queue only while it is not running.
    unsafe { P::switch_thread(save, resume) };
    finish_switch();
}

/// Starts the timer tick that preempts threads (D37): once a thread has run for
/// [`TIME_SLICE_MS`], it lets the next ready thread run at its next [`preemption_point`]. The
/// tick owns this CPU's timer ([`Platform::set_timer_deadline`]) until [`disable_preemption`].
pub fn enable_preemption<P: Platform>() {
    let interval = (P::clock_frequency() * TIME_SLICE_MS / 1000).max(1);
    TICK_INTERVAL.store(interval, Ordering::Relaxed);
    start_slice::<P>(interval);
}

/// Stops the timer tick [`enable_preemption`] started; threads run until they yield again.
/// Call it with interrupts masked, as the kernel runs: the tick's deadline is replaced by one
/// that is never reached, so no tick arrives afterwards.
pub fn disable_preemption<P: Platform>() {
    TICK_INTERVAL.store(0, Ordering::Relaxed);
    P::set_timer_deadline(u64::MAX);
}

/// Starts a time slice: the next timer interrupt, which the tick arms for `interval` from now,
/// ends it.
fn start_slice<P: Platform>(interval: u64) {
    SLICE_START.store(interrupt::timer_interrupts(), Ordering::Relaxed);
    P::set_timer_deadline(P::clock_ticks().saturating_add(interval));
}

/// Where the running thread lets the next one run if its time slice is over (D37): kernel
/// code that runs for long calls it regularly, as cheap as a few loads when the slice is not
/// over. Takes the timer interrupt if one is pending, then, when preemption is on and a timer
/// interrupt has come since the slice started, starts the next slice and yields. Call it as
/// [`yield_now`] is called: with interrupts masked and no lock held.
#[inline]
pub fn preemption_point<P: Platform>() {
    P::interrupt_window();
    if interrupt::timer_interrupts() != SLICE_START.load(Ordering::Relaxed) {
        slice_over::<P>();
    }
}

/// [`preemption_point`]'s slow path, out of line so the check stays a few instructions.
#[cold]
#[inline(never)]
fn slice_over<P: Platform>() {
    let interval = TICK_INTERVAL.load(Ordering::Relaxed);
    if interval == 0 {
        // A timer interrupt for someone else, with preemption off: nothing to do until it is
        // on again, which starts a new slice.
        SLICE_START.store(interrupt::timer_interrupts(), Ordering::Relaxed);
        return;
    }
    // The next thread's slice; it ends at a tick whoever runs then.
    start_slice::<P>(interval);
    yield_now::<P>();
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
    SpinLockGuard::leak(scheduler);
    // SAFETY: as in `yield_now`; `save` is in the exited thread, which `finish_switch` frees
    // only once the switch has left its stack, and nothing resumes it.
    unsafe { P::switch_thread(save, resume) };
    unreachable!("an exited thread was resumed");
}

/// Runs in every thread right after a switch to it: releases the scheduler's lock, which the
/// switch handed over, then frees the thread that exited, if the switch came from one. Its
/// stack is no longer in use, and the lock is not held while it is freed.
fn finish_switch() {
    // SAFETY: every switch is made holding the scheduler's lock and leaves it held for the
    // thread it resumes, which is this one, once.
    let exited = unsafe { SCHEDULER.assume_locked() }.exited.take();
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
    /// frame allocator ([`crate::memory::FRAMES`]) or heap ([`crate::HEAP`]): there is one of
    /// each, and tests run in parallel.
    pub(crate) fn lock_kernel_state_for_test() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        // A test that panicked while holding the lock may leave the scheduler unusable, but
        // reporting that test's failure is what matters.
        LOCK.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// [`lock_kernel_state_for_test`], plus a frame allocator for thread stacks: the
    /// one boot left, or until the guard is dropped one over memory of its own. Never one over
    /// the test platform's RAM, and never left behind: the boot test builds its allocator over
    /// that RAM, and its heap self-test needs the heap to have taken no frames before.
    pub(crate) fn frames_for_test() -> FramesForTest {
        let lock = lock_kernel_state_for_test();
        let mut frames = crate::memory::FRAMES.lock();
        let installed = frames.is_none();
        if installed {
            use eggos_hal::memory::FRAME_SIZE;
            use eggos_hal::{MemoryMap, PhysicalRange};
            const BASE: u64 = 0x40_0000;
            const FRAMES: u64 = 64;
            // Aligned to the largest buddy block, as the direct maps are.
            let block = (FRAME_SIZE as usize) << crate::memory::MAX_ORDER;
            let memory = vec![0u8; FRAMES as usize * FRAME_SIZE as usize + block].leak();
            let start = (memory.as_mut_ptr() as usize).next_multiple_of(block);
            let mut map = MemoryMap::new();
            map.add_ram(PhysicalRange::from_start_length(BASE, FRAMES * FRAME_SIZE));
            *frames = Some(crate::memory::FrameAllocator::new(
                &map,
                start - BASE as usize,
            ));
        }
        FramesForTest {
            installed,
            _lock: lock,
        }
    }

    /// See [`frames_for_test`].
    pub(crate) struct FramesForTest {
        installed: bool,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Drop for FramesForTest {
        fn drop(&mut self) {
            // Before the lock is released (fields drop after this).
            if self.installed {
                crate::memory::FRAMES.lock().take();
            }
        }
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
        let _frames = host::frames_for_test();
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
        let _frames = host::frames_for_test();
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
        let _frames = host::frames_for_test();
        yield_now::<P>();
        yield_now::<P>();
    }

    #[test]
    fn a_thread_at_preemption_points_lets_others_run_once_its_slice_is_over() {
        let _timer = crate::interrupt::lock_timer_interrupts_for_test();
        let _frames = host::frames_for_test();
        static SECOND_RAN: AtomicBool = AtomicBool::new(false);
        static PASSED: AtomicU64 = AtomicU64::new(0);
        SECOND_RAN.store(false, Ordering::Relaxed);
        PASSED.store(0, Ordering::Relaxed);
        let first = spawn::<P, _>(|| {
            // The test platform's clock advances one tick per preemption point, so the 10 ms
            // slice is 10 000 of them at its 1 MHz; a million means it never ended.
            while !SECOND_RAN.load(Ordering::Relaxed)
                && PASSED.fetch_add(1, Ordering::Relaxed) < 1_000_000
            {
                preemption_point::<P>();
            }
        })
        .unwrap();
        let second = spawn::<P, _>(|| SECOND_RAN.store(true, Ordering::Relaxed)).unwrap();
        enable_preemption::<P>();
        first.join::<P>();
        second.join::<P>();
        disable_preemption::<P>();
        assert!(SECOND_RAN.load(Ordering::Relaxed));
        let passed = PASSED.load(Ordering::Relaxed);
        assert!(
            (1000..1_000_000).contains(&passed),
            "{passed} preemption points"
        );
    }

    #[test]
    fn without_preemption_a_preemption_point_never_switches() {
        let _timer = crate::interrupt::lock_timer_interrupts_for_test();
        let _frames = host::frames_for_test();
        static RAN: AtomicBool = AtomicBool::new(false);
        RAN.store(false, Ordering::Relaxed);
        let other = spawn::<P, _>(|| RAN.store(true, Ordering::Relaxed)).unwrap();
        // A timer interrupt for someone else ends no slice while preemption is off.
        crate::interrupt::handle(eggos_hal::Interrupt::Timer);
        preemption_point::<P>();
        preemption_point::<P>();
        assert!(!RAN.load(Ordering::Relaxed));
        other.join::<P>();
        assert!(RAN.load(Ordering::Relaxed));
    }

    #[test]
    #[should_panic(expected = "overflowed its stack")]
    fn a_clobbered_canary_stops_the_kernel_when_checked() {
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
