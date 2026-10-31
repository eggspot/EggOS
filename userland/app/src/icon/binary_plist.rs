//! Binary property lists (`bplist00`, D196): the string a top-level dictionary holds under a
//! key, which is all the installer needs from a bundle's `Info.plist` (`CFBundleIconFile`,
//! `CFBundleExecutable`). Read from the public format: an 8-byte header, objects, an offset
//! table and a 32-byte trailer giving the table's place, its integer width, the object
//! reference width, the object count and the top object. ASCII and UTF-16 strings are read;
//! every offset and count is checked against the file, so a broken list gives `None`.

/// The eight bytes a binary property list starts with.
pub const MAGIC: &[u8; 8] = b"bplist00";

const TRAILER_BYTES: usize = 32;
const ASCII_STRING: u8 = 0x5;
const UTF16_STRING: u8 = 0x6;
const DICTIONARY: u8 = 0xd;
const INTEGER: u8 = 0x1;
const LONG_COUNT: u8 = 0xf;

struct Plist<'p> {
    bytes: &'p [u8],
    offset_width: usize,
    reference_width: usize,
    objects: u64,
    table: usize,
}

fn big_endian(bytes: &[u8]) -> Option<u64> {
    (bytes.len() <= 8).then(|| {
        bytes
            .iter()
            .fold(0u64, |value, &byte| value << 8 | u64::from(byte))
    })
}

impl<'p> Plist<'p> {
    fn open(bytes: &'p [u8]) -> Option<(Self, u64)> {
        if !bytes.starts_with(MAGIC) || bytes.len() < MAGIC.len() + TRAILER_BYTES {
            return None;
        }
        let trailer = bytes.get(bytes.len() - TRAILER_BYTES..)?;
        let offset_width = usize::from(*trailer.get(6)?);
        let reference_width = usize::from(*trailer.get(7)?);
        let objects = big_endian(trailer.get(8..16)?)?;
        let top = big_endian(trailer.get(16..24)?)?;
        let table = usize::try_from(big_endian(trailer.get(24..32)?)?).ok()?;
        let valid =
            (1..=8).contains(&offset_width) && (1..=8).contains(&reference_width) && top < objects;
        valid.then_some((
            Self {
                bytes,
                offset_width,
                reference_width,
                objects,
                table,
            },
            top,
        ))
    }

    fn offset_of(&self, object: u64) -> Option<usize> {
        if object >= self.objects {
            return None;
        }
        let start = self.table.checked_add(
            usize::try_from(object)
                .ok()?
                .checked_mul(self.offset_width)?,
        )?;
        let entry = self
            .bytes
            .get(start..start.checked_add(self.offset_width)?)?;
        usize::try_from(big_endian(entry)?).ok()
    }

    fn count_at(&self, at: usize) -> Option<(usize, usize)> {
        let marker = *self.bytes.get(at)?;
        let low = marker & 0xf;
        if low != LONG_COUNT {
            return Some((usize::from(low), at + 1));
        }
        let integer = *self.bytes.get(at + 1)?;
        if integer >> 4 != INTEGER {
            return None;
        }
        let width = 1usize << (integer & 0xf).min(3);
        let start = at + 2;
        let count = big_endian(self.bytes.get(start..start + width)?)?;
        Some((usize::try_from(count).ok()?, start + width))
    }

    fn reference(&self, at: usize) -> Option<u64> {
        big_endian(self.bytes.get(at..at.checked_add(self.reference_width)?)?)
    }

    fn string<'o>(&self, object: u64, out: &'o mut [u8]) -> Option<&'o [u8]> {
        let at = self.offset_of(object)?;
        let kind = *self.bytes.get(at)? >> 4;
        let (count, start) = self.count_at(at)?;
        match kind {
            ASCII_STRING => {
                let text = self.bytes.get(start..start.checked_add(count)?)?;
                let place = out.get_mut(..count)?;
                place.copy_from_slice(text);
                Some(place)
            }
            UTF16_STRING => {
                let units = self
                    .bytes
                    .get(start..start.checked_add(count.checked_mul(2)?)?)?;
                let mut length = 0;
                for unit in char::decode_utf16(
                    units
                        .chunks_exact(2)
                        .map(|pair| u16::from_be_bytes([pair[0], pair[1]])),
                ) {
                    let mut buffer = [0u8; 4];
                    let encoded = unit.ok()?.encode_utf8(&mut buffer).as_bytes();
                    out.get_mut(length..length + encoded.len())?
                        .copy_from_slice(encoded);
                    length += encoded.len();
                }
                Some(&out[..length])
            }
            _ => None,
        }
    }
}

/// The string the top-level dictionary of the binary property list `bytes` holds under `key`,
/// written into `out` as UTF-8; `None` when the list is not one, has no such string, or it does
/// not fit `out`.
pub fn dictionary_string<'o>(bytes: &[u8], key: &[u8], out: &'o mut [u8]) -> Option<&'o [u8]> {
    let (plist, top) = Plist::open(bytes)?;
    let at = plist.offset_of(top)?;
    if *plist.bytes.get(at)? >> 4 != DICTIONARY {
        return None;
    }
    let (count, keys) = plist.count_at(at)?;
    let values = keys.checked_add(count.checked_mul(plist.reference_width)?)?;
    let mut scratch = [0u8; 64];
    for index in 0..count {
        let key_object = plist.reference(keys + index * plist.reference_width)?;
        if plist.string(key_object, &mut scratch) != Some(key) {
            continue;
        }
        let value = plist.reference(values + index * plist.reference_width)?;
        return plist.string(value, out);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ascii(text: &str) -> Vec<u8> {
        let mut object = vec![0x50 | text.len() as u8];
        object.extend_from_slice(text.as_bytes());
        object
    }

    fn utf16(text: &str) -> Vec<u8> {
        let units: Vec<u16> = text.encode_utf16().collect();
        let mut object = vec![0x6f, 0x10, units.len() as u8];
        for unit in units {
            object.extend_from_slice(&unit.to_be_bytes());
        }
        object
    }

    fn plist(pairs: &[(Vec<u8>, Vec<u8>)]) -> Vec<u8> {
        let count = pairs.len();
        let mut objects = vec![{
            let mut dictionary = vec![0xd0 | count as u8];
            dictionary.extend((0..count).map(|index| (1 + 2 * index) as u8));
            dictionary.extend((0..count).map(|index| (2 + 2 * index) as u8));
            dictionary
        }];
        for (key, value) in pairs {
            objects.push(key.clone());
            objects.push(value.clone());
        }
        let mut bytes = MAGIC.to_vec();
        let mut offsets = Vec::new();
        for object in &objects {
            offsets.push(bytes.len() as u8);
            bytes.extend_from_slice(object);
        }
        let table = bytes.len();
        bytes.extend_from_slice(&offsets);
        let mut trailer = [0u8; 32];
        trailer[6] = 1;
        trailer[7] = 1;
        trailer[8..16].copy_from_slice(&(objects.len() as u64).to_be_bytes());
        trailer[24..32].copy_from_slice(&(table as u64).to_be_bytes());
        bytes.extend_from_slice(&trailer);
        bytes
    }

    #[test]
    fn a_top_level_string_is_found_by_its_key() {
        let bytes = plist(&[
            (ascii("CFBundleExecutable"), ascii("Paint")),
            (ascii("CFBundleIconFile"), utf16("Ic\u{f4}ne")),
        ]);
        let mut out = [0u8; 64];
        assert_eq!(
            dictionary_string(&bytes, b"CFBundleExecutable", &mut out),
            Some(&b"Paint"[..])
        );
        assert_eq!(
            dictionary_string(&bytes, b"CFBundleIconFile", &mut out),
            Some("Ic\u{f4}ne".as_bytes())
        );
        assert_eq!(dictionary_string(&bytes, b"CFBundleName", &mut out), None);
    }

    #[test]
    fn a_broken_list_is_none_not_a_panic() {
        let bytes = plist(&[(ascii("CFBundleIconFile"), ascii("AppIcon"))]);
        let mut out = [0u8; 64];
        for cut in 0..bytes.len() {
            let _ = dictionary_string(&bytes[..cut], b"CFBundleIconFile", &mut out);
        }
        let mut wrong = bytes.clone();
        let table_at = wrong.len() - 8;
        wrong[table_at..].copy_from_slice(&u64::MAX.to_be_bytes());
        assert_eq!(
            dictionary_string(&wrong, b"CFBundleIconFile", &mut out),
            None
        );
        assert_eq!(
            dictionary_string(b"<?xml", b"CFBundleIconFile", &mut out),
            None
        );
        let mut short = [0u8; 3];
        assert_eq!(
            dictionary_string(&bytes, b"CFBundleIconFile", &mut short),
            None
        );
    }
}
