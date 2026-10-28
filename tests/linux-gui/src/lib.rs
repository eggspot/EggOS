//! What `linux-paint`, the Wayland test client (D140), paints, as data both sides share: the
//! program paints it into its `wl_shm` buffer, and the QEMU test paints it on the host to know
//! the picture the desktop must show in the program's window. Pixels are `XRGB8888`, which is
//! the window's `0x00RRGGBB`.

#![no_std]

/// The window's size before the compositor asks for another.
pub const CLIENT_SIZE: (u32, u32) = (400, 300);
/// The toplevel's title.
pub const TITLE: &str = "linux-paint";
/// The toplevel's app id.
pub const APP_ID: &str = "org.eggos.linux-paint";
/// The square a click recolours: x, y, width, height in surface coordinates.
pub const TARGET: (u32, u32, u32, u32) = (40, 60, 120, 90);
/// The handle whose press moves the window.
pub const HANDLE: (u32, u32, u32, u32) = (240, 60, 120, 40);
/// The band at the top.
pub const BAND_HEIGHT: u32 = 24;
/// One typed key's mark: its size and the step to the next.
pub const MARK: (u32, u32) = (8, 12);

/// The background.
pub const BACKGROUND: u32 = 0x00f0_f0e8;
/// The band.
pub const BAND: u32 = 0x0020_5090;
/// The target before a click.
pub const TARGET_COLOUR: u32 = 0x00e0_4040;
/// The target after a click.
pub const CLICKED_COLOUR: u32 = 0x0030_b050;
/// The handle at rest.
pub const HANDLE_COLOUR: u32 = 0x0080_8080;
/// The handle while the client has asked the compositor to move the window.
pub const MOVING_COLOUR: u32 = 0x00f0_a020;
/// A typed key's mark.
pub const MARK_COLOUR: u32 = 0x0010_60d0;

/// What the program shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scene {
    /// The target was clicked.
    pub clicked: bool,
    /// Keys typed so far.
    pub typed: u32,
    /// The handle's press asked for a move that has not ended yet.
    pub handle_moving: bool,
    /// The buffer's size.
    pub size: (u32, u32),
}

impl Default for Scene {
    fn default() -> Self {
        Self::new()
    }
}

fn contains(area: (u32, u32, u32, u32), x: i32, y: i32) -> bool {
    let (left, top, width, height) = area;
    x >= left as i32 && y >= top as i32 && x < (left + width) as i32 && y < (top + height) as i32
}

impl Scene {
    /// The scene before any input, at [`CLIENT_SIZE`].
    pub const fn new() -> Self {
        Self {
            clicked: false,
            typed: 0,
            handle_moving: false,
            size: CLIENT_SIZE,
        }
    }

    /// Whether (`x`, `y`) is on the target.
    pub fn on_target(x: i32, y: i32) -> bool {
        contains(TARGET, x, y)
    }

    /// Whether (`x`, `y`) is on the handle.
    pub fn on_handle(x: i32, y: i32) -> bool {
        contains(HANDLE, x, y)
    }

    /// The colour of pixel (`x`, `y`).
    pub fn pixel(&self, x: u32, y: u32) -> u32 {
        let (x_signed, y_signed) = (x as i32, y as i32);
        if y < BAND_HEIGHT {
            return BAND;
        }
        if contains(TARGET, x_signed, y_signed) {
            return match self.clicked {
                true => CLICKED_COLOUR,
                false => TARGET_COLOUR,
            };
        }
        if contains(HANDLE, x_signed, y_signed) {
            return match self.handle_moving {
                true => MOVING_COLOUR,
                false => HANDLE_COLOUR,
            };
        }
        let marks_top = self.size.1.saturating_sub(24);
        if y >= marks_top && y < marks_top + MARK.0 && x >= 20 {
            let column = (x - 20) / MARK.1;
            if column < self.typed && (x - 20) % MARK.1 < MARK.0 {
                return MARK_COLOUR;
            }
        }
        BACKGROUND
    }

    /// Paints the scene into `pixels`, rows of `width` pixels, at its size.
    pub fn paint(&self, pixels: &mut [u32], width: u32) {
        for (index, pixel) in pixels.iter_mut().enumerate() {
            let (x, y) = (index as u32 % width.max(1), index as u32 / width.max(1));
            *pixel = self.pixel(x, y);
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn the_scene_shows_its_states() {
        let mut scene = Scene::new();
        assert_eq!(scene.pixel(0, 0), BAND);
        assert_eq!(scene.pixel(50, 70), TARGET_COLOUR);
        assert_eq!(scene.pixel(250, 70), HANDLE_COLOUR);
        assert_eq!(scene.pixel(5, 200), BACKGROUND);
        scene.clicked = true;
        scene.handle_moving = true;
        scene.typed = 2;
        assert_eq!(scene.pixel(50, 70), CLICKED_COLOUR);
        assert_eq!(scene.pixel(250, 70), MOVING_COLOUR);
        assert_eq!(scene.pixel(20, 276), MARK_COLOUR);
        assert_eq!(scene.pixel(32, 276), MARK_COLOUR);
        assert_eq!(scene.pixel(29, 276), BACKGROUND);
        assert_eq!(scene.pixel(44, 276), BACKGROUND);
        assert!(Scene::on_target(40, 60) && !Scene::on_target(160, 60));
        assert!(Scene::on_handle(359, 99) && !Scene::on_handle(360, 99));
        let mut pixels = std::vec![0; 400 * 300];
        scene.paint(&mut pixels, 400);
        assert_eq!(pixels[70 * 400 + 50], CLICKED_COLOUR);
    }
}
