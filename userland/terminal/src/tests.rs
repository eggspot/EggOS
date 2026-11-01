use crate::keys::{KeyAction, key_action, key_modifiers};
use crate::options::{self, OptionsError, program_title};
use crate::paste::{PASTE_CAPACITY, PASTE_CHUNK, prepare};
use crate::selection::SelectionMode;
use crate::title::{CUT_TEXT, DEFAULT_TITLE, TITLE_CAPACITY, Title};
use crate::window::{
    DEFAULT_CONTENT, INDICATOR_MS, MAX_COLUMNS, MAX_ROWS, MINIMUM_CONTENT, PASTE, ProcessEnd,
    SELECT_ALL, Shell, ShellOutput, TerminalWindow, exit_line, grid_for,
};
use eggos_element_tree::{Action, MAX_TREE_BYTES, Outcome, TreeWriter, elements, flags};
use eggos_terminal::keymap::{CursorKeys, Modifiers as KeyModifiers};
use eggos_terminal::{ANSI_COLOURS, TERM_FOREGROUND, TERM_SELECTION, ViewMove};
use eggos_toolkit::input::{Input, Modifiers, MouseButton, key};
use eggos_toolkit::menu::Entry;
use eggos_toolkit::tree::PointerShape;
use std::collections::VecDeque;

#[derive(Debug, Default)]
struct FakeShell {
    output: VecDeque<Vec<u8>>,
    ended: bool,
    end: Option<ProcessEnd>,
    written: Vec<u8>,
    writes: Vec<usize>,
    room_for: Option<usize>,
    sizes: Vec<(u16, u16)>,
    program_running: bool,
    new_windows: Vec<Option<String>>,
    clipboard: String,
}

impl Shell for FakeShell {
    fn read(&mut self, buffer: &mut [u8]) -> ShellOutput {
        match self.output.pop_front() {
            Some(bytes) => {
                buffer[..bytes.len()].copy_from_slice(&bytes);
                ShellOutput::Bytes(bytes.len())
            }
            None if self.ended => ShellOutput::Ended,
            None => ShellOutput::Nothing,
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        self.written.extend_from_slice(bytes);
        self.writes.push(bytes.len());
    }

    fn has_room(&mut self) -> bool {
        match &mut self.room_for {
            None => true,
            Some(0) => false,
            Some(chunks) => {
                *chunks -= 1;
                true
            }
        }
    }

    fn set_window_size(&mut self, columns: u16, rows: u16) {
        self.sizes.push((columns, rows));
    }

    fn foreground_is_shell(&self) -> bool {
        !self.program_running
    }

    fn open_new_window(&mut self, folder: Option<&str>) {
        self.new_windows.push(folder.map(str::to_string));
    }

    fn set_clipboard(&mut self, text: &str) {
        self.clipboard = text.to_string();
    }

    fn clipboard(&mut self, into: &mut [u8]) -> usize {
        let bytes = self.clipboard.as_bytes();
        let count = bytes.len().min(into.len());
        into[..count].copy_from_slice(&bytes[..count]);
        bytes.len()
    }

    fn process_end(&mut self) -> Option<ProcessEnd> {
        self.end
    }
}

fn open_window() -> TerminalWindow<FakeShell> {
    let (width, height) = DEFAULT_CONTENT;
    let mut window = TerminalWindow::new(FakeShell::default(), width as i32, height as i32, 0)
        .expect("memory for a window");
    window.resize(width as i32, height as i32);
    window
}

fn shell_writes(window: &mut TerminalWindow<FakeShell>, bytes: &[u8], now: u32) {
    window.shell_mut().output.push_back(bytes.to_vec());
    assert!(window.read_shell(now));
}

fn row(window: &TerminalWindow<FakeShell>, row: u16) -> String {
    let mut text = [0u8; 512];
    let length = window.terminal().row_text(row, &mut text);
    String::from_utf8(text[..length].to_vec()).unwrap()
}

fn press(code: u16, modifiers: u32, character: char) -> Input {
    Input::Key {
        code,
        pressed: true,
        modifiers: Modifiers(modifiers),
        character: character as u32,
    }
}

const SHIFT: u32 = 1;
const CTRL: u32 = 2;
const SUPER: u32 = 8;
const KEY_C: u16 = 46;
const KEY_V: u16 = 47;
const KEY_A: u16 = 30;
const KEY_LEFT_SHIFT: u16 = 42;

fn cell_x(column: u16) -> i32 {
    8 + 8 * i32::from(column)
}

fn cell_y(row: u16) -> i32 {
    8 + 16 * i32::from(row) + 8
}

fn button(x: i32, y: i32, button: MouseButton, pressed: bool) -> Input {
    Input::Button {
        x,
        y,
        button,
        pressed,
    }
}

fn click(window: &mut TerminalWindow<FakeShell>, x: i32, y: i32, at: u32) {
    assert!(window.handle(button(x, y, MouseButton::Left, true), at));
    assert!(window.handle(button(x, y, MouseButton::Left, false), at));
}

fn drag(window: &mut TerminalWindow<FakeShell>, from: (u16, u16), to: (u16, u16), at: u32) {
    let (start_x, start_y) = (cell_x(from.1), cell_y(from.0));
    let (end_x, end_y) = (cell_x(to.1), cell_y(to.0));
    assert!(window.handle(button(start_x, start_y, MouseButton::Left, true), at));
    assert!(window.handle(Input::PointerMove { x: end_x, y: end_y }, at + 1));
    assert!(window.handle(button(end_x, end_y, MouseButton::Left, false), at + 2));
}

fn span(window: &TerminalWindow<FakeShell>) -> ((u32, u16), (u32, u16)) {
    let selection = window.selection().expect("a selection");
    let (start, end) = selection.span(window.terminal());
    ((start.line, start.column), (end.line, end.column))
}

fn described(window: &TerminalWindow<FakeShell>) -> Vec<(String, String, u16)> {
    let mut bytes = vec![0u8; MAX_TREE_BYTES];
    let mut tree = TreeWriter::new(&mut bytes);
    window.describe(&mut tree);
    let written = tree.bytes().to_vec();
    elements(&written)
        .map(|element| {
            let element = element.unwrap();
            (
                String::from_utf8_lossy(element.name).into_owned(),
                String::from_utf8_lossy(element.value).into_owned(),
                element.flags,
            )
        })
        .collect()
}

fn pixel(window: &TerminalWindow<FakeShell>, x: i32, y: i32) -> u32 {
    let width = window.size().0;
    window.pixels()[(y * width + x) as usize]
}
#[test]
fn the_grid_fills_the_content_inside_the_padding_within_its_caps() {
    let (width, height) = DEFAULT_CONTENT;
    assert_eq!(grid_for(width as i32, height as i32), (80, 24));
    let (width, height) = MINIMUM_CONTENT;
    assert_eq!(grid_for(width as i32, height as i32), (20, 5));
    assert_eq!(grid_for(657, 401), (80, 24));
    assert_eq!(grid_for(816, 496), (100, 30));
    assert_eq!(grid_for(10_000, 10_000), (MAX_COLUMNS, MAX_ROWS));
    assert_eq!(grid_for(3, 3), (1, 1));
}

#[test]
fn titles_carry_the_suffix_and_long_ones_are_cut_on_a_character() {
    assert_eq!(Title::of(b"", false).as_str(), DEFAULT_TITLE);
    assert_eq!(Title::default().as_str(), "Terminal");
    assert_eq!(Title::of(b"build", false).as_str(), "build - Terminal");
    let exact = "x".repeat(53);
    assert_eq!(
        Title::of(exact.as_bytes(), false).as_str().len(),
        TITLE_CAPACITY
    );
    let long = "y".repeat(60);
    let title = Title::of(long.as_bytes(), false);
    assert_eq!(
        title.as_str(),
        format!("{}... - Terminal", &long[..CUT_TEXT])
    );
    assert_eq!(title.as_str().len(), TITLE_CAPACITY);
    let accented = format!("{}é", "z".repeat(49));
    assert_eq!(
        Title::of(accented.as_bytes(), true).as_str(),
        format!("{}... - Terminal", "z".repeat(49))
    );
    assert_eq!(Title::of(b"ok\xff\xfe", false).as_str(), "ok - Terminal");
    assert_eq!(Title::of(b"\xff", false).as_str(), DEFAULT_TITLE);
}

#[test]
fn keys_send_what_the_layout_typed_and_the_keymap_s_sequences() {
    let normal = CursorKeys::Normal;
    let sent = |action: KeyAction| match action {
        KeyAction::Send(bytes) => bytes.as_bytes().to_vec(),
        other => panic!("{other:?}"),
    };
    assert_eq!(sent(key_action(30, Modifiers(0), 'a' as u32, normal)), b"a");
    assert_eq!(sent(key_action(30, Modifiers(1), 'A' as u32, normal)), b"A");
    assert_eq!(
        sent(key_action(0, Modifiers(0), 'é' as u32, normal)),
        "é".as_bytes()
    );
    assert_eq!(
        sent(key_action(46, Modifiers(2), 'c' as u32, normal)),
        [0x03]
    );
    assert_eq!(
        sent(key_action(45, Modifiers(4), 'x' as u32, normal)),
        b"\x1bx"
    );
    assert_eq!(
        sent(key_action(16, Modifiers(4 | 32), '@' as u32, normal)),
        b"@",
        "AltGr types the character"
    );
    assert_eq!(sent(key_action(key::ENTER, Modifiers(0), 0, normal)), b"\r");
    assert_eq!(
        sent(key_action(
            key::UP,
            Modifiers(0),
            0,
            CursorKeys::Application
        )),
        b"\x1bOA"
    );
    assert_eq!(
        sent(key_action(49, Modifiers(2), 'n' as u32, normal)),
        [0x0e]
    );
    assert_eq!(key_action(13, Modifiers(0), 0, normal), KeyAction::Nothing);
    assert_eq!(
        key_action(49, Modifiers(1 | 2), 'N' as u32, normal),
        KeyAction::NewWindow
    );
    assert_eq!(
        key_action(49, Modifiers(8), 'n' as u32, normal),
        KeyAction::NewWindow
    );
    assert_eq!(
        key_action(17, Modifiers(1 | 2), 'W' as u32, normal),
        KeyAction::Close
    );
    assert_eq!(
        key_action(key::PAGE_UP, Modifiers(1), 0, normal),
        KeyAction::View(ViewMove::PageBack)
    );
    assert_eq!(
        key_modifiers(Modifiers(1 | 2 | 4 | 8 | 16 | 32)),
        KeyModifiers::SHIFT
            .with(KeyModifiers::CTRL)
            .with(KeyModifiers::ALT)
            .with(KeyModifiers::SUPER)
            .with(KeyModifiers::CAPS_LOCK)
            .with(KeyModifiers::ALT_GRAPH)
    );
}

#[test]
fn the_shell_learns_the_grid_and_sees_typing_and_its_output_is_shown() {
    let mut window = open_window();
    assert_eq!(window.shell().sizes, [(80, 24)]);
    assert!(
        !window.is_showing_size(),
        "the first size shows no indicator"
    );
    shell_writes(&mut window, b"eggsh:/# ", 10);
    assert_eq!(row(&window, 0), "eggsh:/#");
    assert!(window.handle(press(38, 0, 'l'), 20));
    assert!(window.handle(press(31, 0, 's'), 21));
    assert!(window.handle(press(key::ENTER, 0, '\0'), 22));
    assert_eq!(window.shell_mut().written, b"ls\r");
    shell_writes(&mut window, b"\x1b[6n", 30);
    assert!(window.shell().written.ends_with(b"\x1b[1;10R"));
    assert!(!window.take_damage().is_empty());
    assert!(window.take_damage().is_empty(), "nothing changed since");
}

#[test]
fn a_new_grid_is_told_to_the_shell_and_shown_by_the_indicator_for_a_while() {
    let mut window = open_window();
    window.tick(1000);
    window.resize(816, 496);
    assert_eq!(window.shell().sizes.last(), Some(&(100, 30)));
    assert_eq!(window.terminal().size(), (100, 30));
    assert!(window.is_showing_size());
    window.resize(820, 500);
    assert_eq!(
        window.shell().sizes.len(),
        2,
        "the same grid is not told again"
    );
    assert!(
        window
            .deadline(1000)
            .is_some_and(|at| at <= 1000 + INDICATOR_MS)
    );
    window.tick(1000 + INDICATOR_MS);
    assert!(!window.is_showing_size());
}

#[test]
fn closing_asks_only_while_a_program_runs_and_the_question_can_be_cancelled() {
    let mut window = open_window();
    assert!(window.close_requested(0));
    let mut window = open_window();
    window.shell_mut().program_running = true;
    assert!(!window.close_requested(0));
    assert!(window.is_asking_to_close());
    assert!(window.handle(press(30, 0, 'a'), 1));
    assert!(
        window.shell_mut().written.is_empty(),
        "keys go to the question"
    );
    assert!(window.handle(press(key::ESC, 0, '\0'), 2));
    assert!(!window.is_asking_to_close());
    assert!(!window.close_requested(3));
    assert!(
        !window.handle(press(key::ENTER, 0, '\0'), 4),
        "Enter is Close"
    );
    assert!(window.is_closing());
    let mut window = open_window();
    assert!(
        !window.handle(press(17, 1 | 2, 'W'), 5),
        "Ctrl+Shift+W closes"
    );
    let mut window = open_window();
    assert!(window.handle(press(49, 1 | 2, 'N'), 6));
    assert_eq!(window.shell().new_windows, [None]);
}

#[test]
fn a_program_s_title_names_the_window_and_the_shell_s_end_closes_it() {
    let mut window = open_window();
    assert_eq!(window.take_title(), None);
    shell_writes(&mut window, b"\x1b]2;vi notes\x07", 1);
    assert_eq!(window.take_title(), Some("vi notes - Terminal"));
    assert_eq!(window.take_title(), None);
    assert_eq!(window.title(), "vi notes - Terminal");
    window.shell_mut().ended = true;
    assert!(!window.read_shell(2));
    assert!(window.is_closing());
}

#[test]
fn agents_read_the_screen_as_rows_of_text() {
    let mut window = open_window();
    shell_writes(&mut window, b"eggsh:/# echo hi\r\nhi\r\neggsh:/# ", 1);
    let mut bytes = vec![0u8; MAX_TREE_BYTES];
    let mut tree = TreeWriter::new(&mut bytes);
    window.describe(&mut tree);
    let written = tree.bytes();
    let contains = |needle: &[u8]| written.windows(needle.len()).any(|part| part == needle);
    assert!(contains(b"screen") && contains(b"80x24"));
    assert!(contains(b"row 1") && contains(b"eggsh:/# echo hi"));
    assert!(contains(b"row 24"));
    assert_eq!(window.act(2, Action::Invoke), Outcome::NotSupported);
    assert_eq!(
        window.act(2 + 24, Action::Invoke),
        Outcome::NotSupported
    );
    assert_eq!(
        window.act(2 + 25, Action::Invoke),
        Outcome::NoSuchElement
    );
}

#[test]
fn a_drag_selects_boundary_to_boundary_and_a_click_selects_nothing() {
    let mut window = open_window();
    shell_writes(&mut window, b"\r\n\r\n0123456789abcdefghij\x1b[90mXY\x1b[0m", 1);
    click(&mut window, 8 + 8 * 10 + 3, cell_y(2), 10);
    assert!(window.selection().is_none());
    assert!(window.handle(button(8 + 8 * 10 + 3, cell_y(2), MouseButton::Left, true), 1000));
    assert_eq!(span(&window), ((2, 10), (2, 10)));
    assert!(window.handle(Input::PointerMove { x: 8 + 8 * 10 + 5, y: cell_y(2) }, 1001));
    assert_eq!(span(&window), ((2, 10), (2, 11)));
    assert!(window.handle(Input::PointerMove { x: 8 + 8 * 20 + 3, y: cell_y(2) }, 1002));
    assert!(window.handle(button(8 + 8 * 20 + 3, cell_y(2), MouseButton::Left, false), 1003));
    assert_eq!(span(&window), ((2, 10), (2, 20)));
    assert_eq!(window.selected_text(), "abcdefghij");
    let rows = described(&window);
    let flagged = |name: &str| {
        rows.iter()
            .find(|(element, _, _)| element == name)
            .is_some_and(|(_, _, element_flags)| element_flags & flags::SELECTED != 0)
    };
    assert!(flagged("row 3"));
    assert!(!flagged("row 2"));
    assert!(rows.iter().any(|(name, value, _)| name == "selection" && value == "abcdefghij"));
    window.take_damage();
    assert_eq!(pixel(&window, cell_x(10), 8 + 32), TERM_SELECTION);
    assert_ne!(pixel(&window, cell_x(9), 8 + 32), TERM_SELECTION);
    drag(&mut window, (2, 20), (2, 22), 2000);
    window.take_damage();
    let glyph: Vec<u32> = (0..16)
        .flat_map(|y| (0..8).map(move |x| (x, y)))
        .map(|(x, y)| pixel(&window, cell_x(20) + x, 8 + 32 + y))
        .collect();
    assert!(glyph.contains(&TERM_FOREGROUND));
    assert!(!glyph.contains(&ANSI_COLOURS[8]));
    assert!(glyph.contains(&TERM_SELECTION));
}

#[test]
fn a_drag_over_rows_copies_trimmed_rows_joined_by_newlines_but_not_across_a_wrap() {
    let mut window = open_window();
    let mut text = b"\r\n".to_vec();
    text.extend_from_slice(&[b'A'; 75]);
    text.extend_from_slice(b"\r\nmiddle   \r\n0123456789");
    shell_writes(&mut window, &text, 1);
    drag(&mut window, (1, 70), (3, 5), 10);
    assert_eq!(window.selected_text(), "AAAAA\nmiddle\n01234");
    let mut window = open_window();
    let mut text = b"\r\n".to_vec();
    text.extend_from_slice(&[b'B'; 80]);
    text.extend_from_slice(b"CCCCC\r\n0123456789");
    shell_writes(&mut window, &text, 1);
    drag(&mut window, (1, 70), (3, 5), 10);
    assert_eq!(window.selected_text(), "BBBBBBBBBBCCCCC\n01234");
}

fn double_click(window: &mut TerminalWindow<FakeShell>, row: u16, column: u16, at: u32) {
    click(window, cell_x(column) + 3, cell_y(row), at);
    click(window, cell_x(column) + 3, cell_y(row), at + 100);
}

#[test]
fn a_double_click_selects_a_word_and_a_drag_after_it_extends_by_words() {
    let mut window = open_window();
    shell_writes(&mut window, b"ls /home/mea/a.txt x=(1,2)   end", 1);
    double_click(&mut window, 0, 5, 1000);
    assert_eq!(window.selected_text(), "/home/mea/a.txt");
    assert_eq!(window.selection().map(|selection| selection.mode), Some(SelectionMode::Words));
    double_click(&mut window, 0, 19, 2000);
    assert_eq!(window.selected_text(), "x=");
    double_click(&mut window, 0, 21, 3000);
    assert_eq!(window.selected_text(), "(");
    double_click(&mut window, 0, 27, 4000);
    assert_eq!(span(&window), ((0, 26), (0, 29)));
    click(&mut window, cell_x(19) + 3, cell_y(0), 5000);
    assert!(window.handle(button(cell_x(19) + 3, cell_y(0), MouseButton::Left, true), 5100));
    assert!(window.handle(Input::PointerMove { x: cell_x(30) + 3, y: cell_y(0) }, 5101));
    assert!(window.handle(button(cell_x(30) + 3, cell_y(0), MouseButton::Left, false), 5102));
    assert_eq!(span(&window), ((0, 19), (0, 32)));
}

#[test]
fn a_triple_click_selects_a_logical_line_and_shift_click_moves_the_head() {
    let mut window = open_window();
    let mut text = vec![b'X'; 80];
    text.extend_from_slice(b"tail\r\n\r\nabcdefghijkl");
    shell_writes(&mut window, &text, 1);
    for at in [1000, 1100, 1200] {
        click(&mut window, cell_x(3) + 3, cell_y(0), at);
    }
    assert_eq!(span(&window), ((0, 0), (1, 80)));
    assert_eq!(window.selected_text(), format!("{}tail\n", "X".repeat(80)));
    drag(&mut window, (3, 0), (3, 5), 3000);
    assert!(window.handle(press(KEY_LEFT_SHIFT, SHIFT, '\0'), 3100));
    click(&mut window, cell_x(8), cell_y(3), 4000);
    assert_eq!(span(&window), ((3, 0), (3, 8)));
    assert!(window.handle(
        Input::Key {
            code: KEY_LEFT_SHIFT,
            pressed: false,
            modifiers: Modifiers(0),
            character: 0
        },
        4100
    ));
    click(&mut window, cell_x(8), cell_y(3), 5000);
    assert!(window.selection().is_none(), "a plain click clears");
}

#[test]
fn a_selection_moves_with_its_line_and_goes_when_the_line_is_dropped() {
    let mut window = open_window();
    let lines: Vec<String> = (0..24).map(|line| format!("line{line}")).collect();
    shell_writes(&mut window, lines.join("\r\n").as_bytes(), 1);
    drag(&mut window, (23, 0), (23, 4), 10);
    assert_eq!(span(&window), ((23, 0), (23, 4)));
    shell_writes(&mut window, b"\r\nnew", 20);
    assert_eq!(window.terminal().row_of_line_number(23), Some(22));
    assert_eq!(span(&window), ((23, 0), (23, 4)));
    assert_eq!(window.selected_text(), "line");
    shell_writes(&mut window, "\r\nmore".repeat(24).as_bytes(), 30);
    assert_eq!(window.terminal().row_of_line_number(23), None);
    assert!(window.selection().is_some(), "kept in the scrollback");
    shell_writes(&mut window, "\r\nmore".repeat(260).as_bytes(), 40);
    assert!(window.selection().is_none(), "its line left the ring");
    drag(&mut window, (2, 0), (2, 4), 50);
    assert!(window.selection().is_some());
    shell_writes(&mut window, b"\x1b[?1049h", 60);
    assert!(window.selection().is_none());
}

#[test]
fn copy_keys_keep_the_selection_and_ctrl_c_copies_once_then_interrupts() {
    let mut window = open_window();
    shell_writes(&mut window, b"hello world", 1);
    drag(&mut window, (0, 0), (0, 5), 10);
    assert!(window.handle(press(KEY_C, CTRL | SHIFT, 'C'), 20));
    assert_eq!(window.shell().clipboard, "hello");
    assert!(window.selection().is_some());
    window.shell_mut().clipboard.clear();
    assert!(window.handle(press(KEY_C, SUPER, 'c'), 21));
    assert_eq!(window.shell().clipboard, "hello");
    window.shell_mut().clipboard.clear();
    assert!(window.handle(press(KEY_C, CTRL, 'c'), 22));
    assert_eq!(window.shell().clipboard, "hello");
    assert!(window.selection().is_none());
    assert!(window.shell().written.is_empty());
    assert!(window.handle(press(KEY_C, CTRL, 'c'), 23));
    assert_eq!(window.shell().written, [0x03]);
    assert!(window.handle(press(KEY_V, CTRL, 'v'), 24));
    assert_eq!(window.shell().written, [0x03, 0x16]);
    assert!(window.handle(press(KEY_A, CTRL | SHIFT, 'A'), 25));
    assert_eq!(window.selected_text(), "hello world");
}

#[test]
fn a_paste_is_made_safe_bracketed_on_request_and_written_a_chunk_per_room() {
    assert_eq!(prepare("a\r\nb\nc\x1b[31m\td", false), b"a\rb\rc\td");
    assert_eq!(prepare("x", true), b"\x1b[200~x\x1b[201~");
    assert!(prepare("", true).is_empty());
    let mut window = open_window();
    window.shell_mut().clipboard = "a\r\nb\nc\x1b[31m\td".to_string();
    assert!(window.handle(press(KEY_V, CTRL | SHIFT, 'V'), 1));
    assert_eq!(window.shell().written, b"a\rb\rc\td");
    window.shell_mut().written.clear();
    shell_writes(&mut window, b"\x1b[?2004h", 2);
    assert!(window.handle(press(KEY_V, SUPER, 'v'), 3));
    assert_eq!(window.shell().written, b"\x1b[200~a\rb\rc\td\x1b[201~");
    let mut window = open_window();
    window.shell_mut().clipboard = format!("a{}", "é".repeat(35_000));
    window.shell_mut().room_for = Some(3);
    assert!(window.handle(press(KEY_V, CTRL | SHIFT, 'V'), 1));
    assert_eq!(window.shell().writes, [PASTE_CHUNK; 3]);
    assert!(window.is_pasting());
    assert!(window.deadline(1).is_some());
    assert!(window.handle(press(30, 0, 'z'), 2));
    assert_eq!(window.shell().written.len(), 3 * PASTE_CHUNK, "typing waits behind the paste");
    window.shell_mut().room_for = None;
    window.tick(20);
    assert!(!window.is_pasting());
    let written = &window.shell().written;
    assert_eq!(written.len(), PASTE_CAPACITY - 1 + 1);
    assert_eq!(written.last(), Some(&b'z'));
    assert!(written.iter().all(|&byte| byte >= 0x20));
    let mut window = open_window();
    window.shell_mut().clipboard = "ls".to_string();
    assert!(window.handle(button(100, 100, MouseButton::Middle, true), 1));
    assert_eq!(window.shell().written, b"ls");
    let mut window = open_window();
    assert!(window.handle(press(KEY_V, CTRL | SHIFT, 'V'), 1));
    assert!(window.shell().written.is_empty());
}

fn enabled(window: &TerminalWindow<FakeShell>) -> Vec<(&'static str, bool)> {
    window
        .menu()
        .expect("the menu")
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            Entry::Item { label, enabled, .. } => Some((*label, *enabled)),
            Entry::Separator => None,
        })
        .collect()
}

#[test]
fn the_context_menu_offers_copy_paste_and_select_all_and_closes_as_menus_do() {
    let mut window = open_window();
    shell_writes(&mut window, b"eggsh:/# ls", 1);
    window.shell_mut().clipboard = "x".to_string();
    assert!(window.handle(button(100, 100, MouseButton::Right, true), 10));
    assert_eq!(
        enabled(&window),
        [("Copy", false), ("Paste", true), ("Select All", true)]
    );
    for code in [key::DOWN, key::DOWN, key::ENTER] {
        assert!(window.handle(press(code, 0, '\0'), 11));
    }
    assert!(window.menu().is_none());
    assert_eq!(window.selected_text(), "eggsh:/# ls");
    assert!(window.handle(button(100, 100, MouseButton::Right, true), 12));
    assert_eq!(enabled(&window)[0], ("Copy", true));
    for code in [key::DOWN, key::ENTER] {
        assert!(window.handle(press(code, 0, '\0'), 13));
    }
    assert_eq!(window.shell().clipboard, "eggsh:/# ls");
    assert!(window.handle(button(100, 100, MouseButton::Right, true), 14));
    assert!(window.handle(press(key::ESC, 0, '\0'), 15));
    assert!(window.menu().is_none());
    assert!(window.shell().written.is_empty(), "Esc is not sent");
    assert!(window.handle(press(68, SHIFT, '\0'), 16));
    assert!(window.menu().is_some(), "Shift+F10 opens it at the cursor");
    assert!(window.handle(button(600, 350, MouseButton::Left, true), 17));
    assert!(window.menu().is_none());
    assert!(window.selection().is_some(), "the press reached the grid");
    let _ = (PASTE, SELECT_ALL);
}

#[test]
fn the_pointer_is_an_i_beam_over_the_grid() {
    let mut window = open_window();
    assert!(window.handle(Input::PointerMove { x: 100, y: 100 }, 1));
    assert_eq!(window.pointer_shape(), PointerShape::IBeam);
    assert!(window.handle(Input::PointerMove { x: 2, y: 2 }, 2));
    assert_eq!(window.pointer_shape(), PointerShape::Arrow);
}

#[test]
fn the_command_line_takes_a_title_and_a_program_with_its_arguments() {
    let words = [&b"-t"[..], b"Build", b"-e", b"/bin/make", b"all"];
    let parsed = options::parse(&words, |word: &'static [u8]| word).unwrap();
    assert_eq!(parsed.title, Some(&b"Build"[..]));
    assert_eq!(parsed.command, Some(&words[3..]));
    let alone = [&b"-e"[..]];
    let refused = options::parse(&alone, |word: &'static [u8]| word);
    assert_eq!(refused, Err(OptionsError::MissingProgram));
    assert_eq!(
        OptionsError::MissingProgram.message(),
        "terminal: -e needs a program\n"
    );
    let unknown = [&b"-x"[..]];
    assert_eq!(
        options::parse(&unknown, |word: &'static [u8]| word),
        Err(OptionsError::Usage)
    );
    assert_eq!(program_title(b"/bin/ps"), b"ps");
    assert_eq!(program_title(b"ps"), b"ps");
    let mut window = open_window();
    window.set_title_text("Build");
    assert_eq!(window.take_title(), Some("Build - Terminal"));
}

fn command_window(end: ProcessEnd) -> TerminalWindow<FakeShell> {
    let mut window = open_window();
    window.run_command();
    window.set_title_text("ps");
    shell_writes(&mut window, b"output\r\n", 1);
    window.shell_mut().ended = true;
    window.shell_mut().end = Some(end);
    window
}

#[test]
fn a_command_that_succeeds_closes_its_window_and_one_that_fails_leaves_a_line() {
    let mut window = command_window(ProcessEnd::Exited(0));
    assert!(!window.read_shell(2));
    assert!(window.is_closing());
    let mut window = command_window(ProcessEnd::Exited(2));
    assert!(window.read_shell(2));
    assert_eq!(row(&window, 2), "[process exited with status 2]");
    assert_eq!(window.terminal().line(2)[0].foreground, 8);
    assert!(!window.terminal().cursor_visible());
    assert!(!window.is_watching_the_shell());
    assert!(window.handle(press(KEY_C, CTRL | SHIFT, 'C'), 3));
    assert!(window.handle(press(KEY_A, CTRL | SHIFT, 'A'), 4));
    assert!(!window.handle(press(30, 0, 'a'), 5), "a key that would send closes");
    let mut window = command_window(ProcessEnd::Signalled(11));
    assert!(window.read_shell(2));
    assert_eq!(row(&window, 2), "[process ended by signal 11 (SIGSEGV)]");
    assert!(window.close_requested(3), "nothing runs: no question");
    assert_eq!(exit_line(ProcessEnd::Signalled(99)), "[process ended by signal 99]");
    let mut window = open_window();
    window.run_command();
    window.shell_mut().program_running = true;
    assert!(!window.close_requested(1), "a running command asks first");
    let mut window = open_window();
    window.run_command();
    window.failed_to_start("terminal: frob: not found\r\n", 127);
    assert_eq!(row(&window, 0), "terminal: frob: not found");
    assert_eq!(row(&window, 2), "[process exited with status 127]");
}

#[test]
fn a_shell_s_end_closes_a_shell_window_whatever_its_status() {
    let mut window = open_window();
    window.shell_mut().ended = true;
    window.shell_mut().end = Some(ProcessEnd::Exited(1));
    assert!(!window.read_shell(1));
}

#[test]
fn a_new_size_reflows_the_grid_and_clears_the_selection() {
    let mut window = open_window();
    shell_writes(&mut window, &[b'x'; 100], 1);
    drag(&mut window, (0, 0), (0, 5), 10);
    window.resize(976, 400);
    assert_eq!(window.terminal().size(), (120, 24));
    assert_eq!(row(&window, 0), "x".repeat(100));
    assert!(window.selection().is_none());
}

#[test]
fn the_folder_a_shell_reports_is_listed_and_new_windows_open_in_it() {
    let mut window = open_window();
    assert!(!described(&window).iter().any(|(name, _, _)| name == "folder"));
    shell_writes(&mut window, b"\x1b]7;file:///home/mea/Documents\x07", 1);
    assert!(
        described(&window)
            .iter()
            .any(|(name, value, _)| name == "folder" && value == "/home/mea/Documents")
    );
    assert!(window.handle(press(49, CTRL | SHIFT, 'N'), 2));
    assert_eq!(
        window.shell().new_windows,
        [Some("/home/mea/Documents".to_string())]
    );
}

#[test]
fn agents_select_a_row_s_line_or_the_whole_screen() {
    let mut window = open_window();
    shell_writes(&mut window, b"first\r\nsecond", 1);
    assert_eq!(window.act(2 + 2, Action::Select), Outcome::Done);
    assert_eq!(window.selected_text(), "second\n");
    assert_eq!(window.act(2, Action::Select), Outcome::Done);
    assert_eq!(window.selected_text(), "first\nsecond");
    assert_eq!(window.act(2 + 25, Action::Invoke), Outcome::NotSupported);
}
