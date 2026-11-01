//! The translation cache over a directory of `.tx` files: [`load`] checks a file against its
//! key and marks it launched (a damaged one is removed), [`store`] evicts what the cap asks
//! for, writes a temporary file and renames it over the final name, so a reader never sees a
//! partial file. The paths and the directory records the disk directory reads are here too.

use alloc::string::String;
use alloc::vec::Vec;
use eggos_translate::cache::{self, CacheFile, policy};

const DIRENT_RECORD_LENGTH: usize = 16;
const DIRENT_NAME: usize = 19;

/// A cache directory: the `.tx` files of `policy::DIRECTORY` on the disk, or a fake one in
/// tests. Names are file names in the directory.
pub trait CacheFiles {
    /// The whole file, or `None` if it cannot be read.
    fn read(&mut self, name: &str) -> Option<Vec<u8>>;
    /// Creates a new file holding `bytes`; false if it exists or cannot be written.
    fn create(&mut self, name: &str, bytes: &[u8]) -> bool;
    /// Moves `from` to `to`, replacing it, in one step.
    fn rename(&mut self, from: &str, to: &str) -> bool;
    /// Removes the file.
    fn remove(&mut self, name: &str);
    /// The files in the directory.
    fn list(&mut self) -> Vec<policy::CachedFile>;
    /// Marks the file as launched now.
    fn touch(&mut self, name: &str);
    /// The cache's cap in bytes.
    fn cap(&mut self) -> u64;
    /// The time in seconds.
    fn now(&mut self) -> u64;
}

/// The cached translation for `key`, checked, and marked as launched now; `None` when there
/// is none or it is damaged, stale or for another key (a damaged one is removed).
pub fn load(files: &mut impl CacheFiles, key: &[u8; 32]) -> Option<CacheFile> {
    let name = policy::file_name(key);
    let bytes = files.read(&name)?;
    match cache::parse(&bytes) {
        Ok(file) if cache::cache_key(&file.input_digest, file.mode) == *key => {
            files.touch(&name);
            Some(file)
        }
        _ => {
            files.remove(&name);
            None
        }
    }
}

/// Stores `bytes`, the serialized translation for `key`, written by `writer` (the process
/// id): first the evictions the cap asks for, then a temporary file renamed over the final
/// name. False when it could not be stored; the program still runs from the translation in
/// memory.
pub fn store(files: &mut impl CacheFiles, key: &[u8; 32], bytes: &[u8], writer: u64) -> bool {
    let cap = files.cap();
    if bytes.len() as u64 > cap {
        return false;
    }
    let now = files.now();
    let listed = files.list();
    for name in policy::evictions(&listed, cap, bytes.len() as u64, now) {
        files.remove(&name);
    }
    let temporary: String = policy::temporary_name(key, writer);
    if !files.create(&temporary, bytes) {
        files.remove(&temporary);
        return false;
    }
    if !files.rename(&temporary, &policy::file_name(key)) {
        files.remove(&temporary);
        return false;
    }
    true
}

/// The NUL-terminated path of the cache directory.
pub fn directory_path() -> Vec<u8> {
    let mut path = Vec::from(policy::DIRECTORY.as_bytes());
    path.push(0);
    path
}

/// The NUL-terminated path of the file `name` in the cache directory.
pub fn file_path(name: &str) -> Vec<u8> {
    let mut path = Vec::with_capacity(policy::DIRECTORY.len() + name.len() + 2);
    path.extend_from_slice(policy::DIRECTORY.as_bytes());
    path.push(b'/');
    path.extend_from_slice(name.as_bytes());
    path.push(0);
    path
}

/// The NUL-terminated directories to create, outermost first, so that the cache directory
/// exists: every parent below the root, then the directory itself.
pub fn directories_to_create() -> Vec<Vec<u8>> {
    let full = policy::DIRECTORY.as_bytes();
    let mut paths: Vec<Vec<u8>> = full
        .iter()
        .enumerate()
        .skip(1)
        .filter(|&(_, &byte)| byte == b'/')
        .map(|(index, _)| {
            let mut parent = Vec::from(&full[..index]);
            parent.push(0);
            parent
        })
        .collect();
    paths.push(directory_path());
    paths
}

/// The names in `records`, Linux `getdents64` records as one read returned them, without
/// `.` and `..`.
pub fn entry_names(records: &[u8]) -> Vec<&[u8]> {
    let mut names = Vec::new();
    let mut at = 0;
    while at + DIRENT_NAME < records.len() {
        let length = usize::from(u16::from_le_bytes([
            records[at + DIRENT_RECORD_LENGTH],
            records[at + DIRENT_RECORD_LENGTH + 1],
        ]));
        if length == 0 {
            break;
        }
        let record = &records[at..(at + length).min(records.len())];
        at += length;
        let name = &record[DIRENT_NAME.min(record.len())..];
        let name = &name[..name.iter().position(|&b| b == 0).unwrap_or(name.len())];
        if name != b"." && name != b".." {
            names.push(name);
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use eggos_translate::cache::{input_digest, serialize};
    use eggos_translate::codegen::CodegenMode;
    use eggos_translate::image::{ImageInput, Section, Seed, SeedOrigin, translate_image};
    use std::collections::BTreeMap;

    const IMAGE_BASE: u64 = 0x1_4000_0000;
    const TEXT: u32 = 0x1000;

    #[derive(Default)]
    struct FakeCache {
        files: BTreeMap<String, (Vec<u8>, u64)>,
        cap: u64,
        now: u64,
        fail_rename: bool,
    }

    impl CacheFiles for FakeCache {
        fn read(&mut self, name: &str) -> Option<Vec<u8>> {
            self.files.get(name).map(|(bytes, _)| bytes.clone())
        }
        fn create(&mut self, name: &str, bytes: &[u8]) -> bool {
            if self.files.contains_key(name) {
                return false;
            }
            self.files.insert(name.into(), (bytes.to_vec(), self.now));
            true
        }
        fn rename(&mut self, from: &str, to: &str) -> bool {
            if self.fail_rename {
                return false;
            }
            match self.files.remove(from) {
                Some(file) => {
                    self.files.insert(to.into(), file);
                    true
                }
                None => false,
            }
        }
        fn remove(&mut self, name: &str) {
            self.files.remove(name);
        }
        fn list(&mut self) -> Vec<policy::CachedFile> {
            self.files
                .iter()
                .map(|(name, (bytes, last_launch))| policy::CachedFile {
                    name: name.clone(),
                    bytes: bytes.len() as u64,
                    last_launch: *last_launch,
                    in_use: false,
                })
                .collect()
        }
        fn touch(&mut self, name: &str) {
            let now = self.now;
            if let Some(file) = self.files.get_mut(name) {
                file.1 = now;
            }
        }
        fn cap(&mut self) -> u64 {
            self.cap
        }
        fn now(&mut self) -> u64 {
            self.now
        }
    }

    fn cache_file(program: &[u8]) -> (CacheFile, [u8; 32]) {
        let code = [0xc3u8];
        let sections = [Section {
            rva: TEXT,
            bytes: &code,
            executable: true,
        }];
        let seeds = [Seed {
            rva: TEXT,
            origin: SeedOrigin::EntryPoint,
        }];
        let input = ImageInput {
            image_base: IMAGE_BASE,
            sections: &sections,
            seeds: &seeds,
        };
        let mode = CodegenMode::default();
        let digest = input_digest(program);
        let file = CacheFile {
            input_digest: digest,
            mode,
            image_base: IMAGE_BASE,
            image: translate_image(&input, mode).unwrap(),
        };
        (file, cache::cache_key(&digest, mode))
    }

    #[test]
    fn a_stored_translation_is_renamed_into_place_and_loads_back_touched() {
        let (file, key) = cache_file(b"program one");
        let bytes = serialize(&file);
        let mut cache = FakeCache {
            cap: 1 << 30,
            now: 100,
            ..FakeCache::default()
        };
        assert!(store(&mut cache, &key, &bytes, 42));
        assert_eq!(cache.files.len(), 1);
        let name = policy::file_name(&key);
        assert!(cache.files.contains_key(&name));
        cache.now = 200;
        let loaded = load(&mut cache, &key).unwrap();
        assert_eq!(loaded.image.code, file.image.code);
        assert_eq!(cache.files[&name].1, 200);
    }

    #[test]
    fn a_failed_rename_leaves_no_temporary_and_a_damaged_file_is_removed() {
        let (file, key) = cache_file(b"program two");
        let bytes = serialize(&file);
        let mut cache = FakeCache {
            cap: 1 << 30,
            fail_rename: true,
            ..FakeCache::default()
        };
        assert!(!store(&mut cache, &key, &bytes, 7));
        assert!(cache.files.is_empty());
        let mut damaged = bytes.clone();
        let middle = damaged.len() / 2;
        damaged[middle] ^= 0xff;
        cache.files.insert(policy::file_name(&key), (damaged, 0));
        assert!(load(&mut cache, &key).is_none());
        assert!(cache.files.is_empty());
    }

    #[test]
    fn storing_past_the_cap_evicts_the_least_recently_launched_file() {
        let (first, first_key) = cache_file(b"first");
        let (second, second_key) = cache_file(b"second");
        let first = serialize(&first);
        let second = serialize(&second);
        let mut cache = FakeCache {
            cap: first.len() as u64 + second.len() as u64 / 2,
            now: 10,
            ..FakeCache::default()
        };
        assert!(store(&mut cache, &first_key, &first, 1));
        cache.now = 20;
        assert!(store(&mut cache, &second_key, &second, 1));
        assert!(!cache.files.contains_key(&policy::file_name(&first_key)));
        assert!(cache.files.contains_key(&policy::file_name(&second_key)));
        let mut tiny = FakeCache::default();
        assert!(!store(&mut tiny, &first_key, &first, 1));
    }

    #[test]
    fn the_paths_name_the_cache_directory_and_its_parents() {
        assert_eq!(file_path("ab.tx"), b"/var/cache/eggos/translate/ab.tx\0");
        assert_eq!(directory_path(), b"/var/cache/eggos/translate\0");
        let made = directories_to_create();
        let expected: [&[u8]; 4] = [
            b"/var\0",
            b"/var/cache\0",
            b"/var/cache/eggos\0",
            b"/var/cache/eggos/translate\0",
        ];
        assert_eq!(made, expected);
    }

    #[test]
    fn directory_records_give_their_names_without_dot_entries() {
        let record = |name: &[u8]| {
            let length = (DIRENT_NAME + name.len() + 1).next_multiple_of(8);
            let mut bytes = vec![0u8; length];
            bytes[DIRENT_RECORD_LENGTH..DIRENT_RECORD_LENGTH + 2]
                .copy_from_slice(&(length as u16).to_le_bytes());
            bytes[DIRENT_NAME..DIRENT_NAME + name.len()].copy_from_slice(name);
            bytes
        };
        let mut records = record(b".");
        records.extend(record(b".."));
        records.extend(record(b"0123.tx"));
        records.extend(record(b".0123.7.tmp"));
        let expected: [&[u8]; 2] = [b"0123.tx", b".0123.7.tmp"];
        assert_eq!(entry_names(&records), expected);
        assert!(entry_names(&[]).is_empty());
    }
}
