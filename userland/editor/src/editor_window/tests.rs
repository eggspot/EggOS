use super::*;
use crate::editor::tests::Owned;
use eggos_files::memory::MemoryFiles;

const NOTES: &[u8] = b"/home/root/notes.txt";

fn disk() -> MemoryFiles {
    let mut files = MemoryFiles::new();
    files
        .folder("/home")
        .folder("/home/root")
        .folder("/media")
        .folder("/media/host")
        .file("/home/root/notes.txt", b"one egg\ntwo eggs\nEGG three\n", 5)
        .file("/home/root/dos.txt", b"a\r\nb\r\n", 5)
        .file("/home/root/photo.png", b"\x89PNG\r\n\x1a\n\0\0", 5)
        .file("/media/host/shared.txt", b"shared\n", 5)
        .read_only("/media/host");
    files
}

struct Driver<'a> {
    files: MemoryFiles,
    window: EditorWindow<'a>,
    now: u32,
}

impl<'a> Driver<'a> {
    fn new(owned: &'a mut Owned, path: Option<&[u8]>) -> Self {
        let mut files = disk();
        let window = EditorWindow::new(owned.editor(), &mut files, path, 0);
        Self {
            files,
            window,
            now: 0,
        }
    }

    fn input(&mut self, input: Input) -> bool {
        self.now += 10;
        self.window.handle(&mut self.files, input, self.now)
    }

    fn press(&mut self, code: u16, character: char, held: u32) -> bool {
        let open = self.input(Input::Key {
            code,
            pressed: true,
            modifiers: Modifiers(held),
            character: character as u32,
        });
        self.input(Input::Key {
            code,
            pressed: false,
            modifiers: Modifiers(held),
            character: character as u32,
        });
        open
    }

    fn command(&mut self, code: u16) -> bool {
        self.press(code, '\0', Modifiers::CTRL)
    }

    fn type_text(&mut self, text: &str) {
        for character in text.chars() {
            let code = match character {
                '\n' => key::ENTER,
                _ => 2,
            };
            self.press(code, character, 0);
        }
    }

    fn click(&mut self, rect: Rect) {
        let (x, y) = (rect.x + rect.width / 2, rect.y + rect.height / 2);
        self.input(Input::PointerMove { x, y });
        for pressed in [true, false] {
            self.input(Input::Button {
                x,
                y,
                button: MouseButton::Left,
                pressed,
            });
        }
    }

    fn text(&self) -> String {
        String::from_utf8(self.window.editor().document().bytes().to_vec()).unwrap()
    }

    fn saved(&self, path: &str) -> String {
        String::from_utf8(self.files.contents(path).unwrap().to_vec()).unwrap()
    }
}

#[test]
fn the_title_names_the_document_marks_changes_and_cuts_long_names() {
    assert_eq!(title_of(UNTITLED, false).as_str(), "Untitled - Text Editor");
    assert_eq!(
        title_of("notes.txt", true).as_str(),
        "*notes.txt - Text Editor"
    );
    let long = "a".repeat(60);
    let cut = title_of(&long, true);
    assert_eq!(cut.len(), 64);
    assert_eq!(
        cut.as_str(),
        format!("*{}...{TITLE_SUFFIX}", "a".repeat(46))
    );
    assert_eq!(title_of(&"b".repeat(49), true).len(), 64);
}

#[test]
fn a_file_opens_with_its_name_and_typing_marks_it_modified() {
    let mut owned = Owned::new(4096);
    let mut driver = Driver::new(&mut owned, Some(NOTES));
    assert_eq!(driver.window.take_title(), Some("notes.txt - Text Editor"));
    assert_eq!(driver.window.take_title(), None);
    assert_eq!(driver.window.status_text().as_str(), "Ln 1, Col 1");
    assert_eq!(driver.window.state_text(), "");
    driver.type_text("Hi ");
    assert_eq!(driver.text(), "Hi one egg\ntwo eggs\nEGG three\n");
    assert_eq!(driver.window.take_title(), Some("*notes.txt - Text Editor"));
    assert_eq!(driver.window.state_text(), "Modified");
    assert_eq!(driver.window.status_text().as_str(), "Ln 1, Col 4");
    driver.command(code::S);
    assert_eq!(driver.saved("/home/root/notes.txt"), driver.text());
    assert_eq!(driver.window.take_title(), Some("notes.txt - Text Editor"));
    assert_eq!(driver.window.state_text(), "");
}

#[test]
fn a_path_with_nothing_there_is_a_new_file_the_first_save_makes() {
    let mut owned = Owned::new(4096);
    let mut driver = Driver::new(&mut owned, Some(b"/home/root/new.txt"));
    assert!(driver.window.is_new_file());
    assert_eq!(driver.window.state_text(), "New file");
    assert_eq!(driver.window.title(), "new.txt - Text Editor");
    driver.type_text("x\n");
    driver.command(code::S);
    assert_eq!(driver.saved("/home/root/new.txt"), "x\n");
    assert!(!driver.window.is_new_file());
}

#[test]
fn what_is_not_text_does_not_open_and_says_why() {
    let mut owned = Owned::new(4096);
    let driver = Driver::new(&mut owned, Some(b"/home/root/photo.png"));
    assert_eq!(
        driver.window.notice(),
        Some("photo.png is not a text file.")
    );
    assert_eq!(driver.window.path(), None);
    assert_eq!(driver.window.title(), "Untitled - Text Editor");
    let mut owned = Owned::new(4096);
    let driver = Driver::new(&mut owned, Some(b"/home/root"));
    assert_eq!(driver.window.notice(), Some("root is a folder."));
}

#[test]
fn a_crlf_file_says_so_and_stays_crlf() {
    let mut owned = Owned::new(4096);
    let mut driver = Driver::new(&mut owned, Some(b"/home/root/dos.txt"));
    assert_eq!(driver.text(), "a\nb\n");
    assert_eq!(driver.window.editor().document().ending().label(), "CRLF");
    driver.command(code::A);
    assert_eq!(
        driver.window.status_text().as_str(),
        "Ln 3, Col 1 \u{b7} 4 selected"
    );
    driver.type_text("c\n");
    driver.command(code::S);
    assert_eq!(driver.saved("/home/root/dos.txt"), "c\r\n");
}

#[test]
fn find_selects_matches_counts_them_and_wraps() {
    let mut owned = Owned::new(4096);
    let mut driver = Driver::new(&mut owned, Some(NOTES));
    driver.command(code::F);
    assert_eq!(driver.window.bar(), Bar::Find);
    driver.type_text("egg");
    assert_eq!(driver.window.editor().selection(), (4, 7));
    assert_eq!(driver.window.found_text().as_str(), "1 of 3");
    driver.press(key::ENTER, '\r', 0);
    assert_eq!(driver.window.editor().selection(), (12, 15));
    assert_eq!(driver.window.found_text().as_str(), "2 of 3");
    driver.press(code::F3, '\0', 0);
    driver.press(code::F3, '\0', 0);
    assert_eq!(driver.window.found_text().as_str(), "1 of 3, from top");
    driver.press(code::F3, '\0', Modifiers::SHIFT);
    assert_eq!(driver.window.found_text().as_str(), "3 of 3, from end");
    driver.press(key::TAB, '\t', 0);
    driver.press(key::SPACE, ' ', 0);
    assert_eq!(driver.window.found_text().as_str(), "1 of 2, from top");
    assert_eq!(driver.window.editor().selection(), (4, 7));
    driver.press(key::ESC, '\u{1b}', 0);
    assert_eq!(driver.window.bar(), Bar::Closed);
    driver.command(code::F);
    driver.type_text("z");
    assert_eq!(driver.window.found_text().as_str(), "Not found");
    assert_eq!(driver.window.editor().selection(), (4, 7));
    driver.press(key::ESC, '\u{1b}', 0);
    assert_eq!(driver.window.bar(), Bar::Closed);
    assert_eq!(driver.window.editor().selection(), (4, 7));
}

#[test]
fn closing_with_changes_asks_and_each_answer_does_what_it_says() {
    let mut owned = Owned::new(4096);
    let mut driver = Driver::new(&mut owned, Some(NOTES));
    assert!(driver.window.close_requested());
    driver.type_text("x");
    assert!(!driver.window.close_requested());
    assert_eq!(
        driver.window.dialog_line(0),
        Some("Save changes to notes.txt?")
    );
    assert!(driver.press(key::ESC, '\u{1b}', 0));
    assert_eq!(driver.window.dialog_line(0), None);
    assert!(!driver.window.close_requested());
    assert!(!driver.press(key::ENTER, '\r', 0));
    assert_eq!(driver.saved("/home/root/notes.txt"), driver.text());
    let mut owned = Owned::new(4096);
    let mut driver = Driver::new(&mut owned, Some(NOTES));
    driver.type_text("x");
    assert!(!driver.window.close_requested());
    let dont_save = driver.window.dialog_button_rect(0).unwrap();
    driver.click(dont_save);
    assert!(!driver.input(Input::FocusIn));
    assert_eq!(
        driver.saved("/home/root/notes.txt"),
        "one egg\ntwo eggs\nEGG three\n"
    );
}

#[test]
fn saving_an_untitled_document_goes_through_the_save_as_bar() {
    let mut owned = Owned::new(4096);
    let mut driver = Driver::new(&mut owned, None);
    driver.type_text("draft");
    assert!(!driver.window.close_requested());
    driver.press(key::ENTER, '\r', 0);
    assert_eq!(driver.window.bar(), Bar::SaveAs);
    driver.press(key::ESC, '\u{1b}', 0);
    assert_eq!(driver.window.bar(), Bar::Closed);
    assert!(!driver.window.close_requested());
    driver.press(key::ENTER, '\r', 0);
    for _ in 0.."Untitled.txt".len() {
        driver.press(key::BACKSPACE, '\u{8}', 0);
    }
    driver.type_text("draft.txt");
    assert!(!driver.press(key::ENTER, '\r', 0));
    assert_eq!(driver.saved("/home/root/draft.txt"), "draft");
}

#[test]
fn save_as_onto_another_file_asks_to_replace_it() {
    let mut owned = Owned::new(4096);
    let mut driver = Driver::new(&mut owned, Some(NOTES));
    driver.command(code::A);
    driver.type_text("new");
    driver.press(code::S, '\0', Modifiers::CTRL | Modifiers::SHIFT);
    assert_eq!(driver.window.bar(), Bar::SaveAs);
    for _ in 0.."notes.txt".len() {
        driver.press(key::BACKSPACE, '\u{8}', 0);
    }
    driver.type_text("dos.txt");
    driver.press(key::ENTER, '\r', 0);
    assert_eq!(driver.window.dialog_line(0), Some("Replace dos.txt?"));
    assert_eq!(
        driver.window.dialog_line(1),
        Some("A file with that name already exists in /home/root.")
    );
    driver.press(key::ENTER, '\r', 0);
    assert_eq!(driver.saved("/home/root/dos.txt"), "new");
    assert_eq!(driver.window.path(), Some(&b"/home/root/dos.txt"[..]));
    assert_eq!(driver.window.bar(), Bar::Closed);
}

#[test]
fn a_read_only_place_and_a_missing_folder_refuse_a_save() {
    let mut owned = Owned::new(4096);
    let mut driver = Driver::new(&mut owned, Some(b"/media/host/shared.txt"));
    driver.type_text("x");
    driver.command(code::S);
    assert_eq!(
        driver.window.notice(),
        Some("Cannot save here: /media/host is read-only.")
    );
    assert!(driver.window.editor().is_modified());
    let mut owned = Owned::new(4096);
    let mut driver = Driver::new(&mut owned, Some(b"/home/root/gone/new.txt"));
    driver.command(code::S);
    assert_eq!(
        driver.window.notice(),
        Some("/home/root/gone does not exist.")
    );
}

#[test]
fn open_replaces_the_document_and_reports_a_missing_file() {
    let mut owned = Owned::new(4096);
    let mut driver = Driver::new(&mut owned, Some(NOTES));
    driver.click(driver.window.tool_rect(Tool::Open));
    assert_eq!(driver.window.bar(), Bar::Open);
    driver.type_text("nothing.txt");
    driver.press(key::ENTER, '\r', 0);
    assert_eq!(driver.window.notice(), Some("nothing.txt does not exist."));
    assert_eq!(driver.window.bar(), Bar::Open);
    for _ in 0.."nothing.txt".len() {
        driver.press(key::BACKSPACE, '\u{8}', 0);
    }
    driver.type_text("dos.txt");
    driver.press(key::ENTER, '\r', 0);
    assert_eq!(driver.window.bar(), Bar::Closed);
    assert_eq!(driver.text(), "a\nb\n");
    assert_eq!(driver.window.title(), "dos.txt - Text Editor");
}

#[test]
fn the_open_bar_asks_before_it_replaces_changes_typed_while_it_shows() {
    let mut owned = Owned::new(4096);
    let mut driver = Driver::new(&mut owned, Some(NOTES));
    driver.click(driver.window.tool_rect(Tool::Open));
    assert_eq!(driver.window.bar(), Bar::Open);
    driver.type_text("dos.txt");
    let view = driver.window.text_view();
    driver.click(Rect::new(view.x + 40, view.y + 8, 1, 1));
    driver.type_text("x");
    assert!(driver.window.editor().is_modified());
    driver.click(driver.window.action_rect());
    assert_eq!(
        driver.window.dialog_line(0),
        Some("Save changes to notes.txt?")
    );
    assert!(driver.text().contains('x'));
    driver.press(key::ESC, '\u{1b}', 0);
    assert_eq!(driver.window.dialog_line(0), None);
    assert_eq!(driver.window.bar(), Bar::Open);
    assert!(driver.text().contains('x'));
    driver.click(driver.window.action_rect());
    driver.press(key::ENTER, '\r', 0);
    assert!(driver.saved("/home/root/notes.txt").contains('x'));
    assert_eq!(driver.text(), "a\nb\n");
    assert_eq!(driver.window.title(), "dos.txt - Text Editor");
    assert_eq!(driver.window.bar(), Bar::Closed);
}

#[test]
fn the_scroll_bars_of_a_very_wide_line_stay_in_their_tracks() {
    const WIDE: &[u8] = b"/home/root/wide.txt";
    let mut owned = Owned::new(1024 * 1024);
    let mut files = disk();
    files.file("/home/root/wide.txt", &vec![b'a'; 512 * 1024], 5);
    let mut window = EditorWindow::new(owned.editor(), &mut files, Some(WIDE), 0);
    let end = Input::Key {
        code: key::END,
        pressed: true,
        modifiers: Modifiers(0),
        character: 0,
    };
    window.handle(&mut files, end, 10);
    let (track, thumb) = window.horizontal_bar().unwrap();
    assert!(thumb.x > track.x && thumb.right() <= track.right());
    assert!(track.right() - thumb.right() <= 1);
    let (x, y) = (thumb.x + thumb.width / 2, thumb.y + thumb.height / 2);
    for input in [
        Input::PointerMove { x, y },
        Input::Button {
            x,
            y,
            button: MouseButton::Left,
            pressed: true,
        },
        Input::PointerMove { x: track.x, y },
        Input::Button {
            x: track.x,
            y,
            button: MouseButton::Left,
            pressed: false,
        },
    ] {
        window.handle(&mut files, input, 20);
    }
    let (track, thumb) = window.horizontal_bar().unwrap();
    assert_eq!(thumb.x, track.x);
    let area = window.area();
    let (x, y) = (area.x + area.width / 2, area.y + area.height / 2);
    window.handle(&mut files, Input::SidewaysWheel { x, y, notches: 2 }, 30);
    let (_, right) = window.horizontal_bar().unwrap();
    assert!(right.x > thumb.x, "the wheel's sideways turn scrolls right");
    window.handle(&mut files, Input::SidewaysWheel { x, y, notches: -2 }, 40);
    let (_, back) = window.horizontal_bar().unwrap();
    assert_eq!(back.x, thumb.x);
}

#[test]
fn new_asks_about_changes_first() {
    let mut owned = Owned::new(4096);
    let mut driver = Driver::new(&mut owned, Some(NOTES));
    driver.type_text("x");
    driver.command(code::N);
    assert_eq!(
        driver.window.dialog_line(0),
        Some("Save changes to notes.txt?")
    );
    let dont_save = driver.window.dialog_button_rect(0).unwrap();
    driver.click(dont_save);
    assert_eq!(driver.text(), "");
    assert_eq!(driver.window.title(), "Untitled - Text Editor");
}

#[test]
fn undo_redo_cut_and_paste_work_from_the_keys_and_the_menu() {
    let mut owned = Owned::new(4096);
    let mut driver = Driver::new(&mut owned, None);
    driver.type_text("abc");
    driver.command(code::Z);
    assert_eq!(driver.text(), "");
    driver.command(code::Y);
    assert_eq!(driver.text(), "abc");
    driver.command(code::A);
    driver.command(code::X);
    assert_eq!(driver.text(), "");
    driver.command(code::V);
    driver.command(code::V);
    assert_eq!(driver.text(), "abcabc");
    let view = driver.window.text_view();
    driver.input(Input::Button {
        x: view.x + 40,
        y: view.y + 8,
        button: MouseButton::Right,
        pressed: true,
    });
    let menu = driver.window.menu_rect().unwrap();
    assert!(menu.width >= 160);
    let undo = driver.window.menu_entry_rect(0).unwrap();
    driver.click(undo);
    assert_eq!(driver.text(), "abc");
    assert_eq!(driver.window.menu_rect(), None);
}

#[test]
fn clicks_place_the_caret_double_clicks_select_words_and_the_gutter_selects_lines() {
    let mut owned = Owned::new(4096);
    let mut driver = Driver::new(&mut owned, Some(NOTES));
    let x = driver.window.column_left(5) + 1;
    let y = driver.window.row_top(1) + 4;
    driver.click(Rect::new(x, y, 1, 1));
    assert_eq!(driver.window.editor().caret(), 13);
    driver.click(Rect::new(x, y, 1, 1));
    assert_eq!(driver.window.editor().selection(), (12, 16));
    driver.click(Rect::new(4, driver.window.row_top(2) + 4, 1, 1));
    assert_eq!(driver.window.editor().selected(), b"EGG three\n");
}

#[test]
fn the_gutter_grows_with_the_line_count_s_digits() {
    let mut owned = Owned::new(8192);
    let mut driver = Driver::new(&mut owned, None);
    assert_eq!(driver.window.gutter_width(), 32);
    for _ in 0..99 {
        driver.press(key::ENTER, '\r', 0);
    }
    assert_eq!(driver.window.gutter_width(), 40);
    for _ in 0..900 {
        driver.press(key::ENTER, '\r', 0);
    }
    assert_eq!(driver.window.editor().document().line_count(), 1000);
    assert_eq!(driver.window.gutter_width(), 48);
}

#[test]
fn a_full_document_refuses_more_and_says_so() {
    let mut owned = Owned::new(4);
    let mut driver = Driver::new(&mut owned, None);
    driver.type_text("abcde");
    assert_eq!(driver.text(), "abcd");
    assert_eq!(driver.window.notice(), Some("The document is full (4 MB)."));
}

#[test]
fn the_window_draws_its_parts_and_the_caret_blinks() {
    let mut owned = Owned::new(4096);
    let mut driver = Driver::new(&mut owned, Some(NOTES));
    let (width, height) = DEFAULT_SIZE;
    let mut shown = vec![0u32; (width * height) as usize];
    let mut hidden = vec![0u32; (width * height) as usize];
    driver.window.render(&mut shown, width, height, 1, true);
    driver.window.render(&mut hidden, width, height, 1, false);
    assert_ne!(shown, hidden);
    let view = driver.window.text_view();
    let at = |x: i32, y: i32| shown[(y * width as i32 + x) as usize];
    assert_eq!(at(view.x + 2, view.bottom() - 2), colour_of_field());
    assert_eq!(at(2, view.bottom() - 2), eggos_toolkit::colour::MENU);
    let caret_x = driver.window.column_left(0) - 1;
    assert_eq!(
        at(caret_x, driver.window.row_top(0) + 8),
        eggos_toolkit::colour::TEXT
    );
    let deadline = driver.window.deadline(0).unwrap();
    driver.window.tick(deadline);
    assert!(!driver.window.take_damage().is_empty());
}

fn colour_of_field() -> u32 {
    eggos_toolkit::colour::FIELD
}

#[test]
fn an_agent_reads_the_text_as_a_text_field_and_sets_it() {
    use eggos_toolkit::elements::{
        Action, Bounds, MAX_TREE_BYTES, Outcome, Role, TreeWriter, elements, flags,
    };
    let mut owned = Owned::new(4096);
    let mut driver = Driver::new(&mut owned, Some(NOTES));
    let mut bytes = vec![0u8; MAX_TREE_BYTES];
    let mut tree = TreeWriter::new(&mut bytes);
    driver.window.describe(&mut tree);
    let read: Vec<_> = elements(tree.bytes()).map(Result::unwrap).collect();
    assert_eq!(read.len(), 1);
    let text = read[0];
    assert_eq!((text.id, text.role), (TEXT_ELEMENT, Role::TextField));
    assert_eq!(text.name, b"notes.txt");
    assert_eq!(text.value, b"one egg\ntwo eggs\nEGG three\n");
    assert_ne!(text.flags & flags::FOCUSABLE, 0);
    let view = driver.window.text_view();
    assert_eq!(
        text.bounds,
        Bounds::new(view.x, view.y, view.width, view.height)
    );
    assert_eq!(
        driver
            .window
            .act(TEXT_ELEMENT, Action::SetText, "fresh text"),
        Outcome::Done
    );
    assert_eq!(driver.text(), "fresh text");
    assert_eq!(driver.window.take_title(), Some("*notes.txt - Text Editor"));
    driver.command(code::Z);
    assert_eq!(
        driver.text(),
        "one egg\ntwo eggs\nEGG three\n",
        "one step to undo"
    );
    assert_eq!(
        driver.window.act(TEXT_ELEMENT, Action::Focus, ""),
        Outcome::Done
    );
    assert_eq!(
        driver.window.act(TEXT_ELEMENT, Action::Invoke, ""),
        Outcome::NotSupported
    );
    assert_eq!(
        driver.window.act(TEXT_ELEMENT + 1, Action::Focus, ""),
        Outcome::NoSuchElement
    );
    driver.type_text("x");
    assert!(!driver.window.close_requested());
    assert_eq!(
        driver.window.act(TEXT_ELEMENT, Action::SetText, "no"),
        Outcome::Disabled,
        "the save question is open"
    );
}
