//! The cache directory's rules as pure decisions: file names from keys, the size cap, and
//! which files to remove and in what order. Only names this module produced are ever
//! removed, each by its exact name, and never one in use (D191).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// The cache directory.
pub const DIRECTORY: &str = "/var/cache/eggos/translate";

/// The cache files' extension.
pub const EXTENSION: &str = ".tx";

/// The largest cap: 2 GiB.
pub const MAX_CAP_BYTES: u64 = 2 << 30;

/// The cap's share of the volume: one twentieth (5%).
pub const VOLUME_SHARE_DIVISOR: u64 = 20;

/// How long a temporary file may sit unrenamed before it counts as left over by a writer
/// that stopped, in seconds.
pub const STALE_TEMPORARY_SECONDS: u64 = 60 * 60;

const TEMPORARY_EXTENSION: &str = ".tmp";

/// The cache's cap on a volume of `volume_bytes`: 2 GiB or 5% of the volume, whichever is
/// smaller.
pub const fn cap_bytes(volume_bytes: u64) -> u64 {
    let share = volume_bytes / VOLUME_SHARE_DIVISOR;
    if share < MAX_CAP_BYTES {
        share
    } else {
        MAX_CAP_BYTES
    }
}

fn hex(key: &[u8; 32]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(64);
    for &byte in key {
        text.push(char::from(DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(byte & 15)]));
    }
    text
}

fn parse_hex(text: &str) -> Option<[u8; 32]> {
    let digits = text.as_bytes();
    if digits.len() != 64 {
        return None;
    }
    let value = |digit: u8| match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        _ => None,
    };
    let mut key = [0u8; 32];
    for (index, byte) in key.iter_mut().enumerate() {
        *byte = (value(digits[2 * index])? << 4) | value(digits[2 * index + 1])?;
    }
    Some(key)
}

/// The cache file name of `key`: 64 lowercase hex digits and [`EXTENSION`].
pub fn file_name(key: &[u8; 32]) -> String {
    format!("{}{EXTENSION}", hex(key))
}

/// The temporary name a writer (`writer`: its process and a counter, unique in the
/// directory) writes `key`'s file under before renaming it to [`file_name`].
pub fn temporary_name(key: &[u8; 32], writer: u64) -> String {
    format!(".{}.{writer:x}{TEMPORARY_EXTENSION}", hex(key))
}

/// The key of a cache file name, or `None` when the name is not one.
pub fn key_of(name: &str) -> Option<[u8; 32]> {
    parse_hex(name.strip_suffix(EXTENSION)?)
}

fn is_temporary(name: &str) -> bool {
    let Some(rest) = name
        .strip_prefix('.')
        .and_then(|rest| rest.strip_suffix(TEMPORARY_EXTENSION))
    else {
        return false;
    };
    let Some((key, writer)) = rest.split_once('.') else {
        return false;
    };
    parse_hex(key).is_some()
        && !writer.is_empty()
        && writer.bytes().all(|digit| digit.is_ascii_hexdigit())
}

/// One file in the cache directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedFile {
    /// Its name.
    pub name: String,
    /// Its size.
    pub bytes: u64,
    /// When a program last launched from it (its modification time, which launch touches),
    /// in seconds.
    pub last_launch: u64,
    /// Whether a running program maps it, or a writer is writing it.
    pub in_use: bool,
}

/// The names to remove, in order, before writing a file of `incoming` bytes, so the cache
/// stays within `cap`: first temporaries left over by stopped writers, then the least
/// recently launched cache files. Files in use and names that are not the cache's are never
/// chosen, so the result may leave the cache above its cap.
pub fn evictions(files: &[CachedFile], cap: u64, incoming: u64, now: u64) -> Vec<String> {
    let mut chosen = Vec::new();
    for file in files {
        if !file.in_use
            && is_temporary(&file.name)
            && now.saturating_sub(file.last_launch) >= STALE_TEMPORARY_SECONDS
        {
            chosen.push(file.name.clone());
        }
    }
    let mut cached: Vec<&CachedFile> = files
        .iter()
        .filter(|file| key_of(&file.name).is_some())
        .collect();
    let mut total = cached
        .iter()
        .fold(incoming, |total, file| total.saturating_add(file.bytes));
    cached.sort_by(|a, b| {
        a.last_launch
            .cmp(&b.last_launch)
            .then_with(|| a.name.cmp(&b.name))
    });
    for file in cached {
        if total <= cap {
            break;
        }
        if file.in_use {
            continue;
        }
        total -= file.bytes;
        chosen.push(file.name.clone());
    }
    chosen
}

/// The names to remove when an app is removed: the cache files of its modules' keys that no
/// running program uses.
pub fn removals_for_app(files: &[CachedFile], keys: &[[u8; 32]]) -> Vec<String> {
    files
        .iter()
        .filter(|file| !file.in_use)
        .filter(|file| key_of(&file.name).is_some_and(|key| keys.contains(&key)))
        .map(|file| file.name.clone())
        .collect()
}
