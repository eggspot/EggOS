//! EggOS's vDSO (D561): the clock calls programs make most, answered in user mode from the
//! counter the CPU lets programs read and the clock data the kernel keeps in the page just before
//! this image (`__eggos_vvar`), so `clock_gettime`, `gettimeofday` and `time` cost no system
//! call. Built as a small position-independent shared object per architecture by `cargo xtask`
//! and mapped into every program by `execve` (`AT_SYSINFO_EHDR`). A clock it does not answer,
//! data the kernel is changing (an odd sequence) or a counter programs may not read falls back to
//! the system call.

#![no_std]

/// The kernel's clock data, as `kernel/src/linux/vdso.rs` writes it.
#[repr(C)]
pub struct ClockData {
    /// Odd while the kernel changes the data.
    pub sequence: u32,
    /// Whether the counter can be read in user mode.
    pub counter_readable: u32,
    /// Nanoseconds per counter tick, as a 32.32 fixed-point number.
    pub nanoseconds_per_tick: u64,
    /// What the wall clock adds to the monotonic clock, in nanoseconds.
    pub realtime_offset: i64,
}

#[cfg(not(test))]
unsafe extern "C" {
    static __eggos_vvar: ClockData;
}

const CLOCK_REALTIME: i32 = 0;
const CLOCK_MONOTONIC: i32 = 1;
const CLOCK_MONOTONIC_RAW: i32 = 4;
const CLOCK_REALTIME_COARSE: i32 = 5;
const CLOCK_MONOTONIC_COARSE: i32 = 6;
const CLOCK_BOOTTIME: i32 = 7;
const NANOSECONDS_PER_SECOND: u64 = 1_000_000_000;

#[repr(C)]
pub struct Timespec {
    seconds: i64,
    nanoseconds: i64,
}

#[repr(C)]
pub struct Timeval {
    seconds: i64,
    microseconds: i64,
}

#[cfg(target_arch = "x86_64")]
mod arch {
    pub const CLOCK_GETTIME: usize = 228;
    pub const GETTIMEOFDAY: usize = 96;
    pub const TIME: usize = 201;
    pub const CLOCK_GETRES: usize = 229;
    pub const GETCPU: usize = 309;

    #[inline(always)]
    pub fn counter() -> u64 {
        let (low, high): (u32, u32);
        // SAFETY: `lfence; rdtsc` reads the time-stamp counter, which the kernel lets user mode
        // read (`CR4.TSD` clear); it touches no memory.
        unsafe {
            core::arch::asm!("lfence", "rdtsc", out("eax") low, out("edx") high, options(nomem, nostack));
        }
        u64::from(high) << 32 | u64::from(low)
    }

    #[inline(always)]
    pub fn syscall(number: usize, first: usize, second: usize, third: usize) -> isize {
        let result: isize;
        // SAFETY: a Linux system call by its x86_64 convention; the kernel reads and writes only
        // the memory the arguments name, which the caller passed.
        unsafe {
            core::arch::asm!(
                "syscall",
                inlateout("rax") number as isize => result,
                in("rdi") first,
                in("rsi") second,
                in("rdx") third,
                lateout("rcx") _,
                lateout("r11") _,
                options(nostack),
            );
        }
        result
    }
}

#[cfg(target_arch = "aarch64")]
mod arch {
    pub const CLOCK_GETTIME: usize = 113;
    pub const GETTIMEOFDAY: usize = 169;
    pub const TIME: usize = usize::MAX;
    pub const CLOCK_GETRES: usize = 114;
    pub const GETCPU: usize = 168;

    #[inline(always)]
    pub fn counter() -> u64 {
        let value: u64;
        // SAFETY: `CNTVCT_EL0` is readable at EL0 (`CNTKCTL_EL1.EL0VCTEN`); `isb` keeps the read
        // from happening early.
        unsafe { core::arch::asm!("isb", "mrs {}, cntvct_el0", out(reg) value, options(nomem, nostack)) };
        value
    }

    #[inline(always)]
    pub fn syscall(number: usize, first: usize, second: usize, third: usize) -> isize {
        let result: isize;
        // SAFETY: a Linux system call by its AArch64 convention.
        unsafe {
            core::arch::asm!(
                "svc 0",
                in("x8") number,
                inlateout("x0") first as isize => result,
                in("x1") second,
                in("x2") third,
                options(nostack),
            );
        }
        result
    }
}

#[cfg(target_arch = "riscv64")]
mod arch {
    pub const CLOCK_GETTIME: usize = 113;
    pub const GETTIMEOFDAY: usize = 169;
    pub const TIME: usize = usize::MAX;
    pub const CLOCK_GETRES: usize = 114;
    pub const GETCPU: usize = 168;

    #[inline(always)]
    pub fn counter() -> u64 {
        let value: u64;
        // SAFETY: the `time` CSR is readable in U-mode (`scounteren.TM`).
        unsafe { core::arch::asm!("rdtime {}", out(reg) value, options(nomem, nostack)) };
        value
    }

    #[inline(always)]
    pub fn syscall(number: usize, first: usize, second: usize, third: usize) -> isize {
        let result: isize;
        // SAFETY: a Linux system call by its RISC-V convention.
        unsafe {
            core::arch::asm!(
                "ecall",
                in("a7") number,
                inlateout("a0") first as isize => result,
                in("a1") second,
                in("a2") third,
                options(nostack),
            );
        }
        result
    }
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "riscv64")))]
mod arch {
    pub const CLOCK_GETTIME: usize = usize::MAX;
    pub const GETTIMEOFDAY: usize = usize::MAX;
    pub const TIME: usize = usize::MAX;
    pub const CLOCK_GETRES: usize = usize::MAX;
    pub const GETCPU: usize = usize::MAX;
    pub fn counter() -> u64 {
        0
    }
    pub fn syscall(_number: usize, _first: usize, _second: usize, _third: usize) -> isize {
        -38
    }
}

#[cfg(not(test))]
fn data() -> &'static ClockData {
    // SAFETY: the kernel maps its clock data page read-only just before this image, where the
    // linker script places `__eggos_vvar`.
    unsafe { &*core::ptr::addr_of!(__eggos_vvar) }
}

/// The monotonic and realtime clocks in nanoseconds, read consistently; `None` when the counter
/// cannot be read in user mode.
#[cfg(not(test))]
fn now() -> Option<(u64, i64)> {
    let data = data();
    loop {
        // SAFETY: an aligned `u32` the kernel changes only with its own atomic stores.
        let before = unsafe { core::ptr::read_volatile(&data.sequence) };
        if before & 1 != 0 {
            core::hint::spin_loop();
            continue;
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);
        // SAFETY: as above.
        if unsafe { core::ptr::read_volatile(&data.counter_readable) } == 0 {
            return None;
        }
        // SAFETY: as above, for the data's other fields.
        let per_tick = unsafe { core::ptr::read_volatile(&data.nanoseconds_per_tick) };
        // SAFETY: as above.
        let offset = unsafe { core::ptr::read_volatile(&data.realtime_offset) };
        let ticks = arch::counter();
        core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);
        // SAFETY: as above.
        if unsafe { core::ptr::read_volatile(&data.sequence) } != before {
            continue;
        }
        let monotonic = ((u128::from(ticks) * u128::from(per_tick)) >> 32) as u64;
        return Some((monotonic, offset));
    }
}

#[cfg(not(test))]
fn clock_nanoseconds(clock: i32) -> Option<u64> {
    let (monotonic, offset) = now()?;
    match clock {
        CLOCK_REALTIME | CLOCK_REALTIME_COARSE => Some(monotonic.saturating_add_signed(offset)),
        CLOCK_MONOTONIC | CLOCK_MONOTONIC_RAW | CLOCK_MONOTONIC_COARSE | CLOCK_BOOTTIME => {
            Some(monotonic)
        }
        _ => None,
    }
}

/// `clock_gettime(clock, tp)`.
#[cfg(not(test))]
#[unsafe(no_mangle)]
pub extern "C" fn __vdso_clock_gettime(clock: i32, tp: *mut Timespec) -> i32 {
    match clock_nanoseconds(clock) {
        Some(nanoseconds) if !tp.is_null() => {
            // SAFETY: the caller passes a `struct timespec` to fill, as `clock_gettime` takes.
            unsafe {
                tp.write(Timespec {
                    seconds: (nanoseconds / NANOSECONDS_PER_SECOND) as i64,
                    nanoseconds: (nanoseconds % NANOSECONDS_PER_SECOND) as i64,
                });
            }
            0
        }
        _ => arch::syscall(arch::CLOCK_GETTIME, clock as usize, tp as usize, 0) as i32,
    }
}

/// `gettimeofday(tv, tz)`.
#[cfg(not(test))]
#[unsafe(no_mangle)]
pub extern "C" fn __vdso_gettimeofday(tv: *mut Timeval, tz: *mut u8) -> i32 {
    if !tz.is_null() {
        return arch::syscall(arch::GETTIMEOFDAY, tv as usize, tz as usize, 0) as i32;
    }
    match clock_nanoseconds(CLOCK_REALTIME) {
        Some(nanoseconds) => {
            if !tv.is_null() {
                // SAFETY: the caller passes a `struct timeval` to fill.
                unsafe {
                    tv.write(Timeval {
                        seconds: (nanoseconds / NANOSECONDS_PER_SECOND) as i64,
                        microseconds: (nanoseconds % NANOSECONDS_PER_SECOND / 1000) as i64,
                    });
                }
            }
            0
        }
        None => arch::syscall(arch::GETTIMEOFDAY, tv as usize, 0, 0) as i32,
    }
}

/// `time(t)`.
#[cfg(not(test))]
#[unsafe(no_mangle)]
pub extern "C" fn __vdso_time(t: *mut i64) -> i64 {
    let seconds = match clock_nanoseconds(CLOCK_REALTIME) {
        Some(nanoseconds) => (nanoseconds / NANOSECONDS_PER_SECOND) as i64,
        None if arch::TIME != usize::MAX => return arch::syscall(arch::TIME, t as usize, 0, 0) as i64,
        None => {
            let mut now = Timespec {
                seconds: 0,
                nanoseconds: 0,
            };
            arch::syscall(arch::CLOCK_GETTIME, CLOCK_REALTIME as usize, &raw mut now as usize, 0);
            now.seconds
        }
    };
    if !t.is_null() {
        // SAFETY: the caller passes a `time_t` to fill.
        unsafe { t.write(seconds) };
    }
    seconds
}

/// `clock_getres(clock, res)`: the system call's answer.
#[cfg(not(test))]
#[unsafe(no_mangle)]
pub extern "C" fn __vdso_clock_getres(clock: i32, res: *mut Timespec) -> i32 {
    arch::syscall(arch::CLOCK_GETRES, clock as usize, res as usize, 0) as i32
}

/// `getcpu(cpu, node, cache)`: the system call's answer.
#[cfg(not(test))]
#[unsafe(no_mangle)]
pub extern "C" fn __vdso_getcpu(cpu: *mut u32, node: *mut u32, cache: *mut u8) -> i32 {
    arch::syscall(arch::GETCPU, cpu as usize, node as usize, cache as usize) as i32
}

#[cfg(all(target_arch = "aarch64", not(test)))]
#[unsafe(no_mangle)]
pub extern "C" fn __kernel_clock_gettime(clock: i32, tp: *mut Timespec) -> i32 {
    __vdso_clock_gettime(clock, tp)
}

#[cfg(all(target_arch = "aarch64", not(test)))]
#[unsafe(no_mangle)]
pub extern "C" fn __kernel_gettimeofday(tv: *mut Timeval, tz: *mut u8) -> i32 {
    __vdso_gettimeofday(tv, tz)
}

#[cfg(all(target_arch = "aarch64", not(test)))]
#[unsafe(no_mangle)]
pub extern "C" fn __kernel_clock_getres(clock: i32, res: *mut Timespec) -> i32 {
    __vdso_clock_getres(clock, res)
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
