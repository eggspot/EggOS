//! The Windows C runtime boot test's checks of the display (D146): `eggpad.exe`, our own C
//! notepad built by MSVC as a `/MD` program, shows the file it opened, takes typed text, saves
//! it on Ctrl+S and closes by its close control; each step is compared with the desktop the host
//! renders with the window's content painted as eggpad paints it (`tests/windows-crt/eggpad.c`).

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
use eggos_windows_gui::surface::Surface;
use eggos_windows_gui::text;
use std::sync::mpsc;
use std::time::Duration;

const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(30);
const HOST_CLIENT: u16 = 0;
const ROLE_NORMAL: u32 = 0;
const KEY_PAUSE: Duration = Duration::from_millis(100);
const KEY_B: u16 = 48;
const KEY_C: u16 = 46;
const KEY_S: u16 = 31;

const TITLE: &str = "eggpad";
const CLIENT: (u32, u32) = (400, 300);
const MARGIN: i32 = 8;
/// The file the shell writes before running eggpad, and the bytes it holds.
pub(super) const SEEDED_TEXT: &str = "Hello from the shell\n";
const TYPED_TEXT: &str = "abc\n";
const OPENED_STATUS: &str = "21 bytes";
/// The status line after saving the file in text mode, where each of its two line feeds
/// becomes a carriage return and a line feed.
pub(super) const SAVED_STATUS: &str = "Saved 27 bytes";

const BACKGROUND: u32 = 0x00ff_ffff;
const INK: u32 = 0x0000_0000;

fn units(words: &str) -> Vec<u16> {
    words.encode_utf16().collect()
}

fn pad_content(contents: &str, status: &str) -> WindowContent {
    let (width, height) = CLIENT;
    let mut pixels = vec![colour::pixel(BACKGROUND); (width * height) as usize];
    let mut surface = Surface::new(&mut pixels, width, height).expect("the pad fits");
    let line_height = text::extent(&units("M")).1;
    let ink = colour::pixel(INK);
    let mut y = MARGIN;
    for line in contents.split('\n') {
        text::text_out(&mut surface, MARGIN, y, &units(line), ink, None);
        y += line_height;
    }
    let status_y = height as i32 - line_height - MARGIN;
    text::text_out(&mut surface, MARGIN, status_y, &units(status), ink, None);
    WindowContent::Pixels { width, pixels }
}

struct PadSteps<'a> {
    arch: &'a Arch,
    qmp: Qmp,
    desktop: Desktop,
    console: &'a mut Vec<u8>,
    output: &'a mpsc::Receiver<Vec<u8>>,
    screen: (u32, u32),
    window: WindowId,
}

impl PadSteps<'_> {
    fn check(&mut self, at: &str, content: WindowContent, timeout: Duration) -> Result<(), String> {
        check_desktop_frame_within(
            self.arch,
            &mut self.qmp,
            self.console,
            self.output,
            at,
            &self.desktop,
            &[(self.window, content)],
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
        let created = self
            .desktop
            .create(HOST_CLIENT, ROLE_NORMAL, 0, CLIENT.0, CLIENT.1, 0)
            .map_err(|error| self.refused("a window", error))?;
        let id = u32::from(created.window);
        self.desktop
            .set_title(HOST_CLIENT, id, TITLE.as_bytes(), &mut Damage::new())
            .map_err(|error| self.refused("a title", error))?;
        self.desktop
            .commit(HOST_CLIENT, id, &[], &mut Damage::new(), &mut DiscardEvents)
            .map_err(|error| self.refused("a commit", error))?;
        Ok(created.window)
    }

    fn mirror_key(&mut self, code: u16, down: bool, held: Modifiers) {
        std::thread::sleep(KEY_PAUSE);
        self.desktop
            .key(code, down, held, &mut Damage::new(), &mut DiscardEvents);
    }

    fn tap(&mut self, qcode: &'static str, code: u16, held: Modifiers) -> Result<(), String> {
        for down in [true, false] {
            self.qmp.send_input(&[InputEvent::Key { qcode, down }])?;
            self.mirror_key(code, down, held);
        }
        Ok(())
    }

    fn press_control_s(&mut self) -> Result<(), String> {
        self.qmp.send_input(&[InputEvent::Key {
            qcode: "ctrl",
            down: true,
        }])?;
        self.mirror_key(key::LEFT_CTRL, true, Modifiers::CTRL);
        self.tap("s", KEY_S, Modifiers::CTRL)?;
        self.qmp.send_input(&[InputEvent::Key {
            qcode: "ctrl",
            down: false,
        }])?;
        self.mirror_key(key::LEFT_CTRL, false, Modifiers::NONE);
        Ok(())
    }

    fn close(&mut self) -> Result<(), String> {
        let close = self.desktop.close_control(self.window).unwrap_or_default();
        for step in click_steps(close.x + 10, close.y + 14) {
            send_pointer_step(&mut self.qmp, step, self.screen)?;
            apply_pointer_step(&mut self.desktop, step);
        }
        self.desktop
            .destroy(
                HOST_CLIENT,
                u32::from(self.window),
                &mut Damage::new(),
                &mut DiscardEvents,
            )
            .map_err(|error| self.refused("to destroy a window", error))
    }

    fn run(&mut self) -> Result<(), String> {
        let opened = pad_content(SEEDED_TEXT, OPENED_STATUS);
        self.check("eggpad's window with the file", opened, FIRST_FRAME_TIMEOUT)?;

        for (qcode, code) in [
            ("a", key::A),
            ("b", KEY_B),
            ("c", KEY_C),
            ("ret", key::ENTER),
        ] {
            self.tap(qcode, code, Modifiers::NONE)?;
        }
        let edited = format!("{SEEDED_TEXT}{TYPED_TEXT}");
        let typed = pad_content(&edited, OPENED_STATUS);
        self.check("a line typed into eggpad", typed, TERMINAL_FRAME_TIMEOUT)?;

        self.press_control_s()?;
        let saved = pad_content(&edited, SAVED_STATUS);
        self.check(
            "eggpad's status after Ctrl+S",
            saved,
            TERMINAL_FRAME_TIMEOUT,
        )?;

        self.close()?;
        check_desktop_frame_within(
            self.arch,
            &mut self.qmp,
            self.console,
            self.output,
            "eggpad's window closed by its close control",
            &self.desktop,
            &[],
            TERMINAL_FRAME_TIMEOUT,
        )
    }
}

/// Checks the display through the whole of `eggpad.exe`'s run, driving it through QMP.
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
    let mut steps = PadSteps {
        arch,
        qmp,
        desktop,
        console,
        output,
        screen,
        window: WindowId::default(),
    };
    steps.window = steps.create()?;
    steps.run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_paints_eggpad_s_lines_and_status_on_white() {
        let WindowContent::Pixels { width, pixels } = pad_content(SEEDED_TEXT, OPENED_STATUS)
        else {
            panic!("the pad is pixels");
        };
        assert_eq!(width, CLIENT.0);
        assert_eq!(pixels.len(), (CLIENT.0 * CLIENT.1) as usize);
        assert_eq!(pixels[0], colour::pixel(BACKGROUND));
        let ink = colour::pixel(INK);
        let line_height = text::extent(&units("M")).1 as usize;
        let row = |y: usize| &pixels[y * CLIENT.0 as usize..(y + 1) * CLIENT.0 as usize];
        let first_line = (MARGIN as usize..MARGIN as usize + line_height).map(row);
        assert!(first_line.flatten().any(|pixel| *pixel == ink));
        let status_top = CLIENT.1 as usize - line_height - MARGIN as usize;
        let status = (status_top..status_top + line_height).map(row);
        assert!(status.flatten().any(|pixel| *pixel == ink));
        assert_ne!(
            pad_content(SEEDED_TEXT, OPENED_STATUS),
            pad_content(SEEDED_TEXT, SAVED_STATUS)
        );
    }

    #[test]
    fn saving_in_text_mode_adds_a_carriage_return_per_line() {
        let saved = format!("{SEEDED_TEXT}{TYPED_TEXT}");
        let bytes = saved.len() + saved.matches('\n').count();
        assert_eq!(OPENED_STATUS, format!("{} bytes", SEEDED_TEXT.len()));
        assert_eq!(SAVED_STATUS, format!("Saved {bytes} bytes"));
    }
}
