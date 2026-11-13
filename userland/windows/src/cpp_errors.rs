//! The errors msvcp140's thread functions report and the `std::system_error`s they become
//! (D230): `_Thrd_result`'s values, `_Throw_C_error`'s mapping of them to `_Throw_Cpp_error`'s
//! codes, the `std::errc` value and message each code stands for, the `what()` MSVC's runtime
//! builds from them, and the generic category's messages. Pure tables with host tests;
//! `win32/msvcp` throws them.

/// `_Thrd_result`'s values.
pub mod result {
    pub const SUCCESS: i32 = 0;
    pub const NO_MEMORY: i32 = 1;
    pub const TIMED_OUT: i32 = 2;
    pub const BUSY: i32 = 3;
    pub const ERROR: i32 = 4;
}

/// `_Throw_Cpp_error`'s codes, in `<xthreads.h>`'s order, with the `std::errc` each stands for
/// and the message `std::system_error`'s constructor gets.
pub const CPP_ERRORS: [(i32, &[u8]); 7] = [
    (16, b"device or resource busy"),
    (22, b"invalid argument"),
    (3, b"no such process"),
    (12, b"not enough memory"),
    (1, b"operation not permitted"),
    (36, b"resource deadlock would occur"),
    (11, b"resource unavailable try again"),
];

/// What `std::system_error::what()` says for each of [`CPP_ERRORS`]: its message, a colon and
/// the category's message for the code, as MSVC's runtime builds it; NUL-terminated.
pub const CPP_ERROR_WHAT: [&[u8]; 7] = [
    b"device or resource busy: device or resource busy\0",
    b"invalid argument: invalid argument\0",
    b"no such process: no such process\0",
    b"not enough memory: not enough memory\0",
    b"operation not permitted: operation not permitted\0",
    b"resource deadlock would occur: resource deadlock would occur\0",
    b"resource unavailable try again: resource unavailable try again\0",
];

/// `_Throw_Cpp_error`'s code for a busy device or resource.
pub const DEVICE_OR_RESOURCE_BUSY: i32 = 0;
/// `_Throw_Cpp_error`'s code for an invalid argument.
pub const INVALID_ARGUMENT: i32 = 1;
/// `_Throw_Cpp_error`'s code for a resource to try again.
pub const RESOURCE_UNAVAILABLE_TRY_AGAIN: i32 = 6;

/// The `_Throw_Cpp_error` code a `_Thrd_result` other than success stands for.
pub fn cpp_error_of(result: i32) -> i32 {
    match result {
        result::NO_MEMORY | result::TIMED_OUT => RESOURCE_UNAVAILABLE_TRY_AGAIN,
        result::BUSY => DEVICE_OR_RESOURCE_BUSY,
        _ => INVALID_ARGUMENT,
    }
}

/// The generic category's message for each `std::errc` value, by the UCRT's `errno` number:
/// the enumerator's name in words, as `_Syserror_map` gives it; NUL-terminated.
pub const GENERIC_MESSAGES: [(i32, &[u8]); 78] = [
    (1, b"operation not permitted\0"),
    (2, b"no such file or directory\0"),
    (3, b"no such process\0"),
    (4, b"interrupted\0"),
    (5, b"io error\0"),
    (6, b"no such device or address\0"),
    (7, b"argument list too long\0"),
    (8, b"executable format error\0"),
    (9, b"bad file descriptor\0"),
    (10, b"no child process\0"),
    (11, b"resource unavailable try again\0"),
    (12, b"not enough memory\0"),
    (13, b"permission denied\0"),
    (14, b"bad address\0"),
    (16, b"device or resource busy\0"),
    (17, b"file exists\0"),
    (18, b"cross device link\0"),
    (19, b"no such device\0"),
    (20, b"not a directory\0"),
    (21, b"is a directory\0"),
    (22, b"invalid argument\0"),
    (23, b"too many files open in system\0"),
    (24, b"too many files open\0"),
    (25, b"inappropriate io control operation\0"),
    (27, b"file too large\0"),
    (28, b"no space on device\0"),
    (29, b"invalid seek\0"),
    (30, b"read only file system\0"),
    (31, b"too many links\0"),
    (32, b"broken pipe\0"),
    (33, b"argument out of domain\0"),
    (34, b"result out of range\0"),
    (36, b"resource deadlock would occur\0"),
    (38, b"filename too long\0"),
    (39, b"no lock available\0"),
    (40, b"function not supported\0"),
    (41, b"directory not empty\0"),
    (42, b"illegal byte sequence\0"),
    (100, b"address in use\0"),
    (101, b"address not available\0"),
    (102, b"address family not supported\0"),
    (103, b"connection already in progress\0"),
    (104, b"bad message\0"),
    (105, b"operation canceled\0"),
    (106, b"connection aborted\0"),
    (107, b"connection refused\0"),
    (108, b"connection reset\0"),
    (109, b"destination address required\0"),
    (110, b"host unreachable\0"),
    (111, b"identifier removed\0"),
    (112, b"operation in progress\0"),
    (113, b"already connected\0"),
    (114, b"too many symbolic link levels\0"),
    (115, b"message size\0"),
    (116, b"network down\0"),
    (117, b"network reset\0"),
    (118, b"network unreachable\0"),
    (119, b"no buffer space\0"),
    (120, b"no message available\0"),
    (121, b"no link\0"),
    (122, b"no message\0"),
    (123, b"no protocol option\0"),
    (124, b"no stream resources\0"),
    (125, b"not a stream\0"),
    (126, b"not connected\0"),
    (127, b"state not recoverable\0"),
    (128, b"not a socket\0"),
    (129, b"not supported\0"),
    (130, b"operation not supported\0"),
    (132, b"value too large\0"),
    (133, b"owner dead\0"),
    (134, b"protocol error\0"),
    (135, b"protocol not supported\0"),
    (136, b"wrong protocol type\0"),
    (137, b"stream timeout\0"),
    (138, b"timed out\0"),
    (139, b"text file busy\0"),
    (140, b"operation would block\0"),
];

const UNKNOWN_ERROR: &[u8] = b"unknown error\0";

const WINDOWS_ERRORS: [(u32, i32); 78] = [
    (1, 40),
    (2, 2),
    (3, 2),
    (4, 24),
    (5, 13),
    (6, 22),
    (8, 12),
    (12, 13),
    (14, 12),
    (15, 19),
    (16, 13),
    (17, 18),
    (19, 13),
    (20, 19),
    (21, 11),
    (25, 5),
    (29, 5),
    (30, 5),
    (32, 13),
    (33, 39),
    (39, 28),
    (55, 19),
    (80, 17),
    (82, 13),
    (110, 5),
    (111, 38),
    (112, 28),
    (123, 2),
    (131, 22),
    (142, 16),
    (145, 41),
    (170, 16),
    (183, 17),
    (206, 38),
    (212, 39),
    (267, 20),
    (995, 105),
    (998, 13),
    (1012, 5),
    (1013, 5),
    (1237, 11),
    (2401, 16),
    (2404, 16),
    (10004, 4),
    (10009, 9),
    (10013, 13),
    (10014, 14),
    (10022, 22),
    (10024, 24),
    (10035, 140),
    (10036, 112),
    (10037, 103),
    (10038, 128),
    (10039, 109),
    (10040, 115),
    (10041, 136),
    (10042, 123),
    (10043, 135),
    (10045, 130),
    (10047, 102),
    (10048, 100),
    (10049, 101),
    (10050, 116),
    (10051, 118),
    (10052, 117),
    (10053, 106),
    (10054, 108),
    (10055, 119),
    (10056, 113),
    (10057, 126),
    (10060, 138),
    (10061, 107),
    (10063, 38),
    (10065, 110),
    (10066, 41),
    (10067, 23),
    (10070, 2),
    (10091, 11),
];

/// `_Winerror_map`: the `std::errc` value Windows error `code` stands for, 0 when none does.
pub fn errc_of_windows_error(code: u32) -> i32 {
    WINDOWS_ERRORS
        .binary_search_by_key(&code, |&(windows, _)| windows)
        .map_or(0, |at| WINDOWS_ERRORS[at].1)
}

/// The generic category's message for an `std::errc` value, NUL-terminated: what msvcp140's
/// `_Syserror_map` returns. `unknown error` for a value that names none.
pub fn generic_message_terminated(value: i32) -> &'static [u8] {
    GENERIC_MESSAGES
        .iter()
        .find(|(code, _)| *code == value)
        .map_or(UNKNOWN_ERROR, |(_, message)| *message)
}

/// [`generic_message_terminated`] without its NUL.
pub fn generic_message(value: i32) -> &'static [u8] {
    let message = generic_message_terminated(value);
    &message[..message.len() - 1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_results_name_the_errors_msvc_throws() {
        assert_eq!(cpp_error_of(result::BUSY), DEVICE_OR_RESOURCE_BUSY);
        assert_eq!(
            cpp_error_of(result::TIMED_OUT),
            RESOURCE_UNAVAILABLE_TRY_AGAIN
        );
        assert_eq!(
            cpp_error_of(result::NO_MEMORY),
            RESOURCE_UNAVAILABLE_TRY_AGAIN
        );
        assert_eq!(cpp_error_of(result::ERROR), INVALID_ARGUMENT);
        assert_eq!(result::SUCCESS, 0);
        assert_eq!(generic_message(36), b"resource deadlock would occur");
        assert_eq!(generic_message(9999), b"unknown error");
    }

    #[test]
    fn every_generic_message_is_one_terminated_line_of_words_for_its_own_value() {
        for (index, (value, message)) in GENERIC_MESSAGES.iter().enumerate() {
            assert_eq!(message.last(), Some(&0), "{value}");
            let words = &message[..message.len() - 1];
            assert!(
                words
                    .iter()
                    .all(|byte| byte.is_ascii_lowercase() || *byte == b' '),
                "{value}"
            );
            assert!(
                GENERIC_MESSAGES[..index]
                    .iter()
                    .all(|(earlier, _)| earlier != value),
                "{value} twice"
            );
        }
        assert_eq!(generic_message_terminated(22), b"invalid argument\0");
        assert_eq!(generic_message_terminated(140), b"operation would block\0");
        assert_eq!(generic_message_terminated(-1), b"unknown error\0");
    }

    #[test]
    fn the_thread_errors_words_are_the_generic_category_s() {
        for (value, message) in CPP_ERRORS {
            assert_eq!(generic_message(value), message);
        }
    }

    #[test]
    fn what_is_the_message_then_the_category_s_message() {
        for (index, what) in CPP_ERROR_WHAT.iter().enumerate() {
            let (value, message) = CPP_ERRORS[index];
            let mut wanted = message.to_vec();
            wanted.extend_from_slice(b": ");
            wanted.extend_from_slice(generic_message(value));
            wanted.push(0);
            assert_eq!(*what, &wanted[..]);
        }
    }

    #[test]
    fn windows_errors_map_to_their_generic_conditions() {
        assert_eq!(errc_of_windows_error(2), 2);
        assert_eq!(errc_of_windows_error(5), 13);
        assert_eq!(errc_of_windows_error(183), 17);
        assert_eq!(errc_of_windows_error(10061), 107);
        assert_eq!(errc_of_windows_error(0), 0);
        assert_eq!(errc_of_windows_error(9999), 0);
        for (_, errc) in WINDOWS_ERRORS {
            assert!(GENERIC_MESSAGES.iter().any(|(value, _)| *value == errc));
        }
    }
}
