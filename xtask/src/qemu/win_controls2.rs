//! The second Windows controls boot test's checks of the display (D228): `win-controls2.exe`'s
//! window, a click at the end of its word edit, Shift+Left selecting the last word and a letter
//! typed over it, Tab into the password edit and the digits-only edit with letters typed, Tab
//! landing on the group's selected radio button and Down selecting the next one, a click on the
//! scroll bar's right arrow, and Esc closing the window, each frame compared with the desktop
//! the host renders, the window's content painted by `eggos_windows_gui` from the same models.

use super::{
    Desktop, DiscardEvents, TERMINAL_FRAME_TIMEOUT, WindowContent, apply_pointer_step,
    check_desktop_frame_within, click_steps, send_pointer_step,
};
use crate::Arch;
use crate::qmp::{InputEvent, Qmp};
use eggos_drivers::desktop::WindowId;
use eggos_drivers::display::Damage;
use eggos_drivers::input::keymap::Modifiers as DesktopModifiers;
use eggos_windows_gui::colour;
use eggos_windows_gui::controls::LIGHT;
use eggos_windows_gui::controls::button::{self, ButtonKind, ButtonLook};
use eggos_windows_gui::controls::edit::{self, Edit, EditLook, Modifiers};
use eggos_windows_gui::controls::scroll_bar::{self, BarLook, ScrollBar, ScrollInfo};
use eggos_windows_gui::surface::{Rect, Surface};
use eggos_windows_gui_tests::controls::Control;
use eggos_windows_gui_tests::controls2 as scene;
use std::sync::mpsc;
use std::time::Duration;

const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(30);
const CLIENT: u16 = 0;
const ROLE_NORMAL: u32 = 0;
const KEY_PAUSE: Duration = Duration::from_millis(100);
const BUTTON_FACE: u32 = 15;
const ARROW: u32 = 0;
const I_BEAM: u32 = 1;
const VK_LEFT: u32 = 0x25;
const SHIFT: (&str, u16) = ("shift", 42);
const LEFT: (&str, u16) = ("left", 105);
const TAB: (&str, u16) = ("tab", 15);
const DOWN: (&str, u16) = ("down", 108);
const ESCAPE: (&str, u16) = ("esc", 1);
const KEYS: [(char, &str, u16); 4] = [
    ('e', "e", 18),
    ('g', "g", 34),
    ('x', "x", 45),
    ('1', "1", 2),
];

/// Which control has the keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Word,
    Password,
    Number,
    Large,
    Bar,
}

/// The window's state at one moment: the three edits' models, the selected radio, the bar's
/// position, the focus and whether the pointer rests on the bar's right arrow rather than the
/// word edit (each draws hovered, D228).
#[derive(Clone)]
struct Frame {
    word: Box<Edit>,
    password: Box<Edit>,
    number: Box<Edit>,
    large: bool,
    position: i32,
    focus: Focus,
    over_bar: bool,
}

fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

fn rect_of(control: &Control) -> Rect {
    let (x, y, width, height) = control.rect;
    Rect::sized(x, y, width, height)
}

fn visible(control: &Control) -> usize {
    edit::visible_cells(control.rect.2)
}

impl Frame {
    fn started() -> Frame {
        let mut word = Box::new(Edit::with_style(scene::WORD.style));
        word.set_text(&units(scene::WORD_TEXT));
        word.caret_to_end(visible(&scene::WORD));
        Frame {
            word,
            password: Box::new(Edit::with_style(scene::PASSWORD.style)),
            number: Box::new(Edit::with_style(scene::NUMBER.style)),
            large: false,
            position: 0,
            focus: Focus::Word,
            over_bar: false,
        }
    }

    fn typed(edit: &mut Edit, control: &Control, text: &str) {
        for unit in text.encode_utf16() {
            edit.character(u32::from(unit), visible(control));
        }
    }
}

fn at(surface: &mut Surface<'_>, rect: Rect, paint: impl FnOnce(&mut Surface<'_>, Rect)) {
    let saved = surface.clip();
    surface.translate(rect.left, rect.top);
    surface.clip_to(Rect::new(0, 0, rect.width(), rect.height()));
    paint(surface, Rect::new(0, 0, rect.width(), rect.height()));
    surface.translate(-rect.left, -rect.top);
    surface.set_clip(saved);
}

fn paint_edit(
    surface: &mut Surface<'_>,
    control: &Control,
    model: &Edit,
    (focused, hovered): (bool, bool),
) {
    at(surface, rect_of(control), |surface, local| {
        let look = EditLook {
            focused,
            enabled: true,
            hovered,
        };
        edit::paint(surface, local, model, look, &LIGHT)
    });
}

fn paint_button(
    surface: &mut Surface<'_>,
    control: &Control,
    kind: ButtonKind,
    checked: bool,
    focused: bool,
) {
    at(surface, rect_of(control), |surface, local| {
        let look = ButtonLook {
            kind,
            checked,
            pressed: false,
            focused,
            default: false,
            enabled: true,
            hovered: false,
        };
        button::paint(surface, local, &units(control.text), look, &LIGHT)
    });
}

fn bar_at(position: i32) -> ScrollBar {
    let (minimum, maximum, page) = scene::BAR_RANGE;
    let mut bar = ScrollBar::new(false);
    bar.set_info(
        scroll_bar::mask::RANGE | scroll_bar::mask::PAGE | scroll_bar::mask::POSITION,
        &ScrollInfo {
            minimum,
            maximum,
            page,
            position,
        },
    );
    bar
}

/// The window's content in `frame`.
fn window_content(frame: &Frame) -> WindowContent {
    let (width, height) = (scene::CONTENT.0 as u32, scene::CONTENT.1 as u32);
    let face = colour::system_colour(BUTTON_FACE).expect("COLOR_BTNFACE");
    let mut pixels = vec![face; (width * height) as usize];
    let mut surface = Surface::new(&mut pixels, width, height).expect("the content fits");
    paint_edit(
        &mut surface,
        &scene::WORD,
        &frame.word,
        (frame.focus == Focus::Word, !frame.over_bar),
    );
    paint_edit(
        &mut surface,
        &scene::PASSWORD,
        &frame.password,
        (frame.focus == Focus::Password, false),
    );
    paint_edit(
        &mut surface,
        &scene::NUMBER,
        &frame.number,
        (frame.focus == Focus::Number, false),
    );
    paint_button(
        &mut surface,
        &scene::SIZE_BOX,
        ButtonKind::GroupBox,
        false,
        false,
    );
    paint_button(
        &mut surface,
        &scene::SMALL,
        ButtonKind::AutoRadio,
        !frame.large,
        false,
    );
    paint_button(
        &mut surface,
        &scene::LARGE,
        ButtonKind::AutoRadio,
        frame.large,
        frame.focus == Focus::Large,
    );
    let mut bar = bar_at(frame.position);
    let (bar_x, _, bar_width, _) = scene::BAR.rect;
    bar.hover(Some(scene::BAR_RIGHT_ARROW.0 - bar_x), bar_width);
    at(&mut surface, rect_of(&scene::BAR), |surface, local| {
        let look = BarLook {
            enabled: true,
            focused: frame.focus == Focus::Bar,
            hovered: frame.over_bar,
        };
        scroll_bar::paint(surface, local, &bar, look, &LIGHT)
    });
    WindowContent::Pixels { width, pixels }
}

struct Steps<'a> {
    arch: &'a Arch,
    qmp: &'a mut Qmp,
    desktop: &'a mut Desktop,
    console: &'a mut Vec<u8>,
    output: &'a mpsc::Receiver<Vec<u8>>,
    screen: (u32, u32),
    window: WindowId,
    frame: Frame,
}

impl Steps<'_> {
    fn check(&mut self, at: &str, timeout: Duration) -> Result<(), String> {
        let windows = [(self.window, window_content(&self.frame))];
        check_desktop_frame_within(
            self.arch,
            self.qmp,
            self.console,
            self.output,
            at,
            self.desktop,
            &windows,
            timeout,
        )
    }

    fn refused(&self, what: &str, error: impl std::fmt::Debug) -> String {
        format!(
            "{}: the host's desktop refused {what}: {error:?}",
            self.arch.name
        )
    }

    fn create(&mut self) -> Result<WindowId, String> {
        let (width, height) = scene::CONTENT;
        let created = self
            .desktop
            .create(CLIENT, ROLE_NORMAL, 0, width as u32, height as u32, 0)
            .map_err(|error| self.refused("a window", error))?;
        let id = u32::from(created.window);
        self.desktop
            .set_title(CLIENT, id, scene::TITLE.as_bytes(), &mut Damage::new())
            .map_err(|error| self.refused("a title", error))?;
        self.desktop
            .commit(CLIENT, id, &[], &mut Damage::new(), &mut DiscardEvents)
            .map_err(|error| self.refused("a commit", error))?;
        Ok(created.window)
    }

    fn click(&mut self, (x, y): (i32, i32)) -> Result<(), String> {
        let content = self
            .desktop
            .content_rect(self.window)
            .ok_or_else(|| format!("{}: the window {} is gone", self.arch.name, self.window))?;
        for step in click_steps(content.x + x as u32, content.y + y as u32) {
            send_pointer_step(self.qmp, step, self.screen)?;
            apply_pointer_step(self.desktop, step);
        }
        Ok(())
    }

    fn key(&mut self, (qcode, code): (&'static str, u16), down: bool) -> Result<(), String> {
        self.qmp.send_input(&[InputEvent::Key { qcode, down }])?;
        std::thread::sleep(KEY_PAUSE);
        self.desktop.key(
            code,
            down,
            DesktopModifiers::NONE,
            &mut Damage::new(),
            &mut DiscardEvents,
        );
        Ok(())
    }

    fn tap(&mut self, key: (&'static str, u16)) -> Result<(), String> {
        self.key(key, true)?;
        self.key(key, false)
    }

    fn type_text(&mut self, text: &str) -> Result<(), String> {
        for letter in text.chars() {
            let (_, qcode, code) = KEYS
                .iter()
                .find(|(known, _, _)| *known == letter)
                .ok_or_else(|| format!("no key for {letter}"))?;
            self.tap((qcode, *code))?;
        }
        Ok(())
    }

    fn pointer_shape(&mut self, shape: u32) -> Result<(), String> {
        self.desktop
            .set_pointer(CLIENT, u32::from(self.window), shape, &mut Damage::new())
            .map_err(|error| self.refused("a pointer shape", error))
    }

    fn run(&mut self) -> Result<(), String> {
        self.click(scene::WORD_END)?;
        self.pointer_shape(I_BEAM)?;
        self.check(
            "win-controls2's window, its word edit clicked at the end",
            FIRST_FRAME_TIMEOUT,
        )?;

        self.key(SHIFT, true)?;
        for _ in 0..3 {
            self.tap(LEFT)?;
            let held = Modifiers {
                shift: true,
                control: false,
            };
            self.frame
                .word
                .key_with(VK_LEFT, held, visible(&scene::WORD));
        }
        self.key(SHIFT, false)?;
        self.check("nog selected by Shift+Left", TERMINAL_FRAME_TIMEOUT)?;

        self.type_text("x")?;
        Frame::typed(&mut self.frame.word, &scene::WORD, "x");
        self.check("x typed over the selection", TERMINAL_FRAME_TIMEOUT)?;

        self.tap(TAB)?;
        self.type_text("egg")?;
        Frame::typed(&mut self.frame.password, &scene::PASSWORD, "egg");
        self.frame.focus = Focus::Password;
        self.check("egg typed into the password edit", TERMINAL_FRAME_TIMEOUT)?;

        self.tap(TAB)?;
        self.type_text("e1")?;
        Frame::typed(&mut self.frame.number, &scene::NUMBER, "e1");
        self.frame.focus = Focus::Number;
        self.check(
            "the digits-only edit refusing e and taking 1",
            TERMINAL_FRAME_TIMEOUT,
        )?;

        self.tap(TAB)?;
        self.tap(DOWN)?;
        self.frame.large = true;
        self.frame.focus = Focus::Large;
        self.check(
            "Tab onto Small and Down selecting Large",
            TERMINAL_FRAME_TIMEOUT,
        )?;

        self.click(scene::BAR_RIGHT_ARROW)?;
        self.pointer_shape(ARROW)?;
        self.frame.over_bar = true;
        self.frame.position = 1;
        self.frame.focus = Focus::Bar;
        self.check(
            "the bar's right arrow clicked: a line on",
            TERMINAL_FRAME_TIMEOUT,
        )?;

        self.tap(ESCAPE)?;
        self.desktop
            .destroy(
                CLIENT,
                u32::from(self.window),
                &mut Damage::new(),
                &mut DiscardEvents,
            )
            .map_err(|error| self.refused("to destroy a window", error))?;
        check_desktop_frame_within(
            self.arch,
            self.qmp,
            self.console,
            self.output,
            "win-controls2 closed by Esc",
            self.desktop,
            &[],
            TERMINAL_FRAME_TIMEOUT,
        )
    }
}

/// Checks the display through the whole of `win-controls2.exe`'s run, driving it through QMP,
/// on the desktop `win-controls.exe`'s check left.
pub(super) fn check(
    arch: &Arch,
    qmp: &mut Qmp,
    desktop: &mut Desktop,
    console: &mut Vec<u8>,
    output: &mpsc::Receiver<Vec<u8>>,
    screen: (u32, u32),
) -> Result<(), String> {
    let mut steps = Steps {
        arch,
        qmp,
        desktop,
        console,
        output,
        screen,
        window: WindowId::default(),
        frame: Frame::started(),
    };
    steps.window = steps.create()?;
    steps.run()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixels_of(frame: &Frame) -> (u32, Vec<u32>) {
        match window_content(frame) {
            WindowContent::Pixels { width, pixels } => (width, pixels),
            WindowContent::Checkerboard(_) => panic!("the content is pixels"),
        }
    }

    #[test]
    fn the_window_shows_its_edits_group_radios_and_bar_as_designed() {
        let mut frame = Frame::started();
        let (width, pixels) = pixels_of(&frame);
        let at = |pixels: &[u32], x: i32, y: i32| pixels[(y as u32 * width + x as u32) as usize];
        assert_eq!(pixels.len(), 400 * 160);
        assert_eq!(at(&pixels, 16, 16), LIGHT.field_focus_border);
        assert_eq!(at(&pixels, 16, 48), LIGHT.field_border);
        assert_eq!(at(&pixels, 232, 23), LIGHT.group_border);
        assert_eq!(at(&pixels, 249, 48), LIGHT.box_mark);
        assert_eq!(at(&pixels, 249, 80), LIGHT.box_fill);
        frame.large = true;
        frame.focus = Focus::Bar;
        frame.position = 1;
        frame.over_bar = true;
        let (_, moved) = pixels_of(&frame);
        assert_eq!(at(&moved, 383, 121), LIGHT.scroll_arrow_hover_fill);
        frame.focus = Focus::Number;
        frame.over_bar = false;
        let (_, typed) = pixels_of(&frame);
        assert_eq!(at(&typed, 16, 16), LIGHT.field_hover_border);
        assert_eq!(at(&moved, 249, 48), LIGHT.box_fill);
        assert_eq!(at(&moved, 249, 80), LIGHT.box_mark);
        assert_ne!(pixels, moved);
    }

    #[test]
    fn shift_left_three_times_selects_the_last_word_and_x_replaces_it() {
        let mut frame = Frame::started();
        let held = Modifiers {
            shift: true,
            control: false,
        };
        for _ in 0..3 {
            frame
                .word
                .key_with(VK_LEFT, held, visible(&scene::WORD));
        }
        assert_eq!(frame.word.selection(), (4, 7));
        Frame::typed(&mut frame.word, &scene::WORD, "x");
        assert_eq!(frame.word.text(), units("egg x").as_slice());
        Frame::typed(&mut frame.number, &scene::NUMBER, "e1");
        assert_eq!(frame.number.text(), units("1").as_slice());
    }
}
