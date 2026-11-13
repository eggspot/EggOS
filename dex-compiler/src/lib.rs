//! Dalvik methods compiled ahead of time to the machine's own code (M15, D471; D17: apps run
//! natively, never interpreted). A method's virtual registers live in a frame, one 64-bit slot
//! each, whose address the compiled code is called with; a wide value takes its pair's first
//! slot whole. Each instruction becomes a short run of machine instructions on that frame,
//! branches are patched once every instruction's place is known, and the place of each
//! instruction is kept, so a runtime can map a code address back to the DEX instruction.
//!
//! It compiles moves, constants, int and long arithmetic and comparisons, branches and
//! switches, returns, method calls (virtual and interface ones dispatched on the receiver's
//! class), strings, objects with their instance and static fields, arrays, casts and `throw`,
//! float and double arithmetic, comparisons and conversions with SSE (D503), the object work,
//! division and Java's float conversions through the runtime's helpers ([`helper`]), for
//! x86_64 (D471, D472, D499); exceptions are caught by the method's `try` blocks or passed to
//! its caller (D504). A method using anything else (`filled-new-array`, method handles) is
//! refused with the opcode and place, and the layer reports it.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod x86_64;

use alloc::vec::Vec;
use eggos_android_package::dex::Code;

/// Why a method was not compiled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompileError {
    /// An instruction this compiler does not compile yet, at code unit `at`.
    Unsupported { opcode: u8, at: usize },
    /// A branch at code unit `at` to a place that is no instruction's start.
    BadBranch { at: usize },
    /// The code ends inside an instruction.
    Truncated,
}

/// A compiled method: its machine code, and the code offset of each DEX instruction (by the
/// code unit it starts at), in order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Compiled {
    pub code: Vec<u8>,
    pub places: Vec<(u32, u32)>,
}

impl Compiled {
    /// Where the instruction at code unit `at` starts in the machine code.
    pub fn place_of(&self, at: usize) -> Option<usize> {
        let at = u32::try_from(at).ok()?;
        let index = self
            .places
            .binary_search_by_key(&at, |&(unit, _)| unit)
            .ok()?;
        Some(self.places[index].1 as usize)
    }
}

/// The runtime's helpers, by their place in the entry table a compiled method is called with
/// (D472): each takes its argument words and returns its result as a method does.
pub mod helper {
    /// `const-string`: the string with the index in the first word.
    pub const STRING: usize = 0;
    /// `new-instance`: a new object of the type with the index in the first word.
    pub const NEW_INSTANCE: usize = 1;
    /// `iget`: the object's field with the index in the second word.
    pub const FIELD_GET: usize = 2;
    /// `iput`: the third word into the object's field with the index in the second word.
    pub const FIELD_PUT: usize = 3;
    /// `div-int`, throwing `ArithmeticException` on zero.
    pub const DIVIDE_INT: usize = 4;
    /// `rem-int`.
    pub const REMAINDER_INT: usize = 5;
    /// `div-long`.
    pub const DIVIDE_LONG: usize = 6;
    /// `rem-long`.
    pub const REMAINDER_LONG: usize = 7;
    /// `invoke-virtual` and `invoke-interface` (D499): the method index in the first word, the
    /// call's own words after it, the receiver first; calls the receiver's class's method.
    pub const INVOKE_VIRTUAL: usize = 8;
    /// `sget`: the static field with the index in the first word, its class initialized first.
    pub const STATIC_GET: usize = 9;
    /// `sput`: the second word into the static field with the index in the first word.
    pub const STATIC_PUT: usize = 10;
    /// `new-array`: an array of the first word's length, of the type with the index in the
    /// second word.
    pub const NEW_ARRAY: usize = 11;
    /// `array-length`.
    pub const ARRAY_LENGTH: usize = 12;
    /// `aget`: the array's element at the index in the second word.
    pub const ARRAY_GET: usize = 13;
    /// `aput`: the third word into the array's element at the index in the second word.
    pub const ARRAY_PUT: usize = 14;
    /// `throw`: never returns while exceptions are not caught.
    pub const THROW: usize = 15;
    /// `check-cast`: the object in the first word to the type with the index in the second.
    pub const CHECK_CAST: usize = 16;
    /// `instance-of`: 1 when the object is of the type with the index in the second word.
    pub const INSTANCE_OF: usize = 17;
    /// `rem-float` (D503): the remainder of the first word's float by the second's, as C's
    /// `fmod`, the bits in each word's low half.
    pub const REMAINDER_FLOAT: usize = 18;
    /// `rem-double`.
    pub const REMAINDER_DOUBLE: usize = 19;
    /// `float-to-int`, as Java converts: NaN is 0 and values past the range saturate.
    pub const FLOAT_TO_INT: usize = 20;
    /// `float-to-long`.
    pub const FLOAT_TO_LONG: usize = 21;
    /// `double-to-int`.
    pub const DOUBLE_TO_INT: usize = 22;
    /// `double-to-long`.
    pub const DOUBLE_TO_LONG: usize = 23;
    /// Not a helper but a word (D504): the exception being thrown, 0 for none. Compiled code
    /// looks at it after every call, and goes to the handler that catches it or returns.
    pub const PENDING_EXCEPTION: usize = 24;
    /// Whether the pending exception is of the type with the index in the first word.
    pub const CATCHES: usize = 25;
    /// `move-exception`: the pending exception, no longer pending.
    pub const TAKE_EXCEPTION: usize = 26;
    /// How many helpers come before the methods.
    pub const COUNT: usize = 27;
}

/// The entry-table place of the method with index `method` in the DEX file's method table.
pub fn method_entry(method: u32) -> usize {
    helper::COUNT + method as usize
}

/// Compiles `code` for x86_64.
pub fn compile(code: &Code<'_>) -> Result<Compiled, CompileError> {
    x86_64::compile(code)
}
