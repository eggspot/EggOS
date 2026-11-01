//! An app's ahead-of-time translation record (D191, slice 39e2d): what a layer prints in its
//! translate-only mode (`<layer> --translate <program>`), and what the installer keeps as
//! [`FILE_NAME`] in the app's folder, so `app show` lists the refused sites and uninstalling
//! removes the cache file by its exact name. The record is text: the line
//! `eggos-translation 1`, then `key=` (64 lowercase hex digits), `code=` (the bytes of x86_64
//! code translated), `cache=` (the cache file's bytes), `stored=` (`yes` when the cache file is
//! on the disk, `no`), `refused=` (how many instructions were refused) and at most
//! [`MAX_SITES`] lines `site=<guest address in hex> <reason>`, each ending with a newline, in
//! that order. Reading is strict: anything else is no record. Nothing here allocates or
//! panics.

/// The record's file name inside an app's folder.
pub const FILE_NAME: &str = "app.translation";

/// The record's first line: its format and version.
pub const HEADER: &[u8] = b"eggos-translation 1";

/// The option that starts a layer in its translate-only mode, before the program's path.
pub const LAYER_OPTION: &str = "--translate";

/// The most refused sites a record lists; `refused=` counts them all.
pub const MAX_SITES: usize = 8;

/// The longest reason, in bytes.
pub const REASON_CAPACITY: usize = 96;

/// The longest record, in bytes.
pub const LARGEST: usize = 2048;

/// The translation cache's directory (`eggos_translate::cache::policy::DIRECTORY`).
pub const CACHE_DIRECTORY: &str = "/var/cache/eggos/translate";

/// The cache files' extension (`eggos_translate::cache::policy::EXTENSION`).
pub const CACHE_EXTENSION: &str = ".tx";

const KEY_BYTES: usize = 32;
const KEY_DIGITS: usize = 2 * KEY_BYTES;
const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

/// The length of a cache file's path.
pub const CACHE_PATH_LENGTH: usize = CACHE_DIRECTORY.len() + 1 + KEY_DIGITS + CACHE_EXTENSION.len();

/// One refused instruction: where it is and why.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RefusedSite<'a> {
    /// Its guest address.
    pub address: u64,
    /// Why it was refused, in words ([`is_reason`]).
    pub reason: &'a [u8],
}

/// What translating one program made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record<'a> {
    /// The cache key.
    pub key: [u8; KEY_BYTES],
    /// The bytes of x86_64 code translated.
    pub code_bytes: u64,
    /// The cache file's bytes.
    pub cache_bytes: u64,
    /// Whether the cache file is on the disk.
    pub stored: bool,
    /// How many instructions were refused.
    pub refused: u64,
    sites: [RefusedSite<'a>; MAX_SITES],
    site_count: usize,
}

const NO_SITE: RefusedSite<'static> = RefusedSite {
    address: 0,
    reason: b"",
};

impl<'a> Record<'a> {
    /// A record with no sites listed yet.
    pub const fn new(
        key: [u8; KEY_BYTES],
        code_bytes: u64,
        cache_bytes: u64,
        stored: bool,
        refused: u64,
    ) -> Self {
        Self {
            key,
            code_bytes,
            cache_bytes,
            stored,
            refused,
            sites: [NO_SITE; MAX_SITES],
            site_count: 0,
        }
    }

    /// Lists `site`; false when [`MAX_SITES`] or `refused` sites are listed already, or its
    /// reason is no [`is_reason`].
    pub fn push_site(&mut self, site: RefusedSite<'a>) -> bool {
        if self.site_count == MAX_SITES
            || self.site_count as u64 >= self.refused
            || !is_reason(site.reason)
        {
            return false;
        }
        self.sites[self.site_count] = site;
        self.site_count += 1;
        true
    }

    /// The listed sites, the first by address.
    pub fn sites(&self) -> &[RefusedSite<'a>] {
        &self.sites[..self.site_count]
    }

    /// The path of the cache file.
    pub fn cache_path(&self) -> [u8; CACHE_PATH_LENGTH] {
        cache_path(&self.key)
    }
}

/// Whether `reason` may be a site's reason: 1 to [`REASON_CAPACITY`] printable ASCII bytes.
pub fn is_reason(reason: &[u8]) -> bool {
    !reason.is_empty()
        && reason.len() <= REASON_CAPACITY
        && reason.iter().all(|&byte| (b' '..=b'~').contains(&byte))
}

/// The path of the cache file of `key`: the directory, 64 lowercase hex digits and
/// [`CACHE_EXTENSION`].
pub fn cache_path(key: &[u8; KEY_BYTES]) -> [u8; CACHE_PATH_LENGTH] {
    let mut path = [0u8; CACHE_PATH_LENGTH];
    let directory = CACHE_DIRECTORY.as_bytes();
    path[..directory.len()].copy_from_slice(directory);
    path[directory.len()] = b'/';
    let digits = directory.len() + 1;
    for (index, &byte) in key.iter().enumerate() {
        path[digits + 2 * index] = HEX_DIGITS[usize::from(byte >> 4)];
        path[digits + 2 * index + 1] = HEX_DIGITS[usize::from(byte & 15)];
    }
    path[digits + KEY_DIGITS..].copy_from_slice(CACHE_EXTENSION.as_bytes());
    path
}

fn hex_value(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        _ => None,
    }
}

fn parse_key(text: &[u8]) -> Option<[u8; KEY_BYTES]> {
    if text.len() != KEY_DIGITS {
        return None;
    }
    let mut key = [0u8; KEY_BYTES];
    for (index, byte) in key.iter_mut().enumerate() {
        *byte = (hex_value(text[2 * index])? << 4) | hex_value(text[2 * index + 1])?;
    }
    Some(key)
}

fn parse_number(text: &[u8], radix: u64) -> Option<u64> {
    if text.is_empty() || text.len() > 20 {
        return None;
    }
    text.iter().try_fold(0u64, |value, &digit| {
        let digit = u64::from(match radix {
            16 => hex_value(digit)?,
            _ if digit.is_ascii_digit() => digit - b'0',
            _ => return None,
        });
        value.checked_mul(radix)?.checked_add(digit)
    })
}

fn value<'a>(line: Option<&'a [u8]>, key: &[u8]) -> Option<&'a [u8]> {
    let line = line?;
    let rest = line.strip_prefix(key)?;
    rest.strip_prefix(b"=")
}

/// The record in `file`, or `None` when it is no record ([`LARGEST`] bytes at most).
pub fn parse(file: &[u8]) -> Option<Record<'_>> {
    if file.len() > LARGEST {
        return None;
    }
    let body = file.strip_suffix(b"\n")?;
    let mut lines = body.split(|&byte| byte == b'\n');
    if lines.next()? != HEADER {
        return None;
    }
    let key = parse_key(value(lines.next(), b"key")?)?;
    let code_bytes = parse_number(value(lines.next(), b"code")?, 10)?;
    let cache_bytes = parse_number(value(lines.next(), b"cache")?, 10)?;
    let stored = match value(lines.next(), b"stored")? {
        b"yes" => true,
        b"no" => false,
        _ => return None,
    };
    let refused = parse_number(value(lines.next(), b"refused")?, 10)?;
    let mut record = Record::new(key, code_bytes, cache_bytes, stored, refused);
    for line in lines {
        let site = value(Some(line), b"site")?;
        let space = site.iter().position(|&byte| byte == b' ')?;
        let address = parse_number(&site[..space], 16)?;
        let reason = &site[space + 1..];
        if !record.push_site(RefusedSite { address, reason }) {
            return None;
        }
    }
    Some(record)
}

struct Writer<'a> {
    out: &'a mut [u8],
    length: usize,
}

impl Writer<'_> {
    fn put(&mut self, bytes: &[u8]) -> Option<()> {
        let end = self.length.checked_add(bytes.len())?;
        self.out.get_mut(self.length..end)?.copy_from_slice(bytes);
        self.length = end;
        Some(())
    }

    fn number(&mut self, mut value: u64, radix: u64) -> Option<()> {
        let mut digits = [0u8; 20];
        let mut first = digits.len();
        loop {
            first -= 1;
            digits[first] = HEX_DIGITS[(value % radix) as usize];
            value /= radix;
            if value == 0 {
                break;
            }
        }
        self.put(&digits[first..])
    }
}

/// Writes `record` into `out`; its length, or `None` when `out` is too small.
pub fn write(record: &Record<'_>, out: &mut [u8]) -> Option<usize> {
    let mut writer = Writer { out, length: 0 };
    writer.put(HEADER)?;
    writer.put(b"\nkey=")?;
    let path = record.cache_path();
    let digits = CACHE_DIRECTORY.len() + 1;
    writer.put(&path[digits..digits + KEY_DIGITS])?;
    writer.put(b"\ncode=")?;
    writer.number(record.code_bytes, 10)?;
    writer.put(b"\ncache=")?;
    writer.number(record.cache_bytes, 10)?;
    writer.put(if record.stored {
        b"\nstored=yes"
    } else {
        b"\nstored=no"
    })?;
    writer.put(b"\nrefused=")?;
    writer.number(record.refused, 10)?;
    writer.put(b"\n")?;
    for site in record.sites() {
        writer.put(b"site=")?;
        writer.number(site.address, 16)?;
        writer.put(b" ")?;
        writer.put(site.reason)?;
        writer.put(b"\n")?;
    }
    Some(writer.length)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; 32] = [0xab; 32];

    fn sample() -> Record<'static> {
        let mut record = Record::new(KEY, 4096, 20480, true, 3);
        assert!(record.push_site(RefusedSite {
            address: 0x1_4000_1010,
            reason: b"an AVX (VEX-encoded) instruction",
        }));
        assert!(record.push_site(RefusedSite {
            address: 0x1_4000_2000,
            reason: b"an SSE3 instruction",
        }));
        record
    }

    #[test]
    fn a_record_is_written_as_its_lines_and_read_back() {
        let mut out = [0u8; LARGEST];
        let length = write(&sample(), &mut out).unwrap();
        let text = core::str::from_utf8(&out[..length]).unwrap();
        let key = "ab".repeat(32);
        assert_eq!(
            text,
            format!(
                "eggos-translation 1\nkey={key}\ncode=4096\ncache=20480\nstored=yes\nrefused=3\n\
                 site=140001010 an AVX (VEX-encoded) instruction\n\
                 site=140002000 an SSE3 instruction\n"
            )
        );
        assert_eq!(parse(&out[..length]), Some(sample()));
    }

    #[test]
    fn a_record_without_sites_and_not_stored_reads_back() {
        let record = Record::new([0; 32], 10, 8192, false, 0);
        let mut out = [0u8; LARGEST];
        let length = write(&record, &mut out).unwrap();
        let parsed = parse(&out[..length]).unwrap();
        assert!(!parsed.stored);
        assert!(parsed.sites().is_empty());
    }

    #[test]
    fn anything_else_is_no_record() {
        let mut out = [0u8; LARGEST];
        let length = write(&sample(), &mut out).unwrap();
        let good = &out[..length];
        assert_eq!(parse(&good[..length - 1]), None);
        assert_eq!(parse(b"eggos-translation 2\n"), None);
        let mut swapped = good.to_vec();
        swapped[22] = b'G';
        assert_eq!(parse(&swapped), None);
        let mut more = good.to_vec();
        more.extend_from_slice(b"site=1 a third\nsite=2 a fourth\n");
        assert_eq!(parse(&more), None);
        let mut stray = good.to_vec();
        stray.extend_from_slice(b"extra=1\n");
        assert_eq!(parse(&stray), None);
    }

    #[test]
    fn sites_stop_at_the_count_and_the_cap_and_need_a_printable_reason() {
        let mut record = Record::new(KEY, 1, 1, true, 100);
        assert!(!record.push_site(RefusedSite {
            address: 1,
            reason: b"two\nlines",
        }));
        for address in 0..MAX_SITES as u64 {
            assert!(record.push_site(RefusedSite {
                address,
                reason: b"why",
            }));
        }
        assert!(!record.push_site(RefusedSite {
            address: 99,
            reason: b"why",
        }));
        assert_eq!(record.sites().len(), MAX_SITES);
    }

    #[test]
    fn the_cache_path_is_the_directory_the_hex_key_and_the_extension() {
        let path = cache_path(&KEY);
        let expected = format!("/var/cache/eggos/translate/{}.tx", "ab".repeat(32));
        assert_eq!(&path[..], expected.as_bytes());
    }
}
