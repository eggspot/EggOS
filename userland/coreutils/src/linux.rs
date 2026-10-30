use crate::system::{
    CapturedRows, DatagramKind, Duration, FileSystemUsage, STATFS_SIZE, STATX_SIZE, Sender, Status,
    SymbolicLinks, System, SystemName, UTSNAME_SIZE, WindowTree, folder_and_leaf,
    refuses_symbolic_links,
};
use crate::utilities::{self, MAX_ARGUMENTS};
use core::ffi::CStr;
use eggos_net::Ipv4Address;
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
const O_PATH: usize = 0o10_000_000;
#[cfg(target_arch = "aarch64")]
const O_DIRECTORY: usize = 0o40_000;
#[cfg(not(target_arch = "aarch64"))]
const O_DIRECTORY: usize = 0o200_000;
const RESOLVE_NO_MAGICLINKS: u64 = 0x02;
const RESOLVE_NO_SYMLINKS: u64 = 0x04;
const FOLDER_PATH_BYTES: usize = 4096;
const ENOSYS: Errno = Errno(38);
const CLOCK_MONOTONIC: usize = 1;
const CLOCK_REALTIME: usize = 0;
const REBOOT_MAGIC1: usize = 0xfee1_dead;
const REBOOT_MAGIC2: usize = 0x2812_1969;
const EIO: Errno = Errno(5);
const AF_INET: u16 = 2;
const SOCK_DGRAM_CLOEXEC: usize = 2 | 0o2_000_000;
const SOCK_STREAM_CLOEXEC: usize = 1 | 0o2_000_000;
const SOL_SOCKET: usize = 1;
const SO_SNDTIMEO: usize = 21;
const MSG_NOSIGNAL: usize = 0x4000;
const IPPROTO_ICMP: usize = 1;
const MSG_DONTWAIT: usize = 0x40;
const POLLIN: i16 = 1;
const SIGSET_BYTES: usize = 8;
const SOCKADDR_IN_SIZE: usize = 16;
const SIGTERM: usize = 15;
const SIG_UNBLOCK: usize = 1;
const INIT: usize = 1;
const TIOCGWINSZ: usize = 0x5413;

#[repr(C)]
struct PollFd {
    descriptor: i32,
    events: i16,
    returned: i16,
}

fn sockaddr_in(address: Ipv4Address, port: u16) -> [u8; SOCKADDR_IN_SIZE] {
    let mut bytes = [0u8; SOCKADDR_IN_SIZE];
    bytes[..2].copy_from_slice(&AF_INET.to_ne_bytes());
    bytes[2..4].copy_from_slice(&port.to_be_bytes());
    bytes[4..8].copy_from_slice(&address.0);
    bytes
}

#[repr(C)]
struct Timespec {
    seconds: i64,
    nanoseconds: i64,
}

#[repr(C)]
struct Timeval {
    seconds: i64,
    microseconds: i64,
}

fn wait_readable(descriptor: usize, timeout_ms: u32) -> Result<bool, Errno> {
    let mut poll = PollFd {
        descriptor: descriptor as i32,
        events: POLLIN,
        returned: 0,
    };
    let timeout = Timespec {
        seconds: i64::from(timeout_ms / 1000),
        nanoseconds: i64::from(timeout_ms % 1000) * 1_000_000,
    };
    loop {
        let polled = call(
            number::PPOLL,
            [
                &raw mut poll as usize,
                1,
                &raw const timeout as usize,
                0,
                SIGSET_BYTES,
                0,
            ],
        );
        match polled {
            Ok(0) => return Ok(false),
            Ok(_) => return Ok(true),
            Err(Errno::EINTR) => {}
            Err(errno) => return Err(errno),
        }
    }
}

fn call(number: usize, arguments: [usize; 6]) -> Result<usize, Errno> {
    // SAFETY: every call the utilities make passes only memory they own for the call's
    // length, and none maps, unmaps or protects memory.
    Errno::result(unsafe { syscall::call(number, arguments) })
}

#[repr(C)]
struct OpenHow {
    flags: u64,
    mode: u64,
    resolve: u64,
}

fn open_refusing_links(
    directory: usize,
    path: &CStr,
    flags: usize,
    mode: u32,
) -> Result<usize, Errno> {
    let how = OpenHow {
        flags: flags as u64,
        mode: u64::from(mode),
        resolve: RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS,
    };
    call(
        number::OPENAT2,
        [
            directory,
            path.as_ptr() as usize,
            &raw const how as usize,
            core::mem::size_of::<OpenHow>(),
            0,
            0,
        ],
    )
}

struct Folder {
    descriptor: usize,
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = call(number::CLOSE, [self.descriptor, 0, 0, 0, 0, 0]);
    }
}

fn in_folder(path: &CStr) -> Result<(Folder, &CStr), Errno> {
    let (folder, start) = folder_and_leaf(path.to_bytes());
    let mut buffer = [0u8; FOLDER_PATH_BYTES];
    buffer
        .get_mut(..folder.len())
        .filter(|_| folder.len() < FOLDER_PATH_BYTES)
        .ok_or(Errno::ENAMETOOLONG)?
        .copy_from_slice(folder);
    let folder = CStr::from_bytes_until_nul(&buffer).map_err(|_| Errno::EINVAL)?;
    let descriptor =
        open_refusing_links(AT_FDCWD, folder, O_PATH | O_DIRECTORY | O_RDONLY_CLOEXEC, 0)?;
    let folder = Folder { descriptor };
    let leaf =
        CStr::from_bytes_with_nul(&path.to_bytes_with_nul()[start..]).map_err(|_| Errno::EINVAL)?;
    match leaf.is_empty() {
        true => Ok((folder, c".")),
        false => Ok((folder, leaf)),
    }
}

fn status_at(directory: usize, path: &CStr, flags: usize) -> Result<Status, Errno> {
    let mut record = [0u8; STATX_SIZE];
    call(
        number::STATX,
        [
            directory,
            path.as_ptr() as usize,
            flags,
            STATX_BASIC_STATS,
            record.as_mut_ptr() as usize,
            0,
        ],
    )?;
    Ok(Status::from_statx(&record))
}

#[derive(Debug)]
struct Linux {
    no_symlinks: bool,
}

impl Linux {
    fn at<T>(
        &self,
        path: &CStr,
        action: impl FnOnce(usize, &CStr) -> Result<T, Errno>,
    ) -> Result<T, Errno> {
        if !self.no_symlinks {
            return action(AT_FDCWD, path);
        }
        let (folder, leaf) = in_folder(path)?;
        action(folder.descriptor, leaf)
    }

    fn open_file(&self, path: &CStr, flags: usize, mode: u32) -> Result<usize, Errno> {
        match self.no_symlinks {
            true => open_refusing_links(AT_FDCWD, path, flags, mode),
            false => call(
                number::OPENAT,
                [AT_FDCWD, path.as_ptr() as usize, flags, mode as usize, 0, 0],
            ),
        }
    }
}

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
        self.open_file(path, O_RDONLY_CLOEXEC, 0)
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
        let flags = match (links, self.no_symlinks) {
            (SymbolicLinks::Follow, false) => 0,
            _ => AT_SYMLINK_NOFOLLOW,
        };
        match directory {
            Some(descriptor) => status_at(descriptor, path, flags),
            None => self.at(path, |folder, leaf| status_at(folder, leaf, flags)),
        }
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
        self.open_file(path, flags, mode)
    }

    fn append(&mut self, path: &CStr, mode: u32) -> Result<usize, Errno> {
        let flags = O_RDONLY_CLOEXEC | O_WRONLY | O_CREAT | O_APPEND;
        self.open_file(path, flags, mode)
    }

    fn make_directory(&mut self, path: &CStr, mode: u32) -> Result<(), Errno> {
        self.at(path, |folder, leaf| {
            let leaf = leaf.as_ptr() as usize;
            call(number::MKDIRAT, [folder, leaf, mode as usize, 0, 0, 0]).map(|_| ())
        })
    }

    fn remove(&mut self, path: &CStr, directory: bool) -> Result<(), Errno> {
        let flags = if directory { AT_REMOVEDIR } else { 0 };
        self.at(path, |folder, leaf| {
            let leaf = leaf.as_ptr() as usize;
            call(number::UNLINKAT, [folder, leaf, flags, 0, 0, 0]).map(|_| ())
        })
    }

    fn rename(&mut self, from: &CStr, to: &CStr) -> Result<(), Errno> {
        self.at(from, |from_folder, from_leaf| {
            self.at(to, |to_folder, to_leaf| {
                call(
                    number::RENAMEAT2,
                    [
                        from_folder,
                        from_leaf.as_ptr() as usize,
                        to_folder,
                        to_leaf.as_ptr() as usize,
                        0,
                        0,
                    ],
                )
                .map(|_| ())
            })
        })
    }

    fn set_modified_now(&mut self, path: &CStr) -> Result<(), Errno> {
        let no_symlinks = self.no_symlinks;
        self.at(path, |folder, leaf| {
            if no_symlinks && status_at(folder, leaf, AT_SYMLINK_NOFOLLOW)?.is_link() {
                return Err(Errno::ELOOP);
            }
            let leaf = leaf.as_ptr() as usize;
            call(number::UTIMENSAT, [folder, leaf, 0, 0, 0, 0]).map(|_| ())
        })
    }

    fn file_system_usage(&mut self, path: &CStr) -> Result<FileSystemUsage, Errno> {
        let mut record = [0u8; STATFS_SIZE];
        let buffer = record.as_mut_ptr() as usize;
        if self.no_symlinks {
            let descriptor = open_refusing_links(AT_FDCWD, path, O_PATH | O_RDONLY_CLOEXEC, 0)?;
            let result = call(number::FSTATFS, [descriptor, buffer, 0, 0, 0, 0]);
            self.close(descriptor);
            result?;
        } else {
            let path = path.as_ptr() as usize;
            call(number::STATFS, [path, buffer, 0, 0, 0, 0])?;
        }
        Ok(FileSystemUsage::from_statfs(&record))
    }

    fn set_mode(&mut self, path: &CStr, mode: u32) -> Result<(), Errno> {
        let mode = (mode & 0o7777) as usize;
        if !self.no_symlinks {
            let path = path.as_ptr() as usize;
            return call(number::FCHMODAT, [AT_FDCWD, path, mode, 0, 0, 0]).map(|_| ());
        }
        self.at(path, |folder, leaf| {
            if status_at(folder, leaf, AT_SYMLINK_NOFOLLOW)?.is_link() {
                return Err(Errno::ELOOP);
            }
            let leaf = leaf.as_ptr() as usize;
            let changed = call(
                number::FCHMODAT2,
                [folder, leaf, mode, AT_SYMLINK_NOFOLLOW, 0, 0],
            );
            match changed {
                Err(Errno::EOPNOTSUPP) => Err(Errno::ELOOP),
                Err(ENOSYS) => call(number::FCHMODAT, [folder, leaf, mode, 0, 0, 0]).map(|_| ()),
                result => result.map(|_| ()),
            }
        })
    }

    fn read_link(&mut self, path: &CStr, target: &mut [u8]) -> Result<usize, Errno> {
        self.at(path, |folder, leaf| {
            call(
                number::READLINKAT,
                [
                    folder,
                    leaf.as_ptr() as usize,
                    target.as_mut_ptr() as usize,
                    target.len(),
                    0,
                    0,
                ],
            )
        })
    }

    fn make_link(&mut self, target: &CStr, path: &CStr) -> Result<(), Errno> {
        self.at(path, |folder, leaf| {
            let (target, leaf) = (target.as_ptr() as usize, leaf.as_ptr() as usize);
            call(number::SYMLINKAT, [target, folder, leaf, 0, 0, 0]).map(|_| ())
        })
    }

    fn make_hard_link(&mut self, existing: &CStr, path: &CStr) -> Result<(), Errno> {
        self.at(existing, |from_folder, from_leaf| {
            self.at(path, |to_folder, to_leaf| {
                call(
                    number::LINKAT,
                    [
                        from_folder,
                        from_leaf.as_ptr() as usize,
                        to_folder,
                        to_leaf.as_ptr() as usize,
                        0,
                        0,
                    ],
                )
                .map(|_| ())
            })
        })
    }

    fn working_directory(&mut self, out: &mut [u8]) -> Result<usize, Errno> {
        let length = call(
            number::GETCWD,
            [out.as_mut_ptr() as usize, out.len(), 0, 0, 0, 0],
        )?;
        Ok(length.saturating_sub(1))
    }

    fn now(&mut self) -> u64 {
        self.now_with_nanoseconds().seconds
    }

    fn now_with_nanoseconds(&mut self) -> Duration {
        let mut time = Timespec {
            seconds: 0,
            nanoseconds: 0,
        };
        match call(
            number::CLOCK_GETTIME,
            [CLOCK_REALTIME, &raw mut time as usize, 0, 0, 0, 0],
        ) {
            Ok(_) if time.seconds >= 0 => Duration {
                seconds: time.seconds as u64,
                nanoseconds: time.nanoseconds as u32,
            },
            _ => Duration {
                seconds: 0,
                nanoseconds: 0,
            },
        }
    }

    fn set_time(&mut self, since_epoch: Duration) -> Result<(), Errno> {
        let time = Timespec {
            seconds: i64::try_from(since_epoch.seconds).map_err(|_| Errno::EINVAL)?,
            nanoseconds: i64::from(since_epoch.nanoseconds),
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

    fn window_tree(
        &mut self,
        descriptor: usize,
        window: u32,
        tree: &mut [u8],
    ) -> Result<WindowTree, Errno> {
        let mut request = [0u8; crate::wm::GET_TREE_SIZE];
        request[..4].copy_from_slice(&window.to_ne_bytes());
        request[16..24].copy_from_slice(&(tree.as_mut_ptr() as u64).to_ne_bytes());
        request[24..32].copy_from_slice(&(tree.len() as u64).to_ne_bytes());
        let pointer = request.as_mut_ptr() as usize;
        call(
            number::IOCTL,
            [descriptor, crate::wm::GET_TREE as usize, pointer, 0, 0, 0],
        )?;
        let word = |at: usize| {
            u32::from_ne_bytes([
                request[at],
                request[at + 1],
                request[at + 2],
                request[at + 3],
            ])
        };
        Ok(WindowTree {
            pixels_only: word(4) != 0,
            length: (word(8) as usize).min(tree.len()),
        })
    }

    fn open_datagram_socket(&mut self, kind: DatagramKind) -> Result<usize, Errno> {
        let protocol = match kind {
            DatagramKind::Udp => 0,
            DatagramKind::Ping => IPPROTO_ICMP,
        };
        call(
            number::SOCKET,
            [usize::from(AF_INET), SOCK_DGRAM_CLOEXEC, protocol, 0, 0, 0],
        )
    }

    fn send_to(
        &mut self,
        descriptor: usize,
        bytes: &[u8],
        address: Ipv4Address,
        port: u16,
    ) -> Result<usize, Errno> {
        let to = sockaddr_in(address, port);
        call(
            number::SENDTO,
            [
                descriptor,
                bytes.as_ptr() as usize,
                bytes.len(),
                0,
                to.as_ptr() as usize,
                SOCKADDR_IN_SIZE,
            ],
        )
    }

    fn receive_from(
        &mut self,
        descriptor: usize,
        buffer: &mut [u8],
        timeout_ms: u32,
    ) -> Result<Option<(usize, Sender)>, Errno> {
        if !wait_readable(descriptor, timeout_ms)? {
            return Ok(None);
        }
        let mut from = [0u8; SOCKADDR_IN_SIZE];
        let mut length = SOCKADDR_IN_SIZE as u32;
        let received = call(
            number::RECVFROM,
            [
                descriptor,
                buffer.as_mut_ptr() as usize,
                buffer.len(),
                MSG_DONTWAIT,
                from.as_mut_ptr() as usize,
                &raw mut length as usize,
            ],
        );
        match received {
            Ok(count) => Ok(Some((
                count,
                Sender {
                    address: Ipv4Address([from[4], from[5], from[6], from[7]]),
                    port: u16::from_be_bytes([from[2], from[3]]),
                },
            ))),
            Err(Errno::EAGAIN) => Ok(None),
            Err(errno) => Err(errno),
        }
    }

    fn monotonic_us(&mut self) -> u64 {
        let mut time = Timespec {
            seconds: 0,
            nanoseconds: 0,
        };
        match call(
            number::CLOCK_GETTIME,
            [CLOCK_MONOTONIC, &raw mut time as usize, 0, 0, 0, 0],
        ) {
            Ok(_) => {
                (time.seconds.max(0) as u64) * 1_000_000 + (time.nanoseconds.max(0) as u64) / 1000
            }
            Err(_) => 0,
        }
    }

    fn open_stream_socket(&mut self) -> Result<usize, Errno> {
        call(
            number::SOCKET,
            [usize::from(AF_INET), SOCK_STREAM_CLOEXEC, 0, 0, 0, 0],
        )
    }

    fn connect(
        &mut self,
        descriptor: usize,
        address: Ipv4Address,
        port: u16,
        timeout_ms: u32,
    ) -> Result<(), Errno> {
        let timeout = Timeval {
            seconds: i64::from(timeout_ms / 1000),
            microseconds: i64::from(timeout_ms % 1000) * 1000,
        };
        call(
            number::SETSOCKOPT,
            [
                descriptor,
                SOL_SOCKET,
                SO_SNDTIMEO,
                &raw const timeout as usize,
                core::mem::size_of::<Timeval>(),
                0,
            ],
        )?;
        let to = sockaddr_in(address, port);
        let connected = call(
            number::CONNECT,
            [descriptor, to.as_ptr() as usize, SOCKADDR_IN_SIZE, 0, 0, 0],
        );
        match connected {
            Ok(_) => Ok(()),
            Err(Errno::EINPROGRESS) => Err(Errno::ETIMEDOUT),
            Err(errno) => Err(errno),
        }
    }

    fn send(&mut self, descriptor: usize, mut bytes: &[u8]) -> Result<(), Errno> {
        while !bytes.is_empty() {
            let pointer = bytes.as_ptr() as usize;
            let sent = call(
                number::SENDTO,
                [descriptor, pointer, bytes.len(), MSG_NOSIGNAL, 0, 0],
            );
            match sent {
                Ok(0) => return Err(EIO),
                Ok(count) => bytes = &bytes[count.min(bytes.len())..],
                Err(Errno::EINTR) => {}
                Err(errno) => return Err(errno),
            }
        }
        Ok(())
    }

    fn receive(
        &mut self,
        descriptor: usize,
        buffer: &mut [u8],
        timeout_ms: u32,
    ) -> Result<Option<usize>, Errno> {
        if !wait_readable(descriptor, timeout_ms)? {
            return Ok(None);
        }
        let received = call(
            number::RECVFROM,
            [
                descriptor,
                buffer.as_mut_ptr() as usize,
                buffer.len(),
                MSG_DONTWAIT,
                0,
                0,
            ],
        );
        match received {
            Ok(count) => Ok(Some(count)),
            Err(Errno::EAGAIN) => Ok(None),
            Err(errno) => Err(errno),
        }
    }

    fn random(&mut self, out: &mut [u8]) -> Result<(), Errno> {
        let mut filled = 0;
        while filled < out.len() {
            let rest = &mut out[filled..];
            match call(
                number::GETRANDOM,
                [rest.as_mut_ptr() as usize, rest.len(), 0, 0, 0, 0],
            ) {
                Ok(count) => filled += count,
                Err(Errno::EINTR) => {}
                Err(errno) => return Err(errno),
            }
        }
        Ok(())
    }

    fn signal_init(&mut self, signal: u32) -> Result<(), Errno> {
        let default_action = [0usize; 4];
        call(
            number::RT_SIGACTION,
            [
                SIGTERM,
                default_action.as_ptr() as usize,
                0,
                SIGSET_BYTES,
                0,
                0,
            ],
        )?;
        let terminate = 1u64 << (SIGTERM - 1);
        call(
            number::RT_SIGPROCMASK,
            [
                SIG_UNBLOCK,
                &raw const terminate as usize,
                0,
                SIGSET_BYTES,
                0,
                0,
            ],
        )?;
        call(number::KILL, [INIT, signal as usize, 0, 0, 0, 0]).map(|_| ())
    }

    fn terminal_columns(&mut self, descriptor: usize) -> Option<u16> {
        let mut size = [0u16; 4];
        call(
            number::IOCTL,
            [descriptor, TIOCGWINSZ, size.as_mut_ptr() as usize, 0, 0, 0],
        )
        .ok()?;
        Some(size[1])
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
    let system = Linux {
        no_symlinks: refuses_symbolic_links(startup.environment()),
    };
    utilities::run(system, &arguments[..count])
}
