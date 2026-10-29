use super::*;
use crate::memory::{MemoryFiles, Node};
use eggos_toolkit::input::Modifiers;

const CLOCK: u64 = 1_700_000_000;

fn disk() -> MemoryFiles {
    let mut files = MemoryFiles::new();
    files
        .folder("/home/root")
        .folder("/tmp")
        .folder("/Applications")
        .folder("/bin")
        .file("/media/host/notes.txt", b"hello from the host\n", 5)
        .file("/media/host/linux-static", b"\x7fELF\x02\x01\x01", 5)
        .file("/media/host/win-paint.exe", b"MZ\x90\x00", 5)
        .file("/media/host/blob.bin", b"\0\x01\x02", 5)
        .file(
            "/Applications/Hello/app.manifest",
            b"eggos-app 1\nname=Hello\nsource=linux\nrun=/bin/uname\n",
            5,
        )
        .device("/media/host", 3)
        .read_only("/media/host")
        .device("/tmp", 2);
    files
}

fn window(files: &mut MemoryFiles) -> FilesWindow {
    FilesWindow::new(files, None, Architecture::X86_64, false, CLOCK)
}

struct Driver {
    files: MemoryFiles,
    window: FilesWindow,
    now: u32,
}

impl Driver {
    fn new() -> Self {
        let mut files = disk();
        let window = window(&mut files);
        Self {
            files,
            window,
            now: 10_000,
        }
    }

    fn input(&mut self, input: Input) {
        self.input_after(input, 1000);
    }

    fn input_after(&mut self, input: Input, step: u32) {
        self.now += step;
        assert!(self.window.handle(&mut self.files, input, self.now, CLOCK));
    }

    fn press(&mut self, code: u16, modifiers: u32, character: char) {
        self.input_after(
            Input::Key {
                code,
                pressed: true,
                modifiers: Modifiers(modifiers),
                character: character as u32,
            },
            100,
        );
        self.input_after(
            Input::Key {
                code,
                pressed: false,
                modifiers: Modifiers(modifiers),
                character: character as u32,
            },
            100,
        );
    }

    fn typed(&mut self, words: &str) {
        for character in words.chars() {
            self.press(0, 0, character);
        }
    }

    fn click_at(&mut self, x: i32, y: i32, button: MouseButton) {
        self.input(Input::PointerMove { x, y });
        for pressed in [true, false] {
            self.input(Input::Button {
                x,
                y,
                button,
                pressed,
            });
        }
    }

    fn click(&mut self, rect: Rect) {
        self.click_at(
            rect.x + rect.width / 2,
            rect.y + rect.height / 2,
            MouseButton::Left,
        );
    }

    fn double_click(&mut self, rect: Rect) {
        let (x, y) = (rect.x + rect.width / 2, rect.y + rect.height / 2);
        self.input(Input::PointerMove { x, y });
        for pressed in [true, false, true, false] {
            self.now += 40;
            assert!(self.window.handle(
                &mut self.files,
                Input::Button {
                    x,
                    y,
                    button: MouseButton::Left,
                    pressed,
                },
                self.now,
                CLOCK
            ));
        }
    }

    fn place(&mut self, place: Place) {
        let index = self
            .window
            .places()
            .iter()
            .position(|shown| *shown == place)
            .expect("the place is in the sidebar");
        let rect = self.window.place_rect(index);
        self.click(rect);
    }

    fn row(&self, name: &str) -> usize {
        self.window
            .listing()
            .find(name.as_bytes())
            .unwrap_or_else(|| panic!("no {name} in the list"))
    }

    fn select(&mut self, name: &str) {
        let rect = self.window.row_rect(self.row(name));
        self.click(rect);
    }

    fn names(&self) -> Vec<String> {
        let listing = self.window.listing();
        (0..listing.len())
            .map(|index| String::from_utf8_lossy(listing.name(index)).into_owned())
            .collect()
    }

    fn folder(&self) -> String {
        match self.window.location() {
            Location::Folder(path) => String::from_utf8_lossy(path.as_bytes()).into_owned(),
            Location::Trash => "Trash".to_owned(),
        }
    }

    fn request(&mut self) -> Option<Request> {
        self.window.take_request()
    }
}

#[test]
fn it_opens_at_home_making_it_and_lists_the_places_with_the_share_while_mounted() {
    let mut files = MemoryFiles::new();
    files.folder("/media/host");
    let window = window(&mut files);
    assert_eq!(files.node("/home/root"), Some(&Node::Folder));
    assert_eq!(*window.location(), Place::Home.location());
    assert!(!window.places().contains(&Place::HostShare));
    assert_eq!(window.places().last(), Some(&Place::Trash));
    let driver = Driver::new();
    assert_eq!(driver.window.places().len(), 9);
    assert_eq!(driver.window.current_place(), Some(0));
    assert_eq!(Place::Trash.label(true), "Recycle Bin");
}

#[test]
fn the_share_lists_its_files_by_name_and_says_what_they_are() {
    let mut driver = Driver::new();
    driver.place(Place::HostShare);
    assert_eq!(driver.folder(), "/media/host");
    assert_eq!(
        driver.names(),
        ["blob.bin", "linux-static", "notes.txt", "win-paint.exe"]
    );
    let listing = driver.window.listing();
    assert_eq!(
        listing.get(1).unwrap().kind,
        Kind::Program(manifest::Source::Linux)
    );
    assert_eq!(listing.get(2).unwrap().kind, Kind::Text);
}

#[test]
fn double_clicks_run_programs_offer_installs_and_show_text_in_the_terminal() {
    let mut driver = Driver::new();
    driver.place(Place::HostShare);
    let rect = driver.window.row_rect(driver.row("linux-static"));
    driver.double_click(rect);
    match driver.request() {
        Some(Request::TypeInTerminal(line)) => {
            assert_eq!(line.as_bytes(), b"'/media/host/linux-static'")
        }
        other => panic!("{other:?}"),
    }
    let rect = driver.window.row_rect(driver.row("win-paint.exe"));
    driver.double_click(rect);
    assert_eq!(
        driver.request(),
        Some(Request::Install(
            Path::from(b"/media/host/win-paint.exe").unwrap()
        ))
    );
    driver.select("notes.txt");
    driver.press(key::ENTER, 0, '\r');
    match driver.request() {
        Some(Request::TypeInTerminal(line)) => {
            assert_eq!(line.as_bytes(), b"cat '/media/host/notes.txt'")
        }
        other => panic!("{other:?}"),
    }
    driver.select("blob.bin");
    driver.press(key::ENTER, 0, '\r');
    assert_eq!(driver.window.notice(), Some("No app opens blob.bin yet."));
    assert_eq!(driver.request(), None);
}

#[test]
fn text_opens_in_the_text_editor_when_it_is_installed() {
    let mut driver = Driver::new();
    driver.files.file("/bin/text-editor", b"\x7fELF", 5);
    driver.place(Place::HostShare);
    let rect = driver.window.row_rect(driver.row("notes.txt"));
    driver.double_click(rect);
    assert_eq!(
        driver.request(),
        Some(Request::OpenInEditor(
            Path::from(b"/media/host/notes.txt").unwrap()
        ))
    );
    driver.select("blob.bin");
    driver.press(key::ENTER, 0, '\r');
    assert_eq!(driver.window.notice(), Some("No app opens blob.bin yet."));
}

#[test]
fn a_quote_in_a_path_is_written_for_the_shell() {
    let line = TerminalLine::quoted(b"cat ", b"/tmp/it's").unwrap();
    assert_eq!(line.as_bytes(), b"cat '/tmp/it'\\''s'");
    assert_eq!(TerminalLine::quoted(b"", b"/tmp/a\nb"), None);
    assert_eq!(TerminalLine::quoted(b"", &[b'x'; LINE_CAPACITY]), None);
}

#[test]
fn an_app_opens_and_its_delete_offers_uninstall() {
    let mut driver = Driver::new();
    driver.place(Place::Apps);
    assert_eq!(driver.names(), ["Hello"]);
    let rect = driver.window.row_rect(0);
    driver.double_click(rect);
    assert_eq!(
        driver.request(),
        Some(Request::OpenApp(Name::from(b"Hello").unwrap()))
    );
    driver.press(key::DELETE, 0, '\0');
    assert_eq!(
        driver.request(),
        Some(Request::Uninstall(Name::from(b"Hello").unwrap()))
    );
}

#[test]
fn a_new_folder_is_named_in_place_and_renamed_with_f2() {
    let mut driver = Driver::new();
    driver.press(code::N, Modifiers::CTRL, 'n');
    assert_eq!(
        driver.files.node("/home/root/New folder"),
        Some(&Node::Folder)
    );
    driver.press(key::ENTER, 0, '\r');
    driver.press(code::N, Modifiers::SUPER | Modifiers::SHIFT, 'N');
    for _ in 0.."New folder 2".len() {
        driver.press(key::BACKSPACE, 0, '\u{8}');
    }
    driver.typed("Projects");
    driver.press(key::ENTER, 0, '\r');
    assert_eq!(driver.names(), ["New folder", "Projects"]);
    driver.select("New folder");
    driver.press(code::F2, 0, '\0');
    for _ in 0.."New folder".len() {
        driver.press(key::BACKSPACE, 0, '\u{8}');
    }
    driver.typed("Projects");
    driver.press(key::ENTER, 0, '\r');
    assert_eq!(
        driver.window.notice(),
        Some("Projects already exists here.")
    );
    driver.press(key::ESC, 0, '\u{1b}');
    driver.press(code::F2, 0, '\0');
    driver.typed("/x");
    driver.press(key::ENTER, 0, '\r');
    assert_eq!(driver.window.notice(), Some("A name cannot contain \"/\"."));
    driver.press(key::ESC, 0, '\u{1b}');
    assert_eq!(driver.names(), ["New folder", "Projects"]);
}

#[test]
fn copy_out_of_the_share_then_cut_and_paste_move_a_file_and_the_share_refuses_changes() {
    let mut driver = Driver::new();
    driver.place(Place::HostShare);
    driver.select("notes.txt");
    driver.press(code::X, Modifiers::CTRL, 'x');
    assert_eq!(driver.window.notice(), Some("The host share is read-only."));
    driver.press(code::C, Modifiers::CTRL, 'c');
    driver.press(code::V, Modifiers::CTRL, 'v');
    assert_eq!(driver.window.notice(), Some("The host share is read-only."));
    driver.place(Place::Home);
    driver.press(code::V, Modifiers::CTRL, 'v');
    assert_eq!(
        driver.files.contents("/home/root/notes.txt"),
        Some(&b"hello from the host\n"[..])
    );
    driver.press(code::V, Modifiers::CTRL, 'v');
    assert_eq!(driver.names(), ["notes copy.txt", "notes.txt"]);
    assert_eq!(driver.window.selected(), Some(driver.row("notes copy.txt")));
    driver.press(code::N, Modifiers::CTRL, 'n');
    driver.press(key::ENTER, 0, '\r');
    driver.select("notes.txt");
    driver.press(code::X, Modifiers::CTRL, 'x');
    assert!(driver.window.is_cut(driver.row("notes.txt")));
    let folder = driver.window.row_rect(driver.row("New folder"));
    driver.double_click(folder);
    assert_eq!(driver.folder(), "/home/root/New folder");
    driver.press(code::V, Modifiers::CTRL, 'v');
    assert_eq!(driver.names(), ["notes.txt"]);
    assert_eq!(driver.files.node("/home/root/notes.txt"), None);
    driver.press(code::V, Modifiers::CTRL, 'v');
    assert_eq!(driver.names(), ["notes.txt"]);
    driver.press(key::BACKSPACE, 0, '\u{8}');
    assert_eq!(driver.folder(), "/home/root");
    assert_eq!(driver.window.selected(), Some(driver.row("New folder")));
}

#[test]
fn a_folder_is_not_pasted_into_itself() {
    let mut driver = Driver::new();
    driver.press(code::N, Modifiers::CTRL, 'n');
    driver.press(key::ENTER, 0, '\r');
    driver.select("New folder");
    driver.press(code::C, Modifiers::CTRL, 'c');
    driver.press(key::ENTER, 0, '\r');
    driver.press(code::V, Modifiers::CTRL, 'v');
    assert_eq!(
        driver.window.notice(),
        Some("Cannot paste a folder into itself.")
    );
}

#[test]
fn delete_moves_to_the_trash_where_restore_and_permanent_delete_work() {
    let mut driver = Driver::new();
    driver.files.file("/home/root/old.txt", b"old", 1);
    driver.files.file("/home/root/keep.txt", b"keep", 1);
    driver.press(code::F5, 0, '\0');
    driver.select("old.txt");
    driver.press(key::DELETE, 0, '\0');
    assert_eq!(driver.names(), ["keep.txt"]);
    assert_eq!(driver.window.selected(), Some(0));
    assert_eq!(
        driver
            .files
            .contents("/home/root/.local/share/Trash/files/old.txt"),
        Some(&b"old"[..])
    );
    driver.press(key::DELETE, 0, '\0');
    driver.place(Place::Trash);
    assert_eq!(driver.folder(), "Trash");
    assert_eq!(driver.names(), ["keep.txt", "old.txt"]);
    driver.select("old.txt");
    driver.press(key::ENTER, Modifiers::ALT, '\r');
    assert_eq!(driver.window.dialog_line(0), Some("old.txt"));
    assert_eq!(driver.window.dialog_line(4), Some("From: /home/root"));
    driver.press(key::ESC, 0, '\u{1b}');
    let rect = driver.window.row_rect(driver.row("old.txt"));
    driver.click_at(rect.x + 40, rect.y + 10, MouseButton::Right);
    let restore = driver.window.menu_entry_rect(0).unwrap();
    driver.click(restore);
    assert_eq!(driver.names(), ["keep.txt"]);
    assert_eq!(
        driver.files.contents("/home/root/old.txt"),
        Some(&b"old"[..])
    );
    driver.select("keep.txt");
    driver.press(key::DELETE, 0, '\0');
    assert_eq!(
        driver.window.dialog_line(0),
        Some("Delete keep.txt permanently?")
    );
    driver.press(key::ENTER, 0, '\r');
    assert!(driver.names().is_empty());
    assert!(
        driver
            .files
            .paths_under("/home/root/.local/share/Trash/info")
            .is_empty()
    );
}

#[test]
fn empty_trash_asks_then_deletes_everything() {
    let mut driver = Driver::new();
    driver
        .files
        .file("/home/root/a", b"a", 1)
        .file("/home/root/b", b"b", 1);
    driver.press(code::F5, 0, '\0');
    driver.select("a");
    driver.press(key::DELETE, 0, '\0');
    driver.press(key::DELETE, 0, '\0');
    driver.place(Place::Trash);
    let rows = driver.window.rows();
    driver.click_at(rows.x + 40, rows.bottom() - 10, MouseButton::Right);
    let empty = driver.window.menu_entry_rect(0).unwrap();
    driver.click(empty);
    assert_eq!(driver.window.dialog_line(0), Some("Empty the Trash?"));
    assert_eq!(
        driver.window.dialog_line(1),
        Some("Deletes 2 items. This cannot be undone.")
    );
    let button = driver.window.dialog_button_rect(1).unwrap();
    driver.click(button);
    assert!(driver.names().is_empty());
    assert!(
        driver
            .files
            .paths_under("/home/root/.local/share/Trash/files")
            .is_empty()
    );
}

#[test]
fn tmp_has_no_trash_so_delete_asks_first() {
    let mut driver = Driver::new();
    driver.files.file("/tmp/scratch", b"s", 1);
    driver.place(Place::Tmp);
    driver.select("scratch");
    driver.press(key::DELETE, 0, '\0');
    assert_eq!(
        driver.window.dialog_line(1),
        Some("/tmp is not kept, so it has no Trash. This cannot be undone.")
    );
    driver.press(key::ESC, 0, '\u{1b}');
    assert_eq!(driver.names(), ["scratch"]);
    driver.press(key::DELETE, 0, '\0');
    driver.press(key::ENTER, 0, '\r');
    assert!(driver.names().is_empty());
}

#[test]
fn a_long_copy_shows_its_progress_and_can_be_cancelled() {
    let mut driver = Driver::new();
    driver
        .files
        .file("/home/root/big.bin", &vec![1u8; 4 * 1024 * 1024], 1);
    driver.press(code::F5, 0, '\0');
    driver.select("big.bin");
    driver.press(code::C, Modifiers::CTRL, 'c');
    driver.place(Place::Tmp);
    driver.press(code::V, Modifiers::CTRL, 'v');
    assert!(driver.window.is_working());
    assert_eq!(driver.window.dialog_line(0), Some("Copying big.bin..."));
    driver.window.tick(&mut driver.files, driver.now, CLOCK);
    driver.press(key::ESC, 0, '\u{1b}');
    assert!(!driver.window.is_working());
    assert_eq!(driver.window.notice(), Some("Stopped copying big.bin."));
    assert_eq!(driver.files.node("/tmp/big.bin"), None);
    driver.press(code::V, Modifiers::CTRL, 'v');
    while driver.window.is_working() {
        driver.now += 16;
        driver.window.tick(&mut driver.files, driver.now, CLOCK);
    }
    assert_eq!(
        driver.files.contents("/tmp/big.bin").map(<[u8]>::len),
        Some(4 * 1024 * 1024)
    );
}

#[test]
fn the_context_menus_follow_what_is_under_the_pointer() {
    let mut driver = Driver::new();
    driver.place(Place::HostShare);
    let rect = driver.window.row_rect(driver.row("win-paint.exe"));
    driver.click_at(rect.x + 40, rect.y + 10, MouseButton::Right);
    let labels = |driver: &Driver| -> Vec<(&'static str, bool)> {
        driver
            .window
            .menu
            .as_ref()
            .unwrap()
            .0
            .entries()
            .iter()
            .filter_map(|entry| match entry {
                Entry::Item { label, enabled, .. } => Some((*label, *enabled)),
                Entry::Separator => None,
            })
            .collect()
    };
    assert_eq!(
        labels(&driver),
        [
            ("Open", true),
            ("Install...", true),
            ("Cut", false),
            ("Copy", true),
            ("Rename", false),
            ("Move to Trash", false),
            ("Properties", true)
        ]
    );
    driver.press(key::ESC, 0, '\u{1b}');
    driver.place(Place::Tmp);
    let rows = driver.window.rows();
    driver.click_at(rows.x + 40, rows.y + 10, MouseButton::Right);
    assert_eq!(
        labels(&driver),
        [("New folder", true), ("Paste", false), ("Reload", true)]
    );
}

#[test]
fn properties_say_the_kind_size_and_place() {
    let mut driver = Driver::new();
    driver.place(Place::HostShare);
    driver.select("notes.txt");
    driver.press(code::I, Modifiers::SUPER, 'i');
    assert_eq!(driver.window.dialog_line(1), Some("Kind: Text"));
    assert_eq!(driver.window.dialog_line(2), Some("Size: 20 B (20 bytes)"));
    assert_eq!(
        driver.window.dialog_line(3),
        Some("Modified: 1970-01-01 00:00")
    );
    assert_eq!(driver.window.dialog_line(4), Some("Where: /media/host"));
    driver.press(key::ENTER, 0, '\r');
    assert_eq!(driver.window.dialog_line(0), None);
    driver.place(Place::Apps);
    driver.select("Hello");
    driver.press(key::ENTER, Modifiers::ALT, '\r');
    assert_eq!(driver.window.dialog_line(1), Some("Kind: App, Linux"));
    assert_eq!(driver.window.dialog_line(2), Some("Runs: /bin/uname"));
}

#[test]
fn back_up_and_the_path_field_move_between_folders() {
    let mut driver = Driver::new();
    driver.place(Place::System);
    assert_eq!(driver.folder(), "/");
    driver.press(key::LEFT, Modifiers::ALT, '\0');
    assert_eq!(driver.folder(), "/home/root");
    driver.press(code::L, Modifiers::CTRL, 'l');
    for _ in 0.."/home/root".len() {
        driver.press(key::BACKSPACE, 0, '\u{8}');
    }
    driver.typed("/tmp");
    driver.press(key::ENTER, 0, '\r');
    assert_eq!(driver.folder(), "/tmp");
    driver.click(UP_BUTTON);
    assert_eq!(driver.folder(), "/");
    assert_eq!(driver.window.selected(), Some(driver.row("tmp")));
    driver.click(BACK_BUTTON);
    assert_eq!(driver.folder(), "/tmp");
    driver.press(code::L, Modifiers::CTRL, 'l');
    driver.typed("x");
    driver.press(key::ENTER, 0, '\r');
    assert_eq!(
        driver.window.notice(),
        Some("Could not open /tmpx (No such file or directory).")
    );
}

#[test]
fn type_ahead_jumps_to_the_next_name_starting_with_the_letters() {
    let mut driver = Driver::new();
    driver.place(Place::HostShare);
    driver.select("blob.bin");
    driver.press(0, 0, 'w');
    assert_eq!(driver.window.selected(), Some(driver.row("win-paint.exe")));
    driver.now += 5000;
    driver.typed("no");
    assert_eq!(driver.window.selected(), Some(driver.row("notes.txt")));
}

#[test]
fn the_window_draws_its_parts_and_a_notice_ends_after_three_seconds() {
    let mut driver = Driver::new();
    driver.place(Place::HostShare);
    driver.select("notes.txt");
    let (width, height) = DEFAULT_SIZE;
    let mut pixels = vec![0u32; (width * height) as usize];
    driver.window.render(&mut pixels, width, height, 1);
    let at = |x: i32, y: i32| pixels[y as usize * width as usize + x as usize];
    assert_eq!(at(1, 1), colour_of::BAR);
    assert_eq!(at(1, 100), colour_of::DOCK);
    let row = driver.window.row_rect(driver.row("notes.txt"));
    assert_eq!(at(row.x + 100, row.y + 12), colour_of::SELECTION);
    assert_eq!(at(row.x + 200, row.y + 30), colour_of::MENU);
    driver.press(code::X, Modifiers::CTRL, 'x');
    let notice = driver.window.notice_rect().unwrap();
    assert_eq!(notice.bottom(), height as i32 - 24 - 16);
    let deadline = driver.window.deadline(driver.now).unwrap();
    driver.window.tick(&mut driver.files, deadline, CLOCK);
    assert_eq!(driver.window.notice(), None);
}

mod colour_of {
    pub use eggos_toolkit::colour::*;
}

#[test]
fn a_path_being_typed_survives_a_reload_and_closing_stops_a_job() {
    let mut driver = Driver::new();
    driver.press(code::L, Modifiers::CTRL, 'l');
    driver.typed("/x");
    driver.input(Input::FocusOut);
    driver.input(Input::FocusIn);
    assert_eq!(driver.window.path_field.text.as_str(), "/home/root/x");
    driver.click(BACK_BUTTON);
    driver.place(Place::Tmp);
    assert_eq!(driver.window.path_field.text.as_str(), "/tmp");
    driver
        .files
        .file("/home/root/big.bin", &vec![1u8; 4 * 1024 * 1024], 1);
    driver.place(Place::Home);
    driver.select("big.bin");
    driver.press(code::C, Modifiers::CTRL, 'c');
    driver.place(Place::Tmp);
    driver.press(code::V, Modifiers::CTRL, 'v');
    assert!(driver.window.is_working());
    driver.window.close(&mut driver.files);
    assert!(!driver.window.is_working());
    assert_eq!(driver.files.node("/tmp/big.bin"), None);
    assert_eq!(driver.files.open_count(), 0);
}

#[test]
fn a_folder_is_not_pasted_into_itself_through_an_unnormalised_path() {
    let mut driver = Driver::new();
    driver.files.folder("/home/root/Work");
    driver.press(code::F5, 0, '\0');
    driver.select("Work");
    driver.press(code::C, Modifiers::CTRL, 'c');
    driver.press(code::L, Modifiers::CTRL, 'l');
    for _ in 0.."/home/root".len() {
        driver.press(key::BACKSPACE, 0, '\u{8}');
    }
    driver.typed("/home/root/./Work/");
    driver.press(key::ENTER, 0, '\r');
    assert_eq!(driver.folder(), "/home/root/Work");
    driver.press(code::V, Modifiers::CTRL, 'v');
    assert_eq!(
        driver.window.notice(),
        Some("Cannot paste a folder into itself.")
    );
    assert!(driver.files.paths_under("/home/root/Work").is_empty());
}

#[test]
fn renames_keep_names_they_cannot_show_apps_keep_theirs_and_long_paths_are_refused() {
    let mut driver = Driver::new();
    driver.files.file_at(b"/home/root/caf\xe9", b"x", 1);
    driver.press(code::F5, 0, '\0');
    let index = driver.window.listing().find(b"caf\xe9").unwrap();
    let rect = driver.window.row_rect(index);
    driver.click(rect);
    driver.press(code::F2, 0, '\0');
    driver.press(key::ENTER, 0, '\r');
    assert_eq!(driver.window.notice(), None);
    assert_eq!(
        driver.files.paths_under("/home/root"),
        ["/home/root/caf\u{fffd}"]
    );
    driver.place(Place::Apps);
    driver.select("Hello");
    driver.press(code::F2, 0, '\0');
    assert_eq!(
        driver.window.notice(),
        Some("An installed app keeps its name: use Uninstall... to remove it.")
    );
    driver.press(code::L, Modifiers::CTRL, 'l');
    driver.typed(&"/a".repeat(140));
    driver.press(key::ENTER, 0, '\r');
    assert_eq!(
        driver.window.notice(),
        Some("Could not open this folder (File name too long).")
    );
    assert_eq!(driver.folder(), "/Applications");
}

#[test]
fn an_agent_reads_the_entries_as_list_items_and_opens_one_by_its_element() {
    use eggos_toolkit::elements::{Action, MAX_TREE_BYTES, Outcome, Role, TreeWriter, elements};
    let mut files = disk();
    files
        .folder("/home/root/Documents")
        .file("/home/root/notes.txt", b"hi\n", 5);
    let mut window = window(&mut files);
    let mut bytes = vec![0u8; MAX_TREE_BYTES];
    let mut tree = TreeWriter::new(&mut bytes);
    window.describe(&mut tree);
    let read: Vec<_> = elements(tree.bytes()).map(Result::unwrap).collect();
    let entries: Vec<_> = read
        .iter()
        .filter(|element| element.parent == ENTRIES_ELEMENT)
        .map(|element| (element.id, element.role, element.name, element.value))
        .collect();
    let documents = entries
        .iter()
        .find(|entry| entry.2 == b"Documents")
        .copied()
        .unwrap();
    assert_eq!((documents.1, documents.3), (Role::ListItem, &b"folder"[..]));
    assert!(
        entries
            .iter()
            .any(|entry| entry.2 == b"notes.txt" && entry.3 == b"file")
    );
    assert!(
        read.iter()
            .any(|element| element.id == PATH_ELEMENT && element.role == Role::TextField)
    );
    assert!(
        read.iter()
            .any(|element| element.parent == PLACES_ELEMENT && element.name == b"Home")
    );
    assert_eq!(
        window.act(&mut files, documents.0, Action::Invoke),
        Outcome::Done
    );
    match window.location() {
        Location::Folder(path) => assert_eq!(path.as_bytes(), b"/home/root/Documents"),
        Location::Trash => panic!("not opened"),
    }
    assert_eq!(
        window.act(&mut files, FIRST_ENTRY_ELEMENT + 400, Action::Invoke),
        Outcome::NoSuchElement
    );
    assert_eq!(
        window.act(&mut files, PATH_ELEMENT, Action::SetText),
        Outcome::NotSupported
    );
    assert_eq!(
        window.act(&mut files, FIRST_PLACE_ELEMENT, Action::Invoke),
        Outcome::Done
    );
}
