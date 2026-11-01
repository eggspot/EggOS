//! The Windows registry that Windows programs on EggOS see (D230): a hive file of our own per
//! user and one for the machine ([`hive`]), the predefined keys over them with
//! `HKEY_CLASSES_ROOT` merged from both ([`registry`]), key paths and value types in the words
//! regedit uses ([`path`], [`value`]), and the `windows registry` command ([`command`]). The
//! Windows layer's advapi32, the command and Settings' Windows pane all go through
//! [`registry::Registry`], so every front-end reads and writes the same way. Pure logic: the
//! files are reached through [`registry::Store`].

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod command;
pub mod hive;
pub mod path;
pub mod registry;
pub mod value;

#[cfg(test)]
mod tests;

/// The longest key name, in UTF-16 units.
pub const MAX_KEY_NAME_UNITS: usize = 255;
/// The longest value name, in UTF-16 units.
pub const MAX_VALUE_NAME_UNITS: usize = 16_383;
/// The most data one value holds, in bytes.
pub const MAX_DATA_BYTES: usize = 1 << 20;
/// The deepest key below a hive's root.
pub const MAX_DEPTH: usize = 512;
/// The largest hive file, in bytes.
pub const MAX_HIVE_BYTES: usize = 16 << 20;

/// Why a registry operation failed; each has the command model's code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The key does not exist.
    NoSuchKey,
    /// The value does not exist.
    NoSuchValue,
    /// The key is in the machine's hive and only root may change it.
    ReadOnlyKey,
    /// A predefined key cannot be created, deleted or renamed.
    RootKey,
    /// The key has subkeys, so it cannot be deleted alone.
    HasSubkeys,
    /// A name is empty where it may not be, too long, or holds `\` in a key name.
    BadName,
    /// The data cannot be read as the value's type.
    BadData,
    /// The value is larger than [`MAX_DATA_BYTES`].
    ValueTooLarge,
    /// The key would be deeper than [`MAX_DEPTH`].
    KeyTooDeep,
    /// The hive would be larger than [`MAX_HIVE_BYTES`].
    HiveFull,
    /// The disk has no room for the hive.
    DiskFull,
    /// The hive file is damaged.
    HiveDamaged,
    /// The hive file could not be read.
    ReadFailed,
    /// The hive file could not be written.
    WriteFailed,
}

impl Error {
    /// Its code in the command model.
    pub const fn code(self) -> &'static str {
        match self {
            Error::NoSuchKey => "no_such_key",
            Error::NoSuchValue => "no_such_value",
            Error::ReadOnlyKey => "read_only_key",
            Error::RootKey => "root_key",
            Error::HasSubkeys => "has_subkeys",
            Error::BadName => "bad_name",
            Error::BadData => "bad_data",
            Error::ValueTooLarge => "value_too_large",
            Error::KeyTooDeep => "key_too_deep",
            Error::HiveFull => "hive_full",
            Error::DiskFull => "no_space",
            Error::HiveDamaged => "hive_damaged",
            Error::ReadFailed => "io_error",
            Error::WriteFailed => "write_failed",
        }
    }
}

/// `unit` as Windows compares names: upper case, so `Software` and `SOFTWARE` are one key.
pub fn fold(unit: u16) -> u16 {
    if unit < 0x80 {
        return u16::from((unit as u8).to_ascii_uppercase());
    }
    let Some(character) = char::from_u32(u32::from(unit)) else {
        return unit;
    };
    let mut upper = character.to_uppercase();
    match (upper.next(), upper.next()) {
        (Some(single), None) if (single as u32) <= 0xffff => single as u16,
        _ => unit,
    }
}

/// Compares two names as Windows sorts and matches them, ignoring case.
pub fn compare_names(left: &[u16], right: &[u16]) -> core::cmp::Ordering {
    left.iter()
        .map(|unit| fold(*unit))
        .cmp(right.iter().map(|unit| fold(*unit)))
}

/// Whether two names are the same name, ignoring case.
pub fn same_name(left: &[u16], right: &[u16]) -> bool {
    left.len() == right.len() && compare_names(left, right).is_eq()
}
