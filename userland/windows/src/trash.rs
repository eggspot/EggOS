//! The Trash as the Recycle Bin (D135, D178): what a Windows program deletes with
//! `FOF_ALLOWUNDO` goes where the Files app puts it, laid out as the freedesktop.org Trash
//! specification says, `<home>/.local/share/Trash/files/<name>` with the record
//! `info/<name>.trashinfo` of where it came from and when.

use crate::calendar::SystemTime;

/// Where the Trash is under a home folder.
pub const UNDER_HOME: &[u8] = b".local/share/Trash";
/// The extension of a record.
pub const INFO_EXTENSION: &[u8] = b".trashinfo";
const UNESCAPED: &[u8] = b"-_.~/";

/// Writes the record of `original` (an absolute path) trashed at `when` through `out`.
pub fn write_record(original: &[u8], when: &SystemTime, out: &mut impl FnMut(&[u8])) {
    out(b"[Trash Info]\nPath=");
    for byte in original {
        if byte.is_ascii_alphanumeric() || UNESCAPED.contains(byte) {
            out(core::slice::from_ref(byte));
        } else {
            let hex = b"0123456789ABCDEF";
            out(&[
                b'%',
                hex[usize::from(byte >> 4)],
                hex[usize::from(byte & 0xf)],
            ]);
        }
    }
    out(b"\nDeletionDate=");
    let mut date = [0u8; 19];
    let fields = [
        (0, 4, when.year),
        (5, 2, when.month),
        (8, 2, when.day),
        (11, 2, when.hour),
        (14, 2, when.minute),
        (17, 2, when.second),
    ];
    for (at, width, value) in fields {
        let mut value = value;
        for index in (at..at + width).rev() {
            date[index] = b'0' + (value % 10) as u8;
            value /= 10;
        }
    }
    for (at, separator) in [(4, b'-'), (7, b'-'), (10, b'T'), (13, b':'), (16, b':')] {
        date[at] = separator;
    }
    out(&date);
    out(b"\n");
}

/// Writes the name an item named `name` takes in the Trash on try `attempt` (from 1) into
/// `out`: `name` itself, then `name.2`, `name.3`, ...; its length, or `None` if it does not
/// fit.
pub fn trashed_name(name: &[u8], attempt: u32, out: &mut [u8]) -> Option<usize> {
    let mut suffix = [0u8; 11];
    let mut suffix_length = 0;
    if attempt > 1 {
        let mut digits = [0u8; 10];
        let mut count = 0;
        let mut value = attempt;
        while value > 0 {
            digits[count] = b'0' + (value % 10) as u8;
            value /= 10;
            count += 1;
        }
        suffix[0] = b'.';
        for (index, digit) in digits[..count].iter().rev().enumerate() {
            suffix[1 + index] = *digit;
        }
        suffix_length = count + 1;
    }
    let length = name.len() + suffix_length;
    let out = out.get_mut(..length)?;
    out[..name.len()].copy_from_slice(name);
    out[name.len()..].copy_from_slice(&suffix[..suffix_length]);
    Some(length)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_escape_the_path_and_date_the_deletion() {
        let when = SystemTime {
            year: 2026,
            month: 3,
            day: 7,
            hour: 9,
            minute: 5,
            second: 1,
            ..SystemTime::default()
        };
        let mut record = Vec::new();
        write_record(b"/home/root/My notes%.txt", &when, &mut |piece| {
            record.extend_from_slice(piece)
        });
        assert_eq!(
            String::from_utf8(record).unwrap(),
            "[Trash Info]\nPath=/home/root/My%20notes%25.txt\nDeletionDate=2026-03-07T09:05:01\n"
        );
    }

    #[test]
    fn taken_names_get_a_number() {
        let mut out = [0u8; 16];
        let length = trashed_name(b"a.txt", 1, &mut out).unwrap();
        assert_eq!(&out[..length], b"a.txt");
        let length = trashed_name(b"a.txt", 12, &mut out).unwrap();
        assert_eq!(&out[..length], b"a.txt.12");
        assert_eq!(trashed_name(b"a-long-name.txt", 2, &mut out), None);
    }
}
