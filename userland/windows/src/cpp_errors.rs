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

/// The generic category's message for an `std::errc` value, as `strerror` words it.
pub fn generic_message(value: i32) -> &'static [u8] {
    CPP_ERRORS
        .iter()
        .find(|(code, _)| *code == value)
        .map_or(&b"unknown error"[..], |(_, message)| *message)
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
}
