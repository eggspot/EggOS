//! The NSString constants Foundation exports as `NSString *const` variables (D175): exception
//! names, the error domains and `userInfo` keys and the run loop modes, each laid out as clang
//! lays out a string literal (the constant string class, ASCII flags, its bytes and length),
//! with the variable that holds its address, which is what the program imports.

use super::CONSTANT_STRING;
use crate::objc::abi::Id;
use core::sync::atomic::{AtomicUsize, Ordering};

const ASCII_FLAGS: usize = 0x7c8;
const STRING_WORDS: usize = 4;

/// The `userInfo` key of an error's description.
pub const LOCALIZED_DESCRIPTION_KEY: usize = 4;
/// The `userInfo` key of why an error happened.
pub const LOCALIZED_FAILURE_REASON_KEY: usize = 5;

const CONSTANTS: [(&[u8], &[u8]); 12] = [
    (
        b"_NSInvalidArgumentException",
        b"NSInvalidArgumentException\0",
    ),
    (b"_NSRangeException", b"NSRangeException\0"),
    (b"_NSGenericException", b"NSGenericException\0"),
    (
        b"_NSInternalInconsistencyException",
        b"NSInternalInconsistencyException\0",
    ),
    (b"_NSLocalizedDescriptionKey", b"NSLocalizedDescription\0"),
    (
        b"_NSLocalizedFailureReasonErrorKey",
        b"NSLocalizedFailureReason\0",
    ),
    (b"_NSUnderlyingErrorKey", b"NSUnderlyingError\0"),
    (b"_NSCocoaErrorDomain", b"NSCocoaErrorDomain\0"),
    (b"_NSPOSIXErrorDomain", b"NSPOSIXErrorDomain\0"),
    (b"_NSOSStatusErrorDomain", b"NSOSStatusErrorDomain\0"),
    (b"_NSDefaultRunLoopMode", b"kCFRunLoopDefaultMode\0"),
    (b"_NSRunLoopCommonModes", b"kCFRunLoopCommonModes\0"),
];

static STRINGS: [[AtomicUsize; STRING_WORDS]; CONSTANTS.len()] =
    [const { [const { AtomicUsize::new(0) }; STRING_WORDS] }; CONSTANTS.len()];
static VARIABLES: [AtomicUsize; CONSTANTS.len()] = [const { AtomicUsize::new(0) }; CONSTANTS.len()];

/// Lays every constant out; called once Foundation's classes are defined.
pub fn set_up() -> bool {
    for ((string, variable), (_, text)) in STRINGS.iter().zip(&VARIABLES).zip(CONSTANTS) {
        string[0].store(&raw const CONSTANT_STRING as usize, Ordering::Relaxed);
        string[1].store(ASCII_FLAGS, Ordering::Relaxed);
        string[2].store(text.as_ptr() as usize, Ordering::Relaxed);
        string[3].store(text.len() - 1, Ordering::Relaxed);
        variable.store(string.as_ptr() as usize, Ordering::Relaxed);
    }
    true
}

/// The constant string at `index` of the list.
pub fn string(index: usize) -> Id {
    VARIABLES[index].load(Ordering::Relaxed) as Id
}

/// The address of the exported variable `name`, if it is one of these.
pub fn resolve(name: &[u8]) -> Option<u64> {
    let index = CONSTANTS.iter().position(|&(symbol, _)| symbol == name)?;
    Some(&VARIABLES[index] as *const AtomicUsize as u64)
}
