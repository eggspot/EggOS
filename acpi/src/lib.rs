//! An ACPI Machine Language interpreter (ACPI 6.5, chapters 5, 19 and 20; D537), written from
//! the specification.
//!
//! - **Namespace.** [`Namespace`] holds every object by absolute [`Path`], starting with the
//!   predefined scopes and objects; single-segment names follow the search rules up to the root.
//! - **Loading.** [`Interpreter::load_table`] runs a DSDT's or SSDT's terms in the root scope,
//!   which defines devices, methods, regions, fields and the rest, and runs any statement the
//!   table holds outside a method. A DSDT of revision 0 or 1 makes integers 32 bits wide.
//! - **Running.** [`Interpreter::evaluate`] runs a method with its arguments, reads a field or
//!   gives an object. Methods have their seven arguments and eight locals, `If`, `Else`,
//!   `While`, `Break`, `Continue` and `Return`, every expression opcode of chapter 20 but
//!   `LoadTable`, the implicit conversions between integers, strings and buffers, references
//!   (`RefOf`, `Index`, `DerefOf`, `CondRefOf`), and objects they create, which go when they
//!   return. Calls nest [`MAX_CALL_DEPTH`] deep and a loop runs [`MAX_LOOP_ITERATIONS`] times at
//!   most.
//! - **The machine.** Field units read and write their operation regions through a [`Host`]:
//!   system memory, system I/O, PCI configuration space (the function found from `_ADR`,
//!   `_BBN` and `_SEG`), the embedded controller and the rest, unit by unit in the field's
//!   access width, with its update rule; index fields and bank fields write their index or bank
//!   first. `Sleep`, `Stall`, `Timer` and `Notify` go to the host too. `\_OSI` answers the
//!   Windows versions and features firmware tests for.
//! - **Devices.** [`Interpreter::initialize_devices`] runs `\_SB._INI` and every present
//!   device's `_INI`, walking by `_STA` as the specification orders.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod host;
mod interpreter;
mod name;
mod namespace;
mod object;
mod opcodes;
mod stream;

#[cfg(test)]
mod tests;

pub use host::{Host, HostError};
pub use interpreter::{
    DeviceStatus, INTERPRETER_REVISION, Interpreter, MAX_CALL_DEPTH, MAX_LOOP_ITERATIONS,
};
pub use name::{NameSeg, NameString, Path};
pub use namespace::{Namespace, NodeId, REVISION};
pub use object::{
    Access, BufferField, Field, FieldKind, Method, NativeMethod, Object, PciFunction, Region,
    RegionSpace, Target, UpdateRule, object_type,
};

/// Why loading a table or running AML failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The AML ends inside a term.
    UnexpectedEnd,
    /// A name segment holds a character names may not.
    MalformedName,
    /// An opcode the interpreter does not know, or one where it may not stand.
    UnknownOpcode(u16),
    /// No object answers to a name.
    NameNotFound,
    /// An object already has the name a definition gives.
    AlreadyExists,
    /// An object of the wrong kind for the operation.
    WrongType,
    DivideByZero,
    /// An index past the end of a package, buffer or string.
    IndexOutOfRange,
    /// A field unit reaches past its operation region.
    OutsideRegion,
    /// Calls or references nest deeper than the interpreter allows.
    TooDeep,
    /// A `While` ran more than [`MAX_LOOP_ITERATIONS`] times.
    LoopLimit,
    /// The bytes are not a definition block, or a table is missing.
    NotATable,
    /// The machine refused an access.
    Host(HostError),
    /// The AML ran `Fatal`.
    Fatal { kind: u8, code: u32, argument: u64 },
    /// Something the interpreter does not do.
    Unsupported(&'static str),
}

/// The 32-bit compressed EISA ID of a seven-character ID such as `PNP0A03`, as `_HID` and
/// `_CID` give it as an integer: three letters in five bits each, then four hexadecimal digits,
/// stored as big-endian bytes.
pub fn eisa_id(text: &[u8; 7]) -> u32 {
    let letter = |byte: u8| u32::from(byte.wrapping_sub(0x40) & 0x1f);
    let digit = |byte: u8| (byte as char).to_digit(16).unwrap_or(0);
    let vendor = letter(text[0]) << 10 | letter(text[1]) << 5 | letter(text[2]);
    let product = text[3..]
        .iter()
        .fold(0u32, |value, &byte| value << 4 | digit(byte));
    let big_endian = vendor << 16 | product;
    big_endian.swap_bytes()
}
