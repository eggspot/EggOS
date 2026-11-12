//! The calls whose 64-bit values a 32-bit program passes in two registers (D491): `_llseek`
//! (the high and low halves, then where to put the result), `mmap2` (the offset in 4096-byte
//! units), `pread64`, `pwrite64`, `ftruncate64`, `fallocate` and `fadvise64_64` (each value's
//! low half first on a little-endian machine, 32-bit Arm's EABI starting each pair at an even
//! register), and `statfs64` and `fstatfs64` (which take the structure's size). Each is joined
//! here into the call the 64-bit ports make, so the handlers stay the same. On the 64-bit ports
//! [`takes_split_values`] is never asked.

use super::thread::LinuxThread;
use super::{Errno, abi, file_changes, files, memfd, number, statfs};
use crate::process::Process;
use alloc::sync::Arc;
use core::ops::ControlFlow;
use eggos_hal::Platform;
use eggos_hal::user::UserContext;

/// Whether the programs are 32-bit, so some calls split their 64-bit values.
pub const SPLITS_WIDE_VALUES: bool = abi::WORD_BYTES == 4;

/// `mmap2`'s offset unit, whatever the page size.
const MMAP2_UNIT: u64 = 4096;

/// Whether call `number` passes a 64-bit value in two registers on a 32-bit port.
#[inline(always)]
pub fn takes_split_values(number: u64) -> bool {
    matches!(
        number,
        number::LSEEK
            | number::MMAP
            | number::PREAD64
            | number::PWRITE64
            | number::FTRUNCATE
            | number::FALLOCATE
            | number::FADVISE64
            | number::STATFS
            | number::FSTATFS
    )
}

struct Registers {
    values: [u64; 6],
    next: usize,
}

impl Registers {
    fn next(&mut self) -> u64 {
        let value = self.values.get(self.next).copied().unwrap_or(0) & 0xffff_ffff;
        self.next += 1;
        value
    }

    fn pair(&mut self) -> u64 {
        if abi::ALIGNED_REGISTER_PAIRS && self.next % 2 == 1 {
            self.next += 1;
        }
        let first = self.next();
        let second = self.next();
        match cfg!(target_endian = "big") {
            true => first << 32 | second,
            false => second << 32 | first,
        }
    }
}

/// Runs call `number`, one [`takes_split_values`] names, with the program's registers, and
/// sets its result.
#[cold]
#[inline(never)]
pub fn call<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    number: u64,
    context: &mut UserContext,
) -> ControlFlow<()> {
    let registers = Registers {
        values: context.syscall_arguments(),
        next: 0,
    };
    let result = result::<P>(process, thread, number, registers);
    context.set_syscall_result(result.unwrap_or_else(Errno::result));
    ControlFlow::Continue(())
}

fn result<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    number: u64,
    mut registers: Registers,
) -> Result<u64, Errno> {
    let fd = registers.next();
    match number {
        number::LSEEK => {
            let high = registers.next();
            let low = registers.next();
            let result = registers.next();
            let whence = registers.next();
            let offset = super::lseek(thread.files(), fd, high << 32 | low, whence)?;
            process
                .write_user_memory(result, &offset.to_ne_bytes())
                .map(|()| 0)
                .map_err(|_| Errno::EFAULT)
        }
        number::MMAP => {
            let [_, length, protection, flags, file, units] = registers.values;
            let arguments = [fd, length, protection, flags, file, units.wrapping_mul(MMAP2_UNIT)];
            super::memory_call::<P>(process, thread.files(), number::MMAP, arguments)
        }
        number::PREAD64 | number::PWRITE64 => {
            let buffer = registers.next();
            let count = registers.next();
            let offset = registers.pair();
            match number {
                number::PREAD64 => files::pread64(process, thread.files(), fd, buffer, count, offset),
                _ => file_changes::pwrite64(process, thread, fd, buffer, count, offset),
            }
        }
        number::FTRUNCATE => file_changes::ftruncate::<P>(thread, fd, registers.pair()),
        number::FALLOCATE => {
            let mode = registers.next();
            let offset = registers.pair();
            let length = registers.pair();
            memfd::fallocate(thread, fd, mode, offset, length)
        }
        number::FADVISE64 => {
            let advice_first = abi::FADVISE_ADVICE_SECOND.then(|| registers.next());
            let _offset = registers.pair();
            let length = registers.pair();
            let advice = advice_first.unwrap_or_else(|| registers.next());
            files::fadvise64(thread.files(), fd, length, advice)
        }
        number::STATFS | number::FSTATFS => {
            let size = registers.next();
            let buffer = registers.next();
            if size != statfs::STATFS_SIZE as u64 {
                return Err(Errno::EINVAL);
            }
            match number {
                number::STATFS => statfs::statfs(process, thread, fd, buffer),
                _ => statfs::fstatfs(process, thread.files(), fd, buffer),
            }
        }
        _ => Err(Errno::ENOSYS),
    }
}
