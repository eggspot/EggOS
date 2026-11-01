//! The memory devices (D168): `/dev/null`, `/dev/zero`, `/dev/full`, `/dev/random` and
//! `/dev/urandom`, Linux's character devices of major 1, which shells, build tools and C
//! libraries open by name.
//!
//! As on Linux: `null` reads nothing and takes every write without reading it; `zero` reads
//! zeros and takes every write, and a mapping of it is anonymous memory
//! ([`super::memory::mmap_file`]); `full` reads zeros and refuses every write with `ENOSPC`;
//! `random` and `urandom` read the kernel's generator ([`crate::random`], as `getrandom`), and
//! what is written to them is hashed into its input without being credited as entropy.
//! `random` waits until the generator is seeded (`EAGAIN` with `O_NONBLOCK`) and `urandom`
//! never waits, as since Linux 5.18. Seeking any of them leaves them at 0, reads and writes
//! ignore the offset, and they are always ready for `poll`.

use super::{Errno, MAX_RW_COUNT};
use crate::process::Process;
use crate::vfs::devfs;
use crate::{random, thread};
use eggos_hal::Platform;
use eggos_hal::memory::FRAME_SIZE;

/// One of the memory devices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryDevice {
    /// `/dev/null`.
    Null,
    /// `/dev/zero`.
    Zero,
    /// `/dev/full`.
    Full,
    /// `/dev/random`.
    Random,
    /// `/dev/urandom`.
    Urandom,
}

impl MemoryDevice {
    /// The memory device with device number `device` (major, minor), if one has it.
    pub fn of(device: (u32, u32)) -> Option<Self> {
        Some(match device {
            devfs::NULL_DEVICE => Self::Null,
            devfs::ZERO_DEVICE => Self::Zero,
            devfs::FULL_DEVICE => Self::Full,
            devfs::RANDOM_DEVICE => Self::Random,
            devfs::URANDOM_DEVICE => Self::Urandom,
            _ => return None,
        })
    }
}

const CHUNK: usize = 256;

const PREEMPTION_INTERVAL: u64 = 64 * 1024;

/// Reads `device` into the buffers `buffers` lists (base, length), `nonblocking` for a
/// description with `O_NONBLOCK`: how many bytes, at most `MAX_RW_COUNT`; fewer at a byte that
/// is not the program's writable memory, `EFAULT` if that is the first.
pub fn read<P: Platform>(
    process: &Process<P>,
    device: MemoryDevice,
    nonblocking: bool,
    buffers: impl Iterator<Item = (u64, u64)>,
) -> Result<u64, Errno> {
    match device {
        MemoryDevice::Null => Ok(0),
        MemoryDevice::Zero | MemoryDevice::Full => fill(process, buffers, |_| {}),
        MemoryDevice::Random | MemoryDevice::Urandom => {
            if device == MemoryDevice::Random && !random::is_seeded() {
                if nonblocking {
                    return Err(Errno::EAGAIN);
                }
                random::wait_until_seeded::<P>();
            }
            fill(process, buffers, random::fill::<P>)
        }
    }
}

fn fill<P: Platform>(
    process: &Process<P>,
    buffers: impl Iterator<Item = (u64, u64)>,
    mut make: impl FnMut(&mut [u8]),
) -> Result<u64, Errno> {
    let mut chunk = [0u8; CHUNK];
    let mut total = 0u64;
    let mut since_preemption = 0u64;
    for (base, length) in buffers {
        let length = length.min(MAX_RW_COUNT - total);
        let mut done = 0u64;
        while done < length {
            let address = base.wrapping_add(done);
            let piece = (length - done)
                .min(CHUNK as u64)
                .min(FRAME_SIZE - address % FRAME_SIZE) as usize;
            make(&mut chunk[..piece]);
            if process.write_user_memory(address, &chunk[..piece]).is_err() {
                return match total + done {
                    0 => Err(Errno::EFAULT),
                    copied => Ok(copied),
                };
            }
            done += piece as u64;
            since_preemption += piece as u64;
            if since_preemption >= PREEMPTION_INTERVAL {
                since_preemption = 0;
                thread::preemption_point::<P>();
                process.interrupt_here_if_sent_signals_wait();
            }
        }
        total += done;
        if total == MAX_RW_COUNT {
            break;
        }
    }
    Ok(total)
}

/// Writes the buffers `buffers` lists (base, length) to `device`: how many bytes it took, at
/// most `MAX_RW_COUNT`. `null` and `zero` take them all without reading them; `full` is
/// `ENOSPC`; `random` and `urandom` hash what they read into the generator's input, fewer at
/// a byte that is not the program's memory, `EFAULT` if that is the first.
pub fn write<P: Platform>(
    process: &Process<P>,
    device: MemoryDevice,
    buffers: impl Iterator<Item = (u64, u64)>,
) -> Result<u64, Errno> {
    match device {
        MemoryDevice::Full => Err(Errno::ENOSPC),
        MemoryDevice::Null | MemoryDevice::Zero => Ok(buffers
            .fold(0u64, |total, (_, length)| total.saturating_add(length))
            .min(MAX_RW_COUNT)),
        MemoryDevice::Random | MemoryDevice::Urandom => mix_in(process, buffers),
    }
}

fn mix_in<P: Platform>(
    process: &Process<P>,
    buffers: impl Iterator<Item = (u64, u64)>,
) -> Result<u64, Errno> {
    let mut chunk = [0u8; CHUNK];
    let mut total = 0u64;
    let mut since_preemption = 0u64;
    for (base, length) in buffers {
        let length = length.min(MAX_RW_COUNT - total);
        let mut done = 0u64;
        while done < length {
            let address = base.wrapping_add(done);
            let piece = (length - done)
                .min(CHUNK as u64)
                .min(FRAME_SIZE - address % FRAME_SIZE) as usize;
            if process
                .read_user_memory(address, &mut chunk[..piece])
                .is_err()
            {
                return match total + done {
                    0 => Err(Errno::EFAULT),
                    copied => Ok(copied),
                };
            }
            random::add_user_input(&chunk[..piece]);
            done += piece as u64;
            since_preemption += piece as u64;
            if since_preemption >= PREEMPTION_INTERVAL {
                since_preemption = 0;
                thread::preemption_point::<P>();
                process.interrupt_here_if_sent_signals_wait();
            }
        }
        total += done;
        if total == MAX_RW_COUNT {
            break;
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_memory_device_has_linux_s_number() {
        assert_eq!(MemoryDevice::of((1, 3)), Some(MemoryDevice::Null));
        assert_eq!(MemoryDevice::of((1, 5)), Some(MemoryDevice::Zero));
        assert_eq!(MemoryDevice::of((1, 7)), Some(MemoryDevice::Full));
        assert_eq!(MemoryDevice::of((1, 8)), Some(MemoryDevice::Random));
        assert_eq!(MemoryDevice::of((1, 9)), Some(MemoryDevice::Urandom));
        assert_eq!(MemoryDevice::of((1, 4)), None);
        assert_eq!(MemoryDevice::of(devfs::WINDOW_DEVICE), None);
    }
}
