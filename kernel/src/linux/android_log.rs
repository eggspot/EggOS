//! Android's log (D188): the kernel is Android's `logd`. Android programs send their log
//! entries as datagrams to `/dev/socket/logdw` ([`eggos_android_log::WRITER_PATH`]), which the
//! kernel's `AF_UNIX` datagram sockets hand to [`write()`]; the entries are kept as records
//! ([`eggos_android_log`]) in one log of at most [`CAPACITY`] bytes, the oldest dropped first,
//! which `/dev/android_log` shows as a read-only file of root's and `logcat` copies whole with
//! [`SNAPSHOT_REQUEST`].
//!
//! The log takes its memory when the first entry comes and gives it back when it is cleared
//! ([`CLEAR_REQUEST`], a privileged process only): a system where no Android program logs
//! never pays for it. The lock is a leaf lock, held only while records are copied in or out.

use super::Errno;
use crate::process::Process;
use crate::sync::LeafSpinLock;
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use eggos_android_log as format;
use eggos_hal::Platform;

pub use eggos_android_log::{CAPACITY, CLEAR_REQUEST, SNAPSHOT_REQUEST};

static LOG: LeafSpinLock<Log> = LeafSpinLock::new_leaf(Log::new());

#[derive(Debug)]
struct Log {
    records: VecDeque<u8>,
}

impl Log {
    const fn new() -> Self {
        Self {
            records: VecDeque::new(),
        }
    }

    fn oldest_size(&self) -> Option<usize> {
        let low = *self.records.front()?;
        let high = *self.records.get(1)?;
        Some(usize::from(u16::from_le_bytes([low, high])))
    }

    fn append(&mut self, record: &[u8]) -> bool {
        if record.len() > CAPACITY {
            return false;
        }
        if self.records.capacity() < CAPACITY
            && self
                .records
                .try_reserve_exact(CAPACITY - self.records.len())
                .is_err()
        {
            return false;
        }
        while self.records.len() + record.len() > CAPACITY {
            match self.oldest_size() {
                Some(size) if size >= format::RECORD_HEADER_SIZE => {
                    self.records.drain(..size.min(self.records.len()));
                }
                _ => self.records.clear(),
            }
        }
        self.records.extend(record);
        true
    }

    fn copy_from(&self, offset: usize, out: &mut [u8]) -> usize {
        let (front, back) = self.records.as_slices();
        let mut copied = 0;
        let mut skip = offset;
        for part in [front, back] {
            let start = skip.min(part.len());
            skip -= start;
            let rest = &part[start..];
            let count = rest.len().min(out.len() - copied);
            out[copied..copied + count].copy_from_slice(&rest[..count]);
            copied += count;
        }
        copied
    }

    fn snapshot(&self) -> Result<Vec<u8>, Errno> {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(self.records.len())
            .map_err(|_| Errno::ENOMEM)?;
        let (front, back) = self.records.as_slices();
        bytes.extend_from_slice(front);
        bytes.extend_from_slice(back);
        Ok(bytes)
    }
}

/// Keeps the entry `datagram` carries, sent by process `pid`: `false` for a datagram that
/// carries none (too short, a binary or unknown buffer) and without memory for the log.
pub fn write(datagram: &[u8], pid: u32) -> bool {
    let Some(entry) = format::parse_datagram(datagram, pid) else {
        return false;
    };
    let size = format::record_size(&entry);
    let mut record = Vec::new();
    if record.try_reserve_exact(size).is_err() {
        return false;
    }
    record.resize(size, 0);
    if format::encode_record(&entry, &mut record).is_none() {
        return false;
    }
    LOG.lock().append(&record)
}

/// The log's size in bytes, as `stat` reports it.
pub fn size() -> u64 {
    LOG.lock().records.len() as u64
}

/// Copies the log's bytes from `offset` into `buffer`: how many, 0 at its end. Records may be
/// dropped between two reads; [`SNAPSHOT_REQUEST`] copies the whole log at once.
pub fn read_into(offset: u64, buffer: &mut [u8]) -> usize {
    let Ok(offset) = usize::try_from(offset) else {
        return 0;
    };
    LOG.lock().copy_from(offset, buffer)
}

/// Empties the log and gives its memory back.
pub fn clear() {
    let records = core::mem::take(&mut LOG.lock().records);
    drop(records);
}

/// `ioctl(fd, request, argument)` on `/dev/android_log`: [`SNAPSHOT_REQUEST`] writes the log's
/// length as 4 little-endian bytes at `argument`, then its records, into the program's
/// [`format::SNAPSHOT_SIZE`]-byte buffer (`EFAULT` if it is not the program's memory,
/// `ENOMEM` without memory for the copy); [`CLEAR_REQUEST`] empties it, for a privileged
/// process only (`EPERM`). Any other request is `ENOTTY`.
pub fn ioctl<P: Platform>(process: &Process<P>, request: u64, argument: u64) -> Result<u64, Errno> {
    match request as u32 {
        SNAPSHOT_REQUEST => {
            let records = LOG.lock().snapshot()?;
            let length = records.len() as u32;
            process
                .write_user_memory(argument, &length.to_le_bytes())
                .and_then(|()| process.write_user_memory(argument.saturating_add(4), &records))
                .map(|()| 0)
                .map_err(|_| Errno::EFAULT)
        }
        CLEAR_REQUEST => {
            if !super::time::is_privileged(process) {
                return Err(Errno::EPERM);
            }
            clear();
            Ok(0)
        }
        _ => Err(Errno::ENOTTY),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eggos_android_log::{Buffer, Entry, priority};

    fn record(message: &[u8]) -> Vec<u8> {
        let entry = Entry {
            buffer: Buffer::Main,
            priority: priority::INFO,
            pid: 5,
            tid: 6,
            seconds: 7,
            nanoseconds: 8,
            tag: b"tag",
            message,
        };
        let mut bytes = alloc::vec![0; format::record_size(&entry)];
        format::encode_record(&entry, &mut bytes).unwrap();
        bytes
    }

    #[test]
    fn a_full_log_drops_its_oldest_records_whole() {
        let mut log = Log::new();
        let big = record(&[b'm'; 4000]);
        let count = CAPACITY / big.len();
        for _ in 0..count {
            assert!(log.append(&big));
        }
        assert_eq!(log.records.len(), count * big.len());
        let newest = record(&[b'z'; 4000]);
        assert!(log.append(&newest));
        assert!(log.records.len() <= CAPACITY);
        let bytes = log.snapshot().unwrap();
        let entries: Vec<_> = format::records(&bytes).collect();
        assert_eq!(entries.len(), count);
        assert_eq!(entries[0].message[0], b'm');
        assert_eq!(entries[count - 1].message[0], b'z');
    }

    #[test]
    fn reads_copy_from_an_offset_across_the_ring_s_two_halves() {
        let mut log = Log::new();
        let big = record(&[b'a'; 4000]);
        for _ in 0..CAPACITY / big.len() + 3 {
            assert!(log.append(&big));
        }
        let whole = log.snapshot().unwrap();
        let mut out = alloc::vec![0; whole.len()];
        assert_eq!(log.copy_from(0, &mut out), whole.len());
        assert_eq!(out, whole);
        let mut tail = [0u8; 100];
        let at = whole.len() - 50;
        assert_eq!(log.copy_from(at, &mut tail), 50);
        assert_eq!(&tail[..50], &whole[at..]);
        assert_eq!(log.copy_from(whole.len() + 10, &mut tail), 0);
    }

    #[test]
    fn a_datagram_becomes_an_entry_of_its_sender() {
        let entry = Entry {
            buffer: Buffer::Main,
            priority: priority::WARN,
            pid: 0,
            tid: 9,
            seconds: 1,
            nanoseconds: 2,
            tag: b"eggos-kernel-test",
            message: b"written",
        };
        let mut datagram = [0u8; 64];
        let length = format::encode_datagram(&entry, &mut datagram).unwrap();
        assert!(write(&datagram[..length], 77));
        assert!(!write(&datagram[..format::HEADER_SIZE], 77));
        let bytes = LOG.lock().snapshot().unwrap();
        let found = format::records(&bytes)
            .find(|found| found.tag == b"eggos-kernel-test")
            .unwrap();
        assert_eq!(found, Entry { pid: 77, ..entry });
    }
}
