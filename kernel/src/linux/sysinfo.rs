//! `sysinfo` (D176): how long EggOS has run, its memory and how many processes exist, which C
//! libraries read for `sysconf(_SC_PHYS_PAGES)` and `_SC_AVPHYS_PAGES` (musl and glibc both),
//! and GLib for its memory monitor. Linux's `struct sysinfo`, its `long`s a word of the
//! program's width (D493): the uptime in whole seconds (rounded up, as Linux), three load
//! averages (0 here: EggOS keeps none yet), total and free memory in units of `mem_unit` (1
//! byte, or a page when a 32-bit word cannot hold the total, as Linux), no shared, buffer,
//! swap or high memory, and the process count.

use super::Errno;
use super::process_table::TABLE;
use crate::memory;
use crate::process::Process;
use eggos_hal::Platform;
use eggos_hal::memory::FRAME_SIZE;

const W: usize = super::abi::WORD_BYTES;

/// The size of `struct sysinfo`: 112 bytes on 64-bit programs, 64 on 32-bit ones.
pub const SYSINFO_SIZE: usize = (offset::MEMORY_UNIT + 20 - 2 * W).next_multiple_of(W);
const _: () = assert!(W != 8 || SYSINFO_SIZE == 112);
const _: () = assert!(W != 4 || SYSINFO_SIZE == 64);

mod offset {
    use super::W;

    pub const UPTIME: usize = 0;
    pub const TOTAL_RAM: usize = 4 * W;
    pub const FREE_RAM: usize = 5 * W;
    pub const PROCESSES: usize = 10 * W;
    pub const MEMORY_UNIT: usize = (PROCESSES + 2).next_multiple_of(W) + 2 * W;
    const _: () = assert!(W != 8 || TOTAL_RAM == 32 && PROCESSES == 80 && MEMORY_UNIT == 104);
}

/// What `sysinfo` reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Snapshot {
    pub uptime_seconds: u64,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub processes: u16,
}

/// `snapshot` as `struct sysinfo`.
pub fn sysinfo_bytes(snapshot: &Snapshot) -> [u8; SYSINFO_SIZE] {
    let mut bytes = [0u8; SYSINFO_SIZE];
    let unit = match W == 4 && snapshot.total_bytes > u64::from(u32::MAX) {
        true => FRAME_SIZE,
        false => 1,
    };
    let mut put = |at: usize, value: u64| super::abi::put_word(&mut bytes[at..], value);
    put(offset::UPTIME, snapshot.uptime_seconds);
    put(offset::TOTAL_RAM, snapshot.total_bytes / unit);
    put(offset::FREE_RAM, snapshot.free_bytes / unit);
    bytes[offset::PROCESSES..offset::PROCESSES + 2]
        .copy_from_slice(&snapshot.processes.to_ne_bytes());
    bytes[offset::MEMORY_UNIT..offset::MEMORY_UNIT + 4]
        .copy_from_slice(&(unit as u32).to_ne_bytes());
    bytes
}

/// The whole seconds `ticks` of a clock of `frequency` ticks per second make, rounded up.
pub fn uptime_seconds(ticks: u64, frequency: u64) -> u64 {
    ticks.div_ceil(frequency.max(1))
}

/// `sysinfo(buffer)`: writes the `struct sysinfo` of now to `buffer`; `EFAULT` if it is not
/// the program's memory.
pub fn sysinfo<P: Platform>(process: &Process<P>, buffer: u64) -> Result<u64, Errno> {
    let free_frames = memory::FRAMES
        .lock()
        .as_ref()
        .map_or(0, |frames| frames.free_frames());
    let total_frames = memory::total_frames().max(free_frames);
    let processes = TABLE.lock().list().filter(|listed| !listed.ended).count();
    let snapshot = Snapshot {
        uptime_seconds: uptime_seconds(P::clock_ticks(), P::clock_frequency()),
        total_bytes: total_frames * FRAME_SIZE,
        free_bytes: free_frames * FRAME_SIZE,
        processes: u16::try_from(processes).unwrap_or(u16::MAX),
    };
    process
        .write_user_memory(buffer, &sysinfo_bytes(&snapshot))
        .map(|()| 0)
        .map_err(|_| Errno::EFAULT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(bytes: &[u8; SYSINFO_SIZE], at: usize) -> u64 {
        u64::from_ne_bytes(bytes[at..at + 8].try_into().unwrap())
    }

    #[test]
    fn sysinfo_bytes_are_linux_s_layout_in_bytes() {
        let bytes = sysinfo_bytes(&Snapshot {
            uptime_seconds: 42,
            total_bytes: 1 << 30,
            free_bytes: 1 << 29,
            processes: 7,
        });
        assert_eq!(word(&bytes, offset::UPTIME), 42);
        assert_eq!(word(&bytes, 8) | word(&bytes, 16) | word(&bytes, 24), 0);
        assert_eq!(word(&bytes, offset::TOTAL_RAM), 1 << 30);
        assert_eq!(word(&bytes, offset::FREE_RAM), 1 << 29);
        assert!(bytes[48..80].iter().all(|&byte| byte == 0));
        assert_eq!(
            u16::from_ne_bytes([bytes[offset::PROCESSES], bytes[offset::PROCESSES + 1]]),
            7
        );
        assert!(bytes[82..104].iter().all(|&byte| byte == 0));
        assert_eq!(u32::from_ne_bytes(bytes[104..108].try_into().unwrap()), 1);
    }

    #[test]
    fn uptime_is_rounded_up_to_whole_seconds() {
        assert_eq!(uptime_seconds(0, 1000), 0);
        assert_eq!(uptime_seconds(1, 1000), 1);
        assert_eq!(uptime_seconds(2000, 1000), 2);
        assert_eq!(uptime_seconds(2001, 1000), 3);
        assert_eq!(uptime_seconds(5, 0), 5);
    }
}
