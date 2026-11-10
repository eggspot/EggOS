//! `download list`, `download cancel` and `download clear` (D225, docs/ux/downloads-v0.3.md
//! section 10): the downloads `fetch` recorded since boot in `/run/eggos/downloads`, newest
//! first, with the Downloads window's words and its judgement of a `downloading` record whose
//! `fetch` is gone; stopping one with `SIGTERM`; removing the records of those that are over.
//! The window's `Cancel` and `Clear` and the MCP tools `download_list`, `download_cancel` and
//! `download_clear` do the same (D22).

use crate::ls::record_names;
use crate::options::parse_defined;
use crate::output::{FAILURE, Output};
use crate::system::System;
use core::cmp::Ordering;
use core::ffi::CStr;
use eggos_commands::{ErrorDefinition, commands, errors};
use eggos_download_list::{
    CancelError, ClearError, ESRCH, MAX_DOWNLOADS, MAX_RECORD_BYTES, Record, State, cancel,
    clear_refusal, fetch_running, id_of, newest_first, runs_fetch, write_cleared,
};
use eggos_mcp::json::JsonWriter;
use eggos_runtime::errno::Errno;

const FOLDER: &CStr = c"/run/eggos/downloads";
const PROCESSES: &CStr = c"/proc/ps";
const PROCESSES_BYTES: usize = 32 * 1024;
const COMMAND_LINE_BYTES: usize = 4096;
const RECORD_PATH_BYTES: usize = 48;
const DIRECTORY_PIECE: usize = 4096;
const JSON_BYTES: usize = MAX_RECORD_BYTES + 1024;
const USAGE: &[u8] =
    b"usage: download list [--json] | download cancel <id> | download clear [<id>...]";

const ID_WIDTH: usize = 6;
const NAME_WIDTH: usize = 24;
const SIZE_WIDTH: usize = 20;
const STATE_WIDTH: usize = 34;

pub fn download<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let Some((verb, rest)) = arguments.split_first() else {
        output.coded(&errors::USAGE, &[USAGE]);
        return FAILURE;
    };
    match verb.to_bytes() {
        b"list" => list(output, rest),
        b"cancel" => cancel_one(output, rest),
        b"clear" => clear(output, rest),
        other => {
            output.coded(&errors::USAGE, &[b"unknown verb '", other, b"': ", USAGE]);
            FAILURE
        }
    }
}

struct Processes {
    text: [u8; PROCESSES_BYTES],
    length: Option<usize>,
}

impl Processes {
    fn read<S: System>(system: &mut S) -> Self {
        let mut processes = Self {
            text: [0; PROCESSES_BYTES],
            length: None,
        };
        processes.length = read_file(system, PROCESSES, &mut processes.text)
            .ok()
            .filter(|&length| length < PROCESSES_BYTES);
        processes
    }

    fn running(&self, id: u32) -> Option<bool> {
        self.length
            .map(|length| fetch_running(&self.text[..length], id))
    }
}

fn command_line_runs_fetch<S: System>(system: &mut S, pid: u32) -> bool {
    let mut path = [0u8; RECORD_PATH_BYTES];
    let mut length = 0;
    let mut put = |bytes: &[u8]| {
        let end = (length + bytes.len()).min(RECORD_PATH_BYTES - 1);
        path[length..end].copy_from_slice(&bytes[..end - length]);
        length = end;
    };
    put(b"/proc/");
    eggos_download_list::write_decimal(u64::from(pid), &mut put);
    put(b"/cmdline");
    let Ok(path) = CStr::from_bytes_until_nul(&path[..=length]) else {
        return false;
    };
    let mut command_line = [0u8; COMMAND_LINE_BYTES];
    read_file(system, path, &mut command_line)
        .is_ok_and(|length| runs_fetch(&command_line[..length]))
}

struct Downloads {
    order: [(u64, u32); MAX_DOWNLOADS],
    count: usize,
}

impl Downloads {
    fn keep(&mut self, download: (u64, u32)) {
        if self.count < MAX_DOWNLOADS {
            self.order[self.count] = download;
            self.count += 1;
            return;
        }
        let oldest = self
            .order
            .iter_mut()
            .max_by(|first, second| newest_first(**first, **second));
        if let Some(oldest) = oldest
            && newest_first(download, *oldest) == Ordering::Less
        {
            *oldest = download;
        }
    }
}

fn read_file<S: System>(system: &mut S, path: &CStr, text: &mut [u8]) -> Result<usize, Errno> {
    let descriptor = system.open(path)?;
    let mut length = 0;
    let read = loop {
        if length == text.len() {
            break Ok(());
        }
        match system.read(descriptor, &mut text[length..]) {
            Ok(0) => break Ok(()),
            Ok(count) => length += count,
            Err(Errno::EINTR) => {}
            Err(errno) => break Err(errno),
        }
    };
    system.close(descriptor);
    read.map(|()| length)
}

fn record_path(id: u32, out: &mut [u8; RECORD_PATH_BYTES]) -> &CStr {
    let mut length = 0;
    let mut put = |bytes: &[u8]| {
        let end = (length + bytes.len()).min(RECORD_PATH_BYTES - 1);
        out[length..end].copy_from_slice(&bytes[..end - length]);
        length = end;
    };
    put(FOLDER.to_bytes());
    put(b"/");
    eggos_download_list::write_decimal(u64::from(id), &mut put);
    out[length] = 0;
    CStr::from_bytes_until_nul(&out[..=length]).unwrap_or(c"")
}

fn read_record<'t, S: System>(
    system: &mut S,
    id: u32,
    text: &'t mut [u8; MAX_RECORD_BYTES],
) -> Option<Record<'t>> {
    let mut path = [0u8; RECORD_PATH_BYTES];
    let length = read_file(system, record_path(id, &mut path), text).ok()?;
    Record::parse(&text[..length])
}

fn collect<S: System>(system: &mut S) -> Result<Downloads, Errno> {
    let mut downloads = Downloads {
        order: [(0, 0); MAX_DOWNLOADS],
        count: 0,
    };
    let folder = match system.open(FOLDER) {
        Ok(folder) => folder,
        Err(Errno::ENOENT) => return Ok(downloads),
        Err(errno) => return Err(errno),
    };
    let mut piece = [0u8; DIRECTORY_PIECE];
    let mut text = [0u8; MAX_RECORD_BYTES];
    let listed = loop {
        match system.directory_entries(folder, &mut piece) {
            Ok(0) => break Ok(()),
            Ok(length) => {
                for id in record_names(&piece[..length]).filter_map(id_of) {
                    if let Some(record) = read_record(system, id, &mut text) {
                        downloads.keep((record.started, id));
                    }
                }
            }
            Err(Errno::EINTR) => {}
            Err(errno) => break Err(errno),
        }
    };
    system.close(folder);
    listed?;
    downloads.order[..downloads.count]
        .sort_unstable_by(|first, second| newest_first(*first, *second));
    Ok(downloads)
}

fn list<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut json = false;
    let parsed = parse_defined(output, arguments, &commands::DOWNLOAD_LIST, |_, given| {
        json |= given.is_long("json");
        Ok(())
    });
    let Ok(operands) = parsed else {
        return FAILURE;
    };
    if let Some(extra) = operands.all().first() {
        output.coded(
            &errors::USAGE,
            &[b"extra operand '", extra.to_bytes(), b"'"],
        );
        return FAILURE;
    }
    let downloads = match collect(&mut output.system) {
        Ok(downloads) => downloads,
        Err(errno) => {
            output.file_error(FOLDER, errno);
            return FAILURE;
        }
    };
    if downloads.count == 0 {
        if !json {
            output.write(b"No downloads since boot.\n");
        }
        return 0;
    }
    let processes = Processes::read(&mut output.system);
    if !json {
        write_header(output);
    }
    let mut text = [0u8; MAX_RECORD_BYTES];
    for &(_, id) in &downloads.order[..downloads.count] {
        let Some(record) = read_record(&mut output.system, id, &mut text) else {
            continue;
        };
        let record = record.judged(processes.running(id));
        match json {
            true => write_json(output, id, &record),
            false => write_row(output, id, &record),
        }
    }
    0
}

fn columns(bytes: &[u8]) -> usize {
    bytes.iter().filter(|&&byte| byte & 0xc0 != 0x80).count()
}

fn write_padded<S: System>(output: &mut Output<S>, cell: &[u8], width: usize) {
    output.write(cell);
    for _ in columns(cell)..width.max(columns(cell) + 1) {
        output.write(b" ");
    }
}

fn written(write: impl FnOnce(&mut dyn FnMut(&[u8]))) -> ([u8; 256], usize) {
    let mut cell = [0u8; 256];
    let mut length = 0;
    write(&mut |bytes: &[u8]| {
        let end = (length + bytes.len()).min(cell.len());
        cell[length..end].copy_from_slice(&bytes[..end - length]);
        length = end;
    });
    (cell, length)
}

fn write_header<S: System>(output: &mut Output<S>) {
    write_padded(output, b"ID", ID_WIDTH);
    write_padded(output, b"NAME", NAME_WIDTH);
    write_padded(output, b"SIZE", SIZE_WIDTH);
    write_padded(output, b"STATE", STATE_WIDTH);
    output.write(b"FROM\n");
}

fn write_row<S: System>(output: &mut Output<S>, id: u32, record: &Record<'_>) {
    let (digits, length) = written(|out| eggos_download_list::write_decimal(u64::from(id), out));
    write_padded(output, &digits[..length], ID_WIDTH);
    write_padded(output, record.name(), NAME_WIDTH);
    let (size, length) = written(|out| record.write_size(out));
    write_padded(output, &size[..length], SIZE_WIDTH);
    let (state, length) = written(|out| record.write_state(out));
    write_padded(output, &state[..length], STATE_WIDTH);
    output.write(record.host());
    output.write(b"\n");
}

fn write_json<S: System>(output: &mut Output<S>, id: u32, record: &Record<'_>) {
    let mut buffer = [0u8; JSON_BYTES];
    let mut out = JsonWriter::new(&mut buffer);
    let time = |out: &mut JsonWriter<'_>, seconds: Option<u64>| match seconds
        .and_then(|seconds| eggos_calendar::rfc3339(seconds as i64))
    {
        Some(text) => out.string_lossy(&text),
        None => out.null(),
    };
    out.begin_object();
    out.key("id");
    out.u64(u64::from(id));
    for (key, value) in [
        ("name", record.name()),
        ("path", record.path),
        ("url", record.url),
    ] {
        out.key(key);
        out.string_lossy(value);
    }
    let (size, length) = written(|out| record.write_size(out));
    out.key("size_text");
    out.string_lossy(&size[..length]);
    out.key("state");
    out.string(record.state.word());
    let (state, length) = written(|out| record.write_state(out));
    out.key("state_text");
    out.string_lossy(&state[..length]);
    out.key("reason");
    match record.state {
        State::Failed => out.string_lossy(record.reason),
        _ => out.null(),
    }
    out.key("percent");
    match record.percent() {
        Some(percent) => out.u64(percent),
        None => out.null(),
    }
    out.key("received_bytes");
    out.u64(record.received);
    out.key("total_bytes");
    match record.total {
        Some(total) => out.u64(total),
        None => out.null(),
    }
    out.key("started_at");
    time(&mut out, Some(record.started));
    out.key("ended_at");
    time(&mut out, record.ended);
    out.end_object();
    out.end_line();
    if !out.overflowed() {
        output.write(out.as_bytes());
    }
}

fn id_operand(operand: &CStr) -> Option<u32> {
    crate::options::parse_count(operand.to_bytes()).and_then(|id| u32::try_from(id).ok())
}

fn refuse<S: System>(output: &mut Output<S>, error: &ErrorDefinition, id: &[u8], words: &str) {
    output.coded(error, &[id, b": ", words.as_bytes()]);
}

fn cancel_one<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let parsed = parse_defined(output, arguments, &commands::DOWNLOAD_CANCEL, |_, _| Ok(()));
    let Ok(operands) = parsed else {
        return FAILURE;
    };
    let [operand] = operands.all() else {
        output.coded(&errors::USAGE, &[b"usage: download cancel <id>"]);
        return FAILURE;
    };
    let Some(id) = id_operand(operand) else {
        output.coded(
            &errors::USAGE,
            &[b"'", operand.to_bytes(), b"' is not a download's id"],
        );
        return FAILURE;
    };
    let processes = Processes::read(&mut output.system);
    let mut text = [0u8; MAX_RECORD_BYTES];
    let record = read_record(&mut output.system, id, &mut text)
        .map(|record| record.judged(processes.running(id)));
    let system = &mut output.system;
    let cancelled = cancel(record.as_ref(), id, |pid, signal| {
        let is_fetch = processes
            .running(pid)
            .unwrap_or_else(|| command_line_runs_fetch(system, pid));
        if !is_fetch {
            return Err(ESRCH);
        }
        system
            .send_signal(pid as i32, signal)
            .map_err(|errno| i32::from(errno.0))
    });
    let error = match cancelled {
        Ok(()) => return 0,
        Err(CancelError::NoSuchDownload) => &errors::NO_SUCH_DOWNLOAD,
        Err(CancelError::AlreadyEnded) => &errors::ALREADY_ENDED,
        Err(CancelError::NotAllowed) => &errors::PERMISSION_DENIED,
    };
    let words = cancelled.err().map_or("", CancelError::words);
    refuse(output, error, operand.to_bytes(), words);
    FAILURE
}

fn clear<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let parsed = parse_defined(output, arguments, &commands::DOWNLOAD_CLEAR, |_, _| Ok(()));
    let Ok(operands) = parsed else {
        return FAILURE;
    };
    let mut ids = [0u32; MAX_DOWNLOADS];
    let mut count = 0;
    for operand in operands.all() {
        let Some(id) = id_operand(operand) else {
            output.coded(
                &errors::USAGE,
                &[b"'", operand.to_bytes(), b"' is not a download's id"],
            );
            return FAILURE;
        };
        if count < MAX_DOWNLOADS {
            ids[count] = id;
            count += 1;
        }
    }
    let named = count > 0;
    if !named {
        let downloads = match collect(&mut output.system) {
            Ok(downloads) => downloads,
            Err(errno) => {
                output.file_error(FOLDER, errno);
                return FAILURE;
            }
        };
        for (place, &(_, id)) in ids.iter_mut().zip(&downloads.order[..downloads.count]) {
            *place = id;
        }
        count = downloads.count;
    }
    let processes = Processes::read(&mut output.system);
    let mut status = 0;
    let mut cleared = 0;
    let mut text = [0u8; MAX_RECORD_BYTES];
    for &id in &ids[..count] {
        let record = read_record(&mut output.system, id, &mut text)
            .map(|record| record.judged(processes.running(id)));
        match clear_refusal(record.as_ref()) {
            None => {}
            Some(_) if !named => continue,
            Some(refusal) => {
                let error = match refusal {
                    ClearError::NoSuchDownload => &errors::NO_SUCH_DOWNLOAD,
                    ClearError::StillDownloading => &errors::STILL_DOWNLOADING,
                };
                let (digits, length) =
                    written(|out| eggos_download_list::write_decimal(u64::from(id), out));
                refuse(output, error, &digits[..length], refusal.words());
                status = FAILURE;
                continue;
            }
        }
        let mut path = [0u8; RECORD_PATH_BYTES];
        match output.system.remove(record_path(id, &mut path), false) {
            Ok(()) | Err(Errno::ENOENT) => cleared += 1,
            Err(errno) => {
                let mut path = [0u8; RECORD_PATH_BYTES];
                output.file_error(record_path(id, &mut path), errno);
                status = FAILURE;
            }
        }
    }
    if cleared > 0 || status == 0 {
        let (words, length) = written(|out| write_cleared(cleared, out));
        output.write(&words[..length]);
        output.write(b"\n");
    }
    status
}

#[cfg(test)]
pub(crate) mod tests;
