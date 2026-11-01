//! The run-time routines every translated image carries at the start of its code: the
//! indirect dispatch, and the entry trampoline through which native code (the layers) runs
//! guest code and gets control back when it returns.

use alloc::vec::Vec;

use super::entries::HASH_MULTIPLIER;
use crate::codegen::aarch64::emit::{Assembler, EmitError, Label};
use crate::codegen::aarch64::encode::{
    self, AddSub, Bitfield, LoadStore, Logic, MoveWide, Multiply, Pair,
};
use crate::codegen::aarch64::simd::{VectorLoadStore, vector_load_store};
use crate::codegen::aarch64::{Reg, Shift, Size};
use crate::codegen::registers::{ADDRESS, ENTRIES, GS_BASE, SCRATCH, SHADOW_STACK, STATE};
use crate::codegen::trap;

/// The byte offsets of the table `x19` points to at run time.
pub mod entries_table {
    /// The guest image's base address at run time, written by the loader.
    pub const GUEST_BASE: u32 = 0;
    /// The translated code's base address at run time, written by the loader.
    pub const HOST_BASE: u32 = 8;
    /// The first slot: each is a little-endian `u64` with the relative address in the low
    /// half and the code offset in the high half.
    pub const SLOTS: u32 = 16;
}

/// The byte offsets of the guest context the entry trampoline reads and writes.
pub mod context {
    /// `rax` to `r15`, eight bytes each, in register-number order.
    pub const GENERAL: u32 = 0;
    /// The guest address to run; on return, the address the guest returned to.
    pub const RIP: u32 = 128;
    /// The `gs:` base (`x18`).
    pub const GS_BASE: u32 = 136;
    /// The image's entries table (`x19`).
    pub const ENTRIES: u32 = 144;
    /// The thread's return shadow stack position (`x20`), updated on return.
    pub const SHADOW_STACK: u32 = 152;
    /// The thread's guest state block (`x21`).
    pub const STATE: u32 = 160;
    /// `xmm0` to `xmm15`, sixteen bytes each.
    pub const XMM: u32 = 176;
    /// The context's size in bytes; it must be 16-byte aligned.
    pub const SIZE: u32 = 432;
}

/// The return shadow stack: a ring of (guest, host) return-address pairs.
pub mod shadow_stack {
    /// The ring's size in bytes; it is aligned to its size, so wrapping keeps the low bits.
    pub const BYTES: u64 = 64 * 1024;
    /// One (guest, host) pair.
    pub const ENTRY_BYTES: u64 = 16;

    /// The `x20` value a new thread starts with, for a ring at `base` (aligned to
    /// [`BYTES`]); `None` when `base` is not aligned.
    pub const fn initial(base: u64) -> Option<u64> {
        if base.is_multiple_of(BYTES) {
            Some(base)
        } else {
            None
        }
    }
}

const FRAME_BYTES: u16 = 176;
const SAVED_CONTEXT: u32 = 104;
const FIRST_SAVED_VECTOR: u32 = 112;
const SP: Reg = Reg(31);
const LINK: Reg = Reg(30);
const FRAME: Reg = Reg(29);
const T0: Reg = Reg(22);
const T1: Reg = Reg(23);
const T2: Reg = Reg(24);
const T3: Reg = Reg(25);

/// The runtime routines' code and where each one starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Routines {
    /// The words, starting with the dispatch.
    pub words: Vec<u32>,
    /// The word index of the indirect dispatch (guest target in `x16`, NZCV untouched).
    pub dispatch: u32,
    /// The word index of the entry trampoline, an AAPCS64 function taking a pointer to a
    /// guest context (see [`context`]).
    pub enter: u32,
    /// The word index where a guest return to [`super::RETURN_TO_NATIVE_RVA`] lands.
    pub return_to_native: u32,
}

fn load(t: Reg, n: Reg, offset: u32) -> u32 {
    encode::load_store(LoadStore::LdrX, t, n, offset).expect("runtime offsets are encodable")
}

fn store(t: Reg, n: Reg, offset: u32) -> u32 {
    encode::load_store(LoadStore::StrX, t, n, offset).expect("runtime offsets are encodable")
}

fn vector(op: VectorLoadStore, v: u8, n: Reg, offset: u32) -> u32 {
    vector_load_store(op, v, n, offset).expect("runtime offsets are encodable")
}

fn general_pair_offset(pair: u8) -> i32 {
    context::GENERAL.wrapping_add(16 * u32::from(pair)) as i32
}

fn emit_dispatch(asm: &mut Assembler, bits: u8) {
    let hit = asm.label();
    let miss = asm.label();
    let probe = asm.label();
    asm.emit(load(SCRATCH, ENTRIES, entries_table::GUEST_BASE));
    asm.emit(encode::add_sub_shifted(
        AddSub::Sub,
        Size::X,
        SCRATCH,
        ADDRESS,
        SCRATCH,
        Shift::Lsl,
        0,
    ));
    asm.emit(encode::bitfield(
        Bitfield::Ubfm,
        Size::X,
        T0,
        SCRATCH,
        32,
        63,
    ));
    asm.cbz(Size::X, T0, miss, true);
    asm.emit(encode::move_wide(
        MoveWide::Movz,
        Size::W,
        T1,
        HASH_MULTIPLIER as u16,
        0,
    ));
    asm.emit(encode::move_wide(
        MoveWide::Movk,
        Size::W,
        T1,
        (HASH_MULTIPLIER >> 16) as u16,
        1,
    ));
    asm.emit(encode::multiply(
        Multiply::Madd,
        Size::W,
        T0,
        SCRATCH,
        T1,
        Reg::ZR,
    ));
    asm.emit(encode::bitfield(
        Bitfield::Ubfm,
        Size::W,
        T0,
        T0,
        32 - bits,
        31,
    ));
    asm.bind(probe);
    asm.emit(encode::add_sub_shifted(
        AddSub::Add,
        Size::X,
        T1,
        ENTRIES,
        T0,
        Shift::Lsl,
        3,
    ));
    asm.emit(load(T2, T1, entries_table::SLOTS));
    asm.emit(encode::logical_shifted(
        Logic::Eor,
        false,
        Size::W,
        T3,
        T2,
        SCRATCH,
        Shift::Lsl,
        0,
    ));
    asm.cbz(Size::W, T3, hit, false);
    asm.emit(encode::add_sub_immediate(
        AddSub::Add,
        Size::W,
        T3,
        T2,
        1,
        false,
    ));
    asm.cbz(Size::W, T3, miss, false);
    asm.emit(encode::add_sub_immediate(
        AddSub::Add,
        Size::W,
        T0,
        T0,
        1,
        false,
    ));
    asm.emit(
        encode::logical_immediate(Logic::And, Size::W, T0, T0, (1u64 << bits) - 1)
            .expect("a run of ones is a logical immediate"),
    );
    asm.b(probe);
    asm.bind(hit);
    asm.emit(load(T3, ENTRIES, entries_table::HOST_BASE));
    asm.emit(encode::add_sub_shifted(
        AddSub::Add,
        Size::X,
        T3,
        T3,
        T2,
        Shift::Lsr,
        32,
    ));
    asm.emit(encode::br(T3));
    asm.bind(miss);
    asm.emit(encode::brk(trap::UNDISCOVERED));
}

const SAVED_PAIRS: [(Reg, Reg, i32); 6] = [
    (FRAME, LINK, 0),
    (GS_BASE, ENTRIES, 16),
    (SHADOW_STACK, STATE, 32),
    (Reg(22), Reg(23), 48),
    (Reg(24), Reg(25), 64),
    (Reg(26), Reg(27), 80),
];

fn emit_enter(asm: &mut Assembler, dispatch: Label) -> u32 {
    let return_to_native = asm.label();
    asm.emit(encode::add_sub_immediate(
        AddSub::Sub,
        Size::X,
        SP,
        SP,
        FRAME_BYTES,
        false,
    ));
    for (first, second, offset) in SAVED_PAIRS {
        asm.emit(encode::pair(Pair::Stp, first, second, SP, offset));
    }
    asm.emit(encode::pair(
        Pair::Stp,
        Reg(28),
        Reg(0),
        SP,
        SAVED_CONTEXT as i32 - 8,
    ));
    for register in 8u8..16 {
        asm.emit(vector(
            VectorLoadStore::StrD,
            register,
            SP,
            FIRST_SAVED_VECTOR + 8 * u32::from(register - 8),
        ));
    }
    asm.emit(encode::logical_shifted(
        Logic::Orr,
        false,
        Size::X,
        SCRATCH,
        Reg::ZR,
        Reg(0),
        Shift::Lsl,
        0,
    ));
    for register in 0u8..16 {
        asm.emit(vector(
            VectorLoadStore::LdrQ,
            register,
            SCRATCH,
            context::XMM + 16 * u32::from(register),
        ));
    }
    asm.emit(encode::pair(
        Pair::Ldp,
        GS_BASE,
        ENTRIES,
        SCRATCH,
        context::GS_BASE as i32,
    ));
    asm.emit(encode::pair(
        Pair::Ldp,
        SHADOW_STACK,
        STATE,
        SCRATCH,
        context::SHADOW_STACK as i32,
    ));
    asm.emit(load(T0, ENTRIES, entries_table::GUEST_BASE));
    asm.adr(T1, return_to_native);
    asm.emit(encode::add_sub_immediate(
        AddSub::Sub,
        Size::X,
        T2,
        SHADOW_STACK,
        shadow_stack::ENTRY_BYTES as u16,
        false,
    ));
    asm.emit(encode::bitfield(
        Bitfield::Bfm,
        Size::X,
        SHADOW_STACK,
        T2,
        0,
        15,
    ));
    asm.emit(encode::pair(Pair::Stp, T0, T1, SHADOW_STACK, 0));
    asm.emit(load(ADDRESS, SCRATCH, context::RIP));
    for pair in 0u8..8 {
        asm.emit(encode::pair(
            Pair::Ldp,
            Reg(2 * pair),
            Reg(2 * pair + 1),
            SCRATCH,
            general_pair_offset(pair),
        ));
    }
    asm.b(dispatch);
    asm.bind(return_to_native);
    let position = asm.position();
    asm.emit(load(SCRATCH, SP, SAVED_CONTEXT));
    for pair in 0u8..8 {
        asm.emit(encode::pair(
            Pair::Stp,
            Reg(2 * pair),
            Reg(2 * pair + 1),
            SCRATCH,
            general_pair_offset(pair),
        ));
    }
    for register in 0u8..16 {
        asm.emit(vector(
            VectorLoadStore::StrQ,
            register,
            SCRATCH,
            context::XMM + 16 * u32::from(register),
        ));
    }
    asm.emit(store(ADDRESS, SCRATCH, context::RIP));
    asm.emit(store(SHADOW_STACK, SCRATCH, context::SHADOW_STACK));
    for (first, second, offset) in SAVED_PAIRS {
        asm.emit(encode::pair(Pair::Ldp, first, second, SP, offset));
    }
    asm.emit(load(Reg(28), SP, SAVED_CONTEXT - 8));
    for register in 8u8..16 {
        asm.emit(vector(
            VectorLoadStore::LdrD,
            register,
            SP,
            FIRST_SAVED_VECTOR + 8 * u32::from(register - 8),
        ));
    }
    asm.emit(encode::add_sub_immediate(
        AddSub::Add,
        Size::X,
        SP,
        SP,
        FRAME_BYTES,
        false,
    ));
    asm.emit(encode::ret(LINK));
    position
}

/// The runtime routines for an image whose entries table has `2^bits` slots.
///
/// The dispatch takes the guest target in `x16`, finds it in the entries table at `x19`
/// (relative to the guest base stored there) and branches to its code; it uses only `x17`
/// and `x22`-`x25`, which are dead at every block exit, and never touches NZCV. A miss ends in
/// `brk #UNDISCOVERED` with the target still in `x16`.
///
/// The entry trampoline saves the callee-saved registers AAPCS64 asks for (and `x18`), loads
/// the guest's registers, `x18`-`x21` and `rip` from the context in `x0`, pushes a
/// (guest base, return path) pair on the shadow stack and dispatches to `rip`. The native
/// caller must have pushed the guest base address as the return address on the guest stack:
/// a guest `ret` to it comes back through the return path, which writes the guest registers,
/// `rip` and the shadow stack position back to the context and returns to the caller.
pub fn routines(bits: u8) -> Result<Routines, EmitError> {
    let mut asm = Assembler::new();
    let dispatch = asm.label();
    asm.bind(dispatch);
    emit_dispatch(&mut asm, bits);
    if asm.position() % 2 == 1 {
        asm.emit(encode::nop());
    }
    let enter = asm.position();
    let return_to_native = emit_enter(&mut asm, dispatch);
    let assembled = asm.finish()?;
    let mut words = assembled.words;
    if words.len() % 2 == 1 {
        words.push(encode::nop());
    }
    Ok(Routines {
        words,
        dispatch: 0,
        enter,
        return_to_native,
    })
}
