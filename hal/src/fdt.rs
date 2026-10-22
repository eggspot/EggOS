//! Reading a flattened device tree (FDT, "DTB"): the description of the machine that Arm and
//! RISC-V firmware hands the kernel. Today the kernel reads only what it needs to find its
//! memory: the RAM (`/memory` nodes) and what the firmware reserved (the memory reservation
//! block and `/reserved-memory`). Full device discovery comes with the driver model (M5).
//!
//! The format is from the public Devicetree Specification (v0.4, chapter 5): a 40-byte
//! big-endian header, the memory reservation block (pairs of 64-bit address and size ending in
//! a zero pair), and the structure block (a stream of 32-bit tokens, each node's properties
//! before its children), whose property names live in the strings block. This module is
//! architecture-independent and works on a byte slice.

use crate::memory::PhysicalRange;

/// The FDT header's magic number.
pub const MAGIC: u32 = 0xd00d_feed;
/// Length of the FDT header (version 17).
pub const HEADER_LENGTH: usize = 40;

const FDT_BEGIN_NODE: u32 = 1;
const FDT_END_NODE: u32 = 2;
const FDT_PROP: u32 = 3;
const FDT_NOP: u32 = 4;
const FDT_END: u32 = 9;

/// The FDT version this reader understands: the tree's `version` must be at least this and
/// its `last_comp_version` at most this.
const SUPPORTED_VERSION: u32 = 17;

/// Node levels tracked: the root, its children (memory nodes, `/reserved-memory`) and
/// grandchildren (reserved regions). Deeper nodes are counted, not tracked.
const MAX_DEPTH: usize = 3;

/// Why a device tree was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    BadMagic,
    UnsupportedVersion,
    /// A header offset or size points outside the blob, or the structure is cut short.
    Truncated,
    /// The structure block is not a well-formed token stream.
    Malformed,
}

/// The size the header at the start of `header` declares for the whole blob, if `header` holds
/// a valid FDT header. Lets architecture code find out how many bytes to map.
pub fn total_size(header: &[u8]) -> Result<usize, Error> {
    if be32(header, 0).ok_or(Error::Truncated)? != MAGIC {
        return Err(Error::BadMagic);
    }
    let size = be32(header, 4).ok_or(Error::Truncated)?;
    let size = usize::try_from(size).map_err(|_| Error::Truncated)?;
    if size < HEADER_LENGTH {
        return Err(Error::Truncated);
    }
    Ok(size)
}

/// A validated device tree.
#[derive(Debug, Clone, Copy)]
pub struct Fdt<'a> {
    structure: &'a [u8],
    strings: &'a [u8],
    reservations: &'a [u8],
}

impl<'a> Fdt<'a> {
    /// Checks the header of the blob at the start of `blob`, which must hold at least
    /// [`total_size`] bytes.
    pub fn new(blob: &'a [u8]) -> Result<Self, Error> {
        let blob = blob.get(..total_size(blob)?).ok_or(Error::Truncated)?;
        let field = |offset| be32(blob, offset).ok_or(Error::Truncated);
        // Version 17 added the structure block's size, which this reader relies on.
        if field(24)? > SUPPORTED_VERSION || field(20)? < SUPPORTED_VERSION {
            return Err(Error::UnsupportedVersion);
        }
        // The specification aligns the reservation block to 8 bytes and the structure block
        // to 4, so every read below is aligned (the AArch64 kernel is built with `strict-align`, D10).
        if !field(16)?.is_multiple_of(8) || !field(8)?.is_multiple_of(4) {
            return Err(Error::Malformed);
        }
        let section = |offset: usize, size: Option<usize>| -> Result<&'a [u8], Error> {
            let start = usize::try_from(field(offset)?).map_err(|_| Error::Truncated)?;
            let section = blob.get(start..).ok_or(Error::Truncated)?;
            match size {
                Some(size) => {
                    let size = usize::try_from(field(size)?).map_err(|_| Error::Truncated)?;
                    section.get(..size).ok_or(Error::Truncated)
                }
                None => Ok(section),
            }
        };
        Ok(Self {
            structure: section(8, Some(36))?,
            strings: section(12, Some(32))?,
            // The reservation block has no size field; it ends at its zero entry.
            reservations: section(16, None)?,
        })
    }

    /// The memory reservation block's entries: memory the firmware uses and the kernel must not.
    pub fn reservations(&self) -> impl Iterator<Item = PhysicalRange> + 'a {
        self.reservations
            .as_chunks::<16>()
            .0
            .iter()
            .map(|entry| (be64(entry, 0).unwrap_or(0), be64(entry, 8).unwrap_or(0)))
            .take_while(|&(address, size)| address != 0 || size != 0)
            .map(|(address, size)| PhysicalRange::from_start_length(address, size))
    }

    /// Calls `ram` for each range in the `reg` of every `/memory` node (`device_type =
    /// "memory"`, or named `memory` / `memory@...`) and `reserved` for each range in the `reg`
    /// of every `/reserved-memory` child, decoded with the parent's `#address-cells` and
    /// `#size-cells`. Ranges that do not fit in 64 bits are skipped. Disabled nodes (`status`
    /// other than `"okay"`) are still reported as reserved: a reservation is never ignored.
    pub fn memory(
        &self,
        ram: &mut dyn FnMut(PhysicalRange),
        reserved: &mut dyn FnMut(PhysicalRange),
    ) -> Result<(), Error> {
        /// What the walker knows about each open node.
        #[derive(Clone, Copy, Default)]
        struct Node<'a> {
            /// Cells of the `reg` of this node's children (its own `#address-cells` and
            /// `#size-cells`; 2 and 1 when absent, as the specification says).
            child_cells: (u32, u32),
            kind: Kind,
            reg: Option<&'a [u8]>,
            is_memory_type: bool,
            disabled: bool,
        }
        #[derive(Clone, Copy, Default, PartialEq, Eq)]
        enum Kind {
            #[default]
            Other,
            Root,
            /// A child of the root named `memory` or `memory@...`.
            NamedMemory,
            /// A child of the root with any other name.
            RootChild,
            /// `/reserved-memory`.
            ReservedMemory,
            /// A child of `/reserved-memory`.
            Reserved,
        }

        let mut stack = [Node::default(); MAX_DEPTH];
        let mut depth = 0usize;
        let mut at = 0usize;
        loop {
            let token = be32(self.structure, at).ok_or(Error::Truncated)?;
            at += 4;
            match token {
                FDT_BEGIN_NODE => {
                    let name = cstr(self.structure, at).ok_or(Error::Truncated)?;
                    at = align4(at + name.len() + 1);
                    let parent = match depth.checked_sub(1) {
                        None => None,
                        Some(d) if d < MAX_DEPTH => Some(stack[d].kind),
                        // Inside a node too deep to track: nothing of interest.
                        Some(_) => Some(Kind::Other),
                    };
                    let kind = match parent {
                        None => Kind::Root,
                        Some(Kind::Root) => {
                            let base = name.split(|&b| b == b'@').next().unwrap_or(name);
                            match base {
                                b"memory" => Kind::NamedMemory,
                                b"reserved-memory" => Kind::ReservedMemory,
                                _ => Kind::RootChild,
                            }
                        }
                        Some(Kind::ReservedMemory) => Kind::Reserved,
                        Some(_) => Kind::Other,
                    };
                    if depth < MAX_DEPTH {
                        stack[depth] = Node {
                            child_cells: (2, 1),
                            kind,
                            ..Node::default()
                        };
                    }
                    depth += 1;
                }
                FDT_END_NODE => {
                    depth = depth.checked_sub(1).ok_or(Error::Malformed)?;
                    if depth >= MAX_DEPTH {
                        continue;
                    }
                    let node = stack[depth];
                    let cells = match depth.checked_sub(1) {
                        Some(parent) => stack[parent].child_cells,
                        None => continue,
                    };
                    let is_ram = match node.kind {
                        Kind::NamedMemory => true,
                        Kind::RootChild => node.is_memory_type,
                        _ => false,
                    } && !node.disabled;
                    let report: Option<&mut dyn FnMut(PhysicalRange)> = match node.kind {
                        Kind::Reserved => Some(&mut *reserved),
                        _ if is_ram => Some(&mut *ram),
                        _ => None,
                    };
                    if let (Some(report), Some(reg)) = (report, node.reg) {
                        decode_reg(reg, cells, report);
                    }
                }
                FDT_PROP => {
                    let length = be32(self.structure, at).ok_or(Error::Truncated)?;
                    let name_offset = be32(self.structure, at + 4).ok_or(Error::Truncated)?;
                    let length = usize::try_from(length).map_err(|_| Error::Truncated)?;
                    let start = at + 8;
                    let value = start
                        .checked_add(length)
                        .and_then(|end| self.structure.get(start..end))
                        .ok_or(Error::Truncated)?;
                    at = align4(start + length);
                    let name = usize::try_from(name_offset)
                        .ok()
                        .and_then(|offset| cstr(self.strings, offset))
                        .ok_or(Error::Malformed)?;
                    let node = depth
                        .checked_sub(1)
                        .filter(|&d| d < MAX_DEPTH)
                        .map(|d| &mut stack[d]);
                    let Some(node) = node else {
                        continue;
                    };
                    match name {
                        b"#address-cells" => node.child_cells.0 = be32(value, 0).unwrap_or(2),
                        b"#size-cells" => node.child_cells.1 = be32(value, 0).unwrap_or(1),
                        b"reg" => node.reg = Some(value),
                        b"device_type" => node.is_memory_type = value == b"memory\0",
                        b"status" => {
                            node.disabled = value != b"okay\0" && value != b"ok\0";
                        }
                        _ => {}
                    }
                }
                FDT_NOP => {}
                FDT_END => {
                    return if depth == 0 {
                        Ok(())
                    } else {
                        Err(Error::Malformed)
                    };
                }
                _ => return Err(Error::Malformed),
            }
        }
    }
}

/// Decodes a `reg` property: (address, size) pairs of `cells` 32-bit cells each.
fn decode_reg(
    reg: &[u8],
    (address_cells, size_cells): (u32, u32),
    report: &mut dyn FnMut(PhysicalRange),
) {
    let (Ok(address_cells), Ok(size_cells)) =
        (usize::try_from(address_cells), usize::try_from(size_cells))
    else {
        return;
    };
    let entry = (address_cells + size_cells) * 4;
    if entry == 0 {
        return;
    }
    for pair in reg.chunks_exact(entry) {
        let (address, size) = pair.split_at(address_cells * 4);
        if let (Some(address), Some(size)) = (cells_value(address), cells_value(size)) {
            report(PhysicalRange::from_start_length(address, size));
        }
    }
}

/// A big-endian number of 32-bit cells, if it fits in 64 bits.
fn cells_value(cells: &[u8]) -> Option<u64> {
    let mut value = 0u64;
    for cell in cells.as_chunks::<4>().0 {
        if value >> 32 != 0 {
            return None;
        }
        value = value << 32 | u64::from(u32::from_be_bytes(*cell));
    }
    Some(value)
}

/// The NUL-terminated string at `offset`, without its NUL.
fn cstr(bytes: &[u8], offset: usize) -> Option<&[u8]> {
    let rest = bytes.get(offset..)?;
    let end = rest.iter().position(|&b| b == 0)?;
    Some(&rest[..end])
}

const fn align4(offset: usize) -> usize {
    (offset + 3) & !3
}

fn be32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn be64(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_be_bytes(
        bytes.get(offset..offset.checked_add(8)?)?.try_into().ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds device-tree blobs for tests, following the specification's layout.
    #[derive(Default)]
    struct Builder {
        structure: Vec<u8>,
        strings: Vec<u8>,
        reservations: Vec<(u64, u64)>,
    }

    impl Builder {
        fn begin(&mut self, name: &str) -> &mut Self {
            self.token(FDT_BEGIN_NODE);
            self.structure.extend_from_slice(name.as_bytes());
            self.structure.push(0);
            self.pad();
            self
        }
        fn end(&mut self) -> &mut Self {
            self.token(FDT_END_NODE);
            self
        }
        fn prop(&mut self, name: &str, value: &[u8]) -> &mut Self {
            let offset = self.strings.len() as u32;
            self.strings.extend_from_slice(name.as_bytes());
            self.strings.push(0);
            self.token(FDT_PROP);
            self.token(value.len() as u32);
            self.token(offset);
            self.structure.extend_from_slice(value);
            self.pad();
            self
        }
        fn cells(&mut self, name: &str, cells: &[u32]) -> &mut Self {
            let value: Vec<u8> = cells.iter().flat_map(|c| c.to_be_bytes()).collect();
            self.prop(name, &value)
        }
        fn token(&mut self, token: u32) {
            self.structure.extend_from_slice(&token.to_be_bytes());
        }
        fn pad(&mut self) {
            while !self.structure.len().is_multiple_of(4) {
                self.structure.push(0);
            }
        }
        fn build(&mut self) -> Vec<u8> {
            self.token(FDT_END);
            let reservations_offset = HEADER_LENGTH;
            let mut reservations = Vec::new();
            for &(a, s) in self.reservations.iter().chain(&[(0, 0)]) {
                reservations.extend_from_slice(&a.to_be_bytes());
                reservations.extend_from_slice(&s.to_be_bytes());
            }
            let structure_offset = reservations_offset + reservations.len();
            let strings_offset = structure_offset + self.structure.len();
            let total = strings_offset + self.strings.len();
            let header = [
                MAGIC,
                total as u32,
                structure_offset as u32,
                strings_offset as u32,
                reservations_offset as u32,
                17,
                16,
                0,
                self.strings.len() as u32,
                self.structure.len() as u32,
            ];
            let mut blob: Vec<u8> = header.iter().flat_map(|w| w.to_be_bytes()).collect();
            blob.extend_from_slice(&reservations);
            blob.extend_from_slice(&self.structure);
            blob.extend_from_slice(&self.strings);
            blob
        }
    }

    fn memory_of(blob: &[u8]) -> Result<(Vec<PhysicalRange>, Vec<PhysicalRange>), Error> {
        let fdt = Fdt::new(blob)?;
        let (mut ram, mut reserved) = (Vec::new(), Vec::new());
        fdt.memory(&mut |r| ram.push(r), &mut |r| reserved.push(r))?;
        reserved.extend(fdt.reservations());
        Ok((ram, reserved))
    }

    fn range(start: u64, length: u64) -> PhysicalRange {
        PhysicalRange::from_start_length(start, length)
    }

    /// The shape of QEMU `virt`'s tree on RISC-V once OpenSBI has added its reservation.
    fn qemu_like() -> Vec<u8> {
        let mut b = Builder::default();
        b.reservations.push((0x8700_0000, 0x1000));
        b.begin("")
            .cells("#address-cells", &[2])
            .cells("#size-cells", &[2])
            .begin("reserved-memory")
            .cells("#address-cells", &[2])
            .cells("#size-cells", &[2])
            .prop("ranges", &[])
            .begin("mmode_resv0@80000000")
            .cells("reg", &[0, 0x8000_0000, 0, 0x4_0000])
            .prop("no-map", &[])
            .end()
            .end()
            .begin("memory@80000000")
            .prop("device_type", b"memory\0")
            .cells("reg", &[0, 0x8000_0000, 0, 0x800_0000])
            .end()
            .begin("cpus")
            .cells("#address-cells", &[1])
            .cells("#size-cells", &[0])
            .begin("cpu@0")
            .prop("device_type", b"cpu\0")
            .cells("reg", &[0])
            .end()
            .end()
            .end();
        b.build()
    }

    #[test]
    fn finds_ram_and_every_kind_of_reservation() {
        let blob = qemu_like();
        assert_eq!(total_size(&blob), Ok(blob.len()));
        let (ram, reserved) = memory_of(&blob).unwrap();
        assert_eq!(ram, [range(0x8000_0000, 0x800_0000)]);
        assert_eq!(
            reserved,
            [range(0x8000_0000, 0x4_0000), range(0x8700_0000, 0x1000)]
        );
    }

    #[test]
    fn decodes_one_cell_addresses_and_several_ranges() {
        let mut b = Builder::default();
        b.begin("")
            .cells("#address-cells", &[1])
            .cells("#size-cells", &[1])
            .begin("memory")
            .prop("device_type", b"memory\0")
            .cells("reg", &[0x4000_0000, 0x1000_0000, 0x6000_0000, 0x2000])
            .end()
            .end();
        let (ram, _) = memory_of(&b.build()).unwrap();
        assert_eq!(
            ram,
            [range(0x4000_0000, 0x1000_0000), range(0x6000_0000, 0x2000)]
        );
    }

    #[test]
    fn default_cells_are_two_and_one() {
        let mut b = Builder::default();
        b.begin("")
            .begin("ram@1")
            .prop("device_type", b"memory\0")
            .cells("reg", &[1, 0, 0x1000])
            .end()
            .end();
        let (ram, _) = memory_of(&b.build()).unwrap();
        assert_eq!(ram, [range(1 << 32, 0x1000)]);
    }

    #[test]
    fn ignores_nested_and_disabled_memory_and_other_device_types() {
        let mut b = Builder::default();
        b.begin("")
            .cells("#address-cells", &[1])
            .cells("#size-cells", &[1])
            .begin("soc")
            .begin("memory@0")
            .prop("device_type", b"memory\0")
            .cells("reg", &[0, 0x1000])
            .end()
            .end()
            .begin("memory@2000")
            .prop("status", b"disabled\0")
            .cells("reg", &[0x2000, 0x1000])
            .end()
            .begin("cpu@0")
            .prop("device_type", b"cpu\0")
            .cells("reg", &[0, 0x1000])
            .end()
            .end();
        let (ram, reserved) = memory_of(&b.build()).unwrap();
        assert!(ram.is_empty() && reserved.is_empty());
    }

    #[test]
    fn disabled_reservations_still_count() {
        let mut b = Builder::default();
        b.begin("")
            .begin("reserved-memory")
            .cells("#address-cells", &[1])
            .cells("#size-cells", &[1])
            .begin("fw@0")
            .prop("status", b"disabled\0")
            .cells("reg", &[0x1000, 0x1000])
            .end()
            .end()
            .end();
        let (_, reserved) = memory_of(&b.build()).unwrap();
        assert_eq!(reserved, [range(0x1000, 0x1000)]);
    }

    #[test]
    fn skips_values_wider_than_64_bits() {
        let mut b = Builder::default();
        b.begin("")
            .cells("#address-cells", &[3])
            .cells("#size-cells", &[1])
            .begin("memory@0")
            .cells("reg", &[1, 0, 0, 0x1000, 0, 0, 0x2000, 0x1000])
            .end()
            .end();
        let (ram, _) = memory_of(&b.build()).unwrap();
        assert_eq!(ram, [range(0x2000, 0x1000)]);
    }

    #[test]
    fn rejects_bad_headers() {
        let blob = qemu_like();
        let mut bad = blob.clone();
        bad[0] = 0;
        assert_eq!(Fdt::new(&bad).err(), Some(Error::BadMagic));
        assert_eq!(
            Fdt::new(&blob[..blob.len() - 1]).err(),
            Some(Error::Truncated)
        );
        assert_eq!(Fdt::new(&blob[..20]).err(), Some(Error::Truncated));
        let mut old = blob.clone();
        old[20..24].copy_from_slice(&16u32.to_be_bytes());
        assert_eq!(Fdt::new(&old).err(), Some(Error::UnsupportedVersion));
        let mut misaligned = blob.clone();
        misaligned[16..20].copy_from_slice(&44u32.to_be_bytes());
        assert_eq!(Fdt::new(&misaligned).err(), Some(Error::Malformed));
        let mut misaligned = blob.clone();
        let structure = be32(&blob, 8).unwrap() + 2;
        misaligned[8..12].copy_from_slice(&structure.to_be_bytes());
        assert_eq!(Fdt::new(&misaligned).err(), Some(Error::Malformed));
        let mut future = blob.clone();
        future[24..28].copy_from_slice(&18u32.to_be_bytes());
        assert_eq!(Fdt::new(&future).err(), Some(Error::UnsupportedVersion));
        let mut past_end = blob;
        past_end[8..12].copy_from_slice(&0xffff_fff0u32.to_be_bytes());
        assert_eq!(Fdt::new(&past_end).err(), Some(Error::Truncated));
    }

    #[test]
    fn rejects_malformed_structure_without_panicking() {
        let blob = qemu_like();
        let fdt = Fdt::new(&blob).unwrap();
        let structure_offset = be32(&blob, 8).unwrap() as usize;
        let structure_size = be32(&blob, 36).unwrap() as usize;
        // Every truncation and every single-byte corruption of the structure block is either
        // read or rejected, never a panic or a hang.
        for cut in 0..structure_size {
            let short = Fdt {
                structure: &fdt.structure[..cut],
                ..fdt
            };
            assert!(short.memory(&mut |_| {}, &mut |_| {}).is_err());
        }
        for at in structure_offset..structure_offset + structure_size {
            for value in [0u8, 1, 2, 3, 9, 0x7f, 0xff] {
                let mut corrupt = blob.clone();
                corrupt[at] = value;
                if let Ok(fdt) = Fdt::new(&corrupt) {
                    let _ = fdt.memory(&mut |_| {}, &mut |_| {});
                }
            }
        }
        // Nesting deeper than the walker tracks is skipped, not a panic.
        let mut b = Builder::default();
        b.begin("")
            .cells("#address-cells", &[1])
            .cells("#size-cells", &[1]);
        for _ in 0..MAX_DEPTH + 10 {
            b.begin("n").cells("reg", &[0, 0x1000]);
        }
        for _ in 0..MAX_DEPTH + 10 {
            b.end();
        }
        b.begin("memory@0")
            .cells("reg", &[0x4000, 0x1000])
            .end()
            .end();
        let (ram, _) = memory_of(&b.build()).unwrap();
        assert_eq!(ram, [range(0x4000, 0x1000)]);
        // An END_NODE without a node.
        let mut b = Builder::default();
        b.end();
        assert_eq!(memory_of(&b.build()).err(), Some(Error::Malformed));
    }
}
