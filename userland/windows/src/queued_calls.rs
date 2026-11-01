//! Calls queued for a thread's next alertable wait (D189): the completion routines of
//! `ReadFileEx` and `WriteFileEx` and the APC routines of `NtReadFile` and `NtWriteFile`, which
//! Windows runs on the thread that started the I/O only when it waits alertably (`SleepEx`,
//! `WaitForSingleObjectEx`, `WaitForMultipleObjectsEx`), in the order they were queued.

/// How many calls can wait at once, for every thread together.
pub const CAPACITY: usize = 64;

/// One routine of the program to call with three arguments on thread `thread`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueuedCall {
    pub thread: u32,
    pub routine: usize,
    pub arguments: [usize; 3],
}

const EMPTY: QueuedCall = QueuedCall {
    thread: 0,
    routine: 0,
    arguments: [0; 3],
};

/// The queued calls of every thread, oldest first.
#[derive(Debug)]
pub struct QueuedCalls {
    calls: [QueuedCall; CAPACITY],
    length: usize,
}

impl QueuedCalls {
    /// No calls.
    pub const fn new() -> Self {
        Self {
            calls: [EMPTY; CAPACITY],
            length: 0,
        }
    }

    /// Whether no call waits.
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// Queues `call` after the others; `false` when the queue is full.
    pub fn push(&mut self, call: QueuedCall) -> bool {
        if self.length == CAPACITY {
            return false;
        }
        self.calls[self.length] = call;
        self.length += 1;
        true
    }

    /// Takes the oldest call queued for `thread`.
    pub fn take_for(&mut self, thread: u32) -> Option<QueuedCall> {
        let at = self.calls[..self.length]
            .iter()
            .position(|call| call.thread == thread)?;
        let call = self.calls[at];
        self.calls.copy_within(at + 1..self.length, at);
        self.length -= 1;
        Some(call)
    }
}

impl Default for QueuedCalls {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(thread: u32, routine: usize) -> QueuedCall {
        QueuedCall {
            thread,
            routine,
            arguments: [0, routine * 10, 0],
        }
    }

    #[test]
    fn each_thread_takes_its_own_calls_oldest_first() {
        let mut calls = QueuedCalls::new();
        assert!(calls.push(call(1, 1)));
        assert!(calls.push(call(2, 2)));
        assert!(calls.push(call(1, 3)));
        assert_eq!(calls.take_for(1), Some(call(1, 1)));
        assert_eq!(calls.take_for(1), Some(call(1, 3)));
        assert_eq!(calls.take_for(1), None);
        assert!(!calls.is_empty());
        assert_eq!(calls.take_for(2), Some(call(2, 2)));
        assert!(calls.is_empty());
    }

    #[test]
    fn a_full_queue_refuses_more() {
        let mut calls = QueuedCalls::new();
        for routine in 0..CAPACITY {
            assert!(calls.push(call(7, routine)));
        }
        assert!(!calls.push(call(7, CAPACITY)));
        assert_eq!(calls.take_for(7), Some(call(7, 0)));
        assert!(calls.push(call(7, CAPACITY)));
    }
}
