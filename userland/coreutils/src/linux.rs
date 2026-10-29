use crate::system::{
    CapturedRows, Duration, STATX_SIZE, Status, SymbolicLinks, System, SystemName, UTSNAME_SIZE,
};
use crate::utilities::{self, MAX_ARGUMENTS};
use core::ffi::CStr;
use eggos_runtime::errno::Errno;
use eggos_runtime::startup::Startup;
use eggos_runtime::syscall::{self, number};

const AT_FDCWD: usize = -100isize as usize;
const AT_SYMLINK_NOFOLLOW: usize = 0x100;
const O_RDONLY_CLOEXEC: usize = 0o2_000_000;
const O_WRONLY: usize = 0o1;
const O_CREAT: usize = 0o100;
const O_TRUNC: usize = 0o1000;
const O_APPEND: usize = 0o2000;
const AT_REMOVEDIR: usize = 0x200;
const STATX_BASIC_STATS: usize = 0x7ff;
const CLOCK_MONOTONIC: usize = 1;
const CLOCK_REALTIME: usize = 0;
const REBOOT_MAGIC1: usize = 0xfee1_dead;
const REBOOT_MAGIC2: usize = 0x2812_1969;
const EIO: Errno = Errno(5);

#[repr(C)]
struct Timespec {
    seconds: i64,
    nanoseconds: i64,
}

fn call(number: usize, arguments: [usize; 6]) -> Result<usize, Errno> {
    // SAFETY: every call the utilities make passes only memory they own for the call's
    // length, and none maps, unmaps or protects memory.
    Errno::result(unsafe { syscall::call(number, arguments) })
}

#[derive(Debug)]
struct Linux;

impl System for Linux {
    fn write(&mut self, descriptor: usize, mut bytes: &[u8]) -> Result<(), Errno> {
        while !bytes.is_empty() {
            let pointer = bytes.as_ptr() as usize;
            match call(number::WRITE, [descriptor, pointer, bytes.len(), 0, 0, 0]) {
                Ok(0) => return Err(EIO),
                Ok(written) => bytes = &bytes[written.min(bytes.len())..],
                Err(Errno::EINTR) => {}
                Err(errno) => return Err(errno),
            }
        }
        Ok(())
    }

    fn read(&mut self, descriptor: usize, buffer: &mut [u8]) -> Result<usize, Errno> {
        let pointer = buffer.as_mut_ptr() as usize;
        call(number::READ, [descriptor, pointer, buffer.len(), 0, 0, 0])
    }

    fn open(&mut self, path: &CStr) -> Result<usize, Errno> {
        let path = path.as_ptr() as usize;
        call(number::OPENAT, [AT_FDCWD, path, O_RDONLY_CLOEXEC, 0, 0, 0])
    }

    fn close(&mut self, descriptor: usize) {
        let _ = call(number::CLOSE, [descriptor, 0, 0, 0, 0, 0]);
    }

    fn directory_entries(&mut self, descriptor: usize, buffer: &mut [u8]) -> Result<usize, Errno> {
        let pointer = buffer.as_mut_ptr() as usize;
        call(
            number::GETDENTS64,
            [descriptor, pointer, buffer.len(), 0, 0, 0],
        )
    }

    fn status(
        &mut self,
        directory: Option<usize>,
        path: &CStr,
        links: SymbolicLinks,
    ) -> Result<Status, Errno> {
        let mut record = [0u8; STATX_SIZE];
        let flags = match links {
            SymbolicLinks::Follow => 0,
            SymbolicLinks::DoNotFollow => AT_SYMLINK_NOFOLLOW,
        };
        call(
            number::STATX,
            [
                directory.unwrap_or(AT_FDCWD),
                path.as_ptr() as usize,
                flags,
                STATX_BASIC_STATS,
                record.as_mut_ptr() as usize,
                0,
            ],
        )?;
        Ok(Status::from_statx(&record))
    }

    fn system_name(&mut self) -> Result<SystemName, Errno> {
        let mut name = SystemName([0; UTSNAME_SIZE]);
        call(number::UNAME, [name.0.as_mut_ptr() as usize, 0, 0, 0, 0, 0])?;
        Ok(name)
    }

    fn sleep(&mut self, duration: Duration) -> Result<(), Errno> {
        let mut request = Timespec {
            seconds: i64::try_from(duration.seconds).unwrap_or(i64::MAX),
            nanoseconds: i64::from(duration.nanoseconds),
        };
        loop {
            let mut remaining = Timespec {
                seconds: 0,
                nanoseconds: 0,
            };
            match call(
                number::CLOCK_NANOSLEEP,
                [
                    CLOCK_MONOTONIC,
                    0,
                    &raw const request as usize,
                    &raw mut remaining as usize,
                    0,
                    0,
                ],
            ) {
                Err(Errno::EINTR) => request = remaining,
                result => return result.map(|_| ()),
            }
        }
    }

    fn sync(&mut self) {
        let _ = call(number::SYNC, [0; 6]);
    }

    fn reboot(&mut self, command: u32) -> Result<(), Errno> {
        call(
            number::REBOOT,
            [REBOOT_MAGIC1, REBOOT_MAGIC2, command as usize, 0, 0, 0],
        )
        .map(|_| ())
    }

    fn create(&mut self, path: &CStr, mode: u32, truncate: bool) -> Result<usize, Errno> {
        let flags = O_RDONLY_CLOEXEC | O_WRONLY | O_CREAT | if truncate { O_TRUNC } else { 0 };
        let path = path.as_ptr() as usize;
        call(number::OPENAT, [AT_FDCWD, path, flags, mode as usize, 0, 0])
    }

    fn append(&mut self, path: &CStr, mode: u32) -> Result<usize, Errno> {
        let flags = O_RDONLY_CLOEXEC | O_WRONLY | O_CREAT | O_APPEND;
        let path = path.as_ptr() as usize;
        call(number::OPENAT, [AT_FDCWD, path, flags, mode as usize, 0, 0])
    }

    fn make_directory(&mut self, path: &CStr, mode: u32) -> Result<(), Errno> {
        let path = path.as_ptr() as usize;
        call(number::MKDIRAT, [AT_FDCWD, path, mode as usize, 0, 0, 0]).map(|_| ())
    }

    fn remove(&mut self, path: &CStr, directory: bool) -> Result<(), Errno> {
        let flags = if directory { AT_REMOVEDIR } else { 0 };
        let path = path.as_ptr() as usize;
        call(number::UNLINKAT, [AT_FDCWD, path, flags, 0, 0, 0]).map(|_| ())
    }

    fn rename(&mut self, from: &CStr, to: &CStr) -> Result<(), Errno> {
        let (from, to) = (from.as_ptr() as usize, to.as_ptr() as usize);
        call(number::RENAMEAT2, [AT_FDCWD, from, AT_FDCWD, to, 0, 0]).map(|_| ())
    }

    fn set_modified_now(&mut self, path: &CStr) -> Result<(), Errno> {
        let path = path.as_ptr() as usize;
        call(number::UTIMENSAT, [AT_FDCWD, path, 0, 0, 0, 0]).map(|_| ())
    }

    fn working_directory(&mut self, out: &mut [u8]) -> Result<usize, Errno> {
        let length = call(
            number::GETCWD,
            [out.as_mut_ptr() as usize, out.len(), 0, 0, 0, 0],
        )?;
        Ok(length.saturating_sub(1))
    }

    fn now(&mut self) -> u64 {
        let mut time = Timespec {
            seconds: 0,
            nanoseconds: 0,
        };
        match call(
            number::CLOCK_GETTIME,
            [CLOCK_REALTIME, &raw mut time as usize, 0, 0, 0, 0],
        ) {
            Ok(_) => time.seconds.max(0) as u64,
            Err(_) => 0,
        }
    }

    fn set_time(&mut self, seconds: u64) -> Result<(), Errno> {
        let time = Timespec {
            seconds: i64::try_from(seconds).map_err(|_| Errno::EINVAL)?,
            nanoseconds: 0,
        };
        call(
            number::CLOCK_SETTIME,
            [CLOCK_REALTIME, &raw const time as usize, 0, 0, 0, 0],
        )
        .map(|_| ())
    }

    fn device_control(
        &mut self,
        descriptor: usize,
        request: u32,
        argument: &mut [u8],
    ) -> Result<(), Errno> {
        if (request >> 16) & 0x3fff != argument.len() as u32 {
            return Err(Errno::EINVAL);
        }
        let pointer = argument.as_mut_ptr() as usize;
        call(
            number::IOCTL,
            [descriptor, request as usize, pointer, 0, 0, 0],
        )
        .map(|_| ())
    }

    fn capture_window_rows(
        &mut self,
        descriptor: usize,
        window: u32,
        first_row: u32,
        row_count: u32,
        pixels: &mut [u8],
    ) -> Result<CapturedRows, Errno> {
        let mut request = [0u8; crate::wm::CAPTURE_SIZE];
        request[..4].copy_from_slice(&window.to_ne_bytes());
        request[4..8].copy_from_slice(&first_row.to_ne_bytes());
        request[8..12].copy_from_slice(&row_count.to_ne_bytes());
        request[24..32].copy_from_slice(&(pixels.as_mut_ptr() as u64).to_ne_bytes());
        request[32..40].copy_from_slice(&(pixels.len() as u64).to_ne_bytes());
        let pointer = request.as_mut_ptr() as usize;
        call(
            number::IOCTL,
            [
                descriptor,
                crate::wm::CAPTURE_WINDOW as usize,
                pointer,
                0,
                0,
                0,
            ],
        )?;
        let word = |at: usize| {
            u32::from_ne_bytes([
                request[at],
                request[at + 1],
                request[at + 2],
                request[at + 3],
            ])
        };
        Ok(CapturedRows {
            width: word(12),
            height: word(16),
            rows: word(20),
        })
    }
}

/// The program's main function: see the crate's documentation.
pub fn run(startup: &Startup) -> i32 {
    let mut arguments = [c""; MAX_ARGUMENTS];
    let mut count = 0;
    for argument in startup.arguments() {
        if count == MAX_ARGUMENTS {
            syscall::write_all(2, b"coreutils: ");
            syscall::write_all(2, Errno::E2BIG.message());
            syscall::write_all(2, b"\n");
            return 1;
        }
        arguments[count] = argument;
        count += 1;
    }
    utilities::run(Linux, &arguments[..count])
}
