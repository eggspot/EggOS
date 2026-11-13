//! Global hot keys (D398): a client registers a key with modifiers for one of its windows
//! (`REGISTER_HOT_KEY`), and the server, which sees every key, gives that window a `HotKey`
//! event when the person presses exactly that combination, whichever window has the focus,
//! and keeps the key from the focused window. Only combinations with Ctrl, Alt or Super are
//! taken, none the shell's own shortcuts use, each by one client at a time: a client learns
//! of its own combinations and nothing else of the person's typing.

use super::Desktop;
use super::protocol::{EventRecord, EventSink, WindowError, event};
use super::shortcuts::shortcut;
use crate::input::keymap::Modifiers;

/// The most hot keys the server keeps.
pub const MOST_HOT_KEYS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct HotKey {
    slot: usize,
    id: u32,
    code: u16,
    held: u8,
}

const KEPT: u8 = Modifiers::SHIFT.bits()
    | Modifiers::ALT.bits()
    | Modifiers::CTRL.bits()
    | Modifiers::SUPER.bits();
const NEEDED: u8 = Modifiers::ALT.bits() | Modifiers::CTRL.bits() | Modifiers::SUPER.bits();

impl Desktop {
    /// `REGISTER_HOT_KEY` (D398): client `client`'s window `id` takes key `code` with the
    /// modifiers `held` (`Modifiers` bits) as hot key `hot_key`, or gives it up with
    /// `remove`. `NotAllowed` for a combination without Ctrl, Alt or Super or one the shell
    /// uses, `Busy` for one another hot key has, `TableFull` past [`MOST_HOT_KEYS`].
    pub fn register_hot_key(
        &mut self,
        client: u16,
        id: u32,
        (hot_key, code, held): (u32, u16, u8),
        remove: bool,
    ) -> Result<(), WindowError> {
        let slot = self.owned(client, id)?;
        if remove {
            for entry in self.hot_keys.iter_mut() {
                if entry.is_some_and(|entry| entry.slot == slot && entry.id == hot_key) {
                    *entry = None;
                }
            }
            return Ok(());
        }
        let held = held & KEPT;
        if held & NEEDED == 0
            || shortcut(code, Modifiers::from_bits(held), self.profile, false).is_some()
        {
            return Err(WindowError::NotAllowed);
        }
        if self
            .hot_keys
            .iter()
            .flatten()
            .any(|entry| entry.code == code && entry.held == held)
        {
            return Err(WindowError::Busy);
        }
        let free = self
            .hot_keys
            .iter_mut()
            .find(|entry| entry.is_none())
            .ok_or(WindowError::TableFull)?;
        *free = Some(HotKey {
            slot,
            id: hot_key,
            code,
            held,
        });
        Ok(())
    }

    /// Key `code` went down with `modifiers`: whether it was a hot key, which its window
    /// heard of and the focused window does not.
    pub(super) fn hot_key_pressed(
        &mut self,
        code: u16,
        modifiers: Modifiers,
        events: &mut dyn EventSink,
    ) -> bool {
        let held = modifiers.bits() & KEPT;
        let Some(entry) = self
            .hot_keys
            .iter()
            .flatten()
            .find(|entry| entry.code == code && entry.held == held)
            .copied()
        else {
            return false;
        };
        let record = EventRecord::new(
            event::HOT_KEY,
            0,
            entry.id as i32,
            0,
            u32::from(code),
            u32::from(held),
        );
        self.send(entry.slot, record, events);
        true
    }

    /// Forgets the hot keys of the window in `slot`, which is gone.
    pub(super) fn forget_hot_keys(&mut self, slot: usize) {
        for entry in self.hot_keys.iter_mut() {
            if entry.is_some_and(|entry| entry.slot == slot) {
                *entry = None;
            }
        }
    }
}
