//! `getrandom` (M6, D56): the kernel's random number generator ([`crate::random`]) for
//! programs. C libraries and Rust's std call it at startup (hash-map seeds, stack-protector
//! fallbacks) and whenever a program asks for random bytes.

use super::{Errno, MAX_RW_COUNT, ends_in_user_space};
use crate::process::Process;
use crate::{random, thread};
use core::ops::ControlFlow;
use eggos_hal::Platform;
use eggos_hal::memory::FRAME_SIZE;

/// `getrandom`'s flags (`<sys/random.h>`).
pub mod flags {
    /// Return `EAGAIN` instead of waiting for the generator to be seeded.
    pub const NONBLOCK: u32 = 1;
    /// Linux's old blocking pool; since Linux 5.6 the same as no flag.
    pub const RANDOM: u32 = 2;
    /// Return bytes even before the generator is seeded (Linux 5.6).
    pub const INSECURE: u32 = 4;
    /// Every flag there is.
    pub const ALL: u32 = NONBLOCK | RANDOM | INSECURE;
}

/// Bytes between `getrandom`'s preemption points: a long call does not keep its CPU from the
/// threads waiting for it. A multiple of the generator's 64-byte chunks.
const PREEMPTION_INTERVAL: u64 = 4096;

/// What `getrandom` does with its flags, before it looks at the buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Admission {
    /// Fill the buffer now.
    Fill,
    /// Wait until the generator is seeded, then fill it.
    WaitUntilSeeded,
}

/// `getrandom`'s checks of `flags` (an `unsigned int`), given whether the generator is
/// `seeded`: see [`getrandom`].
fn admit(flags: u32, seeded: bool) -> Result<Admission, Errno> {
    let insecure_random = flags::INSECURE | flags::RANDOM;
    if flags & !flags::ALL != 0 || flags & insecure_random == insecure_random {
        Err(Errno::EINVAL)
    } else if seeded || flags & flags::INSECURE != 0 {
        Ok(Admission::Fill)
    } else if flags & flags::NONBLOCK != 0 {
        Err(Errno::EAGAIN)
    } else {
        Ok(Admission::WaitUntilSeeded)
    }
}

/// `getrandom(buffer, length, flags)`: fills `length` bytes at `buffer` with random bytes and
/// returns how many, as Linux does:
/// - `EINVAL` for an unknown flag, or `GRND_INSECURE` with `GRND_RANDOM`;
/// - before the generator is seeded, without `GRND_INSECURE`: `EAGAIN` with `GRND_NONBLOCK`,
///   otherwise the thread waits until it is (before the length is looked at, so a call for no
///   bytes tells whether it is seeded);
/// - at most `MAX_RW_COUNT` bytes; `EFAULT` if the range does not end within user space;
/// - if part of the buffer is not the program's writable memory, the bytes before that part,
///   or `EFAULT` if there are none.
///
/// `flags` is an `unsigned int`: the register's upper half is ignored.
pub fn getrandom<P: Platform>(
    process: &Process<P>,
    buffer: u64,
    length: u64,
    flags: u64,
) -> Result<u64, Errno> {
    match admit(flags as u32, random::is_seeded())? {
        Admission::Fill => {}
        Admission::WaitUntilSeeded => random::wait_until_seeded::<P>(),
    }
    let length = length.min(MAX_RW_COUNT);
    if !ends_in_user_space::<P>(buffer, length) {
        return Err(Errno::EFAULT);
    }
    let mut written = 0u64;
    // Cannot overflow `usize`: at most `MAX_RW_COUNT`.
    let _ = random::fill_with::<P>(length as usize, |chunk| {
        // A page at a time, so the count stops exactly where the memory does.
        let mut done = 0;
        while done < chunk.len() {
            // Cannot wrap: the range ends within user space.
            let address = buffer.wrapping_add(written);
            let piece = (chunk.len() - done).min((FRAME_SIZE - address % FRAME_SIZE) as usize);
            if process
                .write_user_memory(address, &chunk[done..done + piece])
                .is_err()
            {
                return ControlFlow::Break(());
            }
            done += piece;
            written += piece as u64;
        }
        if written.is_multiple_of(PREEMPTION_INTERVAL) {
            thread::preemption_point::<P>();
            process.interrupt_here_if_sent_signals_wait();
        }
        ControlFlow::Continue(())
    });
    if written == 0 && length > 0 {
        Err(Errno::EFAULT)
    } else {
        Ok(written)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flags::{INSECURE, NONBLOCK, RANDOM};

    #[test]
    fn flags_are_checked_as_linux_checks_them() {
        for seeded in [false, true] {
            assert_eq!(admit(8, seeded), Err(Errno::EINVAL));
            assert_eq!(admit(1 << 31 | NONBLOCK, seeded), Err(Errno::EINVAL));
            assert_eq!(admit(INSECURE | RANDOM, seeded), Err(Errno::EINVAL));
            assert_eq!(
                admit(INSECURE | RANDOM | NONBLOCK, seeded),
                Err(Errno::EINVAL)
            );
            assert_eq!(admit(INSECURE, seeded), Ok(Admission::Fill));
            assert_eq!(admit(INSECURE | NONBLOCK, seeded), Ok(Admission::Fill));
        }
        for flags in [0, RANDOM, NONBLOCK, RANDOM | NONBLOCK] {
            assert_eq!(admit(flags, true), Ok(Admission::Fill));
        }
        assert_eq!(admit(0, false), Ok(Admission::WaitUntilSeeded));
        assert_eq!(admit(RANDOM, false), Ok(Admission::WaitUntilSeeded));
        assert_eq!(admit(NONBLOCK, false), Err(Errno::EAGAIN));
        assert_eq!(admit(RANDOM | NONBLOCK, false), Err(Errno::EAGAIN));
    }
}
