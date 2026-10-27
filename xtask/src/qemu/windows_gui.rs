//! The Windows GUI boot test's checks of the display (D129): `win-paint.exe`'s window, the
//! click, the key, the message box and the close, each compared with the desktop the host
//! renders with the window's content painted by `eggos_windows_gui` from the program's own
//! scene.

use super::{
    Desktop, DiscardEvents, TERMINAL_FRAME_TIMEOUT, WindowContent, apply_pointer_step,
    check_desktop_frame_within, click_steps, screen_size, send_pointer_step,
};
use crate::Arch;
use crate::qmp::{InputEvent, Qmp};
use eggos_drivers::desktop::WindowId;
use eggos_drivers::display::Damage;
use eggos_drivers::input::key;
use eggos_drivers::input::keymap::Modifiers;
use eggos_windows_gui::colour;
use eggos_windows_gui::message_box::{self, Layout, Look};
use eggos_windows_gui::shapes;
use eggos_windows_gui::surface::{Rect, Surface};
use eggos_windows_gui::text::{self, Cell, Reading};
use eggos_windows_gui_tests::{self as scene, Painter, Pen, Scene};
use std::sync::mpsc;
use std::time::Duration;

const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(30);
const CLIENT: u16 = 0;
const ROLE_NORMAL: u32 = 0;
const ROLE_DIALOG: u32 = 1;
const KEY_PAUSE: Duration = Duration::from_millis(100);

struct HostPainter<'s, 'p> {
    surface: &'s mut Surface<'p>,
}

fn rect(rect: scene::Rect) -> Rect {
    Rect::new(rect.left, rect.top, rect.right, rect.bottom)
}

fn pen(pen: Pen) -> shapes::Pen {
    shapes::Pen {
        colour: colour::pixel(pen.colour),
        width: pen.width.max(1) as u32,
    }
}

fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

impl Painter for HostPainter<'_, '_> {
    fn fill(&mut self, area: scene::Rect, fill: u32) {
        self.surface.fill(rect(area), colour::pixel(fill));
    }

    fn rectangle(&mut self, area: scene::Rect, outline: Pen, brush: Option<u32>) {
        shapes::rectangle(
            self.surface,
            rect(area),
            Some(pen(outline)),
            brush.map(colour::pixel),
        );
    }

    fn ellipse(&mut self, area: scene::Rect, outline: Pen, brush: Option<u32>) {
        shapes::ellipse(
            self.surface,
            rect(area),
            Some(pen(outline)),
            brush.map(colour::pixel),
        );
    }

    fn line(&mut self, from: (i32, i32), to: (i32, i32), outline: Pen) {
        shapes::line(self.surface, from, to, pen(outline));
    }

    fn pixel(&mut self, x: i32, y: i32, dot: u32) {
        self.surface.pixel(x, y, colour::pixel(dot));
    }

    fn text(&mut self, x: i32, y: i32, words: &str, ink: u32, background: Option<u32>) {
        text::text_out(
            self.surface,
            x,
            y,
            &units(words),
            colour::pixel(ink),
            background.map(colour::pixel),
        );
    }

    fn text_in(
        &mut self,
        area: scene::Rect,
        words: &str,
        format: u32,
        ink: u32,
        background: Option<u32>,
    ) {
        let mut cells = vec![Cell::Space; words.len()];
        let filled = text::decode(&units(words), text::reading(format), &mut cells);
        text::draw_text(
            self.surface,
            &cells[..filled],
            rect(area),
            format,
            colour::pixel(ink),
            background.map(colour::pixel),
        );
    }

    fn text_width(&mut self, words: &str) -> i32 {
        text::extent(&units(words)).0
    }
}

fn scene_content(state: &Scene) -> WindowContent {
    let (width, height) = (scene::CLIENT.0 as u32, scene::CLIENT.1 as u32);
    let mut pixels = vec![colour::pixel(scene::WINDOW_COLOUR); (width * height) as usize];
    let mut surface = Surface::new(&mut pixels, width, height).expect("the scene fits");
    state.paint(&mut HostPainter {
        surface: &mut surface,
    });
    WindowContent::Pixels { width, pixels }
}

fn message_cells() -> Vec<Cell> {
    let message = units(scene::MESSAGE);
    let mut cells = vec![Cell::Space; message.len()];
    let filled = text::decode(&message, Reading::default(), &mut cells);
    cells.truncate(filled);
    cells
}

fn message_box_content(layout: &Layout, cells: &[Cell]) -> WindowContent {
    let (width, height) = (layout.width as u32, layout.height as u32);
    let mut pixels = vec![0; (width * height) as usize];
    let mut surface = Surface::new(&mut pixels, width, height).expect("the box fits");
    let look = Look {
        focused: layout.initial_focus(),
        ..Look::default()
    };
    message_box::paint(&mut surface, layout, cells, look);
    WindowContent::Pixels { width, pixels }
}

struct GuiSteps<'a> {
    arch: &'a Arch,
    qmp: Qmp,
    desktop: Desktop,
    console: &'a mut Vec<u8>,
    output: &'a mpsc::Receiver<Vec<u8>>,
    screen: (u32, u32),
    scene: Scene,
    window: WindowId,
    message_box: Option<(WindowId, WindowContent)>,
}

impl GuiSteps<'_> {
    fn check(&mut self, at: &str, timeout: Duration) -> Result<(), String> {
        let mut windows = vec![(self.window, scene_content(&self.scene))];
        windows.extend(self.message_box.clone());
        check_desktop_frame_within(
            self.arch,
            &mut self.qmp,
            self.console,
            self.output,
            at,
            &self.desktop,
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

    fn create(&mut self, role: u32, parent: u32, size: (u32, u32)) -> Result<WindowId, String> {
        let created = self
            .desktop
            .create(CLIENT, role, parent, size.0, size.1, 0)
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

    fn destroy(&mut self, window: WindowId) -> Result<(), String> {
        self.desktop
            .destroy(
                CLIENT,
                u32::from(window),
                &mut Damage::new(),
                &mut DiscardEvents,
            )
            .map_err(|error| self.refused("to destroy a window", error))
    }

    fn click(&mut self, x: u32, y: u32) -> Result<(), String> {
        for step in click_steps(x, y) {
            send_pointer_step(&mut self.qmp, step, self.screen)?;
            apply_pointer_step(&mut self.desktop, step);
        }
        Ok(())
    }

    fn tap(&mut self, qcode: &'static str, code: u16) -> Result<(), String> {
        for down in [true, false] {
            self.qmp.send_input(&[InputEvent::Key { qcode, down }])?;
            std::thread::sleep(KEY_PAUSE);
            self.desktop.key(
                code,
                down,
                Modifiers::NONE,
                &mut Damage::new(),
                &mut DiscardEvents,
            );
        }
        Ok(())
    }

    fn content(&self, window: WindowId) -> Result<eggos_drivers::display::Rect, String> {
        self.desktop
            .content_rect(window)
            .ok_or_else(|| format!("{}: the window {window} is gone", self.arch.name))
    }

    fn run(&mut self) -> Result<(), String> {
        self.check("win-paint's window painted", FIRST_FRAME_TIMEOUT)?;

        let content = self.content(self.window)?;
        let (x, y) = self.scene.ellipse_centre();
        self.click(content.x + x as u32, content.y + y as u32)?;
        self.scene.clicked = true;
        self.check("the ellipse recoloured by a click", TERMINAL_FRAME_TIMEOUT)?;

        self.tap("a", key::A)?;
        self.scene.type_character(u32::from(b'a'));
        self.check("a letter typed into the window", TERMINAL_FRAME_TIMEOUT)?;

        self.tap("spc", key::SPACE)?;
        let cells = message_cells();
        let work_width = self.desktop.work_area().width as i32;
        let layout = Layout::new(&cells, scene::MESSAGE_TYPE, work_width);
        let message_box = self.create(
            ROLE_DIALOG,
            u32::from(self.window),
            (layout.width as u32, layout.height as u32),
        )?;
        self.message_box = Some((message_box, message_box_content(&layout, &cells)));
        self.check("the message box over the window", TERMINAL_FRAME_TIMEOUT)?;

        self.tap("ret", key::ENTER)?;
        self.destroy(message_box)?;
        self.message_box = None;
        self.scene.message_box_result = Some(message_box::result::OK);
        self.check("the message box answered with OK", TERMINAL_FRAME_TIMEOUT)?;

        let close = self.desktop.close_control(self.window).unwrap_or_default();
        self.click(close.x + 10, close.y + 14)?;
        self.destroy(self.window)?;
        let mut windows = Vec::new();
        windows.extend(self.message_box.clone());
        check_desktop_frame_within(
            self.arch,
            &mut self.qmp,
            self.console,
            self.output,
            "win-paint's window closed by its close control",
            &self.desktop,
            &windows,
            TERMINAL_FRAME_TIMEOUT,
        )
    }
}

/// Checks the display through the whole of `win-paint.exe`'s run, driving it through QMP.
pub fn check(
    arch: &Arch,
    port: u16,
    console: &mut Vec<u8>,
    output: &mpsc::Receiver<Vec<u8>>,
) -> Result<(), String> {
    let mut qmp = Qmp::connect(port)?;
    let screen = screen_size(&mut qmp, arch)?;
    let desktop = Desktop::new(screen.0, screen.1).ok_or_else(|| {
        format!(
            "{}: a {}x{} display holds no desktop",
            arch.name, screen.0, screen.1
        )
    })?;
    let mut steps = GuiSteps {
        arch,
        qmp,
        desktop,
        console,
        output,
        screen,
        scene: Scene::new(),
        window: WindowId::default(),
        message_box: None,
    };
    let client = (scene::CLIENT.0 as u32, scene::CLIENT.1 as u32);
    steps.window = steps.create(ROLE_NORMAL, 0, client)?;
    steps.run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_paints_the_scene_and_the_message_box_as_designed() {
        let WindowContent::Pixels { width, pixels } = scene_content(&Scene::new()) else {
            panic!("the scene is pixels");
        };
        assert_eq!(width, 400);
        assert_eq!(pixels.len(), 400 * 300);
        assert_eq!(pixels[0], colour::pixel(0x0090_5020));
        assert_eq!(pixels[50 * 400 + 2], colour::pixel(scene::WINDOW_COLOUR));
        let (x, y) = Scene::new().ellipse_centre();
        assert_eq!(pixels[(y * 400 + x) as usize], colour::pixel(0x0040_40e0));
        let mut clicked = Scene::new();
        clicked.clicked = true;
        let WindowContent::Pixels { pixels, .. } = scene_content(&clicked) else {
            panic!("the scene is pixels");
        };
        assert_eq!(pixels[(y * 400 + x) as usize], colour::pixel(0x0050_b030));
        let cells = message_cells();
        let layout = Layout::new(&cells, scene::MESSAGE_TYPE, 1280);
        assert_eq!((layout.width, layout.height), (288, 108));
        let WindowContent::Pixels { width, pixels } = message_box_content(&layout, &cells) else {
            panic!("the box is pixels");
        };
        assert_eq!(width, 288);
        assert_eq!(pixels[0], colour::token::MENU);
    }

    #[test]
    fn the_design_tokens_match_the_desktops() {
        use eggos_drivers::desktop::colour as desktop;
        for (windows, desktop) in [
            (colour::token::MENU, desktop::MENU),
            (colour::token::TEXT, desktop::TEXT),
            (colour::token::TEXT_DIM, desktop::TEXT_DIM),
            (colour::token::BUTTON_CLOSE, desktop::BUTTON_CLOSE),
            (colour::token::BUTTON_MINIMISE, desktop::BUTTON_MINIMISE),
            (colour::token::WINDOW_BACKGROUND, desktop::WINDOW_BACKGROUND),
            (colour::token::BUTTON_BORDER, desktop::BORDER_ACTIVE),
        ] {
            assert_eq!(windows, desktop);
        }
    }
}
