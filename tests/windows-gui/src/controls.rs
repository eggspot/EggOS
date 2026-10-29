//! What `win-controls.exe`, the Windows controls test program (D145), builds and shows, as data
//! both sides share: the program creates these controls, menus and dialogs through user32 and
//! comdlg32, and the QEMU test paints the same states with `eggos_windows_gui` on the host. It
//! follows the worked example of `docs/ux/windows-controls-v0.3.md` section 8. Rectangles are
//! `(x, y, width, height)` in the client area.

/// The window's title.
pub const TITLE: &str = "win-controls";
/// The client area under the menu bar.
pub const CLIENT: (i32, i32) = (400, 300);
/// The content: the menu bar and the client area.
pub const CONTENT: (i32, i32) = (400, 324);
/// The outside size the program asks for: the content and EggOS's chrome.
pub const OUTSIDE: (i32, i32) = (CONTENT.0 + 2, CONTENT.1 + 30);
/// The class background, `COLOR_BTNFACE + 1`.
pub const BACKGROUND: usize = 15 + 1;

/// One control the program creates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Control {
    pub id: usize,
    pub class: &'static str,
    pub text: &'static str,
    pub style: u32,
    pub rect: (i32, i32, i32, i32),
}

const WS_CHILD_VISIBLE: u32 = 0x5000_0000;
const WS_TABSTOP: u32 = 0x0001_0000;
const WS_BORDER: u32 = 0x0080_0000;

pub const NAME_LABEL: Control = Control {
    id: 10,
    class: "STATIC",
    text: "Name:",
    style: WS_CHILD_VISIBLE,
    rect: (16, 16, 64, 24),
};
pub const NAME: Control = Control {
    id: 11,
    class: "EDIT",
    text: "",
    style: WS_CHILD_VISIBLE | WS_TABSTOP | WS_BORDER,
    rect: (88, 14, 200, 24),
};
pub const ADD: Control = Control {
    id: 12,
    class: "BUTTON",
    text: "&Add",
    style: WS_CHILD_VISIBLE | WS_TABSTOP | 0x1,
    rect: (296, 14, 88, 24),
};
pub const UPPER_CASE: Control = Control {
    id: 13,
    class: "BUTTON",
    text: "&Upper case",
    style: WS_CHILD_VISIBLE | WS_TABSTOP | 0x3,
    rect: (16, 52, 160, 24),
};
pub const LIST: Control = Control {
    id: 14,
    class: "LISTBOX",
    text: "",
    style: WS_CHILD_VISIBLE | WS_TABSTOP | WS_BORDER | 0x1 | 0x2,
    rect: (16, 88, 200, 160),
};
pub const STATUS: Control = Control {
    id: 15,
    class: "STATIC",
    text: "Starting",
    style: WS_CHILD_VISIBLE,
    rect: (16, 264, 368, 24),
};
/// Every control, in creation order.
pub const CONTROLS: [Control; 6] = [NAME_LABEL, NAME, ADD, UPPER_CASE, LIST, STATUS];

/// The item the list starts with.
pub const FIRST_ITEM: &str = "alpha";
/// What the test types into the edit and adds to the list.
pub const TYPED: &str = "egg";

/// A drop-down's item: its label (`None` for a separator) and its id.
pub type MenuItem = (Option<&'static str>, usize);

/// The menu: the bar's entries and each one's items.
pub const MENU: [(&str, &[MenuItem]); 2] = [
    (
        "File",
        &[
            (Some("&Open..."), OPEN_ID),
            (Some("&Save As..."), SAVE_ID),
            (None, 0),
            (Some("E&xit"), EXIT_ID),
        ],
    ),
    ("&Help", &[(Some("&About"), ABOUT_ID)]),
];
pub const OPEN_ID: usize = 101;
pub const SAVE_ID: usize = 102;
pub const EXIT_ID: usize = 103;
pub const ABOUT_ID: usize = 201;

/// The window timer's id and interval, and the thread timer's interval, in milliseconds.
pub const WINDOW_TIMER: (usize, u32) = (1, 100);
pub const THREAD_TIMER_INTERVAL: u32 = 150;

/// The folder the pickers start in, which the program makes, and the picker's filter.
pub const FOLDER: &str = r"C:\tmp\win-controls";
pub const FILTER: &str = "Text files (*.txt)\0*.txt\0\0";
pub const FILTER_DESCRIPTION: &str = "Text files (*.txt)";
pub const DEFAULT_EXTENSION: &str = "txt";
/// What the test types into the Save As picker, the file it makes and what the file holds.
pub const SAVE_TYPED: &str = "note";
pub const SAVED_NAME: &str = "note.txt";
pub const SAVED_PATH: &str = r"C:\tmp\win-controls\note.txt";
pub const FILE_TEXT: &str = "egg\n";

/// The status line at each step.
pub const STATUS_TIMERS: &str = "Timers 1 2";
pub const STATUS_ADDED: &str = "Added egg";
pub const STATUS_UPPER_CASE: &str = "Upper case on";
pub const STATUS_SELECTED: &str = "Selected egg";
pub const STATUS_SAVED: &str = "Saved note.txt";
pub const STATUS_READ: &str = "Read egg";

/// Which control has the keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    None,
    Name,
    Add,
    UpperCase,
    List,
}

/// The main window's state at one step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    pub name: &'static str,
    pub items: &'static [&'static str],
    pub selected: Option<usize>,
    pub upper_case: bool,
    pub focus: Focus,
    pub status: &'static str,
    pub file_menu_open: bool,
}

const ONE_ITEM: &[&str] = &[FIRST_ITEM];
const TWO_ITEMS: &[&str] = &[FIRST_ITEM, TYPED];

/// The window once both timers fired.
pub const STARTED: Frame = Frame {
    name: "",
    items: ONE_ITEM,
    selected: None,
    upper_case: false,
    focus: Focus::None,
    status: STATUS_TIMERS,
    file_menu_open: false,
};
/// After a click in the edit and `egg` typed.
pub const TYPED_IN: Frame = Frame {
    name: TYPED,
    focus: Focus::Name,
    ..STARTED
};
/// After a click on `Add`.
pub const ADDED: Frame = Frame {
    name: "",
    items: TWO_ITEMS,
    focus: Focus::Add,
    status: STATUS_ADDED,
    ..STARTED
};
/// After a click on `Upper case`.
pub const CHECKED: Frame = Frame {
    upper_case: true,
    focus: Focus::UpperCase,
    status: STATUS_UPPER_CASE,
    ..ADDED
};
/// After a click on the `egg` row.
pub const SELECTED: Frame = Frame {
    selected: Some(1),
    focus: Focus::List,
    status: STATUS_SELECTED,
    ..CHECKED
};
/// With the File menu open.
pub const MENU_OPEN: Frame = Frame {
    file_menu_open: true,
    ..SELECTED
};
/// After the Save As picker made the file.
pub const SAVED: Frame = Frame {
    status: STATUS_SAVED,
    ..SELECTED
};
/// After the Open picker read it back.
pub const READ: Frame = Frame {
    status: STATUS_READ,
    ..SELECTED
};

/// The bar entry and drop-down rows the test clicks, in content coordinates.
pub const FILE_ENTRY_POINT: (i32, i32) = (24, 12);
pub const OPEN_ITEM_POINT: (i32, i32) = (40, 39);
pub const SAVE_ITEM_POINT: (i32, i32) = (40, 63);
pub const EXIT_ITEM_POINT: (i32, i32) = (40, 96);
