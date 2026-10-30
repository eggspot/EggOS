//! uxtheme (D178): the layer draws every window and control in one visual theme, EggOS's
//! (`docs/ux/windows-controls-v0.3.md`), so an app is themed (`IsAppThemed`), a theme a program
//! names for a window is accepted and changes nothing (`SetWindowTheme`), and a dialog's themed
//! background is the one the layer always draws (`EnableThemeDialogTexture`).

use super::gui::try_with;
use super::{Bool, TRUE};
use crate::library::Export;

const S_OK: i32 = 0;
const E_INVALIDARG: i32 = 0x8007_0057_u32 as i32;

fn known(window: usize) -> i32 {
    match try_with(|gui| gui.window(window).map(|_| ())) {
        Ok(()) => S_OK,
        Err(_) => E_INVALIDARG,
    }
}

extern "efiapi" fn set_window_theme(window: usize, _application: usize, _classes: usize) -> i32 {
    known(window)
}

extern "efiapi" fn is_app_themed() -> Bool {
    TRUE
}

extern "efiapi" fn enable_theme_dialog_texture(window: usize, _flags: u32) -> i32 {
    known(window)
}

pub(super) const EXPORTS: &[Export] = exports![
    b"SetWindowTheme" => set_window_theme,
    b"IsAppThemed" => is_app_themed,
    b"EnableThemeDialogTexture" => enable_theme_dialog_texture,
];
