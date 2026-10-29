//! The editing itself (docs/ux/text-editor-v0.3.md sections 5, 10 and 11): a document, its
//! history, the caret and the anchor (the selection is between them), the column Up and Down
//! keep, the editor's own clipboard, and whether the text is the one last saved or loaded.

use crate::columns::{byte_at_column, character_column, view_column};
use crate::document::{Document, Full, LINE_CAPACITY, TEXT_CAPACITY};
use crate::history::{History, Kind};
use crate::words::{next_word_start, previous_word_start, run_at};

/// The history log's room: twice the text's, so any single edit can be undone.
pub const HISTORY_CAPACITY: usize = 2 * TEXT_CAPACITY + 64 * 1024;
/// The clipboard's room: a whole document.
pub const CLIPBOARD_CAPACITY: usize = TEXT_CAPACITY;

/// The memory an editor works in, which the program maps and the host tests allocate: the
/// text, the line index, the history log and the clipboard.
#[derive(Debug)]
pub struct Memory<'a> {
    pub text: &'a mut [u8],
    pub starts: &'a mut [u32],
    pub history: &'a mut [u8],
    pub clipboard: &'a mut [u8],
}

/// The sizes [`Memory`]'s parts have in the program.
pub const MEMORY_SIZES: (usize, usize, usize, usize) = (
    TEXT_CAPACITY,
    LINE_CAPACITY,
    HISTORY_CAPACITY,
    CLIPBOARD_CAPACITY,
);

/// Where the caret goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    LineStart,
    LineEnd,
    WordLeft,
    WordRight,
    DocumentStart,
    DocumentEnd,
    /// Up by this many lines.
    PageUp(usize),
    /// Down by this many lines.
    PageDown(usize),
}

/// The editor. See the module documentation.
#[derive(Debug)]
pub struct Editor<'a> {
    document: Document<'a>,
    history: History<'a>,
    clipboard: &'a mut [u8],
    clipboard_length: usize,
    caret: usize,
    anchor: usize,
    wanted: Option<usize>,
    saved: u32,
}

fn replace(
    document: &mut Document<'_>,
    history: &mut History<'_>,
    (start, end): (usize, usize),
    bytes: &[u8],
    (caret, anchor): (usize, usize),
    kind: Kind,
) -> Result<usize, Full> {
    if !document.fits(&document.bytes()[start..end], bytes) {
        return Err(Full);
    }
    history.record(
        start,
        &document.bytes()[start..end],
        bytes,
        (caret, anchor),
        kind,
    );
    document.remove(start, end);
    document.insert(start, bytes)?;
    Ok(start + bytes.len())
}

impl<'a> Editor<'a> {
    /// An editor of an empty document in `memory`.
    pub fn new(memory: Memory<'a>) -> Self {
        let mut editor = Self {
            document: Document::new(memory.text, memory.starts),
            history: History::new(memory.history),
            clipboard: memory.clipboard,
            clipboard_length: 0,
            caret: 0,
            anchor: 0,
            wanted: None,
            saved: 0,
        };
        editor.history.clear();
        editor.saved = editor.history.current();
        editor
    }

    /// The document.
    pub fn document(&self) -> &Document<'a> {
        &self.document
    }

    /// The document, to load a file into; [`Self::loaded`] must follow.
    pub fn document_mut(&mut self) -> &mut Document<'a> {
        &mut self.document
    }

    /// The document was replaced (a new one, or a file read into it): the caret goes to the
    /// start, the history is forgotten and the text counts as saved.
    pub fn loaded(&mut self) {
        self.caret = 0;
        self.anchor = 0;
        self.wanted = None;
        self.history.clear();
        self.saved = self.history.current();
    }

    /// Empties the document, as New does.
    pub fn clear(&mut self) {
        self.document.clear();
        self.loaded();
    }

    /// The text as it is now was saved.
    pub fn saved(&mut self) {
        self.history.close();
        self.saved = self.history.current();
    }

    /// Whether the text differs from the one last saved or loaded.
    pub fn is_modified(&self) -> bool {
        self.history.current() != self.saved
    }

    /// The caret's byte.
    pub fn caret(&self) -> usize {
        self.caret
    }

    /// The anchor's byte: where the selection starts when it is not empty.
    pub fn anchor(&self) -> usize {
        self.anchor
    }

    /// The selection, start first; empty when the anchor is at the caret.
    pub fn selection(&self) -> (usize, usize) {
        (self.caret.min(self.anchor), self.caret.max(self.anchor))
    }

    /// Whether some text is selected.
    pub fn has_selection(&self) -> bool {
        self.caret != self.anchor
    }

    /// The selected bytes.
    pub fn selected(&self) -> &[u8] {
        let (start, end) = self.selection();
        &self.document.bytes()[start..end]
    }

    /// How many characters are selected, line breaks counted.
    pub fn selected_characters(&self) -> usize {
        character_column(self.selected(), usize::MAX)
    }

    /// The caret's line, 0-based.
    pub fn caret_line(&self) -> usize {
        self.document.line_of(self.caret)
    }

    /// The caret's place as the status bar says it: line and column, both 1-based, the
    /// column in characters.
    pub fn line_and_column(&self) -> (usize, usize) {
        let line = self.caret_line();
        let start = self.document.line_start(line);
        let column = character_column(&self.document.bytes()[start..], self.caret - start);
        (line + 1, column + 1)
    }

    /// The caret's column in the text view.
    pub fn view_column(&self) -> usize {
        let line = self.caret_line();
        let start = self.document.line_start(line);
        view_column(self.document.line(line), self.caret - start)
    }

    /// Whether there is something to undo.
    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    /// Whether there is something to redo.
    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// Whether the clipboard holds text.
    pub fn can_paste(&self) -> bool {
        self.clipboard_length > 0
    }

    /// Puts the caret at `caret` and the anchor at `anchor`, both clamped to the text and
    /// moved back to character boundaries; the step being typed ends.
    pub fn select(&mut self, anchor: usize, caret: usize) {
        let boundary = |at: usize| {
            let mut at = at.min(self.document.len());
            while at > 0
                && at < self.document.len()
                && crate::document::is_continuation(self.document.bytes()[at])
            {
                at -= 1;
            }
            at
        };
        let (anchor, caret) = (boundary(anchor), boundary(caret));
        self.anchor = anchor;
        self.caret = caret;
        self.wanted = None;
        self.history.close();
    }

    /// Puts the caret at `at` with nothing selected.
    pub fn place(&mut self, at: usize) {
        self.select(at, at);
    }

    /// Selects everything, the anchor at the start and the caret at the end.
    pub fn select_all(&mut self) {
        self.select(0, self.document.len());
    }

    /// Selects the word, mark or run of spaces at byte `at`.
    pub fn select_run(&mut self, at: usize) {
        let (start, end) = run_at(self.document.bytes(), at);
        self.select(start, end);
    }

    /// Selects lines `first` to `last` (either order), the last one's line break included.
    pub fn select_lines(&mut self, first: usize, last: usize) {
        let (top, bottom) = (first.min(last), first.max(last));
        let end = match bottom + 1 < self.document.line_count() {
            true => self.document.line_start(bottom + 1),
            false => self.document.len(),
        };
        let start = self.document.line_start(top);
        match last < first {
            true => self.select(end, start),
            false => self.select(start, end),
        }
    }

    fn line_target(&self, line: usize, column: usize) -> usize {
        self.document.line_start(line) + byte_at_column(self.document.line(line), column)
    }

    /// Moves the caret by `motion`; with `extend` the anchor stays, else the selection goes
    /// (a plain Left or Right with a selection lands on its edge).
    pub fn go(&mut self, motion: Motion, extend: bool) {
        let (start, end) = self.selection();
        let line = self.caret_line();
        let last = self.document.line_count() - 1;
        let wanted = self.wanted.unwrap_or_else(|| self.view_column());
        let text = self.document.bytes();
        let (target, keeps_column) = match motion {
            Motion::Left if !extend && start != end => (start, false),
            Motion::Right if !extend && start != end => (end, false),
            Motion::Left => (self.document.previous_boundary(self.caret), false),
            Motion::Right => (self.document.next_boundary(self.caret), false),
            Motion::Up if line == 0 => (0, false),
            Motion::Up => (self.line_target(line - 1, wanted), true),
            Motion::Down if line == last => (self.document.len(), false),
            Motion::Down => (self.line_target(line + 1, wanted), true),
            Motion::PageUp(lines) => (self.line_target(line.saturating_sub(lines), wanted), true),
            Motion::PageDown(lines) => (self.line_target((line + lines).min(last), wanted), true),
            Motion::LineStart => (self.document.line_start(line), false),
            Motion::LineEnd => (self.document.line_end(line), false),
            Motion::WordLeft => (previous_word_start(text, self.caret), false),
            Motion::WordRight => (next_word_start(text, self.caret), false),
            Motion::DocumentStart => (0, false),
            Motion::DocumentEnd => (self.document.len(), false),
        };
        let anchor = match extend {
            true => self.anchor,
            false => target,
        };
        self.select(anchor, target);
        if keeps_column {
            self.wanted = Some(wanted);
        }
    }

    fn edit(&mut self, range: (usize, usize), bytes: &[u8], kind: Kind) -> Result<(), Full> {
        let caret = replace(
            &mut self.document,
            &mut self.history,
            range,
            bytes,
            (self.caret, self.anchor),
            kind,
        )?;
        self.caret = caret;
        self.anchor = caret;
        self.wanted = None;
        Ok(())
    }

    /// Types `character` in place of the selection.
    pub fn type_character(&mut self, character: char) -> Result<(), Full> {
        let mut bytes = [0u8; 4];
        let typed = character.encode_utf8(&mut bytes).as_bytes();
        let kind = match character {
            '\n' | '\t' => Kind::Other,
            _ => Kind::Typing,
        };
        self.edit(self.selection(), typed, kind)
    }

    /// Backspace: takes out the selection, else the character before the caret.
    pub fn backspace(&mut self) {
        let range = match self.has_selection() {
            true => self.selection(),
            false => (self.document.previous_boundary(self.caret), self.caret),
        };
        let kind = match self.has_selection() {
            true => Kind::Other,
            false => Kind::Backspace,
        };
        if range.0 < range.1 {
            let _ = self.edit(range, b"", kind);
        }
    }

    /// Delete: takes out the selection, else the character after the caret.
    pub fn delete(&mut self) {
        let range = match self.has_selection() {
            true => self.selection(),
            false => (self.caret, self.document.next_boundary(self.caret)),
        };
        let kind = match self.has_selection() {
            true => Kind::Other,
            false => Kind::Delete,
        };
        if range.0 < range.1 {
            let _ = self.edit(range, b"", kind);
        }
    }

    /// Copies the selection to the clipboard; nothing without a selection.
    pub fn copy(&mut self) {
        let (start, end) = self.selection();
        if start == end || end - start > self.clipboard.len() {
            return;
        }
        self.clipboard[..end - start].copy_from_slice(&self.document.bytes()[start..end]);
        self.clipboard_length = end - start;
    }

    /// Cut: copies the selection, then takes it out.
    pub fn cut(&mut self) {
        if self.has_selection() {
            self.copy();
            let _ = self.edit(self.selection(), b"", Kind::Other);
        }
    }

    /// Paste: the clipboard's text in place of the selection.
    pub fn paste(&mut self) -> Result<(), Full> {
        if self.clipboard_length == 0 {
            return Ok(());
        }
        let range = self.selection();
        let caret = replace(
            &mut self.document,
            &mut self.history,
            range,
            &self.clipboard[..self.clipboard_length],
            (self.caret, self.anchor),
            Kind::Other,
        )?;
        self.caret = caret;
        self.anchor = caret;
        self.wanted = None;
        Ok(())
    }

    /// Takes the last step back; the caret and selection go back to what they were before it.
    pub fn undo(&mut self) {
        let Some(replay) = self.history.undo() else {
            return;
        };
        let step = replay.step;
        self.document.remove(step.at, step.at + step.inserted);
        let _ = self.document.insert(step.at, replay.removed);
        self.caret = step.caret;
        self.anchor = step.anchor;
        self.wanted = None;
    }

    /// Does the next undone step again; the caret goes after what it put in.
    pub fn redo(&mut self) {
        let Some(replay) = self.history.redo() else {
            return;
        };
        let step = replay.step;
        self.document.remove(step.at, step.at + step.removed);
        let _ = self.document.insert(step.at, replay.inserted);
        self.caret = step.at + step.inserted;
        self.anchor = self.caret;
        self.wanted = None;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) struct Owned {
        text: Vec<u8>,
        starts: Vec<u32>,
        history: Vec<u8>,
        clipboard: Vec<u8>,
    }

    impl Owned {
        pub(crate) fn new(bytes: usize) -> Self {
            Self {
                text: vec![0; bytes],
                starts: vec![0; bytes + 1],
                history: vec![0; 2 * bytes + 1024],
                clipboard: vec![0; bytes],
            }
        }

        pub(crate) fn editor(&mut self) -> Editor<'_> {
            Editor::new(Memory {
                text: &mut self.text,
                starts: &mut self.starts,
                history: &mut self.history,
                clipboard: &mut self.clipboard,
            })
        }
    }

    fn typed(editor: &mut Editor<'_>, text: &str) {
        for character in text.chars() {
            editor.type_character(character).unwrap();
        }
    }

    fn text(editor: &Editor<'_>) -> String {
        String::from_utf8(editor.document().bytes().to_vec()).unwrap()
    }

    #[test]
    fn typing_is_one_undo_step_and_enter_ends_it() {
        let mut owned = Owned::new(256);
        let mut editor = owned.editor();
        typed(&mut editor, "hello\nworld");
        assert!(editor.is_modified());
        assert_eq!(editor.line_and_column(), (2, 6));
        editor.undo();
        assert_eq!(text(&editor), "hello\n");
        editor.undo();
        assert_eq!(text(&editor), "hello");
        editor.undo();
        assert_eq!(text(&editor), "");
        assert!(!editor.is_modified());
        editor.redo();
        editor.redo();
        assert_eq!(text(&editor), "hello\n");
        assert_eq!(editor.caret(), 6);
        typed(&mut editor, "x");
        assert!(!editor.can_redo());
    }

    #[test]
    fn saving_marks_the_text_unmodified_until_it_changes() {
        let mut owned = Owned::new(64);
        let mut editor = owned.editor();
        typed(&mut editor, "ab");
        editor.saved();
        assert!(!editor.is_modified());
        typed(&mut editor, "c");
        assert!(editor.is_modified());
        editor.undo();
        assert!(!editor.is_modified());
        editor.undo();
        assert!(editor.is_modified());
    }

    #[test]
    fn selections_are_replaced_and_cut_copy_paste_use_the_editor_s_clipboard() {
        let mut owned = Owned::new(64);
        let mut editor = owned.editor();
        typed(&mut editor, "one two");
        assert!(!editor.can_paste());
        editor.select(0, 3);
        editor.copy();
        assert!(editor.can_paste());
        editor.go(Motion::DocumentEnd, false);
        editor.paste().unwrap();
        assert_eq!(text(&editor), "one twoone");
        editor.select(4, 7);
        editor.cut();
        assert_eq!(text(&editor), "one one");
        editor.select(0, 3);
        typed(&mut editor, "two");
        assert_eq!(text(&editor), "two one");
        editor.undo();
        assert_eq!(text(&editor), "one one");
        assert_eq!(editor.selection(), (0, 3));
        editor.select_all();
        assert_eq!(editor.selected_characters(), 7);
        editor.backspace();
        assert_eq!(text(&editor), "");
    }

    #[test]
    fn the_caret_moves_by_characters_lines_words_and_pages() {
        let mut owned = Owned::new(128);
        let mut editor = owned.editor();
        typed(&mut editor, "long line\nab\n\tx\nlast");
        editor.go(Motion::DocumentStart, false);
        editor.go(Motion::LineEnd, false);
        assert_eq!(editor.caret(), 9);
        editor.go(Motion::Down, false);
        assert_eq!(editor.caret(), 12);
        editor.go(Motion::Down, false);
        assert_eq!(editor.caret(), 15);
        editor.go(Motion::Down, false);
        assert_eq!(editor.caret(), 20);
        editor.go(Motion::Up, false);
        editor.go(Motion::Up, false);
        editor.go(Motion::Up, false);
        assert_eq!(editor.caret(), 9);
        editor.go(Motion::WordLeft, true);
        assert_eq!(editor.selection(), (5, 9));
        editor.go(Motion::Right, false);
        assert_eq!(editor.caret(), 9);
        editor.go(Motion::PageDown(10), false);
        assert_eq!(editor.caret_line(), 3);
        editor.go(Motion::PageUp(2), false);
        assert_eq!(editor.caret_line(), 1);
        editor.go(Motion::Up, false);
        editor.go(Motion::Up, false);
        assert_eq!(editor.caret(), 0);
        editor.go(Motion::DocumentEnd, true);
        assert_eq!(editor.selection(), (0, 20));
        editor.select_lines(1, 2);
        assert_eq!(editor.selected(), b"ab\n\tx\n");
        editor.select_lines(3, 3);
        assert_eq!(editor.selected(), b"last");
    }

    #[test]
    fn backspace_and_delete_take_whole_characters() {
        let mut owned = Owned::new(64);
        let mut editor = owned.editor();
        typed(&mut editor, "aé€");
        editor.backspace();
        assert_eq!(text(&editor), "aé");
        editor.go(Motion::DocumentStart, false);
        editor.delete();
        assert_eq!(text(&editor), "é");
        editor.delete();
        assert_eq!(text(&editor), "");
        editor.undo();
        assert_eq!(text(&editor), "aé");
        editor.select(2, 2);
        assert_eq!(editor.caret(), 1);
    }

    #[test]
    fn an_edit_past_the_room_is_refused() {
        let mut owned = Owned::new(4);
        let mut editor = owned.editor();
        typed(&mut editor, "abcd");
        assert_eq!(editor.type_character('e'), Err(Full));
        assert_eq!(text(&editor), "abcd");
    }
}
