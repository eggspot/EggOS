//! The minimal desktop of docs/ux/desktop-v0.3.md (D115): the procedural wallpaper, the top
//! bar with the system item, the clock and the power button and its menu, and the terminal's
//! window with its title bar, controls, focus, dragging and minimising to a task chip.
//!
//! [`Desktop`] is pure state and drawing, no device: the kernel feeds it pointer and key
//! events, it records the rectangles they change in a [`Damage`], and [`Desktop::draw`]
//! repaints any rectangle of a framebuffer from scratch. The terminal's cells are not drawn
//! here: the window's grid ([`Desktop::text_area`]) is where [`crate::terminal::Terminal`]
//! renders them, while [`Desktop::terminal_shown`].

mod paint;

#[cfg(test)]
mod tests;

use crate::display::{Damage, Rect};
use crate::font::{GLYPH_HEIGHT, GLYPH_WIDTH};
use crate::input::key;
use crate::terminal::{PADDING, TERM_BACKGROUND, TextArea};
use alloc::vec::Vec;
use paint::{Canvas, egg_half_width, isqrt};

/// The design's colour tokens (section 2), as `0x00RRGGBB` pixels.
pub mod colour {
    pub const WALLPAPER_TOP: u32 = 0x000e_1726;
    pub const WALLPAPER_BOTTOM: u32 = 0x001b_3a5c;
    pub const WALLPAPER_EGG: u32 = 0x001f_3f63;
    pub const ACCENT: u32 = 0x00f2_b33d;
    pub const BAR: u32 = 0x0014_1b26;
    pub const BAR_EDGE: u32 = 0x0020_2938;
    pub const BAR_HOVER: u32 = 0x0026_303f;
    pub const BAR_PRESSED: u32 = 0x0033_4052;
    pub const MENU: u32 = 0x001c_2430;
    pub const MENU_BORDER: u32 = 0x002a_3340;
    pub const MENU_HOVER: u32 = 0x002a_3440;
    pub const TEXT: u32 = 0x00e6_eaf0;
    pub const TEXT_DIM: u32 = 0x008a_94a6;
    pub const TITLE_ACTIVE: u32 = 0x002a_3340;
    pub const TITLE_INACTIVE: u32 = 0x001f_2630;
    pub const BORDER_ACTIVE: u32 = 0x003a_4454;
    pub const BORDER_INACTIVE: u32 = 0x002a_3340;
    pub const BUTTON_CLOSE: u32 = 0x00e5_534b;
    pub const BUTTON_MINIMISE: u32 = 0x00e8_b33c;
    pub const BUTTON_INACTIVE: u32 = 0x004a_5466;
}

/// Height of the top bar, the 1 px edge line included.
pub const BAR_HEIGHT: u32 = 28;
/// Height of a window's title bar.
pub const TITLE_HEIGHT: u32 = 28;
/// The terminal window's grid: 80 columns by 24 rows, fixed in 0.3.0.
pub const GRID_COLUMNS: u16 = 80;
/// See [`GRID_COLUMNS`].
pub const GRID_ROWS: u16 = 24;
/// The terminal window's width, its 1 px border included.
pub const WINDOW_WIDTH: u32 = GRID_COLUMNS as u32 * GLYPH_WIDTH as u32 + 2 * PADDING + 2;
/// The terminal window's height, its 1 px border included.
pub const WINDOW_HEIGHT: u32 =
    GRID_ROWS as u32 * GLYPH_HEIGHT as u32 + 2 * PADDING + TITLE_HEIGHT + 2;
/// The terminal window's title.
pub const WINDOW_TITLE: &str = "Terminal";
/// The power menu's width.
pub const MENU_WIDTH: u32 = 160;
/// One power menu item's height.
pub const MENU_ITEM_HEIGHT: u32 = 32;
/// Glyphs the clock's rectangle holds, centred on the bar.
pub const CLOCK_GLYPHS: usize = 10;

const SHADOW_RINGS: u32 = 4;
const POWER_BUTTON_SIZE: u32 = 28;
const POWER_BUTTON_MARGIN: u32 = 8;
const POWER_ICON_SIZE: u32 = 13;
const POWER_ICON: [u16; 13] = [
    0b0000001000000,
    0b0000001000000,
    0b0001001001000,
    0b0010001000100,
    0b0100001000010,
    0b0100001000010,
    0b0100000000010,
    0b0100000000010,
    0b0010000000100,
    0b0001000001000,
    0b0000111110000,
    0,
    0,
];
const SYSTEM_NAME: &str = "EggOS";
const SYSTEM_NAME_X: u32 = 28;
const BAR_TEXT_Y: u32 = 6;
const BAR_MARK_X: u32 = 12;
const BAR_MARK_HEIGHT: u32 = 14;
const CHIP_X: u32 = 104;
const CHIP_Y: u32 = 4;
const CHIP_HEIGHT: u32 = 20;
const CHIP_PADDING: u32 = 8;
const CONTROL_SIZE: u32 = 12;
const CONTROL_TOP: u32 = 8;
const CLOSE_X: u32 = 12;
const MINIMISE_X: u32 = 32;
const CONTROL_HIT_WIDTH: u32 = 20;
const CLOSE_HIT_X: u32 = 8;
const MINIMISE_HIT_X: u32 = 28;
const MENU_GAP: u32 = 4;
const MENU_TEXT_X: u32 = 12;
const MENU_ITEMS: [(&str, Action); 2] = [
    ("Power off", Action::PowerOff),
    ("Restart", Action::Restart),
];
const SECONDS_PER_MINUTE: u64 = 60;

/// What a pointer or key event asks of the machine, beyond redrawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Nothing more.
    None,
    /// Power the machine off, as `poweroff` does.
    PowerOff,
    /// Restart the machine, as `reboot` does.
    Restart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Drag {
    from_pointer_x: i64,
    from_pointer_y: i64,
}

/// The desktop on a screen of one size.
#[derive(Debug)]
pub struct Desktop {
    width: u32,
    height: u32,
    gradient: Vec<u32>,
    egg: Vec<(u32, u32)>,
    yolk: (u32, u32, u32),
    window_x: u32,
    window_y: u32,
    focused: bool,
    minimised: bool,
    menu_open: bool,
    menu_hover: Option<usize>,
    drag: Option<Drag>,
    pointer: (u32, u32),
    clock: [u8; CLOCK_GLYPHS],
}

impl Desktop {
    /// The desktop of a `width` by `height` screen, its window centred below the bar and
    /// focused, the clock at `up 0:00`; `None` when the window and the bar do not fit.
    pub fn new(width: u32, height: u32) -> Option<Self> {
        if width < WINDOW_WIDTH || height < BAR_HEIGHT + WINDOW_HEIGHT {
            return None;
        }
        let gradient = (0..height).map(|row| gradient_pixel(row, height)).collect();
        let egg_height = height;
        let egg_top = round_percent(25, height);
        let centre = round_percent(82, width);
        let egg = (0..height)
            .map(|row| match row.checked_sub(egg_top) {
                Some(row) => span(centre, egg_half_width(egg_height, row), width),
                None => (0, 0),
            })
            .collect();
        let yolk = (
            centre,
            egg_top + round_percent(62, height),
            round_percent(9, height),
        );
        let mut desktop = Self {
            width,
            height,
            gradient,
            egg,
            yolk,
            window_x: (width - WINDOW_WIDTH) / 2,
            window_y: BAR_HEIGHT + (height - BAR_HEIGHT - WINDOW_HEIGHT) / 2,
            focused: true,
            minimised: false,
            menu_open: false,
            menu_hover: None,
            drag: None,
            pointer: (0, 0),
            clock: [b' '; CLOCK_GLYPHS],
        };
        desktop.set_uptime(0, &mut Damage::new());
        Some(desktop)
    }

    /// The whole screen.
    pub fn screen(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }

    /// The terminal window, its border included, wherever it is (also while minimised).
    pub fn window(&self) -> Rect {
        Rect::new(self.window_x, self.window_y, WINDOW_WIDTH, WINDOW_HEIGHT)
    }

    /// The window's title bar, its border's top row included.
    pub fn title_bar(&self) -> Rect {
        Rect::new(self.window_x, self.window_y, WINDOW_WIDTH, TITLE_HEIGHT + 1)
    }

    /// Where the terminal's grid sits: inside the window's padding.
    pub fn text_area(&self) -> TextArea {
        TextArea {
            x: self.window_x + 1 + PADDING,
            y: self.window_y + 1 + TITLE_HEIGHT + PADDING,
            columns: GRID_COLUMNS,
            rows: GRID_ROWS,
        }
    }

    /// Whether the terminal's grid is on screen and nothing covers it, so the terminal may
    /// render into it.
    pub fn terminal_shown(&self) -> bool {
        !self.minimised && !(self.menu_open && self.menu().overlaps(&self.window()))
    }

    /// Whether keys go to the terminal: its window is focused and shown, and no menu is open.
    pub fn keys_go_to_terminal(&self) -> bool {
        self.focused && !self.minimised && !self.menu_open
    }

    /// Whether the window is focused.
    pub fn focused(&self) -> bool {
        self.focused
    }

    /// Whether the window is minimised to a chip in the bar.
    pub fn minimised(&self) -> bool {
        self.minimised
    }

    /// Whether the power menu is open.
    pub fn menu_open(&self) -> bool {
        self.menu_open
    }

    /// The clock's text, `up m:ss`, centred in [`CLOCK_GLYPHS`] cells.
    pub fn clock_text(&self) -> &str {
        core::str::from_utf8(&self.clock).unwrap_or("")
    }

    /// The clock's rectangle on the bar.
    pub fn clock_rect(&self) -> Rect {
        let width = CLOCK_GLYPHS as u32 * GLYPH_WIDTH as u32;
        Rect::new(
            (self.width - width) / 2,
            BAR_TEXT_Y,
            width,
            GLYPH_HEIGHT as u32,
        )
    }

    /// The power button's hit target.
    pub fn power_button(&self) -> Rect {
        Rect::new(
            self.width - POWER_BUTTON_MARGIN - POWER_BUTTON_SIZE,
            0,
            POWER_BUTTON_SIZE,
            POWER_BUTTON_SIZE,
        )
    }

    /// The power menu, its border included, where it opens.
    pub fn menu(&self) -> Rect {
        let button = self.power_button();
        Rect::new(
            button.right() - MENU_WIDTH,
            BAR_HEIGHT + MENU_GAP,
            MENU_WIDTH,
            MENU_ITEMS.len() as u32 * MENU_ITEM_HEIGHT + 2,
        )
    }

    /// The minimised window's chip in the bar.
    pub fn chip(&self) -> Rect {
        let text = WINDOW_TITLE.len() as u32 * GLYPH_WIDTH as u32;
        Rect::new(CHIP_X, CHIP_Y, text + 2 * CHIP_PADDING, CHIP_HEIGHT)
    }

    /// The window's minimise control's hit target.
    pub fn minimise_control(&self) -> Rect {
        Rect::new(
            self.window_x + MINIMISE_HIT_X,
            self.window_y,
            CONTROL_HIT_WIDTH,
            TITLE_HEIGHT,
        )
    }

    fn close_control(&self) -> Rect {
        Rect::new(
            self.window_x + CLOSE_HIT_X,
            self.window_y,
            CONTROL_HIT_WIDTH,
            TITLE_HEIGHT,
        )
    }

    fn window_with_shadow(&self) -> Rect {
        let window = self.window();
        Rect::new(
            window.x.saturating_sub(SHADOW_RINGS),
            window.y,
            window.width + 2 * SHADOW_RINGS,
            window.height + SHADOW_RINGS,
        )
    }

    fn menu_with_shadow(&self) -> Rect {
        let menu = self.menu();
        Rect::new(
            menu.x - SHADOW_RINGS,
            menu.y,
            menu.width + 2 * SHADOW_RINGS,
            menu.height + SHADOW_RINGS,
        )
    }

    /// Shows `seconds` of uptime on the clock as `up m:ss`; returns whether its text changed,
    /// adding its rectangle to `damage` if so.
    pub fn set_uptime(&mut self, seconds: u64, damage: &mut Damage) -> bool {
        let mut text = [b' '; CLOCK_GLYPHS];
        let mut written = Writer::new(&mut text);
        written.push_str("up ");
        written.push_number(seconds / SECONDS_PER_MINUTE, 1);
        written.push_str(":");
        written.push_number(seconds % SECONDS_PER_MINUTE, 2);
        let length = written.length;
        let shift = (CLOCK_GLYPHS - length) / 2;
        let mut centred = [b' '; CLOCK_GLYPHS];
        centred[shift..shift + length].copy_from_slice(&text[..length]);
        if centred == self.clock {
            return false;
        }
        self.clock = centred;
        damage.add(self.clock_rect());
        true
    }

    /// The pointer moved to (`x`, `y`): drags the window while a drag is on, moves the menu's
    /// hover.
    pub fn pointer_moved(&mut self, x: u32, y: u32, damage: &mut Damage) {
        self.pointer = (x, y);
        if let Some(drag) = self.drag {
            let x = (i64::from(x) - drag.from_pointer_x)
                .clamp(0, i64::from(self.width - WINDOW_WIDTH)) as u32;
            let y = (i64::from(y) - drag.from_pointer_y).clamp(
                i64::from(BAR_HEIGHT),
                i64::from(self.height - WINDOW_HEIGHT),
            ) as u32;
            if (x, y) != (self.window_x, self.window_y) {
                damage.add(self.window_with_shadow());
                self.window_x = x;
                self.window_y = y;
                damage.add(self.window_with_shadow());
            }
        }
        if self.menu_open {
            let hover = self.menu_item_at(x, y);
            if hover != self.menu_hover {
                self.menu_hover = hover;
                damage.add(self.menu());
            }
        }
    }

    fn menu_item_at(&self, x: u32, y: u32) -> Option<usize> {
        let menu = self.menu();
        if !contains(menu, x, y) {
            return None;
        }
        let item = (y.saturating_sub(menu.y + 1) / MENU_ITEM_HEIGHT) as usize;
        (item < MENU_ITEMS.len()).then_some(item)
    }

    /// The left button went down (`pressed`) or up at the pointer.
    pub fn left_button(&mut self, pressed: bool, damage: &mut Damage) -> Action {
        if !pressed {
            self.drag = None;
            return Action::None;
        }
        let (x, y) = self.pointer;
        if self.menu_open {
            let item = self.menu_item_at(x, y);
            self.close_menu(damage);
            if let Some(item) = item {
                return MENU_ITEMS[item].1;
            }
            if contains(self.power_button(), x, y) {
                return Action::None;
            }
        }
        if contains(self.power_button(), x, y) {
            self.open_menu(damage);
            return Action::None;
        }
        if self.minimised && contains(self.chip(), x, y) {
            self.restore(damage);
            return Action::None;
        }
        if y < BAR_HEIGHT {
            return Action::None;
        }
        if !self.minimised && contains(self.window(), x, y) {
            self.set_focus(true, damage);
            if contains(self.minimise_control(), x, y) {
                self.minimise(damage);
            } else if contains(self.title_bar(), x, y) && !contains(self.close_control(), x, y) {
                self.drag = Some(Drag {
                    from_pointer_x: i64::from(x) - i64::from(self.window_x),
                    from_pointer_y: i64::from(y) - i64::from(self.window_y),
                });
            }
            return Action::None;
        }
        self.set_focus(false, damage);
        Action::None
    }

    /// A key went down (`pressed`) or up: the open menu's Up, Down, Enter and Esc.
    pub fn key(&mut self, code: u16, pressed: bool, damage: &mut Damage) -> Action {
        if !self.menu_open || !pressed {
            return Action::None;
        }
        let last = MENU_ITEMS.len() - 1;
        match code {
            key::ESC => self.close_menu(damage),
            key::UP => {
                self.menu_hover = Some(self.menu_hover.map_or(last, |item| item.saturating_sub(1)));
                damage.add(self.menu());
            }
            key::DOWN => {
                self.menu_hover = Some(self.menu_hover.map_or(0, |item| (item + 1).min(last)));
                damage.add(self.menu());
            }
            key::ENTER | key::KEYPAD_ENTER => {
                if let Some(item) = self.menu_hover {
                    self.close_menu(damage);
                    return MENU_ITEMS[item].1;
                }
            }
            _ => {}
        }
        Action::None
    }

    fn open_menu(&mut self, damage: &mut Damage) {
        self.menu_open = true;
        self.menu_hover = self.menu_item_at(self.pointer.0, self.pointer.1);
        damage.add(self.menu_with_shadow());
        damage.add(self.power_button());
    }

    fn close_menu(&mut self, damage: &mut Damage) {
        self.menu_open = false;
        self.menu_hover = None;
        damage.add(self.menu_with_shadow());
        damage.add(self.power_button());
    }

    fn set_focus(&mut self, focused: bool, damage: &mut Damage) {
        if self.focused != focused {
            self.focused = focused;
            if !self.minimised {
                damage.add(self.window());
            }
        }
    }

    /// Hides the window and shows its chip in the bar.
    pub fn minimise(&mut self, damage: &mut Damage) {
        if self.minimised {
            return;
        }
        self.minimised = true;
        self.drag = None;
        damage.add(self.window_with_shadow());
        damage.add(self.chip());
    }

    /// Shows the minimised window again where it was, focused, and removes its chip.
    pub fn restore(&mut self, damage: &mut Damage) {
        if !self.minimised {
            return;
        }
        self.minimised = false;
        self.focused = true;
        damage.add(self.window_with_shadow());
        damage.add(self.chip());
    }

    /// Paints the part of the desktop inside `clip` into `pixels`, rows `stride` pixels apart,
    /// from scratch: the wallpaper, the window and its shadow, the bar and the menu. The
    /// terminal's grid is left in the terminal's background, for the terminal to render into.
    /// Paints nothing outside the screen or `pixels`.
    pub fn draw(&self, pixels: &mut [u32], stride: usize, clip: Rect) {
        let clip = clip.clipped(self.width, self.height);
        let Some(mut canvas) = Canvas::new(pixels, stride, clip) else {
            return;
        };
        self.draw_wallpaper(&mut canvas);
        if !self.minimised {
            canvas.shadow(self.window());
            self.draw_window(&mut canvas);
        }
        self.draw_bar(&mut canvas);
        if self.menu_open {
            canvas.shadow(self.menu());
            self.draw_menu(&mut canvas);
        }
    }

    fn draw_wallpaper(&self, canvas: &mut Canvas<'_>) {
        let clip = canvas.clip();
        let (yolk_x, yolk_y, yolk_radius) = self.yolk;
        for row in clip.y..clip.bottom() {
            canvas.row(row, clip.x, clip.right(), self.gradient[row as usize]);
            let (left, right) = self.egg[row as usize];
            canvas.row(row, left, right, colour::WALLPAPER_EGG);
            let distance = row.abs_diff(yolk_y);
            if distance < yolk_radius {
                let half = isqrt(yolk_radius * yolk_radius - distance * distance);
                let (left, right) = span(yolk_x, half, self.width);
                canvas.row(row, left, right, colour::ACCENT);
            }
        }
    }

    fn draw_window(&self, canvas: &mut Canvas<'_>) {
        let window = self.window();
        let (border, title, title_text, minimise) = if self.focused {
            (
                colour::BORDER_ACTIVE,
                colour::TITLE_ACTIVE,
                colour::TEXT,
                colour::BUTTON_MINIMISE,
            )
        } else {
            (
                colour::BORDER_INACTIVE,
                colour::TITLE_INACTIVE,
                colour::TEXT_DIM,
                colour::BUTTON_INACTIVE,
            )
        };
        canvas.rounded(window, &paint::INSET_R6, true, true, border);
        let inside_x = window.x + 1;
        let inside_width = window.width - 2;
        canvas.rounded(
            Rect::new(inside_x, window.y + 1, inside_width, TITLE_HEIGHT),
            &paint::INSET_INSIDE_R6,
            true,
            false,
            title,
        );
        canvas.rounded(
            Rect::new(
                inside_x,
                window.y + 1 + TITLE_HEIGHT,
                inside_width,
                window.height - 2 - TITLE_HEIGHT,
            ),
            &paint::INSET_INSIDE_R6,
            false,
            true,
            TERM_BACKGROUND,
        );
        for (x, colour) in [(CLOSE_X, colour::BUTTON_INACTIVE), (MINIMISE_X, minimise)] {
            canvas.rounded(
                Rect::new(
                    window.x + x,
                    window.y + CONTROL_TOP,
                    CONTROL_SIZE,
                    CONTROL_SIZE,
                ),
                &paint::INSET_R6,
                true,
                true,
                colour,
            );
        }
        let title_width = WINDOW_TITLE.len() as u32 * GLYPH_WIDTH as u32;
        canvas.text(
            window.x + (window.width - title_width) / 2,
            window.y + 1 + (TITLE_HEIGHT - GLYPH_HEIGHT as u32) / 2,
            WINDOW_TITLE,
            title_text,
        );
    }

    fn draw_bar(&self, canvas: &mut Canvas<'_>) {
        canvas.fill(Rect::new(0, 0, self.width, BAR_HEIGHT - 1), colour::BAR);
        canvas.fill(
            Rect::new(0, BAR_HEIGHT - 1, self.width, 1),
            colour::BAR_EDGE,
        );
        let mark_top = (BAR_HEIGHT - BAR_MARK_HEIGHT) / 2;
        let mark_width = round_percent(72, BAR_MARK_HEIGHT);
        for row in 0..BAR_MARK_HEIGHT {
            let (left, right) = span(
                BAR_MARK_X + mark_width / 2,
                egg_half_width(BAR_MARK_HEIGHT, row),
                self.width,
            );
            canvas.row(mark_top + row, left, right, colour::ACCENT);
        }
        canvas.text(SYSTEM_NAME_X, BAR_TEXT_Y, SYSTEM_NAME, colour::TEXT);
        if self.minimised {
            let chip = self.chip();
            canvas.rounded(chip, &paint::INSET_R4, true, true, colour::BAR_HOVER);
            canvas.text(
                chip.x + CHIP_PADDING,
                BAR_TEXT_Y,
                WINDOW_TITLE,
                colour::TEXT,
            );
        }
        let clock = self.clock_rect();
        canvas.text(clock.x, clock.y, self.clock_text(), colour::TEXT_DIM);
        let button = self.power_button();
        if self.menu_open {
            canvas.fill(
                Rect::new(button.x, button.y, button.width, BAR_HEIGHT - 1),
                colour::BAR_PRESSED,
            );
        }
        let offset = (POWER_BUTTON_SIZE - POWER_ICON_SIZE) / 2;
        for (row, bits) in POWER_ICON.iter().enumerate() {
            for column in 0..POWER_ICON_SIZE {
                if bits & (1 << (POWER_ICON_SIZE - 1 - column)) != 0 {
                    canvas.pixel(
                        button.x + offset + column,
                        button.y + offset + row as u32,
                        colour::TEXT,
                    );
                }
            }
        }
    }

    fn draw_menu(&self, canvas: &mut Canvas<'_>) {
        let menu = self.menu();
        canvas.rounded(menu, &paint::INSET_R6, true, true, colour::MENU_BORDER);
        canvas.rounded(
            Rect::new(menu.x + 1, menu.y + 1, menu.width - 2, menu.height - 2),
            &paint::INSET_INSIDE_R6,
            true,
            true,
            colour::MENU,
        );
        for (index, (label, _)) in MENU_ITEMS.iter().enumerate() {
            let top = menu.y + 1 + index as u32 * MENU_ITEM_HEIGHT;
            if self.menu_hover == Some(index) {
                canvas.fill(
                    Rect::new(menu.x + 1, top + 2, menu.width - 2, MENU_ITEM_HEIGHT - 4),
                    colour::MENU_HOVER,
                );
            }
            canvas.text(
                menu.x + MENU_TEXT_X,
                top + (MENU_ITEM_HEIGHT - GLYPH_HEIGHT as u32) / 2,
                label,
                colour::TEXT,
            );
        }
    }
}

fn contains(rect: Rect, x: u32, y: u32) -> bool {
    x >= rect.x && x < rect.right() && y >= rect.y && y < rect.bottom()
}

fn round_percent(percent: u32, of: u32) -> u32 {
    ((u64::from(percent) * u64::from(of) + 50) / 100) as u32
}

fn span(centre: u32, half: u32, width: u32) -> (u32, u32) {
    (
        centre.saturating_sub(half).min(width),
        centre.saturating_add(half).min(width),
    )
}

fn gradient_pixel(row: u32, height: u32) -> u32 {
    let last = height.saturating_sub(1).max(1) as i64;
    let channel = |shift: u32| {
        let top = i64::from((colour::WALLPAPER_TOP >> shift) & 0xff);
        let bottom = i64::from((colour::WALLPAPER_BOTTOM >> shift) & 0xff);
        ((top + (bottom - top) * i64::from(row) / last) as u32) << shift
    };
    channel(16) | channel(8) | channel(0)
}

struct Writer<'a> {
    bytes: &'a mut [u8],
    length: usize,
}

impl<'a> Writer<'a> {
    fn new(bytes: &'a mut [u8]) -> Self {
        Self { bytes, length: 0 }
    }

    fn push_str(&mut self, text: &str) {
        for &byte in text.as_bytes() {
            if self.length < self.bytes.len() {
                self.bytes[self.length] = byte;
                self.length += 1;
            }
        }
    }

    fn push_number(&mut self, value: u64, minimum_digits: usize) {
        let mut digits = [0u8; 20];
        let mut count = 0;
        let mut rest = value;
        while count < minimum_digits || rest > 0 {
            digits[count] = b'0' + (rest % 10) as u8;
            rest /= 10;
            count += 1;
        }
        for &digit in digits[..count].iter().rev() {
            if self.length < self.bytes.len() {
                self.bytes[self.length] = digit;
                self.length += 1;
            }
        }
    }
}
