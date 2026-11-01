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

/// The `userInfo` key of an error's underlying error.
pub const UNDERLYING_ERROR_KEY: usize = 6;
/// `NSCocoaErrorDomain`.
pub const COCOA_ERROR_DOMAIN: usize = 7;
/// `NSPOSIXErrorDomain`.
pub const POSIX_ERROR_DOMAIN: usize = 8;
/// The default run loop mode, `kCFRunLoopDefaultMode`.
pub const DEFAULT_RUN_LOOP_MODE: usize = 10;
/// The common modes, `kCFRunLoopCommonModes`.
pub const COMMON_RUN_LOOP_MODES: usize = 11;
/// AppKit's mode while a modal window runs.
pub const MODAL_PANEL_RUN_LOOP_MODE: usize = 12;
/// AppKit's mode while a control or menu tracks the pointer.
pub const EVENT_TRACKING_RUN_LOOP_MODE: usize = 13;
/// `NSApplicationWillFinishLaunchingNotification`.
pub const APPLICATION_WILL_FINISH_LAUNCHING: usize = 14;
/// `NSApplicationDidFinishLaunchingNotification`.
pub const APPLICATION_DID_FINISH_LAUNCHING: usize = 15;
/// `NSApplicationWillTerminateNotification`.
pub const APPLICATION_WILL_TERMINATE: usize = 16;
/// `NSWindowWillCloseNotification`.
pub const WINDOW_WILL_CLOSE: usize = 17;
/// The first of the file attribute keys, in `FILE_ATTRIBUTE_KEYS` order.
pub const FIRST_FILE_ATTRIBUTE_KEY: usize = 18;
/// How many file attribute keys there are: `NSFileSize`, `NSFileType`,
/// `NSFileModificationDate`, `NSFilePosixPermissions`, `NSFileReferenceCount`,
/// `NSFileOwnerAccountID`, `NSFileGroupOwnerAccountID`, `NSFileSystemNumber` and
/// `NSFileSystemFileNumber`.
pub const FILE_ATTRIBUTE_KEYS: usize = 9;
/// The first of the file types: regular, directory, symbolic link, character special, block
/// special, socket and unknown.
pub const FIRST_FILE_TYPE: usize = 27;
/// The `userInfo` key of the path a file error is about.
pub const FILE_PATH_ERROR_KEY: usize = 34;

const CONSTANTS: [(&[u8], &[u8]); 35] = [
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
    (b"_NSModalPanelRunLoopMode", b"NSModalPanelRunLoopMode\0"),
    (
        b"_NSEventTrackingRunLoopMode",
        b"NSEventTrackingRunLoopMode\0",
    ),
    (
        b"_NSApplicationWillFinishLaunchingNotification",
        b"NSApplicationWillFinishLaunchingNotification\0",
    ),
    (
        b"_NSApplicationDidFinishLaunchingNotification",
        b"NSApplicationDidFinishLaunchingNotification\0",
    ),
    (
        b"_NSApplicationWillTerminateNotification",
        b"NSApplicationWillTerminateNotification\0",
    ),
    (b"_NSWindowWillCloseNotification", b"NSWindowWillCloseNotification\0"),
    (b"_NSFileSize", b"NSFileSize\0"),
    (b"_NSFileType", b"NSFileType\0"),
    (b"_NSFileModificationDate", b"NSFileModificationDate\0"),
    (b"_NSFilePosixPermissions", b"NSFilePosixPermissions\0"),
    (b"_NSFileReferenceCount", b"NSFileReferenceCount\0"),
    (b"_NSFileOwnerAccountID", b"NSFileOwnerAccountID\0"),
    (b"_NSFileGroupOwnerAccountID", b"NSFileGroupOwnerAccountID\0"),
    (b"_NSFileSystemNumber", b"NSFileSystemNumber\0"),
    (b"_NSFileSystemFileNumber", b"NSFileSystemFileNumber\0"),
    (b"_NSFileTypeRegular", b"NSFileTypeRegular\0"),
    (b"_NSFileTypeDirectory", b"NSFileTypeDirectory\0"),
    (b"_NSFileTypeSymbolicLink", b"NSFileTypeSymbolicLink\0"),
    (b"_NSFileTypeCharacterSpecial", b"NSFileTypeCharacterSpecial\0"),
    (b"_NSFileTypeBlockSpecial", b"NSFileTypeBlockSpecial\0"),
    (b"_NSFileTypeSocket", b"NSFileTypeSocket\0"),
    (b"_NSFileTypeUnknown", b"NSFileTypeUnknown\0"),
    (b"_NSFilePathErrorKey", b"NSFilePath\0"),
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
