use std::ffi::{c_int, c_void};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::FileExt;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, ExitCode};

const PREFIX: &str = "LINUX-MAPPINGS";
const PROGRAM: &str = "/bin/linux-mappings";
const DATA: &str = "/tmp/linux-mappings.data";
const PAGE: usize = 4096;
const TAIL: usize = 100;
const FILE_LENGTH: usize = 2 * PAGE + TAIL;
const MAPPED_PAGES: usize = 4;
const PROT_READ: c_int = 1;
const PROT_WRITE: c_int = 2;
const MAP_SHARED: c_int = 1;
const MAP_PRIVATE: c_int = 2;
const MADV_DONTNEED: c_int = 4;
const MREMAP_MAYMOVE: c_int = 1;
const MS_SYNC: c_int = 4;
const EACCES: i32 = 13;
const ENOMEM: i32 = 12;
const SIGBUS: i32 = 7;
const PAST_END_EXIT: u8 = 3;

unsafe extern "C" {
    fn mmap(
        address: *mut c_void,
        length: usize,
        protection: c_int,
        flags: c_int,
        fd: c_int,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(address: *mut c_void, length: usize) -> c_int;
    fn mprotect(address: *mut c_void, length: usize, protection: c_int) -> c_int;
    fn madvise(address: *mut c_void, length: usize, advice: c_int) -> c_int;
    fn mremap(
        address: *mut c_void,
        old_length: usize,
        new_length: usize,
        flags: c_int,
        ...
    ) -> *mut c_void;
    fn msync(address: *mut c_void, length: usize, flags: c_int) -> c_int;
    fn fork() -> c_int;
    fn waitpid(pid: c_int, status: *mut c_int, options: c_int) -> c_int;
    fn _exit(status: c_int) -> !;
}

type Checked<T> = Result<T, String>;

fn last_errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(-1)
}

fn byte_at(offset: usize) -> u8 {
    (offset % 251) as u8 + 1
}

fn contents() -> Vec<u8> {
    (0..FILE_LENGTH).map(byte_at).collect()
}

fn map_or_errno(
    file: &File,
    length: usize,
    protection: c_int,
    flags: c_int,
    offset: usize,
) -> Result<usize, i32> {
    // SAFETY: a new mapping the kernel places, of `file`'s open descriptor.
    let address = unsafe {
        mmap(
            std::ptr::null_mut(),
            length,
            protection,
            flags,
            file.as_raw_fd(),
            offset as i64,
        )
    };
    match address as isize {
        -1 => Err(last_errno()),
        address => Ok(address as usize),
    }
}

fn map(
    file: &File,
    length: usize,
    protection: c_int,
    flags: c_int,
    offset: usize,
) -> Checked<usize> {
    map_or_errno(file, length, protection, flags, offset)
        .map_err(|errno| format!("mmap: errno {errno}"))
}

fn map_errno(file: &File, protection: c_int, flags: c_int) -> i32 {
    match map_or_errno(file, PAGE, protection, flags, 0) {
        Ok(address) => {
            unmap(address, PAGE);
            0
        }
        Err(errno) => errno,
    }
}

fn unmap(address: usize, length: usize) {
    // SAFETY: `address..address + length` is a mapping this program made and no longer uses.
    unsafe { munmap(address as *mut c_void, length) };
}

fn byte(address: usize) -> u8 {
    // SAFETY: the callers read only pages of mappings this program made that hold file bytes
    // or zeros, never past the file's end (that is the child's `past-end` check alone).
    unsafe { std::ptr::read_volatile(address as *const u8) }
}

fn bytes(address: usize, length: usize) -> Vec<u8> {
    (address..address + length).map(byte).collect()
}

pub fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let result = match args.get(1).map(String::as_str) {
        Some("past-end") => return past_end(),
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
    let expected = contents();
    let mut file = File::create(DATA).map_err(|error| format!("{DATA}: {error}"))?;
    file.write_all(&expected)
        .map_err(|error| format!("{DATA}: {error}"))?;
    drop(file);
    let file = OpenOptions::new()
        .read(true)
        .open(DATA)
        .map_err(|error| format!("{DATA}: {error}"))?;
    private_read(&file, &expected)?;
    past_the_end()?;
    private_write(&file, &expected)?;
    shared_read_only(&file, &expected)?;
    own_program()?;
    fork_reads(&file, &expected)?;
    remap_grows(&file, &expected)?;
    sync(&file)?;
    let _ = std::fs::remove_file(DATA);
    Ok(())
}

fn private_read(file: &File, expected: &[u8]) -> Checked<()> {
    let length = MAPPED_PAGES * PAGE;
    let address = map(file, length, PROT_READ, MAP_PRIVATE, 0)?;
    let read = bytes(address, FILE_LENGTH);
    let zeros = bytes(address + FILE_LENGTH, 3 * PAGE - FILE_LENGTH);
    unmap(address, length);
    if read != expected {
        return Err("a private mapping does not hold the file's bytes".into());
    }
    if zeros.iter().any(|&byte| byte != 0) {
        return Err("the last page is not zero after the file's end".into());
    }
    println!("{PREFIX} private read ok");
    Ok(())
}

fn past_end() -> ExitCode {
    let Ok(file) = File::open(DATA) else {
        return ExitCode::from(1);
    };
    let Ok(address) = map(&file, MAPPED_PAGES * PAGE, PROT_READ, MAP_PRIVATE, 0) else {
        return ExitCode::from(2);
    };
    let past = byte(address + 3 * PAGE);
    println!("{PREFIX} read {past} past the end");
    ExitCode::from(PAST_END_EXIT)
}

fn past_the_end() -> Checked<()> {
    let status = Command::new(PROGRAM)
        .arg("past-end")
        .status()
        .map_err(|error| format!("{PROGRAM} past-end: {error}"))?;
    if status.signal() != Some(SIGBUS) {
        return Err(format!("a read past the file's end ended with {status}"));
    }
    println!("{PREFIX} past the end SIGBUS");
    Ok(())
}

fn private_write(file: &File, expected: &[u8]) -> Checked<()> {
    let address = map(file, PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE, PAGE)?;
    // SAFETY: the first byte of a writable mapping this program just made.
    unsafe { std::ptr::write_volatile(address as *mut u8, 0) };
    let mut in_file = [0u8; 1];
    file.read_exact_at(&mut in_file, PAGE as u64)
        .map_err(|error| format!("{DATA}: {error}"))?;
    let written = byte(address);
    // SAFETY: a range this program mapped.
    let discarded = unsafe { madvise(address as *mut c_void, PAGE, MADV_DONTNEED) };
    let again = byte(address);
    unmap(address, PAGE);
    if written != 0 || in_file[0] != expected[PAGE] {
        return Err("a write to a private mapping reached the file".into());
    }
    if discarded != 0 || again != expected[PAGE] {
        return Err(format!(
            "MADV_DONTNEED ({discarded}) left {again}, not the file's {}",
            expected[PAGE]
        ));
    }
    println!("{PREFIX} private writes stay private, DONTNEED reads the file again");
    Ok(())
}

fn shared_read_only(file: &File, expected: &[u8]) -> Checked<()> {
    let address = map(file, PAGE, PROT_READ, MAP_SHARED, 0)?;
    let read = bytes(address, PAGE);
    // SAFETY: a range this program mapped.
    let protected = unsafe { mprotect(address as *mut c_void, PAGE, PROT_READ | PROT_WRITE) };
    let protect_errno = last_errno();
    unmap(address, PAGE);
    if read != expected[..PAGE] {
        return Err("a shared mapping does not hold the file's bytes".into());
    }
    if protected != -1 || protect_errno != EACCES {
        return Err(format!(
            "mprotect writable of a read-only file's shared mapping: {protected}, errno \
             {protect_errno}"
        ));
    }
    let writable = map_errno(file, PROT_READ | PROT_WRITE, MAP_SHARED);
    if writable != EACCES {
        return Err(format!(
            "a writable shared mapping of a read-only file: errno {writable}"
        ));
    }
    println!("{PREFIX} shared read-only, EACCES for writing");
    Ok(())
}

fn own_program() -> Checked<()> {
    let program = File::open(PROGRAM).map_err(|error| format!("{PROGRAM}: {error}"))?;
    let address = map(&program, PAGE, PROT_READ, MAP_PRIVATE, 0)?;
    let magic = bytes(address, 4);
    unmap(address, PAGE);
    if magic != b"\x7fELF" {
        return Err(format!("{PROGRAM} mapped starts with {magic:?}"));
    }
    println!("{PREFIX} the program's own file");
    Ok(())
}

fn fork_reads(file: &File, expected: &[u8]) -> Checked<()> {
    let length = 2 * PAGE;
    let address = map(file, length, PROT_READ, MAP_PRIVATE, 0)?;
    let first = byte(address);
    // SAFETY: the child only reads its copy of the mapping and ends with `_exit`.
    let child = unsafe { fork() };
    if child == 0 {
        let matches = bytes(address + PAGE, PAGE) == expected[PAGE..2 * PAGE];
        // SAFETY: ends the child at once, as a child of `fork` should.
        unsafe { _exit(if matches { 0 } else { 1 }) };
    }
    if child < 0 {
        unmap(address, length);
        return Err(format!("fork: errno {}", last_errno()));
    }
    let mut status = 0;
    // SAFETY: `status` is this function's own.
    let waited = unsafe { waitpid(child, &mut status, 0) };
    unmap(address, length);
    if waited != child || status != 0 || first != expected[0] {
        return Err(format!(
            "the child read its mapping with status {status:#x}"
        ));
    }
    println!("{PREFIX} fork reads the file's pages");
    Ok(())
}

fn remap_grows(file: &File, expected: &[u8]) -> Checked<()> {
    let address = map(file, PAGE, PROT_READ, MAP_PRIVATE, 0)?;
    let first = byte(address);
    // SAFETY: a mapping this program made, grown or moved by the kernel.
    let grown = unsafe { mremap(address as *mut c_void, PAGE, 2 * PAGE, MREMAP_MAYMOVE) };
    if grown as isize == -1 {
        unmap(address, PAGE);
        return Err(format!("mremap: errno {}", last_errno()));
    }
    let grown = grown as usize;
    let moved_first = byte(grown);
    let second = bytes(grown + PAGE, PAGE);
    unmap(grown, 2 * PAGE);
    if first != expected[0] || moved_first != first || second != expected[PAGE..2 * PAGE] {
        return Err("mremap did not grow the mapping into the file".into());
    }
    println!("{PREFIX} mremap grows into the file");
    Ok(())
}

fn sync(file: &File) -> Checked<()> {
    let address = map(file, PAGE, PROT_READ, MAP_PRIVATE, 0)?;
    // SAFETY: a range this program mapped.
    let synced = unsafe { msync(address as *mut c_void, PAGE, MS_SYNC) };
    unmap(address, PAGE);
    // SAFETY: the range was just unmapped: the call only checks it.
    let unmapped = unsafe { msync(address as *mut c_void, PAGE, MS_SYNC) };
    let errno = last_errno();
    if synced != 0 || unmapped != -1 || errno != ENOMEM {
        return Err(format!("msync: {synced}, then {unmapped} errno {errno}"));
    }
    println!("{PREFIX} msync 0 ENOMEM");
    Ok(())
}
