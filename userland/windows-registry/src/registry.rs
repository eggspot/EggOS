//! The registry over two hives: the user's (`HKEY_CURRENT_USER`, and `HKEY_USERS\<sid>`) and the
//! machine's (`HKEY_LOCAL_MACHINE`, `HKEY_CURRENT_CONFIG` inside it), with `HKEY_CLASSES_ROOT`
//! the user's `Software\Classes` over the machine's: a key the user has hides the machine's
//! values, subkeys of both are listed, and writes go to the user's. Only root changes the
//! machine's hive. Reads use the hive as last read while its file's stamp is unchanged; every
//! write reads the file afresh, changes it and writes it whole to a temporary file renamed over
//! the hive, so a crash leaves the old hive or the new one, never half of one.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::hive::{Hive, Key, Value, is_value_name};
use crate::path::{KeyPath, Root, utf16};
use crate::{Error, MAX_DATA_BYTES, MAX_DEPTH, same_name};

/// One of the two hive files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HiveFile {
    /// The user's, below the home folder.
    User,
    /// The machine's.
    Machine,
}

/// Where the user's hive is, below the home folder.
pub const USER_HIVE_PATH: &str = ".local/share/eggos/windows/user.hive";
/// Where the machine's hive is.
pub const MACHINE_HIVE_PATH: &str = "/var/lib/eggos/windows/machine.hive";
/// What the temporary file's name adds to the hive's.
pub const TEMPORARY_SUFFIX: &str = ".new";
/// What a damaged hive's name gets when `reset` sets it aside.
pub const DAMAGED_SUFFIX: &str = ".damaged";

/// Why the store could not do what it was asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    /// The disk is full.
    DiskFull,
    /// Anything else.
    Failed,
}

/// The files and facts the registry needs from the system.
pub trait Store {
    /// The hive file's bytes, or `None` when it does not exist.
    fn read(&mut self, file: HiveFile) -> Result<Option<Vec<u8>>, StoreError>;
    /// A number that changes whenever the file is replaced, or `None` when it does not exist.
    fn stamp(&mut self, file: HiveFile) -> Option<u64>;
    /// Writes `bytes` in full to the file's temporary twin (making its folder) and flushes it.
    fn write_temporary(&mut self, file: HiveFile, bytes: &[u8]) -> Result<(), StoreError>;
    /// Renames the temporary twin over the file.
    fn replace_with_temporary(&mut self, file: HiveFile) -> Result<(), StoreError>;
    /// Renames the file to its damaged twin, replacing an older one; nothing when it is absent.
    fn set_aside(&mut self, file: HiveFile) -> Result<(), StoreError>;
    /// The user's id: 0 is root, who alone changes the machine's hive.
    fn user_id(&mut self) -> u32;
    /// The time now, in 100 ns since 1601.
    fn now(&mut self) -> u64;
}

/// The user's security identifier, the one key below `HKEY_USERS`: root is an
/// administrator (relative id 500), every other user `1000 + uid`.
pub fn user_sid(user_id: u32) -> String {
    let relative = match user_id {
        0 => 500,
        id => 1000 + u64::from(id),
    };
    format!("S-1-5-21-0-0-0-{relative}")
}

/// What `RegQueryInfoKeyW` tells about a key.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyInfo {
    /// How many subkeys it has.
    pub subkeys: usize,
    /// How many values it has.
    pub values: usize,
    /// The longest subkey name, in UTF-16 units.
    pub longest_subkey: usize,
    /// The longest value name, in UTF-16 units.
    pub longest_value_name: usize,
    /// The largest value's data, in bytes.
    pub longest_data: usize,
    /// When it last changed.
    pub last_write: u64,
}

#[derive(Debug, Default)]
struct Cached {
    stamp: Option<u64>,
    state: Option<Result<Hive, Error>>,
}

#[derive(Debug, Clone)]
struct Place {
    file: HiveFile,
    names: Vec<Vec<u16>>,
}

enum Resolved {
    UsersRoot,
    Places(Vec<Place>),
}

const CLASSES: [&str; 2] = ["Software", "Classes"];
const CURRENT_CONFIG: [&str; 4] = [
    "System",
    "CurrentControlSet",
    "Hardware Profiles",
    "Current",
];

/// The registry: both hives through a [`Store`].
#[derive(Debug)]
pub struct Registry<S: Store> {
    store: S,
    user: Cached,
    machine: Cached,
}

fn load<'a>(
    store: &mut impl Store,
    slot: &'a mut Cached,
    file: HiveFile,
) -> Result<&'a Hive, Error> {
    let stamp = store.stamp(file);
    if slot.state.is_none() || slot.stamp != stamp {
        slot.state = Some(match store.read(file) {
            Ok(None) => Ok(Hive::default()),
            Ok(Some(bytes)) => Hive::decode(&bytes),
            Err(_) => Err(Error::ReadFailed),
        });
        slot.stamp = stamp;
    }
    match &slot.state {
        Some(Ok(hive)) => Ok(hive),
        Some(Err(error)) => Err(*error),
        None => Err(Error::ReadFailed),
    }
}

fn prefixed(prefix: &[&str], names: &[Vec<u16>]) -> Vec<Vec<u16>> {
    prefix
        .iter()
        .map(|name| utf16(name))
        .chain(names.iter().cloned())
        .collect()
}

fn ensure_key<'a>(root: &'a mut Key, names: &[Vec<u16>], now: u64) -> &'a mut Key {
    let mut key = root;
    for name in names {
        let index = match key.subkey_index(name) {
            Ok(index) => index,
            Err(index) => {
                key.subkeys.insert(index, Key::named(name, now));
                key.last_write = now;
                index
            }
        };
        key = &mut key.subkeys[index];
    }
    key
}

impl<S: Store> Registry<S> {
    /// A registry over `store`; nothing is read until it is used.
    pub fn new(store: S) -> Registry<S> {
        Registry {
            store,
            user: Cached::default(),
            machine: Cached::default(),
        }
    }

    /// The store, as given.
    pub fn store(&mut self) -> &mut S {
        &mut self.store
    }

    /// The user's security identifier.
    pub fn user_sid(&mut self) -> String {
        user_sid(self.store.user_id())
    }

    /// Whether this process may change the machine's hive.
    pub fn may_change_machine(&mut self) -> bool {
        self.store.user_id() == 0
    }

    /// The hive, read when its file changed: [`Error::HiveDamaged`] or [`Error::ReadFailed`]
    /// when it cannot be read.
    pub fn hive(&mut self, file: HiveFile) -> Result<&Hive, Error> {
        let slot = match file {
            HiveFile::User => &mut self.user,
            HiveFile::Machine => &mut self.machine,
        };
        load(&mut self.store, slot, file)
    }

    fn resolve(&mut self, path: &KeyPath) -> Result<Resolved, Error> {
        let place = |file, names| Place { file, names };
        Ok(Resolved::Places(match path.root {
            Root::CurrentUser => [place(HiveFile::User, path.names.clone())].into(),
            Root::LocalMachine => [place(HiveFile::Machine, path.names.clone())].into(),
            Root::CurrentConfig => [place(
                HiveFile::Machine,
                prefixed(&CURRENT_CONFIG, &path.names),
            )]
            .into(),
            Root::ClassesRoot => [
                place(HiveFile::User, prefixed(&CLASSES, &path.names)),
                place(HiveFile::Machine, prefixed(&CLASSES, &path.names)),
            ]
            .into(),
            Root::Users => {
                let Some((first, rest)) = path.names.split_first() else {
                    return Ok(Resolved::UsersRoot);
                };
                if !same_name(first, &utf16(&self.user_sid())) {
                    return Err(Error::NoSuchKey);
                }
                [place(HiveFile::User, rest.to_vec())].into()
            }
        }))
    }

    fn with_key<R>(
        &mut self,
        place: &Place,
        read: impl FnOnce(&Key) -> R,
    ) -> Result<Option<R>, Error> {
        Ok(self.hive(place.file)?.root.find(&place.names).map(read))
    }

    fn is_virtual_root(path: &KeyPath) -> bool {
        path.names.is_empty()
    }

    /// Whether the key exists; a predefined key always does.
    pub fn key_exists(&mut self, path: &KeyPath) -> Result<bool, Error> {
        if Self::is_virtual_root(path) {
            return Ok(true);
        }
        match self.resolve(path) {
            Ok(Resolved::UsersRoot) => Ok(true),
            Ok(Resolved::Places(places)) => {
                for place in &places {
                    if self.with_key(place, |_| ())?.is_some() {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Err(Error::NoSuchKey) => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// The key's subkeys' names, sorted ignoring case.
    pub fn subkeys(&mut self, path: &KeyPath) -> Result<Vec<Vec<u16>>, Error> {
        let places = match self.resolve(path)? {
            Resolved::UsersRoot => return Ok([utf16(&self.user_sid())].into()),
            Resolved::Places(places) => places,
        };
        let mut found = false;
        let mut names: Vec<Vec<u16>> = Vec::new();
        for place in &places {
            if let Some(more) = self.with_key(place, |key| {
                key.subkeys
                    .iter()
                    .map(|key| key.name.clone())
                    .collect::<Vec<_>>()
            })? {
                found = true;
                names.extend(more);
            }
        }
        if !found && !Self::is_virtual_root(path) {
            return Err(Error::NoSuchKey);
        }
        names.sort_by(|a, b| crate::compare_names(a, b));
        names.dedup_by(|a, b| same_name(a, b));
        Ok(names)
    }

    /// The key's values, in the order they were made: the first hive's that has the key.
    pub fn values(&mut self, path: &KeyPath) -> Result<Vec<Value>, Error> {
        let places = match self.resolve(path)? {
            Resolved::UsersRoot => return Ok(Vec::new()),
            Resolved::Places(places) => places,
        };
        for place in &places {
            if let Some(values) = self.with_key(place, |key| key.values.clone())? {
                return Ok(values);
            }
        }
        match Self::is_virtual_root(path) {
            true => Ok(Vec::new()),
            false => Err(Error::NoSuchKey),
        }
    }

    /// The value named `name` (empty for the default value).
    pub fn value(&mut self, path: &KeyPath, name: &[u16]) -> Result<Value, Error> {
        if !self.key_exists(path)? {
            return Err(Error::NoSuchKey);
        }
        self.values(path)?
            .into_iter()
            .find(|value| same_name(&value.name, name))
            .ok_or(Error::NoSuchValue)
    }

    /// What `RegQueryInfoKeyW` tells about the key.
    pub fn info(&mut self, path: &KeyPath) -> Result<KeyInfo, Error> {
        let subkeys = self.subkeys(path)?;
        let values = self.values(path)?;
        let mut last_write = 0;
        if let Resolved::Places(places) = self.resolve(path)? {
            for place in &places {
                if let Some(time) = self.with_key(place, |key| key.last_write)? {
                    last_write = time;
                    break;
                }
            }
        }
        Ok(KeyInfo {
            subkeys: subkeys.len(),
            values: values.len(),
            longest_subkey: subkeys.iter().map(Vec::len).max().unwrap_or(0),
            longest_value_name: values.iter().map(|value| value.name.len()).max().unwrap_or(0),
            longest_data: values.iter().map(|value| value.data.len()).max().unwrap_or(0),
            last_write,
        })
    }

    fn write_place(&mut self, path: &KeyPath) -> Result<Place, Error> {
        match self.resolve(path)? {
            Resolved::UsersRoot => Err(Error::RootKey),
            Resolved::Places(mut places) => {
                let place = places.swap_remove(0);
                if place.names.len() > MAX_DEPTH {
                    return Err(Error::KeyTooDeep);
                }
                Ok(place)
            }
        }
    }

    fn change<R>(
        &mut self,
        file: HiveFile,
        edit: impl FnOnce(&mut Hive, u64) -> Result<R, Error>,
    ) -> Result<R, Error> {
        if file == HiveFile::Machine && !self.may_change_machine() {
            return Err(Error::ReadOnlyKey);
        }
        let slot = match file {
            HiveFile::User => &mut self.user,
            HiveFile::Machine => &mut self.machine,
        };
        slot.state = None;
        load(&mut self.store, slot, file)?;
        let now = self.store.now();
        let Some(Ok(hive)) = &mut slot.state else {
            return Err(Error::ReadFailed);
        };
        let saved = edit(hive, now).and_then(|result| Ok((result, hive.encode()?)));
        let (result, bytes) = match saved {
            Ok(saved) => saved,
            Err(error) => {
                slot.state = None;
                return Err(error);
            }
        };
        let written = self
            .store
            .write_temporary(file, &bytes)
            .and_then(|()| self.store.replace_with_temporary(file));
        match written {
            Ok(()) => {
                slot.stamp = self.store.stamp(file);
                Ok(result)
            }
            Err(error) => {
                slot.state = None;
                Err(match error {
                    StoreError::DiskFull => Error::DiskFull,
                    StoreError::Failed => Error::WriteFailed,
                })
            }
        }
    }

    /// Makes the key and the keys above it that are missing; `true` when it made the key.
    pub fn create_key(&mut self, path: &KeyPath) -> Result<bool, Error> {
        if self.key_exists(path)? {
            return Ok(false);
        }
        let place = self.write_place(path)?;
        self.change(place.file, |hive, now| {
            ensure_key(&mut hive.root, &place.names, now);
            Ok(true)
        })
    }

    /// Sets the value named `name` of an existing key, making or replacing it.
    pub fn set_value(
        &mut self,
        path: &KeyPath,
        name: &[u16],
        kind: u32,
        data: Vec<u8>,
    ) -> Result<(), Error> {
        if !is_value_name(name) {
            return Err(Error::BadName);
        }
        if data.len() > MAX_DATA_BYTES {
            return Err(Error::ValueTooLarge);
        }
        if !self.key_exists(path)? {
            return Err(Error::NoSuchKey);
        }
        let place = self.write_place(path)?;
        self.change(place.file, |hive, now| {
            let key = ensure_key(&mut hive.root, &place.names, now);
            match key.values.iter_mut().find(|value| same_name(&value.name, name)) {
                Some(value) => {
                    value.kind = kind;
                    value.data = data;
                }
                None => key.values.push(Value {
                    name: name.to_vec(),
                    kind,
                    data,
                }),
            }
            key.last_write = now;
            Ok(())
        })
    }

    /// Deletes the value named `name`.
    pub fn delete_value(&mut self, path: &KeyPath, name: &[u16]) -> Result<(), Error> {
        if !self.key_exists(path)? {
            return Err(Error::NoSuchKey);
        }
        self.value(path, name)?;
        let place = self.write_place(path)?;
        self.change(place.file, |hive, now| {
            let key = hive.root.find_mut(&place.names).ok_or(Error::ReadOnlyKey)?;
            let index = key
                .values
                .iter()
                .position(|value| same_name(&value.name, name))
                .ok_or(Error::ReadOnlyKey)?;
            key.values.remove(index);
            key.last_write = now;
            Ok(())
        })
    }

    /// Deletes the key, and with `with_subkeys` everything below it; without, a key with
    /// subkeys is [`Error::HasSubkeys`].
    pub fn delete_key(&mut self, path: &KeyPath, with_subkeys: bool) -> Result<(), Error> {
        if Self::is_virtual_root(path) {
            return Err(Error::RootKey);
        }
        if !self.key_exists(path)? {
            return Err(Error::NoSuchKey);
        }
        let place = self.write_place(path)?;
        let Some((last, above)) = place.names.split_last() else {
            return Err(Error::RootKey);
        };
        self.change(place.file, |hive, now| {
            let parent = hive.root.find_mut(above).ok_or(Error::ReadOnlyKey)?;
            let index = parent.subkey_index(last).map_err(|_| Error::ReadOnlyKey)?;
            if !with_subkeys && !parent.subkeys[index].subkeys.is_empty() {
                return Err(Error::HasSubkeys);
            }
            parent.subkeys.remove(index);
            parent.last_write = now;
            Ok(())
        })
    }

    /// Deletes every subkey and value of the key, keeping the key (`RegDeleteTreeW` without a
    /// subkey).
    pub fn clear_key(&mut self, path: &KeyPath) -> Result<(), Error> {
        if !self.key_exists(path)? {
            return Err(Error::NoSuchKey);
        }
        let place = self.write_place(path)?;
        self.change(place.file, |hive, now| {
            if let Some(key) = hive.root.find_mut(&place.names) {
                key.subkeys.clear();
                key.values.clear();
                key.last_write = now;
            }
            Ok(())
        })
    }

    /// How many keys and values are below the key, for a deletion's confirmation.
    pub fn count_below(&mut self, path: &KeyPath) -> Result<(usize, usize), Error> {
        let places = match self.resolve(path)? {
            Resolved::UsersRoot => return Ok((1, 0)),
            Resolved::Places(places) => places,
        };
        Ok(self
            .with_key(&places[0], Key::count_below)?
            .unwrap_or((0, 0)))
    }

    /// Sets a hive's file aside as damaged and starts an empty one.
    pub fn reset(&mut self, file: HiveFile) -> Result<(), Error> {
        if file == HiveFile::Machine && !self.may_change_machine() {
            return Err(Error::ReadOnlyKey);
        }
        let failed = |error| match error {
            StoreError::DiskFull => Error::DiskFull,
            StoreError::Failed => Error::WriteFailed,
        };
        let bytes = Hive::default().encode()?;
        self.store.set_aside(file).map_err(failed)?;
        self.store.write_temporary(file, &bytes).map_err(failed)?;
        self.store.replace_with_temporary(file).map_err(failed)?;
        let slot = match file {
            HiveFile::User => &mut self.user,
            HiveFile::Machine => &mut self.machine,
        };
        slot.state = None;
        Ok(())
    }
}
