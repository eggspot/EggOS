//! The out-of-memory killer (D559): when a program's page fault finds no frame above the reserve,
//! the program with the most writable memory mapped, other than init, the faulting one and the
//! kernel's own, is ended with `SIGKILL`, and the fault is tried once more, as Linux's OOM killer
//! picks the largest. Only when no other program can be ended does the faulting one end. Its
//! memory comes back as its threads stop, so the retry waits a moment for the frame allocator to
//! have frames above the reserve again.

use super::process_table::TABLE;
use super::signal::SIGKILL;
use super::signals::Info;
use crate::process::Process;
use alloc::sync::Arc;
use eggos_hal::Platform;

/// How long the faulting thread waits for the ended program's memory to come back.
const RECLAIM_WAIT: core::time::Duration = core::time::Duration::from_millis(100);

/// Ends the program with the most writable memory other than init and `faulting`, and waits a
/// moment for its memory; whether one was ended.
pub fn end_largest_other<P: Platform>(faulting: u32) -> bool {
    let (init, targets) = {
        let table = TABLE.lock();
        (table.init(), table.every_target_but(faulting))
    };
    let largest = targets
        .into_iter()
        .filter(|target| target.id != init && target.id > 1)
        .filter_map(|target| {
            let running = target.running?;
            let process = running.process.upgrade()?.downcast::<Process<P>>().ok()?;
            let (_, writable) = process.mapped_bytes();
            Some((writable, running, process))
        })
        .max_by_key(|(writable, _, _)| *writable);
    let Some((_, running, process)) = largest else {
        return false;
    };
    super::wait::send_to_running::<P>(&running, Info::from_kernel(SIGKILL));
    drop(running);
    let ended: Arc<Process<P>> = process;
    let mut waited = core::time::Duration::ZERO;
    while !ended.has_ended() && waited < RECLAIM_WAIT {
        crate::thread::sleep::<P>(core::time::Duration::from_millis(10));
        waited += core::time::Duration::from_millis(10);
    }
    drop(ended);
    crate::memory::user_may_take_frames(1)
}
