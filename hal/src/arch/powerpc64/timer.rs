//! The POWER timer (D304): the time base, which every CPU reads at 512 MHz, and each CPU's
//! decrementer, a 32-bit count of time-base ticks that raises interrupt `0x900` when it turns
//! negative. Wake-ups between CPUs are hypervisor doorbells (`msgsnd`, interrupt `0xe80`).

/// POWER9's time-base frequency.
pub(super) const FREQUENCY: u64 = 512_000_000;
/// The longest count the decrementer holds.
const LONGEST_COUNT: u64 = 0x7fff_ffff;
/// `msgsnd`'s message type for a hypervisor doorbell.
const HYPERVISOR_DOORBELL: u64 = 5 << 27;

/// The time base.
#[inline]
pub(super) fn now() -> u64 {
    let value: u64;
    // SAFETY: reading the time base has no side effects.
    unsafe { core::arch::asm!("mftb {}", out(reg) value, options(nomem, nostack)) };
    value
}

fn set_decrementer(count: u64) {
    // SAFETY: the decrementer only counts down and raises its interrupt when it turns
    // negative; the kernel takes that interrupt.
    unsafe { core::arch::asm!("mtspr 22, {}", in(reg) count, options(nomem, nostack)) };
}

/// Disarms this CPU's decrementer.
pub(super) extern "C" fn init() {
    set_decrementer(LONGEST_COUNT);
}

/// Arms this CPU's decrementer for time-base tick `deadline`.
pub(super) fn set_deadline(deadline: u64) {
    let count = deadline.saturating_sub(now()).clamp(1, LONGEST_COUNT);
    set_decrementer(count);
}

/// The decrementer interrupt was taken: it raises no other until armed again.
pub(super) fn end_timer_interrupt() {
    set_decrementer(LONGEST_COUNT);
}

/// Rings CPU `cpu`'s hypervisor doorbell.
pub(super) fn wake(cpu: u64) {
    // SAFETY: a doorbell only raises interrupt 0xe80 on the CPU whose PIR is `cpu`.
    unsafe {
        core::arch::asm!("sync", "msgsnd {}", in(reg) HYPERVISOR_DOORBELL | cpu, options(nostack))
    };
}

/// This CPU's doorbell was taken: it is cleared.
pub(super) fn end_wakeup() {
    // SAFETY: clears this CPU's pending hypervisor doorbell only.
    unsafe { core::arch::asm!("msgclr {}", in(reg) HYPERVISOR_DOORBELL, options(nomem, nostack)) };
}

/// Stops this CPU until an interrupt condition, resuming at the next instruction (`PSSCR` 0:
/// the lightest state, no state loss).
pub(super) fn stop() {
    // SAFETY: `stop` with `PSSCR.EC` 0 only waits; the thread resumes after it with every
    // register as it was.
    unsafe {
        core::arch::asm!("li {zero}, 0", "mtspr 855, {zero}", "stop", zero = out(reg) _, options(nomem, nostack))
    };
}
