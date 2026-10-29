use std::ffi::{c_int, c_long, c_uint, c_void};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::process::ExitCode;
use std::time::{Duration, Instant};

const PREFIX: &str = "LINUX-EVENTS";
const PAGE: usize = 4096;
const SHARED_MEMORY: &str = "/dev/shm/eggos-events";
const SHARED_FILE: &str = "/tmp/eggos-events.data";

const PROT_READ: c_int = 1;
const PROT_WRITE: c_int = 2;
const MAP_SHARED: c_int = 1;
const MAP_PRIVATE: c_int = 2;
const MAP_ANONYMOUS: c_int = 0x20;
const MADV_DONTFORK: c_int = 10;
const MADV_WIPEONFORK: c_int = 18;
const MS_SYNC: c_int = 4;
#[cfg(target_arch = "aarch64")]
const O_NOFOLLOW: c_int = 0o100_000;
#[cfg(not(target_arch = "aarch64"))]
const O_NOFOLLOW: c_int = 0o400_000;
const O_NONBLOCK: c_int = 0o4000;
const O_CLOEXEC: c_int = 0o2_000_000;
const EPOLL_CTL_ADD: c_int = 1;
const EPOLLIN: u32 = 1;
const EFD_SEMAPHORE: c_int = 1;
const CLOCK_MONOTONIC: c_int = 1;
const SIG_BLOCK: c_long = 0;
const SIGUSR1: c_int = 10;
const SIGSEGV: c_int = 11;
const EAGAIN: i32 = 11;
const SIGNALFD_RECORD: usize = 128;
const FD_SET_WORDS: usize = 16;

#[cfg(target_arch = "x86_64")]
const SYS_RT_SIGPROCMASK: c_long = 14;
#[cfg(not(target_arch = "x86_64"))]
const SYS_RT_SIGPROCMASK: c_long = 135;
#[cfg(target_arch = "x86_64")]
const SYS_SELECT: c_long = 23;
#[cfg(target_arch = "x86_64")]
const SYS_PSELECT6: c_long = 270;
#[cfg(not(target_arch = "x86_64"))]
const SYS_PSELECT6: c_long = 72;

const TIMER: u64 = 1;
const COUNTER: u64 = 2;
const SIGNALS: u64 = 3;
const PIPE: u64 = 4;

#[cfg_attr(target_arch = "x86_64", repr(C, packed))]
#[cfg_attr(not(target_arch = "x86_64"), repr(C))]
#[derive(Debug, Clone, Copy, Default)]
struct EpollEvent {
    events: u32,
    data: u64,
}

unsafe extern "C" {
    fn epoll_create1(flags: c_int) -> c_int;
    fn epoll_ctl(epoll: c_int, operation: c_int, fd: c_int, event: *mut EpollEvent) -> c_int;
    fn epoll_wait(epoll: c_int, events: *mut EpollEvent, count: c_int, timeout: c_int) -> c_int;
    fn eventfd(initial: c_uint, flags: c_int) -> c_int;
    fn timerfd_create(clock: c_int, flags: c_int) -> c_int;
    fn timerfd_settime(
        fd: c_int,
        flags: c_int,
        new_value: *const [i64; 4],
        old_value: *mut [i64; 4],
    ) -> c_int;
    fn timerfd_gettime(fd: c_int, current: *mut [i64; 4]) -> c_int;
    fn signalfd(fd: c_int, mask: *const c_void, flags: c_int) -> c_int;
    fn syscall(number: c_long, ...) -> c_long;
    fn kill(pid: c_int, signal: c_int) -> c_int;
    fn getpid() -> c_int;
    fn read(fd: c_int, buffer: *mut c_void, count: usize) -> isize;
    fn write(fd: c_int, buffer: *const c_void, count: usize) -> isize;
    fn close(fd: c_int) -> c_int;
    fn mmap(
        address: *mut c_void,
        length: usize,
        protection: c_int,
        flags: c_int,
        fd: c_int,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(address: *mut c_void, length: usize) -> c_int;
    fn madvise(address: *mut c_void, length: usize, advice: c_int) -> c_int;
    fn msync(address: *mut c_void, length: usize, flags: c_int) -> c_int;
    fn fork() -> c_int;
    fn waitpid(pid: c_int, status: *mut c_int, options: c_int) -> c_int;
    fn _exit(status: c_int) -> !;
}

type Checked<T> = Result<T, String>;

fn last_errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(-1)
}

fn checked(result: c_int, what: &str) -> Checked<c_int> {
    match result {
        -1 => Err(format!("{what}: errno {}", last_errno())),
        result => Ok(result),
    }
}

fn close_fd(fd: c_int) {
    // SAFETY: `fd` is a descriptor this program opened and no longer uses.
    unsafe { close(fd) };
}

fn read_word(fd: c_int) -> Result<u64, i32> {
    let mut word = [0u8; 8];
    // SAFETY: reads at most 8 bytes into `word`, this function's own.
    let read = unsafe { read(fd, word.as_mut_ptr().cast(), word.len()) };
    match read {
        8 => Ok(u64::from_ne_bytes(word)),
        _ => Err(last_errno()),
    }
}

fn write_word(fd: c_int, value: u64) -> Checked<()> {
    let word = value.to_ne_bytes();
    // SAFETY: writes the 8 bytes of `word`, this function's own.
    let written = unsafe { write(fd, word.as_ptr().cast(), word.len()) };
    match written {
        8 => Ok(()),
        _ => Err(format!("write of 8 bytes: errno {}", last_errno())),
    }
}

fn watch(epoll: c_int, fd: c_int, tag: u64) -> Checked<()> {
    let mut event = EpollEvent {
        events: EPOLLIN,
        data: tag,
    };
    // SAFETY: `event` is a `struct epoll_event` this function owns, read by the call.
    let added = unsafe { epoll_ctl(epoll, EPOLL_CTL_ADD, fd, &mut event) };
    checked(added, "epoll_ctl").map(|_| ())
}

fn arm(timer: c_int, interval: Duration, value: Duration) -> Checked<()> {
    let setting = [
        interval.as_secs() as i64,
        i64::from(interval.subsec_nanos()),
        value.as_secs() as i64,
        i64::from(value.subsec_nanos()),
    ];
    // SAFETY: `setting` is a `struct itimerspec` this function owns; no old value is asked for.
    let set = unsafe { timerfd_settime(timer, 0, &setting, std::ptr::null_mut()) };
    checked(set, "timerfd_settime").map(|_| ())
}

fn block_sigusr1() -> Checked<()> {
    let set = 1u64 << (SIGUSR1 - 1);
    let set = &set as *const u64;
    let null = std::ptr::null_mut::<u64>();
    let size: c_long = 8;
    // SAFETY: `rt_sigprocmask` reads the 8-byte set this function owns and writes nothing.
    let blocked = unsafe { syscall(SYS_RT_SIGPROCMASK, SIG_BLOCK, set, null, size) };
    checked(blocked as c_int, "rt_sigprocmask").map(|_| ())
}

pub fn main() -> ExitCode {
    match run_checks() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            println!("{PREFIX} failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_checks() -> Checked<()> {
    event_loop()?;
    select_on_a_pipe()?;
    fork_copies_on_write()?;
    fork_shares_anonymous_memory()?;
    posix_shared_memory()?;
    shared_file_mapping()?;
    fork_advice()?;
    Ok(())
}

#[derive(Debug, Default)]
struct Seen {
    expirations: u64,
    counted: u64,
    signal: i32,
    piped: Vec<u8>,
}

impl Seen {
    fn is_complete(&self) -> bool {
        self.expirations >= 2
            && self.counted == 3
            && self.signal == SIGUSR1
            && !self.piped.is_empty()
    }
}

fn event_loop() -> Checked<()> {
    // SAFETY: creates a descriptor; no memory is passed.
    let epoll = checked(unsafe { epoll_create1(O_CLOEXEC) }, "epoll_create1")?;
    // SAFETY: creates a descriptor; no memory is passed.
    let timer = unsafe { timerfd_create(CLOCK_MONOTONIC, O_NONBLOCK | O_CLOEXEC) };
    let timer = checked(timer, "timerfd_create")?;
    // SAFETY: creates a descriptor; no memory is passed.
    let counter = checked(unsafe { eventfd(0, O_NONBLOCK | O_CLOEXEC) }, "eventfd")?;
    block_sigusr1()?;
    let mut mask = [0u64; 16];
    mask[0] = 1 << (SIGUSR1 - 1);
    // SAFETY: `mask` is a `sigset_t` this function owns, read by the call.
    let signals = unsafe { signalfd(-1, mask.as_ptr().cast(), O_NONBLOCK | O_CLOEXEC) };
    let signals = checked(signals, "signalfd")?;
    let (mut reader, mut writer) = std::io::pipe().map_err(|error| format!("pipe: {error}"))?;
    for (fd, tag) in [
        (timer, TIMER),
        (counter, COUNTER),
        (signals, SIGNALS),
        (reader.as_raw_fd(), PIPE),
    ] {
        watch(epoll, fd, tag)?;
    }
    let tick = Duration::from_millis(20);
    arm(timer, tick, tick)?;
    write_word(counter, 3)?;
    // SAFETY: asks for the process's own id.
    let pid = unsafe { getpid() };
    // SAFETY: sends SIGUSR1, which this thread blocks, to this process.
    checked(unsafe { kill(pid, SIGUSR1) }, "kill")?;
    let waker = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        writer.write_all(b"x")
    });
    let mut seen = Seen::default();
    let started = Instant::now();
    let mut events = [EpollEvent::default(); 8];
    while !seen.is_complete() && started.elapsed() < Duration::from_secs(5) {
        // SAFETY: `events` holds 8 `struct epoll_event`s, which the call may write.
        let ready = unsafe { epoll_wait(epoll, events.as_mut_ptr(), 8, 1000) };
        let ready = checked(ready, "epoll_wait")?;
        for event in &events[..ready as usize] {
            let tag = event.data;
            match tag {
                TIMER => seen.expirations += read_word(timer).unwrap_or(0),
                COUNTER => seen.counted += read_word(counter).unwrap_or(0),
                SIGNALS => {
                    let mut record = [0u8; SIGNALFD_RECORD];
                    // SAFETY: reads one 128-byte record into `record`, this function's own.
                    let read = unsafe { read(signals, record.as_mut_ptr().cast(), record.len()) };
                    if read == SIGNALFD_RECORD as isize {
                        seen.signal = i32::from_ne_bytes(record[..4].try_into().unwrap());
                    }
                }
                _ => {
                    let mut byte = [0u8; 1];
                    if reader
                        .read(&mut byte)
                        .map_err(|error| format!("pipe: {error}"))?
                        == 1
                    {
                        seen.piped.push(byte[0]);
                    }
                }
            }
        }
    }
    let woken = waker
        .join()
        .map_err(|_| "the waking thread panicked".to_string())?;
    woken.map_err(|error| format!("the waking thread's write: {error}"))?;
    arm(timer, Duration::ZERO, Duration::ZERO)?;
    let mut setting = [0i64; 4];
    // SAFETY: `setting` is a `struct itimerspec` the call writes.
    checked(
        unsafe { timerfd_gettime(timer, &mut setting) },
        "timerfd_gettime",
    )?;
    if !seen.is_complete() || setting != [0; 4] || seen.piped != b"x" {
        return Err(format!(
            "the event loop saw {seen:?}, its timer left at {setting:?}"
        ));
    }
    // SAFETY: creates a descriptor; no memory is passed.
    let tokens = unsafe { eventfd(2, EFD_SEMAPHORE | O_NONBLOCK | O_CLOEXEC) };
    let tokens = checked(tokens, "eventfd")?;
    let taken = [read_word(tokens), read_word(tokens), read_word(tokens)];
    if taken != [Ok(1), Ok(1), Err(EAGAIN)] {
        return Err(format!("a semaphore of 2 gave {taken:?}"));
    }
    for fd in [epoll, timer, counter, signals, tokens] {
        close_fd(fd);
    }
    println!(
        "{PREFIX} epoll: timerfd twice, eventfd 3, semaphore 1 1 EAGAIN, signalfd SIGUSR1, a pipe \
         woken by a thread"
    );
    Ok(())
}

fn fd_set(fd: c_int) -> [u64; FD_SET_WORDS] {
    let mut set = [0u64; FD_SET_WORDS];
    set[fd as usize / 64] |= 1 << (fd as usize % 64);
    set
}

fn has(set: &[u64; FD_SET_WORDS], fd: c_int) -> bool {
    set[fd as usize / 64] >> (fd as usize % 64) & 1 != 0
}

fn pselect6(
    count: c_int,
    read: &mut [u64; FD_SET_WORDS],
    write: Option<&mut [u64; FD_SET_WORDS]>,
    timeout: Option<&mut [i64; 2]>,
) -> Checked<c_long> {
    let write = write.map_or(std::ptr::null_mut(), |set| set.as_mut_ptr());
    let timeout = timeout.map_or(std::ptr::null_mut(), |time| time.as_mut_ptr());
    let mask = [0u64, 8];
    let null = std::ptr::null_mut::<u64>();
    let read = read.as_mut_ptr();
    let mask = mask.as_ptr();
    // SAFETY: the sets and the timeout are this program's, written by the call; the sixth
    // argument points to a null signal set and its size.
    let ready = unsafe {
        syscall(
            SYS_PSELECT6,
            count as c_long,
            read,
            write,
            null,
            timeout,
            mask,
        )
    };
    match ready {
        -1 => Err(format!("pselect6: errno {}", last_errno())),
        ready => Ok(ready),
    }
}

#[cfg(target_arch = "x86_64")]
fn times_out(reading: c_int) -> Checked<bool> {
    let mut read = fd_set(reading);
    let mut timeout = [0i64, 10_000];
    let null = std::ptr::null_mut::<u64>();
    let set = read.as_mut_ptr();
    let time = timeout.as_mut_ptr();
    let count = reading as c_long + 1;
    // SAFETY: the read set and the timeout are this function's, written by the call.
    let ready = unsafe { syscall(SYS_SELECT, count, set, null, null, time) };
    Ok(ready == 0 && timeout == [0, 0] && !has(&read, reading))
}

#[cfg(not(target_arch = "x86_64"))]
fn times_out(reading: c_int) -> Checked<bool> {
    let mut read = fd_set(reading);
    let mut timeout = [0i64, 10_000_000];
    let ready = pselect6(reading + 1, &mut read, None, Some(&mut timeout))?;
    Ok(ready == 0 && timeout == [0, 0] && !has(&read, reading))
}

fn select_on_a_pipe() -> Checked<()> {
    let (mut reader, mut writer) = std::io::pipe().map_err(|error| format!("pipe: {error}"))?;
    let (reading, writing) = (reader.as_raw_fd(), writer.as_raw_fd());
    let count = reading.max(writing) + 1;
    if !times_out(reading)? {
        return Err("a wait for an empty pipe did not time out with zero time left".into());
    }
    let (mut read, mut write) = (fd_set(reading), fd_set(writing));
    let mut timeout = [0i64, 0];
    let ready = pselect6(count, &mut read, Some(&mut write), Some(&mut timeout))?;
    if ready != 1 || has(&read, reading) || !has(&write, writing) {
        return Err(format!("pselect6 on an empty pipe: {ready}"));
    }
    let waker = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        writer.write_all(b"y").map(|()| writer)
    });
    let mut read = fd_set(reading);
    let woken = pselect6(count, &mut read, None, None)?;
    let writer = waker
        .join()
        .map_err(|_| "the waking thread panicked".to_string())?
        .map_err(|error| format!("the waking thread's write: {error}"))?;
    let mut byte = [0u8; 1];
    reader
        .read_exact(&mut byte)
        .map_err(|error| format!("pipe: {error}"))?;
    drop(writer);
    if woken != 1 || !has(&read, reading) || byte != *b"y" {
        return Err(format!("pselect6 woken by a write: {woken}"));
    }
    println!("{PREFIX} select and pselect6 on a pipe: timeout 0, ready {ready}, woken {woken}");
    Ok(())
}

fn map_anonymous(flags: c_int) -> Checked<usize> {
    // SAFETY: a new anonymous mapping the kernel places.
    let address = unsafe {
        mmap(
            std::ptr::null_mut(),
            PAGE,
            PROT_READ | PROT_WRITE,
            flags | MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    match address as isize {
        -1 => Err(format!("mmap: errno {}", last_errno())),
        address => Ok(address as usize),
    }
}

fn unmap(address: usize, length: usize) {
    // SAFETY: `address..address + length` is a mapping this program made and no longer uses.
    unsafe { munmap(address as *mut c_void, length) };
}

fn peek(address: usize) -> u8 {
    // SAFETY: the callers read only pages of mappings this program made and still holds (the
    // `MADV_DONTFORK` child's read is the fault it checks for).
    unsafe { std::ptr::read_volatile(address as *const u8) }
}

fn poke(address: usize, value: u8) {
    // SAFETY: the callers write only to writable mappings this program made and still holds.
    unsafe { std::ptr::write_volatile(address as *mut u8, value) };
}

fn in_child(child: impl FnOnce() -> c_int) -> Checked<c_int> {
    // SAFETY: the child only touches memory and ends with `_exit`, as a child of `fork` in a
    // process with one thread may.
    let pid = unsafe { fork() };
    if pid == 0 {
        let status = child();
        // SAFETY: ends the child at once, as a child of `fork` should.
        unsafe { _exit(status) };
    }
    let pid = checked(pid, "fork")?;
    let mut status = 0;
    // SAFETY: `status` is this function's own.
    let waited = unsafe { waitpid(pid, &mut status, 0) };
    match waited == pid {
        true => Ok(status),
        false => Err(format!("waitpid: errno {}", last_errno())),
    }
}

fn fork_copies_on_write() -> Checked<()> {
    let page = map_anonymous(MAP_PRIVATE)?;
    poke(page, 1);
    let status = in_child(|| {
        let before = peek(page);
        poke(page, 2);
        let after = peek(page);
        c_int::from(!(before == 1 && after == 2))
    })?;
    let kept = peek(page);
    unmap(page, PAGE);
    if status != 0 || kept != 1 {
        return Err(format!(
            "copy on write: child status {status:#x}, parent sees {kept}"
        ));
    }
    println!("{PREFIX} fork copies on write: the child saw 1 and wrote 2, the parent kept 1");
    Ok(())
}

fn fork_shares_anonymous_memory() -> Checked<()> {
    let page = map_anonymous(MAP_SHARED)?;
    poke(page, 1);
    let status = in_child(|| {
        let before = peek(page);
        poke(page, 2);
        c_int::from(before != 1)
    })?;
    let seen = peek(page);
    unmap(page, PAGE);
    if status != 0 || seen != 2 {
        return Err(format!(
            "shared anonymous: child status {status:#x}, parent sees {seen}"
        ));
    }
    println!(
        "{PREFIX} fork keeps MAP_SHARED anonymous memory shared: the parent sees the child's 2"
    );
    Ok(())
}

fn map_file(file: &File, length: usize) -> Checked<usize> {
    // SAFETY: a new shared mapping the kernel places, of `file`'s open descriptor.
    let address = unsafe {
        mmap(
            std::ptr::null_mut(),
            length,
            PROT_READ | PROT_WRITE,
            MAP_SHARED,
            file.as_raw_fd(),
            0,
        )
    };
    match address as isize {
        -1 => Err(format!("mmap: errno {}", last_errno())),
        address => Ok(address as usize),
    }
}

fn posix_shared_memory() -> Checked<()> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(O_NOFOLLOW | O_CLOEXEC)
        .open(SHARED_MEMORY)
        .map_err(|error| format!("{SHARED_MEMORY}: {error}"))?;
    file.set_len(2 * PAGE as u64)
        .map_err(|error| format!("ftruncate {SHARED_MEMORY}: {error}"))?;
    let address = map_file(&file, 2 * PAGE)?;
    drop(file);
    let status = in_child(|| {
        poke(address + PAGE + 7, 0x5a);
        0
    })?;
    let seen = peek(address + PAGE + 7);
    unmap(address, 2 * PAGE);
    std::fs::remove_file(SHARED_MEMORY).map_err(|error| format!("unlink: {error}"))?;
    let gone = File::open(SHARED_MEMORY).is_err();
    if status != 0 || seen != 0x5a || !gone {
        return Err(format!(
            "/dev/shm: child status {status:#x}, parent sees {seen:#x}"
        ));
    }
    println!(
        "{PREFIX} /dev/shm: a child's write through MAP_SHARED is the parent's, then unlinked"
    );
    Ok(())
}

fn shared_file_mapping() -> Checked<()> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(SHARED_FILE)
        .map_err(|error| format!("{SHARED_FILE}: {error}"))?;
    file.write_all_at(&[0; PAGE], 0)
        .map_err(|error| format!("{SHARED_FILE}: {error}"))?;
    let address = map_file(&file, PAGE)?;
    for (offset, &byte) in b"shared".iter().enumerate() {
        poke(address + 100 + offset, byte);
    }
    // SAFETY: a range this program mapped.
    let synced = unsafe { msync(address as *mut c_void, PAGE, MS_SYNC) };
    let mut in_file = [0u8; 6];
    let read = file.read_exact_at(&mut in_file, 100);
    let written = file.write_all_at(b"file", 200);
    let through_mapping: Vec<u8> = (200..204).map(|offset| peek(address + offset)).collect();
    unmap(address, PAGE);
    drop(file);
    let _ = std::fs::remove_file(SHARED_FILE);
    if synced != 0 || read.is_err() || in_file != *b"shared" {
        return Err(format!("msync {synced}, then the file held {in_file:?}"));
    }
    if written.is_err() || through_mapping != b"file" {
        return Err(format!(
            "a write to the file showed {through_mapping:?} in the mapping"
        ));
    }
    println!(
        "{PREFIX} MAP_SHARED of a /tmp file: msync then pread sees it, a write is seen through it"
    );
    Ok(())
}

fn advise(address: usize, advice: c_int) -> Checked<()> {
    // SAFETY: a range this program mapped.
    let advised = unsafe { madvise(address as *mut c_void, PAGE, advice) };
    checked(advised, "madvise").map(|_| ())
}

fn fork_advice() -> Checked<()> {
    let kept_from_children = map_anonymous(MAP_PRIVATE)?;
    poke(kept_from_children, 7);
    advise(kept_from_children, MADV_DONTFORK)?;
    let touched = in_child(|| c_int::from(peek(kept_from_children)) + 100)?;
    let wiped = map_anonymous(MAP_PRIVATE)?;
    poke(wiped, 7);
    advise(wiped, MADV_WIPEONFORK)?;
    let read_zero = in_child(|| c_int::from(peek(wiped) != 0))?;
    let kept = [peek(kept_from_children), peek(wiped)];
    unmap(kept_from_children, PAGE);
    unmap(wiped, PAGE);
    if touched & 0x7f != SIGSEGV {
        return Err(format!(
            "a child touching MADV_DONTFORK memory ended with {touched:#x}"
        ));
    }
    if read_zero != 0 || kept != [7, 7] {
        return Err(format!(
            "MADV_WIPEONFORK: child status {read_zero:#x}, parent sees {kept:?}"
        ));
    }
    println!(
        "{PREFIX} MADV_DONTFORK: the child gets SIGSEGV; MADV_WIPEONFORK: the child reads zeros"
    );
    Ok(())
}
