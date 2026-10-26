//! Process creation (D104), through musl and `clone` as C programs make it: a child with a
//! copy of the memory changes its stack, heap and a static while the parent keeps its own; a
//! child from musl's `fork`; descriptors copied, and shared with `CLONE_FILES`; and a child of
//! `vfork` running on the parent's memory until it execs this program's file, which checks
//! its parent and that it starts with a new program's floating-point state, not the rounding
//! mode its parent set before the `vfork`; and a child that forks before its first FP/SIMD
//! instruction, whose own child must still see the rounding mode their ancestor set. Each child
//! ends with its own status, which the program checks as it reaps it with `waitpid`.

use std::ffi::{CStr, CString, c_char, c_void};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};

pub const PROCESSES: &str = "processes";

pub const SPAWNED_BY_VFORK: &str = "spawned-by-vfork";

pub mod child_status {
    pub const MEMORY_COPIED: u8 = 21;
    pub const FORKED: u8 = 22;
    pub const DESCRIPTORS_COPIED: u8 = 24;
    pub const DESCRIPTORS_SHARED: u8 = 25;
    pub const SPAWNED_BY_VFORK: u8 = 26;
    pub const ROUNDING_KEPT_TWO_GENERATIONS: u8 = 27;
    pub const WRONG: u8 = 1;
}

const PROGRAM: &CStr = c"/usr/libexec/eggos-std-selftest";
const SPAWNED_ARGUMENT: &CStr = c"spawned-by-vfork";
const HOSTNAME: &[u8] = b"eggos\n";

const CLONE_FILES: i64 = 0x400;
const CLONE_VFORK: i64 = 0x4000;
const SIGCHLD: i64 = 17;
#[cfg(target_arch = "x86_64")]
const SYS_CLONE: i64 = 56;
#[cfg(not(target_arch = "x86_64"))]
const SYS_CLONE: i64 = 220;

const O_RDONLY: i32 = 0;
const F_GETFD: i32 = 1;
const EBADF: i32 = 9;

const BEFORE: u64 = 0x1111_2222_3333_4444;
const CHANGED: u64 = 0x5555_6666_7777_8888;
const YIELDS_BEFORE_EXEC: u32 = 20;

const FE_TONEAREST: i32 = 0;
#[cfg(target_arch = "x86_64")]
const FE_TOWARDZERO: i32 = 0xc00;
#[cfg(target_arch = "aarch64")]
const FE_TOWARDZERO: i32 = 0xc0_0000;
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
const FE_TOWARDZERO: i32 = 1;

static IN_A_STATIC: AtomicU64 = AtomicU64::new(BEFORE);
static VFORK_CHILD_EXECS: AtomicU64 = AtomicU64::new(0);
static GRANDCHILD_ROUNDING: AtomicU64 = AtomicU64::new(0);

mod libc {
    use std::ffi::{c_char, c_void};

    unsafe extern "C" {
        pub fn fork() -> i32;
        pub fn vfork() -> i32;
        pub fn getpid() -> i32;
        pub fn getppid() -> i32;
        pub fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
        pub fn sched_yield() -> i32;
        pub fn _exit(status: i32) -> !;
        pub fn execve(
            path: *const c_char,
            arguments: *const *const c_char,
            environment: *const *const c_char,
        ) -> i32;
        pub fn open(path: *const c_char, flags: i32, ...) -> i32;
        pub fn read(fd: i32, buffer: *mut c_void, count: usize) -> isize;
        pub fn close(fd: i32) -> i32;
        pub fn fcntl(fd: i32, command: i32, ...) -> i32;
        pub fn syscall(number: i64, ...) -> i64;
        pub fn fesetround(mode: i32) -> i32;
        pub fn fegetround() -> i32;
    }
}

fn getpid() -> i32 {
    // SAFETY: `getpid` touches no memory.
    unsafe { libc::getpid() }
}

fn getppid() -> i32 {
    // SAFETY: `getppid` touches no memory.
    unsafe { libc::getppid() }
}

fn reaped_with(child: i64, status: u8) -> bool {
    let mut raw = -1;
    // SAFETY: `raw` is an `int` that lives across the call.
    let waited = unsafe { libc::waitpid(child as i32, &mut raw, 0) };
    i64::from(waited) == child && raw & 0x7f == 0 && (raw >> 8) & 0xff == i32::from(status)
}

fn exit_child(status: u8) -> ! {
    // SAFETY: `_exit` ends the process at once, running nothing of the parent's state.
    unsafe { libc::_exit(status.into()) }
}

fn is_open(fd: i32) -> bool {
    // SAFETY: `F_GETFD` takes no argument and touches no memory.
    unsafe { libc::fcntl(fd, F_GETFD) != -1 }
}

fn open_hostname() -> i32 {
    // SAFETY: the path is a string; `open` keeps no pointer to it.
    unsafe { libc::open(c"/etc/hostname".as_ptr(), O_RDONLY) }
}

fn close(fd: i32) {
    // SAFETY: `close` touches no memory.
    unsafe { libc::close(fd) };
}

fn reads_hostname(fd: i32) -> bool {
    let mut contents = [0u8; 16];
    // SAFETY: `contents` is writable for its whole length.
    let read = unsafe { libc::read(fd, contents.as_mut_ptr().cast::<c_void>(), contents.len()) };
    usize::try_from(read).is_ok_and(|read| contents.get(..read) == Some(HOSTNAME))
}

fn clone_and_wait(flags: i64) -> i64 {
    // SAFETY: `CLONE_VFORK` without `CLONE_VM`: the child returns 0 here on a copy of the
    // memory, this stack among it, and this thread sleeps until the child has ended; the
    // child only touches its own copy and ends with `_exit`.
    unsafe {
        libc::syscall(
            SYS_CLONE,
            flags | CLONE_VFORK | SIGCHLD,
            0i64,
            0i64,
            0i64,
            0i64,
        )
    }
}

fn write_word(word: *mut u64, value: u64) {
    // SAFETY: the callers pass a live, aligned word of this process's own memory: in a child,
    // its own copy of the parent's.
    unsafe { word.write_volatile(value) };
}

fn read_word(word: *const u64) -> u64 {
    // SAFETY: as in `write_word`.
    unsafe { word.read_volatile() }
}

fn memory_is_copied() -> bool {
    let parent = getpid();
    let mut stack_word = BEFORE;
    let on_the_stack = &raw mut stack_word;
    let on_the_heap = Box::into_raw(Box::new(BEFORE));
    let child = clone_and_wait(0);
    if child == 0 {
        write_word(on_the_stack, CHANGED);
        write_word(on_the_heap, CHANGED);
        IN_A_STATIC.store(CHANGED, Ordering::SeqCst);
        let own = read_word(on_the_stack) == CHANGED
            && read_word(on_the_heap) == CHANGED
            && IN_A_STATIC.load(Ordering::SeqCst) == CHANGED;
        exit_child(match own && getppid() == parent && getpid() != parent {
            true => child_status::MEMORY_COPIED,
            false => child_status::WRONG,
        });
    }
    let kept = read_word(on_the_stack) == BEFORE && read_word(on_the_heap) == BEFORE;
    // SAFETY: from `Box::into_raw` above, freed once, here, in the parent.
    drop(unsafe { Box::from_raw(on_the_heap) });
    child > 0
        && kept
        && IN_A_STATIC.load(Ordering::SeqCst) == BEFORE
        && reaped_with(child, child_status::MEMORY_COPIED)
}

fn fork_through_musl() -> bool {
    let parent = getpid();
    // SAFETY: the child calls only async-signal-safe functions before `_exit`.
    let child = unsafe { libc::fork() };
    if child == 0 {
        exit_child(match getppid() == parent && getpid() != parent {
            true => child_status::FORKED,
            false => child_status::WRONG,
        });
    }
    child > 0 && child != parent && reaped_with(child.into(), child_status::FORKED)
}

fn descriptors_are_copied_or_shared() -> bool {
    let copied = open_hostname();
    let copy = clone_and_wait(0);
    if copy == 0 {
        let inherited = reads_hostname(copied);
        close(copied);
        exit_child(match inherited && !is_open(copied) {
            true => child_status::DESCRIPTORS_COPIED,
            false => child_status::WRONG,
        });
    }
    let kept_open = is_open(copied);
    close(copied);
    let shared = open_hostname();
    let sharer = clone_and_wait(CLONE_FILES);
    if sharer == 0 {
        close(shared);
        exit_child(child_status::DESCRIPTORS_SHARED);
    }
    let closed_by_child =
        !is_open(shared) && std::io::Error::last_os_error().raw_os_error() == Some(EBADF);
    copied >= 0
        && copy > 0
        && kept_open
        && shared >= 0
        && sharer > 0
        && closed_by_child
        && reaped_with(copy, child_status::DESCRIPTORS_COPIED)
        && reaped_with(sharer, child_status::DESCRIPTORS_SHARED)
}

#[inline(never)]
fn exec_from_vfork_child(arguments: &[*const c_char; 4]) -> ! {
    for _ in 0..YIELDS_BEFORE_EXEC {
        // SAFETY: `sched_yield` touches no memory.
        unsafe { libc::sched_yield() };
    }
    VFORK_CHILD_EXECS.store(1, Ordering::SeqCst);
    let environment = [std::ptr::null::<c_char>()];
    // SAFETY: both lists end with a null pointer to strings that outlive the call.
    unsafe { libc::execve(PROGRAM.as_ptr(), arguments.as_ptr(), environment.as_ptr()) };
    exit_child(child_status::WRONG)
}

fn set_rounding(mode: i32) -> bool {
    // SAFETY: `fesetround` changes only this thread's floating-point control state.
    unsafe { libc::fesetround(mode) == 0 }
}

fn rounding() -> i32 {
    // SAFETY: `fegetround` only reads this thread's floating-point control state.
    unsafe { libc::fegetround() }
}

fn vfork_child_runs_on_the_memory_until_it_execs() -> bool {
    let Ok(parent) = CString::new(getpid().to_string()) else {
        return false;
    };
    let arguments = [
        PROGRAM.as_ptr(),
        SPAWNED_ARGUMENT.as_ptr(),
        parent.as_ptr(),
        std::ptr::null(),
    ];
    VFORK_CHILD_EXECS.store(0, Ordering::SeqCst);
    let rounding_changed = set_rounding(FE_TOWARDZERO);
    // SAFETY: the child runs on this thread's stack and memory until it execs or ends, as
    // vfork(2) allows: it only calls a function that yields, writes a static and execs.
    let child = unsafe { libc::vfork() };
    if child == 0 {
        exec_from_vfork_child(&arguments);
    }
    let rounding_restored = set_rounding(FE_TONEAREST);
    rounding_changed
        && rounding_restored
        && child > 0
        && VFORK_CHILD_EXECS.load(Ordering::SeqCst) == 1
        && reaped_with(child.into(), child_status::SPAWNED_BY_VFORK)
}

fn grandchild_keeps_the_rounding_mode() -> ! {
    GRANDCHILD_ROUNDING.store(rounding() as u64, Ordering::SeqCst);
    exit_child(0)
}

fn child_forks_before_its_first_fp_use() -> ! {
    GRANDCHILD_ROUNDING.store(u64::MAX, Ordering::SeqCst);
    // SAFETY: the grandchild runs on this memory until it ends, as vfork(2) allows: it only
    // reads its rounding mode, writes a static and ends with `_exit`.
    let grandchild = unsafe { libc::vfork() };
    if grandchild == 0 {
        grandchild_keeps_the_rounding_mode();
    }
    let kept = GRANDCHILD_ROUNDING.load(Ordering::SeqCst) == FE_TOWARDZERO as u64;
    exit_child(match grandchild > 0 && kept {
        true => child_status::ROUNDING_KEPT_TWO_GENERATIONS,
        false => child_status::WRONG,
    })
}

fn the_rounding_mode_reaches_a_grandchild() -> bool {
    let changed = set_rounding(FE_TOWARDZERO);
    let child = clone_and_wait(0);
    if child == 0 {
        child_forks_before_its_first_fp_use();
    }
    let restored = set_rounding(FE_TONEAREST);
    changed
        && restored
        && child > 0
        && reaped_with(child, child_status::ROUNDING_KEPT_TWO_GENERATIONS)
}

type Check = (&'static str, fn() -> bool);

const CHECKS: [Check; 5] = [
    (
        "a child changes its copy of the stack, heap and statics, not the parent's",
        memory_is_copied,
    ),
    ("musl's fork starts a child", fork_through_musl),
    (
        "a child gets a copy of the descriptors, or shares them with CLONE_FILES",
        descriptors_are_copied_or_shared,
    ),
    (
        "a vfork child runs on the parent's memory and the parent resumes once it execs",
        vfork_child_runs_on_the_memory_until_it_execs,
    ),
    (
        "a child that forks before it uses FP/SIMD hands its rounding mode to its own child",
        the_rounding_mode_reaches_a_grandchild,
    ),
];

pub fn create_processes() -> ExitCode {
    let mut passed = true;
    for (name, check) in CHECKS {
        if !check() {
            println!("EGGOS-USER: processes: {name}: FAILED");
            passed = false;
        }
    }
    if !passed {
        return ExitCode::FAILURE;
    }
    println!("EGGOS-USER: a std program created processes with fork, vfork and clone");
    ExitCode::SUCCESS
}

pub fn spawned_by_vfork() -> ExitCode {
    let parent = std::env::args()
        .nth(2)
        .and_then(|text| text.parse::<i32>().ok());
    let new_program_state = rounding() == FE_TONEAREST;
    let status = match parent == Some(getppid()) && getpid() != getppid() && new_program_state {
        true => child_status::SPAWNED_BY_VFORK,
        false => child_status::WRONG,
    };
    ExitCode::from(status)
}
