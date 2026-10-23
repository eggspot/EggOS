//! Kernel threads and the scheduler (D35, D37, D39, D43, D44).
//!
//! M4: threads that run kernel code on their own stacks, scheduled round-robin on each CPU. A
//! thread runs until it calls [`yield_now`], blocks ([`sleep_until`], [`WaitQueue::wait_until`],
//! [`JoinHandle::join`]), its function returns, or its time slice is over at a
//! [`preemption_point`]; [`spawn`] starts one. The boot code becomes a thread too, the first
//! time it uses the scheduler (its stack is the boot stack). Every CPU has an idle thread, which
//! runs only when every other thread on that CPU is blocked: it waits for an interrupt and lets
//! the first thread that becomes ready run. A secondary CPU's start code becomes its idle thread.
//!
//! Every CPU has its own scheduler state (D43): its running thread, ready threads, sleepers, idle
//! thread and timer, in its per-CPU block ([`Cpu`]). A thread stays on the CPU [`spawn`] placed
//! it on (an idle CPU if there is one, otherwise the next CPU in turn): it is woken there, sleeps
//! on that CPU's timer, and runs there, until balancing moves it. A thread made ready on another
//! CPU is announced to it: an idle CPU gets the wake-up interrupt ([`cpu::wake`]), a busy one with
//! preemption on a reschedule request its next preemption point sees, so a tick is armed for the
//! new thread's turn.
//!
//! Balancing (D44): an idle CPU with nothing ready takes a ready thread from a busy CPU, one
//! running another thread, and the thread stays on its new CPU. A busy CPU whose slice ends with
//! threads ready sends an idle CPU the wake-up interrupt, so that it takes one: a thread waits a
//! slice or a few while a CPU is idle (the woken CPU takes from the next busy CPU after it, not
//! necessarily the one whose slice ended). Both happen off the hot paths (in the idle thread and
//! when a slice ends), so a switch, a wait and a wake cost what they did.
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
//! Locking: one lock, the scheduler's, guards every CPU's scheduler state and every
//! [`WaitQueue`] for now; per-CPU and per-queue locks come once a workload shows contention
//! (D45). It is taken before the
//! heap's and the frame allocator's, never after; an exited thread's stack is freed after it
//! is released. A switch is made holding it, and the lock is handed over to the thread the
//! switch resumes, which releases it (`finish_switch`): from choosing the next thread until
//! the previous one's context is saved, nothing else can take the scheduler, on this CPU or
//! another.

use crate::cpu::{self, Cpu};
use crate::heap::{FrameSource, GlobalFrames};
use crate::sync::{SpinLock, SpinLockGuard};
use alloc::boxed::Box;
use alloc::sync::Arc;
use core::alloc::Layout;
use core::cell::{Cell, OnceCell, UnsafeCell};
use core::fmt;
use core::num::NonZeroUsize;
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use core::time::Duration;
use eggos_hal::Platform;
use eggos_hal::fp::FpState;
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

/// Identifies a thread for as long as the kernel runs; never reused. The boot thread is 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ThreadId(u64);

impl ThreadId {
    const BOOT: Self = Self(0);

    /// A new id, never handed out before; the boot thread's is reserved.
    fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

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
    /// Direct-map address of the lowest byte, which holds the canary. Never 0, so an
    /// `Option<Stack>` is one word and a switch checks a thread's stack with one load fewer.
    base: NonZeroUsize,
}

impl Stack {
    fn allocate() -> Option<Self> {
        let base = GlobalFrames.allocate_frames(STACK_ORDER)?;
        let base = NonZeroUsize::new(base).expect("the direct map never maps address 0");
        let stack = Self { base };
        // SAFETY: `base` is the first, 16 KiB-aligned word of the frames just allocated, which
        // this stack owns and which the direct map makes writable.
        unsafe { stack.canary().write(STACK_CANARY) };
        Some(stack)
    }

    fn canary(&self) -> *mut u64 {
        self.base.get() as *mut u64
    }

    fn top(&self) -> usize {
        self.base.get() + STACK_SIZE
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
        unsafe { GlobalFrames.free_frames(self.base.get(), STACK_ORDER) };
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
    /// `None` for the boot thread, which runs on the boot stack, and a secondary CPU's idle
    /// thread, which runs on the stack the CPU started on.
    stack: Option<Stack>,
    joined: Arc<Joined>,
    /// The next thread in the [`ThreadList`] that holds this one: the ready threads, a wait
    /// queue or the sleepers. A thread is in at most one list, and in none while it runs.
    next: Option<NonNull<Thread>>,
    /// While it sleeps: the [`Platform::clock_ticks`] it wakes at.
    wake_at: u64,
    /// The CPU it runs on (D43): it is made ready there when woken, and sleeps on that CPU's
    /// timer, until balancing moves it to an idle CPU while it is ready (D44).
    cpu: &'static Cpu,
    /// Whether it runs a program in user mode ([`spawn_user`]), where it reaches no preemption
    /// point until a system call, an interrupt or an exception (D48).
    user: bool,
    /// A user thread's FP/SIMD registers, once its program has used them ([`start_fp`], D54).
    fp: OnceCell<FpRegisters>,
}

/// A thread's FP/SIMD registers (D54): where they are saved, and the CPU that last loaded them.
/// Only switches to and from the thread, with the scheduler's lock held, and the thread itself
/// while it runs ([`start_fp`]) touch them.
#[derive(Debug)]
struct FpRegisters {
    /// Allocated by [`start_fp`], freed with the thread. A raw pointer: the switch writes the
    /// registers through it while the scheduler holds only shared references to the thread.
    state: NonNull<FpState>,
    /// The CPU that last loaded them ([`CpuFp::switch`]).
    cpu: Cell<*const Cpu>,
}

impl Drop for FpRegisters {
    fn drop(&mut self) {
        // SAFETY: allocated with this layout in `start_fp`, owned only by `self`.
        unsafe { alloc::alloc::dealloc(self.state.as_ptr().cast(), Layout::new::<FpState>()) };
    }
}

impl Thread {
    fn boxed(
        id: ThreadId,
        context: usize,
        stack: Option<Stack>,
        cpu: &'static Cpu,
        user: bool,
    ) -> ThreadBox {
        ThreadBox::new(Self {
            id,
            context,
            stack,
            joined: Arc::new(Joined {
                finished: AtomicBool::new(false),
                joiners: WaitQueue::new(),
            }),
            next: None,
            wake_at: 0,
            cpu,
            user,
            fp: OnceCell::new(),
        })
    }

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
#[repr(transparent)]
struct ThreadBox(NonNull<Thread>);

// SAFETY: a `ThreadBox` owns its `Thread` as a `Box` would. Everything in a `Thread` is `Send`
// (its `cpu` is a shared reference to a `Sync` block) but its `next` link, a raw pointer that
// only the `ThreadList` holding the thread follows, with the scheduler's lock held, and its FP
// registers' pointers, which only the scheduler's switches, with the lock held, and the thread
// itself follow; so moving the owner to another CPU shares nothing unguarded.
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
        // SAFETY: owned by `self`. Only the switch (holding the scheduler's lock, handed over
        // across the switch) and a `ThreadList` (once `into_raw` has given the box up) write
        // it without a reference, and no `get` overlaps either; `move_to` borrows `self`
        // uniquely.
        unsafe { self.0.as_ref() }
    }

    /// Moves the thread, which is ready, to `cpu` (D44): it runs there, and sleeps and is made
    /// ready there from now on.
    fn move_to(&mut self, cpu: &'static Cpu) {
        // SAFETY: owned by `self`, borrowed uniquely. The thread is ready, just taken from a
        // ready list with the scheduler's lock held: it does not run, and the switch that saved
        // it finished before the lock was handed on (see `switch_to`), so nothing else reaches
        // it while this reference lives.
        unsafe { self.0.as_mut().cpu = cpu };
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

// SAFETY: the list owns its threads as `ThreadBox`es do (see there), and its links are only
// followed through `&mut self` or `&self`.
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
        if self.head.is_none() {
            self.head = Some(thread);
        } else {
            // SAFETY: with a head, the tail is the last thread the list owns; no reference to
            // it is live.
            unsafe { (*self.tail.as_ptr()).next = Some(thread) };
        }
        // Written after the list is read, so that the compiler need not read it again (it
        // cannot tell the thread from the list's memory).
        // SAFETY: owned by this list now and in no other (it was in none, so it is not the
        // tail); no reference to it is live.
        unsafe { (*thread.as_ptr()).next = None };
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
/// waiting threads are linked through themselves, so waiting and waking never allocate. A
/// waiter on any CPU may be woken from any CPU; it runs again on its own. Guarded by the
/// scheduler's lock for now; per-queue locks come once a workload shows contention (D45).
pub struct WaitQueue {
    /// Only reached through [`Self::waiters`], with the scheduler locked.
    waiters: UnsafeCell<ThreadList>,
}

// SAFETY: the list is only reached with the scheduler's lock held (`waiters` takes the
// lock's `Schedulers` by `&mut`), which serialises every use of it; the threads in it are `Send`.
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

    /// The waiting threads. Borrowing the scheduler's `Schedulers` (whose one instance is
    /// behind its lock) for as long as the list proves the lock is held and no other reference
    /// to the list exists.
    fn waiters<'a>(&'a self, _schedulers: &'a mut Schedulers) -> &'a mut ThreadList {
        // SAFETY: see above: every reference to the list is made here, each borrowing the one
        // `Schedulers` mutably for its whole life, so they never overlap.
        unsafe { &mut *self.waiters.get() }
    }

    /// Returns once `condition` holds, running other threads meanwhile: while it does not,
    /// the thread waits in this queue until a [`Self::wake_one`] or [`Self::wake_all`], then
    /// checks again. `condition` runs with the scheduler's lock held, so a wake cannot slip in
    /// between its check and the wait: whoever makes it hold must do so before waking the
    /// queue. It must be quick, must not block or wake a queue (the scheduler's lock is not
    /// re-entrant), and may take only locks that come after the scheduler's (the heap's, the
    /// frame allocator's). Call it as [`yield_now`] is called: with interrupts masked and no
    /// lock held. Starts the scheduler if it has not started (see [`start`]), which stops the
    /// kernel if there is no memory for that.
    pub fn wait_until<P: Platform>(&self, mut condition: impl FnMut() -> bool) {
        loop {
            let mut schedulers = SCHEDULER.lock();
            if condition() {
                return;
            }
            start_here_or_panic::<P>(&mut schedulers);
            block::<P>(schedulers, |schedulers, _, thread| {
                self.waiters(schedulers).push_back(thread);
            });
        }
    }

    /// Wakes the thread that has waited longest, if one waits; returns whether one did. It
    /// runs on its own CPU, after the threads already ready to run there. Meant for queues
    /// whose waiters all wait for the same condition: a woken thread whose own condition does
    /// not hold waits again at the back, and the wake is spent; otherwise use
    /// [`Self::wake_all`]. Inlined, so that waking a queue nobody waits in costs its few
    /// instructions where it is called.
    #[inline]
    pub fn wake_one<P: Platform>(&self) -> bool {
        let mut schedulers = SCHEDULER.lock();
        let Some(thread) = self.waiters(&mut schedulers).pop_front() else {
            return false;
        };
        let wake = make_ready::<P>(&mut schedulers, thread);
        drop(schedulers);
        if let Some(cpu) = wake {
            cpu::wake::<P>(cpu);
        }
        true
    }

    /// Wakes every waiting thread, in the order they began to wait; returns how many. Idle
    /// CPUs among theirs get the wake-up interrupt with the scheduler's lock still held: the
    /// threads cannot be kept anywhere else meanwhile without allocating.
    pub fn wake_all<P: Platform>(&self) -> usize {
        let mut schedulers = SCHEDULER.lock();
        let mut woken = 0;
        while let Some(thread) = self.waiters(&mut schedulers).pop_front() {
            if let Some(cpu) = make_ready::<P>(&mut schedulers, thread) {
                cpu::wake::<P>(cpu);
            }
            woken += 1;
        }
        woken
    }
}

/// What a CPU must be told about a thread made ready on it ([`Scheduler::push_ready`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Notice {
    /// Nothing: it runs the thread in turn, and a tick is armed if one is needed for that.
    Nothing,
    /// Its scheduler must look at its ready threads at its next preemption point, yield or
    /// block ([`Cpu::request_reschedule`]): with preemption on, its running thread was alone,
    /// so no tick is armed to end its slice.
    Reschedule,
    /// Its idle thread waits for an interrupt: it needs the wake-up interrupt ([`cpu::wake`]),
    /// unless it is the CPU making the thread ready.
    Wake,
}

/// One CPU's scheduler state, in its per-CPU block ([`Cpu::scheduler`]), reached only with the
/// scheduler's lock held ([`Schedulers::of`]). Not `Debug`: reading it needs the lock.
#[repr(transparent)]
pub(crate) struct CpuScheduler(UnsafeCell<Scheduler>);

// SAFETY: the state is only reached through `Schedulers::of`, which takes the one `Schedulers`
// behind the scheduler's lock by `&mut`, so every use is serialised by that lock; everything in
// it is `Send`.
unsafe impl Sync for CpuScheduler {}

impl CpuScheduler {
    pub(crate) const fn new() -> Self {
        Self(UnsafeCell::new(Scheduler::new()))
    }
}

impl fmt::Debug for CpuScheduler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CpuScheduler").finish_non_exhaustive()
    }
}

/// What the scheduler's lock guards beyond the wait queues: every CPU's [`Scheduler`], reached
/// one at a time through [`Self::of`], and what they share.
#[derive(Debug)]
struct Schedulers {
    /// The time slice in clock ticks while preemption is on, 0 while it is off. Each CPU's
    /// scheduler keeps a copy, so a CPU that starts later takes it from here.
    slice_ticks: u64,
    /// Where [`Self::place`] looks first when no CPU is idle: an index into the CPU table.
    next_placement: usize,
}

static SCHEDULER: SpinLock<Schedulers> = SpinLock::new(Schedulers {
    slice_ticks: 0,
    next_placement: 0,
});

impl Schedulers {
    /// `cpu`'s scheduler state. Borrowing `self` (whose one instance is behind the scheduler's
    /// lock) for as long as the state proves the lock is held and no other reference to any
    /// CPU's state exists.
    #[inline]
    fn of<'a>(&'a mut self, cpu: &'a Cpu) -> &'a mut Scheduler {
        // SAFETY: see above: every reference to a CPU's state is made here, each borrowing the
        // one `Schedulers` mutably for its whole life, so they never overlap. (Host tests build
        // `Schedulers` of their own only over CPU blocks of their own, which `SCHEDULER` never
        // reaches.)
        unsafe { &mut *cpu.scheduler.0.get() }
    }

    /// The CPU a new thread runs on, from `here`, the CPU spawning it: an idle CPU if there is
    /// one, the lowest-numbered, so new threads spread over the idle CPUs first; otherwise the
    /// next CPU in turn whose scheduler has started. O(CPUs); spawning is not a hot path.
    fn place(&mut self, here: &'static Cpu) -> &'static Cpu {
        self.place_among(cpu::CPUS.all(), here)
    }

    /// [`Self::place`] among `cpus`; `here` if there is no other.
    fn place_among(&mut self, cpus: &[&'static Cpu], here: &'static Cpu) -> &'static Cpu {
        if cpus.len() <= 1 {
            return here;
        }
        if let Some(&idle) = cpus.iter().find(|&&cpu| self.of(cpu).is_idle()) {
            return idle;
        }
        for _ in 0..cpus.len() {
            let index = self.next_placement % cpus.len();
            self.next_placement = (index + 1) % cpus.len();
            let cpu = cpus[index];
            if self.of(cpu).has_started() {
                return cpu;
            }
        }
        here
    }

    /// A thread for `here`'s idle thread to run when none is ready there (D44): the first ready
    /// thread of the next busy CPU after `here`, moved to `here`. A busy CPU runs a thread other
    /// than its idle thread, so its ready threads wait; one whose idle thread runs is left alone,
    /// since it runs its ready threads itself, having been told of them. The boot thread is never
    /// moved: it runs the boot CPU's setup and the self-tests, which expect to run there; a CPU
    /// whose first ready thread is the boot thread is passed over. O(CPUs), on the idle path.
    fn steal(&mut self, here: &'static Cpu) -> Option<ThreadBox> {
        self.steal_among(cpu::CPUS.all(), here)
    }

    /// [`Self::steal`] among `cpus`.
    fn steal_among(&mut self, cpus: &[&'static Cpu], here: &'static Cpu) -> Option<ThreadBox> {
        let first = here.index() + 1;
        for offset in 0..cpus.len() {
            let cpu = cpus[(first + offset) % cpus.len()];
            if ptr::eq(cpu, here) {
                continue;
            }
            let scheduler = self.of(cpu);
            // Its idle thread is in its slot only while another thread runs.
            let busy = scheduler.idle.is_some();
            let movable = scheduler
                .ready
                .front()
                .is_some_and(|thread| thread.id != ThreadId::BOOT);
            if busy && movable {
                let mut thread = scheduler.ready.pop_front().expect("a thread is ready");
                thread.move_to(here);
                return Some(thread);
            }
        }
        None
    }

    /// The idle CPU to send the wake-up interrupt to when `here`'s slice ends with threads ready
    /// (D44), so that it takes one ([`Self::steal`]): the lowest-numbered idle CPU not yet sent
    /// one since it last looked, marked as sent. O(CPUs), once per slice that ends.
    fn idle_cpu_to_wake(&mut self, here: &'static Cpu) -> Option<&'static Cpu> {
        self.idle_cpu_to_wake_among(cpu::CPUS.all(), here)
    }

    /// [`Self::idle_cpu_to_wake`] among `cpus`.
    fn idle_cpu_to_wake_among(
        &mut self,
        cpus: &[&'static Cpu],
        here: &'static Cpu,
    ) -> Option<&'static Cpu> {
        for &cpu in cpus {
            let scheduler = self.of(cpu);
            if !ptr::eq(cpu, here) && scheduler.is_idle() && !scheduler.wakeup_sent {
                scheduler.wakeup_sent = true;
                return Some(cpu);
            }
        }
        None
    }

    /// Sets the time slice to `slice_ticks` (0 for none) on every CPU. Turning preemption on
    /// asks every other CPU to look, so those with ready threads arm their ticks.
    fn set_slice_ticks(&mut self, here: &'static Cpu, slice_ticks: u64) {
        self.slice_ticks = slice_ticks;
        for &cpu in cpu::CPUS.all() {
            self.of(cpu).slice_ticks = slice_ticks;
            if slice_ticks != 0 && !ptr::eq(cpu, here) {
                cpu.request_reschedule();
            }
        }
        // Also before the CPU table exists.
        self.of(here).slice_ticks = slice_ticks;
    }
}

/// Makes `thread`, woken or new, ready on its CPU. Returns that CPU if it needs the wake-up
/// interrupt, to send once the scheduler's lock is released.
#[inline]
fn make_ready<P: Platform>(schedulers: &mut Schedulers, thread: ThreadBox) -> Option<&'static Cpu> {
    let target = thread.get().cpu;
    match schedulers.of(target).push_ready(thread) {
        Notice::Nothing => None,
        Notice::Reschedule => schedulers.of(target).request_reschedule::<P>(target),
        // This CPU's own idle thread looks at its ready threads once it runs on.
        Notice::Wake => (!ptr::eq(target, cpu::current::<P>())).then_some(target),
    }
}

/// One CPU's scheduler state.
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
    /// The time slice in clock ticks while preemption is on ([`enable_preemption`]); 0 while
    /// it is off. A copy of [`Schedulers::slice_ticks`].
    slice_ticks: u64,
    /// While preemption is on: the clock tick at which the running thread's slice ends.
    slice_end: u64,
    /// The timer deadline the scheduler armed and has not seen fire: `u64::MAX` if none.
    /// Arming again for a later deadline is skipped (the earlier one fires first, and the
    /// scheduler then arms what it needs), so a switch rarely pays for arming the timer. Code
    /// that arms the timer itself (self-tests, benchmarks) does so only while no thread sleeps
    /// and preemption is off, when this is `u64::MAX` and the scheduler needs no deadline.
    armed: u64,
    /// Set when another CPU sends this one the wake-up interrupt for a thread it made ready
    /// here, cleared by the idle thread when it next looks at the ready threads: until then,
    /// further threads made ready here need no interrupt of their own.
    wakeup_sent: bool,
    /// Whose FP/SIMD registers the CPU holds (D54).
    fp: CpuFp,
}

/// What a CPU's FP/SIMD registers are cleared to before a program that has none runs
/// ([`CpuFp::clear_and_turn_off`]).
static INITIAL_FP_STATE: FpState = FpState::new();

/// Whose FP/SIMD registers a CPU holds, and whether user mode may use them (D54).
#[derive(Debug)]
struct CpuFp {
    /// The thread whose registers were last loaded or saved here: they are still in the CPU's
    /// registers. Thread ids are never reused, so one that exited never matches again. None
    /// when they hold no program's values, but [`FpState::new`]'s or the firmware's.
    owner: Option<ThreadId>,
    /// Whether they are on for user mode ([`Platform::set_user_fp`]): while a thread that has
    /// FP registers runs here, and from then until a program without any is about to run.
    user_on: bool,
    /// Whether the running thread has FP registers, so they are this CPU's ([`Self::owner`]
    /// is it) and on: a switch away saves them. With a user thread to switch to, the one thing
    /// a switch looks at before [`Self::switch`] (kernel threads switch without it).
    running_has_fp: bool,
}

impl CpuFp {
    const fn new() -> Self {
        Self {
            owner: None,
            user_on: false,
            running_has_fp: false,
        }
    }

    /// On a switch from `previous` to `next` on `here`, either of which has FP registers or
    /// runs a program: saves `previous`'s if it has any (they are this CPU's), then loads
    /// `next`'s unless they are still here, or clears them and turns them off if `next` runs
    /// a program that has none.
    fn switch<P: Platform>(&mut self, previous: &Thread, next: &Thread, here: &'static Cpu) {
        if self.running_has_fp
            && let Some(registers) = previous.fp.get()
        {
            debug_assert_eq!(self.owner, Some(previous.id));
            // SAFETY: interrupts are masked, as the kernel runs. The registers are on: they
            // were turned on when `previous`, which has FP registers, was switched to (or gave
            // itself some, `start_fp`), and stay on while it runs. `state` is allocated and
            // only the scheduler, holding its lock, and `previous` follow it, which no longer
            // runs. The registers were last saved or loaded with `state`, as `owner` says.
            unsafe { P::save_fp(registers.state.as_ptr()) };
        }
        self.running_has_fp = false;
        match next.fp.get() {
            Some(registers) => self.load::<P>(next.id, registers, here),
            None if next.user && self.user_on => self.clear_and_turn_off::<P>(),
            None => {}
        }
    }

    /// Before a program that has no FP/SIMD registers runs, with them on: loads
    /// [`INITIAL_FP_STATE`] over whatever program's they hold, then turns them off. Off alone
    /// is not enough: some CPUs read registers that are off speculatively, and the program
    /// could read another's that way (LazyFP, CVE-2018-3665). Out of line, as it runs only
    /// when a program that has never used FP/SIMD follows one that has.
    #[cold]
    #[inline(never)]
    fn clear_and_turn_off<P: Platform>(&mut self) {
        if self.owner.take().is_some() {
            // SAFETY: interrupts are masked and the registers on (`user_on`). The initial state
            // is a static, valid for a read, holding `FpState::new`'s values. The previous
            // owner's registers were saved when it was switched away from.
            unsafe { P::restore_fp(&INITIAL_FP_STATE) };
        }
        // SAFETY: interrupts are masked; turning them off only makes FP/SIMD trap, and they
        // hold no program's values.
        unsafe { P::set_user_fp(false) };
        self.user_on = false;
    }

    /// Turns the registers on for thread `id`, running next on `here`, and loads its own
    /// unless the CPU still holds them: it was the last owner here and has not loaded them on
    /// another CPU since.
    fn load<P: Platform>(&mut self, id: ThreadId, registers: &FpRegisters, here: &'static Cpu) {
        self.running_has_fp = true;
        if !self.user_on {
            // SAFETY: interrupts are masked; the thread's own registers are loaded below, or
            // are still the CPU's, before it runs.
            unsafe { P::set_user_fp(true) };
            self.user_on = true;
        }
        if self.owner != Some(id) || !ptr::eq(registers.cpu.get(), here) {
            // SAFETY: interrupts are masked and the registers on. `state` is allocated and
            // holds what `FpState::new` or a save wrote; only the scheduler, holding its lock,
            // and the thread follow it.
            unsafe { P::restore_fp(registers.state.as_ptr()) };
            self.owner = Some(id);
            registers.cpu.set(here);
        }
    }
}

impl Scheduler {
    const fn new() -> Self {
        Self {
            current: None,
            ready: ThreadList::new(),
            sleeping: ThreadList::new(),
            idle: None,
            exited: None,
            slice_ticks: 0,
            slice_end: 0,
            armed: u64::MAX,
            wakeup_sent: false,
            fp: CpuFp::new(),
        }
    }

    /// Makes the code running now, on `here`, the boot thread and creates the idle thread,
    /// unless the scheduler has started already. (A secondary CPU's has: [`start_secondary`].)
    fn start<P: Platform>(&mut self, here: &'static Cpu) -> Result<(), SpawnError> {
        if self.current.is_some() {
            return Ok(());
        }
        let stack = Stack::allocate().ok_or(SpawnError::OutOfMemory)?;
        // SAFETY: the stack's frames are allocated, writable through the direct map and used
        // by nothing else; the idle thread never exits, so they stay allocated.
        let context = unsafe { P::prepare_thread(stack.top(), idle_thread::<P>, 0) };
        let boot = Thread::boxed(ThreadId::BOOT, 0, None, here, false);
        self.idle = Some(Thread::boxed(
            ThreadId::next(),
            context,
            Some(stack),
            here,
            false,
        ));
        self.current = Some(boot);
        Ok(())
    }

    fn has_started(&self) -> bool {
        self.current.is_some()
    }

    /// Whether the CPU has started and has nothing to run: its idle thread runs (it is not in
    /// its slot) and no thread is ready.
    fn is_idle(&self) -> bool {
        self.has_started() && self.idle.is_none() && self.ready.is_empty()
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

    /// Adds a woken or new thread to the ready threads, from this CPU or another; returns what
    /// the CPU must be told. The scheduler does not arm the timer here, which only this CPU's
    /// own scheduler can do: a running thread that was alone with preemption on gets a
    /// reschedule request instead, and its next preemption point arms the tick, which is where
    /// an armed tick would have ended its slice anyway.
    #[inline]
    fn push_ready(&mut self, thread: ThreadBox) -> Notice {
        let was_alone = self.ready.is_empty();
        self.ready.push_back(thread);
        if self.idle.is_none() {
            if self.wakeup_sent {
                return Notice::Nothing;
            }
            self.wakeup_sent = true;
            Notice::Wake
        } else if was_alone && self.slice_ticks != 0 {
            Notice::Reschedule
        } else {
            Notice::Nothing
        }
    }

    /// Asks `target`, this scheduler's CPU, to look at its ready threads ([`Notice::Reschedule`]);
    /// returns `target` if it also needs the wake-up interrupt, because the thread it runs is in
    /// user mode, where it would see the request only at its next system call or interrupt, if
    /// ever (D48). Not if `target` is this CPU: its user thread runs kernel code now and passes
    /// a preemption point before it runs its program again. Out of line, as it is reached only
    /// with preemption on: inlined, it made the wake paths dearer even when it was not.
    #[cold]
    #[inline(never)]
    fn request_reschedule<P: Platform>(&self, target: &'static Cpu) -> Option<&'static Cpu> {
        target.request_reschedule();
        let running_user = self
            .current
            .as_ref()
            .is_some_and(|thread| thread.get().user);
        (running_user && !ptr::eq(target, cpu::current::<P>())).then_some(target)
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

    /// [`timer_events`]' slow path, for this CPU's own scheduler.
    #[inline]
    fn timer_fired<P: Platform>(&mut self) -> bool {
        // The timer fires once per arming, so whatever was armed has fired, unless only a
        // reschedule request came.
        if cpu::current::<P>().see_scheduler_events() != 0 {
            self.armed = u64::MAX;
        }
        let now = P::clock_ticks();
        while self
            .sleeping
            .front()
            .is_some_and(|thread| thread.wake_at <= now)
        {
            let thread = self.sleeping.pop_front().expect("a sleeper is due");
            self.ready.push_back(thread);
        }
        // Whatever happens next, the timer is armed again: the sleepers and the tick need it.
        let slice_over = self.tick_needed() && now >= self.slice_end;
        if slice_over {
            self.start_slice_now::<P>();
        } else {
            self.update_timer::<P>();
        }
        slice_over
    }
}

/// If the timer has fired, or another CPU asked, since this CPU's scheduler last looked: wakes
/// the sleepers whose time has come and arms the timer for what is needed next; a slice that
/// ended with threads ready wakes an idle CPU to take one (D44). Returns whether the running
/// thread's slice is over; the next slice has then started, for the thread the caller must
/// switch to (another is ready, or no slice would have ended). A load and a branch
/// when neither happened. The slow path finds its CPU itself, so no caller keeps one in a
/// register across the call.
#[inline]
fn timer_events<P: Platform>(schedulers: &mut Schedulers) -> bool {
    if cpu::local_unseen_by_scheduler::<P>() == 0 {
        return false;
    }
    timer_fired::<P>(schedulers)
}

/// [`timer_events`]' slow path. A slice that ends with threads ready wakes an idle CPU, if
/// there is one, to take one of them (D44).
#[cold]
#[inline(never)]
fn timer_fired<P: Platform>(schedulers: &mut Schedulers) -> bool {
    let slice_over = schedulers.of(cpu::current::<P>()).timer_fired::<P>();
    if slice_over {
        wake_idle_cpu::<P>(schedulers);
    }
    slice_over
}

/// Sends the wake-up interrupt to an idle CPU ([`Schedulers::idle_cpu_to_wake`]) with the
/// scheduler's lock held, so that it takes a ready thread once the lock is released.
#[cold]
#[inline(never)]
fn wake_idle_cpu<P: Platform>(schedulers: &mut Schedulers) {
    if let Some(cpu) = schedulers.idle_cpu_to_wake(cpu::current::<P>()) {
        cpu::wake::<P>(cpu);
    }
}

/// Starts the scheduler on this CPU if it has not started ([`Scheduler::start`]), finding the
/// CPU itself, as [`timer_fired`] does.
fn start_here<P: Platform>(schedulers: &mut Schedulers) -> Result<(), SpawnError> {
    let here = cpu::current::<P>();
    schedulers.of(here).start::<P>(here)
}

/// [`start_here`] for the blocking calls, which cannot fail: without the memory for the idle
/// thread's stack, nothing more can run anyway.
fn start_here_or_panic<P: Platform>(schedulers: &mut Schedulers) {
    if start_here::<P>(schedulers).is_err() {
        panic!("no memory for the idle thread's stack");
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

    /// Waits, running other threads, until this one has exited and its stack is freed. Call it
    /// as [`yield_now`] is called: with interrupts masked and no lock held.
    pub fn join<P: Platform>(self) {
        debug_assert!(
            SCHEDULER
                .lock()
                .of(cpu::current::<P>())
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
    start_here::<P>(&mut SCHEDULER.lock())
}

/// Starts this secondary CPU's scheduler (D43): the code running now, on the stack the CPU
/// started on, becomes its idle thread, and threads may be placed on it from then on. Then call
/// [`run_idle`]. Its idle thread has no stack canary: the start stack has none.
pub(crate) fn start_secondary<P: Platform>() {
    let here = cpu::current::<P>();
    let mut schedulers = SCHEDULER.lock();
    let slice_ticks = schedulers.slice_ticks;
    let scheduler = schedulers.of(here);
    debug_assert!(
        !scheduler.has_started(),
        "a CPU started its scheduler twice"
    );
    scheduler.slice_ticks = slice_ticks;
    scheduler.current = Some(Thread::boxed(ThreadId::next(), 0, None, here, false));
}

/// Runs this CPU's idle thread, which [`start_secondary`] made of the code running now.
pub(crate) fn run_idle<P: Platform>() -> ! {
    idle_loop::<P>()
}

/// Starts a thread that runs `function` on a stack of its own, after the threads already
/// ready to run on the CPU it is placed on (`Schedulers::place`): an idle one if there is
/// one. The caller keeps running.
pub fn spawn<P: Platform, F: FnOnce() + Send + 'static>(
    function: F,
) -> Result<JoinHandle, SpawnError> {
    spawn_thread::<P, F>(function, false)
}

/// [`spawn`] for a thread that runs a program in user mode ([`crate::process::spawn`]): a CPU
/// running it gets the wake-up interrupt with a reschedule request (D48).
pub(crate) fn spawn_user<P: Platform, F: FnOnce() + Send + 'static>(
    function: F,
) -> Result<JoinHandle, SpawnError> {
    spawn_thread::<P, F>(function, true)
}

fn spawn_thread<P: Platform, F: FnOnce() + Send + 'static>(
    function: F,
    user: bool,
) -> Result<JoinHandle, SpawnError> {
    let stack = Stack::allocate().ok_or(SpawnError::OutOfMemory)?;
    let function: Box<dyn FnOnce() + Send> = Box::new(function);
    // A thin pointer, so it fits the one-word argument.
    let start = Box::into_raw(Box::new(function));
    // SAFETY: the stack's frames are allocated, writable through the direct map and used by
    // nothing else; they stay allocated until the thread has exited (`finish_switch`).
    let context = unsafe { P::prepare_thread(stack.top(), thread_start::<P>, start as usize) };
    let mut schedulers = SCHEDULER.lock();
    if let Err(error) = start_here::<P>(&mut schedulers) {
        drop(schedulers);
        // SAFETY: from `Box::into_raw` above; the thread that would have owned it never
        // existed. (The stack is freed on return.)
        drop(unsafe { Box::from_raw(start) });
        return Err(error);
    }
    let cpu = schedulers.place(cpu::current::<P>());
    let thread = Thread::boxed(ThreadId::next(), context, Some(stack), cpu, user);
    let handle = JoinHandle {
        id: thread.get().id,
        joined: thread.get().joined.clone(),
    };
    let wake = make_ready::<P>(&mut schedulers, thread);
    drop(schedulers);
    if let Some(cpu) = wake {
        cpu::wake::<P>(cpu);
    }
    Ok(handle)
}

/// What [`start_fp`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FpStart {
    /// The thread had no FP/SIMD registers and now has them, as a new program's, on.
    Started,
    /// It had them already, so the trap was not about them.
    AlreadyHad,
    /// It is a kernel thread (running a program directly, as self-tests do), which never has
    /// FP/SIMD registers.
    KernelThread,
    /// There was no memory for them.
    OutOfMemory,
}

/// Gives the running thread, a user thread whose program just trapped on an illegal
/// instruction (or returned from a signal handler with FP/SIMD values in its frame, D82),
/// FP/SIMD registers if it has none yet (D54): a new program's values ([`FpState::new`]),
/// loaded and on for user mode on this CPU, so the instruction can run again. Call it with
/// interrupts masked and no lock held. Once per thread at most; a thread that never uses them
/// costs nothing.
#[cold]
#[inline(never)]
pub(crate) fn start_fp<P: Platform>() -> FpStart {
    let layout = Layout::new::<FpState>();
    // SAFETY: `FpState` is not zero-sized.
    let Some(state) = NonNull::new(unsafe { alloc::alloc::alloc(layout) }.cast::<FpState>()) else {
        return FpStart::OutOfMemory;
    };
    // SAFETY: just allocated with `FpState`'s layout, so valid for a write and aligned.
    unsafe { state.as_ptr().write(FpState::new()) };
    let registers = FpRegisters {
        state,
        cpu: Cell::new(ptr::null()),
    };
    let here = cpu::current::<P>();
    let mut schedulers = SCHEDULER.lock();
    let scheduler = schedulers.of(here);
    let running = scheduler
        .current
        .as_ref()
        .expect("a user thread runs")
        .get();
    let refused = if !running.user {
        Some(FpStart::KernelThread)
    } else if running.fp.get().is_some() {
        Some(FpStart::AlreadyHad)
    } else {
        None
    };
    if let Some(refused) = refused {
        drop(schedulers);
        drop(registers);
        return refused;
    }
    let registers = running.fp.get_or_init(|| registers);
    scheduler.fp.load::<P>(running.id, registers, here);
    FpStart::Started
}

/// Calls `f` with the running thread's FP/SIMD registers, as its program left them, if it has
/// any ([`start_fp`]); the thread then runs with what `f` left in them. `None`, without calling
/// `f`, if it has none. Call it with interrupts masked and no lock held. For signal frames
/// (D82): not on a hot path, it saves and loads the registers every time.
#[cold]
#[inline(never)]
pub(crate) fn with_running_fp<P: Platform, R>(f: impl FnOnce(&mut FpState) -> R) -> Option<R> {
    let here = cpu::current::<P>();
    let mut schedulers = SCHEDULER.lock();
    let scheduler = schedulers.of(here);
    if !scheduler.fp.running_has_fp {
        return None;
    }
    let running = scheduler.current.as_ref()?.get();
    let registers = running.fp.get()?;
    debug_assert_eq!(scheduler.fp.owner, Some(running.id));
    let state = registers.state.as_ptr();
    // SAFETY: interrupts are masked and the registers on: the running thread has FP
    // registers, which are this CPU's (`running_has_fp`). `state` is allocated, and only the
    // scheduler, whose lock is held, and the running thread, which is here, follow it.
    unsafe { P::save_fp(state) };
    // SAFETY: as above; nothing else uses `*state` until the lock is dropped.
    let result = f(unsafe { &mut *state });
    // SAFETY: as above. `*state` holds what the save wrote, changed only through `FpState`'s
    // own methods, which keep it loadable.
    unsafe { P::restore_fp(state) };
    Some(result)
}

/// Lets the next ready thread run, if there is one; returns when this thread's turn comes
/// again.
pub fn yield_now<P: Platform>() {
    let mut schedulers = SCHEDULER.lock();
    // Sleepers that are due run before this thread's next turn.
    timer_events::<P>(&mut schedulers);
    let here = cpu::current::<P>();
    let Some(next) = schedulers.of(here).ready.pop_front() else {
        return;
    };
    switch_to::<P>(schedulers, here, next, |schedulers, here, previous| {
        schedulers.of(here).ready.push_back(previous);
    });
}

/// Sleeps until [`Platform::clock_ticks`] reaches `deadline`, running other threads
/// meanwhile; returns at once if it has. The thread wakes at its next chance after the
/// deadline, never before: when the CPU is idle, at the timer interrupt; otherwise at the
/// running thread's next preemption point, yield or block. Call it as [`yield_now`] is
/// called: with interrupts masked and no lock held. Starts the scheduler if it has not started
/// (see [`WaitQueue::wait_until`]).
pub fn sleep_until<P: Platform>(deadline: u64) {
    let mut schedulers = SCHEDULER.lock();
    if P::clock_ticks() >= deadline {
        return;
    }
    start_here_or_panic::<P>(&mut schedulers);
    block::<P>(schedulers, |schedulers, here, thread| {
        let scheduler = schedulers.of(here);
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

/// Turns preemption on (D37), on every CPU: every [`TIME_SLICE_MS`], the running thread lets
/// the next ready thread run at its next [`preemption_point`]. A slice starts now, when a slice
/// ends, and when the idle thread lets a thread run; a thread that runs after another yields,
/// blocks or exits has the rest of the slice under way (D39). The timer ends it only while
/// another thread is ready to run on its CPU. The other CPUs start their slices at their next
/// preemption point, yield or block.
pub fn enable_preemption<P: Platform>() {
    let here = cpu::current::<P>();
    let mut schedulers = SCHEDULER.lock();
    schedulers.set_slice_ticks(here, (P::clock_frequency() * TIME_SLICE_MS / 1000).max(1));
    schedulers.of(here).start_slice_now::<P>();
}

/// Turns preemption off, on every CPU; threads run until they yield or block. Call it with
/// interrupts masked, as the kernel runs: this CPU's timer is armed again for its sleepers
/// alone, or for never, so no tick arrives afterwards. Another CPU may take one more tick,
/// which ends no slice.
pub fn disable_preemption<P: Platform>() {
    let here = cpu::current::<P>();
    let mut schedulers = SCHEDULER.lock();
    schedulers.set_slice_ticks(here, 0);
    let scheduler = schedulers.of(here);
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
    if cpu::local_unseen_by_scheduler::<P>() != 0 {
        preemption_point_timer_fired::<P>();
    }
}

/// [`preemption_point`] for a thread about to run a program in user mode (D48), where it passes
/// no other until a system call, an interrupt or an exception: repeated until the scheduler has
/// seen every timer interrupt and reschedule request on this CPU. A thread resumed by a switch
/// may find some it did not see itself (the thread before it exited, or switched to it from
/// a yield or block, without looking), and their timer has fired already: unseen, nothing would
/// arm it again or wake the sleepers while the program runs. As cheap as
/// [`preemption_point`] when there is nothing to see.
#[inline]
pub fn preemption_point_before_user_mode<P: Platform>() {
    loop {
        P::interrupt_window();
        if cpu::local_unseen_by_scheduler::<P>() == 0 {
            return;
        }
        preemption_point_timer_fired::<P>();
    }
}

/// [`preemption_point`]'s slow path, out of line so the check stays a few instructions.
#[cold]
#[inline(never)]
fn preemption_point_timer_fired<P: Platform>() {
    let mut schedulers = SCHEDULER.lock();
    if !timer_events::<P>(&mut schedulers) {
        return;
    }
    let here = cpu::current::<P>();
    let next = schedulers
        .of(here)
        .ready
        .pop_front()
        .expect("a slice ends only while another thread is ready");
    switch_to::<P>(schedulers, here, next, |schedulers, here, previous| {
        schedulers.of(here).ready.push_back(previous);
    });
}

/// Stops the running thread until something makes it ready again: `park` puts it where it
/// waits (a wait queue, the sleepers). The next ready thread on this CPU runs, or its idle
/// thread if none is. The scheduler must have started on this CPU.
fn block<P: Platform>(
    mut schedulers: SpinLockGuard<'_, Schedulers>,
    park: impl FnOnce(&mut Schedulers, &'static Cpu, ThreadBox),
) {
    timer_events::<P>(&mut schedulers);
    let here = cpu::current::<P>();
    let scheduler = schedulers.of(here);
    let next = match scheduler.ready.pop_front() {
        Some(next) => next,
        None => scheduler
            .idle
            .take()
            .expect("the scheduler started, so the idle thread is not running"),
    };
    switch_to::<P>(schedulers, here, next, park);
}

/// Switches from the running thread on `here`, this CPU, to `next`, after `park` has put the
/// running one where it goes (the ready threads, a wait queue, the sleepers, the exited slot,
/// or the idle slot for the idle thread), given `here` too. Returns when the running thread is
/// resumed.
fn switch_to<P: Platform>(
    mut schedulers: SpinLockGuard<'_, Schedulers>,
    here: &'static Cpu,
    next: ThreadBox,
    park: impl FnOnce(&mut Schedulers, &'static Cpu, ThreadBox),
) {
    let fp = &schedulers.of(here).fp;
    let next_thread = next.get();
    if fp.running_has_fp || next_thread.user && (fp.user_on || next_thread.fp.get().is_some()) {
        return switch_to_with_fp::<P>(schedulers, here, next, park);
    }
    switch_now::<P>(schedulers, here, next, park);
}

/// [`switch_to`] when the running thread or `next` has FP/SIMD registers or runs a program:
/// [`CpuFp::switch`] first.
#[inline(never)]
fn switch_to_with_fp<P: Platform>(
    mut schedulers: SpinLockGuard<'_, Schedulers>,
    here: &'static Cpu,
    next: ThreadBox,
    park: impl FnOnce(&mut Schedulers, &'static Cpu, ThreadBox),
) {
    let scheduler = schedulers.of(here);
    let running = scheduler
        .current
        .as_ref()
        .expect("a thread is ready, so one runs")
        .get();
    scheduler.fp.switch::<P>(running, next.get(), here);
    switch_now::<P>(schedulers, here, next, park);
}

/// The switch itself: see [`switch_to`].
#[inline(always)]
fn switch_now<P: Platform>(
    mut schedulers: SpinLockGuard<'_, Schedulers>,
    here: &'static Cpu,
    next: ThreadBox,
    park: impl FnOnce(&mut Schedulers, &'static Cpu, ThreadBox),
) {
    let resume = next.get().context;
    let previous = schedulers.of(here).replace_current(next);
    let save = previous.context();
    park(&mut schedulers, here, previous);
    // Held across the switch, for the thread it resumes to release.
    SpinLockGuard::leak(schedulers);
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
    let here = cpu::current::<P>();
    let mut schedulers = SCHEDULER.lock();
    let scheduler = schedulers.of(here);
    let next = match scheduler.ready.pop_front() {
        Some(next) => next,
        None => scheduler
            .idle
            .take()
            .expect("a spawned thread runs, so the idle thread does not"),
    };
    switch_to::<P>(schedulers, here, next, |schedulers, here, previous| {
        let scheduler = schedulers.of(here);
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
    let mut schedulers = unsafe { SCHEDULER.assume_locked() };
    // Checked before it is taken, so a switch from a thread that did not exit writes nothing;
    // with one load through the per-CPU register where the slot is one word (a null pointer
    // for none: `ThreadBox` is a transparent `NonNull`), as cheap as a global was.
    let none_exited = if EXITED_SLOT_IS_ONE_WORD {
        // SAFETY: the per-CPU register holds this CPU's block, where `EXITED_SLOT` is the
        // offset of its scheduler's `exited` slot: an aligned word in the state's `UnsafeCell`,
        // which nothing writes meanwhile, since this thread holds the scheduler's lock.
        unsafe { P::cpu_local_u64::<EXITED_SLOT>() == 0 }
    } else {
        schedulers.of(cpu::current::<P>()).exited.is_none()
    };
    if none_exited {
        return;
    }
    let exited = schedulers.of(cpu::current::<P>()).exited.take();
    drop(schedulers);
    if let Some(exited) = exited {
        free_exited::<P>(exited);
    }
}

/// Where a CPU's [`Scheduler::exited`] slot is in its per-CPU block.
const EXITED_SLOT: usize =
    core::mem::offset_of!(Cpu, scheduler) + core::mem::offset_of!(Scheduler, exited);
/// Whether the slot is one 8-byte word, nullable: on 64-bit architectures.
const EXITED_SLOT_IS_ONE_WORD: bool =
    size_of::<Option<ThreadBox>>() == 8 && EXITED_SLOT.is_multiple_of(8);

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

/// The boot CPU's idle thread, prepared by [`Scheduler::start`].
extern "C" fn idle_thread<P: Platform>(_: usize) -> ! {
    finish_switch::<P>();
    idle_loop::<P>()
}

/// A CPU's idle thread: runs only when no other thread on its CPU is ready, and lets the first
/// that becomes ready run, or else one that waits on a busy CPU (D44). Meanwhile it arms the
/// timer for the earliest sleeper alone (no tick) and waits for an interrupt.
fn idle_loop<P: Platform>() -> ! {
    let here = cpu::current::<P>();
    loop {
        // A process that ended while its address space stayed loaded here is freed now (D48).
        crate::address_space::release_unused::<P>(here);
        let mut schedulers = SCHEDULER.lock();
        // This look answers every wake-up sent so far: a thread made ready from now on needs
        // one of its own.
        schedulers.of(here).wakeup_sent = false;
        timer_events::<P>(&mut schedulers);
        let next = match schedulers.of(here).ready.pop_front() {
            Some(next) => Some(next),
            None => schedulers.steal(here),
        };
        if let Some(next) = next {
            switch_to::<P>(schedulers, here, next, |schedulers, here, idle| {
                let scheduler = schedulers.of(here);
                scheduler.idle = Some(idle);
                // A fresh slice: the one under way was the idle thread's.
                scheduler.start_slice::<P>();
            });
            continue;
        }
        let scheduler = schedulers.of(here);
        // Exactly what is needed, so the CPU is not woken early for a stale deadline.
        let deadline = scheduler.timer_needed();
        if deadline != scheduler.armed {
            scheduler.arm::<P>(deadline);
        }
        drop(schedulers);
        // A thread becomes ready on this CPU, or one to take waits on a busy CPU, only through
        // an interrupt: the timer's, or the wake-up interrupt from the CPU that made it ready
        // here or whose slice ended with threads ready. One already pending is taken at once.
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
    use core::sync::atomic::AtomicU64;

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
        assert!(SCHEDULER.lock().of(cpu::current::<P>()).exited.is_none());
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
        crate::interrupt::handle::<P>(eggos_hal::Interrupt::Timer);
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
        assert!(SCHEDULER.lock().of(cpu::current::<P>()).sleeping.is_empty());
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
        // The new thread asks for the tick, which the next preemption point arms (D43).
        assert!(cpu::current::<P>().reschedule_requested());
        preemption_point::<P>();
        let deadline = crate::tests::timer_deadline();
        assert_ne!(deadline, u64::MAX, "a ready thread brings the tick back");
        assert!(deadline <= P::clock_ticks() + 10_000, "within one slice");
        other.join::<P>();
        disable_preemption::<P>();
        assert_eq!(crate::tests::timer_deadline(), u64::MAX);
    }

    #[test]
    fn the_timer_stays_armed_when_a_slice_ends_in_a_yield() {
        let _timer = crate::interrupt::lock_timer_interrupts_for_test();
        let _frames = host::frames_for_test();
        static ARMED_WHILE_OTHER_READY: AtomicU64 = AtomicU64::new(0);
        start::<P>().unwrap();
        disable_preemption::<P>();
        enable_preemption::<P>();
        let other = spawn::<P, _>(|| {
            // The boot thread is ready: the tick must be armed.
            ARMED_WHILE_OTHER_READY.store(crate::tests::timer_deadline(), Ordering::Relaxed);
        })
        .unwrap();
        // The slice ends (the test platform jumps its clock to the deadline) and the boot
        // thread notices in `yield_now`, not at a preemption point.
        P::wait_for_interrupt();
        yield_now::<P>();
        other.join::<P>();
        disable_preemption::<P>();
        assert_ne!(ARMED_WHILE_OTHER_READY.load(Ordering::Relaxed), u64::MAX);
    }

    #[test]
    fn a_thread_the_idle_thread_lets_run_gets_a_fresh_slice() {
        let _timer = crate::interrupt::lock_timer_interrupts_for_test();
        let _frames = host::frames_for_test();
        static SLICE_LEFT: AtomicU64 = AtomicU64::new(0);
        disable_preemption::<P>();
        enable_preemption::<P>();
        let sleeper = spawn::<P, _>(|| {
            // Longer than a slice; the boot thread blocks in `join`, so the idle thread runs.
            sleep_until::<P>(P::clock_ticks() + 50_000);
            // Another thread ready brings the tick back, at the next preemption point: one
            // slice from now, not the old one.
            let other = spawn::<P, _>(|| {}).unwrap();
            preemption_point::<P>();
            SLICE_LEFT.store(
                crate::tests::timer_deadline().saturating_sub(P::clock_ticks()),
                Ordering::Relaxed,
            );
            other.join::<P>();
        })
        .unwrap();
        sleeper.join::<P>();
        disable_preemption::<P>();
        let left = SLICE_LEFT.load(Ordering::Relaxed);
        assert!(
            (5_000..=10_000).contains(&left),
            "{left} ticks of the slice left"
        );
    }

    /// A thread for the scheduler-state tests, which never runs.
    fn idle_thread_for_test(cpu: &'static Cpu) -> ThreadBox {
        Thread::boxed(ThreadId::next(), 0, None, cpu, false)
    }

    /// Takes every thread out of `scheduler`, so that it can be dropped.
    fn empty(scheduler: &mut Scheduler) {
        while scheduler.ready.pop_front().is_some() {}
        scheduler.current = None;
        scheduler.idle = None;
    }

    #[test]
    fn a_thread_made_ready_on_an_idle_cpu_wakes_it_once_per_look() {
        let cpu = Cpu::for_test(1);
        let mut scheduler = Scheduler::new();
        // Started, and its idle thread runs: it is not in its slot.
        scheduler.current = Some(idle_thread_for_test(cpu));
        assert!(scheduler.is_idle());
        assert_eq!(
            scheduler.push_ready(idle_thread_for_test(cpu)),
            Notice::Wake
        );
        assert!(!scheduler.is_idle());
        // The wake-up sent covers the next thread too, until the idle thread looks.
        assert_eq!(
            scheduler.push_ready(idle_thread_for_test(cpu)),
            Notice::Nothing
        );
        scheduler.wakeup_sent = false;
        assert_eq!(
            scheduler.push_ready(idle_thread_for_test(cpu)),
            Notice::Wake
        );
        empty(&mut scheduler);
    }

    #[test]
    fn a_thread_made_ready_on_a_busy_cpu_asks_for_a_tick_only_if_one_is_needed() {
        let cpu = Cpu::for_test(1);
        let mut scheduler = Scheduler::new();
        scheduler.current = Some(idle_thread_for_test(cpu));
        scheduler.idle = Some(idle_thread_for_test(cpu));
        // Without preemption, it runs when the running thread yields or blocks.
        assert_eq!(
            scheduler.push_ready(idle_thread_for_test(cpu)),
            Notice::Nothing
        );
        empty(&mut scheduler);
        scheduler.current = Some(idle_thread_for_test(cpu));
        scheduler.idle = Some(idle_thread_for_test(cpu));
        scheduler.slice_ticks = 10;
        // The running thread was alone: no tick is armed.
        assert_eq!(
            scheduler.push_ready(idle_thread_for_test(cpu)),
            Notice::Reschedule
        );
        // Another was ready already, so the tick is armed.
        assert_eq!(
            scheduler.push_ready(idle_thread_for_test(cpu)),
            Notice::Nothing
        );
        empty(&mut scheduler);
    }

    #[test]
    fn a_reschedule_request_to_a_cpu_running_a_user_thread_needs_the_wake_up_interrupt() {
        let (here, other) = (cpu::current::<P>(), Cpu::for_test(2));
        let mut scheduler = Scheduler::new();
        scheduler.current = Some(idle_thread_for_test(other));
        // A kernel thread passes a preemption point soon: the request alone does.
        assert!(scheduler.request_reschedule::<P>(other).is_none());
        empty(&mut scheduler);
        scheduler.current = Some(Thread::boxed(ThreadId::next(), 0, None, other, true));
        // In user mode it may never pass one: the interrupt takes the CPU back.
        assert!(
            scheduler
                .request_reschedule::<P>(other)
                .is_some_and(|cpu| ptr::eq(cpu, other))
        );
        // On this CPU, the user thread runs kernel code now, and passes one before user mode.
        assert!(scheduler.request_reschedule::<P>(here).is_none());
        empty(&mut scheduler);
    }

    #[test]
    fn make_ready_on_another_cpu_hands_back_an_idle_one_and_flags_a_busy_one() {
        let (idle, busy) = (Cpu::for_test(1), Cpu::for_test(2));
        let mut schedulers = Schedulers {
            slice_ticks: 10,
            next_placement: 0,
        };
        schedulers.of(idle).current = Some(idle_thread_for_test(idle));
        let busy_state = schedulers.of(busy);
        busy_state.current = Some(idle_thread_for_test(busy));
        busy_state.idle = Some(idle_thread_for_test(busy));
        busy_state.slice_ticks = 10;
        let woken = make_ready::<P>(&mut schedulers, idle_thread_for_test(idle));
        assert!(woken.is_some_and(|cpu| ptr::eq(cpu, idle)));
        assert!(!idle.reschedule_requested());
        assert!(make_ready::<P>(&mut schedulers, idle_thread_for_test(busy)).is_none());
        assert!(busy.reschedule_requested());
        assert_eq!(busy.see_scheduler_events(), 0);
        empty(schedulers.of(idle));
        empty(schedulers.of(busy));
    }

    #[test]
    fn new_threads_go_to_idle_cpus_first_then_to_each_started_cpu_in_turn() {
        let cpus: Vec<&'static Cpu> = (0..4).map(Cpu::for_test).collect();
        let cpus: &'static [&'static Cpu] = cpus.leak();
        let here = cpus[0];
        let mut schedulers = Schedulers {
            slice_ticks: 0,
            next_placement: 0,
        };
        // Nothing started but this CPU: nowhere else to go.
        assert!(ptr::eq(schedulers.place_among(cpus, here), here));
        assert!(ptr::eq(schedulers.place_among(&cpus[..1], here), here));
        // CPUs 0, 1 and 3 run threads; CPU 2 has not started.
        for &cpu in [cpus[0], cpus[1], cpus[3]].iter() {
            let state = schedulers.of(cpu);
            state.current = Some(idle_thread_for_test(cpu));
            state.idle = Some(idle_thread_for_test(cpu));
        }
        let placed: Vec<usize> = (0..4)
            .map(|_| schedulers.place_among(cpus, here).index())
            .collect();
        assert_eq!(placed, [0, 1, 3, 0]);
        // CPU 3 goes idle: its idle thread runs.
        let idle = schedulers.of(cpus[3]).idle.take();
        assert_eq!(schedulers.place_among(cpus, here).index(), 3);
        schedulers.of(cpus[3]).idle = idle;
        for &cpu in cpus {
            empty(schedulers.of(cpu));
        }
    }

    #[test]
    fn an_idle_cpu_takes_a_ready_thread_from_the_next_busy_cpu_but_never_the_boot_thread() {
        let cpus: Vec<&'static Cpu> = (0..4).map(Cpu::for_test).collect();
        let cpus: &'static [&'static Cpu] = cpus.leak();
        let mut schedulers = Schedulers {
            slice_ticks: 0,
            next_placement: 0,
        };
        // CPU 1 looks; its idle thread runs and nothing is ready there.
        schedulers.of(cpus[1]).current = Some(idle_thread_for_test(cpus[1]));
        assert!(schedulers.steal_among(cpus, cpus[1]).is_none());
        // CPU 2's idle thread runs too, with a thread just made ready there: it runs it itself.
        let woken = schedulers.of(cpus[2]);
        woken.current = Some(idle_thread_for_test(cpus[2]));
        woken.ready.push_back(idle_thread_for_test(cpus[2]));
        // CPUs 3 and 0 are busy, with threads ready: the boot thread first on CPU 3.
        for &cpu in [cpus[3], cpus[0]].iter() {
            let busy = schedulers.of(cpu);
            busy.current = Some(idle_thread_for_test(cpu));
            busy.idle = Some(idle_thread_for_test(cpu));
        }
        let boot = Thread::boxed(ThreadId::BOOT, 0, None, cpus[3], false);
        schedulers.of(cpus[3]).ready.push_back(boot);
        let waiting = idle_thread_for_test(cpus[0]);
        let id = waiting.get().id;
        schedulers.of(cpus[0]).ready.push_back(waiting);
        // From CPU 1 on, CPU 2 is left alone and CPU 3's boot thread stays: CPU 0's is taken.
        let taken = schedulers
            .steal_among(cpus, cpus[1])
            .expect("CPU 0 has one ready");
        assert_eq!(taken.get().id, id);
        assert!(ptr::eq(taken.get().cpu, cpus[1]));
        assert!(schedulers.of(cpus[0]).ready.is_empty());
        assert!(schedulers.steal_among(cpus, cpus[1]).is_none());
        // A CPU never takes from itself.
        assert!(schedulers.steal_among(cpus, cpus[3]).is_none());
        drop(taken);
        for &cpu in cpus {
            empty(schedulers.of(cpu));
        }
    }

    #[test]
    fn a_slice_that_ends_with_threads_ready_wakes_one_idle_cpu_per_look() {
        let cpus: Vec<&'static Cpu> = (0..4).map(Cpu::for_test).collect();
        let cpus: &'static [&'static Cpu] = cpus.leak();
        let mut schedulers = Schedulers {
            slice_ticks: 10,
            next_placement: 0,
        };
        // CPUs 0 and 1 are busy; CPU 2 has not started; CPU 3's idle thread runs.
        for &cpu in [cpus[0], cpus[1]].iter() {
            let busy = schedulers.of(cpu);
            busy.current = Some(idle_thread_for_test(cpu));
            busy.idle = Some(idle_thread_for_test(cpu));
        }
        schedulers.of(cpus[3]).current = Some(idle_thread_for_test(cpus[3]));
        let woken = schedulers.idle_cpu_to_wake_among(cpus, cpus[0]);
        assert!(woken.is_some_and(|cpu| ptr::eq(cpu, cpus[3])));
        // Until it looks, the wake-up already sent covers it.
        assert!(schedulers.idle_cpu_to_wake_among(cpus, cpus[1]).is_none());
        schedulers.of(cpus[3]).wakeup_sent = false;
        // A CPU never wakes itself.
        assert!(schedulers.idle_cpu_to_wake_among(cpus, cpus[3]).is_none());
        assert!(schedulers.idle_cpu_to_wake_among(cpus, cpus[1]).is_some());
        for &cpu in cpus {
            empty(schedulers.of(cpu));
        }
    }

    #[test]
    fn a_program_without_fp_registers_runs_after_they_are_cleared() {
        use crate::tests::{FpCall, fp_calls};
        let _frames = host::frames_for_test();
        let here = cpu::current::<P>();
        let thread = |id, user, fp| Thread {
            id: ThreadId(id),
            context: 0,
            stack: None,
            joined: Arc::new(Joined {
                finished: AtomicBool::new(false),
                joiners: WaitQueue::new(),
            }),
            next: None,
            wake_at: 0,
            cpu: here,
            user,
            fp,
        };
        let layout = Layout::new::<FpState>();
        // SAFETY: `FpState` is not zero-sized.
        let state = NonNull::new(unsafe { alloc::alloc::alloc(layout) }.cast::<FpState>()).unwrap();
        // SAFETY: just allocated with `FpState`'s layout.
        unsafe { state.as_ptr().write(FpState::new()) };
        let with_fp = thread(
            u64::MAX - 1,
            true,
            OnceCell::from(FpRegisters {
                state,
                cpu: Cell::new(ptr::null()),
            }),
        );
        let without_fp = thread(u64::MAX - 2, true, OnceCell::new());
        let kernel = thread(u64::MAX - 3, false, OnceCell::new());
        let mut fp = CpuFp::new();
        fp_calls();
        fp.switch::<P>(&without_fp, &with_fp, here);
        fp.switch::<P>(&with_fp, &without_fp, here);
        let state = state.as_ptr() as usize;
        let initial = &raw const INITIAL_FP_STATE as usize;
        assert_eq!(
            fp_calls(),
            [
                FpCall::SetUser(true),
                FpCall::Restore(state),
                FpCall::Save(state),
                FpCall::Restore(initial),
                FpCall::SetUser(false),
            ]
        );
        assert_eq!(fp.owner, None);
        fp.switch::<P>(&without_fp, &with_fp, here);
        assert_eq!(fp_calls(), [FpCall::SetUser(true), FpCall::Restore(state)]);
        fp.switch::<P>(&with_fp, &kernel, here);
        assert_eq!(fp_calls(), [FpCall::Save(state)]);
        fp.switch::<P>(&kernel, &without_fp, here);
        assert_eq!(
            fp_calls(),
            [FpCall::Restore(initial), FpCall::SetUser(false)]
        );
    }

    #[test]
    #[should_panic(expected = "overflowed its stack")]
    fn a_clobbered_canary_stops_the_kernel_when_checked() {
        // No scheduler and no frame allocator: the thread never runs, and its stack is leaked
        // host memory, which must not reach the frame allocator, so the thread is never dropped.
        let memory = Box::leak(Box::new([0u64; STACK_SIZE / 8]));
        let stack = Stack {
            base: NonZeroUsize::new(memory.as_mut_ptr() as usize).unwrap(),
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
            cpu: cpu::current::<P>(),
            user: false,
            fp: OnceCell::new(),
        });
        thread.check_stack();
        memory[0] = 0;
        thread.check_stack();
    }
}
