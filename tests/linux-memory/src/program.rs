use std::ffi::{c_char, c_int, c_long, c_void};
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, ExitCode};

const PREFIX: &str = "LINUX-MEMORY";
const PROGRAM: &str = "/bin/linux-memory";
const ENOMEM: i32 = 12;
const ENXIO: i32 = 6;
const SIGKILL: i32 = 9;
const PROT_READ: c_int = 1;
const PROT_WRITE: c_int = 2;
const MAP_SHARED: c_int = 1;
const MAP_PRIVATE: c_int = 2;
const MAP_ANONYMOUS: c_int = 0x20;
const MFD_CLOEXEC: u32 = 1;
const PAGE: usize = 4096;
const PIECE: usize = 1 << 20;
const MOST_PIECES: usize = 1024;
const MOST_PAIRS: usize = 480;
const SOCKET_CHUNK: usize = 64 * 1024;
const WINDOW_CLIENTS: usize = 16;
const WINDOWS_PER_CLIENT: usize = 4;
const WINDOW_CREATE: c_long = 0xc020_7701;
const WINDOW_CREATE_SIZE: usize = 32;
const LARGEST_WINDOW: u32 = 8192;
const SLACK_KB: u64 = 4096;

#[cfg(target_arch = "x86_64")]
const SYS_IOCTL: c_long = 16;
#[cfg(not(target_arch = "x86_64"))]
const SYS_IOCTL: c_long = 29;

unsafe extern "C" {
    fn memfd_create(name: *const c_char, flags: u32) -> c_int;
    fn ftruncate(fd: c_int, length: i64) -> c_int;
    fn mmap(
        address: *mut c_void,
        length: usize,
        protection: c_int,
        flags: c_int,
        fd: c_int,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(address: *mut c_void, length: usize) -> c_int;
    fn syscall(number: c_long, ...) -> c_long;
}

type Checked<T> = Result<T, String>;

fn errno(error: &std::io::Error) -> i32 {
    error.raw_os_error().unwrap_or(-1)
}

fn last_errno() -> i32 {
    errno(&std::io::Error::last_os_error())
}

#[derive(Debug, Clone, Copy, Default)]
struct Memory {
    total: u64,
    free: u64,
    reserve: u64,
}

fn read_memory(buffer: &mut [u8]) -> Result<Memory, i32> {
    let mut file = File::open("/proc/meminfo").map_err(|error| errno(&error))?;
    let mut length = 0;
    loop {
        match file.read(&mut buffer[length..]) {
            Ok(0) => break,
            Ok(read) => length += read,
            Err(error) => return Err(errno(&error)),
        }
        if length == buffer.len() {
            break;
        }
    }
    let mut memory = Memory::default();
    for line in buffer[..length].split(|&byte| byte == b'\n') {
        let mut words = line
            .split(|&byte| byte == b' ')
            .filter(|word| !word.is_empty());
        let (Some(name), Some(value)) = (words.next(), words.next()) else {
            continue;
        };
        let value = value
            .iter()
            .try_fold(0u64, |sum, &digit| match digit {
                b'0'..=b'9' => Some(sum * 10 + u64::from(digit - b'0')),
                _ => None,
            })
            .unwrap_or(0);
        match name {
            b"MemTotal:" => memory.total = value,
            b"MemFree:" => memory.free = value,
            b"KernelReserve:" => memory.reserve = value,
            _ => {}
        }
    }
    Ok(memory)
}

fn memory_now() -> Checked<Memory> {
    let mut buffer = [0u8; 512];
    read_memory(&mut buffer).map_err(|errno| format!("/proc/meminfo: errno {errno}"))
}

fn memory_file(length: usize) -> Checked<File> {
    // SAFETY: a nul-terminated name; the call only reads it.
    let fd = unsafe { memfd_create(c"linux-memory".as_ptr(), MFD_CLOEXEC) };
    if fd < 0 {
        return Err(format!("memfd_create: errno {}", last_errno()));
    }
    // SAFETY: `fd` is the descriptor just made, owned by nothing else.
    let file = unsafe { File::from_raw_fd(fd) };
    // SAFETY: `file` is an open memory file.
    if unsafe { ftruncate(file.as_raw_fd(), length as i64) } != 0 {
        return Err(format!("ftruncate: errno {}", last_errno()));
    }
    Ok(file)
}

fn map_shared(file: &File, offset: usize, length: usize) -> Result<usize, i32> {
    // SAFETY: a new mapping the kernel places, of `file`'s open descriptor.
    let address = unsafe {
        mmap(
            std::ptr::null_mut(),
            length,
            PROT_READ | PROT_WRITE,
            MAP_SHARED,
            file.as_raw_fd(),
            offset as i64,
        )
    };
    match address as isize {
        -1 => Err(last_errno()),
        address => Ok(address as usize),
    }
}

fn unmap(address: usize, length: usize) {
    // SAFETY: `address..address + length` is a mapping this program made and no longer uses.
    unsafe { munmap(address as *mut c_void, length) };
}

pub fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let result = match args.get(1).map(String::as_str) {
        Some("anonymous") => use_up_anonymous_memory(),
        _ => run_checks(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            println!("{PREFIX} failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_checks() -> Checked<()> {
    let before = memory_now()?;
    if before.reserve == 0 || before.total == 0 {
        return Err(format!("no reserve in /proc/meminfo: {before:?}"));
    }
    memory_file_as_large_as_memory(before)?;
    memory_files_up_to_the_reserve()?;
    anonymous_memory()?;
    socket_buffers(before)?;
    window_buffers(before)?;
    let after = memory_now()?;
    if after.free + SLACK_KB < before.free {
        return Err(format!(
            "memory did not come back: {} KiB free before, {} after",
            before.free, after.free
        ));
    }
    println!("{PREFIX} memory back");
    Ok(())
}

fn memory_file_as_large_as_memory(memory: Memory) -> Checked<()> {
    let length = (memory.total * 1024) as usize;
    let file = memory_file(length)?;
    match map_shared(&file, 0, length) {
        Err(ENOMEM) => {
            println!("{PREFIX} memfd ENOMEM for a mapping as large as memory");
            Ok(())
        }
        Err(errno) => Err(format!("a mapping as large as memory: errno {errno}")),
        Ok(address) => {
            unmap(address, length);
            Err("a mapping as large as memory was made".into())
        }
    }
}

fn memory_files_up_to_the_reserve() -> Checked<()> {
    let mut buffer = [0u8; 512];
    read_memory(&mut buffer).map_err(|errno| format!("/proc/meminfo: errno {errno}"))?;
    let file = memory_file(PIECE * MOST_PIECES)?;
    let mut pieces = vec![usize::MAX; MOST_PIECES];
    let mut count = 0;
    let refused = loop {
        if count == MOST_PIECES {
            break None;
        }
        match map_shared(&file, count * PIECE, PIECE) {
            Ok(address) => {
                pieces[count] = address;
                count += 1;
            }
            Err(errno) => break Some((errno, read_memory(&mut buffer))),
        }
    };
    for &address in &pieces[..count] {
        unmap(address, PIECE);
    }
    drop(file);
    match refused {
        None => Err(format!("{MOST_PIECES} MiB of memory files were all mapped")),
        Some((ENOMEM, Ok(at_limit))) if at_limit.free >= at_limit.reserve => {
            println!("{PREFIX} memfd ENOMEM with the reserve still free");
            Ok(())
        }
        Some((ENOMEM, Ok(at_limit))) => Err(format!(
            "memory files took the reserve: {} KiB free of {} reserved",
            at_limit.free, at_limit.reserve
        )),
        Some((ENOMEM, Err(errno))) => Err(format!("/proc/meminfo at the limit: errno {errno}")),
        Some((errno, _)) => Err(format!("memory file piece {count}: errno {errno}")),
    }
}

fn anonymous_memory() -> Checked<()> {
    let status = Command::new(PROGRAM)
        .arg("anonymous")
        .status()
        .map_err(|error| format!("{PROGRAM} anonymous: {error}"))?;
    match status.signal() {
        Some(SIGKILL) => {
            println!("{PREFIX} anonymous killed by SIGKILL");
            Ok(())
        }
        _ => Err(format!("using up anonymous memory ended with {status}")),
    }
}

fn use_up_anonymous_memory() -> Checked<()> {
    let memory = memory_now()?;
    let length = (memory.total * 1024 * 2) as usize;
    // SAFETY: a new private anonymous mapping the kernel places.
    let address = unsafe {
        mmap(
            std::ptr::null_mut(),
            length,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    if address as isize == -1 {
        return Err(format!(
            "mapping twice memory's size: errno {}",
            last_errno()
        ));
    }
    let start = address.cast::<u8>();
    for page in 0..length / PAGE {
        // SAFETY: inside the mapping just made, readable and writable.
        unsafe { start.wrapping_add(page * PAGE).write_volatile(1) };
    }
    Err("twice memory's size was written without the program being ended".into())
}

fn socket_buffers(memory: Memory) -> Checked<()> {
    let limit = memory.total * 1024 / 8;
    let chunk = vec![0x5au8; SOCKET_CHUNK];
    let mut pairs = Vec::with_capacity(MOST_PAIRS);
    let mut written = 0u64;
    let outcome = 'filling: loop {
        if pairs.len() == MOST_PAIRS {
            break Err(format!("{MOST_PAIRS} full socket pairs and no ENOMEM"));
        }
        let (sender, receiver) =
            UnixStream::pair().map_err(|error| format!("socketpair: {error}"))?;
        sender
            .set_nonblocking(true)
            .map_err(|error| format!("O_NONBLOCK: {error}"))?;
        loop {
            match (&sender).write(&chunk) {
                Ok(count) => written += count as u64,
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) if errno(&error) == ENOMEM => break 'filling Ok(()),
                Err(error) => break 'filling Err(format!("a socket write: {error}")),
            }
        }
        pairs.push((sender, receiver));
    };
    drop(pairs);
    outcome?;
    if written > limit || written < limit / 2 {
        return Err(format!(
            "sockets took {written} bytes before ENOMEM, the cap being {limit}"
        ));
    }
    println!("{PREFIX} sockets ENOMEM within an eighth of memory");
    Ok(())
}

fn create_window(window_server: &File) -> Result<(u32, u32), i32> {
    let mut create = [0u8; WINDOW_CREATE_SIZE];
    create[8..12].copy_from_slice(&LARGEST_WINDOW.to_ne_bytes());
    create[12..16].copy_from_slice(&LARGEST_WINDOW.to_ne_bytes());
    // SAFETY: `ioctl` on an open descriptor of `/dev/window` with `CREATE`, whose argument is
    // `create`, 32 bytes the kernel reads and writes during the call.
    let result = unsafe {
        syscall(
            SYS_IOCTL,
            window_server.as_raw_fd() as c_long,
            WINDOW_CREATE,
            create.as_mut_ptr(),
        )
    };
    if result < 0 {
        return Err(last_errno());
    }
    let field = |at: usize| u32::from_ne_bytes(create[at..at + 4].try_into().unwrap_or([0; 4]));
    Ok((field(8), field(12)))
}

fn buffer_block(width: u32, height: u32) -> u64 {
    let frames = (u64::from(width) * u64::from(height) * 4)
        .div_ceil(PAGE as u64)
        .max(1);
    frames.next_power_of_two() * PAGE as u64
}

fn window_buffers(memory: Memory) -> Checked<()> {
    let limit = memory.total * 1024 / 4;
    let mut clients = Vec::with_capacity(WINDOW_CLIENTS);
    let mut pinned = 0u64;
    let refused = 'clients: loop {
        if clients.len() == WINDOW_CLIENTS {
            break false;
        }
        let window_server = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/window")
            .map_err(|error| match errno(&error) {
                ENXIO => "/dev/window: no display".to_string(),
                _ => format!("/dev/window: {error}"),
            })?;
        for _ in 0..WINDOWS_PER_CLIENT {
            match create_window(&window_server) {
                Ok((width, height)) => pinned += buffer_block(width, height),
                Err(ENOMEM) => break 'clients true,
                Err(errno) => return Err(format!("a window: errno {errno}")),
            }
        }
        clients.push(window_server);
    };
    drop(clients);
    if !refused {
        return Err(format!("{WINDOW_CLIENTS} clients' windows and no ENOMEM"));
    }
    if pinned > limit || pinned == 0 {
        return Err(format!(
            "windows took {pinned} bytes before ENOMEM, the cap being {limit}"
        ));
    }
    println!("{PREFIX} windows ENOMEM within a quarter of memory");
    Ok(())
}
