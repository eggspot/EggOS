use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use eggos_runtime::startup::{Startup, auxv};
use eggos_runtime::syscall::{self, number};

pub const PASSED_LINE: &[u8] =
    b"EGGOS-USER: a program built from Rust source passed its startup checks\n";

const PAGE: usize = 4096;
const STACK_LIMIT: u64 = 8 << 20;
const EAGAIN: isize = 11;
const ENOMEM: isize = 12;
const EFAULT: isize = 14;
const EINVAL: isize = 22;
const ENOSYS: isize = 38;
const ETIMEDOUT: isize = 110;

const PROT_NONE: usize = 0;
const PROT_READ: usize = 1;
const PROT_WRITE: usize = 2;
const MAP_PRIVATE: usize = 2;
const MAP_ANONYMOUS: usize = 0x20;

const SIGSEGV: usize = 11;
const SIGPIPE: usize = 13;
const SIG_IGN: usize = 1;
const SA_SIGINFO: usize = 4;
const SA_ONSTACK: usize = 0x0800_0000;
#[cfg(target_arch = "x86_64")]
const SA_RESTORER: usize = 0x0400_0000;
const SEGV_ACCERR: i32 = 2;
const SS_DISABLE: i32 = 2;
const SIGNAL_SET_SIZE: usize = 8;
const ALTERNATE_STACK_SIZE: usize = 4 * PAGE;

type Check = (&'static str, fn(&Startup) -> bool);

static CHECKS: [Check; 19] = [
    (
        "the stack holds the arguments and the environment",
        arguments,
    ),
    (
        "the auxiliary vector describes the program",
        auxiliary_vector,
    ),
    ("statics are zeroed and writable", statics),
    ("set_tid_address answers the thread's id", thread_id),
    ("set_robust_list takes Linux's list head only", robust_list),
    ("the thread pointer is kept", thread_pointer),
    ("the stack limit is 8 MiB", stack_limit),
    ("descriptors 0 to 2 are open", standard_descriptors),
    ("SIGPIPE can be ignored", ignore_sigpipe),
    (
        "a stack of AT_MINSIGSTKSZ bytes is accepted",
        smallest_alternate_stack,
    ),
    ("an alternate signal stack is taken", alternate_stack),
    (
        "a fault runs its handler on the alternate stack",
        fault_handler,
    ),
    ("getrandom fills a buffer", random),
    ("the break grows and shrinks", program_break),
    ("anonymous memory is mapped and unmapped", anonymous_memory),
    ("floating-point arithmetic is IEEE double", floating_point),
    ("the console is a terminal and a character device", console),
    ("the monotonic clock and uname answer", clock_and_uname),
    (
        "futex waits, times out and wakes with Linux's errors",
        futex_waits_and_times_out,
    ),
];

/// Runs every check in order and returns the exit status: 0 if all passed.
pub fn run(startup: &Startup) -> i32 {
    let mut failed = false;
    for (name, check) in &CHECKS {
        if !check(startup) {
            failed = true;
            syscall::write_all(1, b"EGGOS-USER: the Rust program's check failed: ");
            syscall::write_all(1, name.as_bytes());
            syscall::write_all(1, b"\n");
        }
    }
    if failed {
        return 1;
    }
    syscall::write_all(1, PASSED_LINE);
    0
}

fn call(number: usize, arguments: [usize; 6]) -> isize {
    // SAFETY: every call below passes only memory this program owns for the call's length,
    // and maps, protects and unmaps only memory it mapped for the check.
    unsafe { syscall::call(number, arguments) }
}

fn arguments(startup: &Startup) -> bool {
    startup.argument_count() == 1
        && startup.argument_end_marker().is_null()
        && startup
            .arguments()
            .next()
            .is_some_and(|program| program.to_bytes().ends_with(b"eggos-selftest"))
        && startup
            .environment()
            .any(|string| string.to_bytes() == b"TERM=linux")
}

fn auxiliary_vector(startup: &Startup) -> bool {
    unsafe extern "C" {
        fn _start();
    }
    let value = |kind| startup.auxiliary_value(kind);
    let random_bytes = value(auxv::RANDOM).map(|address| {
        // SAFETY: Linux's `AT_RANDOM` points to 16 bytes on the start-up stack.
        unsafe { (address as *const [u8; 16]).read_unaligned() }
    });
    value(auxv::PAGESZ) == Some(PAGE)
        && value(auxv::ENTRY) == Some(_start as *const () as usize)
        && value(auxv::BASE) == Some(0)
        && value(auxv::SECURE) == Some(0)
        && value(auxv::CLKTCK) == Some(100)
        && value(auxv::UID) == Some(0)
        && value(auxv::EUID) == Some(0)
        && value(auxv::GID) == Some(0)
        && value(auxv::EGID) == Some(0)
        && !startup.program_headers().is_empty()
        && random_bytes.is_some_and(|bytes| bytes != [0; 16])
}

static ZEROED: [AtomicU64; 1024] = [const { AtomicU64::new(0) }; 1024];

fn statics(_: &Startup) -> bool {
    let zeroed = ZEROED.iter().all(|word| word.load(Ordering::Relaxed) == 0);
    for (index, word) in ZEROED.iter().enumerate() {
        word.store(index as u64, Ordering::Relaxed);
    }
    zeroed
        && ZEROED
            .iter()
            .enumerate()
            .all(|(index, word)| word.load(Ordering::Relaxed) == index as u64)
}

static CLEAR_CHILD_TID: AtomicU64 = AtomicU64::new(0);

fn thread_id(_: &Startup) -> bool {
    let address = &raw const CLEAR_CHILD_TID as usize;
    let tid = call(number::SET_TID_ADDRESS, [address, 0, 0, 0, 0, 0]);
    tid > 0 && tid == call(number::GETTID, [0; 6]) && tid == call(number::GETPID, [0; 6])
}

fn robust_list(_: &Startup) -> bool {
    let head = [0usize; 3];
    let head_address = head.as_ptr() as usize;
    call(number::SET_ROBUST_LIST, [head_address, 24, 0, 0, 0, 0]) == 0
        && call(number::SET_ROBUST_LIST, [head_address, 23, 0, 0, 0, 0]) == -EINVAL
}

#[repr(C, align(64))]
struct ThreadControlBlock {
    self_pointer: usize,
    words: [usize; 7],
}

static mut THREAD_CONTROL_BLOCK: ThreadControlBlock = ThreadControlBlock {
    self_pointer: 0,
    words: [0; 7],
};

fn thread_pointer(_: &Startup) -> bool {
    let block = &raw mut THREAD_CONTROL_BLOCK;
    // SAFETY: only this check uses the block, once.
    unsafe { (*block).self_pointer = block as usize };
    if !set_thread_pointer(block as usize) {
        return false;
    }
    call(number::GETPID, [0; 6]);
    read_thread_pointer() == block as usize
}

#[cfg(target_arch = "x86_64")]
fn set_thread_pointer(address: usize) -> bool {
    const ARCH_SET_FS: usize = 0x1002;
    const ARCH_GET_FS: usize = 0x1003;
    let mut read_back = 0usize;
    call(number::ARCH_PRCTL, [ARCH_SET_FS, address, 0, 0, 0, 0]) == 0
        && call(
            number::ARCH_PRCTL,
            [ARCH_GET_FS, &raw mut read_back as usize, 0, 0, 0, 0],
        ) == 0
        && read_back == address
}

#[cfg(target_arch = "x86_64")]
fn read_thread_pointer() -> usize {
    let value: usize;
    // SAFETY: the FS base points to the thread control block, whose first word is its own
    // address, as x86_64's TLS ABI lays it out.
    unsafe {
        core::arch::asm!("mov {}, qword ptr fs:[0]", out(reg) value, options(nostack, readonly))
    };
    value
}

#[cfg(target_arch = "aarch64")]
fn set_thread_pointer(address: usize) -> bool {
    // SAFETY: `TPIDR_EL0` is the program's thread pointer; nothing here uses thread-local
    // storage through it.
    unsafe { core::arch::asm!("msr tpidr_el0, {}", in(reg) address, options(nostack)) };
    true
}

#[cfg(target_arch = "aarch64")]
fn read_thread_pointer() -> usize {
    let value: usize;
    // SAFETY: reading `TPIDR_EL0` has no side effect.
    unsafe { core::arch::asm!("mrs {}, tpidr_el0", out(reg) value, options(nostack, nomem)) };
    value
}

#[cfg(target_arch = "riscv64")]
fn set_thread_pointer(address: usize) -> bool {
    // SAFETY: `tp` is the program's thread pointer; nothing here uses thread-local storage
    // through it.
    unsafe { core::arch::asm!("mv tp, {}", in(reg) address, options(nostack, nomem)) };
    true
}

#[cfg(target_arch = "riscv64")]
fn read_thread_pointer() -> usize {
    let value: usize;
    // SAFETY: reading `tp` has no side effect.
    unsafe { core::arch::asm!("mv {}, tp", out(reg) value, options(nostack, nomem)) };
    value
}

fn stack_limit(_: &Startup) -> bool {
    const RLIMIT_STACK: usize = 3;
    let mut limit = [0u64; 2];
    call(
        number::PRLIMIT64,
        [0, RLIMIT_STACK, 0, limit.as_mut_ptr() as usize, 0, 0],
    ) == 0
        && limit[0] == STACK_LIMIT
}

#[repr(C)]
struct PollDescriptor {
    descriptor: i32,
    events: i16,
    returned_events: i16,
}

fn standard_descriptors(_: &Startup) -> bool {
    const POLLNVAL: i16 = 0x20;
    let mut descriptors = [0, 1, 2].map(|descriptor| PollDescriptor {
        descriptor,
        events: 0,
        returned_events: 0,
    });
    let timeout = [0i64; 2];
    let ready = call(
        number::PPOLL,
        [
            descriptors.as_mut_ptr() as usize,
            descriptors.len(),
            timeout.as_ptr() as usize,
            0,
            SIGNAL_SET_SIZE,
            0,
        ],
    );
    ready >= 0
        && descriptors
            .iter()
            .all(|descriptor| descriptor.returned_events & POLLNVAL == 0)
}

#[cfg(not(target_arch = "riscv64"))]
#[repr(C)]
#[derive(Default)]
struct SignalAction {
    handler: usize,
    flags: usize,
    restorer: usize,
    mask: u64,
}

#[cfg(target_arch = "riscv64")]
#[repr(C)]
#[derive(Default)]
struct SignalAction {
    handler: usize,
    flags: usize,
    mask: u64,
}

fn signal_action(signal: usize, action: Option<&SignalAction>) -> Option<SignalAction> {
    let mut old = SignalAction::default();
    let new = action.map_or(0, |action| action as *const SignalAction as usize);
    let old_address = &raw mut old as usize;
    (call(
        number::RT_SIGACTION,
        [signal, new, old_address, SIGNAL_SET_SIZE, 0, 0],
    ) == 0)
        .then_some(old)
}

fn ignore_sigpipe(_: &Startup) -> bool {
    let ignore = SignalAction {
        handler: SIG_IGN,
        ..SignalAction::default()
    };
    signal_action(SIGPIPE, Some(&ignore)).is_some_and(|old| old.handler == 0)
        && signal_action(SIGPIPE, None).is_some_and(|now| now.handler == SIG_IGN)
}

#[repr(C)]
#[derive(Default)]
struct SignalStack {
    base: usize,
    flags: i32,
    size: usize,
}

static ALTERNATE_STACK: AtomicUsize = AtomicUsize::new(0);

fn map(length: usize, protection: usize) -> Option<usize> {
    let address = call(
        number::MMAP,
        [
            0,
            length,
            protection,
            MAP_PRIVATE | MAP_ANONYMOUS,
            usize::MAX,
            0,
        ],
    );
    (address > 0 && (address as usize).is_multiple_of(PAGE)).then_some(address as usize)
}

fn smallest_alternate_stack(startup: &Startup) -> bool {
    const MINSIGSTKSZ: usize = 2048;
    let Some(size) = startup
        .auxiliary_value(auxv::MINSIGSTKSZ)
        .filter(|&size| size >= MINSIGSTKSZ)
    else {
        return false;
    };
    let Some(mapping) = map(size, PROT_READ | PROT_WRITE) else {
        return false;
    };
    let smallest = SignalStack {
        base: mapping,
        flags: 0,
        size,
    };
    let too_small = SignalStack {
        size: size - 1,
        ..smallest
    };
    let disabled = SignalStack {
        flags: SS_DISABLE,
        ..SignalStack::default()
    };
    let set = |stack: &SignalStack| {
        call(
            number::SIGALTSTACK,
            [&raw const *stack as usize, 0, 0, 0, 0, 0],
        )
    };
    let one_byte_less_refused = set(&too_small) == -ENOMEM;
    let smallest_accepted = set(&smallest) == 0;
    let disabled_again = set(&disabled) == 0;
    let unmapped = call(number::MUNMAP, [mapping, size, 0, 0, 0, 0]) == 0;
    one_byte_less_refused && smallest_accepted && disabled_again && unmapped
}

fn alternate_stack(_: &Startup) -> bool {
    let Some(mapping) = map(PAGE + ALTERNATE_STACK_SIZE, PROT_READ | PROT_WRITE) else {
        return false;
    };
    let guarded = call(number::MPROTECT, [mapping, PAGE, PROT_NONE, 0, 0, 0]) == 0;
    let stack = SignalStack {
        base: mapping + PAGE,
        flags: 0,
        size: ALTERNATE_STACK_SIZE,
    };
    let mut old = SignalStack::default();
    let mut now = SignalStack::default();
    let taken = call(
        number::SIGALTSTACK,
        [&raw const stack as usize, &raw mut old as usize, 0, 0, 0, 0],
    ) == 0
        && call(number::SIGALTSTACK, [0, &raw mut now as usize, 0, 0, 0, 0]) == 0;
    ALTERNATE_STACK.store(stack.base, Ordering::Relaxed);
    guarded && taken && old.flags == SS_DISABLE && now.base == stack.base && now.flags == 0
}

static FAULT_PAGE: AtomicUsize = AtomicUsize::new(0);
static FAULTS_HANDLED: AtomicUsize = AtomicUsize::new(0);

#[repr(C)]
struct SignalInfo {
    signal: i32,
    error: i32,
    code: i32,
    padding: i32,
    address: usize,
}

extern "C" fn on_fault(signal: i32, info: *const SignalInfo, _context: *mut u8) {
    let here = 0u8;
    let here = &raw const here as usize;
    let stack = ALTERNATE_STACK.load(Ordering::Relaxed);
    let page = FAULT_PAGE.load(Ordering::Relaxed);
    // SAFETY: with `SA_SIGINFO` the kernel passes a `siginfo_t` on the handler's frame.
    let info = unsafe { &*info };
    let expected = signal == SIGSEGV as i32
        && info.signal == SIGSEGV as i32
        && info.code == SEGV_ACCERR
        && info.address == page
        && (stack..stack + ALTERNATE_STACK_SIZE).contains(&here)
        && halve_repeatedly(0.75) == 0.75 / 1024.0;
    if expected {
        FAULTS_HANDLED.store(
            FAULTS_HANDLED.load(Ordering::Relaxed) + 1,
            Ordering::Relaxed,
        );
    }
    call(
        number::MPROTECT,
        [page, PAGE, PROT_READ | PROT_WRITE, 0, 0, 0],
    );
}

#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
extern "C" fn restore_after_handler() -> ! {
    core::arch::naked_asm!("mov eax, 15", "syscall", "ud2")
}

fn fault_handler(_: &Startup) -> bool {
    let Some(page) = map(PAGE, PROT_NONE) else {
        return false;
    };
    FAULT_PAGE.store(page, Ordering::Relaxed);
    #[cfg_attr(not(target_arch = "x86_64"), allow(unused_mut))]
    let mut action = SignalAction {
        handler: on_fault as *const () as usize,
        flags: SA_SIGINFO | SA_ONSTACK,
        ..SignalAction::default()
    };
    #[cfg(target_arch = "x86_64")]
    {
        action.flags |= SA_RESTORER;
        action.restorer = restore_after_handler as *const () as usize;
    }
    let installed = signal_action(SIGSEGV, Some(&action)).is_some_and(|old| old.handler == 0);
    if !installed {
        return false;
    }
    let kept = core::hint::black_box(2.0f64).sqrt_approximation();
    // SAFETY: `page` is this check's own page; the store faults until the handler makes it
    // writable, then completes.
    unsafe { (page as *mut u64).write_volatile(0x5eed) };
    // SAFETY: the page is mapped and writable now.
    let stored = unsafe { (page as *const u64).read_volatile() };
    stored == 0x5eed
        && FAULTS_HANDLED.load(Ordering::Relaxed) == 1
        && kept == 2.0f64.sqrt_approximation()
        && call(number::MUNMAP, [page, PAGE, 0, 0, 0, 0]) == 0
}

fn random(_: &Startup) -> bool {
    let mut bytes = [0u8; 32];
    call(
        number::GETRANDOM,
        [bytes.as_mut_ptr() as usize, bytes.len(), 0, 0, 0, 0],
    ) == bytes.len() as isize
        && bytes != [0; 32]
}

fn program_break(_: &Startup) -> bool {
    const GROWTH: usize = 16 * PAGE;
    let start = call(number::BRK, [0; 6]);
    if start <= 0 {
        return false;
    }
    let start = start as usize;
    if call(number::BRK, [start + GROWTH, 0, 0, 0, 0, 0]) != (start + GROWTH) as isize {
        return false;
    }
    let mut pages = (start..start + GROWTH).step_by(PAGE);
    for address in pages.clone() {
        // SAFETY: the break now covers these pages, which only this check uses.
        unsafe { (address as *mut usize).write_volatile(address) };
    }
    // SAFETY: as above.
    let kept = pages.all(|address| unsafe { (address as *const usize).read_volatile() } == address);
    kept && call(number::BRK, [start, 0, 0, 0, 0, 0]) == start as isize
}

fn anonymous_memory(_: &Startup) -> bool {
    const LENGTH: usize = 1 << 20;
    let Some(mapping) = map(LENGTH, PROT_READ | PROT_WRITE) else {
        return false;
    };
    // SAFETY: the mapping is this check's own, `LENGTH` writable bytes, zeroed by Linux.
    let words = unsafe { core::slice::from_raw_parts_mut(mapping as *mut u64, LENGTH / 8) };
    let zeroed = words.iter().all(|&word| word == 0);
    for (index, word) in words.iter_mut().enumerate() {
        *word = index as u64 * 0x9e37_79b9;
    }
    let kept = words
        .iter()
        .enumerate()
        .all(|(index, &word)| word == index as u64 * 0x9e37_79b9);
    zeroed && kept && call(number::MUNMAP, [mapping, LENGTH, 0, 0, 0, 0]) == 0
}

trait SquareRoot {
    fn sqrt_approximation(self) -> Self;
}

impl SquareRoot for f64 {
    fn sqrt_approximation(self) -> f64 {
        let mut root = self.max(1.0);
        for _ in 0..8 {
            root = 0.5 * (root + self / root);
        }
        root
    }
}

fn halve_repeatedly(value: f64) -> f64 {
    (0..10).fold(core::hint::black_box(value), |value, _| value / 2.0)
}

fn floating_point(_: &Startup) -> bool {
    let root = core::hint::black_box(2.0f64).sqrt_approximation();
    call(number::GETPID, [0; 6]);
    let squares: [f64; 16] = core::array::from_fn(|index| (index * index) as f64);
    let sum: f64 = core::hint::black_box(squares).iter().sum();
    (root * root - 2.0).abs() <= f64::EPSILON * 2.0
        && sum == 1240.0
        && core::hint::black_box(0.1f64) + 0.2 != 0.3
        && halve_repeatedly(3.0) == 3.0 / 1024.0
}

fn console(_: &Startup) -> bool {
    const TIOCGWINSZ: usize = 0x5413;
    const S_IFMT: u32 = 0o170_000;
    const S_IFCHR: u32 = 0o020_000;
    #[cfg(target_arch = "x86_64")]
    const MODE_OFFSET: usize = 24;
    #[cfg(not(target_arch = "x86_64"))]
    const MODE_OFFSET: usize = 16;
    let mut window = [0u16; 4];
    let mut status = [0u8; 144];
    let terminal = call(
        number::IOCTL,
        [1, TIOCGWINSZ, window.as_mut_ptr() as usize, 0, 0, 0],
    ) == 0;
    let stated = call(number::FSTAT, [1, status.as_mut_ptr() as usize, 0, 0, 0, 0]) == 0;
    let mode = u32::from_ne_bytes([
        status[MODE_OFFSET],
        status[MODE_OFFSET + 1],
        status[MODE_OFFSET + 2],
        status[MODE_OFFSET + 3],
    ]);
    terminal && stated && mode & S_IFMT == S_IFCHR
}

fn clock_and_uname(_: &Startup) -> bool {
    const CLOCK_MONOTONIC: usize = 1;
    const FIELD: usize = 65;
    let mut first = [0i64; 2];
    let mut second = [0i64; 2];
    let mut names = [0u8; 6 * FIELD];
    let read = |time: &mut [i64; 2]| {
        call(
            number::CLOCK_GETTIME,
            [CLOCK_MONOTONIC, time.as_mut_ptr() as usize, 0, 0, 0, 0],
        ) == 0
    };
    read(&mut first)
        && read(&mut second)
        && second >= first
        && (0..1_000_000_000).contains(&first[1])
        && call(number::UNAME, [names.as_mut_ptr() as usize, 0, 0, 0, 0, 0]) == 0
        && names[..FIELD].starts_with(b"Linux\0")
}

fn futex_waits_and_times_out(_: &Startup) -> bool {
    const WAIT: usize = 0;
    const WAKE: usize = 1;
    const WAIT_BITSET: usize = 9;
    const WAKE_BITSET: usize = 10;
    const PRIVATE: usize = 128;
    const CLOCK_REALTIME: usize = 256;
    const UNKNOWN: usize = 99;
    const CLOCK_MONOTONIC: usize = 1;
    const MILLISECOND: i64 = 1_000_000;
    static WORD: AtomicU32 = AtomicU32::new(7);
    let word = WORD.as_ptr() as usize;
    let futex = |address: usize, op: usize, value: usize, timeout: usize, bitset: usize| {
        call(number::FUTEX, [address, op, value, timeout, 0, bitset])
    };
    let now = || {
        let mut time = [0i64; 2];
        call(
            number::CLOCK_GETTIME,
            [CLOCK_MONOTONIC, time.as_mut_ptr() as usize, 0, 0, 0, 0],
        );
        time[0] * 1_000 * MILLISECOND + time[1]
    };
    let one_millisecond = [0i64, MILLISECOND];
    let bad_nanoseconds = [0i64, 1_000 * MILLISECOND];
    let past = [0i64, 1];
    let start = now();
    let timed_out = futex(
        word,
        WAIT | PRIVATE,
        7,
        one_millisecond.as_ptr() as usize,
        0,
    ) == -ETIMEDOUT
        && now() - start >= MILLISECOND;
    timed_out
        && futex(word, WAIT | PRIVATE, 8, 0, 0) == -EAGAIN
        && futex(word, WAIT, 8, 0, 0) == -EAGAIN
        && futex(word, WAIT_BITSET | PRIVATE, 7, past.as_ptr() as usize, 1) == -ETIMEDOUT
        && futex(
            word,
            WAIT_BITSET | PRIVATE | CLOCK_REALTIME,
            7,
            past.as_ptr() as usize,
            1,
        ) == -ETIMEDOUT
        && futex(
            word,
            WAIT | PRIVATE,
            7,
            bad_nanoseconds.as_ptr() as usize,
            0,
        ) == -EINVAL
        && futex(word + 1, WAIT | PRIVATE, 7, 0, 0) == -EINVAL
        && futex(word, WAIT_BITSET | PRIVATE, 7, 0, 0) == -EINVAL
        && futex(word, WAKE | PRIVATE, 1, 0, 0) == 0
        && futex(word, WAKE_BITSET | PRIVATE, 1, 0, 0) == -EINVAL
        && futex(word, WAKE | PRIVATE | CLOCK_REALTIME, 1, 0, 0) == -ENOSYS
        && futex(word, UNKNOWN, 0, 0, 0) == -ENOSYS
        && futex(0, WAIT | PRIVATE, 0, 0, 0) == -EFAULT
}
