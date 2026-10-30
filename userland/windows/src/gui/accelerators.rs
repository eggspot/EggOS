//! Keyboard accelerator tables (D178, `docs/ux/windows-controls-v0.3.md` section 12): made by
//! `CreateAcceleratorTableW` from `ACCEL` entries or by `LoadAcceleratorsW` from an
//! `RT_ACCELERATOR` resource, and matched by `TranslateAcceleratorW` against a key press (an
//! `FVIRTKEY` entry, its Shift, Ctrl and Alt matched exactly) or a typed character.

use super::message;
use super::slots::Slots;

/// The most tables a program has at once.
pub const MAX_TABLES: usize = 8;
/// The most entries in one table.
pub const MAX_ENTRIES: usize = 128;
/// The size of one `ACCEL`, as `CreateAcceleratorTableW` reads them.
pub const ACCEL_SIZE: usize = 6;
const RESOURCE_ENTRY_SIZE: usize = 8;
const LAST_ENTRY: u16 = 0x80;
const HANDLE_BASE: usize = 0x150_0000;

/// An entry's `fVirt` flags.
pub mod flag {
    pub const VIRTUAL_KEY: u8 = 0x01;
    pub const SHIFT: u8 = 0x04;
    pub const CONTROL: u8 = 0x08;
    pub const ALT: u8 = 0x10;
}

/// One accelerator: its flags, its key (a virtual key, or a character) and its command id.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Accelerator {
    pub flags: u8,
    pub key: u16,
    pub command: u16,
}

/// One table.
#[derive(Debug, Clone, Copy)]
pub struct AcceleratorTable {
    entries: [Accelerator; MAX_ENTRIES],
    count: usize,
}

/// The modifiers held when a key message came.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Held {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
}

fn word(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

impl AcceleratorTable {
    fn new() -> Self {
        Self {
            entries: [Accelerator::default(); MAX_ENTRIES],
            count: 0,
        }
    }

    /// A table of the `ACCEL` entries in `bytes` (6 bytes each: `fVirt`, a pad byte, `key`,
    /// `cmd`); `None` when there is none or more than [`MAX_ENTRIES`].
    pub fn from_accel(bytes: &[u8]) -> Option<Self> {
        let count = bytes.len() / ACCEL_SIZE;
        if count == 0 || count > MAX_ENTRIES {
            return None;
        }
        let mut table = Self::new();
        for (slot, entry) in table.entries.iter_mut().zip(bytes.chunks_exact(ACCEL_SIZE)) {
            *slot = Accelerator {
                flags: entry[0],
                key: word(entry, 2),
                command: word(entry, 4),
            };
        }
        table.count = count;
        Some(table)
    }

    /// A table of an `RT_ACCELERATOR` resource's entries (8 bytes each: `fFlags`, `wAnsi`,
    /// `wId` and padding, the last with `0x80` in its flags); `None` when it holds none or
    /// more than [`MAX_ENTRIES`].
    pub fn from_resource(bytes: &[u8]) -> Option<Self> {
        let mut table = Self::new();
        for entry in bytes.chunks_exact(RESOURCE_ENTRY_SIZE) {
            let flags = word(entry, 0);
            *table.entries.get_mut(table.count)? = Accelerator {
                flags: flags as u8 & !(LAST_ENTRY as u8),
                key: word(entry, 2),
                command: word(entry, 4),
            };
            table.count += 1;
            if flags & LAST_ENTRY != 0 {
                break;
            }
        }
        (table.count > 0).then_some(table)
    }

    pub fn entries(&self) -> &[Accelerator] {
        &self.entries[..self.count]
    }

    /// The command of the first entry key message `number` with `wparam` matches, `held` the
    /// modifiers then: a `WM_KEYDOWN` or `WM_SYSKEYDOWN` matches a virtual-key entry of that
    /// key and exactly those modifiers, a `WM_CHAR` a character entry without `FALT` and a
    /// `WM_SYSCHAR` one with it.
    pub fn command_for(&self, number: u32, wparam: usize, held: Held) -> Option<u16> {
        let key = u16::try_from(wparam).ok()?;
        self.entries().iter().find_map(|entry| {
            let virtual_key = entry.flags & flag::VIRTUAL_KEY != 0;
            let wants = |flag: u8| entry.flags & flag != 0;
            let matched = match number {
                message::KEY_DOWN | message::SYS_KEY_DOWN => {
                    virtual_key
                        && entry.key == key
                        && wants(flag::SHIFT) == held.shift
                        && wants(flag::CONTROL) == held.control
                        && wants(flag::ALT) == held.alt
                }
                message::CHAR => !virtual_key && !wants(flag::ALT) && entry.key == key,
                message::SYS_CHAR => !virtual_key && wants(flag::ALT) && entry.key == key,
                _ => false,
            };
            matched.then_some(entry.command)
        })
    }
}

/// The program's accelerator tables by handle.
pub type AcceleratorTables = Slots<AcceleratorTable, MAX_TABLES>;

pub const fn new_tables() -> AcceleratorTables {
    Slots::new(HANDLE_BASE)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VK_F1: u16 = 0x70;

    #[test]
    fn accel_entries_make_a_table_that_matches_keys_with_exact_modifiers() {
        let bytes = [
            flag::VIRTUAL_KEY | flag::CONTROL,
            0,
            b'O',
            0,
            0x34,
            0x12,
            flag::VIRTUAL_KEY,
            0,
            VK_F1 as u8,
            0,
            7,
            0,
            0,
            0,
            b'q',
            0,
            8,
            0,
        ];
        let table = AcceleratorTable::from_accel(&bytes).unwrap();
        assert_eq!(table.entries().len(), 3);
        let control = Held {
            control: true,
            ..Held::default()
        };
        let key = usize::from(b'O');
        assert_eq!(
            table.command_for(message::KEY_DOWN, key, control),
            Some(0x1234)
        );
        assert_eq!(
            table.command_for(message::KEY_DOWN, key, Held::default()),
            None
        );
        let control_shift = Held {
            shift: true,
            ..control
        };
        assert_eq!(
            table.command_for(message::KEY_DOWN, key, control_shift),
            None
        );
        assert_eq!(table.command_for(message::KEY_UP, key, control), None);
        assert_eq!(
            table.command_for(message::KEY_DOWN, usize::from(VK_F1), Held::default()),
            Some(7)
        );
        assert_eq!(
            table.command_for(message::CHAR, usize::from(b'q'), Held::default()),
            Some(8)
        );
        assert_eq!(
            table.command_for(message::SYS_CHAR, usize::from(b'q'), Held::default()),
            None
        );
        assert!(AcceleratorTable::from_accel(&[]).is_none());
        assert!(AcceleratorTable::from_accel(&[0u8; ACCEL_SIZE * (MAX_ENTRIES + 1)]).is_none());
    }

    #[test]
    fn a_resource_table_ends_at_the_entry_marked_last() {
        let entry = |flags: u16, key: u16, id: u16| {
            let mut bytes = Vec::new();
            for value in [flags, key, id, 0] {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
            bytes
        };
        let mut bytes = entry(3, VK_F1, 65411);
        bytes.extend(entry(0x8b, u16::from(b'R'), 65400));
        bytes.extend(entry(3, 0x71, 1));
        let table = AcceleratorTable::from_resource(&bytes).unwrap();
        assert_eq!(table.entries().len(), 2);
        assert_eq!(
            table.entries()[1],
            Accelerator {
                flags: 0x0b,
                key: u16::from(b'R'),
                command: 65400
            }
        );
        let held = Held {
            control: true,
            ..Held::default()
        };
        assert_eq!(
            table.command_for(message::KEY_DOWN, usize::from(b'R'), held),
            Some(65400)
        );
        assert!(AcceleratorTable::from_resource(&[1, 2, 3]).is_none());
    }

    #[test]
    fn tables_are_kept_by_handle() {
        let mut tables = new_tables();
        let table = AcceleratorTable::from_accel(&[1, 0, 65, 0, 1, 0]).unwrap();
        let handle = tables.insert(table).unwrap();
        assert_eq!(
            tables.get(handle).map(|table| table.entries().len()),
            Some(1)
        );
        assert!(tables.remove(handle).is_some());
        assert!(tables.get(handle).is_none());
    }
}
