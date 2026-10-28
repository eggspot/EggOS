//! The kernel command line a loader leaves in memory (x86_64's PVH `cmdline_paddr`), checked
//! and copied at boot, before the frame allocator can hand out the memory it is in: where it may
//! be ([`region`]), how much of it is taken ([`prefix`]), and the copy itself
//! ([`CommandLineCopy`]), which needs no `unsafe`.

use crate::memory::PhysicalRange;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

/// The most bytes of the command line kept.
pub const MAX_BYTES: usize = 1024;

/// Where the command line at physical `address` may be read: `address` up to the end of the
/// low memory (`low`, never given to the allocator and mapped whole, where QEMU's PVH option
/// ROM keeps it) or of the RAM range in `ram` holding it, at most [`MAX_BYTES`], and never past
/// `map_end`, the end of the memory the boot direct map reaches. `None` for address 0 or
/// anywhere else.
pub fn region(
    address: u64,
    low: PhysicalRange,
    ram: &mut dyn Iterator<Item = PhysicalRange>,
    map_end: u64,
) -> Option<PhysicalRange> {
    if address == 0 {
        return None;
    }
    let inside = |range: &PhysicalRange| range.start <= address && address < range.end;
    let mut holder = inside(&low).then_some(low);
    for range in ram {
        if holder.is_some() {
            break;
        }
        holder = inside(&range).then_some(range);
    }
    let holder = holder?;
    let end = holder
        .end
        .min(map_end)
        .min(address.saturating_add(MAX_BYTES as u64));
    (address < end).then_some(PhysicalRange {
        start: address,
        end,
    })
}

/// The command line in `bytes`: up to its NUL; without one inside `bytes`, up to the last
/// space, so a token cut by the end is never taken; nothing without a space either.
pub fn prefix(bytes: &[u8]) -> &[u8] {
    let window = bytes.get(..bytes.len().min(MAX_BYTES)).unwrap_or(bytes);
    let end = window
        .iter()
        .position(|&byte| byte == 0)
        .or_else(|| window.iter().rposition(|&byte| byte == b' '))
        .unwrap_or(0);
    window.get(..end).unwrap_or_default()
}

/// The command line, copied once at boot and read any time after.
#[derive(Debug)]
pub struct CommandLineCopy {
    bytes: [AtomicU8; MAX_BYTES],
    length: AtomicUsize,
    taken: AtomicBool,
}

impl CommandLineCopy {
    /// Nothing copied yet.
    pub const fn new() -> Self {
        Self {
            bytes: [const { AtomicU8::new(0) }; MAX_BYTES],
            length: AtomicUsize::new(0),
            taken: AtomicBool::new(false),
        }
    }

    /// Whether [`Self::store`] ran.
    pub fn is_taken(&self) -> bool {
        self.taken.load(Ordering::Acquire)
    }

    /// Keeps `line`, at most [`MAX_BYTES`] of it; only the first call counts.
    pub fn store(&self, line: &[u8]) {
        if self.taken.swap(true, Ordering::AcqRel) {
            return;
        }
        let line = line.get(..line.len().min(MAX_BYTES)).unwrap_or_default();
        for (slot, &byte) in self.bytes.iter().zip(line) {
            slot.store(byte, Ordering::Relaxed);
        }
        self.length.store(line.len(), Ordering::Release);
    }

    /// The line kept, copied into `out`.
    pub fn load<'o>(&self, out: &'o mut [u8; MAX_BYTES]) -> &'o [u8] {
        let length = self.length.load(Ordering::Acquire).min(MAX_BYTES);
        for (byte, slot) in out.iter_mut().zip(&self.bytes).take(length) {
            *byte = slot.load(Ordering::Relaxed);
        }
        out.get(..length).unwrap_or_default()
    }
}

impl Default for CommandLineCopy {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOW: PhysicalRange = PhysicalRange {
        start: 0,
        end: 0x10_0000,
    };
    const MAP_END: u64 = 1 << 32;

    fn ram() -> [PhysicalRange; 2] {
        [
            PhysicalRange {
                start: 0x10_0000,
                end: 0x800_0000,
            },
            PhysicalRange {
                start: 0x1_0000_0000,
                end: 0x2_0000_0000,
            },
        ]
    }

    fn region_of(address: u64) -> Option<PhysicalRange> {
        region(address, LOW, &mut ram().into_iter(), MAP_END)
    }

    #[test]
    fn the_line_may_be_in_low_memory_or_in_ram_and_is_clipped_to_its_holder() {
        assert_eq!(region_of(0), None);
        let in_rom = region_of(0xc_2000).unwrap();
        assert_eq!(
            (in_rom.start, in_rom.end),
            (0xc_2000, 0xc_2000 + MAX_BYTES as u64)
        );
        let at_edge = region_of(0xf_ff00).unwrap();
        assert_eq!(at_edge.end, 0x10_0000);
        let in_ram = region_of(0x20_0000).unwrap();
        assert_eq!(in_ram.end - in_ram.start, MAX_BYTES as u64);
        let ram_edge = region_of(0x7ff_fff0).unwrap();
        assert_eq!(ram_edge.end, 0x800_0000);
        assert_eq!(region_of(0x900_0000), None);
        assert_eq!(region_of(0x1_0000_1000), None);
        assert_eq!(region_of(u64::MAX), None);
    }

    #[test]
    fn the_line_ends_at_its_nul_or_at_the_last_whole_word() {
        assert_eq!(
            prefix(b"eggos.mcp_allow=write\0junk"),
            b"eggos.mcp_allow=write"
        );
        assert_eq!(prefix(b"\0eggos.mcp_allow=run"), b"");
        assert_eq!(prefix(b"a=1 eggos.mcp_allow=wr"), b"a=1");
        assert_eq!(prefix(b"eggos.mcp_allow=write"), b"");
        assert_eq!(prefix(b""), b"");
        let mut long = vec![b'a'; MAX_BYTES + 10];
        long[3] = b' ';
        assert_eq!(prefix(&long), b"aaa");
        long[MAX_BYTES + 2] = 0;
        assert_eq!(prefix(&long), b"aaa");
    }

    #[test]
    fn the_copy_keeps_the_first_line_stored() {
        let copy = CommandLineCopy::new();
        let mut out = [0u8; MAX_BYTES];
        assert!(!copy.is_taken());
        assert_eq!(copy.load(&mut out), b"");
        copy.store(b"eggos.mcp_allow=write,install");
        copy.store(b"eggos.mcp_allow=run");
        assert!(copy.is_taken());
        assert_eq!(copy.load(&mut out), b"eggos.mcp_allow=write,install");
        let big = CommandLineCopy::new();
        big.store(&[b'x'; MAX_BYTES + 5]);
        assert_eq!(big.load(&mut out).len(), MAX_BYTES);
    }
}
