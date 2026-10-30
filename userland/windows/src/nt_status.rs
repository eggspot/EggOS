//! NT status codes, what ntdll's functions return (D189): the ones the layer reports and the
//! Win32 error code `RtlNtStatusToDosError` turns each into, from Microsoft's public
//! documentation of both lists.

use crate::errors::code as error;

pub const SUCCESS: u32 = 0;
pub const PENDING: u32 = 0x103;
pub const UNSUCCESSFUL: u32 = 0xC000_0001;
pub const NOT_IMPLEMENTED: u32 = 0xC000_0002;
pub const INVALID_HANDLE: u32 = 0xC000_0008;
pub const INVALID_PARAMETER: u32 = 0xC000_000D;
pub const END_OF_FILE: u32 = 0xC000_0011;
pub const NO_MEMORY: u32 = 0xC000_0017;
pub const ACCESS_DENIED: u32 = 0xC000_0022;
pub const OBJECT_NAME_NOT_FOUND: u32 = 0xC000_0034;
pub const OBJECT_NAME_COLLISION: u32 = 0xC000_0035;
pub const OBJECT_PATH_NOT_FOUND: u32 = 0xC000_003A;
pub const DISK_FULL: u32 = 0xC000_007F;
pub const PIPE_CLOSING: u32 = 0xC000_00B1;
pub const NOT_SUPPORTED: u32 = 0xC000_00BB;
pub const DIRECTORY_NOT_EMPTY: u32 = 0xC000_0101;
pub const TOO_MANY_OPENED_FILES: u32 = 0xC000_011F;
pub const CANCELLED: u32 = 0xC000_0120;
pub const PIPE_BROKEN: u32 = 0xC000_014B;

const PAIRS: [(u32, u32); 19] = [
    (SUCCESS, error::SUCCESS),
    (PENDING, error::IO_PENDING),
    (UNSUCCESSFUL, error::GEN_FAILURE),
    (NOT_IMPLEMENTED, error::INVALID_FUNCTION),
    (INVALID_HANDLE, error::INVALID_HANDLE),
    (INVALID_PARAMETER, error::INVALID_PARAMETER),
    (END_OF_FILE, error::HANDLE_EOF),
    (NO_MEMORY, error::NOT_ENOUGH_MEMORY),
    (ACCESS_DENIED, error::ACCESS_DENIED),
    (OBJECT_NAME_NOT_FOUND, error::FILE_NOT_FOUND),
    (OBJECT_NAME_COLLISION, error::ALREADY_EXISTS),
    (OBJECT_PATH_NOT_FOUND, error::PATH_NOT_FOUND),
    (DISK_FULL, error::DISK_FULL),
    (PIPE_CLOSING, error::NO_DATA),
    (NOT_SUPPORTED, error::NOT_SUPPORTED),
    (DIRECTORY_NOT_EMPTY, error::DIR_NOT_EMPTY),
    (TOO_MANY_OPENED_FILES, error::TOO_MANY_OPEN_FILES),
    (CANCELLED, error::OPERATION_ABORTED),
    (PIPE_BROKEN, error::BROKEN_PIPE),
];

/// The Win32 error code for `status`, as `RtlNtStatusToDosError` gives it;
/// `ERROR_MR_MID_NOT_FOUND` for one the layer never reports.
pub fn dos_error(status: u32) -> u32 {
    PAIRS
        .iter()
        .find(|(nt, _)| *nt == status)
        .map_or(error::MR_MID_NOT_FOUND, |(_, win32)| *win32)
}

/// The status that reports Win32 error `code`; `STATUS_UNSUCCESSFUL` for one with no status
/// of its own.
pub fn for_dos_error(code: u32) -> u32 {
    PAIRS
        .iter()
        .find(|(_, win32)| *win32 == code)
        .map_or(UNSUCCESSFUL, |(nt, _)| *nt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_end_of_a_pipe_and_of_a_file_are_the_errors_windows_gives() {
        assert_eq!(dos_error(PIPE_BROKEN), 109);
        assert_eq!(dos_error(END_OF_FILE), 38);
        assert_eq!(dos_error(PIPE_CLOSING), 232);
        assert_eq!(dos_error(CANCELLED), 995);
        assert_eq!(dos_error(SUCCESS), 0);
        assert_eq!(dos_error(PENDING), 997);
    }

    #[test]
    fn an_unknown_status_is_mr_mid_not_found_and_an_unknown_error_unsuccessful() {
        assert_eq!(dos_error(0xC000_9999), 317);
        assert_eq!(for_dos_error(1460), UNSUCCESSFUL);
    }

    #[test]
    fn every_error_turns_back_into_its_status() {
        for (status, code) in PAIRS {
            assert_eq!(for_dos_error(code), status);
            assert_eq!(dos_error(status), code);
        }
    }
}
