//! The machine's CPUs (D40, D41). The kernel numbers them, 0 for the CPU that booted and then in
//! the firmware's order, gives each a per-CPU block ([`Cpu`]) that the CPU reaches through its
//! per-CPU register ([`current`]), and starts the others ([`start_secondaries`]). Every CPU
//! takes its own interrupts: its timer's, and the wake-up interrupt another CPU sends it with
//! [`wake`], each counted in its block. Each block also holds the CPU's part of the scheduler
//! (D43): a secondary CPU checks its timer, starts its scheduler, reports in and runs its idle
//! thread, which runs the threads placed on it.

use crate::heap::{FrameSource, GlobalFrames};
use crate::thread::{self, CpuScheduler, STACK_ORDER, STACK_SIZE};
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, AtomicUsize, Ordering};
use eggos_hal::cpu::{
    CpuStart, HAL_CPU_LOCAL_OFFSET, HalCpuLocal, MAX_PAGE_TABLE_ROOTS, StartCpuError,
};
use eggos_hal::{Interrupt, Platform};

/// More CPUs than any machine has, against a corrupt firmware table: the CPUs listed beyond it
/// stay stopped.
pub const CPU_LIMIT: usize = 4096;

/// A CPU's per-CPU block: what only it changes, reached through its per-CPU register.
/// Allocated once, never freed.
#[derive(Debug)]
#[repr(C)]
pub struct Cpu {
    /// The block's own address, first: the HAL's per-CPU register contract
    /// ([`Platform::set_cpu_local`]). Set before the block is used, never changed after.
    this: AtomicPtr<Cpu>,
    /// The HAL's part, at [`HAL_CPU_LOCAL_OFFSET`]; the kernel never touches it.
    _hal: HalCpuLocal,
    /// What this CPU's scheduler has not looked at yet (D39, D43): in the low 63 bits, the timer
    /// interrupts the CPU has taken since it last looked; in the top bit ([`RESCHEDULE`]),
    /// another CPU's request that it look ([`Self::request_reschedule`]). Not 0 means it must
    /// look. Counted by the CPU's interrupt handler, read by the preemption point's fast path
    /// with one load ([`local_unseen_by_scheduler`]), and emptied by the scheduler
    /// ([`Self::see_scheduler_events`]), which moves the timer interrupts to
    /// [`Self::timer_interrupts_seen`]. Other CPUs only set the top bit.
    unseen_by_scheduler: AtomicU64,
    /// Timer interrupts the scheduler has looked at. Changed only by the CPU itself.
    timer_interrupts_seen: AtomicU64,
    /// Wake-up interrupts this CPU has taken ([`wake`]). Changed only by the CPU itself.
    wakeups: AtomicU64,
    index: usize,
    /// Set before the block is used, never changed after.
    hardware_id: AtomicU64,
    /// Set by the CPU itself once it runs the kernel's code.
    online: AtomicBool,
    /// The first root of the user address space active on this CPU
    /// ([`crate::address_space::AddressSpace::with_active`]), or [`NO_ADDRESS_SPACE`] while the
    /// kernel's empty low half is (D46). Changed only by the CPU itself.
    active_address_space: AtomicU64,
    /// The CPU's part of the scheduler: its running, ready and sleeping threads (D43).
    pub(crate) scheduler: CpuScheduler,
}

/// The top bit of [`Cpu::unseen_by_scheduler`]: another CPU asks this CPU's scheduler to look at
/// its ready threads.
const RESCHEDULE: u64 = 1 << 63;

const _: () = assert!(core::mem::offset_of!(Cpu, _hal) == HAL_CPU_LOCAL_OFFSET);

/// [`Cpu::swap_active_address_space`]'s value while no user address space is active: not a
/// table's address, which is frame-aligned.
pub(crate) const NO_ADDRESS_SPACE: u64 = u64::MAX;

impl Cpu {
    const fn new(index: usize) -> Self {
        Self {
            this: AtomicPtr::new(ptr::null_mut()),
            _hal: HalCpuLocal::new(),
            unseen_by_scheduler: AtomicU64::new(0),
            timer_interrupts_seen: AtomicU64::new(0),
            wakeups: AtomicU64::new(0),
            index,
            hardware_id: AtomicU64::new(0),
            online: AtomicBool::new(false),
            active_address_space: AtomicU64::new(NO_ADDRESS_SPACE),
            scheduler: CpuScheduler::new(),
        }
    }

    /// A CPU block for host tests that place threads on CPUs the test machine does not have.
    #[cfg(test)]
    pub(crate) fn for_test(index: usize) -> &'static Self {
        let cpu: &'static Self = Box::leak(Box::new(Self::new(index)));
        cpu.set_identity(index as u64);
        cpu
    }

    /// The kernel's number for the CPU: 0 for the boot CPU.
    pub fn index(&self) -> usize {
        self.index
    }

    /// The firmware's id for the CPU: the MPIDR affinity on AArch64, the hart id on RISC-V,
    /// the local APIC id on x86.
    pub fn hardware_id(&self) -> u64 {
        self.hardware_id.load(Ordering::Relaxed)
    }

    /// Whether the CPU runs the kernel's code: true for the boot CPU, and for a secondary CPU
    /// once it has reported in.
    pub fn is_online(&self) -> bool {
        self.online.load(Ordering::Acquire)
    }

    /// Timer interrupts the CPU has taken since boot. Read from another CPU, it can be briefly
    /// behind, never ahead, while the CPU's scheduler moves its unseen interrupts to the seen
    /// ones ([`Self::see_scheduler_events`] empties the unseen ones first, and they are read
    /// last).
    pub fn timer_interrupts(&self) -> u64 {
        let seen = self.timer_interrupts_seen.load(Ordering::Acquire);
        seen + self.timer_interrupts_unseen()
    }

    /// Timer interrupts the CPU has taken that its scheduler has not looked at yet.
    pub(crate) fn timer_interrupts_unseen(&self) -> u64 {
        self.unseen_by_scheduler.load(Ordering::Acquire) & !RESCHEDULE
    }

    /// For the scheduler, on this CPU, with interrupts masked: marks everything so far as seen,
    /// so [`local_unseen_by_scheduler`] is 0 until the next timer interrupt or reschedule
    /// request. Returns how many timer interrupts it saw: none if only a request came.
    pub(crate) fn see_scheduler_events(&self) -> u64 {
        let timer_interrupts = self.unseen_by_scheduler.swap(0, Ordering::AcqRel) & !RESCHEDULE;
        self.timer_interrupts_seen
            .fetch_add(timer_interrupts, Ordering::Release);
        timer_interrupts
    }

    /// Asks this CPU's scheduler to look at its ready threads at its next preemption point,
    /// yield or block, without an interrupt (D43): set by another CPU that made a thread ready
    /// here, with the scheduler's lock held, so the look finds the thread.
    pub(crate) fn request_reschedule(&self) {
        self.unseen_by_scheduler
            .fetch_or(RESCHEDULE, Ordering::Release);
    }

    /// [`Self::unseen_by_scheduler`] as it is, both parts, for the benchmarks to poll as cheaply
    /// as a global.
    #[cfg(any(test, feature = "qemu-bench"))]
    pub(crate) fn unseen_by_scheduler(&self) -> u64 {
        self.unseen_by_scheduler.load(Ordering::Relaxed)
    }

    /// Whether another CPU's reschedule request waits for this CPU's scheduler.
    #[cfg(test)]
    pub(crate) fn reschedule_requested(&self) -> bool {
        self.unseen_by_scheduler.load(Ordering::Relaxed) & RESCHEDULE != 0
    }

    /// Wake-up interrupts the CPU has taken since boot ([`wake`]).
    pub fn wakeups(&self) -> u64 {
        self.wakeups.load(Ordering::Relaxed)
    }

    /// Counts `interrupt`, which this CPU, whose block this is, has taken.
    pub(crate) fn count(&self, interrupt: Interrupt) {
        let counter = match interrupt {
            Interrupt::Timer => &self.unseen_by_scheduler,
            Interrupt::Wakeup => &self.wakeups,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// For this CPU, with interrupts masked: records `root` as the first root of the user
    /// address space now active here ([`NO_ADDRESS_SPACE`] for none) and returns the one
    /// recorded before.
    pub(crate) fn swap_active_address_space(&self, root: u64) -> u64 {
        self.active_address_space.swap(root, Ordering::Relaxed)
    }

    /// Sets the block's address and the CPU's hardware id, before anything uses the block.
    fn set_identity(&self, hardware_id: u64) {
        self.this
            .store(ptr::from_ref(self).cast_mut(), Ordering::Relaxed);
        self.hardware_id.store(hardware_id, Ordering::Relaxed);
    }
}

/// Every CPU the kernel knows, by index; set once, by [`start_secondaries`].
#[derive(Debug)]
pub struct CpuTable {
    /// A leaked slice of the CPUs, published with one store so that a reader never sees a
    /// pointer and a length that do not belong together.
    cpus: AtomicPtr<&'static [&'static Cpu]>,
    /// CPUs running the kernel's code, the boot CPU included.
    online: AtomicUsize,
}

impl CpuTable {
    const fn new() -> Self {
        Self {
            cpus: AtomicPtr::new(ptr::null_mut()),
            online: AtomicUsize::new(1),
        }
    }

    /// The CPUs, by index: the boot CPU first, then those started (or being started). Empty
    /// until [`start_secondaries`].
    pub fn all(&self) -> &'static [&'static Cpu] {
        let cpus = self.cpus.load(Ordering::Acquire);
        // SAFETY: `publish` stored a leaked slice, set once and never changed (Release, read
        // here with Acquire).
        unsafe { cpus.as_ref() }.copied().unwrap_or(&[])
    }

    /// CPUs running the kernel's code, the boot CPU included.
    pub fn online(&self) -> usize {
        self.online.load(Ordering::Acquire)
    }

    /// Publishes `cpus` as the table's CPUs; false if it already has some.
    fn publish(&self, cpus: &'static [&'static Cpu]) -> bool {
        let cpus: &'static mut &'static [&'static Cpu] = Box::leak(Box::new(cpus));
        self.cpus
            .compare_exchange(ptr::null_mut(), cpus, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
}

/// The machine's CPUs.
pub static CPUS: CpuTable = CpuTable::new();

/// The boot CPU's block, in the image so that it exists from the first instruction on.
static BOOT_CPU: Cpu = Cpu::new(0);

/// Makes [`current`] work on the boot CPU: gives it its block, index 0. Called once by the boot
/// CPU, before any other CPU starts.
pub fn init_boot_cpu<P: Platform>() {
    BOOT_CPU.set_identity(P::boot_cpu());
    BOOT_CPU.online.store(true, Ordering::Release);
    // SAFETY: `BOOT_CPU` is a static made by `Cpu::new`, whose first word now holds its own
    // address and whose HAL part is at the offset the HAL expects.
    unsafe { P::set_cpu_local(ptr::from_ref(&BOOT_CPU) as usize) };
}

/// The CPU this code runs on. Only valid once [`init_boot_cpu`] has run (on secondary CPUs,
/// always).
#[inline]
pub fn current<P: Platform>() -> &'static Cpu {
    // SAFETY: the per-CPU register holds a `Cpu` block, set by `init_boot_cpu` on the boot
    // CPU and by `secondary_main` on the others; blocks are never freed.
    unsafe { &*(P::cpu_local() as *const Cpu) }
}

/// This CPU's [`Cpu::unseen_by_scheduler`], with one load: as cheap as a global on every
/// architecture. Not 0 when a timer interrupt or a reschedule request has come since its
/// scheduler last looked. Only valid once [`init_boot_cpu`] has run.
#[inline]
pub(crate) fn local_unseen_by_scheduler<P: Platform>() -> u64 {
    // SAFETY: the per-CPU register holds this CPU's block (see `current`), in which the field
    // is an aligned `AtomicU64`.
    unsafe { P::cpu_local_u64::<{ core::mem::offset_of!(Cpu, unseen_by_scheduler) }>() }
}

/// Raises the wake-up interrupt on `cpu`, which must not be the calling CPU: it stops waiting
/// for an interrupt, and whatever the caller wrote before is visible to it (D41).
/// Call it with interrupts masked, as the kernel runs.
///
/// A CPU that has not reported in yet is left alone (D43): a thread can be placed on it once
/// its scheduler has started, just before it reports in, and its idle thread looks at its
/// ready threads after reporting in, before it first waits. Seeing it not online here means
/// that look has not taken the scheduler's lock yet, so it finds whatever the caller queued.
///
/// Out of line and cold: inlined, the check made the scheduler's wake paths dearer even when
/// they send no interrupt.
#[cold]
#[inline(never)]
pub fn wake<P: Platform>(cpu: &Cpu) {
    if cpu.is_online() {
        P::wake_cpu(cpu.hardware_id());
    }
}

/// Why [`start_secondaries`] stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartError {
    /// The CPUs were started already.
    AlreadyStarted,
    /// No frames for a CPU's stack.
    OutOfMemory,
    /// The firmware or the machine would not start the CPU with this hardware id.
    Refused(u64, StartCpuError),
}

/// What [`start_secondaries`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartSummary {
    /// CPUs asked to start, which then report in on their own (see [`CpuTable::online`]).
    pub started: usize,
    /// CPUs the firmware lists beyond [`CPU_LIMIT`], left stopped.
    pub left_stopped: usize,
}

/// Starts every CPU platform `P`'s firmware lists, other than the boot CPU, on the kernel's
/// page tables `roots`: numbers them from 1 in the firmware's order, publishes them in
/// [`CPUS`], then starts each on a stack of its own, where it reports in and parks. Returns
/// once every start is requested; the CPUs come online on their own. Stops at the first CPU
/// that cannot be started; the CPUs started before it run, and its stack stays allocated.
///
/// # Safety
/// `roots` must be the kernel's own page tables, active ([`crate::paging::enable`]) and
/// allocated and unchanged from then on, as they are. [`init_boot_cpu`] must have run.
pub unsafe fn start_secondaries<P: Platform>(roots: &[u64]) -> Result<StartSummary, StartError> {
    let mut ids = Vec::new();
    P::cpus(&mut |id| ids.push(id));
    let mut page_table_roots = [0; MAX_PAGE_TABLE_ROOTS];
    page_table_roots[..roots.len()].copy_from_slice(roots);
    let start = |id, start: &CpuStart| {
        // SAFETY: the caller guarantees the page tables; `start_with` keeps `start` and its
        // stack as the HAL requires, and starts one CPU at a time.
        unsafe { P::start_cpu(id, start) }
    };
    start_with(
        &CPUS,
        &BOOT_CPU,
        &ids,
        page_table_roots,
        secondary_main::<P>,
        start,
    )
}

/// [`start_secondaries`] for the CPUs `ids` other than `boot`'s, into `table`, starting each
/// with `start_cpu` to run `entry` on its [`Cpu`].
fn start_with(
    table: &'static CpuTable,
    boot: &'static Cpu,
    ids: &[u64],
    page_table_roots: [u64; MAX_PAGE_TABLE_ROOTS],
    entry: extern "C" fn(usize) -> !,
    mut start_cpu: impl FnMut(u64, &CpuStart) -> Result<(), StartCpuError>,
) -> Result<StartSummary, StartError> {
    let secondaries = ids.iter().filter(|&&id| id != boot.hardware_id());
    let listed = secondaries.clone().count();
    let count = listed.min(CPU_LIMIT - 1);
    let mut cpus: Vec<&'static Cpu> = Vec::with_capacity(count + 1);
    cpus.push(boot);
    for (index, &hardware_id) in (1..).zip(secondaries.take(count)) {
        let cpu: &'static Cpu = Box::leak(Box::new(Cpu::new(index)));
        cpu.set_identity(hardware_id);
        cpus.push(cpu);
    }
    if !table.publish(Vec::leak(cpus)) {
        return Err(StartError::AlreadyStarted);
    }
    for &cpu in &table.all()[1..] {
        let stack = GlobalFrames
            .allocate_frames(STACK_ORDER)
            .ok_or(StartError::OutOfMemory)?;
        // Leaked: the CPU may read it until it calls its entry, which the boot CPU does not
        // wait for. A few words, once per CPU.
        let start = Box::leak(Box::new(CpuStart {
            stack_top: stack + STACK_SIZE,
            entry,
            argument: ptr::from_ref(cpu) as usize,
            page_table_roots,
        }));
        // A refused CPU keeps its stack, like its `CpuStart`: on x86 a CPU that missed its
        // deadline may still start late, and must not find the frames reused.
        start_cpu(cpu.hardware_id(), start)
            .map_err(|error| StartError::Refused(cpu.hardware_id(), error))?;
    }
    Ok(StartSummary {
        started: count,
        left_stopped: listed - count,
    })
}

/// Where a secondary CPU starts, on its own stack with interrupts masked, given its [`Cpu`]:
/// sets its per-CPU register, takes one timer interrupt (so a CPU whose timer or interrupt
/// controller does not work never reports in), starts its scheduler, reports in, then runs its
/// idle thread on that stack (D43): threads placed on the CPU run from then on.
extern "C" fn secondary_main<P: Platform>(cpu: usize) -> ! {
    // SAFETY: `start_with` passes the address of a leaked `Cpu`, made by `Cpu::new`, whose
    // first word holds that address and whose HAL part is at the offset the HAL expects.
    unsafe { P::set_cpu_local(cpu) };
    let cpu = current::<P>();
    // A deadline already passed fires at once.
    P::set_timer_deadline(P::clock_ticks());
    while cpu.timer_interrupts() == 0 {
        P::wait_for_interrupt();
    }
    thread::start_secondary::<P>();
    report_in(cpu, &CPUS);
    thread::run_idle::<P>()
}

/// Marks `cpu` online in `table`.
fn report_in(cpu: &Cpu, table: &CpuTable) {
    cpu.online.store(true, Ordering::Release);
    table.online.fetch_add(1, Ordering::AcqRel);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thread::host::frames_for_test;
    use eggos_hal::Interrupt;
    use eggos_hal::cpu::StartCpuError;

    extern "C" fn never(_: usize) -> ! {
        unreachable!("the test's CPUs do not run")
    }

    /// A table and a boot CPU with hardware id `boot` of a test's own.
    fn machine(boot: u64) -> (&'static CpuTable, &'static Cpu) {
        let boot_cpu: &'static Cpu = Box::leak(Box::new(Cpu::new(0)));
        boot_cpu.set_identity(boot);
        (Box::leak(Box::new(CpuTable::new())), boot_cpu)
    }

    fn free_frames() -> u64 {
        crate::memory::FRAMES.lock().as_ref().unwrap().free_frames()
    }

    #[test]
    fn a_cpu_that_has_not_reported_in_is_not_woken() {
        // The test platform has no wake-up interrupt: raising one panics.
        wake::<crate::tests::TestPlatform>(Cpu::for_test(1));
    }

    #[test]
    fn numbers_the_other_cpus_in_firmware_order_and_starts_each_on_its_block() {
        let _frames = frames_for_test();
        let (table, boot) = machine(3);
        let roots = [0x1000, 0x2000];
        let mut seen = Vec::new();
        let started = start_with(table, boot, &[7, 3, 9], roots, never, |id, start| {
            // What the HAL's entry code would do: run on the stack, find the block.
            assert_eq!(start.stack_top % 16, 0);
            assert_eq!(start.page_table_roots, roots);
            // SAFETY: the argument is a leaked `Cpu`, whose first word is `this`.
            let first_word = unsafe { *(start.argument as *const usize) };
            assert_eq!(first_word, start.argument);
            // SAFETY: as above.
            let cpu = unsafe { &*(start.argument as *const Cpu) };
            seen.push((id, cpu.index()));
            report_in(cpu, table);
            Ok(())
        });
        assert_eq!(
            started,
            Ok(StartSummary {
                started: 2,
                left_stopped: 0
            })
        );
        assert_eq!(seen, [(7, 1), (9, 2)]);
        let ids: Vec<_> = table.all().iter().map(|cpu| cpu.hardware_id()).collect();
        assert_eq!(ids, [3, 7, 9]);
        assert_eq!(table.online(), 3);
        assert!(table.all()[1..].iter().all(|cpu| cpu.is_online()));
        assert_eq!(
            start_with(table, boot, &[7], roots, never, |_, _| Ok(())),
            Err(StartError::AlreadyStarted)
        );
    }

    #[test]
    fn a_refused_cpu_stops_the_start_and_keeps_its_stack() {
        let _frames = frames_for_test();
        let (table, boot) = machine(0);
        let before = free_frames();
        let mut starts = 0;
        let result = start_with(table, boot, &[0, 1, 2, 4], [0; 2], never, |id, _| {
            starts += 1;
            if id == 2 {
                Err(StartCpuError::InvalidCpu)
            } else {
                Ok(())
            }
        });
        assert_eq!(
            result,
            Err(StartError::Refused(2, StartCpuError::InvalidCpu))
        );
        assert_eq!(starts, 2);
        // CPUs 1 and 2 keep their stacks (2 might still start late); CPU 4's is never taken.
        assert_eq!(free_frames(), before - (2 << STACK_ORDER));
        assert_eq!(table.online(), 1);
        assert_eq!(table.all().len(), 4);
    }

    #[test]
    fn interrupts_are_counted_per_kind_and_the_scheduler_sees_timer_interrupts_once() {
        let cpu = Cpu::new(1);
        cpu.count(Interrupt::Timer);
        cpu.count(Interrupt::Timer);
        cpu.count(Interrupt::Wakeup);
        assert_eq!(
            (cpu.timer_interrupts(), cpu.timer_interrupts_unseen()),
            (2, 2)
        );
        assert_eq!(cpu.wakeups(), 1);
        assert_eq!(cpu.see_scheduler_events(), 2);
        assert_eq!(
            (cpu.timer_interrupts(), cpu.timer_interrupts_unseen()),
            (2, 0)
        );
        cpu.count(Interrupt::Timer);
        assert_eq!(
            (cpu.timer_interrupts(), cpu.timer_interrupts_unseen()),
            (3, 1)
        );
        assert_eq!(cpu.wakeups(), 1);
    }

    #[test]
    fn a_reschedule_request_is_seen_once_and_counts_no_timer_interrupt() {
        let cpu = Cpu::new(1);
        cpu.count(Interrupt::Timer);
        cpu.request_reschedule();
        cpu.request_reschedule();
        assert!(cpu.reschedule_requested());
        assert_eq!(
            (cpu.timer_interrupts(), cpu.timer_interrupts_unseen()),
            (1, 1)
        );
        assert_eq!(cpu.see_scheduler_events(), 1);
        assert!(!cpu.reschedule_requested());
        assert_eq!(cpu.unseen_by_scheduler.load(Ordering::Relaxed), 0);
        // A request alone: the scheduler looks, and sees no timer interrupt.
        cpu.request_reschedule();
        assert_ne!(cpu.unseen_by_scheduler.load(Ordering::Relaxed), 0);
        assert_eq!(cpu.see_scheduler_events(), 0);
        assert_eq!(cpu.timer_interrupts(), 1);
    }

    #[test]
    fn one_cpu_starts_nothing() {
        let _frames = frames_for_test();
        let (table, boot) = machine(5);
        let started = start_with(table, boot, &[5], [0; 2], never, |_, _| {
            unreachable!("there is no other CPU")
        });
        assert_eq!(
            started,
            Ok(StartSummary {
                started: 0,
                left_stopped: 0
            })
        );
        assert_eq!(table.all().len(), 1);
        // Also when the firmware lists none at all.
        let (table, boot) = machine(5);
        assert!(start_with(table, boot, &[], [0; 2], never, |_, _| Ok(())).is_ok());
        assert_eq!(table.all().len(), 1);
    }
}
