//! Kernel threads and the scheduler (D35, D37, D39).
//!
//! M4: threads that run kernel code on their own stacks, scheduled round-robin on one CPU. A
//! thread runs until it calls [`yield_now`], blocks ([`sleep_until`], [`WaitQueue::wait_until`],
//! [`JoinHandle::join`]), its function returns, or its time slice is over at a
//! [`preemption_point`]; [`spawn`] starts one. The boot code becomes a thread too, the first
//! time it uses the scheduler (its stack is the boot stack), and so does the CPU's idle thread,
//! which runs only when every other thread is blocked: it waits for an interrupt and lets the
//! first thread that becomes ready run. Per-CPU run queues follow in the next step.
//!
//! Preemption is voluntary inside the kernel (D37): kernel code runs with interrupts masked,
//! and while [`enable_preemption`] is on, a thread whose [`TIME_SLICE_MS`] slice is over lets
//! the next one run at its next [`preemption_point`], which opens a window for the timer's
//! interrupt. Nothing is preempted while it holds a lock, so locks cost nothing extra and an
//! interrupt handler never finds one held. User code will be preempted on its return to user
//! mode.
//!
//! The CPU's one timer is shared (D39): it is armed for the earlier of the running thread's
//! slice end, only while another thread is ready to run (no tick otherwise: tickless), and the
//! earliest sleeping thread's wake time. The interrupt handler only counts timer interrupts;
//! the scheduler notices the count change at the next preemption point, yield, block or in the
//! idle thread, and then wakes the sleepers whose time has come and ends the slice if it is
//! over. So the interrupt path costs what it did, and a sleeper is woken as late as the
//! running thread's next preemption point, as a slice ends.
//!
//! The context switch itself is the HAL's ([`Platform::switch_thread`]): it saves only the
//! registers a function call must preserve, on the thread's own stack, so a switch is a few
//! dozen instructions and never allocates. Neither does anything else on a switch: the run
//! queue, the wait queues and the sleepers are lists linked through the threads themselves.
//!
//! Each thread has a 16 KiB stack ([`STACK_SIZE`]) from the buddy allocator, reached through
//! the direct map, with a canary in its lowest word that every switch away from the thread
//! checks: an overflow that reached it stops the kernel with the thread's name instead of
//! silently corrupting the memory below. Guard pages need a kernel virtual area and exception
//! stacks first (roadmap, paging follow-ups).
//!
//! Locking: the scheduler's lock, which also guards every [`WaitQueue`], is taken before the
//! heap's and the frame allocator's, never after; an exited thread's stack is freed after it
//! is released. A switch is made holding it, and the lock is handed over to the thread the
//! switch resumes, which releases it ([`finish_switch`]): from choosing the next thread until
//! the previous one's context is saved, nothing else can take the scheduler, on this CPU or
//! another.

use crate::heap::{FrameSource, GlobalFrames};
use crate::interrupt;
use crate::sync::{SpinLock, SpinLockGuard};
use alloc::boxed::Box;
use alloc::sync::Arc;
use core::cell::UnsafeCell;
use core::fmt;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use core::time::Duration;
use eggos_hal::Platform;
use eggos_hal::memory::FRAME_SIZE;
use eggos_hal::time::mul_div_ceil;

/// The buddy order of a thread's stack: 2^2 frames.
pub const STACK_ORDER: u32 = 2;
/// The size of a thread's stack, in bytes.
pub const STACK_SIZE: usize = (FRAME_SIZE as usize) << STACK_ORDER;
/// The value of a stack's lowest word as long as the thread has not overflowed it.
const STACK_CANARY: u64 = 0x4567_4753_7461_636b;

/// How long a thread runs, while preemption is on, before it lets the next one run.
pub const TIME_SLICE_MS: u64 = 10;

/// [`interrupt::timer_interrupts`] when the scheduler last looked at the timer: a different
/// count means the timer has fired since, so a sleeper may be due or the slice over. Per-CPU
/// with SMP. Written only with the scheduler's lock held, read without it by the preemption
/// point's fast path.
static TIMER_INTERRUPTS_SEEN: AtomicU64 = AtomicU64::new(0);

/// Identifies a thread for as long as the kernel runs; never reused. The boot thread is 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ThreadId(u64);

impl fmt::Display for ThreadId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Why [`spawn`] or [`start`] failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnError {
    /// No frames for a stack. (The threads' small allocations are infallible, like every
    /// `Box`: the heap running out there stops the kernel.)
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

/// What a [`JoinHandle`] and its thread share.
#[derive(Debug)]
struct Joined {
    /// Set once the thread has exited and its stack is freed.
    finished: AtomicBool,
    /// The threads in [`JoinHandle::join`].
    joiners: WaitQueue,
}

/// A thread the scheduler knows about.
#[derive(Debug)]
struct Thread {
    id: ThreadId,
    /// The saved context while the thread is not running ([`Platform::switch_thread`]).
    context: usize,
    /// `None` for the boot thread, which runs on the boot stack.
    stack: Option<Stack>,
    joined: Arc<Joined>,
    /// The next thread in the [`ThreadList`] that holds this one: the ready threads, a wait
    /// queue or the sleepers. A thread is in at most one list, and in none while it runs.
    next: Option<NonNull<Thread>>,
    /// While it sleeps: the [`Platform::clock_ticks`] it wakes at.
    wake_at: u64,
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

/// An owned, heap-allocated [`Thread`] with a stable address. A raw pointer rather than a
/// `Box`, because the switch writes the thread's context through a pointer to it while the
/// scheduler moves the owner between lists and `current`, which a `Box`'s uniqueness does not
/// allow.
#[derive(Debug)]
struct ThreadBox(NonNull<Thread>);

// SAFETY: a `ThreadBox` owns its `Thread` as a `Box` would, and `Thread` is `Send`.
unsafe impl Send for ThreadBox {}

impl ThreadBox {
    fn new(thread: Thread) -> Self {
        Self(NonNull::from(Box::leak(Box::new(thread))))
    }

    /// Gives up ownership, for a [`ThreadList`] to hold the thread.
    fn into_raw(self) -> NonNull<Thread> {
        let thread = self.0;
        core::mem::forget(self);
        thread
    }

    /// # Safety
    /// `thread` must come from [`Self::into_raw`], and be taken back once.
    unsafe fn from_raw(thread: NonNull<Thread>) -> Self {
        Self(thread)
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

    /// The thread, taken out of its box.
    fn into_inner(self) -> Thread {
        // SAFETY: allocated by `Box` in `new`, owned only by `self`, which gives it up here.
        *unsafe { Box::from_raw(self.into_raw().as_ptr()) }
    }
}

impl Drop for ThreadBox {
    fn drop(&mut self) {
        // SAFETY: allocated by `Box` in `new`, owned only by `self`.
        drop(unsafe { Box::from_raw(self.0.as_ptr()) });
    }
}

/// Threads linked through their `next` field, which the list owns: adding and removing one
/// never allocates. First in, first out, or ordered by wake time for the sleepers.
#[derive(Debug)]
struct ThreadList {
    head: Option<NonNull<Thread>>,
    /// The last thread; meaningless while the list is empty (`pop_front` leaves it stale, so a
    /// switch does not pay for clearing it).
    tail: NonNull<Thread>,
}

// SAFETY: the list owns its threads as `ThreadBox`es do.
unsafe impl Send for ThreadList {}

impl ThreadList {
    const fn new() -> Self {
        Self {
            head: None,
            tail: NonNull::dangling(),
        }
    }

    fn is_empty(&self) -> bool {
        self.head.is_none()
    }

    fn front(&self) -> Option<&Thread> {
        // SAFETY: the list owns its threads, and changing them takes `&mut self`.
        self.head.map(|thread| unsafe { thread.as_ref() })
    }

    #[inline]
    fn push_back(&mut self, thread: ThreadBox) {
        let thread = thread.into_raw();
        // SAFETY: owned by this list now and in no other; no reference to it is live.
        unsafe { (*thread.as_ptr()).next = None };
        if self.head.is_none() {
            self.head = Some(thread);
        } else {
            // SAFETY: with a head, the tail is the last thread the list owns; no reference to
            // it is live.
            unsafe { (*self.tail.as_ptr()).next = Some(thread) };
        }
        self.tail = thread;
    }

    #[inline]
    fn pop_front(&mut self) -> Option<ThreadBox> {
        let head = self.head?;
        // SAFETY: the head is a thread the list owns; no reference to it is live. Its `next`
        // is left as it is: whoever holds it next sets it before reading it.
        self.head = unsafe { (*head.as_ptr()).next };
        // SAFETY: from `into_raw` in `push_back` or `insert_by_wake_at`, and unlinked here.
        Some(unsafe { ThreadBox::from_raw(head) })
    }

    /// Adds `thread`, to wake at `wake_at`, after every thread that wakes at or before then:
    /// the list stays ordered by wake time, and threads due at the same time wake in the
    /// order they went to sleep. O(n) in the sleepers; finding the earliest is O(1).
    fn insert_by_wake_at(&mut self, thread: ThreadBox, wake_at: u64) {
        let thread = thread.into_raw();
        let mut previous: Option<NonNull<Thread>> = None;
        let mut following = self.head;
        while let Some(candidate) = following {
            // SAFETY: a thread the list owns; the reference ends before anything is written.
            let candidate = unsafe { candidate.as_ref() };
            if candidate.wake_at > wake_at {
                break;
            }
            previous = following;
            following = candidate.next;
        }
        // SAFETY: `thread` is owned by this list now and in no other; no reference to it is
        // live.
        unsafe {
            (*thread.as_ptr()).wake_at = wake_at;
        }
        // SAFETY: as above.
        unsafe {
            (*thread.as_ptr()).next = following;
        }
        match previous {
            // SAFETY: a thread the list owns; no reference to it is live.
            Some(previous) => unsafe { (*previous.as_ptr()).next = Some(thread) },
            None => self.head = Some(thread),
        }
        if following.is_none() {
            self.tail = thread;
        }
    }
}

impl Drop for ThreadList {
    fn drop(&mut self) {
        // A thread in a list is waiting to run; dropping it would free a stack it may still
        // be needed for, and leave whoever joins it waiting forever.
        assert!(self.is_empty(), "a list of waiting threads was dropped");
    }
}

/// Threads waiting for something: one waits with [`Self::wait_until`] until a condition holds,
/// and whoever makes it hold wakes it with [`Self::wake_one`] or [`Self::wake_all`]. The
/// waiting threads are linked through themselves, so waiting and waking never allocate.
/// Guarded by the scheduler's lock for now; per-queue locks come with SMP.
pub struct WaitQueue {
    /// Only reached through [`Self::waiters`], with the scheduler locked.
    waiters: UnsafeCell<ThreadList>,
}

// SAFETY: the list is only reached with the scheduler's lock held (`waiters` takes the
// scheduler by `&mut`), which serialises every use of it; the threads in it are `Send`.
unsafe impl Sync for WaitQueue {}

impl fmt::Debug for WaitQueue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WaitQueue").finish_non_exhaustive()
    }
}

impl Default for WaitQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl WaitQueue {
    pub const fn new() -> Self {
        Self {
            waiters: UnsafeCell::new(ThreadList::new()),
        }
    }

    /// The waiting threads. Borrowing the scheduler (whose one instance is behind its lock)
    /// for as long as the list proves the lock is held and no other reference to the list
    /// exists.
    fn waiters<'a>(&'a self, _scheduler: &'a mut Scheduler) -> &'a mut ThreadList {
        // SAFETY: see above: every reference to the list is made here, each borrowing the one
        // `Scheduler` mutably for its whole life, so they never overlap.
        unsafe { &mut *self.waiters.get() }
    }

    /// Returns once `condition` holds, running other threads meanwhile: while it does not,
    /// the thread waits in this queue until a [`Self::wake_one`] or [`Self::wake_all`], then
    /// checks again. `condition` runs with the scheduler's lock held, so a wake cannot slip in
    /// between its check and the wait: whoever makes it hold must do so before waking the
    /// queue. It must be quick, must not block, and may take only locks that come after the
    /// scheduler's (the heap's, the frame allocator's). Starts the scheduler if it has not
    /// started (see [`start`]), which stops the kernel if there is no memory for that.
    pub fn wait_until<P: Platform>(&self, mut condition: impl FnMut() -> bool) {
        loop {
            let mut scheduler = SCHEDULER.lock();
            if condition() {
                return;
            }
            scheduler.start_or_stop::<P>();
            block::<P>(scheduler, |scheduler, thread| {
                self.waiters(scheduler).push_back(thread);
            });
        }
    }

    /// Wakes the thread that has waited longest, if one waits; returns whether one did. It
    /// runs after the threads already ready to run.
    pub fn wake_one<P: Platform>(&self) -> bool {
        let mut scheduler = SCHEDULER.lock();
        let Some(thread) = self.waiters(&mut scheduler).pop_front() else {
            return false;
        };
        scheduler.make_ready::<P>(thread);
        true
    }

    /// Wakes every waiting thread, in the order they began to wait; returns how many.
    pub fn wake_all<P: Platform>(&self) -> usize {
        let mut scheduler = SCHEDULER.lock();
        let mut woken = 0;
        while let Some(thread) = self.waiters(&mut scheduler).pop_front() {
            scheduler.ready.push_back(thread);
            woken += 1;
        }
        if woken > 0 {
            scheduler.update_timer::<P>();
        }
        woken
    }
}

/// The scheduler's state for the one CPU.
#[derive(Debug)]
struct Scheduler {
    /// The running thread; `None` until the scheduler [`start`]s.
    current: Option<ThreadBox>,
    /// Threads ready to run, in the order they will.
    ready: ThreadList,
    /// Sleeping threads, the earliest to wake first.
    sleeping: ThreadList,
    /// The idle thread while it does not run; `None` while it runs (or before [`start`]).
    idle: Option<ThreadBox>,
    /// A thread that has exited, until the next thread to run frees it (no thread can free the
    /// stack it is running on).
    exited: Option<ThreadBox>,
    next_id: u64,
    /// The time slice in clock ticks while preemption is on ([`enable_preemption`]); 0 while
    /// it is off.
    slice_ticks: u64,
    /// While preemption is on: the clock tick at which the running thread's slice ends.
    slice_end: u64,
    /// The timer deadline the scheduler armed and has not seen fire: `u64::MAX` if none.
    /// Arming again for a later deadline is skipped (the earlier one fires first, and the
    /// scheduler then arms what it needs), so a switch rarely pays for arming the timer.
    armed: u64,
}

static SCHEDULER: SpinLock<Scheduler> = SpinLock::new(Scheduler {
    current: None,
    ready: ThreadList::new(),
    sleeping: ThreadList::new(),
    idle: None,
    exited: None,
    next_id: 0,
    slice_ticks: 0,
    slice_end: 0,
    armed: u64::MAX,
});

impl Scheduler {
    /// Makes the code running now the boot thread and creates the idle thread, unless the
    /// scheduler has started already.
    fn start<P: Platform>(&mut self) -> Result<(), SpawnError> {
        if self.current.is_some() {
            return Ok(());
        }
        let stack = Stack::allocate().ok_or(SpawnError::OutOfMemory)?;
        // SAFETY: the stack's frames are allocated, writable through the direct map and used
        // by nothing else; the idle thread never exits, so they stay allocated.
        let context = unsafe { P::prepare_thread(stack.top(), idle_thread::<P>, 0) };
        let boot = self.new_thread(0, None);
        self.idle = Some(self.new_thread(context, Some(stack)));
        self.current = Some(boot);
        Ok(())
    }

    /// [`Self::start`] for the blocking calls, which cannot fail: without the memory for the
    /// idle thread's stack, nothing more can run anyway.
    fn start_or_stop<P: Platform>(&mut self) {
        if self.start::<P>().is_err() {
            panic!("no memory for the idle thread's stack");
        }
    }

    fn new_thread(&mut self, context: usize, stack: Option<Stack>) -> ThreadBox {
        let id = ThreadId(self.next_id);
        self.next_id += 1;
        ThreadBox::new(Thread {
            id,
            context,
            stack,
            joined: Arc::new(Joined {
                finished: AtomicBool::new(false),
                joiners: WaitQueue::new(),
            }),
            next: None,
            wake_at: 0,
        })
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

    /// Adds a woken or new thread to the ready threads.
    fn make_ready<P: Platform>(&mut self, thread: ThreadBox) {
        self.ready.push_back(thread);
        // The running thread may have been alone, with no tick armed.
        self.update_timer::<P>();
    }

    /// Whether the running thread's slice needs the timer: preemption is on, another thread is
    /// ready to run, and the running thread is not the idle thread (which lets them run
    /// anyway).
    fn tick_needed(&self) -> bool {
        self.slice_ticks != 0 && !self.ready.is_empty() && self.idle.is_some()
    }

    /// The earliest clock tick the scheduler needs the timer to fire at: the slice's end if
    /// [`Self::tick_needed`], and the earliest sleeper's wake time. `u64::MAX` for never.
    fn timer_needed(&self) -> u64 {
        let sleeper = self
            .sleeping
            .front()
            .map_or(u64::MAX, |thread| thread.wake_at);
        if self.tick_needed() {
            sleeper.min(self.slice_end)
        } else {
            sleeper
        }
    }

    /// Arms the timer for [`Self::timer_needed`] if that is earlier than what is armed.
    #[inline]
    fn update_timer<P: Platform>(&mut self) {
        let deadline = self.timer_needed();
        if deadline < self.armed {
            self.arm::<P>(deadline);
        }
    }

    /// Out of line: arming costs far more than the call on every architecture (an SBI call on
    /// RISC-V), and keeping it out of the switch path keeps that path's registers free.
    #[inline(never)]
    fn arm<P: Platform>(&mut self, deadline: u64) {
        P::set_timer_deadline(deadline);
        self.armed = deadline;
    }

    /// Starts the running thread's time slice, if preemption is on and it is not the idle
    /// thread.
    #[inline]
    fn start_slice<P: Platform>(&mut self) {
        if self.slice_ticks != 0 && self.idle.is_some() {
            self.start_slice_now::<P>();
        }
    }

    #[inline(never)]
    fn start_slice_now<P: Platform>(&mut self) {
        self.slice_end = P::clock_ticks().saturating_add(self.slice_ticks);
        self.update_timer::<P>();
    }

    /// If the timer has fired since the scheduler last looked: wakes the sleepers whose time
    /// has come and arms the timer for what is needed next. Returns whether the running
    /// thread's slice is over (then the caller yields, which starts the next slice, so the
    /// tick is not armed here). Two loads and a compare when it has not fired.
    #[inline]
    fn timer_events<P: Platform>(&mut self) -> bool {
        let seen = interrupt::timer_interrupts();
        if seen == TIMER_INTERRUPTS_SEEN.load(Ordering::Relaxed) {
            return false;
        }
        self.timer_fired::<P>(seen)
    }

    /// [`Self::timer_events`]' slow path; `seen` is the timer interrupt count now.
    #[cold]
    #[inline(never)]
    fn timer_fired<P: Platform>(&mut self, seen: u64) -> bool {
        TIMER_INTERRUPTS_SEEN.store(seen, Ordering::Relaxed);
        // The timer fires once per arming, so whatever was armed has fired.
        self.armed = u64::MAX;
        let now = P::clock_ticks();
        while self
            .sleeping
            .front()
            .is_some_and(|thread| thread.wake_at <= now)
        {
            let thread = self.sleeping.pop_front().expect("a sleeper is due");
            self.ready.push_back(thread);
        }
        let slice_over = self.tick_needed() && now >= self.slice_end;
        if !slice_over {
            self.update_timer::<P>();
        }
        slice_over
    }
}

/// A started thread. Dropping the handle lets the thread run on, unjoined.
#[derive(Debug)]
pub struct JoinHandle {
    id: ThreadId,
    joined: Arc<Joined>,
}

impl JoinHandle {
    pub fn id(&self) -> ThreadId {
        self.id
    }

    /// Whether the thread has exited and its stack is freed.
    pub fn is_finished(&self) -> bool {
        self.joined.finished.load(Ordering::Acquire)
    }

    /// Waits, running other threads, until this one has exited and its stack is freed.
    pub fn join<P: Platform>(self) {
        debug_assert!(
            SCHEDULER
                .lock()
                .current
                .as_ref()
                .is_none_or(|thread| thread.get().id != self.id),
            "a thread joined itself, which never returns"
        );
        self.joined.joiners.wait_until::<P>(|| self.is_finished());
    }
}

/// Starts the scheduler, if it has not started: the code running now becomes the boot thread,
/// and the idle thread gets its stack. [`spawn`] does this the first time; call it first to
/// count a thread's frames alone.
pub fn start<P: Platform>() -> Result<(), SpawnError> {
    SCHEDULER.lock().start::<P>()
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
    if let Err(error) = scheduler.start::<P>() {
        drop(scheduler);
        // SAFETY: from `Box::into_raw` above; the thread that would have owned it never
        // existed. (The stack is freed on return.)
        drop(unsafe { Box::from_raw(start) });
        return Err(error);
    }
    let thread = scheduler.new_thread(context, Some(stack));
    let handle = JoinHandle {
        id: thread.get().id,
        joined: thread.get().joined.clone(),
    };
    scheduler.make_ready::<P>(thread);
    Ok(handle)
}

/// Lets the next ready thread run, if there is one; returns when this thread's turn comes
/// again.
pub fn yield_now<P: Platform>() {
    let mut scheduler = SCHEDULER.lock();
    // Sleepers that are due run before this thread's next turn.
    scheduler.timer_events::<P>();
    let Some(next) = scheduler.ready.pop_front() else {
        return;
    };
    switch_to::<P>(scheduler, next, |scheduler, previous| {
        scheduler.ready.push_back(previous);
    });
}

/// Sleeps until [`Platform::clock_ticks`] reaches `deadline`, running other threads
/// meanwhile; returns at once if it has. The thread wakes at its next chance after the
/// deadline, never before: when the CPU is idle, at the timer interrupt; otherwise at the
/// running thread's next preemption point, yield or block. Starts the scheduler if it has not
/// started (see [`WaitQueue::wait_until`]).
pub fn sleep_until<P: Platform>(deadline: u64) {
    let mut scheduler = SCHEDULER.lock();
    if P::clock_ticks() >= deadline {
        return;
    }
    scheduler.start_or_stop::<P>();
    block::<P>(scheduler, |scheduler, thread| {
        scheduler.sleeping.insert_by_wake_at(thread, deadline);
        scheduler.update_timer::<P>();
    });
}

/// Sleeps for at least `duration` (see [`sleep_until`]).
pub fn sleep<P: Platform>(duration: Duration) {
    let nanoseconds = u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
    let ticks = mul_div_ceil(nanoseconds, P::clock_frequency(), 1_000_000_000);
    sleep_until::<P>(P::clock_ticks().saturating_add(ticks));
}

/// Turns preemption on (D37): every [`TIME_SLICE_MS`], the running thread lets the next ready
/// thread run at its next [`preemption_point`]. A slice starts now and with every switch; the
/// timer ends it only while another thread is ready to run.
pub fn enable_preemption<P: Platform>() {
    let mut scheduler = SCHEDULER.lock();
    scheduler.slice_ticks = (P::clock_frequency() * TIME_SLICE_MS / 1000).max(1);
    scheduler.start_slice::<P>();
}

/// Turns preemption off; threads run until they yield or block. Call it with interrupts
/// masked, as the kernel runs: the timer is armed again for the sleepers alone, or for never,
/// so no tick arrives afterwards.
pub fn disable_preemption<P: Platform>() {
    let mut scheduler = SCHEDULER.lock();
    scheduler.slice_ticks = 0;
    let deadline = scheduler.timer_needed();
    P::set_timer_deadline(deadline);
    scheduler.armed = deadline;
}

/// Where the running thread lets the next one run if its time slice is over (D37), and where
/// sleepers that are due wake: kernel code that runs for long calls it regularly. As cheap as
/// a few loads when the timer has not fired. Takes the timer interrupt if one is pending, then,
/// if a timer interrupt has come since the scheduler last looked, wakes the sleepers that are
/// due and yields if the slice is over. Call it as [`yield_now`] is called: with interrupts
/// masked and no lock held.
#[inline]
pub fn preemption_point<P: Platform>() {
    P::interrupt_window();
    if interrupt::timer_interrupts() != TIMER_INTERRUPTS_SEEN.load(Ordering::Relaxed) {
        preemption_point_timer_fired::<P>();
    }
}

/// [`preemption_point`]'s slow path, out of line so the check stays a few instructions.
#[cold]
#[inline(never)]
fn preemption_point_timer_fired<P: Platform>() {
    let mut scheduler = SCHEDULER.lock();
    if !scheduler.timer_events::<P>() {
        return;
    }
    let next = scheduler
        .ready
        .pop_front()
        .expect("a slice ends only while another thread is ready");
    // The next thread's slice; it ends at a tick whoever runs then.
    scheduler.start_slice::<P>();
    switch_to::<P>(scheduler, next, |scheduler, previous| {
        scheduler.ready.push_back(previous);
    });
}

/// Stops the running thread until something makes it ready again: `park` puts it where it
/// waits (a wait queue, the sleepers). The next ready thread runs, or the idle thread if none
/// is. The scheduler must have started.
fn block<P: Platform>(
    mut scheduler: SpinLockGuard<'_, Scheduler>,
    park: impl FnOnce(&mut Scheduler, ThreadBox),
) {
    scheduler.timer_events::<P>();
    let next = match scheduler.ready.pop_front() {
        Some(next) => next,
        None => scheduler
            .idle
            .take()
            .expect("the scheduler started, so the idle thread is not running"),
    };
    switch_to::<P>(scheduler, next, park);
}

/// Switches from the running thread to `next`, after `park` has put the running one where it
/// goes (the ready threads, a wait queue, the sleepers, the exited slot, or the idle slot for
/// the idle thread). Returns when the running thread is resumed.
fn switch_to<P: Platform>(
    mut scheduler: SpinLockGuard<'_, Scheduler>,
    next: ThreadBox,
    park: impl FnOnce(&mut Scheduler, ThreadBox),
) {
    let resume = next.get().context;
    let previous = scheduler.replace_current(next);
    let save = previous.context();
    park(&mut scheduler, previous);
    // Held across the switch, for the thread it resumes to release.
    SpinLockGuard::leak(scheduler);
    // SAFETY: interrupts are masked, as the kernel runs. `save` points into the previous
    // thread, which `park` keeps allocated (in a list or a slot of the scheduler) and which is
    // not resumed before this switch has saved it: that takes the scheduler's lock, which stays
    // held until then. `resume` was saved by the switch away from `next` (or prepared by
    // `spawn` or `start`) and not resumed since: a thread is in a list or slot only while it
    // is not running.
    unsafe { P::switch_thread(save, resume) };
    finish_switch::<P>();
}

/// Ends the running thread: switches to the next one, which frees this one's stack.
fn exit<P: Platform>() -> ! {
    let mut scheduler = SCHEDULER.lock();
    let next = match scheduler.ready.pop_front() {
        Some(next) => next,
        None => scheduler
            .idle
            .take()
            .expect("a spawned thread runs, so the idle thread does not"),
    };
    switch_to::<P>(scheduler, next, |scheduler, previous| {
        debug_assert!(
            scheduler.exited.is_none(),
            "every switch frees the exited thread"
        );
        scheduler.exited = Some(previous);
    });
    unreachable!("an exited thread was resumed");
}

/// Runs in every thread right after a switch to it: releases the scheduler's lock, which the
/// switch handed over, then, if the switch came from a thread that exited, frees it (its stack
/// is no longer in use, and the lock is not held while it is freed) and wakes its joiners.
#[inline]
fn finish_switch<P: Platform>() {
    // SAFETY: every switch is made holding the scheduler's lock and leaves it held for the
    // thread it resumes, which is this one, once.
    let mut scheduler = unsafe { SCHEDULER.assume_locked() };
    // Checked before it is taken, so a switch from a thread that did not exit writes nothing.
    if scheduler.exited.is_none() {
        return;
    }
    let exited = scheduler.exited.take();
    drop(scheduler);
    if let Some(exited) = exited {
        free_exited::<P>(exited);
    }
}

/// [`finish_switch`]'s slow path, out of line so a switch from a thread that did not exit
/// does not pay for its registers.
#[cold]
#[inline(never)]
fn free_exited<P: Platform>(exited: ThreadBox) {
    let Thread { stack, joined, .. } = exited.into_inner();
    drop(stack);
    joined.finished.store(true, Ordering::Release);
    joined.joiners.wake_all::<P>();
}

/// Where every spawned thread starts, on its own stack: `argument` is the function `spawn`
/// boxed.
extern "C" fn thread_start<P: Platform>(argument: usize) -> ! {
    finish_switch::<P>();
    // SAFETY: `spawn` passed a pointer from `Box::into_raw` to this thread alone, which starts
    // once.
    let function = unsafe { Box::from_raw(argument as *mut Box<dyn FnOnce() + Send>) };
    function();
    exit::<P>()
}

/// The idle thread: runs only when no other thread is ready, and lets the first that becomes
/// ready run. Meanwhile it arms the timer for the earliest sleeper alone (no tick) and waits
/// for an interrupt.
extern "C" fn idle_thread<P: Platform>(_: usize) -> ! {
    finish_switch::<P>();
    loop {
        let mut scheduler = SCHEDULER.lock();
        scheduler.timer_events::<P>();
        if let Some(next) = scheduler.ready.pop_front() {
            switch_to::<P>(scheduler, next, |scheduler, idle| {
                scheduler.idle = Some(idle);
            });
            continue;
        }
        // Exactly what is needed, so the CPU is not woken early for a stale deadline.
        let deadline = scheduler.timer_needed();
        if deadline != scheduler.armed {
            scheduler.arm::<P>(deadline);
        }
        drop(scheduler);
        // Nothing can make a thread ready on this CPU without an interrupt, and one that is
        // already pending is taken at once.
        P::wait_for_interrupt();
    }
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
        // The idle thread's stack is taken once, when the scheduler starts.
        start::<P>().unwrap();
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
        // Off even if a test that turned it on failed before turning it off.
        disable_preemption::<P>();
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
    fn sleepers_wake_in_deadline_order_and_never_early() {
        let _timer = crate::interrupt::lock_timer_interrupts_for_test();
        let _frames = host::frames_for_test();
        disable_preemption::<P>();
        static WOKEN: SpinLock<Vec<(u64, bool)>> = SpinLock::new(Vec::new());
        WOKEN.lock().clear();
        let base = P::clock_ticks();
        // Each sleeps until `base + 1000 * order`; spawned out of order.
        let handles: Vec<JoinHandle> = [3u64, 1, 2]
            .into_iter()
            .map(|order| {
                spawn::<P, _>(move || {
                    let deadline = base + 1000 * order;
                    sleep_until::<P>(deadline);
                    let on_time = P::clock_ticks() >= deadline;
                    WOKEN.lock().push((order, on_time));
                })
                .unwrap()
            })
            .collect();
        // The boot thread blocks in `join`; with every thread asleep, the idle thread runs
        // and waits for the timer, which the test platform's clock jumps to.
        for handle in handles {
            handle.join::<P>();
        }
        assert_eq!(*WOKEN.lock(), [(1, true), (2, true), (3, true)]);
        assert!(SCHEDULER.lock().sleeping.is_empty());
    }

    #[test]
    fn a_deadline_already_passed_does_not_block() {
        let _timer = crate::interrupt::lock_timer_interrupts_for_test();
        let _frames = host::frames_for_test();
        static RAN: AtomicBool = AtomicBool::new(false);
        RAN.store(false, Ordering::Relaxed);
        let other = spawn::<P, _>(|| RAN.store(true, Ordering::Relaxed)).unwrap();
        sleep_until::<P>(0);
        sleep::<P>(Duration::ZERO);
        assert!(!RAN.load(Ordering::Relaxed), "sleeping switched threads");
        other.join::<P>();
        assert!(RAN.load(Ordering::Relaxed));
    }

    #[test]
    fn a_wait_queue_wakes_one_thread_or_all_in_the_order_they_waited() {
        let _frames = host::frames_for_test();
        static QUEUE: WaitQueue = WaitQueue::new();
        static OPEN: AtomicU64 = AtomicU64::new(0);
        static LOG: SpinLock<Vec<u64>> = SpinLock::new(Vec::new());
        OPEN.store(0, Ordering::Relaxed);
        LOG.lock().clear();
        assert!(!QUEUE.wake_one::<P>(), "nobody waits yet");
        let handles: Vec<JoinHandle> = (1..=3u64)
            .map(|thread| {
                spawn::<P, _>(move || {
                    QUEUE.wait_until::<P>(|| OPEN.load(Ordering::Relaxed) >= thread);
                    LOG.lock().push(thread);
                })
                .unwrap()
            })
            .collect();
        // Every thread runs once and waits.
        yield_now::<P>();
        assert!(LOG.lock().is_empty());
        // Woken while its condition does not hold, a thread waits again.
        assert!(QUEUE.wake_one::<P>());
        yield_now::<P>();
        assert!(LOG.lock().is_empty());
        OPEN.store(1, Ordering::Relaxed);
        // Thread 1 waited again after threads 2 and 3: wake all of them, then two of them
        // wait again.
        assert_eq!(QUEUE.wake_all::<P>(), 3);
        yield_now::<P>();
        assert_eq!(*LOG.lock(), [1]);
        OPEN.store(3, Ordering::Relaxed);
        assert!(QUEUE.wake_one::<P>());
        yield_now::<P>();
        assert_eq!(*LOG.lock(), [1, 2]);
        assert!(QUEUE.wake_one::<P>());
        for handle in handles {
            handle.join::<P>();
        }
        assert_eq!(*LOG.lock(), [1, 2, 3]);
        assert!(!QUEUE.wake_one::<P>());
    }

    #[test]
    fn a_condition_that_holds_never_waits() {
        let _frames = host::frames_for_test();
        static QUEUE: WaitQueue = WaitQueue::new();
        static RAN: AtomicBool = AtomicBool::new(false);
        RAN.store(false, Ordering::Relaxed);
        let other = spawn::<P, _>(|| RAN.store(true, Ordering::Relaxed)).unwrap();
        QUEUE.wait_until::<P>(|| true);
        assert!(!RAN.load(Ordering::Relaxed), "waiting switched threads");
        other.join::<P>();
    }

    #[test]
    fn the_tick_is_armed_only_while_another_thread_is_ready() {
        let _timer = crate::interrupt::lock_timer_interrupts_for_test();
        let _frames = host::frames_for_test();
        start::<P>().unwrap();
        disable_preemption::<P>();
        enable_preemption::<P>();
        // Only the boot thread is runnable: no tick.
        assert_eq!(crate::tests::timer_deadline(), u64::MAX);
        let other = spawn::<P, _>(|| {}).unwrap();
        let deadline = crate::tests::timer_deadline();
        assert_ne!(deadline, u64::MAX, "a ready thread brings the tick back");
        assert!(deadline <= P::clock_ticks() + 10_000, "within one slice");
        other.join::<P>();
        disable_preemption::<P>();
        assert_eq!(crate::tests::timer_deadline(), u64::MAX);
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
            joined: Arc::new(Joined {
                finished: AtomicBool::new(false),
                joiners: WaitQueue::new(),
            }),
            next: None,
            wake_at: 0,
        });
        thread.check_stack();
        memory[0] = 0;
        thread.check_stack();
    }
}
