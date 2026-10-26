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
use crate::random::SeedSource;

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
        // to 4, so every read below is aligned (the AArch64 kernel is built `strict-align`, D10).
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

impl Fdt<'_> {
    /// Calls `cpu` with the hardware id of every enabled CPU: the first address in the `reg` of
    /// each child of `/cpus` whose `device_type` is `"cpu"` and whose `status` is absent or
    /// `"okay"` (Devicetree Specification v0.4, section 3.8), decoded with `/cpus`'
    /// `#address-cells` (the MPIDR affinity on Arm, the hart id on RISC-V). Other children,
    /// such as `cpu-map`, and ids wider than 64 bits are skipped.
    pub fn cpus(&self, cpu: &mut dyn FnMut(u64)) -> Result<(), Error> {
        /// What the walker knows about an open child of `/cpus`.
        #[derive(Clone, Copy, Default)]
        struct Cpu<'a> {
            reg: Option<&'a [u8]>,
            is_cpu_type: bool,
            disabled: bool,
        }
        // `/cpus`' `#address-cells`, once inside it; the specification's default otherwise.
        let mut address_cells = 2u32;
        let mut in_cpus = false;
        let mut child = Cpu::default();
        let mut depth = 0usize;
        let mut at = 0usize;
        loop {
            let token = be32(self.structure, at).ok_or(Error::Truncated)?;
            at += 4;
            match token {
                FDT_BEGIN_NODE => {
                    let name = cstr(self.structure, at).ok_or(Error::Truncated)?;
                    at = align4(at + name.len() + 1);
                    if depth == 1 && name == b"cpus" {
                        in_cpus = true;
                        address_cells = 2;
                    } else if in_cpus && depth == 2 {
                        child = Cpu::default();
                    }
                    depth += 1;
                }
                FDT_END_NODE => {
                    depth = depth.checked_sub(1).ok_or(Error::Malformed)?;
                    if in_cpus && depth == 2 && child.is_cpu_type && !child.disabled {
                        let id = usize::try_from(address_cells)
                            .ok()
                            .and_then(|cells| child.reg?.get(..cells.checked_mul(4)?))
                            .and_then(cells_value);
                        if let Some(id) = id {
                            cpu(id);
                        }
                    } else if in_cpus && depth == 1 {
                        in_cpus = false;
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
                    if !in_cpus {
                        continue;
                    }
                    let name = usize::try_from(name_offset)
                        .ok()
                        .and_then(|offset| cstr(self.strings, offset))
                        .ok_or(Error::Malformed)?;
                    match (depth, name) {
                        (2, b"#address-cells") => address_cells = be32(value, 0).unwrap_or(2),
                        (3, b"reg") => child.reg = Some(value),
                        (3, b"device_type") => child.is_cpu_type = value == b"cpu\0",
                        (3, b"status") => {
                            child.disabled = value != b"okay\0" && value != b"ok\0";
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

impl<'a> Fdt<'a> {
    /// Calls `property` with the name and value of each property of `/chosen`, the node where
    /// firmware passes the kernel its parameters (Devicetree Specification v0.4, section 3.6).
    pub fn chosen(&self, property: &mut dyn FnMut(&'a [u8], &'a [u8])) -> Result<(), Error> {
        let mut in_chosen = false;
        let mut depth = 0usize;
        let mut at = 0usize;
        loop {
            let token = be32(self.structure, at).ok_or(Error::Truncated)?;
            at += 4;
            match token {
                FDT_BEGIN_NODE => {
                    let name = cstr(self.structure, at).ok_or(Error::Truncated)?;
                    at = align4(at + name.len() + 1);
                    if depth == 1 && name == b"chosen" {
                        in_chosen = true;
                    }
                    depth += 1;
                }
                FDT_END_NODE => {
                    depth = depth.checked_sub(1).ok_or(Error::Malformed)?;
                    if depth == 1 {
                        in_chosen = false;
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
                    if !(in_chosen && depth == 2) {
                        continue;
                    }
                    let name = usize::try_from(name_offset)
                        .ok()
                        .and_then(|offset| cstr(self.strings, offset))
                        .ok_or(Error::Malformed)?;
                    property(name, value);
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

/// The `compatible` strings of a RISC-V PLIC.
const PLIC_COMPATIBLES: [&[u8]; 2] = [b"riscv,plic0", b"sifive,plic-1.0.0"];

/// The RISC-V interrupt cause a hart's supervisor external interrupt has, as a PLIC's
/// `interrupts-extended` names it.
const SUPERVISOR_EXTERNAL_INTERRUPT: u32 = 9;

/// One token of the structure block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token<'a> {
    BeginNode(&'a [u8]),
    EndNode,
    Property(&'a [u8], &'a [u8]),
}

impl<'a> Fdt<'a> {
    /// The structure block's node and property tokens, in order, `FDT_NOP` skipped, until
    /// `FDT_END`; an error ends them.
    fn tokens(&self) -> impl Iterator<Item = Result<Token<'a>, Error>> + 'a {
        let (structure, strings) = (self.structure, self.strings);
        let mut at = 0usize;
        let mut done = false;
        core::iter::from_fn(move || {
            while !done {
                let next = (|| {
                    let token = be32(structure, at).ok_or(Error::Truncated)?;
                    at += 4;
                    match token {
                        FDT_BEGIN_NODE => {
                            let name = cstr(structure, at).ok_or(Error::Truncated)?;
                            at = align4(at + name.len() + 1);
                            Ok(Some(Token::BeginNode(name)))
                        }
                        FDT_END_NODE => Ok(Some(Token::EndNode)),
                        FDT_PROP => {
                            let length = be32(structure, at).ok_or(Error::Truncated)?;
                            let name_offset = be32(structure, at + 4).ok_or(Error::Truncated)?;
                            let length = usize::try_from(length).map_err(|_| Error::Truncated)?;
                            let start = at + 8;
                            let value = start
                                .checked_add(length)
                                .and_then(|end| structure.get(start..end))
                                .ok_or(Error::Truncated)?;
                            at = align4(start + length);
                            let name = usize::try_from(name_offset)
                                .ok()
                                .and_then(|offset| cstr(strings, offset))
                                .ok_or(Error::Malformed)?;
                            Ok(Some(Token::Property(name, value)))
                        }
                        FDT_NOP => Ok(None),
                        FDT_END => {
                            done = true;
                            Ok(None)
                        }
                        _ => Err(Error::Malformed),
                    }
                })();
                match next {
                    Ok(Some(token)) => return Some(Ok(token)),
                    Ok(None) => {}
                    Err(error) => {
                        done = true;
                        return Some(Err(error));
                    }
                }
            }
            None
        })
    }

    /// The index of hart `hart`'s supervisor-mode context in the first RISC-V PLIC the tree
    /// describes (`compatible` `"riscv,plic0"` or `"sifive,plic-1.0.0"`), or `None` when there is
    /// no PLIC or it does not interrupt that hart in supervisor mode. The PLIC's
    /// `interrupts-extended` lists one (interrupt-controller phandle, cause) pair per context, in
    /// context order; the hart's is the pair naming the `interrupt-controller` child of the
    /// `/cpus` node whose `reg` is `hart`, with cause 9, the supervisor external interrupt (RISC-V
    /// PLIC binding; a hart's local interrupt controller has `#interrupt-cells` = 1).
    pub fn plic_supervisor_context(&self, hart: u64) -> Result<Option<usize>, Error> {
        let mut depth = 0usize;
        let mut in_cpus = false;
        let mut cpus_address_cells = 2u32;
        let mut cpu_is_hart = false;
        let mut in_hart_controller = false;
        let mut hart_controller = None;
        let mut node_is_plic = false;
        let mut node_contexts = None;
        let mut plic_contexts: Option<&[u8]> = None;
        for token in self.tokens() {
            let finish_node =
                |is_plic: bool, contexts: Option<&'a [u8]>, found: &mut Option<&'a [u8]>| {
                    if is_plic && found.is_none() {
                        *found = contexts;
                    }
                };
            match token? {
                Token::BeginNode(name) => {
                    finish_node(node_is_plic, node_contexts, &mut plic_contexts);
                    (node_is_plic, node_contexts) = (false, None);
                    match depth {
                        1 => in_cpus = name == b"cpus",
                        2 if in_cpus => cpu_is_hart = false,
                        3 if in_cpus => {
                            in_hart_controller =
                                cpu_is_hart && name.starts_with(b"interrupt-controller");
                        }
                        _ => {}
                    }
                    depth += 1;
                }
                Token::EndNode => {
                    finish_node(node_is_plic, node_contexts, &mut plic_contexts);
                    (node_is_plic, node_contexts) = (false, None);
                    depth = depth.checked_sub(1).ok_or(Error::Malformed)?;
                    if depth == 3 {
                        in_hart_controller = false;
                    }
                }
                Token::Property(name, value) => {
                    match (in_cpus, depth, name) {
                        (true, 2, b"#address-cells") => {
                            cpus_address_cells = be32(value, 0).unwrap_or(2);
                        }
                        (true, 3, b"reg") => {
                            cpu_is_hart = usize::try_from(cpus_address_cells)
                                .ok()
                                .and_then(|cells| value.get(..cells.checked_mul(4)?))
                                .and_then(cells_value)
                                == Some(hart);
                        }
                        (true, 4, b"phandle") if in_hart_controller => {
                            hart_controller = be32(value, 0);
                        }
                        _ => {}
                    }
                    match name {
                        b"compatible" => {
                            node_is_plic = value
                                .split(|&byte| byte == 0)
                                .any(|string| PLIC_COMPATIBLES.contains(&string));
                        }
                        b"interrupts-extended" => node_contexts = Some(value),
                        _ => {}
                    }
                }
            }
        }
        if depth != 0 {
            return Err(Error::Malformed);
        }
        let (Some(contexts), Some(controller)) = (plic_contexts, hart_controller) else {
            return Ok(None);
        };
        Ok(contexts.as_chunks::<8>().0.iter().position(|pair| {
            be32(pair, 0) == Some(controller)
                && be32(pair, 4) == Some(SUPERVISOR_EXTERNAL_INTERRUPT)
        }))
    }
}

/// The `/chosen` properties that hold seed material, and what each is.
const SEED_PROPERTIES: [(&[u8], SeedSource); 2] = [
    (b"rng-seed", SeedSource::DeviceTreeRngSeed),
    (b"kaslr-seed", SeedSource::DeviceTreeKaslrSeed),
];

/// The most seed properties [`take_seeds`] takes from one tree.
const MAX_SEEDS: usize = 4;

/// Hands each seed in `/chosen` of the tree at the start of `blob` (`rng-seed`, `kaslr-seed`)
/// to `seed` with its source, then removes it from the tree: the whole property (its token,
/// length, name and padded value) is overwritten with `FDT_NOP` tokens,
/// which every reader skips (Devicetree Specification v0.4, section 5.4.1). So nothing that
/// reads the tree later, a kernel started after this one included, sees the seed, or sees an
/// empty one it might trust. Returns how many it took. A tree that is invalid or malformed
/// anywhere is left as it is.
pub fn take_seeds(
    blob: &mut [u8],
    seed: &mut dyn FnMut(SeedSource, &[u8]),
) -> Result<usize, Error> {
    // (source, offset of the value in `blob`, its length)
    let mut found = [None; MAX_SEEDS];
    {
        let base = blob.as_ptr().addr();
        let tree = Fdt::new(blob)?;
        let mut count = 0;
        tree.chosen(&mut |name, value| {
            let source = SEED_PROPERTIES
                .iter()
                .find_map(|&(seed_name, source)| (seed_name == name).then_some(source));
            if let Some(source) = source
                && count < MAX_SEEDS
            {
                found[count] = Some((source, value.as_ptr().addr() - base, value.len()));
                count += 1;
            }
        })?;
    }
    let mut taken = 0;
    for (source, start, length) in found.into_iter().flatten() {
        let value = start..start + length;
        seed(source, &blob[value]);
        // The property's token, length and name offset precede the value; the walk above read
        // the token after its padding, so the padded end is within the structure block, whose
        // offset in the blob is 4-byte aligned. The value's words are overwritten too.
        let property = start - 12..align4(start + length);
        for word in blob[property].as_chunks_mut::<4>().0 {
            *word = FDT_NOP.to_be_bytes();
        }
        taken += 1;
    }
    Ok(taken)
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

    /// QEMU `virt`'s RISC-V shape for interrupts: harts with their local interrupt controllers,
    /// and a PLIC whose contexts go M-mode, S-mode for each hart in turn.
    fn with_plic(plic_first: bool, compatible: &[u8]) -> Vec<u8> {
        let mut b = Builder::default();
        b.begin("")
            .cells("#address-cells", &[2])
            .cells("#size-cells", &[2]);
        let plic = |b: &mut Builder| {
            b.begin("soc")
                .begin("plic@c000000")
                .cells("interrupts-extended", &[3, 11, 3, 9, 5, 11, 5, 9])
                .prop("compatible", compatible)
                .end()
                .end();
        };
        if plic_first {
            plic(&mut b);
        }
        b.begin("cpus")
            .cells("#address-cells", &[1])
            .cells("#size-cells", &[0]);
        for (hart, phandle) in [(0u32, 3u32), (1, 5)] {
            b.begin(&format!("cpu@{hart}"))
                .prop("device_type", b"cpu\0")
                .cells("reg", &[hart])
                .begin("interrupt-controller")
                .cells("#interrupt-cells", &[1])
                .cells("phandle", &[phandle])
                .end()
                .end();
        }
        b.end();
        if !plic_first {
            plic(&mut b);
        }
        b.end();
        b.build()
    }

    fn plic_context(blob: &[u8], hart: u64) -> Result<Option<usize>, Error> {
        Fdt::new(blob)?.plic_supervisor_context(hart)
    }

    #[test]
    fn finds_each_harts_supervisor_context_in_the_plic() {
        let blob = with_plic(false, b"sifive,plic-1.0.0\0riscv,plic0\0");
        assert_eq!(plic_context(&blob, 0), Ok(Some(1)));
        assert_eq!(plic_context(&blob, 1), Ok(Some(3)));
        assert_eq!(plic_context(&blob, 2), Ok(None));
    }

    #[test]
    fn the_plic_may_come_before_the_cpus() {
        let blob = with_plic(true, b"riscv,plic0\0");
        assert_eq!(plic_context(&blob, 1), Ok(Some(3)));
    }

    #[test]
    fn a_tree_without_a_plic_has_no_context() {
        assert_eq!(
            plic_context(&with_plic(false, b"riscv,aplic\0"), 0),
            Ok(None)
        );
        assert_eq!(plic_context(&qemu_like(), 0), Ok(None));
    }

    #[test]
    fn plic_lookup_rejects_malformed_structure_without_panicking() {
        let mut blob = with_plic(false, b"riscv,plic0\0");
        let end = blob.len() - b"#address-cells\0".len();
        blob.truncate(end);
        assert!(plic_context(&blob, 0).is_err());
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

    fn cpus_of(blob: &[u8]) -> Result<Vec<u64>, Error> {
        let mut ids = Vec::new();
        Fdt::new(blob)?.cpus(&mut |id| ids.push(id))?;
        Ok(ids)
    }

    #[test]
    fn finds_enabled_cpus_in_order() {
        // QEMU `virt`'s shape on RISC-V: one-cell hart ids, an interrupt controller inside each
        // CPU and a `cpu-map` among them.
        let mut b = Builder::default();
        b.begin("")
            .cells("#address-cells", &[2])
            .begin("cpus")
            .cells("#address-cells", &[1])
            .cells("#size-cells", &[0]);
        for hart in [0u32, 1, 2, 3] {
            b.begin(&format!("cpu@{hart}"))
                .prop("device_type", b"cpu\0")
                .cells("reg", &[hart])
                .prop(
                    "status",
                    if hart == 2 {
                        b"disabled\0" as &[u8]
                    } else {
                        b"okay\0"
                    },
                )
                .begin("interrupt-controller")
                .cells("reg", &[99])
                .end()
                .end();
        }
        b.begin("cpu-map")
            .begin("cluster0")
            .begin("core0")
            .cells("cpu", &[1])
            .end()
            .end()
            .end()
            .end() // cpus
            .begin("memory@80000000")
            .prop("device_type", b"memory\0")
            .end()
            .end();
        assert_eq!(cpus_of(&b.build()), Ok(vec![0, 1, 3]));
    }

    #[test]
    fn decodes_two_cell_cpu_ids_and_ignores_cpus_outside_cpus() {
        // Arm: MPIDR affinity in two cells, Aff3 in the high one.
        let mut b = Builder::default();
        b.begin("")
            .begin("soc")
            .begin("cpus")
            .begin("cpu@7")
            .prop("device_type", b"cpu\0")
            .cells("reg", &[0, 7])
            .end()
            .end()
            .end()
            .begin("cpus")
            .cells("#address-cells", &[2])
            .begin("cpu@0")
            .prop("device_type", b"cpu\0")
            .cells("reg", &[0, 0])
            .end()
            .begin("cpu@100000001")
            .prop("device_type", b"cpu\0")
            .cells("reg", &[1, 1])
            .end()
            .begin("cpu@bad")
            .prop("device_type", b"cpu\0")
            .cells("reg", &[5])
            .end()
            .begin("not-a-cpu@2")
            .cells("reg", &[0, 2])
            .end()
            .end()
            .end();
        assert_eq!(cpus_of(&b.build()), Ok(vec![0, 0x1_0000_0001]));
    }

    #[test]
    fn cpus_walker_rejects_malformed_structure_without_panicking() {
        let mut b = Builder::default();
        b.end();
        assert_eq!(cpus_of(&b.build()), Err(Error::Malformed));
        let mut b = Builder::default();
        b.begin("").begin("cpus");
        assert_eq!(cpus_of(&b.build()), Err(Error::Malformed));
    }

    /// A tree with QEMU `virt`'s `/chosen` seeds, the RNG seed of an odd length so its value
    /// is padded, and a nested node with a property of the same name, which is not a seed.
    fn with_seeds() -> Vec<u8> {
        let mut b = Builder::default();
        b.begin("")
            .cells("#address-cells", &[2])
            .cells("#size-cells", &[2])
            .begin("chosen")
            .prop("rng-seed", &[0xa5; 31])
            .prop("bootargs", b"console=ttyS0\0")
            .prop("kaslr-seed", &[0x5a; 8])
            .begin("nested")
            .prop("rng-seed", &[0x77; 4])
            .end()
            .end()
            .begin("memory@80000000")
            .prop("device_type", b"memory\0")
            .cells("reg", &[0, 0x8000_0000, 0, 0x800_0000])
            .end()
            .end();
        b.build()
    }

    fn chosen_of(blob: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
        let mut properties = Vec::new();
        Fdt::new(blob)
            .unwrap()
            .chosen(&mut |name, value| properties.push((name.to_vec(), value.to_vec())))
            .unwrap();
        properties
    }

    #[test]
    fn chosen_lists_only_its_own_properties() {
        let names: Vec<_> = chosen_of(&with_seeds())
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(names, [&b"rng-seed"[..], b"bootargs", b"kaslr-seed"]);
    }

    #[test]
    fn seeds_are_handed_over_then_removed_from_the_tree() {
        let mut blob = with_seeds();
        let mut seeds = Vec::new();
        let taken = take_seeds(&mut blob, &mut |source, bytes| {
            seeds.push((source, bytes.to_vec()));
        });
        assert_eq!(taken, Ok(2));
        assert_eq!(
            seeds,
            [
                (SeedSource::DeviceTreeRngSeed, vec![0xa5; 31]),
                (SeedSource::DeviceTreeKaslrSeed, vec![0x5a; 8]),
            ]
        );
        // The tree still reads, without the seeds; the rest is as it was.
        assert_eq!(
            chosen_of(&blob),
            [(b"bootargs".to_vec(), b"console=ttyS0\0".to_vec())]
        );
        assert_eq!(
            memory_of(&blob).unwrap().0,
            [range(0x8000_0000, 0x800_0000)]
        );
        assert!(
            !blob
                .windows(8)
                .any(|bytes| bytes == [0xa5; 8] || bytes == [0x5a; 8])
        );
        // The nested property is not a seed and stays.
        assert!(blob.windows(4).any(|bytes| bytes == [0x77; 4]));
        // Taking again finds nothing.
        assert_eq!(
            take_seeds(&mut blob, &mut |_, _| panic!("no seed left")),
            Ok(0)
        );
    }

    #[test]
    fn a_malformed_tree_keeps_its_seeds() {
        let mut b = Builder::default();
        b.begin("").begin("chosen").prop("rng-seed", &[0xa5; 32]);
        let mut blob = b.build();
        let before = blob.clone();
        assert_eq!(
            take_seeds(&mut blob, &mut |_, _| panic!("not handed over")),
            Err(Error::Malformed)
        );
        assert_eq!(blob, before);
    }
}
