use crate::system::System;
use core::cell::Cell;
use core::ffi::CStr;
use eggos_download_list::{
    FOLDER, MAX_RECORD_BYTES, PARENT_FOLDER, Record, State, WRITE_INTERVAL_US, reason_of,
};

const MAX_PATH_BYTES: usize = 4096;
const RECORD_PATH_BYTES: usize = 64;
const FOLDER_MODE: u32 = 0o755;
const RECORD_MODE: u32 = 0o644;

pub(super) struct Recorder<'u> {
    url: &'u [u8],
    partial: &'u CStr,
    path: [u8; MAX_PATH_BYTES],
    path_length: usize,
    record: [u8; RECORD_PATH_BYTES],
    temporary: [u8; RECORD_PATH_BYTES],
    started: u64,
    received: Cell<u64>,
    total: Cell<Option<u64>>,
    written_us: Cell<u64>,
}

fn record_path(out: &mut [u8; RECORD_PATH_BYTES], pid: u32, temporary: bool) -> Option<()> {
    let mut length = 0;
    let mut put = |bytes: &[u8]| -> Option<()> {
        let end = length + bytes.len();
        out.get_mut(length..end)?.copy_from_slice(bytes);
        length = end;
        Some(())
    };
    put(FOLDER.as_bytes())?;
    put(if temporary { &b"/."[..] } else { b"/" })?;
    let mut fits = Some(());
    eggos_download_list::write_decimal(u64::from(pid), &mut |digits| {
        fits = fits.and(put(digits));
    });
    fits?;
    if temporary {
        put(b".new")?;
    }
    put(b"\0")
}

fn c_path(bytes: &[u8]) -> Option<&CStr> {
    CStr::from_bytes_until_nul(bytes).ok()
}

impl<'u> Recorder<'u> {
    pub(super) fn start<S: System>(system: &mut S, url: &'u [u8], partial: &'u CStr) -> Self {
        let mut recorder = Self {
            url,
            partial,
            path: [0; MAX_PATH_BYTES],
            path_length: 0,
            record: [0; RECORD_PATH_BYTES],
            temporary: [0; RECORD_PATH_BYTES],
            started: system.now(),
            received: Cell::new(0),
            total: Cell::new(None),
            written_us: Cell::new(system.monotonic_us()),
        };
        recorder.path_length = absolute(system, partial.to_bytes(), &mut recorder.path);
        let pid = system.process_id();
        let named = record_path(&mut recorder.record, pid, false)
            .and_then(|()| record_path(&mut recorder.temporary, pid, true));
        if named.is_none() {
            recorder.record[0] = 0;
        }
        let _ = system.catch_termination();
        for folder in [PARENT_FOLDER, FOLDER] {
            let mut bytes = [0u8; RECORD_PATH_BYTES];
            bytes[..folder.len()].copy_from_slice(folder.as_bytes());
            if let Some(folder) = c_path(&bytes) {
                let _ = system.make_directory(folder, FOLDER_MODE);
            }
        }
        recorder.write(system, State::Downloading, b"", None);
        recorder
    }

    pub(super) fn set_total(&self, total: Option<u64>) {
        self.total.set(total);
    }

    pub(super) fn progress<S: System>(&self, system: &mut S, received: u64) {
        self.received.set(received);
        let now = system.monotonic_us();
        if now.wrapping_sub(self.written_us.get()) >= WRITE_INTERVAL_US {
            self.written_us.set(now);
            self.write(system, State::Downloading, b"", None);
        }
    }

    pub(super) fn done<S: System>(&self, system: &mut S) {
        let ended = system.now();
        self.write(system, State::Done, b"", Some(ended));
    }

    pub(super) fn failed<S: System>(&self, system: &mut S, message: &[u8]) {
        let ended = system.now();
        self.write(system, State::Failed, reason_of(message), Some(ended));
    }

    pub(super) fn cancelled<S: System>(&self, system: &mut S) {
        let _ = system.remove(self.partial, false);
        let ended = system.now();
        self.write(system, State::Cancelled, b"", Some(ended));
    }

    fn write<S: System>(&self, system: &mut S, state: State, reason: &[u8], ended: Option<u64>) {
        let (Some(record), Some(temporary)) = (c_path(&self.record), c_path(&self.temporary))
        else {
            return;
        };
        if record.is_empty() {
            return;
        }
        let shown = Record {
            url: self.url,
            path: &self.path[..self.path_length],
            received: self.received.get(),
            total: self.total.get(),
            state,
            reason,
            started: self.started,
            ended,
        };
        let mut text = [0u8; MAX_RECORD_BYTES];
        let Some(length) = shown.write(&mut text) else {
            return;
        };
        let Ok(file) = system.create(temporary, RECORD_MODE, true) else {
            return;
        };
        let written = system.write(file, &text[..length]);
        system.close(file);
        if written.is_ok() {
            let _ = system.rename(temporary, record);
        }
    }
}

fn absolute<S: System>(system: &mut S, path: &[u8], out: &mut [u8; MAX_PATH_BYTES]) -> usize {
    let mut length = 0;
    if path.first() != Some(&b'/')
        && let Ok(folder) = system.working_directory(out)
    {
        length = folder;
        if length > 0 && out[length - 1] != b'/' && length < out.len() {
            out[length] = b'/';
            length += 1;
        }
    }
    let taken = path.len().min(out.len() - length);
    out[length..length + taken].copy_from_slice(&path[..taken]);
    length + taken
}

#[cfg(test)]
mod tests;
