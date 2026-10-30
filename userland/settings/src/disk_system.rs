//! The `disk` command's system (D192): the disks file, the disks' files through `openat`,
//! `pread64`, `pwrite64` and `fsync`, `statfs` of a mount point, the clock and the terminal.

use core::ffi::CStr;
use eggos_files::storage::{DISKS_FILE, Fault, Usage, WholeDisks};
use eggos_runtime::errno::Errno;
use eggos_runtime::startup::Startup;
use eggos_runtime::syscall::{call, number, write_all};
use eggos_settings::disk::{self, Console, USAGE, USAGE_STATUS};

const AT_FDCWD: usize = -100isize as usize;
const O_RDONLY_CLOEXEC: usize = 0o2_000_000;
const O_RDWR_CLOEXEC: usize = 0o2 | 0o2_000_000;
const PATH_CAPACITY: usize = 64;
const STATFS_SIZE: usize = 120;
const TCGETS: usize = 0x5401;
const TERMIOS_SIZE: usize = 64;
const STANDARD_INPUT: usize = 0;
const STANDARD_OUTPUT: usize = 1;
const STANDARD_ERROR: usize = 2;
const CLOCK_REALTIME: usize = 0;
const MOST_A_CALL: usize = 60 * 1024;
const MAX_ARGUMENTS: usize = 8;

fn syscall(number: usize, arguments: [usize; 6]) -> Result<usize, Errno> {
    // SAFETY: every caller passes buffers it owns and borrows for the call, sized as the call
    // reads or writes them, or plain numbers; no call here maps or unmaps memory.
    Errno::result(unsafe { call(number, arguments) })
}

fn fault(errno: Errno) -> Fault {
    match errno.0 {
        code if code == Errno::EPERM.0 || code == Errno::EACCES.0 => Fault::NotAllowed,
        code if code == Errno::EBUSY.0 => Fault::Busy,
        code if code == Errno::EROFS.0 => Fault::ReadOnly,
        _ => Fault::Failed,
    }
}

fn c_path(path: &str) -> [u8; PATH_CAPACITY] {
    let mut bytes = [0u8; PATH_CAPACITY];
    let length = path.len().min(PATH_CAPACITY - 1);
    bytes[..length].copy_from_slice(&path.as_bytes()[..length]);
    bytes
}

fn open(path: &str, flags: usize) -> Result<usize, Fault> {
    let path = c_path(path);
    syscall(
        number::OPENAT,
        [AT_FDCWD, path.as_ptr() as usize, flags, 0, 0, 0],
    )
    .map_err(fault)
}

/// The disks as the kernel offers them.
#[derive(Debug)]
pub struct LinuxDisks;

impl WholeDisks for LinuxDisks {
    fn read_disks_file(&mut self, buffer: &mut [u8]) -> Result<usize, Fault> {
        let descriptor = open(DISKS_FILE, O_RDONLY_CLOEXEC)?;
        let mut length = 0;
        while length < buffer.len() {
            let rest = &mut buffer[length..];
            let read = syscall(
                number::READ,
                [descriptor, rest.as_mut_ptr() as usize, rest.len(), 0, 0, 0],
            );
            match read {
                Ok(0) => break,
                Ok(read) => length += read,
                Err(errno) => {
                    self.close(descriptor);
                    return Err(fault(errno));
                }
            }
        }
        self.close(descriptor);
        Ok(length)
    }

    fn open(&mut self, device: &str, writable: bool) -> Result<usize, Fault> {
        let flags = match writable {
            true => O_RDWR_CLOEXEC,
            false => O_RDONLY_CLOEXEC,
        };
        open(device, flags)
    }

    fn read_at(&mut self, handle: usize, offset: u64, buffer: &mut [u8]) -> Result<(), Fault> {
        let mut done = 0;
        while done < buffer.len() {
            let piece = &mut buffer[done..];
            let length = piece.len().min(MOST_A_CALL);
            let at = offset.saturating_add(done as u64) as usize;
            let read = syscall(
                number::PREAD64,
                [handle, piece.as_mut_ptr() as usize, length, at, 0, 0],
            )
            .map_err(fault)?;
            if read == 0 {
                return Err(Fault::Failed);
            }
            done += read;
        }
        Ok(())
    }

    fn write_at(&mut self, handle: usize, offset: u64, bytes: &[u8]) -> Result<(), Fault> {
        let mut done = 0;
        while done < bytes.len() {
            let piece = &bytes[done..];
            let length = piece.len().min(MOST_A_CALL);
            let at = offset.saturating_add(done as u64) as usize;
            let written = syscall(
                number::PWRITE64,
                [handle, piece.as_ptr() as usize, length, at, 0, 0],
            )
            .map_err(fault)?;
            if written == 0 {
                return Err(Fault::Failed);
            }
            done += written;
        }
        Ok(())
    }

    fn flush(&mut self, handle: usize) -> Result<(), Fault> {
        syscall(number::FSYNC, [handle, 0, 0, 0, 0, 0])
            .map(|_| ())
            .map_err(fault)
    }

    fn close(&mut self, handle: usize) {
        let _ = syscall(number::CLOSE, [handle, 0, 0, 0, 0, 0]);
    }

    fn usage(&mut self, place: &str) -> Option<Usage> {
        let path = c_path(place);
        let mut record = [0u8; STATFS_SIZE];
        syscall(
            number::STATFS,
            [
                path.as_ptr() as usize,
                record.as_mut_ptr() as usize,
                0,
                0,
                0,
                0,
            ],
        )
        .ok()?;
        let word = |offset: usize| {
            let mut value = [0u8; 8];
            value.copy_from_slice(&record[offset..offset + 8]);
            u64::from_ne_bytes(value)
        };
        let fragment = word(72);
        let block = if fragment == 0 { word(8) } else { fragment };
        let (blocks, free, available) = (word(16), word(24), word(32));
        Some(Usage {
            size_bytes: blocks.saturating_mul(block),
            used_bytes: blocks.saturating_sub(free).saturating_mul(block),
            available_bytes: available.saturating_mul(block),
        })
    }

    fn now(&mut self) -> u64 {
        let mut time = [0u64; 2];
        match syscall(
            number::CLOCK_GETTIME,
            [CLOCK_REALTIME, time.as_mut_ptr() as usize, 0, 0, 0, 0],
        ) {
            Ok(_) => time[0],
            Err(_) => 0,
        }
    }
}

fn is_terminal(descriptor: usize) -> bool {
    let mut settings = [0u8; TERMIOS_SIZE];
    syscall(
        number::IOCTL,
        [descriptor, TCGETS, settings.as_mut_ptr() as usize, 0, 0, 0],
    )
    .is_ok()
}

/// The command's standard input, output and error.
#[derive(Debug)]
pub struct Terminal;

impl Console for Terminal {
    fn write_output(&mut self, bytes: &[u8]) {
        write_all(STANDARD_OUTPUT, bytes);
    }

    fn write_error(&mut self, bytes: &[u8]) {
        write_all(STANDARD_ERROR, bytes);
    }

    fn is_terminal(&mut self) -> bool {
        is_terminal(STANDARD_INPUT) && is_terminal(STANDARD_ERROR)
    }

    fn read_answer(&mut self, buffer: &mut [u8]) -> usize {
        let mut length = 0;
        while length < buffer.len() {
            let rest = &mut buffer[length..];
            match syscall(
                number::READ,
                [
                    STANDARD_INPUT,
                    rest.as_mut_ptr() as usize,
                    rest.len(),
                    0,
                    0,
                    0,
                ],
            ) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    length += read;
                    if buffer[..length].contains(&b'\n') {
                        break;
                    }
                }
            }
        }
        length
    }
}

/// Runs `disk` with the program's arguments; the exit status.
pub fn run(startup: &Startup) -> i32 {
    let mut arguments = [""; MAX_ARGUMENTS];
    let mut count = 0;
    for argument in startup.arguments().skip(1).map(CStr::to_bytes) {
        let Ok(argument) = core::str::from_utf8(argument) else {
            write_all(STANDARD_ERROR, USAGE.as_bytes());
            return USAGE_STATUS;
        };
        if count == arguments.len() {
            write_all(STANDARD_ERROR, USAGE.as_bytes());
            return USAGE_STATUS;
        }
        arguments[count] = argument;
        count += 1;
    }
    disk::run(&mut LinuxDisks, &mut Terminal, &arguments[..count])
}
