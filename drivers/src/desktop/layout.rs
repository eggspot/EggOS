//! The shell's layout (docs/ux/desktop-v0.3-gui.md, sections 6, 11 and 12): one function of the
//! screen's size, the scale and the profile gives the panels, the work area windows go in,
//! where the launcher opens and which side the desktop icons start from. Nothing else in the
//! shell knows the screen's size, so a shell in user space can use the same function.

use super::shortcuts::Profile;
use crate::display::Rect;

/// Height of the top bar (macOS and Linux profiles), its edge line included, at scale 1.
pub const TOP_BAR_HEIGHT: u32 = 28;
/// Height of the taskbar (Windows profile), its edge line included, at scale 1.
pub const TASKBAR_HEIGHT: u32 = 40;
/// Height of the dock (macOS profile) at scale 1.
pub const DOCK_HEIGHT: u32 = 56;
/// The room between the dock and the screen's bottom edge, and between the dock and the work
/// area, at scale 1.
pub const DOCK_MARGIN: u32 = 8;
/// The launcher's size at scale 1.
pub const LAUNCHER_SIZE: (u32, u32) = (480, 400);
/// The room between the launcher and the taskbar, at scale 1.
pub const LAUNCHER_GAP: u32 = 8;

/// The side of the work area the desktop icons' first column is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconSide {
    /// The left edge (Windows and Linux habits).
    Left,
    /// The right edge (macOS habits).
    Right,
}

/// Where the shell's parts are on one screen in one profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    /// The whole screen.
    pub screen: Rect,
    /// The top bar, in the macOS and Linux profiles.
    pub top_bar: Option<Rect>,
    /// The taskbar, in the Windows profile.
    pub taskbar: Option<Rect>,
    /// The full-width band the dock is centred in, in the macOS profile.
    pub dock_band: Option<Rect>,
    /// Where windows go: the screen less the panels.
    pub work_area: Rect,
    /// Where the launcher opens, its border included.
    pub launcher: Rect,
    /// Which side the desktop icons start from.
    pub icon_side: IconSide,
}

impl Layout {
    /// The panels there are, each once.
    pub fn panels(&self) -> impl Iterator<Item = Rect> + '_ {
        [self.top_bar, self.taskbar, self.dock_band]
            .into_iter()
            .flatten()
    }
}

/// The shell's layout on a `screen_width` by `screen_height` screen at `scale` (1 or 2;
/// anything else counts as 1) in `profile`.
pub fn layout(screen_width: u32, screen_height: u32, scale: u32, profile: Profile) -> Layout {
    let scale = if scale == 2 { 2 } else { 1 };
    let screen = Rect::new(0, 0, screen_width, screen_height);
    let fit = |size: u32| size.saturating_mul(scale).min(screen_height);
    let (top_bar, taskbar, dock_band) = match profile {
        Profile::Windows => {
            let height = fit(TASKBAR_HEIGHT);
            let bar = Rect::new(0, screen_height - height, screen_width, height);
            (None, Some(bar), None)
        }
        Profile::Macos => {
            let bar = Rect::new(0, 0, screen_width, fit(TOP_BAR_HEIGHT));
            let height = fit(DOCK_HEIGHT);
            let bottom = screen_height.saturating_sub(fit(DOCK_MARGIN));
            let top = bottom.saturating_sub(height).max(bar.bottom());
            let band = Rect::new(0, top, screen_width, bottom.saturating_sub(top));
            (Some(bar), None, Some(band))
        }
        Profile::Linux => {
            let bar = Rect::new(0, 0, screen_width, fit(TOP_BAR_HEIGHT));
            (Some(bar), None, None)
        }
    };
    let top = top_bar.map_or(0, |bar| bar.bottom());
    let bottom = match (taskbar, dock_band) {
        (Some(bar), _) => bar.y,
        (None, Some(band)) => band.y.saturating_sub(fit(DOCK_MARGIN)).max(top),
        (None, None) => screen_height,
    };
    let work_area = Rect::new(0, top, screen_width, bottom.saturating_sub(top));
    let (width, height) = (
        (LAUNCHER_SIZE.0 * scale).min(work_area.width),
        (LAUNCHER_SIZE.1 * scale).min(work_area.height),
    );
    let x = (screen_width - width) / 2;
    let y = match taskbar {
        Some(bar) => bar
            .y
            .saturating_sub(LAUNCHER_GAP * scale + height)
            .max(work_area.y),
        None => work_area.y + (work_area.height - height) / 2,
    };
    let icon_side = match profile {
        Profile::Macos => IconSide::Right,
        Profile::Windows | Profile::Linux => IconSide::Left,
    };
    Layout {
        screen,
        top_bar,
        taskbar,
        dock_band,
        work_area,
        launcher: Rect::new(x, y, width, height),
        icon_side,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZES: [(u32, u32, u32); 7] = [
        (1280, 800, 1),
        (1024, 768, 1),
        (800, 600, 1),
        (1920, 1080, 1),
        (360, 640, 1),
        (2560, 1600, 2),
        (3840, 2160, 2),
    ];

    fn inside(inner: Rect, outer: Rect) -> bool {
        inner.x >= outer.x
            && inner.y >= outer.y
            && inner.right() <= outer.right()
            && inner.bottom() <= outer.bottom()
    }

    #[test]
    fn the_work_areas_at_1280x800_are_the_design_s() {
        let work = |profile| layout(1280, 800, 1, profile).work_area;
        assert_eq!(work(Profile::Windows), Rect::new(0, 0, 1280, 760));
        assert_eq!(work(Profile::Macos), Rect::new(0, 28, 1280, 700));
        assert_eq!(work(Profile::Linux), Rect::new(0, 28, 1280, 772));
    }

    #[test]
    fn each_profile_has_its_panels() {
        let windows = layout(1280, 800, 1, Profile::Windows);
        assert_eq!(windows.taskbar, Some(Rect::new(0, 760, 1280, 40)));
        assert_eq!((windows.top_bar, windows.dock_band), (None, None));
        let macos = layout(1280, 800, 1, Profile::Macos);
        assert_eq!(macos.top_bar, Some(Rect::new(0, 0, 1280, 28)));
        assert_eq!(macos.dock_band, Some(Rect::new(0, 736, 1280, 56)));
        assert_eq!(macos.taskbar, None);
        let linux = layout(1280, 800, 1, Profile::Linux);
        assert_eq!(linux.top_bar, Some(Rect::new(0, 0, 1280, 28)));
        assert_eq!((linux.taskbar, linux.dock_band), (None, None));
    }

    #[test]
    fn the_launcher_opens_above_the_taskbar_or_centred_in_the_work_area() {
        assert_eq!(
            layout(1280, 800, 1, Profile::Windows).launcher,
            Rect::new(400, 352, 480, 400)
        );
        assert_eq!(
            layout(1280, 800, 1, Profile::Macos).launcher,
            Rect::new(400, 178, 480, 400)
        );
        assert_eq!(
            layout(1280, 800, 1, Profile::Linux).launcher,
            Rect::new(400, 214, 480, 400)
        );
    }

    #[test]
    fn the_icons_start_on_the_profile_s_side() {
        assert_eq!(
            layout(1280, 800, 1, Profile::Windows).icon_side,
            IconSide::Left
        );
        assert_eq!(
            layout(1280, 800, 1, Profile::Macos).icon_side,
            IconSide::Right
        );
        assert_eq!(
            layout(1280, 800, 1, Profile::Linux).icon_side,
            IconSide::Left
        );
    }

    #[test]
    fn at_every_size_the_parts_stay_on_screen_and_apart() {
        for (width, height, scale) in SIZES {
            for profile in Profile::ALL {
                let layout = layout(width, height, scale, profile);
                let screen = layout.screen;
                let at = format!("{width}x{height}@{scale} {profile:?}");
                assert!(inside(layout.work_area, screen), "{at}");
                assert!(!layout.work_area.is_empty(), "{at}");
                assert!(inside(layout.launcher, layout.work_area), "{at}");
                let panels: Vec<Rect> = layout.panels().collect();
                assert!(!panels.is_empty(), "{at}");
                for (index, panel) in panels.iter().enumerate() {
                    assert!(inside(*panel, screen), "{at}");
                    assert!(!panel.overlaps(&layout.work_area), "{at}: {panel:?}");
                    for other in &panels[index + 1..] {
                        assert!(!panel.overlaps(other), "{at}");
                    }
                }
            }
        }
    }

    #[test]
    fn scale_2_doubles_the_panels() {
        let macos = layout(2560, 1600, 2, Profile::Macos);
        assert_eq!(macos.top_bar, Some(Rect::new(0, 0, 2560, 56)));
        assert_eq!(macos.dock_band, Some(Rect::new(0, 1472, 2560, 112)));
        assert_eq!(macos.work_area, Rect::new(0, 56, 2560, 1400));
        let windows = layout(2560, 1600, 2, Profile::Windows);
        assert_eq!(windows.taskbar, Some(Rect::new(0, 1520, 2560, 80)));
        assert_eq!(windows.launcher, Rect::new(800, 704, 960, 800));
    }
}
