//! One client's event queue (D127, D128): a ring of [`QUEUE_CAPACITY`] records. A `Resize`
//! replaces a `Resize` of the same window still waiting, so only the latest size is pending.
//! When it is full, a pointer motion replaces the newest record if that is a motion of the
//! same window too; otherwise the oldest record is dropped and counted, and the next record
//! taken is `Overflow` with the count.

use super::protocol::{EventRecord, QUEUE_CAPACITY, event};
use alloc::boxed::Box;
use alloc::vec::Vec;

/// See the module documentation.
#[derive(Debug, Clone)]
pub struct EventQueue {
    records: Box<[EventRecord]>,
    start: usize,
    length: usize,
    dropped: u32,
}

impl EventQueue {
    /// An empty queue, its records on the heap; `None` without memory for them.
    pub fn new() -> Option<Self> {
        let mut records = Vec::new();
        records.try_reserve_exact(QUEUE_CAPACITY).ok()?;
        records.resize(QUEUE_CAPACITY, EventRecord::default());
        Some(Self {
            records: records.into_boxed_slice(),
            start: 0,
            length: 0,
            dropped: 0,
        })
    }

    /// Whether a record waits to be taken.
    pub fn is_empty(&self) -> bool {
        self.length == 0 && self.dropped == 0
    }

    /// How many records [`Self::pop`] would give, the `Overflow` one included.
    pub fn len(&self) -> usize {
        self.length + usize::from(self.dropped > 0)
    }

    /// How many records it takes before it must drop one.
    pub fn room(&self) -> usize {
        QUEUE_CAPACITY.saturating_sub(self.len())
    }

    /// Adds `record` at the end, making room as the module documentation says.
    pub fn push(&mut self, record: EventRecord) {
        if record.kind == event::RESIZE
            && let Some(waiting) = (0..self.length)
                .map(|index| (self.start + index) % QUEUE_CAPACITY)
                .find(|&at| {
                    self.records[at].kind == event::RESIZE
                        && self.records[at].window == record.window
                })
        {
            self.records[waiting] = record;
            return;
        }
        if self.length == QUEUE_CAPACITY {
            let newest = (self.start + self.length - 1) % QUEUE_CAPACITY;
            let last = self.records[newest];
            if record.kind == event::POINTER_MOVE
                && last.kind == event::POINTER_MOVE
                && last.window == record.window
            {
                self.records[newest] = record;
                return;
            }
            self.start = (self.start + 1) % QUEUE_CAPACITY;
            self.length -= 1;
            self.dropped = self.dropped.saturating_add(1);
        }
        self.records[(self.start + self.length) % QUEUE_CAPACITY] = record;
        self.length += 1;
    }

    /// Takes the oldest record: `Overflow` with the count dropped if records were dropped since
    /// the last one taken, stamped with the time of the record after it (or 0).
    pub fn pop(&mut self) -> Option<EventRecord> {
        if self.dropped > 0 {
            let dropped = core::mem::take(&mut self.dropped);
            let time_ms = match self.length {
                0 => 0,
                _ => self.records[self.start].time_ms,
            };
            let mut overflow = EventRecord::new(event::OVERFLOW, 0, dropped as i32, 0, 0, 0);
            overflow.time_ms = time_ms;
            return Some(overflow);
        }
        if self.length == 0 {
            return None;
        }
        let record = self.records[self.start];
        self.start = (self.start + 1) % QUEUE_CAPACITY;
        self.length -= 1;
        Some(record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(kind: u16, window: u16, a: i32) -> EventRecord {
        EventRecord::new(kind, window, a, 0, 0, 0)
    }

    #[test]
    fn records_come_out_in_the_order_they_went_in_across_the_ring_s_end() {
        let mut queue = EventQueue::new().unwrap();
        assert!(queue.is_empty() && queue.pop().is_none());
        for round in 0..3 {
            for index in 0..200 {
                queue.push(record(event::KEY, 1, round * 1000 + index));
            }
            for index in 0..200 {
                assert_eq!(
                    queue.pop(),
                    Some(record(event::KEY, 1, round * 1000 + index))
                );
            }
        }
        assert!(queue.is_empty());
    }

    #[test]
    fn a_full_queue_merges_motion_of_the_same_window_into_the_newest_record() {
        let mut queue = EventQueue::new().unwrap();
        for index in 0..QUEUE_CAPACITY as i32 {
            queue.push(record(event::POINTER_MOVE, 3, index));
        }
        queue.push(record(event::POINTER_MOVE, 3, 999));
        assert_eq!(queue.len(), QUEUE_CAPACITY);
        let mut last = None;
        while let Some(taken) = queue.pop() {
            assert_ne!(taken.kind, event::OVERFLOW);
            last = Some(taken);
        }
        assert_eq!(last, Some(record(event::POINTER_MOVE, 3, 999)));
    }

    #[test]
    fn a_full_queue_drops_the_oldest_and_reports_the_count_first() {
        let mut queue = EventQueue::new().unwrap();
        for index in 0..QUEUE_CAPACITY as i32 {
            let mut key = record(event::KEY, 1, index);
            key.time_ms = index as u32;
            queue.push(key);
        }
        queue.push(record(event::POINTER_MOVE, 1, 1000));
        queue.push(record(event::POINTER_MOVE, 2, 1001));
        queue.push(record(event::BUTTON, 1, 1002));
        assert_eq!(queue.len(), QUEUE_CAPACITY + 1);
        let overflow = queue.pop().unwrap();
        assert_eq!((overflow.kind, overflow.a), (event::OVERFLOW, 3));
        assert_eq!(overflow.time_ms, 3);
        assert_eq!(
            queue.pop(),
            Some({
                let mut key = record(event::KEY, 1, 3);
                key.time_ms = 3;
                key
            })
        );
        let rest: alloc::vec::Vec<_> = core::iter::from_fn(|| queue.pop()).collect();
        assert_eq!(rest.len(), QUEUE_CAPACITY - 1);
        assert_eq!(rest[QUEUE_CAPACITY - 2], record(event::BUTTON, 1, 1002));
        assert!(queue.is_empty());
    }

    #[test]
    fn a_resize_replaces_the_one_of_its_window_still_waiting() {
        let mut queue = EventQueue::new().unwrap();
        queue.push(record(event::RESIZE, 1, 100));
        queue.push(record(event::KEY, 1, 7));
        queue.push(record(event::RESIZE, 2, 300));
        queue.push(record(event::RESIZE, 1, 200));
        assert_eq!(queue.pop(), Some(record(event::RESIZE, 1, 200)));
        assert_eq!(queue.pop(), Some(record(event::KEY, 1, 7)));
        assert_eq!(queue.pop(), Some(record(event::RESIZE, 2, 300)));
        assert!(queue.is_empty());
        queue.push(record(event::RESIZE, 1, 400));
        assert_eq!(queue.pop(), Some(record(event::RESIZE, 1, 400)));
    }

    #[test]
    fn an_event_record_is_32_bytes_laid_out_as_the_contract_says() {
        let mut key = EventRecord::new(event::KEY, 0x0141, -2, 1, 0x13, 0x41);
        key.time_ms = 0x0102_0304;
        let bytes = key.to_bytes();
        assert_eq!(bytes.len(), 32);
        assert_eq!(&bytes[0..2], &event::KEY.to_ne_bytes());
        assert_eq!(&bytes[2..4], &0x0141u16.to_ne_bytes());
        assert_eq!(&bytes[4..8], &0x0102_0304u32.to_ne_bytes());
        assert_eq!(&bytes[8..12], &(-2i32).to_ne_bytes());
        assert_eq!(&bytes[12..16], &1i32.to_ne_bytes());
        assert_eq!(&bytes[16..20], &0x13u32.to_ne_bytes());
        assert_eq!(&bytes[20..24], &0x41u32.to_ne_bytes());
        assert_eq!(&bytes[24..32], &[0; 8]);
    }
}
