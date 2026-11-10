//! What `win-controls2.exe`, the second Windows controls test program (D228), builds, as data
//! both sides share: the program creates these controls through user32 and the QEMU test
//! paints the same states with `eggos_windows_gui` on the host. It has no menu bar, so the
//! content is the client area. Rectangles are `(x, y, width, height)` in the client area.

use crate::controls::Control;

/// The window's title.
pub const TITLE: &str = "win-controls2";
/// The client area, which is the whole content.
pub const CONTENT: (i32, i32) = (400, 160);
/// The outside size the program asks for: the content and EggOS's chrome.
pub const OUTSIDE: (i32, i32) = (CONTENT.0 + 2, CONTENT.1 + 30);
/// The class background, `COLOR_BTNFACE + 1`.
pub const BACKGROUND: usize = 15 + 1;

const WS_CHILD_VISIBLE: u32 = 0x5000_0000;
const WS_CHILD: u32 = 0x4000_0000;
const WS_TABSTOP: u32 = 0x0001_0000;
const WS_GROUP: u32 = 0x0002_0000;
const WS_BORDER: u32 = 0x0080_0000;
const ES_PASSWORD: u32 = 0x0020;
const ES_NUMBER: u32 = 0x2000;
const BS_GROUPBOX: u32 = 0x7;
const BS_AUTORADIOBUTTON: u32 = 0x9;
const SBS_HORZ: u32 = 0x0;

/// A single-line edit the program fills with [`WORD_TEXT`].
pub const WORD: Control = Control {
    id: 21,
    class: "EDIT",
    text: "",
    style: WS_CHILD_VISIBLE | WS_TABSTOP | WS_BORDER,
    rect: (16, 16, 200, 24),
};
/// `ES_PASSWORD`: shows `*` for each character.
pub const PASSWORD: Control = Control {
    id: 22,
    class: "EDIT",
    text: "",
    style: WS_CHILD_VISIBLE | WS_TABSTOP | WS_BORDER | ES_PASSWORD,
    rect: (16, 48, 200, 24),
};
/// `ES_NUMBER`: takes only digits.
pub const NUMBER: Control = Control {
    id: 23,
    class: "EDIT",
    text: "",
    style: WS_CHILD_VISIBLE | WS_TABSTOP | WS_BORDER | ES_NUMBER,
    rect: (16, 80, 200, 24),
};
/// The group box around the two radio buttons.
pub const SIZE_BOX: Control = Control {
    id: 24,
    class: "BUTTON",
    text: "Size",
    style: WS_CHILD_VISIBLE | WS_GROUP | BS_GROUPBOX,
    rect: (232, 16, 152, 88),
};
/// The first radio button of the group, selected at the start.
pub const SMALL: Control = Control {
    id: 25,
    class: "BUTTON",
    text: "Small",
    style: WS_CHILD_VISIBLE | WS_GROUP | WS_TABSTOP | BS_AUTORADIOBUTTON,
    rect: (240, 36, 136, 24),
};
/// The second radio button of the group.
pub const LARGE: Control = Control {
    id: 26,
    class: "BUTTON",
    text: "Large",
    style: WS_CHILD_VISIBLE | BS_AUTORADIOBUTTON,
    rect: (240, 68, 136, 24),
};
/// A horizontal `SCROLLBAR` control, the start of the next group.
pub const BAR: Control = Control {
    id: 27,
    class: "SCROLLBAR",
    text: "",
    style: WS_CHILD_VISIBLE | WS_GROUP | SBS_HORZ,
    rect: (16, 120, 368, 17),
};
/// A hidden list whose items carry values (`LB_SETITEMDATA`).
pub const VALUES: Control = Control {
    id: 28,
    class: "LISTBOX",
    text: "",
    style: WS_CHILD,
    rect: (0, 0, 40, 40),
};
/// A hidden drop-down list combo box whose items carry values (`CB_SETITEMDATA`).
pub const CHOICES: Control = Control {
    id: 29,
    class: "COMBOBOX",
    text: "",
    style: WS_CHILD | 0x3,
    rect: (0, 0, 40, 120),
};

/// Every control, in creation order (which is the tab order).
pub const CONTROLS: [Control; 9] = [
    WORD, PASSWORD, NUMBER, SIZE_BOX, SMALL, LARGE, BAR, VALUES, CHOICES,
];

/// The word edit's text at the start.
pub const WORD_TEXT: &str = "egg nog";
/// The bar's `nMin`, `nMax` and `nPage`; it starts at position 0.
pub const BAR_RANGE: (i32, i32, u32) = (0, 99, 10);
/// The point of the bar's right arrow the test clicks, in the client area.
pub const BAR_RIGHT_ARROW: (i32, i32) = (BAR.rect.0 + BAR.rect.2 - 8, BAR.rect.1 + 8);
/// The point of the word edit the test clicks, past its text, putting the caret at its end.
pub const WORD_END: (i32, i32) = (WORD.rect.0 + 190, WORD.rect.1 + 12);
