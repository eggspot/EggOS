//! The undo and redo history of a text field or a text area (docs/ux/toolkit-v0.6.md section
//! 5.1): steps of removed and inserted text, typing and runs of deletion grouped until a pause,
//! a move or another kind of edit, in a fixed budget of steps and bytes that drops the oldest.

/// The longest pause between two edits of one step.
pub const UNDO_GROUP_MS: u32 = 1000;
/// A text area's budget.
pub const TEXT_AREA_UNDO_STEPS: usize = 64;
pub const TEXT_AREA_UNDO_BYTES: usize = 8192;
/// The focused text field's budget, which holds any one edit of a 255-byte field.
pub const FIELD_UNDO_STEPS: usize = 16;
pub const FIELD_UNDO_BYTES: usize = 512;

/// A text area's history.
pub type AreaHistory = History<TEXT_AREA_UNDO_STEPS, TEXT_AREA_UNDO_BYTES>;
/// The focused text field's history.
pub type FieldHistory = History<FIELD_UNDO_STEPS, FIELD_UNDO_BYTES>;

/// What made an edit, which decides whether it joins the open step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    Typing,
    DeletingBackward,
    DeletingForward,
    Alone,
}

#[derive(Debug, Clone, Copy)]
struct Step {
    at: u32,
    removed: u16,
    inserted: u16,
    start: u16,
    caret: u32,
    anchor: u32,
    kind: EditKind,
    last_ms: u32,
    open: bool,
}

const NO_STEP: Step = Step {
    at: 0,
    removed: 0,
    inserted: 0,
    start: 0,
    caret: 0,
    anchor: 0,
    kind: EditKind::Alone,
    last_ms: 0,
    open: false,
};

/// What an undo or a redo does to the text: removes `remove` bytes at `at`, inserts `insert`
/// there, and puts the caret and the selection's anchor back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Change<'a> {
    pub at: usize,
    pub remove: usize,
    pub insert: &'a str,
    pub caret: usize,
    pub anchor: usize,
}

/// A history of at most `STEPS` steps holding at most `BYTES` bytes of text.
#[derive(Clone)]
pub struct History<const STEPS: usize, const BYTES: usize> {
    steps: [Step; STEPS],
    count: usize,
    done: usize,
    bytes: [u8; BYTES],
    used: usize,
    staged: usize,
    overflowed: bool,
    changes: u32,
}

impl<const STEPS: usize, const BYTES: usize> core::fmt::Debug for History<STEPS, BYTES> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("History")
            .field("count", &self.count)
            .field("done", &self.done)
            .field("used", &self.used)
            .finish()
    }
}

impl<const STEPS: usize, const BYTES: usize> Default for History<STEPS, BYTES> {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl<const STEPS: usize, const BYTES: usize> History<STEPS, BYTES> {
    /// Nothing to undo or redo.
    pub const EMPTY: Self = Self {
        steps: [NO_STEP; STEPS],
        count: 0,
        done: 0,
        bytes: [0; BYTES],
        used: 0,
        staged: 0,
        overflowed: false,
        changes: 0,
    };

    /// Forgets every step: the text is no longer what they describe.
    pub fn clear(&mut self) {
        self.count = 0;
        self.done = 0;
        self.used = 0;
        self.staged = 0;
        self.overflowed = false;
        self.changes = self.changes.wrapping_add(1);
    }

    /// Ends the open step, so the next edit starts a new one.
    pub fn close(&mut self) {
        if let Some(last) = self.count.checked_sub(1) {
            self.steps[last].open = false;
        }
    }

    /// Whether there is a step to undo.
    pub fn can_undo(&self) -> bool {
        self.done > 0
    }

    /// Whether there is a step to redo.
    pub fn can_redo(&self) -> bool {
        self.done < self.count
    }

    /// A count that moves on with every recorded edit, undo, redo and clearing: what tells an
    /// owner that a change of its text went unrecorded.
    pub fn changes(&self) -> u32 {
        self.changes
    }

    fn drop_redo(&mut self) {
        if self.done < self.count {
            self.used = usize::from(self.steps[self.done].start);
            self.count = self.done;
        }
    }

    fn drop_oldest(&mut self, pending: usize) {
        if self.count == 0 {
            return;
        }
        let oldest = self.steps[0];
        let length = usize::from(oldest.removed) + usize::from(oldest.inserted);
        self.bytes.copy_within(length..self.used + pending, 0);
        self.used -= length;
        self.steps.copy_within(1..self.count, 0);
        self.count -= 1;
        self.done = self.done.saturating_sub(1);
        for step in &mut self.steps[..self.count] {
            step.start -= length as u16;
        }
    }

    /// Keeps `removed`, the text an edit is about to take out, for [`Self::commit`]. It drops
    /// every step to redo.
    pub fn stage(&mut self, removed: &str) {
        self.drop_redo();
        self.staged = 0;
        self.overflowed = removed.len() > BYTES;
        if self.overflowed {
            return;
        }
        while self.used + removed.len() > BYTES && self.count > 0 {
            self.drop_oldest(0);
        }
        self.bytes[self.used..self.used + removed.len()].copy_from_slice(removed.as_bytes());
        self.staged = removed.len();
    }

    /// Records the edit at byte `at` that took out the text [`Self::stage`] kept and put in
    /// `inserted`, made with the caret at `caret` and the anchor at `anchor`, at `now_ms`: it
    /// joins the open step when it continues it, else it is a new step. An edit larger than the
    /// budget clears the history.
    pub fn commit(
        &mut self,
        at: usize,
        inserted: &str,
        caret: usize,
        anchor: usize,
        kind: EditKind,
        now_ms: u32,
    ) {
        let removed = core::mem::take(&mut self.staged);
        if removed == 0 && inserted.is_empty() && !self.overflowed {
            return;
        }
        if self.overflowed || removed + inserted.len() > BYTES {
            self.clear();
            return;
        }
        self.changes = self.changes.wrapping_add(1);
        while self.used + removed + inserted.len() > BYTES && self.count > 0 {
            self.drop_oldest(removed);
        }
        let end = self.used + removed;
        self.bytes[end..end + inserted.len()].copy_from_slice(inserted.as_bytes());
        let pending = removed + inserted.len();
        if self.join(at, removed, inserted.len(), caret, anchor, kind, now_ms) {
            self.used += pending;
            return;
        }
        self.close();
        if self.count == STEPS {
            self.drop_oldest(pending);
        }
        self.steps[self.count] = Step {
            at: at as u32,
            removed: removed as u16,
            inserted: inserted.len() as u16,
            start: self.used as u16,
            caret: caret as u32,
            anchor: anchor as u32,
            kind,
            last_ms: now_ms,
            open: kind != EditKind::Alone,
        };
        self.count += 1;
        self.done = self.count;
        self.used += pending;
    }

    #[allow(clippy::too_many_arguments)]
    fn join(
        &mut self,
        at: usize,
        removed: usize,
        inserted: usize,
        caret: usize,
        anchor: usize,
        kind: EditKind,
        now_ms: u32,
    ) -> bool {
        let Some(index) = self.count.checked_sub(1) else {
            return false;
        };
        let last = self.steps[index];
        let last_end = last.at as usize + usize::from(last.inserted);
        let continues = last.open
            && last.kind == kind
            && now_ms.wrapping_sub(last.last_ms) <= UNDO_GROUP_MS
            && caret == anchor
            && caret == last_end;
        let adjacent = match kind {
            EditKind::Typing => removed == 0 && at == last_end,
            EditKind::DeletingBackward => inserted == 0 && at + removed == last.at as usize,
            EditKind::DeletingForward => inserted == 0 && at == last.at as usize,
            EditKind::Alone => false,
        };
        if !(continues && adjacent) {
            return false;
        }
        let step = &mut self.steps[index];
        match kind {
            EditKind::DeletingBackward => {
                let start = usize::from(step.start);
                self.bytes[start..self.used + removed].rotate_right(removed);
                step.at = at as u32;
                step.removed += removed as u16;
            }
            EditKind::DeletingForward => step.removed += removed as u16,
            _ => step.inserted += inserted as u16,
        }
        step.last_ms = now_ms;
        true
    }

    fn text(&self, start: usize, length: usize) -> &str {
        core::str::from_utf8(&self.bytes[start..start + length]).unwrap_or("")
    }

    /// Takes the last step back: what to do to the text, or None with nothing to undo.
    pub fn undo(&mut self) -> Option<Change<'_>> {
        self.done = self.done.checked_sub(1)?;
        self.steps[self.done].open = false;
        self.changes = self.changes.wrapping_add(1);
        let step = self.steps[self.done];
        Some(Change {
            at: step.at as usize,
            remove: usize::from(step.inserted),
            insert: self.text(usize::from(step.start), usize::from(step.removed)),
            caret: step.caret as usize,
            anchor: step.anchor as usize,
        })
    }

    /// Does the last undone step again: what to do to the text, or None with nothing to redo.
    pub fn redo(&mut self) -> Option<Change<'_>> {
        if self.done == self.count {
            return None;
        }
        let step = self.steps[self.done];
        self.done += 1;
        self.changes = self.changes.wrapping_add(1);
        let after = step.at as usize + usize::from(step.inserted);
        Some(Change {
            at: step.at as usize,
            remove: usize::from(step.removed),
            insert: self.text(
                usize::from(step.start) + usize::from(step.removed),
                usize::from(step.inserted),
            ),
            caret: after,
            anchor: after,
        })
    }
}

/// Records the change of a short text from `before` to `after` (a text field's), found by
/// comparing them, the edit made with the caret at `caret` and the anchor at `anchor`.
pub fn record_difference<const STEPS: usize, const BYTES: usize>(
    history: &mut History<STEPS, BYTES>,
    before: &str,
    after: &str,
    caret: usize,
    anchor: usize,
    kind: EditKind,
    now_ms: u32,
) {
    let limit = caret.max(anchor).min(before.len()).min(after.len());
    let mut prefix = before
        .bytes()
        .zip(after.bytes())
        .take(limit)
        .take_while(|(old, new)| old == new)
        .count();
    while !before.is_char_boundary(prefix) || !after.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let room = (before.len() - prefix).min(after.len() - prefix);
    let mut suffix = before
        .bytes()
        .rev()
        .zip(after.bytes().rev())
        .take(room)
        .take_while(|(old, new)| old == new)
        .count();
    while !before.is_char_boundary(before.len() - suffix)
        || !after.is_char_boundary(after.len() - suffix)
    {
        suffix -= 1;
    }
    history.stage(&before[prefix..before.len() - suffix]);
    history.commit(
        prefix,
        &after[prefix..after.len() - suffix],
        caret,
        anchor,
        kind,
        now_ms,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(text: &mut String, change: Change<'_>) {
        text.replace_range(change.at..change.at + change.remove, change.insert);
    }

    fn typed(history: &mut FieldHistory, text: &mut String, character: char, now_ms: u32) {
        let caret = text.len();
        let before = text.clone();
        text.push(character);
        record_difference(history, &before, text, caret, caret, EditKind::Typing, now_ms);
    }

    #[test]
    fn typing_in_bursts_undoes_in_bursts_and_redoes() {
        let mut history = FieldHistory::EMPTY;
        let mut text = String::new();
        for (index, character) in "hello".chars().enumerate() {
            typed(&mut history, &mut text, character, index as u32 * 100);
        }
        for (index, character) in " you".chars().enumerate() {
            typed(&mut history, &mut text, character, 3000 + index as u32 * 100);
        }
        let change = history.undo().unwrap();
        apply(&mut text, change);
        assert_eq!(text, "hello");
        let change = history.undo().unwrap();
        apply(&mut text, change);
        assert_eq!(text, "");
        assert!(history.undo().is_none());
        let change = history.redo().unwrap();
        apply(&mut text, change);
        assert_eq!(text, "hello");
        assert_eq!(change.caret, 5);
    }

    #[test]
    fn a_run_of_backspaces_is_one_step_and_a_new_edit_drops_the_redo() {
        let mut history = FieldHistory::EMPTY;
        let mut text = String::from("abcdef");
        for at in (3..6).rev() {
            let before = text.clone();
            text.remove(at);
            record_difference(
                &mut history,
                &before,
                &text,
                at + 1,
                at + 1,
                EditKind::DeletingBackward,
                at as u32,
            );
        }
        assert_eq!(text, "abc");
        let change = history.undo().unwrap();
        assert_eq!(change.insert, "def");
        apply(&mut text, change);
        assert_eq!(text, "abcdef");
        typed(&mut history, &mut text, 'g', 10);
        assert!(!history.can_redo());
    }

    #[test]
    fn the_budget_drops_the_oldest_and_an_edit_too_large_clears() {
        let mut history = History::<2, 8>::EMPTY;
        history.stage("");
        history.commit(0, "abc", 0, 0, EditKind::Alone, 0);
        history.stage("");
        history.commit(3, "def", 3, 3, EditKind::Alone, 0);
        history.stage("");
        history.commit(6, "gh", 6, 6, EditKind::Alone, 0);
        assert_eq!(history.undo().unwrap().remove, 2);
        assert_eq!(history.undo().unwrap().remove, 3);
        assert!(history.undo().is_none());
        history.stage("");
        history.commit(0, "123456789", 0, 0, EditKind::Alone, 0);
        assert!(!history.can_undo() && !history.can_redo());
    }
}
