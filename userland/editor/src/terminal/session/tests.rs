use super::*;
use crate::editor::tests::Owned;
use eggos_files::memory::MemoryFiles;

const COLOUR: Style = Style {
    colour: true,
    one_cell: false,
};

fn disk() -> MemoryFiles {
    let mut files = MemoryFiles::new();
    files
        .folder("/home")
        .folder("/home/root")
        .file("/home/root/main.rs", b"fn main() {\n    let name = \"world\";\n}\n", 5)
        .file("/home/root/notes.txt", b"one egg\ntwo eggs\nEGG three\n", 5);
    files
}

struct Screen {
    cells: Vec<Vec<char>>,
    row: usize,
    column: usize,
}

impl Screen {
    fn new(rows: usize, columns: usize) -> Self {
        Self {
            cells: vec![vec![' '; columns]; rows],
            row: 0,
            column: 0,
        }
    }

    fn apply(&mut self, bytes: &[u8]) {
        let text = core::str::from_utf8(bytes).unwrap();
        let mut characters = text.chars().peekable();
        while let Some(character) = characters.next() {
            if character != '\x1b' {
                if let Some(cell) = self.cells.get_mut(self.row).and_then(|row| row.get_mut(self.column)) {
                    *cell = character;
                }
                self.column += 1;
                continue;
            }
            assert_eq!(characters.next(), Some('['));
            let mut parameters = String::new();
            let final_byte = loop {
                let next = characters.next().unwrap();
                if next.is_ascii_alphabetic() {
                    break next;
                }
                parameters.push(next);
            };
            match final_byte {
                'H' => {
                    let mut numbers = parameters.split(';').map(|part| part.parse::<usize>().unwrap_or(1));
                    self.row = numbers.next().unwrap_or(1) - 1;
                    self.column = numbers.next().unwrap_or(1) - 1;
                }
                'K' => {
                    if let Some(row) = self.cells.get_mut(self.row) {
                        for cell in row.iter_mut().skip(self.column) {
                            *cell = ' ';
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn row(&self, index: usize) -> String {
        self.cells[index].iter().collect::<String>()
    }
}

struct Driver<'a> {
    files: MemoryFiles,
    editor: TerminalEditor<'a>,
    screen: Screen,
}

impl<'a> Driver<'a> {
    fn new(owned: &'a mut Owned, scratch: &'a mut [u8], path: Option<&[u8]>, rows: usize, columns: usize) -> Self {
        let mut files = disk();
        let mut editor = TerminalEditor::new(owned.editor(), scratch, COLOUR, rows, columns);
        editor.open(&mut files, path);
        let mut driver = Self {
            files,
            editor,
            screen: Screen::new(rows, columns),
        };
        driver.draw();
        driver
    }

    fn draw(&mut self) {
        let mut out = vec![0u8; 1 << 16];
        let length = self.editor.frame(&mut out);
        self.screen.apply(&out[..length]);
    }

    fn keys(&mut self, bytes: &[u8]) -> Outcome {
        let outcome = self.editor.feed(&mut self.files, bytes);
        self.draw();
        outcome
    }
}

#[test]
fn the_screen_has_its_title_text_status_and_hints_at_80x24() {
    let mut owned = Owned::new(4096);
    let mut scratch = vec![0u8; 4096];
    let driver = Driver::new(&mut owned, &mut scratch, Some(b"/home/root/main.rs"), 24, 80);
    assert_eq!(driver.screen.row(0).trim(), "main.rs - Text Editor");
    assert!(driver.screen.row(1).starts_with("fn main() {"));
    assert!(driver.screen.row(22).trim_start().starts_with("Ln 1, Col 1"));
    assert!(driver.screen.row(22).trim_end().ends_with("Rust  LF"));
    assert!(driver.screen.row(23).starts_with("^S Save  ^Q Quit  F1 Keys"));
}

#[test]
fn typing_marks_the_title_and_ctrl_s_saves() {
    let mut owned = Owned::new(4096);
    let mut scratch = vec![0u8; 4096];
    let mut driver = Driver::new(&mut owned, &mut scratch, Some(b"/home/root/notes.txt"), 24, 80);
    driver.keys(b"Hi ");
    assert_eq!(driver.screen.row(0).trim(), "*notes.txt - Text Editor");
    assert!(driver.screen.row(22).contains("Modified"));
    driver.keys(b"\x13");
    assert_eq!(driver.screen.row(0).trim(), "notes.txt - Text Editor");
    assert_eq!(driver.files.contents("/home/root/notes.txt").unwrap(), b"Hi one egg\ntwo eggs\nEGG three\n");
}

#[test]
fn quitting_with_changes_asks_and_each_answer_does_what_it_says() {
    let mut owned = Owned::new(4096);
    let mut scratch = vec![0u8; 4096];
    let mut driver = Driver::new(&mut owned, &mut scratch, Some(b"/home/root/notes.txt"), 24, 80);
    driver.keys(b"x");
    assert_eq!(driver.keys(b"\x11"), Outcome::Going);
    assert!(driver.screen.row(22).contains("Save changes to notes.txt?"));
    assert!(driver.screen.row(23).starts_with("Y Save  N Don't save  Esc Cancel"));
    driver.keys(b"\x1b");
    assert!(driver.editor.waits_for_escape());
    assert_eq!(driver.editor.escape_timed_out(&mut driver.files), Outcome::Going);
    assert_eq!(driver.keys(b"\x11"), Outcome::Going);
    assert_eq!(driver.keys(b"n"), Outcome::Quit);
    assert_eq!(driver.files.contents("/home/root/notes.txt").unwrap(), b"one egg\ntwo eggs\nEGG three\n");
}

#[test]
fn find_selects_counts_and_moves_between_matches() {
    let mut owned = Owned::new(4096);
    let mut scratch = vec![0u8; 4096];
    let mut driver = Driver::new(&mut owned, &mut scratch, Some(b"/home/root/notes.txt"), 24, 80);
    driver.keys(b"\x06egg");
    assert!(driver.screen.row(22).trim_end().ends_with("1 of 3"), "{}", driver.screen.row(22));
    driver.keys(b"\x1b[B");
    assert!(driver.screen.row(22).trim_end().ends_with("2 of 3"));
    driver.keys(b"\x1bc");
    assert!(driver.screen.row(22).contains("Find (match case): egg"));
}

#[test]
fn replace_all_is_one_undo_step() {
    let mut owned = Owned::new(4096);
    let mut scratch = vec![0u8; 4096];
    let mut driver = Driver::new(&mut owned, &mut scratch, Some(b"/home/root/notes.txt"), 24, 80);
    driver.keys(b"\x12egg\rhen\ra");
    assert!(driver.screen.row(22).contains("3 replaced"), "{}", driver.screen.row(22));
    assert_eq!(driver.editor.editor().document().bytes(), b"one hen\ntwo hens\nhen three\n");
    driver.keys(b"\x1a");
    assert_eq!(driver.editor.editor().document().bytes(), b"one egg\ntwo eggs\nEGG three\n");
}

#[test]
fn a_bracketed_paste_is_one_step_with_line_breaks_made_plain() {
    let mut owned = Owned::new(4096);
    let mut scratch = vec![0u8; 4096];
    let mut driver = Driver::new(&mut owned, &mut scratch, None, 24, 80);
    driver.keys(b"\x1b[200~a\r\nb\tc\x1b[201~");
    assert_eq!(driver.editor.editor().document().bytes(), b"a\nb\tc");
    driver.keys(b"\x1a");
    assert_eq!(driver.editor.editor().document().bytes(), b"");
}

#[test]
fn go_to_line_refuses_out_of_range_and_moves_the_caret() {
    let mut owned = Owned::new(4096);
    let mut scratch = vec![0u8; 4096];
    let mut driver = Driver::new(&mut owned, &mut scratch, Some(b"/home/root/notes.txt"), 24, 80);
    driver.keys(b"\x079\r");
    assert!(driver.screen.row(22).contains("Enter a line from 1 to 4"));
    driver.keys(b"\x7f2\r");
    assert_eq!(driver.editor.editor().line_and_column(), (2, 1));
}

#[test]
fn small_screens_drop_rows_and_long_lines_show_edge_marks() {
    let mut owned = Owned::new(4096);
    let mut scratch = vec![0u8; 4096];
    let mut driver = Driver::new(&mut owned, &mut scratch, Some(b"/home/root/notes.txt"), 2, 10);
    assert!(driver.screen.row(0).starts_with("one egg"));
    assert!(driver.screen.row(1).trim_start().starts_with("Ln 1"));
    driver.keys(b"0123456789");
    assert!(driver.screen.row(0).starts_with('<'), "{}", driver.screen.row(0));
}

#[test]
fn widths_and_styles_follow_the_terminal() {
    assert_eq!(cells_of('日', COLOUR), 2);
    assert_eq!(cells_of('\u{301}', COLOUR), 0);
    let egg = Style::from_environment(Some(b"xterm-256color"), None, Some(b"EggTerminal"));
    assert_eq!(cells_of('日', egg), 1);
    assert!(egg.colour);
    assert!(!Style::from_environment(Some(b"dumb"), None, None).colour);
    assert!(!Style::from_environment(Some(b"xterm"), Some(b"1"), None).colour);
    assert!(!Style::from_environment(None, None, None).colour);
    assert_eq!(size_report(b"\x1b[24;80R"), Some((24, 80, 8)));
    assert_eq!(size_report(b"\x1b[1;2R"), None);
}
