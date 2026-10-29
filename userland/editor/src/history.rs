//! Undo and redo (docs/ux/text-editor-v0.3.md section 5): every edit is a step that took out
//! some bytes at a place and put others there, with the caret and anchor of before. The steps
//! live one after another in one byte log the caller gives: a header, the bytes taken out, the
//! bytes put in and the record's length, so the log is walked both ways. Typing, a run of
//! Backspace and a run of Delete grow the last step until something else happens. At most
//! [`MOST_STEPS`] steps are kept, fewer when the log is full; the oldest go first. Each state of
//! the text has a serial number, so the editor knows whether it is the saved one.

/// The most steps kept.
pub const MOST_STEPS: usize = 1024;
const HEADER: usize = 32;
const TRAILER: usize = 4;

/// What kind of edit a step is, which says what may grow it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Anything that ends the step at once: Enter, Tab, paste, cut, undoing.
    Other,
    /// Typed characters.
    Typing,
    /// Characters taken out by Backspace.
    Backspace,
    /// Characters taken out by Delete.
    Delete,
}

impl Kind {
    fn byte(self) -> u8 {
        match self {
            Self::Other => 0,
            Self::Typing => 1,
            Self::Backspace => 2,
            Self::Delete => 3,
        }
    }

    fn from_byte(byte: u8) -> Self {
        match byte {
            1 => Self::Typing,
            2 => Self::Backspace,
            3 => Self::Delete,
            _ => Self::Other,
        }
    }
}

/// A step: at `at`, `removed` bytes were taken out and `inserted` put in, when the caret and
/// anchor were `caret` and `anchor`; the text's serial was `before` and became `after`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub at: usize,
    pub removed: usize,
    pub inserted: usize,
    pub caret: usize,
    pub anchor: usize,
    pub before: u32,
    pub after: u32,
    pub kind: Kind,
}

impl Step {
    fn length(&self) -> usize {
        HEADER + self.removed + self.inserted + TRAILER
    }
}

fn word(log: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([log[at], log[at + 1], log[at + 2], log[at + 3]])
}

fn put(log: &mut [u8], at: usize, value: u32) {
    log[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

/// A step taken back or done again: the step and the bytes it took out and put in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Replay<'a> {
    pub step: Step,
    pub removed: &'a [u8],
    pub inserted: &'a [u8],
}

/// The history. See the module documentation.
pub struct History<'a> {
    log: &'a mut [u8],
    done: usize,
    used: usize,
    done_steps: usize,
    steps: usize,
    open: bool,
    base: u32,
    next_serial: u32,
}

impl core::fmt::Debug for History<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("History")
            .field("done_steps", &self.done_steps)
            .field("steps", &self.steps)
            .finish_non_exhaustive()
    }
}

impl<'a> History<'a> {
    /// An empty history in `log`.
    pub fn new(log: &'a mut [u8]) -> Self {
        Self {
            log,
            done: 0,
            used: 0,
            done_steps: 0,
            steps: 0,
            open: false,
            base: 0,
            next_serial: 1,
        }
    }

    fn serial(&mut self) -> u32 {
        let serial = self.next_serial;
        self.next_serial = self.next_serial.wrapping_add(1).max(1);
        serial
    }

    /// Forgets every step: the text as it is now is a new state.
    pub fn clear(&mut self) {
        self.done = 0;
        self.used = 0;
        self.done_steps = 0;
        self.steps = 0;
        self.open = false;
        self.base = self.serial();
    }

    /// The serial of the text as it is now.
    pub fn current(&self) -> u32 {
        match self.done {
            0 => self.base,
            done => self.step_before(done).after,
        }
    }

    fn step_at(&self, start: usize) -> Step {
        let log = &*self.log;
        Step {
            at: word(log, start) as usize,
            removed: word(log, start + 4) as usize,
            inserted: word(log, start + 8) as usize,
            caret: word(log, start + 12) as usize,
            anchor: word(log, start + 16) as usize,
            before: word(log, start + 20),
            after: word(log, start + 24),
            kind: Kind::from_byte(log[start + 28]),
        }
    }

    fn start_before(&self, end: usize) -> usize {
        end - word(&self.log[..], end - TRAILER) as usize
    }

    fn step_before(&self, end: usize) -> Step {
        self.step_at(self.start_before(end))
    }

    fn write_step(&mut self, start: usize, step: &Step) {
        let log = &mut *self.log;
        put(log, start, step.at as u32);
        put(log, start + 4, step.removed as u32);
        put(log, start + 8, step.inserted as u32);
        put(log, start + 12, step.caret as u32);
        put(log, start + 16, step.anchor as u32);
        put(log, start + 20, step.before);
        put(log, start + 24, step.after);
        log[start + 28..start + HEADER].copy_from_slice(&[step.kind.byte(), 0, 0, 0]);
        let end = start + step.length();
        put(log, end - TRAILER, step.length() as u32);
    }

    /// Whether there is a step to undo.
    pub fn can_undo(&self) -> bool {
        self.done > 0
    }

    /// Whether there is a step to redo.
    pub fn can_redo(&self) -> bool {
        self.done < self.used
    }

    /// Ends the step being grown: the next edit is a step of its own.
    pub fn close(&mut self) {
        self.open = false;
    }

    fn drop_oldest(&mut self) {
        let length = self.step_at(0).length();
        self.base = self.step_at(0).after;
        self.log.copy_within(length..self.used, 0);
        self.used -= length;
        self.done -= length.min(self.done);
        self.steps -= 1;
        self.done_steps = self.done_steps.saturating_sub(1);
    }

    fn grow(&mut self, at: usize, removed: &[u8], inserted: &[u8], kind: Kind) -> bool {
        if !self.open || self.done == 0 || self.done != self.used {
            return false;
        }
        let start = self.start_before(self.done);
        let mut step = self.step_at(start);
        let added = removed.len() + inserted.len();
        if step.kind != kind || self.used + added > self.log.len() {
            return false;
        }
        let payload = start + HEADER;
        match kind {
            Kind::Typing if removed.is_empty() && at == step.at + step.inserted => {
                let end = payload + step.removed + step.inserted;
                self.log[end..end + inserted.len()].copy_from_slice(inserted);
                step.inserted += inserted.len();
            }
            Kind::Backspace
                if inserted.is_empty() && step.inserted == 0 && at + removed.len() == step.at =>
            {
                self.log
                    .copy_within(payload..payload + step.removed, payload + removed.len());
                self.log[payload..payload + removed.len()].copy_from_slice(removed);
                step.removed += removed.len();
                step.at = at;
            }
            Kind::Delete if inserted.is_empty() && step.inserted == 0 && at == step.at => {
                let end = payload + step.removed;
                self.log[end..end + removed.len()].copy_from_slice(removed);
                step.removed += removed.len();
            }
            _ => return false,
        }
        step.after = self.serial();
        self.write_step(start, &step);
        self.done = start + step.length();
        self.used = self.done;
        true
    }

    /// Records an edit that took `removed` out at `at` and put `inserted` there, the caret and
    /// anchor having been `caret` and `anchor`; a [`Kind`] other than [`Kind::Other`] may grow
    /// the last step instead. Steps that were undone are forgotten.
    pub fn record(
        &mut self,
        at: usize,
        removed: &[u8],
        inserted: &[u8],
        (caret, anchor): (usize, usize),
        kind: Kind,
    ) {
        if self.grow(at, removed, inserted, kind) {
            return;
        }
        self.used = self.done;
        self.steps = self.done_steps;
        let before = self.current();
        let step = Step {
            at,
            removed: removed.len(),
            inserted: inserted.len(),
            caret,
            anchor,
            before,
            after: self.serial(),
            kind,
        };
        let length = step.length();
        if length > self.log.len() {
            self.done = 0;
            self.used = 0;
            self.done_steps = 0;
            self.steps = 0;
            self.base = step.after;
            self.open = false;
            return;
        }
        while self.steps > 0 && (self.used + length > self.log.len() || self.steps >= MOST_STEPS)
        {
            self.drop_oldest();
        }
        let start = self.used;
        let payload = start + HEADER;
        self.log[payload..payload + removed.len()].copy_from_slice(removed);
        self.log[payload + removed.len()..payload + removed.len() + inserted.len()]
            .copy_from_slice(inserted);
        self.write_step(start, &step);
        self.used = start + length;
        self.done = self.used;
        self.steps += 1;
        self.done_steps = self.steps;
        self.open = kind != Kind::Other;
    }

    /// Takes the last done step back: the caller puts `removed` back in place of `inserted`.
    pub fn undo(&mut self) -> Option<Replay<'_>> {
        if self.done == 0 {
            return None;
        }
        let start = self.start_before(self.done);
        let step = self.step_at(start);
        self.done = start;
        self.done_steps -= 1;
        self.open = false;
        let payload = start + HEADER;
        Some(Replay {
            step,
            removed: &self.log[payload..payload + step.removed],
            inserted: &self.log[payload + step.removed..payload + step.removed + step.inserted],
        })
    }

    /// Does the next undone step again: the caller puts `inserted` in place of `removed`.
    pub fn redo(&mut self) -> Option<Replay<'_>> {
        if self.done >= self.used {
            return None;
        }
        let start = self.done;
        let step = self.step_at(start);
        self.done = start + step.length();
        self.done_steps += 1;
        self.open = false;
        let payload = start + HEADER;
        Some(Replay {
            step,
            removed: &self.log[payload..payload + step.removed],
            inserted: &self.log[payload + step.removed..payload + step.removed + step.inserted],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replayed(replay: Option<Replay<'_>>) -> (usize, Vec<u8>, Vec<u8>) {
        let replay = replay.unwrap();
        (
            replay.step.at,
            replay.removed.to_vec(),
            replay.inserted.to_vec(),
        )
    }

    #[test]
    fn typing_grows_one_step_until_something_else_happens() {
        let mut log = vec![0u8; 1024];
        let mut history = History::new(&mut log);
        history.clear();
        let saved = history.current();
        history.record(0, b"", b"a", (0, 0), Kind::Typing);
        history.record(1, b"", b"b", (1, 1), Kind::Typing);
        assert_ne!(history.current(), saved);
        history.record(2, b"", b"\n", (2, 2), Kind::Other);
        history.record(3, b"", b"c", (3, 3), Kind::Typing);
        history.close();
        history.record(4, b"", b"d", (4, 4), Kind::Typing);
        assert_eq!(replayed(history.undo()), (4, vec![], b"d".to_vec()));
        assert_eq!(replayed(history.undo()), (3, vec![], b"c".to_vec()));
        assert_eq!(replayed(history.undo()), (2, vec![], b"\n".to_vec()));
        let step = history.undo().unwrap().step;
        assert_eq!((step.at, step.inserted, step.caret), (0, 2, 0));
        assert_eq!(history.current(), saved);
        assert!(history.undo().is_none());
        assert_eq!(replayed(history.redo()), (0, vec![], b"ab".to_vec()));
        assert!(history.can_redo());
        history.record(2, b"", b"x", (2, 2), Kind::Typing);
        assert!(!history.can_redo());
        assert!(history.redo().is_none());
    }

    #[test]
    fn backspace_and_delete_runs_grow_their_steps() {
        let mut log = vec![0u8; 1024];
        let mut history = History::new(&mut log);
        history.record(4, b"d", b"", (5, 5), Kind::Backspace);
        history.record(3, b"c", b"", (4, 4), Kind::Backspace);
        history.record(1, b"a", b"", (1, 1), Kind::Delete);
        history.record(1, b"b", b"", (1, 1), Kind::Delete);
        assert_eq!(replayed(history.undo()), (1, b"ab".to_vec(), vec![]));
        let replay = history.undo().unwrap();
        assert_eq!((replay.step.at, replay.removed), (3, &b"cd"[..]));
        assert_eq!(replay.step.caret, 5);
    }

    #[test]
    fn the_oldest_steps_go_when_the_log_or_the_count_is_full() {
        let mut log = vec![0u8; 3 * (HEADER + TRAILER + 1)];
        let mut history = History::new(&mut log);
        for at in 0..5 {
            history.record(at, b"", b"x", (at, at), Kind::Other);
        }
        let mut undone = 0;
        while history.undo().is_some() {
            undone += 1;
        }
        assert_eq!(undone, 3);
        let mut log = vec![0u8; 64 * 1024];
        let mut history = History::new(&mut log);
        for at in 0..MOST_STEPS + 10 {
            history.record(at, b"", b"x", (at, at), Kind::Other);
        }
        let mut undone = 0;
        while history.undo().is_some() {
            undone += 1;
        }
        assert_eq!(undone, MOST_STEPS);
        let mut small = vec![0u8; 8];
        let mut history = History::new(&mut small);
        history.record(0, b"", b"too long for the log", (0, 0), Kind::Other);
        assert!(!history.can_undo());
    }
}
