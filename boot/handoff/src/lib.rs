//! What EggOS's UEFI loader hands the kernel (D236): the memory map, the command line, the
//! firmware's ACPI root and device tree, the framebuffer the firmware set up and the partition
//! the machine booted from, in one structure at a physical address the kernel is given at entry.
//!
//! The memory map's entries have the layout of PVH's `hvm_memmap_table_entry` (and E820's), so on
//! x86_64 the loader also builds a PVH start info that points at the same table and the kernel's
//! PVH path reads it unchanged.

#![cfg_attr(not(test), no_std)]

/// The first eight bytes of a [`Handoff`].
pub const MAGIC: u64 = u64::from_le_bytes(*b"EGGBOOT1");

/// The layout version this crate writes and reads.
pub const VERSION: u32 = 1;

/// The most memory map entries a loader writes.
pub const MAX_MEMORY_RANGES: usize = 1024;

/// The most bytes of command line a loader passes.
pub const MAX_COMMAND_LINE: usize = 4095;

/// Kinds of [`MemoryRange`], E820's numbers and two of EggOS's own.
pub mod memory_kind {
    pub const RAM: u32 = 1;
    pub const RESERVED: u32 = 2;
    pub const ACPI_RECLAIMABLE: u32 = 3;
    pub const ACPI_NVS: u32 = 4;
    pub const UNUSABLE: u32 = 5;
    pub const PERSISTENT: u32 = 7;
    /// The loader's handoff data: the handoff, the start info, the command line and this map.
    pub const BOOT_DATA: u32 = 0x4547_0001;
    /// UEFI runtime services' code and data, which stay in use after boot.
    pub const FIRMWARE_RUNTIME: u32 = 0x4547_0002;
}

/// How a framebuffer's pixel is laid out in memory.
pub mod pixel_format {
    /// No framebuffer.
    pub const NONE: u32 = 0;
    /// Bytes red, green, blue, unused.
    pub const RGBX: u32 = 1;
    /// Bytes blue, green, red, unused: a `u32` `0x00RRGGBB` on a little-endian CPU.
    pub const BGRX: u32 = 2;
}

/// One range of physical memory and what it holds.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemoryRange {
    pub start: u64,
    pub length: u64,
    pub kind: u32,
    pub reserved: u32,
}

/// Bytes of one [`MemoryRange`]: PVH's and E820's 24.
pub const MEMORY_RANGE_BYTES: usize = 24;

const _: () = assert!(core::mem::size_of::<MemoryRange>() == MEMORY_RANGE_BYTES);

impl MemoryRange {
    pub const fn end(&self) -> u64 {
        self.start.saturating_add(self.length)
    }
}

/// The framebuffer the firmware's graphics output set up.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Framebuffer {
    pub address: u64,
    pub size: u64,
    pub width: u32,
    pub height: u32,
    /// Pixels from the start of one row to the next.
    pub stride_pixels: u32,
    /// One of [`pixel_format`].
    pub format: u32,
}

impl Framebuffer {
    /// Whether there is one the kernel can draw into: a known format, rows inside the memory.
    pub fn is_usable(&self) -> bool {
        let rows = u64::from(self.stride_pixels).saturating_mul(u64::from(self.height));
        self.address != 0
            && self.width != 0
            && self.height != 0
            && self.stride_pixels >= self.width
            && matches!(self.format, pixel_format::RGBX | pixel_format::BGRX)
            && rows.saturating_mul(4) <= self.size
    }
}

/// What the loader hands the kernel.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Handoff {
    pub magic: u64,
    pub version: u32,
    /// Bytes of this structure as the loader wrote it.
    pub length: u32,
    /// Physical address of `memory_map_entries` [`MemoryRange`]s, sorted, not overlapping.
    pub memory_map: u64,
    pub memory_map_entries: u32,
    pub command_line_length: u32,
    /// Physical address of the command line's bytes, not NUL-ended.
    pub command_line: u64,
    /// Physical address of the ACPI RSDP, 0 for none.
    pub rsdp: u64,
    /// Physical address of a flattened device tree, 0 for none.
    pub device_tree: u64,
    pub framebuffer: Framebuffer,
    /// The GPT unique partition GUID of the partition the loader was read from, all zero when
    /// unknown, in the byte order GPT stores it.
    pub boot_partition: [u8; 16],
    /// Physical memory the kernel image was loaded at.
    pub kernel_start: u64,
    pub kernel_end: u64,
    /// The physical memory of the handoff's own data ([`memory_kind::BOOT_DATA`]).
    pub boot_data_start: u64,
    pub boot_data_end: u64,
    /// Physical address of the UEFI system table, for runtime services; 0 for none.
    pub uefi_system_table: u64,
}

/// Bytes of a [`Handoff`].
pub const HANDOFF_BYTES: usize = core::mem::size_of::<Handoff>();

impl Handoff {
    /// An empty handoff with the magic, version and length filled in.
    pub const fn new() -> Self {
        Self {
            magic: MAGIC,
            version: VERSION,
            length: HANDOFF_BYTES as u32,
            memory_map: 0,
            memory_map_entries: 0,
            command_line_length: 0,
            command_line: 0,
            rsdp: 0,
            device_tree: 0,
            framebuffer: Framebuffer {
                address: 0,
                size: 0,
                width: 0,
                height: 0,
                stride_pixels: 0,
                format: pixel_format::NONE,
            },
            boot_partition: [0; 16],
            kernel_start: 0,
            kernel_end: 0,
            boot_data_start: 0,
            boot_data_end: 0,
            uefi_system_table: 0,
        }
    }

    /// The handoff in `bytes`, if they hold one this crate understands.
    pub fn read(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < HANDOFF_BYTES {
            return None;
        }
        // SAFETY: `bytes` holds at least `HANDOFF_BYTES` bytes, every bit pattern is a valid
        // `Handoff` (integers and byte arrays only), and `read_unaligned` takes any alignment.
        let handoff = unsafe { bytes.as_ptr().cast::<Handoff>().read_unaligned() };
        let entries = handoff.memory_map_entries as usize;
        (handoff.magic == MAGIC
            && handoff.version == VERSION
            && handoff.length as usize >= HANDOFF_BYTES
            && entries <= MAX_MEMORY_RANGES
            && handoff.command_line_length as usize <= MAX_COMMAND_LINE)
            .then_some(handoff)
    }

    /// The handoff's bytes.
    pub fn as_bytes(&self) -> &[u8; HANDOFF_BYTES] {
        // SAFETY: `Handoff` is `repr(C)` with no padding (every field is 8-byte aligned or packs
        // to it), so all its bytes are initialized, and the array has its size.
        unsafe { &*(self as *const Self).cast::<[u8; HANDOFF_BYTES]>() }
    }

    /// Whether the partition the loader came from is known.
    pub fn knows_boot_partition(&self) -> bool {
        self.boot_partition != [0; 16]
    }
}

/// The memory map entries in `bytes`, at most `entries` of them.
pub fn memory_ranges(bytes: &[u8], entries: usize) -> impl Iterator<Item = MemoryRange> + '_ {
    bytes
        .as_chunks::<MEMORY_RANGE_BYTES>()
        .0
        .iter()
        .take(entries)
        .map(|entry| {
            let word = |offset: usize| {
                u64::from_le_bytes(entry[offset..offset + 8].try_into().unwrap_or_default())
            };
            MemoryRange {
                start: word(0),
                length: word(8),
                kind: u32::from_le_bytes(entry[16..20].try_into().unwrap_or_default()),
                reserved: 0,
            }
        })
}

/// UEFI memory types (UEFI 2.10, 7.2, `EFI_MEMORY_TYPE`).
pub mod uefi_memory_type {
    pub const RESERVED: u32 = 0;
    pub const LOADER_CODE: u32 = 1;
    pub const LOADER_DATA: u32 = 2;
    pub const BOOT_SERVICES_CODE: u32 = 3;
    pub const BOOT_SERVICES_DATA: u32 = 4;
    pub const RUNTIME_SERVICES_CODE: u32 = 5;
    pub const RUNTIME_SERVICES_DATA: u32 = 6;
    pub const CONVENTIONAL: u32 = 7;
    pub const UNUSABLE: u32 = 8;
    pub const ACPI_RECLAIM: u32 = 9;
    pub const ACPI_NVS: u32 = 10;
    pub const MEMORY_MAPPED_IO: u32 = 11;
    pub const MEMORY_MAPPED_IO_PORT_SPACE: u32 = 12;
    pub const PAL_CODE: u32 = 13;
    pub const PERSISTENT: u32 = 14;
    pub const UNACCEPTED: u32 = 15;
}

/// The [`memory_kind`] of UEFI memory type `kind` after `ExitBootServices`; `None` for device
/// memory, which is no part of the memory map the kernel maps as RAM.
pub fn kind_of_uefi_type(kind: u32) -> Option<u32> {
    use uefi_memory_type as uefi;
    match kind {
        uefi::LOADER_CODE
        | uefi::LOADER_DATA
        | uefi::BOOT_SERVICES_CODE
        | uefi::BOOT_SERVICES_DATA
        | uefi::CONVENTIONAL => Some(memory_kind::RAM),
        uefi::RUNTIME_SERVICES_CODE | uefi::RUNTIME_SERVICES_DATA => {
            Some(memory_kind::FIRMWARE_RUNTIME)
        }
        uefi::ACPI_RECLAIM => Some(memory_kind::ACPI_RECLAIMABLE),
        uefi::ACPI_NVS => Some(memory_kind::ACPI_NVS),
        uefi::UNUSABLE | uefi::UNACCEPTED => Some(memory_kind::UNUSABLE),
        uefi::PERSISTENT => Some(memory_kind::PERSISTENT),
        uefi::MEMORY_MAPPED_IO | uefi::MEMORY_MAPPED_IO_PORT_SPACE => None,
        _ => Some(memory_kind::RESERVED),
    }
}

/// Collects a memory map into `out`: ranges sorted by start, neighbours of one kind merged, and
/// where ranges overlap the more restrictive kind kept (anything over RAM). Returns how many
/// entries `out` holds; ranges past its capacity are dropped, RAM first.
#[derive(Debug)]
pub struct MemoryMapWriter<'a> {
    out: &'a mut [MemoryRange],
    count: usize,
}

impl<'a> MemoryMapWriter<'a> {
    pub fn new(out: &'a mut [MemoryRange]) -> Self {
        Self { out, count: 0 }
    }

    /// Adds `range` in its place.
    pub fn add(&mut self, range: MemoryRange) {
        if range.length == 0 {
            return;
        }
        let position = self.out[..self.count]
            .iter()
            .position(|existing| existing.start > range.start)
            .unwrap_or(self.count);
        if position > 0 {
            let before = &mut self.out[position - 1];
            if before.kind == range.kind && before.end() >= range.start {
                before.length = before.end().max(range.end()) - before.start;
                self.merge_from(position - 1);
                return;
            }
        }
        if self.count == self.out.len() {
            if range.kind == memory_kind::RAM {
                return;
            }
            let Some(ram) = self.out[..self.count]
                .iter()
                .rposition(|existing| existing.kind == memory_kind::RAM)
            else {
                return;
            };
            self.out.copy_within(ram + 1..self.count, ram);
            self.count -= 1;
            return self.add(range);
        }
        self.out.copy_within(position..self.count, position + 1);
        self.out[position] = range;
        self.count += 1;
        self.merge_from(position);
    }

    fn merge_from(&mut self, index: usize) {
        while index + 1 < self.count {
            let (current, next) = (self.out[index], self.out[index + 1]);
            if current.kind == next.kind && current.end() >= next.start {
                self.out[index].length = current.end().max(next.end()) - current.start;
                self.out.copy_within(index + 2..self.count, index + 1);
                self.count -= 1;
            } else {
                break;
            }
        }
    }

    /// Marks `start..end` as `kind`, cutting it out of the ranges it overlaps.
    pub fn mark(&mut self, start: u64, end: u64, kind: u32) {
        if start >= end {
            return;
        }
        let mut index = 0;
        while index < self.count {
            let range = self.out[index];
            if range.end() <= start || range.start >= end {
                index += 1;
                continue;
            }
            let head = (range.start < start).then(|| MemoryRange {
                length: start - range.start,
                ..range
            });
            let tail = (range.end() > end).then(|| MemoryRange {
                start: end,
                length: range.end() - end,
                ..range
            });
            self.out.copy_within(index + 1..self.count, index);
            self.count -= 1;
            if let Some(head) = head {
                self.add(head);
            }
            if let Some(tail) = tail {
                self.add(tail);
            }
            index = 0;
        }
        self.add(MemoryRange {
            start,
            length: end - start,
            kind,
            reserved: 0,
        });
    }

    /// How many entries the map holds.
    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

/// PVH's `hvm_start_info`, version 1 (Xen's public `arch-x86/hvm/start_info.h`), as the loader
/// builds it on x86_64 for the kernel's PVH path.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PvhStartInfo {
    pub magic: u32,
    pub version: u32,
    pub flags: u32,
    pub module_count: u32,
    pub module_list: u64,
    pub command_line: u64,
    pub rsdp: u64,
    pub memory_map: u64,
    pub memory_map_entries: u32,
    pub reserved: u32,
}

/// `XEN_HVM_START_MAGIC_VALUE`.
pub const PVH_START_MAGIC: u32 = 0x336e_c578;

const _: () = assert!(core::mem::size_of::<PvhStartInfo>() == 56);

impl PvhStartInfo {
    /// The start info for `handoff`'s command line, RSDP and memory map.
    pub fn for_handoff(handoff: &Handoff, command_line_with_nul: u64) -> Self {
        Self {
            magic: PVH_START_MAGIC,
            version: 1,
            flags: 0,
            module_count: 0,
            module_list: 0,
            command_line: command_line_with_nul,
            rsdp: handoff.rsdp,
            memory_map: handoff.memory_map,
            memory_map_entries: handoff.memory_map_entries,
            reserved: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(start: u64, length: u64, kind: u32) -> MemoryRange {
        MemoryRange {
            start,
            length,
            kind,
            reserved: 0,
        }
    }

    #[test]
    fn a_handoff_reads_back_from_its_bytes() {
        let mut handoff = Handoff::new();
        handoff.rsdp = 0xe_0000;
        handoff.memory_map_entries = 3;
        let read = Handoff::read(handoff.as_bytes()).unwrap();
        assert_eq!(read, handoff);
    }

    #[test]
    fn a_handoff_with_another_magic_or_too_many_ranges_is_refused() {
        let mut handoff = Handoff::new();
        handoff.magic = 0;
        assert!(Handoff::read(handoff.as_bytes()).is_none());
        let mut handoff = Handoff::new();
        handoff.memory_map_entries = MAX_MEMORY_RANGES as u32 + 1;
        assert!(Handoff::read(handoff.as_bytes()).is_none());
        assert!(Handoff::read(&[0; 8]).is_none());
    }

    #[test]
    fn neighbours_of_one_kind_merge_and_sort() {
        let mut out = [MemoryRange::default(); 8];
        let mut map = MemoryMapWriter::new(&mut out);
        map.add(range(0x2000, 0x1000, memory_kind::RAM));
        map.add(range(0, 0x1000, memory_kind::RAM));
        map.add(range(0x1000, 0x1000, memory_kind::RAM));
        map.add(range(0x3000, 0x1000, memory_kind::ACPI_NVS));
        assert_eq!(map.len(), 2);
        assert_eq!(out[0], range(0, 0x3000, memory_kind::RAM));
        assert_eq!(out[1], range(0x3000, 0x1000, memory_kind::ACPI_NVS));
    }

    #[test]
    fn marking_cuts_the_range_out_of_ram() {
        let mut out = [MemoryRange::default(); 8];
        let mut map = MemoryMapWriter::new(&mut out);
        map.add(range(0, 0x10_000, memory_kind::RAM));
        map.mark(0x4000, 0x6000, memory_kind::BOOT_DATA);
        assert_eq!(map.len(), 3);
        assert_eq!(out[0], range(0, 0x4000, memory_kind::RAM));
        assert_eq!(out[1], range(0x4000, 0x2000, memory_kind::BOOT_DATA));
        assert_eq!(out[2], range(0x6000, 0xa000, memory_kind::RAM));
    }

    #[test]
    fn a_full_map_drops_ram_before_anything_else() {
        let mut out = [MemoryRange::default(); 2];
        let mut map = MemoryMapWriter::new(&mut out);
        map.add(range(0, 0x1000, memory_kind::RAM));
        map.add(range(0x2000, 0x1000, memory_kind::RESERVED));
        map.add(range(0x4000, 0x1000, memory_kind::ACPI_NVS));
        assert_eq!(map.len(), 2);
        assert_eq!(out[0].kind, memory_kind::RESERVED);
        assert_eq!(out[1].kind, memory_kind::ACPI_NVS);
    }

    #[test]
    fn uefi_types_become_kinds_and_device_memory_is_left_out() {
        assert_eq!(
            kind_of_uefi_type(uefi_memory_type::BOOT_SERVICES_DATA),
            Some(memory_kind::RAM)
        );
        assert_eq!(
            kind_of_uefi_type(uefi_memory_type::RUNTIME_SERVICES_CODE),
            Some(memory_kind::FIRMWARE_RUNTIME)
        );
        assert_eq!(kind_of_uefi_type(uefi_memory_type::MEMORY_MAPPED_IO), None);
        assert_eq!(kind_of_uefi_type(99), Some(memory_kind::RESERVED));
    }

    #[test]
    fn memory_ranges_read_the_pvh_layout() {
        let mut bytes = [0u8; 48];
        bytes[..8].copy_from_slice(&0x1000u64.to_le_bytes());
        bytes[8..16].copy_from_slice(&0x2000u64.to_le_bytes());
        bytes[16..20].copy_from_slice(&memory_kind::RAM.to_le_bytes());
        bytes[24..32].copy_from_slice(&0x8000u64.to_le_bytes());
        bytes[32..40].copy_from_slice(&0x100u64.to_le_bytes());
        bytes[40..44].copy_from_slice(&memory_kind::RESERVED.to_le_bytes());
        let ranges: Vec<_> = memory_ranges(&bytes, 2).collect();
        assert_eq!(ranges[0], range(0x1000, 0x2000, memory_kind::RAM));
        assert_eq!(ranges[1], range(0x8000, 0x100, memory_kind::RESERVED));
    }

    #[test]
    fn a_framebuffer_is_usable_only_with_a_known_format_inside_its_memory() {
        let framebuffer = Framebuffer {
            address: 0x8000_0000,
            size: 1920 * 1080 * 4,
            width: 1920,
            height: 1080,
            stride_pixels: 1920,
            format: pixel_format::BGRX,
        };
        assert!(framebuffer.is_usable());
        assert!(
            !Framebuffer {
                format: pixel_format::NONE,
                ..framebuffer
            }
            .is_usable()
        );
        assert!(
            !Framebuffer {
                stride_pixels: 2048,
                ..framebuffer
            }
            .is_usable()
        );
    }
}
