//! The Linux GUI boot test's checks of the display (D140): `linux-paint`'s window through
//! `eggway`, its click, key, title-bar drag, the move it asks for from a press on its handle
//! (and the stale one that moves nothing), its maximise and its close, each compared with the
//! desktop the host renders with the window's content painted by `eggos_linux_gui_tests` from
//! the program's own scene.

use super::{
    Desktop, DiscardEvents, PointerStep, TERMINAL_FRAME_TIMEOUT, WindowContent, apply_pointer_step,
    check_desktop_frame_within, click_steps, screen_size, send_pointer_step,
};
use crate::Arch;
use crate::qmp::{InputEvent, Qmp};
use eggos_drivers::desktop::protocol::move_or_resize;
use eggos_drivers::desktop::{Part, WindowId};
use eggos_drivers::display::{Damage, Rect};
use eggos_drivers::input::key;
use eggos_drivers::input::keymap::Modifiers;
use eggos_linux_gui_tests::{self as scene, Scene};
use std::sync::mpsc;
use std::time::Duration;

const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(30);
const SLOW_FRAME_TIMEOUT: Duration = Duration::from_secs(20);
const CLIENT: u16 = 0;
const KEY_PAUSE: Duration = Duration::from_millis(100);
const TITLE_DRAG: (i32, i32) = (80, 40);
const HANDLE_DRAG: (i32, i32) = (-60, 30);

fn content_of(scene: &Scene) -> WindowContent {
    let (width, height) = scene.size;
    let mut pixels = vec![0; (width * height) as usize];
    scene.paint(&mut pixels, width);
    WindowContent::Pixels { width, pixels }
}

fn shifted(x: u32, by: i32) -> u32 {
    (i64::from(x) + i64::from(by)).max(0) as u32
}

struct GuiSteps<'a> {
    arch: &'a Arch,
    qmp: Qmp,
    desktop: Desktop,
    console: &'a mut Vec<u8>,
    output: &'a mpsc::Receiver<Vec<u8>>,
    screen: (u32, u32),
    scene: Scene,
    window: Option<WindowId>,
}

impl GuiSteps<'_> {
    fn check(&mut self, at: &str, timeout: Duration) -> Result<(), String> {
        let windows: Vec<_> = self
            .window
            .map(|window| (window, content_of(&self.scene)))
            .into_iter()
            .collect();
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

    fn window(&self) -> Result<WindowId, String> {
        self.window
            .ok_or_else(|| format!("{}: linux-paint's window is gone", self.arch.name))
    }

    fn content(&self) -> Result<Rect, String> {
        let window = self.window()?;
        self.desktop
            .content_rect(window)
            .ok_or_else(|| format!("{}: the window {window} is gone", self.arch.name))
    }

    fn create(&mut self) -> Result<(), String> {
        let created = self
            .desktop
            .create(CLIENT, 0, 0, scene::CLIENT_SIZE.0, scene::CLIENT_SIZE.1, 0)
            .map_err(|error| self.refused("a window", error))?;
        let id = u32::from(created.window);
        self.desktop
            .set_title(CLIENT, id, scene::TITLE.as_bytes(), &mut Damage::new())
            .map_err(|error| self.refused("a title", error))?;
        self.desktop
            .commit(CLIENT, id, &[], &mut Damage::new(), &mut DiscardEvents)
            .map_err(|error| self.refused("a commit", error))?;
        self.window = Some(created.window);
        Ok(())
    }

    fn step(&mut self, x: u32, y: u32, button: Option<bool>) -> Result<(), String> {
        let step = PointerStep { x, y, button };
        send_pointer_step(&mut self.qmp, step, self.screen)?;
        apply_pointer_step(&mut self.desktop, step);
        Ok(())
    }

    fn click(&mut self, x: u32, y: u32) -> Result<(), String> {
        for step in click_steps(x, y) {
            self.step(step.x, step.y, step.button)?;
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

    fn drag(&mut self, from: (u32, u32), by: (i32, i32)) -> Result<(), String> {
        let (x, y) = from;
        for (fraction, button) in [(1, None), (2, None), (2, Some(false))] {
            self.step(
                shifted(x, by.0 * fraction / 2),
                shifted(y, by.1 * fraction / 2),
                button,
            )?;
        }
        Ok(())
    }

    fn expect_moved(&self, before: Rect, by: (i32, i32), what: &str) -> Result<(), String> {
        let after = self.content()?;
        match (after.x, after.y) == (shifted(before.x, by.0), shifted(before.y, by.1)) {
            true => Ok(()),
            false => Err(format!(
                "{}: {what} moved the window from ({}, {}) to ({}, {})",
                self.arch.name, before.x, before.y, after.x, after.y
            )),
        }
    }

    fn title_drag(&mut self) -> Result<(), String> {
        let window = self.window()?;
        let title = self
            .desktop
            .title_bar_of(window)
            .ok_or_else(|| format!("{}: no title bar", self.arch.name))?;
        let before = self.content()?;
        let grab = (title.x + title.width / 2, title.y + title.height / 2);
        self.step(grab.0, grab.1, Some(true))?;
        self.drag(grab, TITLE_DRAG)?;
        self.expect_moved(before, TITLE_DRAG, "the title-bar drag")?;
        self.check(
            "the window dragged by its title bar",
            TERMINAL_FRAME_TIMEOUT,
        )
    }

    fn handle_drag(&mut self) -> Result<(), String> {
        let window = self.window()?;
        let content = self.content()?;
        let (x, y, width, height) = scene::HANDLE;
        let press = (content.x + x + width / 2, content.y + y + height / 2);
        self.step(press.0, press.1, None)?;
        self.step(press.0, press.1, Some(true))?;
        self.desktop
            .start_move_or_resize(
                CLIENT,
                u32::from(window),
                move_or_resize::MOVE,
                0,
                &mut Damage::new(),
                &mut DiscardEvents,
            )
            .map_err(|error| self.refused("a move", error))?;
        self.scene.handle_moving = true;
        self.check(
            "the handle's press asked eggway to move the window",
            SLOW_FRAME_TIMEOUT,
        )?;
        self.drag(press, HANDLE_DRAG)?;
        self.expect_moved(content, HANDLE_DRAG, "the client's move")?;
        self.check(
            "the window moved by the client's request",
            TERMINAL_FRAME_TIMEOUT,
        )?;
        let moved = self.content()?;
        let back = (
            shifted(press.0, HANDLE_DRAG.0) + 4,
            shifted(press.1, HANDLE_DRAG.1) + 4,
        );
        self.step(back.0, back.1, None)?;
        self.scene.handle_moving = false;
        self.check(
            "a move with a stale serial left the window where it was",
            SLOW_FRAME_TIMEOUT,
        )?;
        self.expect_moved(moved, (0, 0), "the stale move")
    }

    fn maximise(&mut self) -> Result<(), String> {
        let window = self.window()?;
        let control = self
            .desktop
            .control_of(window, Part::Maximise)
            .ok_or_else(|| format!("{}: no maximise control", self.arch.name))?;
        self.click(
            control.x + control.width / 2,
            control.y + control.height / 2,
        )?;
        let content = self.content()?;
        let id = u32::from(window);
        self.desktop
            .set_size(
                CLIENT,
                id,
                content.width,
                content.height,
                &mut Damage::new(),
                &mut DiscardEvents,
            )
            .map_err(|error| self.refused("a size", error))?;
        self.desktop
            .commit(CLIENT, id, &[], &mut Damage::new(), &mut DiscardEvents)
            .map_err(|error| self.refused("a commit", error))?;
        self.scene.size = (content.width, content.height);
        self.check(
            "the window maximised and painted at its size",
            SLOW_FRAME_TIMEOUT,
        )
    }

    fn close(&mut self) -> Result<(), String> {
        let window = self.window()?;
        let close = self.desktop.close_control(window).unwrap_or_default();
        self.click(close.x + close.width / 2, close.y + close.height / 2)?;
        self.desktop
            .destroy(
                CLIENT,
                u32::from(window),
                &mut Damage::new(),
                &mut DiscardEvents,
            )
            .map_err(|error| self.refused("to destroy a window", error))?;
        self.window = None;
        self.check("the window closed by its close control", SLOW_FRAME_TIMEOUT)
    }

    fn run(&mut self) -> Result<(), String> {
        self.create()?;
        self.check(
            "linux-paint's window painted with the desktop's chrome",
            FIRST_FRAME_TIMEOUT,
        )?;
        let content = self.content()?;
        let (x, y, width, height) = scene::TARGET;
        self.click(content.x + x + width / 2, content.y + y + height / 2)?;
        self.scene.clicked = true;
        self.check("the target recoloured by a click", TERMINAL_FRAME_TIMEOUT)?;
        self.tap("a", key::A)?;
        self.scene.typed = 1;
        self.check("a key's mark in the window", TERMINAL_FRAME_TIMEOUT)?;
        self.title_drag()?;
        self.handle_drag()?;
        self.maximise()?;
        self.close()
    }
}

/// Checks the display through the whole of `linux-paint`'s run, driving it through QMP.
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
    GuiSteps {
        arch,
        qmp,
        desktop,
        console,
        output,
        screen,
        scene: Scene::new(),
        window: None,
    }
    .run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_paints_linux_paint_s_scene() {
        let WindowContent::Pixels { width, pixels } = content_of(&Scene::new()) else {
            panic!("the scene is pixels");
        };
        assert_eq!(width, 400);
        assert_eq!(pixels.len(), 400 * 300);
        assert_eq!(pixels[0], scene::BAND);
        assert_eq!(pixels[100 * 400 + 100], scene::TARGET_COLOUR);
        assert_eq!(shifted(10, -20), 0);
        assert_eq!(shifted(10, 20), 30);
    }
}
