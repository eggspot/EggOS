//! The C runtime's text for each `errno` value (D189), as `strerror` returns it and Microsoft
//! documents the `errno` constants: NUL-terminated, with "Unknown error" for the numbers no
//! constant has.

const UNKNOWN: &[u8] = b"Unknown error\0";

const MESSAGES: [&[u8]; 43] = [
    b"No error\0",
    b"Operation not permitted\0",
    b"No such file or directory\0",
    b"No such process\0",
    b"Interrupted function call\0",
    b"Input/output error\0",
    b"No such device or address\0",
    b"Arg list too long\0",
    b"Exec format error\0",
    b"Bad file descriptor\0",
    b"No child processes\0",
    b"Resource temporarily unavailable\0",
    b"Not enough space\0",
    b"Permission denied\0",
    b"Bad address\0",
    UNKNOWN,
    b"Resource device\0",
    b"File exists\0",
    b"Improper link\0",
    b"No such device\0",
    b"Not a directory\0",
    b"Is a directory\0",
    b"Invalid argument\0",
    b"Too many open files in system\0",
    b"Too many open files\0",
    b"Inappropriate I/O control operation\0",
    UNKNOWN,
    b"File too large\0",
    b"No space left on device\0",
    b"Invalid seek\0",
    b"Read-only file system\0",
    b"Too many links\0",
    b"Broken pipe\0",
    b"Domain error\0",
    b"Result too large\0",
    UNKNOWN,
    b"Resource deadlock avoided\0",
    UNKNOWN,
    b"Filename too long\0",
    b"No locks available\0",
    b"Function not implemented\0",
    b"Directory not empty\0",
    b"Illegal byte sequence\0",
];

/// The text for `errno` value `number`, with its NUL.
pub fn message(number: i32) -> &'static [u8] {
    usize::try_from(number)
        .ok()
        .and_then(|index| MESSAGES.get(index))
        .copied()
        .unwrap_or(UNKNOWN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_errno_has_its_documented_text() {
        assert_eq!(message(0), b"No error\0");
        assert_eq!(message(2), b"No such file or directory\0");
        assert_eq!(message(9), b"Bad file descriptor\0");
        assert_eq!(message(12), b"Not enough space\0");
        assert_eq!(message(22), b"Invalid argument\0");
        assert_eq!(message(34), b"Result too large\0");
        assert_eq!(message(42), b"Illegal byte sequence\0");
    }

    #[test]
    fn numbers_without_a_constant_are_unknown_errors() {
        for number in [-1, 15, 26, 35, 37, 43, 80, i32::MAX] {
            assert_eq!(message(number), b"Unknown error\0");
        }
    }

    #[test]
    fn every_text_ends_with_its_only_nul() {
        for text in MESSAGES {
            assert_eq!(
                text.iter().position(|byte| *byte == 0),
                Some(text.len() - 1)
            );
        }
    }
}
