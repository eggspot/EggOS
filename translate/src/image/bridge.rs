//! The calling-convention bridges between translated x64 code and the native AArch64 code of
//! a layer (slice 39d2 of D191).
//!
//! A guest call to an imported function goes through the import address table, whose slot
//! the loader fills with the slot's own guest address. The entries table maps that address to
//! an import stub in the translated code ([`add_import_stubs`]), which jumps to the forward
//! bridge the loader generated for the import ([`forward_bridge`]): it keeps the registers
//! Win64 says a callee keeps but AAPCS64 does not, moves the Win64 arguments to their AAPCS64
//! places ([`argument_moves`]), calls the native function with the default FPCR, restores the
//! guest's state and returns to the guest as a translated `ret` does. Native code calls guest
//! functions through the entry trampoline of [`super::runtime`]; the bridge records the guest
//! stack and shadow stack positions in the thread block ([`thread`]) so a callback continues
//! below them.

use alloc::vec::Vec;

use super::entries::{EMPTY_KEY, EntriesTable};
use super::link::{ImageError, TranslatedImage};
use super::runtime;
use crate::codegen::aarch64::emit::{Assembler, EmitError};
use crate::codegen::aarch64::encode::{self, AddSub, Bitfield, LoadStore, Logic, Pair};
use crate::codegen::aarch64::simd::{self, VectorLoadStore, vector_load_store};
use crate::codegen::aarch64::{Reg, Shift, Size};
use crate::codegen::registers::{
    ADDRESS, GS_BASE, GUEST_STACK, SCRATCH, SHADOW_STACK, STATE, TEMPORARIES,
};
use crate::codegen::state;

/// The byte offsets of the per-thread block `x21` points to: the guest state block of
/// [`crate::codegen::state`], then the fields the bridges keep.
pub mod thread {
    use crate::codegen::state;

    /// The guest `rsp` when the thread last called native code: a native call back into
    /// guest code runs below it.
    pub const NATIVE_CALL_GUEST_STACK: u32 = state::SIZE;
    /// The shadow stack position (`x20`) when the thread last called native code.
    pub const NATIVE_CALL_SHADOW_STACK: u32 = NATIVE_CALL_GUEST_STACK + 8;
    /// The block's size in bytes.
    pub const SIZE: u32 = NATIVE_CALL_SHADOW_STACK + 8;
}

/// The bytes of one import stub in the translated code.
pub const IMPORT_STUB_BYTES: u32 = 16;
/// Where, in an import stub, the loader writes the bridge's address.
pub const IMPORT_STUB_LITERAL: u32 = 8;
/// The host registers of Win64's first four integer arguments: `rcx`, `rdx`, `r8`, `r9`.
pub const WIN64_ARGUMENT_REGISTERS: [u8; 4] = [1, 2, 8, 9];
/// Where the fifth Win64 argument is, from the guest `rsp` at the call: after the return
/// address and the 32 bytes of home space.
pub const WIN64_FIRST_STACK_ARGUMENT: u32 = 40;
/// The most arguments a bridge moves.
pub const MAX_ARGUMENTS: usize = 32;
/// The arguments a bridge for a function of unknown signature moves: fourteen integers,
/// which covers every export the layers have, variadic ones included.
pub const DEFAULT_ARGUMENTS: usize = 14;
const AAPCS64_ARGUMENT_REGISTERS: u8 = 8;
const SAVED_GENERAL_BYTES: u32 = 80;
const SAVED_VECTOR_BYTES: u32 = 160;
const FIRST_SAVED_VECTOR: u8 = 6;
const SP: Reg = Reg(31);
const LINK_TARGET: Reg = SCRATCH;
const STACK_BASE: Reg = ADDRESS;

/// How a function takes one argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ArgumentKind {
    /// An integer or pointer of up to 64 bits (or a small structure passed as one).
    Integer,
    /// A `float` or `double`.
    FloatingPoint,
}

/// Where a Win64 argument is when the bridge starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    /// A host general register (`rcx` is `x1`, `rdx` `x2`, `r8` `x8`, `r9` `x9`).
    Register(u8),
    /// A vector register (`xmm<n>` is `v<n>`).
    Vector(u8),
    /// The guest stack, this many bytes above the guest `rsp` at the call.
    GuestStack(u32),
}

/// Where an AAPCS64 argument goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Destination {
    /// `x0` to `x7`.
    Register(u8),
    /// `v0` to `v7`.
    Vector(u8),
    /// The native stack, this many bytes above `sp` at the call.
    NativeStack(u32),
}

/// One argument's move.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ArgumentMove {
    /// Where it is.
    pub source: Source,
    /// Where it goes.
    pub destination: Destination,
}

/// The moves that take arguments of `kinds` (at most [`MAX_ARGUMENTS`]) from a Win64 call to
/// an AAPCS64 one, in an order that reads every source before it is overwritten.
///
/// Win64 gives each of the first four arguments the register of its position, integer or
/// vector, and the rest stack slots; AAPCS64 counts integers and floating-point values apart.
/// A variadic function is an all-integer one: Win64 passes its floating-point values in the
/// integer registers too, and Windows on Arm takes them there.
pub fn argument_moves(kinds: &[ArgumentKind]) -> Vec<ArgumentMove> {
    let mut moves = Vec::with_capacity(kinds.len().min(MAX_ARGUMENTS));
    let mut integers = 0u8;
    let mut vectors = 0u8;
    let mut stack = 0u32;
    for (position, &kind) in kinds.iter().take(MAX_ARGUMENTS).enumerate() {
        let source = match (position, kind) {
            (0..4, ArgumentKind::Integer) => Source::Register(WIN64_ARGUMENT_REGISTERS[position]),
            (0..4, ArgumentKind::FloatingPoint) => Source::Vector(position as u8),
            _ => Source::GuestStack(WIN64_FIRST_STACK_ARGUMENT + 8 * (position as u32 - 4)),
        };
        let destination = match kind {
            ArgumentKind::Integer if integers < AAPCS64_ARGUMENT_REGISTERS => {
                integers += 1;
                Destination::Register(integers - 1)
            }
            ArgumentKind::FloatingPoint if vectors < AAPCS64_ARGUMENT_REGISTERS => {
                vectors += 1;
                Destination::Vector(vectors - 1)
            }
            _ => {
                stack += 8;
                Destination::NativeStack(stack - 8)
            }
        };
        moves.push(ArgumentMove {
            source,
            destination,
        });
    }
    moves
}

/// The native stack the moves need for arguments, rounded up to 16 bytes.
pub fn native_stack_bytes(moves: &[ArgumentMove]) -> u32 {
    let end = moves
        .iter()
        .filter_map(|step| match step.destination {
            Destination::NativeStack(offset) => Some(offset + 8),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    end.next_multiple_of(16)
}

fn load(t: Reg, n: Reg, offset: u32) -> Result<u32, EmitError> {
    encode::load_store(LoadStore::LdrX, t, n, offset).ok_or(EmitError::OutOfRange { at: 0 })
}

fn store(t: Reg, n: Reg, offset: u32) -> Result<u32, EmitError> {
    encode::load_store(LoadStore::StrX, t, n, offset).ok_or(EmitError::OutOfRange { at: 0 })
}

fn vector(op: VectorLoadStore, v: u8, n: Reg, offset: u32) -> Result<u32, EmitError> {
    vector_load_store(op, v, n, offset).ok_or(EmitError::OutOfRange { at: 0 })
}

fn copy(d: Reg, n: Reg) -> u32 {
    encode::logical_shifted(Logic::Orr, false, Size::X, d, Reg::ZR, n, Shift::Lsl, 0)
}

const SAVED_PAIRS: [(Reg, Reg); 4] = [
    (Reg(3), GUEST_STACK),
    (Reg(5), Reg(6)),
    (Reg(7), Reg(12)),
    (Reg(13), Reg(14)),
];

fn emit_move(asm: &mut Assembler, step: ArgumentMove) -> Result<(), EmitError> {
    match (step.source, step.destination) {
        (Source::Register(from), Destination::Register(to)) => {
            asm.emit(copy(Reg(to), Reg(from)));
        }
        (Source::Vector(from), Destination::Vector(to)) => {
            if from != to {
                asm.emit(simd::vector_move(to, from));
            }
        }
        (Source::GuestStack(offset), Destination::Register(to)) => {
            asm.emit(load(Reg(to), STACK_BASE, offset)?);
        }
        (Source::GuestStack(offset), Destination::Vector(to)) => {
            asm.emit(vector(VectorLoadStore::LdrD, to, STACK_BASE, offset)?);
        }
        (Source::GuestStack(offset), Destination::NativeStack(at)) => {
            asm.emit(load(LINK_TARGET, STACK_BASE, offset)?);
            asm.emit(store(LINK_TARGET, SP, at)?);
        }
        (Source::Register(from), Destination::NativeStack(at)) => {
            asm.emit(store(Reg(from), SP, at)?);
        }
        (Source::Vector(from), Destination::NativeStack(at)) => {
            asm.emit(vector(VectorLoadStore::StrD, from, SP, at)?);
        }
        (Source::Register(_), Destination::Vector(_))
        | (Source::Vector(_), Destination::Register(_)) => {
            return Err(EmitError::OutOfRange { at: asm.position() });
        }
    }
    Ok(())
}

/// The forward bridge for a native function at `native` taking arguments as `moves` says,
/// for an image whose indirect dispatch is at `dispatch`: position-independent AArch64 words
/// with the two addresses in a literal pool at the end.
///
/// The import stub jumps to it with the guest return address at the guest `rsp` and the
/// call's pair on the shadow stack. It saves `rbx`, `rsp`, `rbp`, `rsi`, `rdi`, `r12`-`r15`,
/// `xmm6`-`xmm15` and FPCR on the native stack, records the guest `rsp` and `x20` in the
/// thread block, clears FPCR, moves the arguments, calls the function (its result is already
/// in `rax`/`x0` or `xmm0`/`v0`), restores all of it, reloads `x18` from the state block and
/// returns to the guest: through the shadow stack when its pair matches, else through the
/// dispatch.
pub fn forward_bridge(
    moves: &[ArgumentMove],
    native: u64,
    dispatch: u64,
) -> Result<Vec<u32>, EmitError> {
    let outgoing = native_stack_bytes(moves);
    let saved = outgoing;
    let saved_vectors = saved + SAVED_GENERAL_BYTES;
    let frame = saved_vectors + SAVED_VECTOR_BYTES;
    let frame = u16::try_from(frame).map_err(|_| EmitError::OutOfRange { at: 0 })?;
    let mut asm = Assembler::new();
    let native = asm.literal(native);
    let dispatch = asm.literal(dispatch);
    let miss = asm.label();
    asm.emit(encode::add_sub_immediate(
        AddSub::Sub,
        Size::X,
        SP,
        SP,
        frame,
        false,
    ));
    for (index, (first, second)) in SAVED_PAIRS.into_iter().enumerate() {
        asm.emit(encode::pair(
            Pair::Stp,
            first,
            second,
            SP,
            (saved + 16 * index as u32) as i32,
        ));
    }
    asm.emit(simd::mrs_fpcr(LINK_TARGET));
    asm.emit(encode::pair(
        Pair::Stp,
        Reg(15),
        LINK_TARGET,
        SP,
        (saved + 64) as i32,
    ));
    for register in FIRST_SAVED_VECTOR..16 {
        let at = saved_vectors + 16 * u32::from(register - FIRST_SAVED_VECTOR);
        asm.emit(vector(VectorLoadStore::StrQ, register, SP, at)?);
    }
    asm.emit(store(GUEST_STACK, STATE, thread::NATIVE_CALL_GUEST_STACK)?);
    asm.emit(store(
        SHADOW_STACK,
        STATE,
        thread::NATIVE_CALL_SHADOW_STACK,
    )?);
    asm.emit(simd::msr_fpcr(Reg::ZR));
    asm.emit(copy(STACK_BASE, GUEST_STACK));
    for &step in moves {
        emit_move(&mut asm, step)?;
    }
    asm.load_literal(LINK_TARGET, native);
    asm.emit(encode::blr(LINK_TARGET));
    for (index, (first, second)) in SAVED_PAIRS.into_iter().enumerate() {
        asm.emit(encode::pair(
            Pair::Ldp,
            first,
            second,
            SP,
            (saved + 16 * index as u32) as i32,
        ));
    }
    asm.emit(encode::pair(
        Pair::Ldp,
        Reg(15),
        LINK_TARGET,
        SP,
        (saved + 64) as i32,
    ));
    asm.emit(simd::msr_fpcr(LINK_TARGET));
    for register in FIRST_SAVED_VECTOR..16 {
        let at = saved_vectors + 16 * u32::from(register - FIRST_SAVED_VECTOR);
        asm.emit(vector(VectorLoadStore::LdrQ, register, SP, at)?);
    }
    asm.emit(encode::add_sub_immediate(
        AddSub::Add,
        Size::X,
        SP,
        SP,
        frame,
        false,
    ));
    asm.emit(load(GS_BASE, STATE, state::GS_BASE)?);
    emit_return_to_guest(&mut asm, miss)?;
    asm.bind(miss);
    asm.load_literal(LINK_TARGET, dispatch);
    asm.emit(encode::br(LINK_TARGET));
    Ok(asm.finish()?.words)
}

fn emit_return_to_guest(
    asm: &mut Assembler,
    miss: crate::codegen::aarch64::emit::Label,
) -> Result<(), EmitError> {
    let [host, guest, _, next, ..] = TEMPORARIES;
    asm.emit(load(ADDRESS, GUEST_STACK, 0)?);
    asm.emit(encode::add_sub_immediate(
        AddSub::Add,
        Size::X,
        GUEST_STACK,
        GUEST_STACK,
        8,
        false,
    ));
    asm.emit(encode::pair(Pair::Ldp, guest, host, SHADOW_STACK, 0));
    asm.emit(encode::add_sub_immediate(
        AddSub::Add,
        Size::X,
        next,
        SHADOW_STACK,
        runtime::shadow_stack::ENTRY_BYTES as u16,
        false,
    ));
    asm.emit(encode::bitfield(
        Bitfield::Bfm,
        Size::X,
        SHADOW_STACK,
        next,
        0,
        15,
    ));
    asm.emit(encode::logical_shifted(
        Logic::Eor,
        false,
        Size::X,
        guest,
        guest,
        ADDRESS,
        Shift::Lsl,
        0,
    ));
    asm.cbz(Size::X, guest, miss, true);
    asm.emit(encode::br(host));
    Ok(())
}

/// The words of one import stub: a jump to the address in its literal, which the loader
/// writes at [`IMPORT_STUB_LITERAL`].
pub fn import_stub(bridge: u64) -> [u32; 4] {
    [
        encode::load_literal(LINK_TARGET, 2),
        encode::br(LINK_TARGET),
        bridge as u32,
        (bridge >> 32) as u32,
    ]
}

/// Adds an import stub for every import address table slot in `slot_rvas` to `image`'s code
/// and maps each slot's relative address to its stub in the entries table, growing the table
/// (and rewriting the dispatch for the new size) when it needs room. The stubs' literals stay
/// zero until the loader writes each bridge's address; [`EntriesTable::lookup`] of a slot's
/// relative address gives its stub's offset.
pub fn add_import_stubs(image: &mut TranslatedImage, slot_rvas: &[u32]) -> Result<(), ImageError> {
    let mut fresh: Vec<u32> = slot_rvas
        .iter()
        .copied()
        .filter(|&rva| rva != EMPTY_KEY && image.entries.lookup(rva).is_none())
        .collect();
    fresh.sort_unstable();
    fresh.dedup();
    if fresh.is_empty() {
        return Ok(());
    }
    let present: Vec<(u32, u32)> = image
        .entries
        .slots()
        .iter()
        .filter(|&&slot| slot as u32 != EMPTY_KEY)
        .map(|&slot| (slot as u32, (slot >> 32) as u32))
        .collect();
    let old_bits = image.entries.bits();
    let bits = EntriesTable::bits_for(present.len() + fresh.len() + 1).max(old_bits);
    if bits != old_bits {
        let old = runtime::routines(old_bits).map_err(|_| ImageError::TooManyEntries)?;
        let new = runtime::routines(bits).map_err(|_| ImageError::TooManyEntries)?;
        let bytes = new.words.len() * 4;
        if new.words.len() != old.words.len()
            || new.enter != old.enter
            || new.return_to_native != old.return_to_native
            || image.code.len() < bytes
        {
            return Err(ImageError::TooManyEntries);
        }
        for (index, word) in new.words.iter().enumerate() {
            image.code[index * 4..index * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }
    }
    let mut table = EntriesTable::with_bits(bits);
    for (rva, host) in present {
        if !table.insert(rva, host) {
            return Err(ImageError::TooManyEntries);
        }
    }
    if !image.code.len().is_multiple_of(8) {
        image.code.extend_from_slice(&encode::nop().to_le_bytes());
    }
    for rva in fresh {
        let at = u32::try_from(image.code.len()).map_err(|_| ImageError::TooLarge)?;
        for word in import_stub(0) {
            image.code.extend_from_slice(&word.to_le_bytes());
        }
        if !table.insert(rva, at) {
            return Err(ImageError::TooManyEntries);
        }
    }
    image.stats.entries = table.len() as u32;
    image.entries = table;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::CodegenMode;
    use crate::image::{ImageInput, Section, Seed, SeedOrigin, translate_image};

    const INTEGER: ArgumentKind = ArgumentKind::Integer;
    const FLOAT: ArgumentKind = ArgumentKind::FloatingPoint;

    #[test]
    fn integer_arguments_take_registers_then_the_guest_stack_then_the_native_stack() {
        let moves = argument_moves(&[INTEGER; DEFAULT_ARGUMENTS]);
        let expected_sources = [
            Source::Register(1),
            Source::Register(2),
            Source::Register(8),
            Source::Register(9),
            Source::GuestStack(40),
            Source::GuestStack(48),
        ];
        for (step, source) in moves.iter().zip(expected_sources) {
            assert_eq!(step.source, source);
        }
        for (index, step) in moves.iter().take(8).enumerate() {
            assert_eq!(step.destination, Destination::Register(index as u8));
        }
        assert_eq!(moves[8].source, Source::GuestStack(72));
        assert_eq!(moves[8].destination, Destination::NativeStack(0));
        assert_eq!(moves[13].source, Source::GuestStack(112));
        assert_eq!(moves[13].destination, Destination::NativeStack(40));
        assert_eq!(native_stack_bytes(&moves), 48);
    }

    #[test]
    fn floating_point_arguments_are_counted_apart_from_integers() {
        let ldexp = argument_moves(&[FLOAT, INTEGER]);
        assert_eq!(ldexp[0].source, Source::Vector(0));
        assert_eq!(ldexp[0].destination, Destination::Vector(0));
        assert_eq!(ldexp[1].source, Source::Register(2));
        assert_eq!(ldexp[1].destination, Destination::Register(0));
        let line = argument_moves(&[INTEGER, INTEGER, FLOAT, FLOAT, FLOAT, FLOAT]);
        let destinations: Vec<Destination> = line.iter().map(|step| step.destination).collect();
        assert_eq!(
            destinations,
            [
                Destination::Register(0),
                Destination::Register(1),
                Destination::Vector(0),
                Destination::Vector(1),
                Destination::Vector(2),
                Destination::Vector(3),
            ]
        );
        assert_eq!(line[2].source, Source::Vector(2));
        assert_eq!(line[5].source, Source::GuestStack(48));
        assert_eq!(native_stack_bytes(&line), 0);
    }

    #[test]
    fn a_forward_bridge_saves_calls_restores_and_returns_through_the_shadow_stack() {
        let moves = argument_moves(&[INTEGER; DEFAULT_ARGUMENTS]);
        let native = 0x0000_7F00_1234_5678;
        let dispatch = 0x0000_7E00_0000_0040;
        let words = forward_bridge(&moves, native, dispatch).unwrap();
        let frame = 48 + SAVED_GENERAL_BYTES + SAVED_VECTOR_BYTES;
        assert_eq!(
            words[0],
            encode::add_sub_immediate(AddSub::Sub, Size::X, SP, SP, frame as u16, false)
        );
        let count = |word: u32| words.iter().filter(|&&w| w == word).count();
        assert_eq!(count(encode::blr(SCRATCH)), 1);
        assert_eq!(count(simd::msr_fpcr(Reg::ZR)), 1);
        assert_eq!(count(simd::mrs_fpcr(SCRATCH)), 1);
        assert_eq!(count(simd::msr_fpcr(SCRATCH)), 1);
        assert_eq!(count(encode::br(TEMPORARIES[0])), 1);
        assert_eq!(count(encode::br(SCRATCH)), 1);
        assert_eq!(
            count(load(GS_BASE, STATE, state::GS_BASE).unwrap()),
            1,
            "x18 comes back from the state block"
        );
        assert_eq!(
            count(store(GUEST_STACK, STATE, thread::NATIVE_CALL_GUEST_STACK).unwrap()),
            1
        );
        let blr = words
            .iter()
            .position(|&w| w == encode::blr(SCRATCH))
            .unwrap();
        let restore = words
            .iter()
            .position(|&w| w == encode::pair(Pair::Ldp, Reg(3), GUEST_STACK, SP, 48))
            .unwrap();
        assert!(restore > blr);
        let tail = &words[words.len() - 4..];
        assert_eq!(u64::from(tail[0]) | u64::from(tail[1]) << 32, native);
        assert_eq!(u64::from(tail[2]) | u64::from(tail[3]) << 32, dispatch);
    }

    #[test]
    fn identical_vector_places_need_no_move() {
        let sqrt = argument_moves(&[FLOAT]);
        let with = forward_bridge(&sqrt, 1, 2).unwrap();
        let without = forward_bridge(&[], 1, 2).unwrap();
        assert_eq!(with, without);
    }

    #[test]
    fn import_stubs_are_entries_and_the_dispatch_follows_a_grown_table() {
        let code = [0xC3u8];
        let sections = [Section {
            rva: 0x1000,
            bytes: &code,
            executable: true,
        }];
        let seeds = [Seed {
            rva: 0x1000,
            origin: SeedOrigin::EntryPoint,
        }];
        let input = ImageInput {
            image_base: 0x1_4000_0000,
            sections: &sections,
            seeds: &seeds,
        };
        let mut image = translate_image(&input, CodegenMode::default()).unwrap();
        let entry = image.entries.lookup(0x1000).unwrap();
        let old_bits = image.entries.bits();
        let slots: Vec<u32> = (0..40u32).map(|index| 0x2000 + 8 * index).collect();
        add_import_stubs(&mut image, &slots).unwrap();
        assert!(image.entries.bits() > old_bits);
        assert_eq!(image.entries.lookup(0x1000), Some(entry));
        assert_eq!(image.entries.len(), 42);
        let routines = runtime::routines(image.entries.bits()).unwrap();
        for (index, word) in routines.words.iter().enumerate() {
            let at = index * 4;
            assert_eq!(&image.code[at..at + 4], &word.to_le_bytes());
        }
        for &slot in &slots {
            let at = image.entries.lookup(slot).unwrap() as usize;
            assert!(at.is_multiple_of(8));
            let expected = import_stub(0);
            for (index, word) in expected.iter().enumerate() {
                let byte = at + 4 * index;
                assert_eq!(&image.code[byte..byte + 4], &word.to_le_bytes());
            }
        }
        let length = image.code.len();
        add_import_stubs(&mut image, &slots).unwrap();
        assert_eq!(image.code.len(), length);
    }
}
