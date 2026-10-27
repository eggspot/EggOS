//! Win32 error codes (Microsoft's "System Error Codes"), the code each Linux error number
//! becomes, and the system message `FormatMessage` gives for each code the layer sets.

pub mod code {
    pub const SUCCESS: u32 = 0;
    pub const INVALID_FUNCTION: u32 = 1;
    pub const FILE_NOT_FOUND: u32 = 2;
    pub const PATH_NOT_FOUND: u32 = 3;
    pub const TOO_MANY_OPEN_FILES: u32 = 4;
    pub const ACCESS_DENIED: u32 = 5;
    pub const INVALID_HANDLE: u32 = 6;
    pub const NOT_ENOUGH_MEMORY: u32 = 8;
    pub const INVALID_DATA: u32 = 13;
    pub const NO_MORE_FILES: u32 = 18;
    pub const WRITE_PROTECT: u32 = 19;
    pub const BROKEN_PIPE: u32 = 109;
    pub const NOT_SUPPORTED: u32 = 50;
    pub const FILE_EXISTS: u32 = 80;
    pub const INVALID_PARAMETER: u32 = 87;
    pub const DISK_FULL: u32 = 112;
    pub const INSUFFICIENT_BUFFER: u32 = 122;
    pub const INVALID_NAME: u32 = 123;
    pub const MOD_NOT_FOUND: u32 = 126;
    pub const PROC_NOT_FOUND: u32 = 127;
    pub const NEGATIVE_SEEK: u32 = 131;
    pub const DIR_NOT_EMPTY: u32 = 145;
    pub const BUSY: u32 = 170;
    pub const ALREADY_EXISTS: u32 = 183;
    pub const FILENAME_EXCED_RANGE: u32 = 206;
    pub const ENVVAR_NOT_FOUND: u32 = 203;
    pub const DIRECTORY: u32 = 267;
    pub const NOT_OWNER: u32 = 288;
    pub const TOO_MANY_POSTS: u32 = 298;
    pub const INVALID_ADDRESS: u32 = 487;
    pub const NO_UNICODE_TRANSLATION: u32 = 1113;
    pub const INVALID_WINDOW_HANDLE: u32 = 1400;
    pub const CANNOT_FIND_WINDOW_CLASS: u32 = 1407;
    pub const CLASS_ALREADY_EXISTS: u32 = 1410;
    pub const CLASS_DOES_NOT_EXIST: u32 = 1411;
    pub const CLASS_HAS_WINDOWS: u32 = 1412;
    pub const INVALID_INDEX: u32 = 1413;
    pub const TIMEOUT: u32 = 1460;
    pub const NOT_ENOUGH_QUOTA: u32 = 1816;
}

/// The Win32 error code for Linux error number `errno`.
pub fn for_errno(errno: u16) -> u32 {
    match errno {
        2 => code::FILE_NOT_FOUND,
        20 => code::PATH_NOT_FOUND,
        1 | 13 | 21 | 30 => code::ACCESS_DENIED,
        9 => code::INVALID_HANDLE,
        12 => code::NOT_ENOUGH_MEMORY,
        16 => code::BUSY,
        17 => code::FILE_EXISTS,
        23 | 24 => code::TOO_MANY_OPEN_FILES,
        28 => code::DISK_FULL,
        32 => code::BROKEN_PIPE,
        36 => code::FILENAME_EXCED_RANGE,
        38 | 95 => code::NOT_SUPPORTED,
        39 => code::DIR_NOT_EMPTY,
        110 => code::TIMEOUT,
        _ => code::INVALID_PARAMETER,
    }
}

/// The system's message for `code`, in English, as `FormatMessage` with
/// `FORMAT_MESSAGE_FROM_SYSTEM` gives it (it ends with a period; Windows adds `\r\n`).
pub fn message(code: u32) -> Option<&'static str> {
    Some(match code {
        code::SUCCESS => "The operation completed successfully.",
        code::INVALID_FUNCTION => "Incorrect function.",
        code::FILE_NOT_FOUND => "The system cannot find the file specified.",
        code::PATH_NOT_FOUND => "The system cannot find the path specified.",
        code::TOO_MANY_OPEN_FILES => "The system cannot open the file.",
        code::ACCESS_DENIED => "Access is denied.",
        code::INVALID_HANDLE => "The handle is invalid.",
        code::NOT_ENOUGH_MEMORY => {
            "Not enough memory resources are available to process this command."
        }
        code::INVALID_DATA => "The data is invalid.",
        code::NO_MORE_FILES => "There are no more files.",
        code::WRITE_PROTECT => "The media is write protected.",
        code::NOT_SUPPORTED => "The request is not supported.",
        code::FILE_EXISTS => "The file exists.",
        code::INVALID_PARAMETER => "The parameter is incorrect.",
        code::BROKEN_PIPE => "The pipe has been ended.",
        code::DISK_FULL => "There is not enough space on the disk.",
        code::INSUFFICIENT_BUFFER => "The data area passed to a system call is too small.",
        code::INVALID_NAME => "The filename, directory name, or volume label syntax is incorrect.",
        code::MOD_NOT_FOUND => "The specified module could not be found.",
        code::PROC_NOT_FOUND => "The specified procedure could not be found.",
        code::NEGATIVE_SEEK => {
            "An attempt was made to move the file pointer before the beginning of the file."
        }
        code::DIR_NOT_EMPTY => "The directory is not empty.",
        code::BUSY => "The requested resource is in use.",
        code::ALREADY_EXISTS => "Cannot create a file when that file already exists.",
        code::FILENAME_EXCED_RANGE => "The filename or extension is too long.",
        code::ENVVAR_NOT_FOUND => {
            "The system could not find the environment option that was entered."
        }
        code::DIRECTORY => "The directory name is invalid.",
        code::NOT_OWNER => "Attempt to release mutex not owned by caller.",
        code::TOO_MANY_POSTS => "Too many posts were made to a semaphore.",
        code::NO_UNICODE_TRANSLATION => {
            "No mapping for the Unicode character exists in the target multi-byte code page."
        }
        code::INVALID_ADDRESS => "Attempt to access invalid address.",
        code::TIMEOUT => "This operation returned because the timeout period expired.",
        code::INVALID_WINDOW_HANDLE => "Invalid window handle.",
        code::CANNOT_FIND_WINDOW_CLASS => "Cannot find window class.",
        code::CLASS_ALREADY_EXISTS => "Class already exists.",
        code::CLASS_DOES_NOT_EXIST => "Class does not exist.",
        code::CLASS_HAS_WINDOWS => "Class still has open windows.",
        code::INVALID_INDEX => "Invalid index.",
        code::NOT_ENOUGH_QUOTA => "Not enough quota is available to process this command.",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_errors_become_the_win32_codes_windows_reports() {
        assert_eq!(for_errno(2), code::FILE_NOT_FOUND);
        assert_eq!(for_errno(20), code::PATH_NOT_FOUND);
        for errno in [1, 13, 21, 30] {
            assert_eq!(for_errno(errno), code::ACCESS_DENIED);
        }
        assert_eq!(for_errno(9), code::INVALID_HANDLE);
        assert_eq!(for_errno(17), code::FILE_EXISTS);
        assert_eq!(for_errno(39), code::DIR_NOT_EMPTY);
        assert_eq!(for_errno(36), code::FILENAME_EXCED_RANGE);
        assert_eq!(for_errno(38), code::NOT_SUPPORTED);
        assert_eq!(for_errno(22), code::INVALID_PARAMETER);
        assert_eq!(for_errno(4095), code::INVALID_PARAMETER);
    }

    #[test]
    fn every_code_the_layer_sets_has_a_message() {
        for code in [
            code::SUCCESS,
            code::FILE_NOT_FOUND,
            code::PATH_NOT_FOUND,
            code::ACCESS_DENIED,
            code::INVALID_HANDLE,
            code::NO_MORE_FILES,
            code::INVALID_PARAMETER,
            code::INSUFFICIENT_BUFFER,
            code::ALREADY_EXISTS,
            code::ENVVAR_NOT_FOUND,
            code::NOT_OWNER,
            code::TOO_MANY_POSTS,
            code::NO_UNICODE_TRANSLATION,
            code::INVALID_WINDOW_HANDLE,
            code::CANNOT_FIND_WINDOW_CLASS,
            code::CLASS_ALREADY_EXISTS,
            code::CLASS_DOES_NOT_EXIST,
            code::CLASS_HAS_WINDOWS,
            code::INVALID_INDEX,
            code::NOT_ENOUGH_QUOTA,
        ] {
            let message = message(code).unwrap();
            assert!(message.ends_with('.'), "{message}");
        }
        assert_eq!(
            message(code::FILE_NOT_FOUND),
            Some("The system cannot find the file specified.")
        );
        assert_eq!(message(99_999), None);
    }
}
