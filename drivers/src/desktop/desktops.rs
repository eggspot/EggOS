//! Virtual desktops (D204, docs/ux/desktops-overview-screenshots-v0.3.md section 1): 1 to
//! [`MAX_DESKTOPS`], numbered by position; each normal window is on one, and only the current
//! desktop's windows are shown, hit and focused (dialogs follow their parent; panels and popups
//! show on every desktop). A new window opens on the current desktop; focusing a window on
//! another switches to it first.

use super::protocol::Role;
use super::protocol::{EventSink, WindowError, desktop_action};
use super::{Damage, Desktop, Rect};

/// The most desktops.
pub const MAX_DESKTOPS: u8 = 8;

/// Why a desktop action was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopError {
    /// No desktop or window has that number.
    NoSuch,
    /// There are [`MAX_DESKTOPS`] already.
    TooMany,
    /// The only desktop cannot be closed.
    Last,
}

impl Desktop {
    /// How many desktops there are, which is shown (from 0), and each one's normal windows.
    pub fn desktops(&self) -> (u8, u8, [u8; MAX_DESKTOPS as usize]) {
        let mut counts = [0u8; MAX_DESKTOPS as usize];
        for window in self.windows.iter().flatten() {
            if window.role == Role::Normal && window.parent.is_none() {
                let count = &mut counts[usize::from(window.desktop.min(MAX_DESKTOPS - 1))];
                *count = count.saturating_add(1);
            }
        }
        (self.desktops, self.current_desktop, counts)
    }

    /// Shows desktop `desktop` (from 0): its windows become the shown ones and the top-most
    /// takes the focus.
    pub fn switch_desktop(
        &mut self,
        desktop: u8,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Result<(), DesktopError> {
        if desktop >= self.desktops {
            return Err(DesktopError::NoSuch);
        }
        if desktop == self.current_desktop {
            return Ok(());
        }
        self.drag = None;
        self.resize = None;
        self.current_desktop = desktop;
        damage.add(Rect::new(0, 0, self.width, self.height));
        self.focus_top(damage, events);
        Ok(())
    }

    /// Makes a new desktop after the last and shows it.
    pub fn new_desktop(
        &mut self,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Result<u8, DesktopError> {
        if self.desktops == MAX_DESKTOPS {
            return Err(DesktopError::TooMany);
        }
        self.desktops += 1;
        let made = self.desktops - 1;
        self.switch_desktop(made, damage, events)?;
        Ok(made)
    }

    /// Closes desktop `desktop`: its windows move to the one before it (after it when it is
    /// the first), the desktops after it are renumbered, and the one that took its windows is
    /// shown. How many windows moved.
    pub fn close_desktop(
        &mut self,
        desktop: u8,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Result<usize, DesktopError> {
        if desktop >= self.desktops {
            return Err(DesktopError::NoSuch);
        }
        if self.desktops == 1 {
            return Err(DesktopError::Last);
        }
        let receiver = desktop.saturating_sub(1);
        let mut moved = 0;
        for window in self.windows.iter_mut().flatten() {
            if window.desktop == desktop {
                window.desktop = receiver;
                moved += usize::from(window.role == Role::Normal && window.parent.is_none());
            } else if window.desktop > desktop {
                window.desktop -= 1;
            }
        }
        self.desktops -= 1;
        self.current_desktop = u8::MAX;
        self.switch_desktop(receiver, damage, events)?;
        Ok(moved)
    }

    /// Moves the window at `slot` to desktop `desktop`, its dialogs with it; shows that desktop
    /// when `follow`.
    pub fn move_to_desktop(
        &mut self,
        slot: usize,
        desktop: u8,
        follow: bool,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Result<(), DesktopError> {
        if desktop >= self.desktops {
            return Err(DesktopError::NoSuch);
        }
        let Some(window) = self.windows.get(slot).and_then(Option::as_ref) else {
            return Err(DesktopError::NoSuch);
        };
        if window.role != Role::Normal || window.parent.is_some() {
            return Err(DesktopError::NoSuch);
        }
        self.window_mut(slot).desktop = desktop;
        damage.add(Rect::new(0, 0, self.width, self.height));
        match follow {
            true => {
                self.switch_desktop(desktop, damage, events)?;
                self.activate(slot, damage, events);
            }
            false => self.focus_top(damage, events),
        }
        Ok(())
    }

    /// Does `wm desktop`'s `action` ([`desktop_action`]'s numbers) with desktop `number`
    /// (from 1) and window `window`: what it reports back (the new desktop's number for
    /// `NEW`, the windows moved for `CLOSE`, else 0).
    pub fn desktop_action(
        &mut self,
        action: u32,
        number: u32,
        window: u32,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Result<u32, WindowError> {
        let desktop = number
            .checked_sub(1)
            .and_then(|desktop| u8::try_from(desktop).ok());
        let refused = |error: DesktopError| match error {
            DesktopError::NoSuch => WindowError::Invalid,
            DesktopError::TooMany => WindowError::TableFull,
            DesktopError::Last => WindowError::Busy,
        };
        let desktop_or_invalid = || desktop.ok_or(WindowError::Invalid);
        match action {
            desktop_action::SWITCH => {
                self.switch_desktop(desktop_or_invalid()?, damage, events)
                    .map_err(refused)?;
                Ok(0)
            }
            desktop_action::NEW => self
                .new_desktop(damage, events)
                .map(|made| u32::from(made) + 1)
                .map_err(refused),
            desktop_action::CLOSE => self
                .close_desktop(desktop_or_invalid()?, damage, events)
                .map(|moved| moved as u32)
                .map_err(refused),
            desktop_action::MOVE => {
                let id = super::WindowId::try_from(window).map_err(|_| WindowError::Invalid)?;
                let slot = self.find(id).ok_or(WindowError::NoSuchWindow)?;
                self.move_to_desktop(slot, desktop_or_invalid()?, false, damage, events)
                    .map_err(refused)?;
                Ok(0)
            }
            _ => Err(WindowError::Invalid),
        }
    }

    /// The desktop the window at `slot` is on.
    pub fn desktop_of(&self, slot: usize) -> Option<u8> {
        self.windows
            .get(slot)?
            .as_ref()
            .map(|window| window.desktop)
    }
}
