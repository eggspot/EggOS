//! The descriptor table's calls through musl, as a C program makes them (D101): `dup`, `dup2`
//! (`dup3` on AArch64 and RISC-V), `dup3`, `fcntl`, `close`, `close_range`, and a descriptor
//! closed or duplicated by one thread seen by another, which shares the table.

const EBADF: i32 = 9;
const EINVAL: i32 = 22;
const F_DUPFD_CLOEXEC: i32 = 1030;
const F_GETFD: i32 = 1;
const F_SETFD: i32 = 2;
const F_GETFL: i32 = 3;
const F_SETFL: i32 = 4;
const FD_CLOEXEC: i32 = 1;
const O_RDWR: i32 = 2;
const O_ACCMODE: i32 = 3;
const O_APPEND: i32 = 0o2000;
const O_CLOEXEC: i32 = 0o2_000_000;
const SYS_CLOSE_RANGE: i64 = 436;
const CLOSE_RANGE_CLOEXEC: u32 = 1 << 2;

mod libc {
    unsafe extern "C" {
        pub fn dup(old: i32) -> i32;
        pub fn dup2(old: i32, new: i32) -> i32;
        pub fn dup3(old: i32, new: i32, flags: i32) -> i32;
        pub fn fcntl(fd: i32, command: i32, ...) -> i32;
        pub fn close(fd: i32) -> i32;
        pub fn read(fd: i32, buffer: *mut std::ffi::c_void, count: usize) -> isize;
        pub fn write(fd: i32, buffer: *const std::ffi::c_void, count: usize) -> isize;
        pub fn syscall(number: i64, ...) -> i64;
    }
}

fn error() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

fn fails_with(result: i64, errno: i32) -> bool {
    result == -1 && error() == errno
}

fn dup(old: i32) -> i32 {
    // SAFETY: `dup` touches no memory.
    unsafe { libc::dup(old) }
}

fn dup2(old: i32, new: i32) -> i32 {
    // SAFETY: `dup2` touches no memory.
    unsafe { libc::dup2(old, new) }
}

fn dup3(old: i32, new: i32, flags: i32) -> i32 {
    // SAFETY: `dup3` touches no memory.
    unsafe { libc::dup3(old, new, flags) }
}

fn fcntl(fd: i32, command: i32, argument: i32) -> i32 {
    // SAFETY: the commands used here take an `int` argument or none, and touch no memory.
    unsafe { libc::fcntl(fd, command, argument) }
}

fn close(fd: i32) -> i32 {
    // SAFETY: `close` touches no memory; only descriptors this module opened, or standard
    // input it restores, are closed.
    unsafe { libc::close(fd) }
}

fn read_one_byte(fd: i32) -> isize {
    let mut byte = 0u8;
    // SAFETY: `byte` is one writable byte.
    unsafe { libc::read(fd, (&raw mut byte).cast(), 1) }
}

fn write_nothing(fd: i32) -> isize {
    // SAFETY: a count of 0 reads no memory.
    unsafe { libc::write(fd, std::ptr::null(), 0) }
}

fn descriptor_flags(fd: i32) -> i32 {
    fcntl(fd, F_GETFD, 0)
}

fn is_closed(fd: i32) -> bool {
    fails_with(descriptor_flags(fd).into(), EBADF)
}

fn close_range(first: u32, last: u32, flags: u32) -> i64 {
    // SAFETY: `close_range` takes three `unsigned int`s and touches no memory.
    unsafe { libc::syscall(SYS_CLOSE_RANGE, first, last, flags) }
}

/// Whether every call answered as on Linux, naming on standard output each group of calls
/// that did not. Leaves the descriptors as it found them.
pub fn descriptors_behave_as_on_linux() -> bool {
    let checks: [(&str, fn() -> bool); 4] = [
        (
            "duplicates take the lowest free number",
            duplicates_take_the_lowest_free_number,
        ),
        (
            "flags are per descriptor, status flags per description",
            flags_are_per_descriptor_and_status_per_description,
        ),
        (
            "a closed standard input reads EBADF until restored",
            a_closed_standard_input_reads_ebadf_until_restored,
        ),
        ("threads share the table", threads_share_the_table),
    ];
    let mut passed = true;
    for (name, check) in checks {
        if !check() {
            println!("EGGOS-USER: the descriptor check failed: {name}");
            passed = false;
        }
    }
    passed
}

fn duplicates_take_the_lowest_free_number() -> bool {
    let first = dup(1);
    let second = dup(1);
    let closed_first = close(first) == 0;
    let again = dup(2);
    let to_high = dup3(1, 40, O_CLOEXEC);
    let same = fails_with(dup3(1, 1, 0).into(), EINVAL);
    let bad_flag = fails_with(dup3(1, 41, 1).into(), EINVAL);
    let at_least = fcntl(1, F_DUPFD_CLOEXEC, 30);
    let at_least_flags = descriptor_flags(at_least);
    let empty_write = write_nothing(at_least);
    let range_closed = close_range(first as u32, u32::MAX, 0) == 0;
    first >= 3
        && second == first + 1
        && closed_first
        && again == first
        && to_high == 40
        && same
        && bad_flag
        && at_least == 30
        && at_least_flags == FD_CLOEXEC
        && empty_write == 0
        && range_closed
        && [first, second, 30, 40].into_iter().all(is_closed)
}

fn flags_are_per_descriptor_and_status_per_description() -> bool {
    let copy = dup(2);
    let cloexec_set = fcntl(copy, F_SETFD, FD_CLOEXEC) == 0;
    let original_flags = descriptor_flags(2);
    let copy_flags = descriptor_flags(copy);
    let before = fcntl(2, F_GETFL, 0);
    let appended = fcntl(copy, F_SETFL, before | O_APPEND) == 0;
    let shared = fcntl(2, F_GETFL, 0);
    let restored = fcntl(copy, F_SETFL, before) == 0;
    let marked = close_range(copy as u32, copy as u32, CLOSE_RANGE_CLOEXEC) == 0;
    let cleared = fcntl(copy, F_SETFD, 0) == 0 && descriptor_flags(copy) == 0;
    let marked_again = close_range(copy as u32, copy as u32, CLOSE_RANGE_CLOEXEC) == 0
        && descriptor_flags(copy) == FD_CLOEXEC;
    let closed = close(copy) == 0 && fails_with(close(copy).into(), EBADF);
    cloexec_set
        && original_flags == 0
        && copy_flags == FD_CLOEXEC
        && before & O_ACCMODE == O_RDWR
        && appended
        && shared == before | O_APPEND
        && restored
        && fcntl(2, F_GETFL, 0) == before
        && marked
        && cleared
        && marked_again
        && closed
}

fn a_closed_standard_input_reads_ebadf_until_restored() -> bool {
    let saved = dup(0);
    let closed = close(0) == 0;
    let read_closed = fails_with(read_one_byte(0) as i64, EBADF);
    let reused = dup(saved);
    let back = close(0) == 0 && dup2(saved, 0) == 0;
    let saved_closed = close(saved) == 0;
    saved >= 3 && closed && read_closed && reused == 0 && back && saved_closed
}

fn threads_share_the_table() -> bool {
    let copy = dup(1);
    let seen = std::thread::spawn(move || {
        let seen = descriptor_flags(copy) == 0 && close(copy) == 0;
        (seen, dup(2))
    })
    .join();
    let Ok((seen, opened)) = seen else {
        return false;
    };
    let closed_here = descriptor_flags(opened) == 0 && close(opened) == 0;
    copy >= 3 && seen && opened == copy && closed_here && is_closed(copy)
}
