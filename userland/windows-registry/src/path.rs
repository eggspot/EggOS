//! Key paths in regedit's words: a predefined key's long name (`HKEY_CURRENT_USER`) or short one
//! (`HKCU`), then key names separated by `\`.

use alloc::string::String;
use alloc::vec::Vec;

use crate::Error;
use crate::hive::is_key_name;

/// A predefined key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Root {
    /// `HKEY_CLASSES_ROOT`: the user's `Software\Classes` over the machine's.
    ClassesRoot,
    /// `HKEY_CURRENT_USER`: the user's hive.
    CurrentUser,
    /// `HKEY_LOCAL_MACHINE`: the machine's hive, which only root changes.
    LocalMachine,
    /// `HKEY_USERS`: the user's hive below the user's SID.
    Users,
    /// `HKEY_CURRENT_CONFIG`: the machine's current hardware profile.
    CurrentConfig,
}

impl Root {
    /// The roots Settings shows, in its order.
    pub const SHOWN: [Root; 3] = [Root::CurrentUser, Root::LocalMachine, Root::ClassesRoot];
    /// Every root.
    pub const ALL: [Root; 5] = [
        Root::ClassesRoot,
        Root::CurrentUser,
        Root::LocalMachine,
        Root::Users,
        Root::CurrentConfig,
    ];

    /// Its long name, as every printed path starts.
    pub const fn long_name(self) -> &'static str {
        match self {
            Root::ClassesRoot => "HKEY_CLASSES_ROOT",
            Root::CurrentUser => "HKEY_CURRENT_USER",
            Root::LocalMachine => "HKEY_LOCAL_MACHINE",
            Root::Users => "HKEY_USERS",
            Root::CurrentConfig => "HKEY_CURRENT_CONFIG",
        }
    }

    /// Its short name.
    pub const fn short_name(self) -> &'static str {
        match self {
            Root::ClassesRoot => "HKCR",
            Root::CurrentUser => "HKCU",
            Root::LocalMachine => "HKLM",
            Root::Users => "HKU",
            Root::CurrentConfig => "HKCC",
        }
    }

    /// Its predefined handle (`HKEY_CLASSES_ROOT` is `0x80000000`).
    pub const fn handle(self) -> u32 {
        match self {
            Root::ClassesRoot => 0x8000_0000,
            Root::CurrentUser => 0x8000_0001,
            Root::LocalMachine => 0x8000_0002,
            Root::Users => 0x8000_0003,
            Root::CurrentConfig => 0x8000_0005,
        }
    }

    /// The root with this predefined handle.
    pub fn from_handle(handle: u32) -> Option<Root> {
        Root::ALL.into_iter().find(|root| root.handle() == handle)
    }

    /// The root this long or short name names, ignoring case.
    pub fn from_name(name: &str) -> Option<Root> {
        Root::ALL.into_iter().find(|root| {
            root.long_name().eq_ignore_ascii_case(name)
                || root.short_name().eq_ignore_ascii_case(name)
        })
    }
}

/// A key: its root and the names below it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyPath {
    /// The predefined key it is under.
    pub root: Root,
    /// The key names below the root, outermost first.
    pub names: Vec<Vec<u16>>,
}

impl KeyPath {
    /// The root itself.
    pub fn root(root: Root) -> KeyPath {
        KeyPath {
            root,
            names: Vec::new(),
        }
    }

    /// Reads `HKCU\Software\X` or `HKEY_CURRENT_USER\Software\X`; empty names between `\`s are
    /// left out, as Windows does.
    pub fn parse(text: &str) -> Result<KeyPath, Error> {
        let mut parts = text.split('\\');
        let root = parts
            .next()
            .and_then(Root::from_name)
            .ok_or(Error::NoSuchKey)?;
        let mut names = Vec::new();
        for part in parts.filter(|part| !part.is_empty()) {
            let name = utf16(part);
            if !is_key_name(&name) {
                return Err(Error::BadName);
            }
            names.push(name);
        }
        Ok(KeyPath { root, names })
    }

    /// The key `subkey` names below this one (`a\b`, empty names left out).
    pub fn join(&self, subkey: &[u16]) -> Result<KeyPath, Error> {
        let mut names = self.names.clone();
        for part in subkey.split(|unit| *unit == u16::from(b'\\')) {
            if part.is_empty() {
                continue;
            }
            if !is_key_name(part) {
                return Err(Error::BadName);
            }
            names.push(part.to_vec());
        }
        Ok(KeyPath {
            root: self.root,
            names,
        })
    }

    /// Its full path with the root's long name.
    pub fn text(&self) -> String {
        let mut text = String::from(self.root.long_name());
        for name in &self.names {
            text.push('\\');
            push_utf16(&mut text, name);
        }
        text
    }

    /// Its last name, or the root's long name.
    pub fn leaf(&self) -> String {
        match self.names.last() {
            Some(name) => utf8(name),
            None => String::from(self.root.long_name()),
        }
    }
}

/// `text` in UTF-16.
pub fn utf16(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// `units` as text, with U+FFFD for an unpaired surrogate.
pub fn utf8(units: &[u16]) -> String {
    let mut text = String::new();
    push_utf16(&mut text, units);
    text
}

fn push_utf16(text: &mut String, units: &[u16]) {
    text.extend(
        char::decode_utf16(units.iter().copied())
            .map(|character| character.unwrap_or(char::REPLACEMENT_CHARACTER)),
    );
}
