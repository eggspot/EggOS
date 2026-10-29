//! Reading a file or bytes in memory at any offset (D148): what the archive and icon readers
//! take, so the same code reads an installed program, a `.zip`, a bundle's `.icns` and a test's
//! bytes, and the bytes of a range one at a time for the inflater.

use crate::files::{Files, Handle, Kind};
use eggos_png::inflate::ByteSource;

const CHUNK_BYTES: usize = 4096;

/// Bytes read at any offset.
pub trait Reader {
    /// How many bytes there are.
    fn size(&self) -> u64;

    /// Reads into `buffer` from `offset`: how many bytes, fewer only at the end; 0 at or past
    /// the end or on an error.
    fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> usize;

    /// Fills `buffer` from `offset`; whether it was filled.
    fn read_exact_at(&mut self, offset: u64, buffer: &mut [u8]) -> bool {
        let mut done = 0;
        while done < buffer.len() {
            let read = self.read_at(offset + done as u64, &mut buffer[done..]);
            if read == 0 {
                return false;
            }
            done += read;
        }
        true
    }
}

/// Bytes in memory.
#[derive(Debug, Clone, Copy)]
pub struct Bytes<'a>(pub &'a [u8]);

impl Reader for Bytes<'_> {
    fn size(&self) -> u64 {
        self.0.len() as u64
    }

    fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> usize {
        let rest = usize::try_from(offset)
            .ok()
            .and_then(|start| self.0.get(start..))
            .unwrap_or_default();
        let count = rest.len().min(buffer.len());
        buffer[..count].copy_from_slice(&rest[..count]);
        count
    }
}

/// A file open in a [`Files`]; closing it stays with whoever opened it.
pub struct FileReader<'f, F: Files> {
    files: &'f mut F,
    file: Handle,
    size: u64,
}

impl<F: Files> core::fmt::Debug for FileReader<'_, F> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("FileReader")
            .field("file", &self.file)
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

impl<'f, F: Files> FileReader<'f, F> {
    /// A reader of `file`, open in `files`, `size` bytes long.
    pub fn new(files: &'f mut F, file: Handle, size: u64) -> Self {
        Self { files, file, size }
    }
}

impl<F: Files> Reader for FileReader<'_, F> {
    fn size(&self) -> u64 {
        self.size
    }

    fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> usize {
        if offset >= self.size {
            return 0;
        }
        let room = (self.size - offset).min(buffer.len() as u64) as usize;
        self.files
            .read_at(self.file, offset, &mut buffer[..room])
            .unwrap_or(0)
    }
}

/// Opens the regular file at `path`, calls `body` with a reader of it and closes it; `None`
/// when it is no regular file or cannot be opened.
pub fn read_file<F: Files, T>(
    files: &mut F,
    path: &[u8],
    body: impl FnOnce(&mut FileReader<'_, F>) -> T,
) -> Option<T> {
    let status = files
        .status(path)
        .ok()
        .filter(|status| status.kind == Kind::File)?;
    let file = files.open(path).ok()?;
    let result = body(&mut FileReader::new(files, file, status.size));
    files.close(file);
    Some(result)
}

/// The bytes `start..end` of a reader, one at a time, read 4 KiB at once.
pub struct RangeBytes<'r, R: Reader> {
    reader: &'r mut R,
    next: u64,
    end: u64,
    buffer: [u8; CHUNK_BYTES],
    at: usize,
    filled: usize,
}

impl<R: Reader> core::fmt::Debug for RangeBytes<'_, R> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("RangeBytes")
            .field("next", &self.next)
            .field("end", &self.end)
            .finish_non_exhaustive()
    }
}

impl<'r, R: Reader> RangeBytes<'r, R> {
    /// The bytes of `reader` from `start` up to `end`.
    pub fn new(reader: &'r mut R, start: u64, end: u64) -> Self {
        Self {
            reader,
            next: start,
            end,
            buffer: [0; CHUNK_BYTES],
            at: 0,
            filled: 0,
        }
    }
}

impl<R: Reader> ByteSource for RangeBytes<'_, R> {
    fn next_byte(&mut self) -> Option<u8> {
        if self.at == self.filled {
            if self.next >= self.end {
                return None;
            }
            let want = (self.end - self.next).min(CHUNK_BYTES as u64) as usize;
            let read = self.reader.read_at(self.next, &mut self.buffer[..want]);
            if read == 0 {
                return None;
            }
            self.next += read as u64;
            self.at = 0;
            self.filled = read;
        }
        let byte = self.buffer[self.at];
        self.at += 1;
        Some(byte)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeFiles;

    #[test]
    fn bytes_and_files_read_at_any_offset_and_ranges_byte_by_byte() {
        let data: Vec<u8> = (0..10_000u32).map(|index| (index * 7) as u8).collect();
        let mut bytes = Bytes(&data);
        let mut buffer = [0u8; 8];
        assert_eq!(bytes.read_at(9_996, &mut buffer), 4);
        assert_eq!(buffer[..4], data[9_996..]);
        assert_eq!(bytes.read_at(10_000, &mut buffer), 0);
        assert_eq!(bytes.read_at(u64::MAX, &mut buffer), 0);
        assert!(!bytes.read_exact_at(9_999, &mut buffer));
        let mut files = FakeFiles::new();
        files.file("/media/host/data", &data);
        let read = read_file(&mut files, b"/media/host/data", |reader| {
            assert_eq!(reader.size(), 10_000);
            let mut middle = [0u8; 5];
            assert!(reader.read_exact_at(5_000, &mut middle));
            assert_eq!(middle, data[5_000..5_005]);
            let mut range = RangeBytes::new(reader, 4_000, 9_000);
            let mut taken = Vec::new();
            while let Some(byte) = range.next_byte() {
                taken.push(byte);
            }
            taken
        });
        assert_eq!(read.unwrap(), data[4_000..9_000]);
        assert_eq!(files.open_count(), 0);
        assert!(read_file(&mut files, b"/media/host", |_| ()).is_none());
        assert!(read_file(&mut files, b"/missing", |_| ()).is_none());
    }
}
