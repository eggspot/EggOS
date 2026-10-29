//! `SetTimer`'s timers (D145): each fires as one `WM_TIMER` at a time, after the messages
//! posted, the input and the paints, as Windows orders them; the clock is passed in.

use super::message;
use super::queue::{Filter, Message};

/// The most timers a program has at once.
pub const MAX_TIMERS: usize = 32;
/// The shortest interval, Windows' `USER_TIMER_MINIMUM`.
pub const MINIMUM_INTERVAL_MS: u32 = 10;
const FIRST_THREAD_TIMER_ID: usize = 0x7f00;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Timer {
    window: usize,
    id: usize,
    interval: u32,
    due: u32,
    thread: u32,
    procedure: usize,
}

/// The program's timers.
#[derive(Debug)]
pub struct Timers {
    entries: [Option<Timer>; MAX_TIMERS],
    next_id: usize,
}

impl Default for Timers {
    fn default() -> Self {
        Self::new()
    }
}

fn reached(now: u32, due: u32) -> bool {
    now.wrapping_sub(due) as i32 >= 0
}

impl Timers {
    pub const fn new() -> Self {
        Self {
            entries: [None; MAX_TIMERS],
            next_id: FIRST_THREAD_TIMER_ID,
        }
    }

    /// `SetTimer`: a window's timer `id` (replaced when it exists), or with `window` 0 a
    /// thread timer of `thread` (`id` replaced when it is one of the thread's, else a new id);
    /// the timer's id, `None` when the table is full.
    pub fn set(
        &mut self,
        window: usize,
        id: usize,
        interval: u32,
        procedure: usize,
        thread: u32,
        now: u32,
    ) -> Option<usize> {
        let existing = self.entries.iter().position(|entry| {
            entry.is_some_and(|timer| {
                timer.window == window && timer.id == id && (window != 0 || timer.thread == thread)
            })
        });
        let id = match (window, existing) {
            (0, None) => self.fresh_id(),
            _ => id,
        };
        let slot = existing.or_else(|| self.entries.iter().position(Option::is_none))?;
        let interval = interval.max(MINIMUM_INTERVAL_MS);
        self.entries[slot] = Some(Timer {
            window,
            id,
            interval,
            due: now.wrapping_add(interval),
            thread,
            procedure,
        });
        Some(id)
    }

    fn fresh_id(&mut self) -> usize {
        loop {
            let id = self.next_id;
            self.next_id = match self.next_id.checked_add(1) {
                Some(next) => next,
                None => FIRST_THREAD_TIMER_ID,
            };
            if !self
                .entries
                .iter()
                .any(|entry| entry.is_some_and(|timer| timer.window == 0 && timer.id == id))
            {
                return id;
            }
        }
    }

    /// `KillTimer`; whether there was such a timer.
    pub fn kill(&mut self, window: usize, id: usize, thread: u32) -> bool {
        let found = self.entries.iter().position(|entry| {
            entry.is_some_and(|timer| {
                timer.window == window && timer.id == id && (window != 0 || timer.thread == thread)
            })
        });
        match found {
            Some(slot) => {
                self.entries[slot] = None;
                true
            }
            None => false,
        }
    }

    /// Removes every timer of `window`, which is gone.
    pub fn kill_window(&mut self, window: usize) {
        for entry in &mut self.entries {
            if entry.is_some_and(|timer| timer.window == window) {
                *entry = None;
            }
        }
    }

    /// The first due timer `filter` accepts as a `WM_TIMER`; taking it (`remove`) starts its
    /// next interval, a look leaves it due.
    pub fn take_due(&mut self, filter: &Filter, now: u32, remove: bool) -> Option<Message> {
        if !filter.accepts_number(message::TIMER) {
            return None;
        }
        for timer in self.entries.iter_mut().flatten() {
            let message = Message {
                window: timer.window,
                message: message::TIMER,
                wparam: timer.id,
                lparam: timer.procedure,
                time: now,
                point: (0, 0),
                thread: timer.thread,
            };
            if !reached(now, timer.due) || !filter.accepts(&message) {
                continue;
            }
            if remove {
                timer.due = now.wrapping_add(timer.interval);
            }
            return Some(message);
        }
        None
    }

    /// The milliseconds until `thread`'s next timer is due (0 when one is), `None` without one.
    pub fn next_due(&self, thread: u32, now: u32) -> Option<u32> {
        self.entries
            .iter()
            .flatten()
            .filter(|timer| timer.thread == thread)
            .map(|timer| match reached(now, timer.due) {
                true => 0,
                false => timer.due.wrapping_sub(now),
            })
            .min()
    }

    /// Whether `procedure` is the `TIMERPROC` of `thread`'s timer `id` of `window`, so
    /// `DispatchMessageW` may call it.
    pub fn has_procedure(&self, window: usize, id: usize, procedure: usize, thread: u32) -> bool {
        procedure != 0
            && self.entries.iter().flatten().any(|timer| {
                timer.window == window
                    && timer.id == id
                    && timer.procedure == procedure
                    && timer.thread == thread
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn any(thread: u32) -> Filter {
        Filter {
            thread,
            ..Filter::default()
        }
    }

    #[test]
    fn a_timer_fires_once_per_interval_and_a_look_does_not_restart_it() {
        let mut timers = Timers::new();
        assert_eq!(timers.set(0x10, 1, 100, 0, 7, 1000), Some(1));
        assert_eq!(timers.take_due(&any(7), 1099, true), None);
        assert_eq!(timers.next_due(7, 1050), Some(50));
        let peeked = timers.take_due(&any(7), 1100, false).unwrap();
        assert_eq!((peeked.window, peeked.wparam), (0x10, 1));
        assert!(timers.take_due(&any(7), 1100, false).is_some());
        assert!(timers.take_due(&any(7), 1150, true).is_some());
        assert_eq!(timers.take_due(&any(7), 1200, true), None);
        assert_eq!(timers.next_due(7, 1200), Some(50));
        assert_eq!(timers.next_due(8, 1200), None);
        assert_eq!(timers.take_due(&any(8), 5000, true), None);
    }

    #[test]
    fn thread_timers_get_fresh_ids_and_window_timers_are_replaced() {
        let mut timers = Timers::new();
        let first = timers.set(0, 0, 50, 0x99, 7, 0).unwrap();
        let second = timers.set(0, 0, 50, 0x99, 7, 0).unwrap();
        assert_ne!(first, second);
        assert_eq!(timers.set(0, first, 80, 0x99, 7, 0), Some(first));
        assert_eq!(timers.set(0x10, 3, 1, 0, 7, 0), Some(3));
        assert_eq!(timers.set(0x10, 3, 500, 0, 7, 0), Some(3));
        assert_eq!(timers.next_due(7, 0), Some(50));
        assert!(timers.has_procedure(0, first, 0x99, 7));
        assert!(!timers.has_procedure(0, first, 0x98, 7));
        assert!(!timers.has_procedure(0, first, 0x99, 8));
        assert!(timers.kill(0, first, 7));
        assert!(!timers.kill(0, first, 7));
        timers.kill_window(0x10);
        assert!(!timers.kill(0x10, 3, 7));
    }

    #[test]
    fn intervals_are_at_least_ten_milliseconds_and_the_clock_may_wrap() {
        let mut timers = Timers::new();
        timers.set(0x10, 1, 0, 0, 7, u32::MAX - 4);
        assert_eq!(timers.take_due(&any(7), 4, true), None);
        assert!(timers.take_due(&any(7), 5, true).is_some());
    }
}
