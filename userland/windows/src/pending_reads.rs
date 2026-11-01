//! Overlapped reads still in progress (D189): an overlapped `ReadFile` on a pipe with nothing
//! to read yet returns `ERROR_IO_PENDING` and is kept here, by its `OVERLAPPED`, until a wait
//! on its event finds the pipe readable, `GetOverlappedResult` waits for it, or `CancelIo`
//! cancels it.

/// How many reads can be in progress at once.
pub const CAPACITY: usize = 16;

/// A read in progress: up to `length` bytes from descriptor `fd` into the program's `buffer`,
/// for the `OVERLAPPED` at `overlapped`, whose event is `event` (0 for none).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingRead {
    pub fd: usize,
    pub buffer: usize,
    pub length: usize,
    pub overlapped: usize,
    pub event: usize,
}

/// The reads in progress.
#[derive(Debug)]
pub struct PendingReads {
    reads: [Option<PendingRead>; CAPACITY],
}

impl PendingReads {
    /// None.
    pub const fn new() -> Self {
        Self {
            reads: [None; CAPACITY],
        }
    }

    /// Whether no read is in progress.
    pub fn is_empty(&self) -> bool {
        self.reads.iter().all(Option::is_none)
    }

    /// Keeps `read`, replacing one for the same `OVERLAPPED`; `false` when all places are taken.
    pub fn insert(&mut self, read: PendingRead) -> bool {
        let place = self
            .reads
            .iter()
            .position(|kept| kept.is_some_and(|kept| kept.overlapped == read.overlapped))
            .or_else(|| self.reads.iter().position(Option::is_none));
        match place {
            Some(at) => {
                self.reads[at] = Some(read);
                true
            }
            None => false,
        }
    }

    /// Takes the read for the `OVERLAPPED` at `overlapped`.
    pub fn take(&mut self, overlapped: usize) -> Option<PendingRead> {
        self.take_first(|read| read.overlapped == overlapped)
    }

    /// Takes one read whose event is `event`.
    pub fn take_with_event(&mut self, event: usize) -> Option<PendingRead> {
        self.take_first(|read| read.event == event && event != 0)
    }

    /// Takes one read from descriptor `fd`.
    pub fn take_from(&mut self, fd: usize) -> Option<PendingRead> {
        self.take_first(|read| read.fd == fd)
    }

    fn take_first(&mut self, wanted: impl Fn(&PendingRead) -> bool) -> Option<PendingRead> {
        let slot = self
            .reads
            .iter_mut()
            .find(|read| read.as_ref().is_some_and(&wanted))?;
        slot.take()
    }
}

impl Default for PendingReads {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(fd: usize, overlapped: usize, event: usize) -> PendingRead {
        PendingRead {
            fd,
            buffer: 0x1000,
            length: 16,
            overlapped,
            event,
        }
    }

    #[test]
    fn a_read_is_found_by_its_overlapped_its_event_and_its_descriptor() {
        let mut reads = PendingReads::new();
        assert!(reads.is_empty());
        assert!(reads.insert(read(3, 0x100, 0x40)));
        assert!(reads.insert(read(4, 0x200, 0x44)));
        assert_eq!(reads.take_with_event(0x44), Some(read(4, 0x200, 0x44)));
        assert_eq!(reads.take(0x200), None);
        assert_eq!(reads.take_from(3), Some(read(3, 0x100, 0x40)));
        assert!(reads.is_empty());
    }

    #[test]
    fn a_new_read_for_the_same_overlapped_replaces_the_old_one() {
        let mut reads = PendingReads::new();
        assert!(reads.insert(read(3, 0x100, 0x40)));
        assert!(reads.insert(read(5, 0x100, 0x48)));
        assert_eq!(reads.take(0x100), Some(read(5, 0x100, 0x48)));
        assert!(reads.is_empty());
    }

    #[test]
    fn a_full_table_refuses_another_read() {
        let mut reads = PendingReads::new();
        for at in 0..CAPACITY {
            assert!(reads.insert(read(3, 0x100 + at, 0)));
        }
        assert!(!reads.insert(read(3, 0x900, 0)));
        assert_eq!(reads.take_with_event(0), None);
    }
}
