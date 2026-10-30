//! What the layer needs to run an x86_64 program on an arm64 computer through ahead-of-time
//! translation (D191, slice 39e2), as pure logic with host tests: the translator's input from
//! the laid-out, unrelocated image (its instruction sections, else its executable segments,
//! with every segment readable for jump tables; seeds from the entry point,
//! `LC_FUNCTION_STARTS`, the exports trie and the rebase targets that land in code), the
//! pointers that bind imports with their names, and how a variadic export of the layer is
//! entered, so its bridge can call the function that takes a `va_list` instead.

use crate::dyld_info::{self, Location, Table};
use crate::fixups::{ChainedFixups, POINTER_64, Source};
use crate::macho::{self, InvalidMachO, MachO, VM_PROT_EXECUTE, read_u32, read_u64};
use alloc::vec::Vec;
use eggos_translate::image::{Section, Seed, SeedOrigin, function_starts};

const HEADER_SIZE: usize = 32;
const SEGMENT_COMMAND_SIZE: usize = 72;
const SECTION_SIZE: usize = 80;
const LC_FUNCTION_STARTS: u32 = 0x26;
const S_ATTR_PURE_INSTRUCTIONS: u32 = 0x8000_0000;
const S_ATTR_SOME_INSTRUCTIONS: u32 = 0x400;
const EXPORT_KIND_MASK: u64 = 0x3;
const EXPORT_KIND_REGULAR: u64 = 0x0;
const EXPORT_REEXPORT: u64 = 0x8;
const CHAINED_BIND: u64 = 1 << 63;
const CHAINED_ORDINAL: u64 = 0xff_ffff;
const CHAINED_TARGET: u64 = 0xf_ffff_ffff;
const MOVE_FROM_SP: u32 = 0x9100_03E0;
const REGISTER_FIELD: u32 = 0x1F;
const BRANCH: u32 = 0x1400_0000;
const BRANCH_OPCODE: u32 = 0xFC00_0000;
const BRANCH_OFFSET_BITS: u32 = 26;
const LAST_ARGUMENT_REGISTER: u32 = 7;
const INSTRUCTION_BYTES: i64 = 4;

/// A pointer the loader binds to an imported symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportSlot<'a> {
    /// The pointer's address relative to the image base.
    pub rva: u32,
    /// The symbol's name, with its leading underscore.
    pub name: &'a [u8],
    /// Where the symbol comes from.
    pub source: Source,
    /// Whether nothing defining it is allowed.
    pub weak: bool,
    /// What the loader adds to the symbol's address; a function's call slot has none.
    pub addend: i64,
}

/// The image's rebased and bound pointers, as the translator and the loader need them.
#[derive(Debug, Default)]
pub struct PointerFixups<'a> {
    /// The targets of the rebased pointers, relative to the image base.
    pub rebase_targets: Vec<u32>,
    /// The bound pointers.
    pub binds: Vec<ImportSlot<'a>>,
}

fn load_commands<'a>(program: &MachO<'a>) -> impl Iterator<Item = (u32, &'a [u8])> + 'a {
    let bytes = program.bytes;
    let count = read_u32(bytes, 16).unwrap_or(0);
    let mut at = HEADER_SIZE;
    (0..count).map_while(move |_| {
        let kind = read_u32(bytes, at)?;
        let size = read_u32(bytes, at.checked_add(4)?)? as usize;
        let command = bytes.get(at..at.checked_add(size)?)?;
        at += size;
        Some((kind, command))
    })
}

fn instruction_sections(program: &MachO<'_>) -> Vec<(u64, u64)> {
    let mut found = Vec::new();
    for (kind, command) in load_commands(program) {
        if kind != macho::command::SEGMENT_64 {
            continue;
        }
        let protection = read_u32(command, 60).unwrap_or(0);
        let sections = read_u32(command, 64).unwrap_or(0) as usize;
        if protection & VM_PROT_EXECUTE == 0 {
            continue;
        }
        for index in 0..sections {
            let at = SEGMENT_COMMAND_SIZE + index * SECTION_SIZE;
            let flags = read_u32(command, at + 64).unwrap_or(0);
            if flags & (S_ATTR_PURE_INSTRUCTIONS | S_ATTR_SOME_INSTRUCTIONS) == 0 {
                continue;
            }
            if let (Some(address), Some(size)) =
                (read_u64(command, at + 32), read_u64(command, at + 40))
            {
                found.push((address, size));
            }
        }
    }
    found
}

/// The translator's sections of the image laid out in `image` from `program.base_address()`:
/// the sections that hold instructions (`S_ATTR_PURE_INSTRUCTIONS` or
/// `S_ATTR_SOME_INSTRUCTIONS` in an executable segment) as executable ones, or, when there are
/// none or one is outside the image, the executable segments whole; then every segment as a
/// readable one, so jump tables in `__const` are found.
pub fn sections<'a>(program: &MachO<'_>, image: &'a [u8]) -> Vec<Section<'a>> {
    let base = program.base_address();
    let piece = |address: u64, size: u64, executable: bool| -> Option<Section<'a>> {
        let rva = address.checked_sub(base)?;
        let start = usize::try_from(rva).ok()?;
        let end = start.checked_add(usize::try_from(size).ok()?)?;
        Some(Section {
            rva: u32::try_from(rva).ok()?,
            bytes: image.get(start..end)?,
            executable,
        })
    };
    let code = instruction_sections(program)
        .into_iter()
        .map(|(address, size)| piece(address, size, true))
        .collect::<Option<Vec<_>>>()
        .filter(|code| !code.is_empty())
        .unwrap_or_else(|| {
            program
                .segments()
                .iter()
                .filter(|segment| segment.initprot & VM_PROT_EXECUTE != 0)
                .filter_map(|segment| piece(segment.vmaddr, segment.vmsize, true))
                .collect()
        });
    let readable = program
        .segments()
        .iter()
        .filter_map(|segment| piece(segment.vmaddr, segment.vmsize, false));
    code.into_iter().chain(readable).collect()
}

fn linkedit<'a>(program: &MachO<'a>, wanted: u32) -> Option<&'a [u8]> {
    let (_, command) = load_commands(program).find(|&(kind, _)| kind == wanted)?;
    let offset = read_u32(command, 8)? as usize;
    let size = read_u32(command, 12)? as usize;
    program.bytes.get(offset..offset.checked_add(size)?)
}

fn uleb128(bytes: &[u8]) -> Option<(u64, usize)> {
    let mut value = 0u64;
    for (index, &byte) in bytes.iter().enumerate() {
        let shift = 7 * u32::try_from(index).ok()?;
        let bits = u64::from(byte & 0x7F);
        if shift >= 64 || (shift > 0 && bits >> (64 - shift) != 0) {
            return None;
        }
        value |= bits << shift;
        if byte & 0x80 == 0 {
            return Some((value, index + 1));
        }
    }
    None
}

/// The addresses, relative to the image base, of the regular symbols an exports trie
/// (`LC_DYLD_EXPORTS_TRIE` or `LC_DYLD_INFO`'s export area) exports; re-exports, thread-local
/// and absolute symbols are left out. `None` when the trie is malformed: a number or an edge
/// runs past it, a child offset is outside it, or it has more nodes than bytes (a cycle).
pub fn export_addresses(trie: &[u8]) -> Option<Vec<u32>> {
    let mut addresses = Vec::new();
    let mut pending = Vec::new();
    pending.push(0usize);
    let mut visited = 0usize;
    while let Some(node) = pending.pop() {
        visited += 1;
        if visited > trie.len() {
            return None;
        }
        let (terminal, used) = uleb128(trie.get(node..)?)?;
        let information = node + used;
        let children = information.checked_add(usize::try_from(terminal).ok()?)?;
        if terminal > 0 {
            let (flags, used) = uleb128(trie.get(information..)?)?;
            if flags & EXPORT_REEXPORT == 0 && flags & EXPORT_KIND_MASK == EXPORT_KIND_REGULAR {
                let (address, _) = uleb128(trie.get(information + used..)?)?;
                addresses.push(u32::try_from(address).ok()?);
            }
        }
        let count = *trie.get(children)?;
        let mut at = children + 1;
        for _ in 0..count {
            let edge = trie.get(at..)?.iter().position(|&byte| byte == 0)?;
            at += edge + 1;
            let (child, used) = uleb128(trie.get(at..)?)?;
            at += used;
            let child = usize::try_from(child).ok()?;
            if child == 0 || child >= trie.len() {
                return None;
            }
            pending.push(child);
        }
    }
    Some(addresses)
}

fn location_rva(program: &MachO<'_>, location: Location) -> Result<u32, InvalidMachO> {
    let outside = InvalidMachO("a fixup is outside its segment");
    let segment = program
        .segment_by_command_index(location.segment)
        .ok_or(outside)?;
    if location
        .offset
        .checked_add(8)
        .is_none_or(|end| end > segment.vmsize)
    {
        return Err(outside);
    }
    u32::try_from(segment.vmaddr - program.base_address() + location.offset).map_err(|_| outside)
}

/// The image's rebase targets and bound pointers, from its chained fixups (`chained`, parsed
/// from `program`) or its `LC_DYLD_INFO` opcodes, read from `image`, the program laid out from
/// `program.base_address()` before any fixup is applied, which this leaves as it is.
pub fn pointer_fixups<'a>(
    program: &MachO<'a>,
    chained: Option<&ChainedFixups<'a>>,
    image: &mut [u8],
) -> Result<PointerFixups<'a>, InvalidMachO> {
    let mut fixups = PointerFixups::default();
    let base = program.base_address();
    if let Some(chained) = chained {
        chained.walk(image, |location, raw, format| {
            let rva = u32::try_from(location)
                .map_err(|_| InvalidMachO("a chained pointer is outside the image"))?;
            if raw & CHAINED_BIND != 0 {
                let import = chained.import((raw & CHAINED_ORDINAL) as usize)?;
                fixups.binds.push(ImportSlot {
                    rva,
                    name: import.name,
                    source: import.source,
                    weak: import.weak,
                    addend: import.addend.wrapping_add(((raw >> 24) & 0xff) as i64),
                });
            } else {
                let target = raw & CHAINED_TARGET;
                let target = match format {
                    POINTER_64 => target.wrapping_sub(base),
                    _ => target,
                };
                if let Ok(target) = u32::try_from(target) {
                    fixups.rebase_targets.push(target);
                }
            }
            Ok(None)
        })?;
    } else if let Some(info) = program.dyld_info {
        let table = |range: macho::Range| {
            program
                .bytes
                .get(range.offset..range.offset + range.size)
                .unwrap_or_default()
        };
        dyld_info::rebases(table(info.rebase), |location| {
            let rva = location_rva(program, location)?;
            let value = read_u64(image, rva as usize)
                .ok_or(InvalidMachO("a fixup is outside the image"))?;
            if let Ok(target) = u32::try_from(value.wrapping_sub(base)) {
                fixups.rebase_targets.push(target);
            }
            Ok(())
        })?;
        for (range, kind) in [
            (info.bind, Table::Binds),
            (info.lazy_bind, Table::LazyBinds),
        ] {
            dyld_info::binds(table(range), kind, |bind| {
                fixups.binds.push(ImportSlot {
                    rva: location_rva(program, bind.location)?,
                    name: bind.name,
                    source: bind.source,
                    weak: bind.weak_import,
                    addend: bind.addend,
                });
                Ok(())
            })?;
        }
    }
    Ok(fixups)
}

/// The translator's seeds: the entry point, every `LC_FUNCTION_STARTS` start, the exports
/// trie's regular symbols, and the rebase targets that land in an executable section of
/// `sections` (which may be pointers to data kept in code, so the translator checks them).
pub fn seeds(program: &MachO<'_>, sections: &[Section<'_>], rebase_targets: &[u32]) -> Vec<Seed> {
    let seed = |rva: u32, origin: SeedOrigin| Seed { rva, origin };
    let mut seeds = Vec::new();
    if let Ok(entry) = u32::try_from(program.entry) {
        seeds.push(seed(entry, SeedOrigin::EntryPoint));
    }
    if let Some(starts) =
        linkedit(program, LC_FUNCTION_STARTS).and_then(|blob| function_starts(blob, 0))
    {
        seeds.extend(
            starts
                .into_iter()
                .map(|rva| seed(rva, SeedOrigin::FunctionStart)),
        );
    }
    if let Some(exports) =
        linkedit(program, macho::command::DYLD_EXPORTS_TRIE).and_then(export_addresses)
    {
        seeds.extend(exports.into_iter().map(|rva| seed(rva, SeedOrigin::Export)));
    }
    let in_code = |rva: u32| {
        sections.iter().any(|section| {
            section.executable
                && rva >= section.rva
                && u64::from(rva - section.rva) < section.bytes.len() as u64
        })
    };
    seeds.extend(
        rebase_targets
            .iter()
            .copied()
            .filter(|&rva| in_code(rva))
            .map(|rva| seed(rva, SeedOrigin::Relocation)),
    );
    seeds
}

/// How the layer's variadic export whose first two instructions are `words` takes its
/// arguments: the variadic entry of `libsystem::variadic` is `mov x<named>, sp` then `b` to
/// the function that takes the `named` arguments and a `va_list`. Returns `named` and that
/// function's offset from the entry; `None` for any other function.
pub fn variadic_entry(words: [u32; 2]) -> Option<(u8, i64)> {
    let [first, second] = words;
    let register = first & REGISTER_FIELD;
    if first & !REGISTER_FIELD != MOVE_FROM_SP || register > LAST_ARGUMENT_REGISTER {
        return None;
    }
    if second & BRANCH_OPCODE != BRANCH {
        return None;
    }
    let shift = 32 - BRANCH_OFFSET_BITS;
    let offset = i64::from(((second << shift) as i32) >> shift);
    Some((
        register as u8,
        INSTRUCTION_BYTES + offset * INSTRUCTION_BYTES,
    ))
}
