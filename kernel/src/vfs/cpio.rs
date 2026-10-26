//! The `newc` cpio archive format (D102), as `cpio(5)` describes it: each member is a
//! 110-byte header of ASCII (the magic `070701`, or `070702` with a checksum, then thirteen
//! 8-digit hexadecimal fields), its name with a terminating NUL padded to a multiple of four
//! bytes from the header's start, then its data padded to a multiple of four bytes; a member
//! named `TRAILER!!!` ends the archive.
//!
//! [`Members`] reads an archive in place: every name and every member's data is a slice of the
//! archive, nothing is copied. An archive that does not follow the format is reported as an
//! [`Error`], never read past its end.

/// A member of an archive, its name and data borrowed from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Member<'a> {
    /// The inode number the archive's writer gave it.
    pub inode: u32,
    /// Its file type and permission bits (`st_mode`).
    pub mode: u32,
    /// Its owner's user id.
    pub owner: u32,
    /// Its group id.
    pub group: u32,
    /// How many links it had.
    pub links: u32,
    /// When it was last modified, in seconds since the Unix epoch.
    pub modified: u32,
    /// Its path, without the terminating NUL.
    pub name: &'a [u8],
    /// Its contents.
    pub data: &'a [u8],
    /// Where its data starts in the archive.
    pub data_offset: usize,
}

/// Why an archive cannot be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// A header, name or data runs past the archive's end, or the trailer is missing.
    Truncated,
    /// A header does not start with `070701` or `070702`.
    BadMagic,
    /// A header field is not eight hexadecimal digits.
    BadNumber,
    /// A name is empty or its last byte is not NUL.
    BadName,
}

/// The size of a header.
pub const HEADER_SIZE: usize = 110;

/// The name of the member that ends an archive.
pub const TRAILER: &[u8] = b"TRAILER!!!";

/// The members of an archive, in order, up to its trailer: an iterator that stops after the
/// first error.
#[derive(Debug, Clone)]
pub struct Members<'a> {
    archive: &'a [u8],
    offset: usize,
    finished: bool,
}

impl<'a> Members<'a> {
    /// The members of `archive`.
    pub fn of(archive: &'a [u8]) -> Self {
        Self {
            archive,
            offset: 0,
            finished: false,
        }
    }

    fn member(&self) -> Result<(Member<'a>, usize), Error> {
        let start = self.offset;
        let header = self
            .archive
            .get(start..start.checked_add(HEADER_SIZE).ok_or(Error::Truncated)?)
            .ok_or(Error::Truncated)?;
        if &header[..6] != b"070701" && &header[..6] != b"070702" {
            return Err(Error::BadMagic);
        }
        let field = |index: usize| hexadecimal(&header[6 + 8 * index..14 + 8 * index]);
        let name_size = field(11)? as usize;
        let data_size = field(6)? as usize;
        let name_start = start + HEADER_SIZE;
        let name_end = name_start.checked_add(name_size).ok_or(Error::Truncated)?;
        let name = self
            .archive
            .get(name_start..name_end)
            .ok_or(Error::Truncated)?;
        let Some((&0, name)) = name.split_last() else {
            return Err(Error::BadName);
        };
        if name.is_empty() {
            return Err(Error::BadName);
        }
        let data_start = aligned(name_end).ok_or(Error::Truncated)?;
        let data_end = data_start.checked_add(data_size).ok_or(Error::Truncated)?;
        let data = self
            .archive
            .get(data_start..data_end)
            .ok_or(Error::Truncated)?;
        let member = Member {
            inode: field(0)?,
            mode: field(1)?,
            owner: field(2)?,
            group: field(3)?,
            links: field(4)?,
            modified: field(5)?,
            name,
            data,
            data_offset: data_start,
        };
        Ok((member, aligned(data_end).ok_or(Error::Truncated)?))
    }
}

impl<'a> Iterator for Members<'a> {
    type Item = Result<Member<'a>, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        match self.member() {
            Ok((member, _)) if member.name == TRAILER => {
                self.finished = true;
                None
            }
            Ok((member, next)) => {
                self.offset = next;
                Some(Ok(member))
            }
            Err(error) => {
                self.finished = true;
                Some(Err(error))
            }
        }
    }
}

fn aligned(offset: usize) -> Option<usize> {
    offset.checked_add(3).map(|end| end & !3)
}

fn hexadecimal(digits: &[u8]) -> Result<u32, Error> {
    digits.iter().try_fold(0u32, |value, &digit| {
        let digit = match digit {
            b'0'..=b'9' => digit - b'0',
            b'a'..=b'f' => digit - b'a' + 10,
            b'A'..=b'F' => digit - b'A' + 10,
            _ => return Err(Error::BadNumber),
        };
        Ok(value << 4 | u32::from(digit))
    })
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use alloc::vec::Vec;

    /// A member for [`archive`]: its path, mode, modification time and data.
    pub type Entry<'a> = (&'a str, u32, u32, &'a [u8]);

    /// A `newc` archive of `entries`, with its trailer, as `cargo xtask` writes one.
    pub fn archive(entries: &[Entry<'_>]) -> Vec<u8> {
        let mut bytes = Vec::new();
        let trailer = ("TRAILER!!!", 0, 0, &[][..]);
        for (index, (name, mode, modified, data)) in
            entries.iter().copied().chain([trailer]).enumerate()
        {
            let fields = [
                index as u32 + 1,
                mode,
                0,
                0,
                1,
                modified,
                data.len() as u32,
                0,
                0,
                0,
                0,
                name.len() as u32 + 1,
                0,
            ];
            bytes.extend_from_slice(b"070701");
            for field in fields {
                bytes.extend_from_slice(alloc::format!("{field:08X}").as_bytes());
            }
            bytes.extend_from_slice(name.as_bytes());
            bytes.push(0);
            while bytes.len() % 4 != 0 {
                bytes.push(0);
            }
            bytes.extend_from_slice(data);
            while bytes.len() % 4 != 0 {
                bytes.push(0);
            }
        }
        bytes
    }

    fn members(archive: &[u8]) -> Result<Vec<Member<'_>>, Error> {
        Members::of(archive).collect()
    }

    #[test]
    fn members_are_read_in_place_with_their_fields_up_to_the_trailer() {
        let bytes = archive(&[
            ("etc", 0o040755, 7, b""),
            ("etc/hostname", 0o100644, 1_700_000_000, b"eggos\n"),
            ("a", 0o100600, 0, b"xyz"),
        ]);
        let members = members(&bytes).unwrap();
        assert_eq!(members.len(), 3);
        assert_eq!(members[0].name, b"etc");
        assert_eq!(members[0].mode, 0o040755);
        assert_eq!(members[0].modified, 7);
        assert_eq!(members[1].name, b"etc/hostname");
        assert_eq!(members[1].data, b"eggos\n");
        assert_eq!(members[1].modified, 1_700_000_000);
        assert_eq!(members[1].inode, 2);
        assert_eq!(
            &bytes[members[1].data_offset..][..6],
            b"eggos\n",
            "the data is the archive's own bytes"
        );
        assert_eq!(members[2].data, b"xyz");
        assert_eq!(members[2].mode, 0o100600);
    }

    #[test]
    fn an_archive_with_only_a_trailer_has_no_members_and_checksummed_headers_are_read() {
        assert_eq!(members(&archive(&[])), Ok(Vec::new()));
        let mut bytes = archive(&[("x", 0o100644, 0, b"1")]);
        bytes[5] = b'2';
        assert_eq!(members(&bytes).unwrap()[0].data, b"1");
    }

    #[test]
    fn malformed_archives_are_rejected_without_reading_past_their_end() {
        let good = archive(&[("etc/hostname", 0o100644, 0, b"eggos\n")]);
        for end in 0..good.len() - 1 {
            let result = members(&good[..end]);
            assert!(result.is_err(), "a cut at {end} was accepted");
        }
        let mut bad_magic = good.clone();
        bad_magic[0] = b'1';
        assert_eq!(members(&bad_magic), Err(Error::BadMagic));
        let mut bad_number = good.clone();
        bad_number[6] = b'g';
        assert_eq!(members(&bad_number), Err(Error::BadNumber));
        let mut unterminated = good.clone();
        unterminated[HEADER_SIZE + 12] = b'x';
        assert_eq!(members(&unterminated), Err(Error::BadName));
        let mut empty_name = good.clone();
        empty_name[6 + 8 * 11..6 + 8 * 12].copy_from_slice(b"00000000");
        assert_eq!(members(&empty_name), Err(Error::BadName));
        let mut huge_data = good.clone();
        huge_data[6 + 8 * 6..6 + 8 * 7].copy_from_slice(b"FFFFFFFF");
        assert_eq!(members(&huge_data), Err(Error::Truncated));
        let mut huge_name = good.clone();
        huge_name[6 + 8 * 11..6 + 8 * 12].copy_from_slice(b"FFFFFFFF");
        assert_eq!(members(&huge_name), Err(Error::Truncated));
        let mut no_trailer = good;
        let trailer_start = no_trailer.len() - (HEADER_SIZE + 12);
        no_trailer.truncate(trailer_start);
        assert_eq!(members(&no_trailer), Err(Error::Truncated));
    }
}
