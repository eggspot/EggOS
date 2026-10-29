use std::ffi::{c_int, c_void};
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixListener, UnixStream};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

const PREFIX: &str = "LINUX-SOCKETS";
const PROGRAM: &str = "/bin/linux-sockets";
const SOCKET_PATH: &str = "/tmp/eggos-sockets.sock";
const MISSING_PATH: &str = "/tmp/eggos-sockets-missing";
const FILE_PATH: &str = "/tmp/eggos-sockets-file";
const CLOSED_PATH: &str = "/tmp/eggos-sockets-closed";
const ABSTRACT_NAME: &[u8] = b"eggos-sockets-test";
const PIPE_TEXT: &[u8] = b"through a passed pipe\n";
const LARGE: usize = 1 << 20;

const AF_UNIX: c_int = 1;
const AF_INET6: c_int = 10;
const SOCK_STREAM: c_int = 1;
const SOCK_DGRAM: c_int = 2;
const SOL_SOCKET: c_int = 1;
const SO_PEERCRED: c_int = 17;
const SCM_RIGHTS: c_int = 1;
const POLLIN: i16 = 0x1;
const POLLOUT: i16 = 0x4;
const POLLHUP: i16 = 0x10;
const POLLRDHUP: i16 = 0x2000;
const ENOENT: i32 = 2;
const EAGAIN: i32 = 11;
const EPIPE: i32 = 32;
const ESOCKTNOSUPPORT: i32 = 94;
const EAFNOSUPPORT: i32 = 97;
const EADDRINUSE: i32 = 98;
const ECONNREFUSED: i32 = 111;
const CMSG_HEADER: usize = 16;
const ENODEV: i32 = 19;
const EPERM: i32 = 1;
const PROT_READ: c_int = 1;
const PROT_WRITE: c_int = 2;
const MAP_SHARED: c_int = 1;
const MAP_PRIVATE: c_int = 2;
const MFD_CLOEXEC: u32 = 1;
const MFD_ALLOW_SEALING: u32 = 2;
const F_ADD_SEALS: c_int = 1033;
const F_SEAL_SHRINK: c_int = 2;
const MEMFD_SIZE: usize = 64 * 1024;
const HALF: usize = MEMFD_SIZE / 2;
const PAGE: usize = 4096;
const CLIENT_TEXT: &[u8] = b"written by the client";
const SERVER_TEXT: &[u8] = b"written by the server";

#[repr(C)]
struct IoVec {
    base: *mut c_void,
    length: usize,
}

#[repr(C)]
struct MessageHeader {
    name: *mut c_void,
    name_length: u32,
    buffers: *mut IoVec,
    buffer_count: c_int,
    padding: c_int,
    control: *mut c_void,
    control_length: u32,
    control_padding: c_int,
    flags: c_int,
}

#[repr(C)]
struct PollFd {
    fd: c_int,
    events: i16,
    returned: i16,
}

unsafe extern "C" {
    fn sendmsg(fd: c_int, message: *const MessageHeader, flags: c_int) -> isize;
    fn recvmsg(fd: c_int, message: *mut MessageHeader, flags: c_int) -> isize;
    fn poll(fds: *mut PollFd, count: u64, timeout: c_int) -> c_int;
    fn getsockopt(
        fd: c_int,
        level: c_int,
        name: c_int,
        value: *mut c_void,
        length: *mut u32,
    ) -> c_int;
    fn socket(domain: c_int, kind: c_int, protocol: c_int) -> c_int;
    fn memfd_create(name: *const std::ffi::c_char, flags: u32) -> c_int;
    fn posix_fallocate(fd: c_int, offset: i64, length: i64) -> c_int;
    fn fcntl(fd: c_int, command: c_int, ...) -> c_int;
    fn mmap(
        address: *mut c_void,
        length: usize,
        protection: c_int,
        flags: c_int,
        fd: c_int,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(address: *mut c_void, length: usize) -> c_int;
}

struct Mapping {
    address: *mut u8,
    length: usize,
}

impl Mapping {
    fn new(file: &File, length: usize, protection: c_int, flags: c_int) -> Result<Self, i32> {
        // SAFETY: a new mapping chosen by the kernel, of `file`'s descriptor, which is open.
        let address = unsafe {
            mmap(
                std::ptr::null_mut(),
                length,
                protection,
                flags,
                file.as_raw_fd(),
                0,
            )
        };
        if address as isize == -1 {
            return Err(errno(&std::io::Error::last_os_error()));
        }
        Ok(Self {
            address: address.cast(),
            length,
        })
    }

    fn read(&self, offset: usize, length: usize) -> Vec<u8> {
        (offset..offset + length)
            .map(|index| {
                let at = self.address.wrapping_add(index);
                // SAFETY: `at` is inside the mapping, which stays mapped while `self` lives;
                // another process may write it, so it is read as volatile.
                unsafe { at.read_volatile() }
            })
            .collect()
    }

    fn write(&self, offset: usize, bytes: &[u8]) {
        for (index, &byte) in bytes.iter().enumerate() {
            let at = self.address.wrapping_add(offset + index);
            // SAFETY: as in `read`, and the mapping is writable.
            unsafe { at.write_volatile(byte) };
        }
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: the mapping came from `mmap` with this length, and nothing uses it any more.
        unsafe { munmap(self.address.cast(), self.length) };
    }
}

type Checked<T> = Result<T, String>;

fn failed(what: &str, error: impl std::fmt::Display) -> String {
    format!("{what}: {error}")
}

fn errno(error: &std::io::Error) -> i32 {
    error.raw_os_error().unwrap_or(-1)
}

pub fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let result = match args.get(1).map(String::as_str) {
        Some("client") => client(args.get(2).map_or(SOCKET_PATH, String::as_str)),
        _ => server(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            println!("{PREFIX} failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn server() -> Checked<()> {
    errors()?;
    pair()?;
    abstract_name()?;
    conversation()
}

fn connect_error(path: &str) -> i32 {
    match UnixStream::connect(path) {
        Ok(_) => 0,
        Err(error) => errno(&error),
    }
}

fn socket_error(domain: c_int, kind: c_int) -> i32 {
    // SAFETY: `socket` takes no pointers.
    let fd = unsafe { socket(domain, kind, 0) };
    if fd >= 0 {
        // SAFETY: the call just opened `fd`, and nothing else owns it.
        drop(unsafe { OwnedFd::from_raw_fd(fd) });
        return 0;
    }
    errno(&std::io::Error::last_os_error())
}

fn errors() -> Checked<()> {
    let _ = std::fs::remove_file(CLOSED_PATH);
    let missing = connect_error(MISSING_PATH);
    std::fs::write(FILE_PATH, b"x").map_err(|error| failed("write a file", error))?;
    let not_a_socket = connect_error(FILE_PATH);
    let _ = std::fs::remove_file(FILE_PATH);
    let listener = UnixListener::bind(CLOSED_PATH).map_err(|error| failed("bind", error))?;
    drop(listener);
    let closed = connect_error(CLOSED_PATH);
    let in_use = match UnixListener::bind(CLOSED_PATH) {
        Ok(_) => 0,
        Err(error) => errno(&error),
    };
    let _ = std::fs::remove_file(CLOSED_PATH);
    let family = socket_error(AF_INET6, SOCK_STREAM);
    let datagram = socket_error(AF_UNIX, SOCK_DGRAM);
    let expected = [
        ENOENT,
        ECONNREFUSED,
        ECONNREFUSED,
        EADDRINUSE,
        EAFNOSUPPORT,
        ESOCKTNOSUPPORT,
    ];
    let got = [missing, not_a_socket, closed, in_use, family, datagram];
    if got != expected {
        return Err(format!("errors {got:?}, expected {expected:?}"));
    }
    println!(
        "{PREFIX} errors ENOENT ECONNREFUSED ECONNREFUSED EADDRINUSE EAFNOSUPPORT ESOCKTNOSUPPORT"
    );
    Ok(())
}

fn poll_one(fd: &impl AsRawFd, events: i16, milliseconds: c_int) -> Checked<(c_int, i16)> {
    let mut entry = PollFd {
        fd: fd.as_raw_fd(),
        events,
        returned: 0,
    };
    // SAFETY: `entry` is one live `struct pollfd`, and the count passed is 1.
    let ready = unsafe { poll(&mut entry, 1, milliseconds) };
    if ready < 0 {
        return Err(failed("poll", std::io::Error::last_os_error()));
    }
    Ok((ready, entry.returned))
}

fn pair() -> Checked<()> {
    let (mut first, mut second) =
        UnixStream::pair().map_err(|error| failed("socketpair", error))?;
    first
        .write_all(b"ping")
        .map_err(|error| failed("write", error))?;
    let (ready, events) = poll_one(&second, POLLIN, 1000)?;
    if ready != 1 || events & POLLIN == 0 {
        return Err(format!("poll after a write: {ready} {events:#x}"));
    }
    let mut ping = [0u8; 4];
    second
        .read_exact(&mut ping)
        .map_err(|error| failed("read", error))?;
    if &ping != b"ping" {
        return Err(format!("read {ping:?}"));
    }
    let (empty, _) = poll_one(&second, POLLIN, 0)?;
    second
        .set_nonblocking(true)
        .map_err(|error| failed("nonblocking", error))?;
    let would_block = match second.read(&mut ping) {
        Err(error) if error.kind() == ErrorKind::WouldBlock => errno(&error),
        other => return Err(format!("a nonblocking read gave {other:?}")),
    };
    drop(first);
    let (closed, events) = poll_one(&second, POLLIN | POLLOUT | POLLRDHUP, 1000)?;
    let hung_up = POLLIN | POLLHUP | POLLRDHUP | POLLOUT;
    if closed != 1 || events & hung_up != hung_up {
        return Err(format!("poll after a close: {closed} {events:#x}"));
    }
    let end = second
        .read(&mut ping)
        .map_err(|error| failed("read", error))?;
    let broken = match second.write(b"x") {
        Err(error) => errno(&error),
        Ok(written) => return Err(format!("a write after a close wrote {written}")),
    };
    if empty != 0 || would_block != EAGAIN || end != 0 || broken != EPIPE {
        return Err(format!("pair {empty} {would_block} {end} {broken}"));
    }
    println!("{PREFIX} pair ping poll timeout EAGAIN hangup eof EPIPE");
    Ok(())
}

fn abstract_name() -> Checked<()> {
    let address = SocketAddr::from_abstract_name(ABSTRACT_NAME)
        .map_err(|error| failed("abstract name", error))?;
    let listener = UnixListener::bind_addr(&address).map_err(|error| failed("bind", error))?;
    let mut client =
        UnixStream::connect_addr(&address).map_err(|error| failed("connect", error))?;
    let (mut accepted, _) = listener.accept().map_err(|error| failed("accept", error))?;
    client
        .write_all(b"abstract")
        .map_err(|error| failed("write", error))?;
    let mut read = [0u8; 8];
    accepted
        .read_exact(&mut read)
        .map_err(|error| failed("read", error))?;
    let name = accepted
        .local_addr()
        .map_err(|error| failed("getsockname", error))?;
    if &read != b"abstract" || name.as_abstract_name() != Some(ABSTRACT_NAME) {
        return Err(format!("abstract {read:?} {name:?}"));
    }
    println!("{PREFIX} abstract ok");
    Ok(())
}

fn peer_pid(stream: &UnixStream) -> Checked<u32> {
    let mut credentials = [0u32; 3];
    let mut length = std::mem::size_of_val(&credentials) as u32;
    // SAFETY: `credentials` is 12 writable bytes, and `length` says so.
    let result = unsafe {
        getsockopt(
            stream.as_raw_fd(),
            SOL_SOCKET,
            SO_PEERCRED,
            credentials.as_mut_ptr().cast(),
            &mut length,
        )
    };
    if result != 0 {
        return Err(failed("SO_PEERCRED", std::io::Error::last_os_error()));
    }
    Ok(credentials[0])
}

fn read_line(stream: &mut UnixStream) -> Checked<String> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        stream
            .read_exact(&mut byte)
            .map_err(|error| failed("read a line", error))?;
        if byte[0] == b'\n' {
            return String::from_utf8(line).map_err(|error| failed("a line", error));
        }
        line.push(byte[0]);
    }
}

fn pattern() -> Vec<u8> {
    (0..LARGE).map(|index| (index * 7 % 251) as u8).collect()
}

fn conversation() -> Checked<()> {
    let _ = std::fs::remove_file(SOCKET_PATH);
    let listener = UnixListener::bind(SOCKET_PATH).map_err(|error| failed("bind", error))?;
    let mut child = Command::new(PROGRAM)
        .arg("client")
        .arg(SOCKET_PATH)
        .spawn()
        .map_err(|error| failed("spawn the client", error))?;
    let (mut stream, _) = listener.accept().map_err(|error| failed("accept", error))?;
    let greeting = read_line(&mut stream)?;
    let pid = peer_pid(&stream)?;
    let name = stream
        .local_addr()
        .map_err(|error| failed("getsockname", error))?;
    let name = name.as_pathname().map(|path| path.display().to_string());
    if pid != child.id() || name.as_deref() != Some(SOCKET_PATH) {
        return Err(format!("accepted {pid} {name:?}, child {}", child.id()));
    }
    println!("{PREFIX} accepted peer pid ok name {SOCKET_PATH} {greeting}");

    let (quiet, _) = poll_one(&stream, POLLIN, 0)?;
    if quiet != 0 {
        return Err("the client wrote before go".into());
    }
    stream
        .write_all(b"g")
        .map_err(|error| failed("go", error))?;
    let started = Instant::now();
    let (woken, events) = poll_one(&stream, POLLIN, 5000)?;
    let waited = started.elapsed();
    let mut byte = [0u8; 1];
    stream
        .read_exact(&mut byte)
        .map_err(|error| failed("read", error))?;
    if woken != 1 || events & POLLIN == 0 || &byte != b"w" || waited < Duration::from_millis(50) {
        return Err(format!(
            "woken {woken} {events:#x} {byte:?} after {waited:?}"
        ));
    }
    println!("{PREFIX} poll woken by the client");

    let passed = receive_descriptor(&stream)?;
    let mut text = Vec::new();
    File::from(passed)
        .read_to_end(&mut text)
        .map_err(|error| failed("read the passed pipe", error))?;
    if text != PIPE_TEXT {
        return Err(format!("passed pipe {text:?}"));
    }
    println!(
        "{PREFIX} rights pipe {}",
        String::from_utf8_lossy(PIPE_TEXT).trim_end()
    );

    shared_memory(&mut stream)?;

    let mut large = vec![0u8; LARGE];
    stream
        .read_exact(&mut large)
        .map_err(|error| failed("read 1 MiB", error))?;
    if large != pattern() {
        return Err("the 1 MiB came back changed".into());
    }
    println!("{PREFIX} large {LARGE} bytes ok");

    stream
        .write_all(b"bye")
        .map_err(|error| failed("bye", error))?;
    let end = stream
        .read(&mut byte)
        .map_err(|error| failed("read", error))?;
    let status = child.wait().map_err(|error| failed("wait", error))?;
    let broken = match stream.write(b"x") {
        Err(error) => errno(&error),
        Ok(written) => return Err(format!("a write after the close wrote {written}")),
    };
    let _ = std::fs::remove_file(SOCKET_PATH);
    if end != 0 || broken != EPIPE || !status.success() {
        return Err(format!("close {end} {broken} {status}"));
    }
    println!("{PREFIX} peer closed eof EPIPE child 0");
    Ok(())
}

fn send_descriptor(stream: &UnixStream, fd: RawFd, byte: u8) -> Checked<()> {
    let mut data = [byte];
    let mut buffer = IoVec {
        base: data.as_mut_ptr().cast(),
        length: data.len(),
    };
    let mut control = [0u8; 24];
    control[..8].copy_from_slice(&((CMSG_HEADER + 4) as u64).to_ne_bytes());
    control[8..12].copy_from_slice(&SOL_SOCKET.to_ne_bytes());
    control[12..16].copy_from_slice(&SCM_RIGHTS.to_ne_bytes());
    control[16..20].copy_from_slice(&fd.to_ne_bytes());
    let header = MessageHeader {
        name: std::ptr::null_mut(),
        name_length: 0,
        buffers: &mut buffer,
        buffer_count: 1,
        padding: 0,
        control: control.as_mut_ptr().cast(),
        control_length: (CMSG_HEADER + 4) as u32,
        control_padding: 0,
        flags: 0,
    };
    // SAFETY: `header` points to `buffer`, `data` and `control`, all live for the call, with
    // their lengths.
    let sent = unsafe { sendmsg(stream.as_raw_fd(), &header, 0) };
    if sent != 1 {
        return Err(failed("sendmsg", std::io::Error::last_os_error()));
    }
    Ok(())
}

fn receive_descriptor(stream: &UnixStream) -> Checked<OwnedFd> {
    let mut data = [0u8; 1];
    let mut buffer = IoVec {
        base: data.as_mut_ptr().cast(),
        length: data.len(),
    };
    let mut control = [0u8; 24];
    let mut header = MessageHeader {
        name: std::ptr::null_mut(),
        name_length: 0,
        buffers: &mut buffer,
        buffer_count: 1,
        padding: 0,
        control: control.as_mut_ptr().cast(),
        control_length: control.len() as u32,
        control_padding: 0,
        flags: 0,
    };
    // SAFETY: `header` points to `buffer`, `data` and `control`, all live and writable for the
    // call, with their lengths.
    let received = unsafe { recvmsg(stream.as_raw_fd(), &mut header, 0) };
    if received != 1 {
        return Err(failed("recvmsg", std::io::Error::last_os_error()));
    }
    let length = u64::from_ne_bytes(control[..8].try_into().unwrap());
    let level = c_int::from_ne_bytes(control[8..12].try_into().unwrap());
    let kind = c_int::from_ne_bytes(control[12..16].try_into().unwrap());
    if length != (CMSG_HEADER + 4) as u64 || level != SOL_SOCKET || kind != SCM_RIGHTS {
        return Err(format!("control message {length} {level} {kind}"));
    }
    let fd = RawFd::from_ne_bytes(control[16..20].try_into().unwrap());
    // SAFETY: the kernel just installed `fd` for this message, and nothing else owns it.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn client(path: &str) -> Checked<()> {
    let mut stream = UnixStream::connect(path).map_err(|error| failed("connect", error))?;
    let peer = stream
        .peer_addr()
        .map_err(|error| failed("getpeername", error))?;
    let peer = peer
        .as_pathname()
        .map_or_else(|| "unnamed".to_owned(), |path| path.display().to_string());
    stream
        .write_all(format!("peer {peer}\n").as_bytes())
        .map_err(|error| failed("greeting", error))?;
    let mut go = [0u8; 1];
    stream
        .read_exact(&mut go)
        .map_err(|error| failed("go", error))?;
    std::thread::sleep(Duration::from_millis(100));
    stream
        .write_all(b"w")
        .map_err(|error| failed("wake", error))?;

    let (reader, mut writer) = std::io::pipe().map_err(|error| failed("pipe", error))?;
    send_descriptor(&stream, reader.as_raw_fd(), b'r')?;
    drop(reader);
    writer
        .write_all(PIPE_TEXT)
        .map_err(|error| failed("write the pipe", error))?;
    drop(writer);

    let memory = create_shared_memory()?;
    let mapping = Mapping::new(&memory, MEMFD_SIZE, PROT_READ | PROT_WRITE, MAP_SHARED)
        .map_err(|error| format!("client mmap {error}"))?;
    mapping.write(0, CLIENT_TEXT);
    send_descriptor(&stream, memory.as_raw_fd(), b'm')?;
    let mut turn = [0u8; 1];
    stream
        .read_exact(&mut turn)
        .map_err(|error| failed("the server's turn", error))?;
    let seen = mapping.read(HALF, SERVER_TEXT.len());
    let answer = match &turn == b"s" && seen == SERVER_TEXT {
        true => b"c",
        false => b"x",
    };
    stream
        .write_all(answer)
        .map_err(|error| failed("confirm", error))?;
    drop(mapping);
    drop(memory);

    stream
        .write_all(&pattern())
        .map_err(|error| failed("write 1 MiB", error))?;
    let mut bye = [0u8; 3];
    stream
        .read_exact(&mut bye)
        .map_err(|error| failed("bye", error))?;
    if &bye != b"bye" {
        return Err(format!("bye {bye:?}"));
    }
    Ok(())
}

fn create_shared_memory() -> Checked<File> {
    // SAFETY: the name is a NUL-terminated string that lives for the call.
    let fd = unsafe { memfd_create(c"eggos-sockets".as_ptr(), MFD_CLOEXEC | MFD_ALLOW_SEALING) };
    if fd < 0 {
        return Err(failed("memfd_create", std::io::Error::last_os_error()));
    }
    // SAFETY: the call just opened `fd`, and nothing else owns it.
    let memory = File::from(unsafe { OwnedFd::from_raw_fd(fd) });
    // SAFETY: `posix_fallocate` takes no pointers.
    let allocated = unsafe { posix_fallocate(memory.as_raw_fd(), 0, MEMFD_SIZE as i64) };
    if allocated != 0 {
        return Err(format!("posix_fallocate {allocated}"));
    }
    // SAFETY: `F_ADD_SEALS` takes an `int`, passed as one.
    let sealed = unsafe { fcntl(memory.as_raw_fd(), F_ADD_SEALS, F_SEAL_SHRINK) };
    if sealed != 0 {
        return Err(failed("F_ADD_SEALS", std::io::Error::last_os_error()));
    }
    Ok(memory)
}

fn shared_memory(stream: &mut UnixStream) -> Checked<()> {
    let memory = File::from(receive_descriptor(stream)?);
    let size = memory
        .metadata()
        .map_err(|error| failed("fstat", error))?
        .len();
    let mapping = Mapping::new(&memory, MEMFD_SIZE, PROT_READ | PROT_WRITE, MAP_SHARED)
        .map_err(|error| format!("server mmap {error}"))?;
    let seen = mapping.read(0, CLIENT_TEXT.len());
    if seen != CLIENT_TEXT {
        return Err(format!("the server saw {seen:?}"));
    }
    mapping.write(HALF, SERVER_TEXT);
    stream
        .write_all(b"s")
        .map_err(|error| failed("the server's turn", error))?;
    let mut confirmed = [0u8; 1];
    stream
        .read_exact(&mut confirmed)
        .map_err(|error| failed("confirm", error))?;
    let private = Mapping::new(&memory, PAGE, PROT_READ, MAP_PRIVATE)
        .map_err(|error| format!("private mmap {error}"))?;
    let private_seen = private.read(0, CLIENT_TEXT.len());
    let writable_private = match Mapping::new(&memory, PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE) {
        Ok(_) => 0,
        Err(error) => error,
    };
    let shrink = match memory.set_len(PAGE as u64) {
        Ok(()) => 0,
        Err(error) => errno(&error),
    };
    if &confirmed != b"c"
        || size != MEMFD_SIZE as u64
        || private_seen != CLIENT_TEXT
        || writable_private != ENODEV
        || shrink != EPERM
    {
        return Err(format!(
            "memfd {confirmed:?} {size} {private_seen:?} {writable_private} {shrink}"
        ));
    }
    println!("{PREFIX} memfd shared both ways size {MEMFD_SIZE} seal EPERM private ENODEV");
    Ok(())
}
