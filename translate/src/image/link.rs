use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use super::discover::{Discovery, discover};
use super::entries::EntriesTable;
use super::liveness::live_outs;
use super::runtime::{self, Routines};
use super::{ImageInput, RETURN_TO_NATIVE_RVA};
use crate::codegen::aarch64::Size;
use crate::codegen::aarch64::encode::{self, AddSub};
use crate::codegen::flags::Flags;
use crate::codegen::registers::{ADDRESS, SCRATCH};
use crate::codegen::{
    Block, BlockEnd, CodegenMode, ExitKind, LowerError, LowerErrorKind, TranslatedBlock,
    translate_block, trap,
};
use crate::{DecodeErrorKind, Refusal};

/// Why a site of the image traps instead of running.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SiteReason {
    /// The decoder refuses the instruction (an extension EggOS does not translate).
    Refused(Refusal),
    /// The instruction decodes but the code generator does not lower it yet.
    NotLowered,
}

/// An instruction that ends the program with a report if it ever runs; listed at install.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RefusedSite {
    /// Its address relative to the image base (its stub passes the guest address in `x16`).
    pub rva: u32,
    /// Its length in bytes, so the report can show its bytes.
    pub length: u8,
    /// Why.
    pub reason: SiteReason,
}

/// One instruction's code offset and guest address, for faults, unwinding and crash reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BackMapEntry {
    /// The byte offset of its first AArch64 instruction in the image's code.
    pub host: u32,
    /// Its guest address relative to the image base.
    pub rva: u32,
}

/// What translating an image found and produced.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TranslationStats {
    /// The bytes of guest instructions translated.
    pub guest_bytes: u64,
    /// The basic blocks.
    pub blocks: u32,
    /// The guest instructions.
    pub instructions: u32,
    /// The entries table's entries.
    pub entries: u32,
    /// The recognised jump tables.
    pub jump_tables: u32,
    /// The entries the linear sweep found.
    pub swept_entries: u32,
    /// The indirect jumps left to the run-time dispatch.
    pub unresolved_jumps: u32,
    /// The veneers that carry branches beyond `b`'s 128 MiB.
    pub veneers: u32,
}

/// A translated image: the code (runtime routines first, then every block, stub and veneer),
/// its tables and the refused sites. Code offsets are bytes from the code's start, which the
/// loader maps at a 4 KiB-aligned address (veneers use `adrp`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranslatedImage {
    /// The AArch64 code, little-endian.
    pub code: Vec<u8>,
    /// The byte offset of the indirect dispatch.
    pub dispatch: u32,
    /// The byte offset of the entry trampoline.
    pub enter: u32,
    /// Guest relative address to code offset, for every indirectly reachable block and for
    /// [`RETURN_TO_NATIVE_RVA`].
    pub entries: EntriesTable,
    /// Code offset to guest address for every instruction, sorted by code offset.
    pub back_map: Vec<BackMapEntry>,
    /// The sites that trap with a report, sorted by address.
    pub refused_sites: Vec<RefusedSite>,
    /// The byte offset of every 64-bit literal that holds a guest address; the loader adds
    /// the difference between the real and the preferred image base to each.
    pub guest_address_literals: Vec<u32>,
    /// What was found and produced.
    pub stats: TranslationStats,
}

/// Why an image could not be translated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageError {
    /// A block's code could not be finished (a translator bug, never the program's fault).
    Lower(LowerError),
    /// The code would pass 2 GiB, or a branch could not reach its veneer.
    TooLarge,
    /// The entries table could not hold every entry.
    TooManyEntries,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Limits {
    pub(crate) segment_words: u32,
    pub(crate) direct_reach_words: u32,
}

impl Limits {
    pub(crate) const DEFAULT: Limits = Limits {
        segment_words: 8 << 20,
        direct_reach_words: 16 << 20,
    };
}

const MAX_CODE_WORDS: u64 = 1 << 29;
const BRANCH_REACH_WORDS: i64 = 1 << 25;
const VENEER_WORDS: u32 = 4;
const PAGE_BYTES: i64 = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Destination {
    Piece(usize),
    Dispatch,
}

#[derive(Debug, Default)]
struct Piece {
    guest: Option<u64>,
    words: Vec<u32>,
    exits: Vec<(u32, ExitKind)>,
    starts: Vec<(u32, u64)>,
    literals: Vec<u32>,
}

impl Piece {
    fn from_block(block: TranslatedBlock) -> Piece {
        Piece {
            guest: Some(block.guest),
            words: block.code,
            exits: block
                .exits
                .iter()
                .map(|exit| (exit.word, exit.kind))
                .collect(),
            starts: block
                .starts
                .iter()
                .map(|start| (start.word, start.guest))
                .collect(),
            literals: block.guest_address_literals,
        }
    }

    fn trap_stub(guest: u64, code: u16) -> Piece {
        Piece {
            guest: Some(guest),
            words: alloc::vec![
                encode::load_literal(ADDRESS, 2),
                encode::brk(code),
                guest as u32,
                (guest >> 32) as u32,
            ],
            exits: Vec::new(),
            starts: alloc::vec![(0, guest)],
            literals: alloc::vec![2],
        }
    }

    fn padded_len(&self) -> u32 {
        (self.words.len() as u32).next_multiple_of(2)
    }
}

struct Translated {
    pieces: Vec<Piece>,
    sites: Vec<RefusedSite>,
}

fn site(input: &ImageInput<'_>, address: u64, length: u8, reason: SiteReason) -> RefusedSite {
    RefusedSite {
        rva: input.rva(address).unwrap_or(u32::MAX),
        length,
        reason,
    }
}

fn translate_blocks(
    input: &ImageInput<'_>,
    discovery: &Discovery,
    mode: CodegenMode,
) -> Result<Translated, ImageError> {
    let live = live_outs(discovery);
    let mut translated = Translated {
        pieces: Vec::with_capacity(discovery.blocks.len()),
        sites: Vec::new(),
    };
    for (block, live_out) in discovery.blocks.iter().zip(live) {
        if let BlockEnd::Refused {
            address,
            error: DecodeErrorKind::Unsupported { refusal, length },
        } = block.end
        {
            translated
                .sites
                .push(site(input, address, length, SiteReason::Refused(refusal)));
        }
        match translate_block(block, mode, live_out) {
            Ok(code) => translated.pieces.push(Piece::from_block(code)),
            Err(LowerError {
                address,
                kind: LowerErrorKind::Unhandled(_) | LowerErrorKind::Operands(_),
            }) => not_lowered(input, block, address, mode, &mut translated)?,
            Err(error) => return Err(ImageError::Lower(error)),
        }
    }
    Ok(translated)
}

fn not_lowered(
    input: &ImageInput<'_>,
    block: &Block,
    address: u64,
    mode: CodegenMode,
    translated: &mut Translated,
) -> Result<(), ImageError> {
    let kept: Vec<_> = block
        .instructions
        .iter()
        .take_while(|instruction| instruction.address != address)
        .copied()
        .collect();
    let length = block
        .instructions
        .get(kept.len())
        .map_or(1, |instruction| instruction.length);
    if !kept.is_empty() {
        let head = Block {
            start: block.start,
            instructions: kept,
            end: BlockEnd::FallThrough(address),
        };
        let code = translate_block(&head, mode, Flags::ALL).map_err(ImageError::Lower)?;
        translated.pieces.push(Piece::from_block(code));
    }
    translated
        .sites
        .push(site(input, address, length, SiteReason::NotLowered));
    translated
        .pieces
        .push(Piece::trap_stub(address, trap::REFUSED));
    Ok(())
}

fn add_undiscovered_stubs(pieces: &mut Vec<Piece>) -> BTreeMap<u64, usize> {
    let mut by_guest: BTreeMap<u64, usize> = pieces
        .iter()
        .enumerate()
        .filter_map(|(index, piece)| piece.guest.map(|guest| (guest, index)))
        .collect();
    let mut missing = BTreeSet::new();
    for piece in pieces.iter() {
        for &(_, kind) in &piece.exits {
            if let ExitKind::Jump(target) = kind
                && !by_guest.contains_key(&target)
            {
                missing.insert(target);
            }
        }
    }
    for target in missing {
        by_guest.insert(target, pieces.len());
        pieces.push(Piece::trap_stub(target, trap::UNDISCOVERED));
    }
    by_guest
}

struct Layout {
    offsets: Vec<u32>,
    veneers: Vec<(u32, u32)>,
    exit_veneer: BTreeMap<(usize, usize), usize>,
    total_words: u32,
}

fn destination(kind: ExitKind, by_guest: &BTreeMap<u64, usize>) -> Destination {
    match kind {
        ExitKind::Jump(target) => by_guest
            .get(&target)
            .map_or(Destination::Dispatch, |&piece| Destination::Piece(piece)),
        ExitKind::Indirect => Destination::Dispatch,
    }
}

fn place(pieces: &[Piece], start: u32, islands: &BTreeMap<usize, u32>) -> (Vec<u32>, u32) {
    let mut offsets = Vec::with_capacity(pieces.len());
    let mut position = start;
    for (index, piece) in pieces.iter().enumerate() {
        offsets.push(position);
        position += piece.padded_len();
        if let Some(&veneers) = islands.get(&index) {
            position += veneers * VENEER_WORDS;
        }
    }
    (offsets, position)
}

fn lay_out(
    pieces: &[Piece],
    by_guest: &BTreeMap<u64, usize>,
    routines: &Routines,
    limits: Limits,
) -> Result<Layout, ImageError> {
    let start = routines.words.len() as u32;
    let target_word = |offsets: &[u32], destination: Destination| match destination {
        Destination::Piece(piece) => offsets[piece],
        Destination::Dispatch => routines.dispatch,
    };
    let (first, _) = place(pieces, start, &BTreeMap::new());
    let mut far: Vec<(usize, usize, Destination)> = Vec::new();
    for (index, piece) in pieces.iter().enumerate() {
        for (exit, &(word, kind)) in piece.exits.iter().enumerate() {
            let destination = destination(kind, by_guest);
            let at = i64::from(first[index] + word);
            let target = i64::from(target_word(&first, destination));
            if (target - at).abs() >= i64::from(limits.direct_reach_words) {
                far.push((index, exit, destination));
            }
        }
    }
    let mut island_after: BTreeMap<usize, u32> = BTreeMap::new();
    let mut island_of_piece = Vec::with_capacity(pieces.len());
    if !far.is_empty() {
        let mut segment_start = start;
        let mut segment_pieces = Vec::new();
        for (index, &offset) in first.iter().enumerate() {
            if offset - segment_start >= limits.segment_words && !segment_pieces.is_empty() {
                let last = index - 1;
                for &piece in &segment_pieces {
                    island_of_piece.push((piece, last));
                }
                segment_pieces.clear();
                segment_start = offset;
            }
            segment_pieces.push(index);
        }
        let last = pieces.len().saturating_sub(1);
        for &piece in &segment_pieces {
            island_of_piece.push((piece, last));
        }
        let island_for: BTreeMap<usize, usize> = island_of_piece.iter().copied().collect();
        for &(piece, _, _) in &far {
            *island_after.entry(island_for[&piece]).or_insert(0) += 1;
        }
    }
    let (offsets, total_words) = place(pieces, start, &island_after);
    if u64::from(total_words) > MAX_CODE_WORDS {
        return Err(ImageError::TooLarge);
    }
    let island_for: BTreeMap<usize, usize> = island_of_piece.iter().copied().collect();
    let mut next_in_island: BTreeMap<usize, u32> = BTreeMap::new();
    let mut veneers = Vec::with_capacity(far.len());
    let mut exit_veneer = BTreeMap::new();
    for &(piece, exit, destination) in &far {
        let island = island_for[&piece];
        let island_start = offsets[island] + pieces[island].padded_len();
        let slot = next_in_island.entry(island).or_insert(0);
        let position = island_start + *slot * VENEER_WORDS;
        *slot += 1;
        exit_veneer.insert((piece, exit), veneers.len());
        veneers.push((position, target_word(&offsets, destination)));
    }
    Ok(Layout {
        offsets,
        veneers,
        exit_veneer,
        total_words,
    })
}

fn branch(at: u32, target: u32) -> Result<u32, ImageError> {
    let delta = i64::from(target) - i64::from(at);
    if !(-BRANCH_REACH_WORDS..BRANCH_REACH_WORDS).contains(&delta) {
        return Err(ImageError::TooLarge);
    }
    Ok(encode::b(delta as i32))
}

fn veneer(at: u32, target: u32) -> [u32; VENEER_WORDS as usize] {
    let at_bytes = i64::from(at) * 4;
    let target_bytes = i64::from(target) * 4;
    let pages = target_bytes / PAGE_BYTES - at_bytes / PAGE_BYTES;
    [
        encode::adrp(SCRATCH, pages as i32),
        encode::add_sub_immediate(
            AddSub::Add,
            Size::X,
            SCRATCH,
            SCRATCH,
            (target_bytes % PAGE_BYTES) as u16,
            false,
        ),
        encode::br(SCRATCH),
        encode::nop(),
    ]
}

/// Translates a whole image for a CPU described by `mode`: discovery, cross-block flag
/// liveness, the lowering of every block, the layout (runtime routines, blocks, stubs for
/// sites the code generator does not lower and for branches to undiscovered addresses, and
/// veneers where a branch is beyond 128 MiB), the linking of every exit into a branch, and
/// the tables.
pub fn translate_image(
    input: &ImageInput<'_>,
    mode: CodegenMode,
) -> Result<TranslatedImage, ImageError> {
    translate_image_with(input, mode, Limits::DEFAULT)
}

pub(crate) fn translate_image_with(
    input: &ImageInput<'_>,
    mode: CodegenMode,
    limits: Limits,
) -> Result<TranslatedImage, ImageError> {
    let discovery = discover(input);
    let Translated {
        mut pieces,
        mut sites,
    } = translate_blocks(input, &discovery, mode)?;
    let by_guest = add_undiscovered_stubs(&mut pieces);
    let bits = EntriesTable::bits_for(discovery.entries.len() + 1);
    let routines = runtime::routines(bits).map_err(|error| {
        ImageError::Lower(LowerError {
            address: input.image_base,
            kind: LowerErrorKind::Emit(error),
        })
    })?;
    let layout = lay_out(&pieces, &by_guest, &routines, limits)?;
    let mut words = Vec::with_capacity(layout.total_words as usize);
    words.extend_from_slice(&routines.words);
    let mut veneer_words = Vec::new();
    for &(at, target) in &layout.veneers {
        veneer_words.push((at, veneer(at, target)));
    }
    let mut back_map = Vec::new();
    let mut guest_address_literals = Vec::new();
    let mut stats = TranslationStats {
        blocks: discovery.blocks.len() as u32,
        jump_tables: discovery.jump_tables.len() as u32,
        swept_entries: discovery.swept_entries,
        unresolved_jumps: discovery.unresolved_jumps,
        veneers: layout.veneers.len() as u32,
        ..TranslationStats::default()
    };
    for block in &discovery.blocks {
        stats.instructions += block.instructions.len() as u32;
        stats.guest_bytes += block
            .instructions
            .iter()
            .map(|instruction| u64::from(instruction.length))
            .sum::<u64>();
    }
    for (index, piece) in pieces.iter().enumerate() {
        let offset = layout.offsets[index];
        words.resize(offset as usize, encode::nop());
        let mut code = piece.words.clone();
        for (exit, &(word, kind)) in piece.exits.iter().enumerate() {
            let at = offset + word;
            let target = match layout.exit_veneer.get(&(index, exit)) {
                Some(&veneer) => layout.veneers[veneer].0,
                None => match destination(kind, &by_guest) {
                    Destination::Piece(target) => layout.offsets[target],
                    Destination::Dispatch => routines.dispatch,
                },
            };
            code[word as usize] = branch(at, target)?;
        }
        words.extend_from_slice(&code);
        for &(word, guest) in &piece.starts {
            if let Some(rva) = input.rva(guest) {
                back_map.push(BackMapEntry {
                    host: (offset + word) * 4,
                    rva,
                });
            }
        }
        for &word in &piece.literals {
            guest_address_literals.push((offset + word) * 4);
        }
    }
    words.resize(layout.total_words as usize, encode::nop());
    for (at, veneer) in veneer_words {
        let at = at as usize;
        words[at..at + veneer.len()].copy_from_slice(&veneer);
    }
    let mut entries = EntriesTable::with_bits(bits);
    let return_to_native = routines.return_to_native * 4;
    if !entries.insert(RETURN_TO_NATIVE_RVA, return_to_native) {
        return Err(ImageError::TooManyEntries);
    }
    for &entry in &discovery.entries {
        let (Some(rva), Some(&piece)) = (input.rva(entry), by_guest.get(&entry)) else {
            continue;
        };
        if !entries.insert(rva, layout.offsets[piece] * 4) {
            return Err(ImageError::TooManyEntries);
        }
    }
    stats.entries = entries.len() as u32;
    sites.sort_by_key(|site| site.rva);
    let mut code = Vec::with_capacity(words.len() * 4);
    for word in words {
        code.extend_from_slice(&word.to_le_bytes());
    }
    Ok(TranslatedImage {
        code,
        dispatch: routines.dispatch * 4,
        enter: routines.enter * 4,
        entries,
        back_map,
        refused_sites: sites,
        guest_address_literals,
        stats,
    })
}

#[cfg(test)]
pub(crate) fn veneer_target(at: u32, words: &[u32]) -> Option<u32> {
    let adrp = words.first()?;
    let add = words.get(1)?;
    if adrp & 0x9F00_0000 != 0x9000_0000 || add & 0xFFC0_0000 != 0x9100_0000 {
        return None;
    }
    let immlo = (adrp >> 29) & 3;
    let immhi = (adrp >> 5) & 0x7_FFFF;
    let pages = (((immhi << 2) | immlo) << 11) as i32 >> 11;
    let page = (i64::from(at) * 4 / PAGE_BYTES + i64::from(pages)) * PAGE_BYTES;
    let low = i64::from((add >> 10) & 0xFFF);
    u32::try_from((page + low) / 4).ok()
}
