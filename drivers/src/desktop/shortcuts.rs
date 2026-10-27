//! The keys the window server takes before any window sees them (docs/ux/desktop-v0.3-gui.md,
//! section 5), per layout profile (section 11).

use super::windows::ControlSide;
use crate::input::key;
use crate::input::keymap::Modifiers;

/// Whose habits the desktop follows: where the window controls are and which keys close and
/// switch windows. Every profile's shortcuts that do not clash work in every profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Profile {
    /// Controls on the right, Ctrl as the primary modifier.
    Windows,
    /// Controls on the left, Cmd (the Super key) as the primary modifier; the default in 0.3.0.
    #[default]
    Macos,
    /// Controls on the right, Ctrl as the primary modifier, Super for the shell.
    Linux,
}

impl Profile {
    /// The side of the title bar the controls are on.
    pub const fn control_side(self) -> ControlSide {
        match self {
            Self::Macos => ControlSide::Left,
            Self::Windows | Self::Linux => ControlSide::Right,
        }
    }
}

/// The modifier a switcher opened with, whose release ends it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    Alt,
    Super,
}

impl Held {
    /// Whether key `code` is this modifier.
    pub const fn is(self, code: u16) -> bool {
        match self {
            Self::Alt => matches!(code, key::LEFT_ALT | key::RIGHT_ALT),
            Self::Super => matches!(code, key::LEFT_SUPER | key::RIGHT_SUPER),
        }
    }
}

/// What a shortcut asks of the window server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortcut {
    /// Open the switcher, or move its selection: forward, or back with `back`.
    Switch { back: bool, held: Held },
    /// Focus the next window in the switcher's order (back: the last) without the popup.
    Cycle { back: bool },
    /// Send the focused window a close request.
    Close,
    /// Open the focused window's window menu.
    WindowMenu,
    /// Maximise the focused window.
    Maximise,
    /// Restore the focused window if it is maximised, else minimise it.
    RestoreOrMinimise,
    /// Minimise the focused window.
    Minimise,
    /// Snap the focused window to the left half of the work area, or the right one.
    Snap { right: bool },
    /// Minimise every window, or restore the windows that minimised.
    ShowDesktop,
    /// Open the power menu.
    PowerMenu,
}

/// The shortcut key `code` pressed with `modifiers` is in `profile`, if it is one; Ctrl
/// shortcuts are left to a focused window that `takes_ctrl_keys`.
pub fn shortcut(
    code: u16,
    modifiers: Modifiers,
    profile: Profile,
    takes_ctrl_keys: bool,
) -> Option<Shortcut> {
    let held = Modifiers::from_bits(modifiers.bits() & !Modifiers::CAPS_LOCK.bits());
    let shift = held.contains(Modifiers::SHIFT);
    let without_shift = Modifiers::from_bits(held.bits() & !Modifiers::SHIFT.bits());
    let only = |wanted: Modifiers| held == wanted;
    let alt = Modifiers::ALT;
    let super_key = Modifiers::SUPER;
    Some(match code {
        key::TAB if without_shift == alt => Shortcut::Switch {
            back: shift,
            held: Held::Alt,
        },
        key::TAB if without_shift == super_key && profile == Profile::Macos => Shortcut::Switch {
            back: shift,
            held: Held::Super,
        },
        key::ESC if without_shift == alt => Shortcut::Cycle { back: shift },
        key::F4 if only(alt) => Shortcut::Close,
        key::W if only(super_key) && profile == Profile::Macos => Shortcut::Close,
        key::W if only(Modifiers::CTRL) && profile != Profile::Macos && !takes_ctrl_keys => {
            Shortcut::Close
        }
        key::SPACE if only(alt) => Shortcut::WindowMenu,
        key::UP if only(super_key) => Shortcut::Maximise,
        key::DOWN if only(super_key) => Shortcut::RestoreOrMinimise,
        key::LEFT if only(super_key) => Shortcut::Snap { right: false },
        key::RIGHT if only(super_key) => Shortcut::Snap { right: true },
        key::M if only(super_key) && profile == Profile::Macos => Shortcut::Minimise,
        key::H if only(super_key) && profile == Profile::Linux => Shortcut::Minimise,
        key::D if only(super_key) => Shortcut::ShowDesktop,
        key::X if only(super_key) => Shortcut::PowerMenu,
        _ => return None,
    })
}
