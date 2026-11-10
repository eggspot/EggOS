//! The memory devices (D168): `/dev/null`, `/dev/zero`, `/dev/full`, `/dev/random`,
//! `/dev/urandom` and `/dev/kmsg`, Linux's character devices of major 1, which shells, build
//! tools and C libraries open by name.
//!
//! As on Linux: `null` reads nothing and takes every write without reading it; `zero` reads
//! zeros and takes every write, and a mapping of it is anonymous memory
//! ([`super::memory::mmap_file`]); `full` reads zeros and refuses every write with `ENOSPC`;
//! `random` and `urandom` read the kernel's generator ([`crate::random`], as `getrandom`), and
//! what is written to them is hashed into its input without being credited as entropy.
//! `random` waits until the generator is seeded (`EAGAIN` with `O_NONBLOCK`) and `urandom`
//! never waits, as since Linux 5.18. `kmsg` (D224) has one job on EggOS: what is written to it
//! goes to the serial console only, never to the screen's terminal, so init's `init:` lines
//! reach the boot log while the Terminal's first text stays the banner and the shell's; only a
//! privileged writer may write it (`EACCES` for anyone else), its control characters but the
//! newline (C0, DEL and C1, raw or UTF-8) and bytes that are not UTF-8 text are left out so no
//! writer can drive the serial terminal, and it reads nothing, since the
//! kernel keeps no log to replay. Seeking any of them leaves them at
//! 0, reads and writes ignore the offset, and they are always ready for `poll`.

use super::credentials::Credentials;
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
    /// `/dev/kmsg`: writes go to the serial console only (D224).
    Kmsg,
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
            devfs::KMSG_DEVICE => Self::Kmsg,
            _ => return None,
        })
    }
}

const CHUNK: usize = 256;
const SHOWN: usize = CHUNK + 4;

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
        MemoryDevice::Null | MemoryDevice::Kmsg => Ok(0),
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
/// `ENOSPC`; `random` and `urandom` hash what they read into the generator's input and `kmsg`
/// writes it to the serial console as UTF-8 text without its control characters but the newline
/// (`EACCES` unless the process is privileged), fewer at a byte that is not the program's
/// memory, `EFAULT` if that is the first.
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
        MemoryDevice::Random | MemoryDevice::Urandom => {
            take(process, buffers, random::add_user_input)
        }
        MemoryDevice::Kmsg => {
            may_write_kernel_log(&process.credentials_and_limits().lock().credentials)?;
            let mut shown = [0u8; SHOWN];
            let mut text = PrintableText::default();
            take(process, buffers, |bytes| {
                P::console_write(text.printable(bytes, &mut shown));
            })
        }
    }
}

fn may_write_kernel_log(credentials: &Credentials) -> Result<(), Errno> {
    match credentials.is_privileged() {
        true => Ok(()),
        false => Err(Errno::EACCES),
    }
}

#[derive(Default)]
struct PrintableText {
    started: [u8; 4],
    started_length: usize,
    still_needed: usize,
}

impl PrintableText {
    fn printable<'shown>(&mut self, bytes: &[u8], shown: &'shown mut [u8; SHOWN]) -> &'shown [u8] {
        let mut length = 0;
        for &byte in bytes.iter().take(CHUNK) {
            if self.still_needed > 0 && byte & 0xc0 == 0x80 {
                self.started[self.started_length] = byte;
                self.started_length += 1;
                self.still_needed -= 1;
                if self.still_needed == 0 {
                    let character = &self.started[..self.started_length];
                    if !is_c1_control(character) && core::str::from_utf8(character).is_ok() {
                        shown[length..length + character.len()].copy_from_slice(character);
                        length += character.len();
                    }
                    self.started_length = 0;
                }
                continue;
            }
            self.started_length = 0;
            self.still_needed = 0;
            let needed = match byte {
                b'\n' | 0x20..=0x7e => {
                    shown[length] = byte;
                    length += 1;
                    continue;
                }
                0xc2..=0xdf => 1,
                0xe0..=0xef => 2,
                0xf0..=0xf4 => 3,
                _ => continue,
            };
            self.started[0] = byte;
            self.started_length = 1;
            self.still_needed = needed;
        }
        &shown[..length]
    }
}

fn is_c1_control(character: &[u8]) -> bool {
    matches!(character, [0xc2, 0x80..=0x9f])
}

fn take<P: Platform>(
    process: &Process<P>,
    buffers: impl Iterator<Item = (u64, u64)>,
    mut consume: impl FnMut(&[u8]),
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
            consume(&chunk[..piece]);
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
        assert_eq!(MemoryDevice::of((1, 11)), Some(MemoryDevice::Kmsg));
        assert_eq!(MemoryDevice::of((1, 4)), None);
        assert_eq!(MemoryDevice::of(devfs::WINDOW_DEVICE), None);
    }

    #[test]
    fn only_a_privileged_process_writes_the_kernel_log() {
        use super::super::credentials::{Ids, Kind};
        assert_eq!(may_write_kernel_log(&Credentials::new()), Ok(()));
        let mut user = Credentials::new();
        user.set_ids(Kind::User, Ids::all(1000)).unwrap();
        assert_eq!(may_write_kernel_log(&user), Err(Errno::EACCES));
    }

    fn printable(bytes: &[u8]) -> std::vec::Vec<u8> {
        let mut shown = [0u8; SHOWN];
        PrintableText::default()
            .printable(bytes, &mut shown)
            .to_vec()
    }

    #[test]
    fn the_kernel_log_keeps_text_and_newlines_and_leaves_out_other_control_bytes() {
        assert_eq!(
            printable(b"init: \x1b[2Jstarting\r\tsh\x07\x7f\n"),
            b"init: [2Jstartingsh\n"
        );
        assert_eq!(
            printable("caf\u{e9} \u{20ac} \u{1f600}\n".as_bytes()),
            "caf\u{e9} \u{20ac} \u{1f600}\n".as_bytes()
        );
    }

    #[test]
    fn the_kernel_log_leaves_out_c1_controls_raw_or_encoded_and_bytes_that_are_not_text() {
        assert_eq!(printable(b"a\x9b2Jb\x85c\x90d\n"), b"a2Jbcd\n");
        assert_eq!(
            printable("a\u{9b}2J\u{85}b\u{a0}c\n".as_bytes()),
            "a2Jb\u{a0}c\n".as_bytes()
        );
        assert_eq!(printable(b"a\xe9b\xc0\xafc\xed\xa0\x80d\xff\n"), b"abcd\n");
        assert_eq!(printable(b"a\xe2\x82b\n"), b"ab\n");
    }

    #[test]
    fn a_character_split_between_chunks_is_kept_whole() {
        let mut text = PrintableText::default();
        let mut shown = [0u8; SHOWN];
        let euro = "\u{20ac}".as_bytes();
        assert_eq!(text.printable(&[b'a', euro[0]], &mut shown), b"a");
        assert_eq!(text.printable(&euro[1..2], &mut shown), b"");
        assert_eq!(
            text.printable(&[euro[2], b'\n'], &mut shown),
            "\u{20ac}\n".as_bytes()
        );
        assert_eq!(text.printable(&[0xc2], &mut shown), b"");
        assert_eq!(text.printable(&[0x9b, b'x'], &mut shown), b"x");
    }
}
