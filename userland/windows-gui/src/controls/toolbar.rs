//! comctl32's toolbar (section 26 of the design, D178): the [`Toolbar`] model (buttons with
//! their images, commands, states, styles and texts, the strings of `TB_ADDSTRINGW`, the image
//! lists and sizes the program set, the button the pointer holds), its layout (one row from
//! `x = 2`, `y = 4`, one button height, uniform or autosized widths) and its painting.

use super::common::{CommonPalette, Images, TextArena, plain_cells};
use super::{MAX_LABEL_CELLS, Palette, draw_fitted, outline};
use crate::surface::{Rect, Surface};
use crate::text::{CELL_HEIGHT, CELL_WIDTH, Cell};

/// The most buttons a toolbar holds.
pub const MAX_BUTTONS: usize = 64;
/// The most strings `TB_ADDSTRINGW` keeps.
pub const MAX_STRINGS: usize = 128;
/// `I_IMAGENONE`: a button without an image.
pub const IMAGE_NONE: i32 = -2;
const TEXT_UNITS: usize = 2048;
const ROW_LEFT: i32 = 2;
const ROW_TOP: i32 = 4;
const ROW_BELOW: i32 = 2;
const DEFAULT_IMAGE: (i32, i32) = (16, 16);
const DEFAULT_PADDING: (i32, i32) = (7, 6);
const SEPARATOR_WIDTH: i32 = 8;
const SEPARATOR_COLUMN: i32 = 3;
const SEPARATOR_INSET: i32 = 2;
const DIVIDER_LIGHT: u32 = 0x00ff_ffff;
const CONTENT_INSET: i32 = 4;
const IMAGE_GAP: i32 = 2;
const TEXT_PADDING: i32 = 12;
const IMAGE_PADDING: i32 = 8;
const LIST_PADDING: i32 = 16;
const CLASSIC_TEXT_ROWS: i32 = 26;

/// The toolbar's styles this layer reads.
pub mod style {
    pub const LIST: u32 = 0x1000;
    pub const NO_DIVIDER: u32 = 0x40;
}

/// `TB_SETEXTENDEDSTYLE`'s styles this layer reads.
pub mod extended {
    pub const MIXED_BUTTONS: u32 = 0x8;
}

/// A button's styles (`BTNS_*`).
pub mod button_style {
    pub const SEPARATOR: u8 = 0x1;
    pub const CHECK: u8 = 0x2;
    pub const GROUP: u8 = 0x4;
    pub const AUTOSIZE: u8 = 0x10;
    pub const SHOW_TEXT: u8 = 0x40;
}

/// A button's states (`TBSTATE_*`).
pub mod state {
    pub const CHECKED: u8 = 0x1;
    pub const PRESSED: u8 = 0x2;
    pub const ENABLED: u8 = 0x4;
    pub const HIDDEN: u8 = 0x8;
}

/// Where a button's text comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonText {
    None,
    /// String `i` of `TB_ADDSTRINGW`'s.
    String(usize),
    /// Its own text, given as a pointer and copied.
    Own,
}

/// One button as `TBBUTTON` gives it, with the width `TBIF_SIZE` set (0 for none).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Button {
    pub image: i32,
    pub command: i32,
    pub state: u8,
    pub style: u8,
    pub data: usize,
    pub string: isize,
    pub text: ButtonText,
    pub width: u16,
}

impl Button {
    const EMPTY: Button = Button {
        image: IMAGE_NONE,
        command: 0,
        state: 0,
        style: 0,
        data: 0,
        string: -1,
        text: ButtonText::None,
        width: 0,
    };

    pub fn is_separator(&self) -> bool {
        self.style & button_style::SEPARATOR != 0
    }

    pub fn is_hidden(&self) -> bool {
        self.state & state::HIDDEN != 0
    }

    pub fn is_enabled(&self) -> bool {
        self.state & state::ENABLED != 0
    }

    pub fn is_checked(&self) -> bool {
        self.state & state::CHECKED != 0
    }
}

/// The sizes every button shares: the image size, whether there is an image list, the button
/// height `bh` and the width of buttons without `BTNS_AUTOSIZE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub image: (i32, i32),
    pub images: bool,
    pub height: i32,
    pub uniform: i32,
}

/// A toolbar's buttons and settings.
#[derive(Debug, Clone)]
pub struct Toolbar {
    buttons: [Button; MAX_BUTTONS],
    count: usize,
    own: TextArena<TEXT_UNITS, MAX_BUTTONS>,
    strings: TextArena<TEXT_UNITS, MAX_STRINGS>,
    string_count: usize,
    held: Option<usize>,
    inside: bool,
    pub image_list: usize,
    pub hot_list: usize,
    pub disabled_list: usize,
    pub bitmap_size: (i32, i32),
    pub button_size: (i32, i32),
    pub padding: (i32, i32),
    pub extended_style: u32,
    pub max_text_rows: i32,
    pub bitmaps: i32,
    pub struct_size: usize,
}

impl Default for Toolbar {
    fn default() -> Self {
        Self::new()
    }
}

impl Toolbar {
    pub const fn new() -> Self {
        Self {
            buttons: [Button::EMPTY; MAX_BUTTONS],
            count: 0,
            own: TextArena::new(),
            strings: TextArena::new(),
            string_count: 0,
            held: None,
            inside: false,
            image_list: 0,
            hot_list: 0,
            disabled_list: 0,
            bitmap_size: DEFAULT_IMAGE,
            button_size: (0, 0),
            padding: DEFAULT_PADDING,
            extended_style: 0,
            max_text_rows: 1,
            bitmaps: 0,
            struct_size: 0,
        }
    }

    pub fn count(&self) -> usize {
        self.count
    }

    pub fn button(&self, index: usize) -> Option<&Button> {
        self.buttons[..self.count].get(index)
    }

    pub fn button_mut(&mut self, index: usize) -> Option<&mut Button> {
        self.buttons[..self.count].get_mut(index)
    }

    /// The button the pointer holds, if any.
    pub fn held(&self) -> Option<usize> {
        self.held
    }

    /// `TB_ADDSTRINGW`'s run of `\0`-separated strings ending with an empty one: the index of
    /// the first, `None` when there is no room.
    pub fn add_strings(&mut self, run: &[u16]) -> Option<usize> {
        let first = self.string_count;
        for string in run.split(|unit| *unit == 0) {
            if string.is_empty() {
                break;
            }
            if self.string_count == MAX_STRINGS || !self.strings.set(self.string_count, string) {
                return None;
            }
            self.string_count += 1;
        }
        Some(first)
    }

    /// String `index` of `TB_ADDSTRINGW`'s.
    pub fn string(&self, index: usize) -> Option<&[u16]> {
        (index < self.string_count).then(|| self.strings.get(index))
    }

    pub fn string_count(&self) -> usize {
        self.string_count
    }

    /// Button `index`'s text.
    pub fn text(&self, index: usize) -> &[u16] {
        match self.button(index).map(|button| button.text) {
            Some(ButtonText::String(string)) => self.string(string).unwrap_or(&[]),
            Some(ButtonText::Own) => self.own.get(index),
            _ => &[],
        }
    }

    /// Inserts `button` at `index` (the end when past it) with `own` as its own text when it
    /// has one; its index, `None` when the toolbar is full.
    pub fn insert(&mut self, index: usize, button: Button, own: &[u16]) -> Option<usize> {
        if self.count == MAX_BUTTONS {
            return None;
        }
        let index = index.min(self.count);
        let mut button = button;
        let own = match button.text {
            ButtonText::Own => own,
            _ => &[][..],
        };
        if !self.own.insert(index, own) {
            button.text = ButtonText::None;
        }
        self.buttons.copy_within(index..self.count, index + 1);
        self.buttons[index] = button;
        self.count += 1;
        if let Some(held) = self.held
            && held >= index
        {
            self.held = Some(held + 1);
        }
        Some(index)
    }

    /// Gives button `index` its own text; false when there is no room.
    pub fn set_text(&mut self, index: usize, units: &[u16]) -> bool {
        let Some(button) = self.button_mut(index) else {
            return false;
        };
        button.text = ButtonText::Own;
        self.own.set(index, units)
    }

    /// `TB_DELETEBUTTON`; false for no such button.
    pub fn delete(&mut self, index: usize) -> bool {
        if index >= self.count {
            return false;
        }
        self.own.remove(index);
        self.buttons.copy_within(index + 1..self.count, index);
        self.count -= 1;
        self.held = match self.held {
            Some(held) if held == index => None,
            Some(held) if held > index => Some(held - 1),
            other => other,
        };
        true
    }

    /// `TB_COMMANDTOINDEX`.
    pub fn index_of(&self, command: i32) -> Option<usize> {
        self.buttons[..self.count]
            .iter()
            .position(|button| button.command == command)
    }

    /// Checks or unchecks button `index`: checking a `BTNS_GROUP` button unchecks the group
    /// buttons next to it with no separator between.
    pub fn set_checked(&mut self, index: usize, checked: bool) {
        let Some(button) = self.button_mut(index) else {
            return;
        };
        if !checked {
            button.state &= !state::CHECKED;
            return;
        }
        button.state |= state::CHECKED;
        if button.style & button_style::GROUP == 0 {
            return;
        }
        let in_group =
            |button: &Button| button.style & button_style::GROUP != 0 && !button.is_separator();
        let (before, after) = self.buttons[..self.count].split_at_mut(index);
        let neighbours = before
            .iter_mut()
            .rev()
            .take_while(|button| in_group(button));
        let next = after[1..].iter_mut().take_while(|button| in_group(button));
        for button in neighbours.chain(next) {
            button.state &= !state::CHECKED;
        }
    }

    /// A click on button `index`: a `BTNS_CHECK` button toggles (a group button stays checked);
    /// its command, `None` when it takes no click.
    pub fn click(&mut self, index: usize) -> Option<i32> {
        let button = *self.button(index)?;
        if button.is_separator() || button.is_hidden() || !button.is_enabled() {
            return None;
        }
        if button.style & button_style::CHECK != 0 {
            let group = button.style & button_style::GROUP != 0;
            self.set_checked(index, group || !button.is_checked());
        }
        Some(button.command)
    }

    /// A left button-down on button `index`; whether it holds it now.
    pub fn press(&mut self, index: usize) -> bool {
        let takes = self.button(index).is_some_and(|button| {
            !button.is_separator() && !button.is_hidden() && button.is_enabled()
        });
        if takes {
            self.held = Some(index);
            self.inside = true;
        }
        takes
    }

    /// The pointer moved while a button is held, `inside` it or not; whether its look changed.
    pub fn pointer_moved(&mut self, inside: bool) -> bool {
        if self.held.is_none() || self.inside == inside {
            return false;
        }
        self.inside = inside;
        true
    }

    /// The release of the held button, `inside` it or not: the click's command when inside.
    pub fn release(&mut self, inside: bool) -> Option<i32> {
        let held = self.held.take()?;
        self.inside = false;
        match inside {
            true => self.click(held),
            false => None,
        }
    }

    fn shows_text(&self, index: usize) -> bool {
        let Some(button) = self.button(index) else {
            return false;
        };
        let mixed = self.extended_style & extended::MIXED_BUTTONS != 0;
        !self.text(index).is_empty()
            && self.max_text_rows != 0
            && (!mixed || button.style & button_style::SHOW_TEXT != 0)
    }

    fn shows_image(&self, index: usize, images: bool) -> bool {
        images && self.button(index).is_some_and(|button| button.image >= 0)
    }

    fn text_cells(&self, index: usize) -> i32 {
        let mut cells = [Cell::Space; MAX_LABEL_CELLS];
        plain_cells(self.text(index), &mut cells) as i32
    }

    fn visible(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.count).filter(|&index| !self.buttons[index].is_hidden())
    }

    fn natural(&self, index: usize, list: bool, image: (i32, i32), images: bool) -> i32 {
        let text = self.shows_text(index);
        let picture = self.shows_image(index, images);
        let cells = self.text_cells(index) * CELL_WIDTH;
        match (text, picture) {
            (true, false) => cells + TEXT_PADDING,
            (true, true) if list => image.0 + cells + LIST_PADDING,
            (true, true) => image.0.max(cells) + TEXT_PADDING,
            (false, _) => image.0 + IMAGE_PADDING,
        }
    }

    /// The sizes the buttons share for a toolbar of `style` whose image list's images are
    /// `image_size` (`None` without a list).
    pub fn layout(&self, style: u32, image_size: Option<(i32, i32)>) -> Layout {
        let images = image_size.is_some();
        let image = image_size.unwrap_or(self.bitmap_size);
        let list = style & style::LIST != 0;
        let single = image.1.max(CELL_HEIGHT) + 2 * CONTENT_INSET;
        let height = self
            .visible()
            .filter(|&index| !self.buttons[index].is_separator())
            .map(|index| {
                let classic = !list && self.shows_text(index) && self.shows_image(index, images);
                match classic {
                    true => image.1 + CLASSIC_TEXT_ROWS,
                    false => single,
                }
            })
            .fold(single, i32::max)
            .max(self.button_size.1);
        let uniform = self
            .visible()
            .filter(|&index| {
                let button = &self.buttons[index];
                !button.is_separator() && button.style & button_style::AUTOSIZE == 0
            })
            .map(|index| self.natural(index, list, image, images))
            .fold(0, i32::max)
            .max(self.button_size.0);
        Layout {
            image,
            images,
            height,
            uniform,
        }
    }

    /// Button `index`'s width: the one `TBIF_SIZE` set, a separator's 8 (or its positive
    /// `iBitmap`), an autosized button's natural width, every other the uniform width; 0 when
    /// hidden.
    pub fn width(&self, index: usize, style: u32, layout: &Layout) -> i32 {
        let Some(button) = self.button(index) else {
            return 0;
        };
        match () {
            () if button.is_hidden() => 0,
            () if button.width > 0 => i32::from(button.width),
            () if button.is_separator() && button.image > 0 => button.image,
            () if button.is_separator() => SEPARATOR_WIDTH,
            () if button.style & button_style::AUTOSIZE != 0 => {
                self.natural(index, style & style::LIST != 0, layout.image, layout.images)
            }
            () => layout.uniform,
        }
    }

    /// Button `index`'s rectangle in the toolbar (from (0, 0)); `None` when hidden or absent.
    pub fn button_rect(&self, index: usize, style: u32, layout: &Layout) -> Option<Rect> {
        if self.button(index)?.is_hidden() {
            return None;
        }
        let left = (0..index).fold(ROW_LEFT, |left, at| left + self.width(at, style, layout));
        Some(Rect::sized(
            left,
            ROW_TOP,
            self.width(index, style, layout),
            layout.height,
        ))
    }

    /// The buttons' total width (`TB_GETMAXSIZE`).
    pub fn total_width(&self, style: u32, layout: &Layout) -> i32 {
        (0..self.count)
            .map(|index| self.width(index, style, layout))
            .sum()
    }

    /// The toolbar's own height when docked: `bh + 6`.
    pub fn bar_height(layout: &Layout) -> i32 {
        layout.height + ROW_TOP + ROW_BELOW
    }

    /// The button (not a separator) under (`x`, `y`).
    pub fn hit(&self, x: i32, y: i32, style: u32, layout: &Layout) -> Option<usize> {
        self.visible().find(|&index| {
            !self.buttons[index].is_separator()
                && self
                    .button_rect(index, style, layout)
                    .is_some_and(|rect| rect.contains(x, y))
        })
    }
}

/// How a toolbar looks at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolbarLook {
    pub style: u32,
    pub enabled: bool,
}

fn button_colours(
    toolbar: &Toolbar,
    index: usize,
    enabled: bool,
    palette: &Palette,
    common: &CommonPalette,
) -> (Option<u32>, Option<u32>, bool) {
    let Some(button) = toolbar.button(index) else {
        return (None, None, false);
    };
    if !enabled || !button.is_enabled() {
        return (None, None, true);
    }
    let held = toolbar.held == Some(index) && toolbar.inside;
    let pressed = held || button.state & state::PRESSED != 0;
    match (pressed, button.is_checked()) {
        (true, _) => (
            Some(palette.button_pressed_fill),
            Some(palette.button_pressed_border),
            false,
        ),
        (false, true) => (
            Some(palette.button_pressed_fill),
            Some(common.button_hover_border),
            false,
        ),
        (false, false) => (None, None, false),
    }
}

fn draw_text(
    surface: &mut Surface<'_>,
    toolbar: &Toolbar,
    index: usize,
    at: (i32, i32, i32),
    colour: u32,
) {
    let (x, y, room) = at;
    let mut cells = [Cell::Space; MAX_LABEL_CELLS];
    let count = plain_cells(toolbar.text(index), &mut cells);
    draw_fitted(surface, x, y, &cells[..count], room, colour);
}

fn draw_content(
    surface: &mut Surface<'_>,
    toolbar: &Toolbar,
    index: usize,
    (rect, layout, look): (Rect, &Layout, ToolbarLook),
    (colour, greyed): (u32, bool),
    images: &dyn Images,
) {
    let text = toolbar.shows_text(index);
    let picture = toolbar.shows_image(index, layout.images);
    let (image_width, image_height) = layout.image;
    let cells = toolbar.text_cells(index) * CELL_WIDTH;
    let text_top = rect.top + (rect.height() - CELL_HEIGHT) / 2;
    let image = toolbar
        .button(index)
        .map_or(IMAGE_NONE, |button| button.image);
    let (list, from_disabled) = match greyed && toolbar.disabled_list != 0 {
        true => (toolbar.disabled_list, true),
        false => (toolbar.image_list, false),
    };
    let grey_image = greyed && !from_disabled;
    match (text, picture) {
        (true, false) => {
            let x = rect.left + (rect.width() - cells) / 2;
            let room = rect.width() - TEXT_PADDING;
            draw_text(surface, toolbar, index, (x, text_top, room), colour);
        }
        (false, true) => images.draw(
            surface,
            list,
            image,
            rect.left + (rect.width() - image_width) / 2,
            rect.top + (rect.height() - image_height) / 2,
            grey_image,
        ),
        (true, true) if look.style & style::LIST != 0 => {
            let image_left = rect.left + CONTENT_INSET;
            let image_top = rect.top + (rect.height() - image_height) / 2;
            images.draw(surface, list, image, image_left, image_top, grey_image);
            let x = image_left + image_width + CONTENT_INSET;
            let room = rect.width() - image_width - LIST_PADDING;
            draw_text(surface, toolbar, index, (x, text_top, room), colour);
        }
        (true, true) => {
            let image_left = rect.left + (rect.width() - image_width) / 2;
            let image_top = rect.top + CONTENT_INSET;
            images.draw(surface, list, image, image_left, image_top, grey_image);
            let x = rect.left + (rect.width() - cells) / 2;
            let y = rect.top + CONTENT_INSET + image_height + IMAGE_GAP;
            let room = rect.width() - TEXT_PADDING;
            draw_text(surface, toolbar, index, (x, y, room), colour);
        }
        (false, false) => {}
    }
}

/// Paints `toolbar` over the whole of `rect` (a toolbar whose top-left is (0, 0)) with the
/// images of `images`.
pub fn paint(
    surface: &mut Surface<'_>,
    rect: Rect,
    toolbar: &Toolbar,
    look: ToolbarLook,
    (palette, common): (&Palette, &CommonPalette),
    images: &dyn Images,
) {
    let saved = surface.clip();
    surface.clip_to(rect);
    surface.fill(rect, palette.surface);
    if look.style & style::NO_DIVIDER == 0 {
        surface.fill(
            Rect::new(rect.left, rect.top, rect.right, rect.top + 1),
            palette.menu_separator,
        );
        surface.fill(
            Rect::new(rect.left, rect.top + 1, rect.right, rect.top + 2),
            DIVIDER_LIGHT,
        );
    }
    let layout = toolbar.layout(look.style, images.size(toolbar.image_list));
    for index in 0..toolbar.count {
        let Some(button) = toolbar.button_rect(index, look.style, &layout) else {
            continue;
        };
        let button = button.offset(rect.left, rect.top);
        if toolbar.buttons[index].is_separator() {
            let x = button.left + SEPARATOR_COLUMN;
            surface.fill(
                Rect::new(
                    x,
                    button.top + SEPARATOR_INSET,
                    x + 1,
                    button.bottom - SEPARATOR_INSET,
                ),
                palette.menu_separator,
            );
            continue;
        }
        let (fill, border, greyed) = button_colours(toolbar, index, look.enabled, palette, common);
        if let Some(fill) = fill {
            surface.fill(button, fill);
        }
        if let Some(border) = border {
            outline(surface, button, border);
        }
        let colour = match greyed {
            true => palette.text_disabled,
            false => palette.text,
        };
        draw_content(
            surface,
            toolbar,
            index,
            (button, &layout, look),
            (colour, greyed),
            images,
        );
    }
    surface.set_clip(saved);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::LIGHT;
    use crate::controls::common::{COMMON_LIGHT, NoImages};

    fn units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    const EXAMPLE_STYLE: u32 = 0x184c;
    const AUTOSIZE: u8 = button_style::AUTOSIZE;

    fn button(command: i32, string: usize, style: u8) -> Button {
        Button {
            image: IMAGE_NONE,
            command,
            state: state::ENABLED,
            style,
            data: 0,
            string: string as isize,
            text: ButtonText::String(string),
            width: 0,
        }
    }

    fn example() -> Toolbar {
        let mut toolbar = Toolbar::new();
        let run: Vec<u16> = "Step\0Reset\0Show tree\0\0".encode_utf16().collect();
        assert_eq!(toolbar.add_strings(&run), Some(0));
        for (index, command) in [1001, 1002, 1003].into_iter().enumerate() {
            toolbar.insert(usize::MAX, button(command, index, AUTOSIZE), &[]);
        }
        toolbar
    }

    struct Squares;

    impl Images for Squares {
        fn size(&self, list: usize) -> Option<(i32, i32)> {
            (list == 1).then_some((16, 16))
        }

        fn draw(&self, surface: &mut Surface<'_>, _: usize, _: i32, x: i32, y: i32, grey: bool) {
            let colour = if grey { 0x00a6_a6a6 } else { 0x00ff_0000 };
            surface.fill(Rect::sized(x, y, 16, 16), colour);
        }
    }

    fn render(toolbar: &Toolbar, style: u32, images: &dyn Images) -> Vec<u32> {
        let mut pixels = vec![0u32; 480 * 60];
        let mut surface = Surface::new(&mut pixels, 480, 60).unwrap();
        let look = ToolbarLook {
            style,
            enabled: true,
        };
        paint(
            &mut surface,
            Rect::new(0, 0, 480, 30),
            toolbar,
            look,
            (&LIGHT, &COMMON_LIGHT),
            images,
        );
        pixels
    }

    fn text_at(pixels: &[u32], text: &str, x: i32, y: i32, colour: u32) -> bool {
        let mut reference = vec![1u32; 128 * 16];
        let mut surface = Surface::new(&mut reference, 128, 16).unwrap();
        let mut cells = [Cell::Space; MAX_LABEL_CELLS];
        let count = plain_cells(&units(text), &mut cells);
        crate::text::draw_cells(&mut surface, 0, 0, &cells[..count], 0, None);
        (0..16).all(|row| {
            (0..128).all(|column| {
                let ink = reference[(row * 128 + column) as usize] == 0;
                let at = pixels[((y + row) * 480 + x + column) as usize];
                !ink || at == colour
            })
        })
    }

    #[test]
    fn the_examples_buttons_are_sized_and_placed_as_the_design_says() {
        let toolbar = example();
        let layout = toolbar.layout(EXAMPLE_STYLE, None);
        assert_eq!(layout.height, 24);
        assert_eq!(Toolbar::bar_height(&layout), 30);
        let rects: Vec<Rect> = (0..3)
            .map(|index| toolbar.button_rect(index, EXAMPLE_STYLE, &layout).unwrap())
            .collect();
        assert_eq!(
            rects,
            [
                Rect::sized(2, 4, 44, 24),
                Rect::sized(46, 4, 52, 24),
                Rect::sized(98, 4, 84, 24)
            ]
        );
        let pixels = render(&toolbar, EXAMPLE_STYLE, &NoImages);
        assert!(
            pixels[..480 * 30]
                .iter()
                .all(|&pixel| pixel == 0x00f0_f0f0 || pixel == 0)
        );
        assert!(text_at(&pixels, "Step", 8, 8, 0));
        assert!(text_at(&pixels, "Reset", 52, 8, 0));
        assert!(text_at(&pixels, "Show tree", 104, 8, 0));
        assert_eq!(toolbar.hit(24, 16, EXAMPLE_STYLE, &layout), Some(0));
        assert_eq!(toolbar.total_width(EXAMPLE_STYLE, &layout), 180);
    }

    #[test]
    fn a_held_button_is_pressed_and_a_release_inside_clicks() {
        let mut toolbar = example();
        assert!(toolbar.press(0));
        let pixels = render(&toolbar, EXAMPLE_STYLE, &NoImages);
        let at = |x: i32, y: i32| pixels[(y * 480 + x) as usize];
        assert_eq!(at(2, 4), 0x0000_5499);
        assert_eq!(at(45, 27), 0x0000_5499);
        assert_eq!(at(3, 5), 0x00cc_e4f7);
        assert_eq!(at(46, 10), 0x00f0_f0f0);
        assert!(toolbar.pointer_moved(false));
        assert_eq!(toolbar.release(false), None);
        assert!(toolbar.press(0));
        assert_eq!(toolbar.release(true), Some(1001));
        assert_eq!(toolbar.held(), None);
    }

    #[test]
    fn check_groups_uncheck_their_neighbours_up_to_a_separator() {
        let mut toolbar = Toolbar::new();
        let group = button_style::CHECK | button_style::GROUP;
        for command in 0..3 {
            toolbar.insert(usize::MAX, button(command, 0, group), &[]);
        }
        let mut separator = button(9, 0, button_style::SEPARATOR);
        separator.image = 0;
        toolbar.insert(usize::MAX, separator, &[]);
        toolbar.insert(usize::MAX, button(4, 0, group), &[]);
        toolbar.set_checked(4, true);
        assert_eq!(toolbar.click(0), Some(0));
        assert!(toolbar.button(0).unwrap().is_checked());
        assert_eq!(toolbar.click(2), Some(2));
        assert!(!toolbar.button(0).unwrap().is_checked());
        assert!(toolbar.button(2).unwrap().is_checked());
        assert!(toolbar.button(4).unwrap().is_checked());
        toolbar.click(2);
        assert!(toolbar.button(2).unwrap().is_checked());
        let mut plain = Toolbar::new();
        plain.insert(0, button(7, 0, button_style::CHECK), &[]);
        plain.click(0);
        assert!(plain.button(0).unwrap().is_checked());
        plain.click(0);
        assert!(!plain.button(0).unwrap().is_checked());
    }

    #[test]
    fn widths_follow_the_four_layouts_and_autosize() {
        let mut toolbar = Toolbar::new();
        let mut with_image = button(1, 0, 0);
        with_image.image = 0;
        with_image.text = ButtonText::Own;
        toolbar.insert(0, with_image, &units("Open"));
        let mut image_only = button(2, 0, 0);
        image_only.image = 1;
        image_only.text = ButtonText::None;
        toolbar.insert(1, image_only, &[]);
        let list = toolbar.layout(style::LIST, Some((16, 16)));
        assert_eq!(list.height, 24);
        assert_eq!(list.uniform, 16 + 32 + 16);
        let classic = toolbar.layout(0, Some((16, 16)));
        assert_eq!(classic.height, 42);
        assert_eq!(classic.uniform, 32 + 12);
        let none = toolbar.layout(0, None);
        assert_eq!(none.uniform, 32 + 12);
        toolbar.button_mut(1).unwrap().style = button_style::AUTOSIZE;
        let classic = toolbar.layout(0, Some((16, 16)));
        assert_eq!(toolbar.width(1, 0, &classic), 24);
        toolbar.button_size = (60, 50);
        let raised = toolbar.layout(0, Some((16, 16)));
        assert_eq!((raised.uniform, raised.height), (60, 50));
        toolbar.button_mut(0).unwrap().state |= state::HIDDEN;
        let layout = toolbar.layout(0, Some((16, 16)));
        assert_eq!(toolbar.button_rect(0, 0, &layout), None);
        assert_eq!(
            toolbar.button_rect(1, 0, &layout).map(|rect| rect.left),
            Some(2)
        );
    }

    #[test]
    fn states_disable_check_and_grey_images() {
        let mut toolbar = Toolbar::new();
        let mut first = button(1, 0, 0);
        first.image = 0;
        first.text = ButtonText::None;
        toolbar.insert(0, first, &[]);
        let mut second = first;
        second.state = 0;
        toolbar.insert(1, second, &[]);
        let mut separator = button(0, 0, button_style::SEPARATOR);
        separator.image = 0;
        toolbar.insert(1, separator, &[]);
        toolbar.image_list = 1;
        toolbar.set_checked(0, true);
        let pixels = render(&toolbar, 0, &Squares);
        let at = |x: i32, y: i32| pixels[(y * 480 + x) as usize];
        assert_eq!(at(0, 0), 0x00d7_d7d7);
        assert_eq!(at(0, 1), 0x00ff_ffff);
        assert_eq!(at(2, 4), 0x0000_78d7);
        assert_eq!(at(3, 5), 0x00cc_e4f7);
        assert_eq!(at(6, 8), 0x00ff_0000);
        assert_eq!(at(29, 6), 0x00d7_d7d7);
        assert_eq!(at(29, 5), 0x00f0_f0f0);
        assert_eq!(at(29, 26), 0x00f0_f0f0);
        assert_eq!(at(38, 8), 0x00a6_a6a6);
        assert_eq!(at(34, 4), 0x00f0_f0f0);
        assert_eq!(toolbar.click(2), None);
    }
}
