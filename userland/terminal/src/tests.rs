use crate::keys::{KeyAction, key_action, key_modifiers};
use crate::title::{CUT_TEXT, DEFAULT_TITLE, TITLE_CAPACITY, Title};
use crate::window::{
    DEFAULT_CONTENT, INDICATOR_MS, MAX_COLUMNS, MAX_ROWS, MINIMUM_CONTENT, Shell, ShellOutput,
    TerminalWindow, grid_for,
};
use eggos_element_tree::{MAX_TREE_BYTES, TreeWriter};
use eggos_terminal::ViewMove;
use eggos_terminal::keymap::{CursorKeys, Modifiers as KeyModifiers};
use eggos_toolkit::input::{Input, Modifiers, key};
use std::collections::VecDeque;

#[derive(Debug, Default)]
struct FakeShell {
    output: VecDeque<Vec<u8>>,
    ended: bool,
    written: Vec<u8>,
    sizes: Vec<(u16, u16)>,
    program_running: bool,
    new_windows: usize,
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
    }

    fn set_window_size(&mut self, columns: u16, rows: u16) {
        self.sizes.push((columns, rows));
    }

    fn foreground_is_shell(&self) -> bool {
        !self.program_running
    }

    fn open_new_window(&mut self) {
        self.new_windows += 1;
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
    assert_eq!(window.shell().new_windows, 1);
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
    assert_eq!(window.act(2), eggos_element_tree::Outcome::NotSupported);
    assert_eq!(
        window.act(2 + 24),
        eggos_element_tree::Outcome::NotSupported
    );
    assert_eq!(
        window.act(2 + 25),
        eggos_element_tree::Outcome::NoSuchElement
    );
}
