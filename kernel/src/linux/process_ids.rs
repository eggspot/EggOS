//! Process and thread ids after the first round (D183): once ids 1 to
//! [`ProcessId::MAX`] have all been given out, ids are handed out again from
//! [`FIRST_REUSED`] up, cyclically, as Linux does, and only ids nothing holds: no process,
//! ended and not yet reaped included, no thread, no process group or session (which outlive
//! their leader), not the console's session or foreground group. The search runs only when
//! the run of free ids it found last is used up: it takes the ids in use from the process
//! table and from each running process's threads, and finds the first free run after the last
//! id handed out, wrapping round. The [`RECENT`] ids handed out last, and the [`RECENT`] ids
//! just before where the search starts, are held back, so an id whose process or thread is
//! not in the table or its process yet is not handed out twice. A child's id goes into the
//! process table the moment it is allocated, and one another process took in between is
//! passed over ([`crate::process::Process::new_child`]), so a wait or a signal sent to an id
//! reaches the one process that holds it. Until the first round ends none of this runs: an id
//! costs one atomic update, as before.

use super::process_table::TABLE;
use crate::process::{Process, ProcessId};
use crate::sync::SpinLock;
use alloc::sync::Arc;
use alloc::vec::Vec;
use eggos_hal::Platform;

/// The first id handed out again: lower ids, which the first programs and services took,
/// stay theirs (Linux's `RESERVED_PIDS`).
pub const FIRST_REUSED: u32 = 300;

/// How many of the ids handed out last are held back from the next search.
pub const RECENT: usize = 64;

struct Reuse {
    next: u32,
    end: u32,
    recent: [u32; RECENT],
    recent_at: usize,
}

impl Reuse {
    fn remember(&mut self, id: u32) {
        self.recent[self.recent_at] = id;
        self.recent_at = (self.recent_at + 1) % RECENT;
    }
}

static REUSE: SpinLock<Reuse> = SpinLock::new(Reuse {
    next: 0,
    end: 0,
    recent: [0; RECENT],
    recent_at: 0,
});

/// The next id once the first round is over (see the module documentation); `None` when
/// every id is held, or there is no memory to look.
#[cold]
#[inline(never)]
pub fn allocate_again<P: Platform>() -> Option<u32> {
    let mut held: Vec<Arc<Process<P>>> = Vec::new();
    let id = {
        let mut reuse = REUSE.lock();
        if reuse.next >= reuse.end {
            let cursor = match reuse.end {
                0 => ProcessId::MAX + 1,
                end => end,
            };
            let used = ids_in_use::<P>(&reuse.recent, &mut held)?;
            let (start, end) = free_run(&used, cursor)?;
            reuse.next = start;
            reuse.end = end;
        }
        let id = reuse.next;
        reuse.next += 1;
        reuse.remember(id);
        id
    };
    drop(held);
    Some(id)
}

fn ids_in_use<P: Platform>(recent: &[u32], held: &mut Vec<Arc<Process<P>>>) -> Option<Vec<u32>> {
    let mut used = Vec::new();
    let running = TABLE.lock().ids_in_use(&mut used).ok()?;
    held.try_reserve(running.len()).ok()?;
    for reach in &running {
        let Some(process) = reach
            .process
            .upgrade()
            .and_then(|process| process.downcast::<Process<P>>().ok())
        else {
            continue;
        };
        process.signals().lock().other_thread_ids(&mut used).ok()?;
        held.push(process);
    }
    used.try_reserve(recent.len()).ok()?;
    used.extend_from_slice(recent);
    used.sort_unstable();
    used.dedup();
    Some(used)
}

/// The first run of ids `used` (sorted, each once) does not hold, as `(first, end)` with `end`
/// past its last: at `cursor` or after, up to [`ProcessId::MAX`]; otherwise from
/// [`FIRST_REUSED`] up to [`RECENT`] ids before `cursor`. `None` when there is none.
pub fn free_run(used: &[u32], cursor: u32) -> Option<(u32, u32)> {
    let after = cursor.max(FIRST_REUSED);
    first_free(used, after, ProcessId::MAX + 1)
        .or_else(|| first_free(used, FIRST_REUSED, cursor.saturating_sub(RECENT as u32)))
}

fn first_free(used: &[u32], start: u32, end: u32) -> Option<(u32, u32)> {
    let mut candidate = start;
    let from = used.partition_point(|&id| id < start);
    for &id in &used[from..] {
        if id >= end {
            break;
        }
        if id > candidate {
            return Some((candidate, id));
        }
        candidate = id + 1;
    }
    (candidate < end).then_some((candidate, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    const END: u32 = ProcessId::MAX + 1;

    #[test]
    fn the_first_search_starts_again_above_the_reserved_ids() {
        assert_eq!(free_run(&[1, 2, 300, 301, 305], END), Some((302, 305)));
        assert_eq!(free_run(&[], END), Some((300, END - RECENT as u32)));
    }

    #[test]
    fn a_search_goes_on_after_the_last_id_handed_out() {
        let used = [400, 500, 501, 502, 900];
        assert_eq!(free_run(&used, 500), Some((503, 900)));
        assert_eq!(free_run(&used, 503), Some((503, 900)));
        assert_eq!(free_run(&used, 901), Some((901, END)));
    }

    #[test]
    fn past_the_largest_id_the_search_wraps_to_the_reserved_end() {
        let used: Vec<u32> = (FIRST_REUSED..=ProcessId::MAX - 10).collect();
        assert_eq!(
            free_run(&used, ProcessId::MAX - 10),
            Some((ProcessId::MAX - 9, END))
        );
        let mut full_above: Vec<u32> = (5000..=ProcessId::MAX).collect();
        full_above.insert(0, 4000);
        assert_eq!(free_run(&full_above, 6000), Some((300, 4000)));
    }

    #[test]
    fn the_ids_just_before_the_cursor_are_held_back() {
        let used: Vec<u32> = (FIRST_REUSED..2000)
            .chain(2000 + RECENT as u32..=ProcessId::MAX)
            .collect();
        assert_eq!(free_run(&used, 2000 + RECENT as u32), None);
        assert_eq!(
            free_run(&used, 2000 + 2 * RECENT as u32),
            Some((2000, 2000 + RECENT as u32))
        );
    }

    #[test]
    fn every_id_held_finds_none() {
        let used: Vec<u32> = (1..=ProcessId::MAX).collect();
        assert_eq!(free_run(&used, 1234), None);
        assert_eq!(free_run(&used, END), None);
    }

    #[test]
    fn the_ids_handed_out_last_are_remembered_round_and_round() {
        let mut reuse = Reuse {
            next: 0,
            end: 0,
            recent: [0; RECENT],
            recent_at: 0,
        };
        for id in 0..RECENT as u32 + 3 {
            reuse.remember(1000 + id);
        }
        assert_eq!(reuse.recent[0], 1000 + RECENT as u32);
        assert_eq!(reuse.recent[2], 1002 + RECENT as u32);
        assert_eq!(reuse.recent[3], 1003);
        assert_eq!(reuse.recent_at, 3);
    }
}
