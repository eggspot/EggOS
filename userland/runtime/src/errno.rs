//! Linux's error numbers, as system calls return them negated, and what each means.

/// A system call's error number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Errno(pub u16);

impl Errno {
    /// `EPERM`.
    pub const EPERM: Self = Self(1);
    /// `ENOENT`.
    pub const ENOENT: Self = Self(2);
    /// `EINTR`.
    pub const EINTR: Self = Self(4);
    /// `ENXIO`.
    pub const ENXIO: Self = Self(6);
    /// `E2BIG`.
    pub const E2BIG: Self = Self(7);
    /// `ENOEXEC`.
    pub const ENOEXEC: Self = Self(8);
    /// `EACCES`.
    pub const EACCES: Self = Self(13);
    /// `ENOTDIR`.
    pub const ENOTDIR: Self = Self(20);
    /// `EISDIR`.
    pub const EISDIR: Self = Self(21);
    /// `EINVAL`.
    pub const EINVAL: Self = Self(22);
    /// `ENOTTY`.
    pub const ENOTTY: Self = Self(25);
    /// `ENAMETOOLONG`.
    pub const ENAMETOOLONG: Self = Self(36);
    /// `EEXIST`.
    pub const EEXIST: Self = Self(17);
    /// `EXDEV`.
    pub const EXDEV: Self = Self(18);
    /// `EROFS`.
    pub const EROFS: Self = Self(30);
    /// `ENOTEMPTY`.
    pub const ENOTEMPTY: Self = Self(39);

    /// A system call's result: the non-negative value it returned, or the error of a negative
    /// one.
    pub fn result(value: isize) -> Result<usize, Self> {
        match value {
            0.. => Ok(value as usize),
            _ => Err(Self(value.unsigned_abs().min(usize::from(u16::MAX)) as u16)),
        }
    }

    /// What the error means, as `strerror` says it.
    pub fn message(self) -> &'static [u8] {
        match self.0 {
            1 => b"Operation not permitted",
            2 => b"No such file or directory",
            3 => b"No such process",
            4 => b"Interrupted system call",
            5 => b"Input/output error",
            6 => b"No such device or address",
            7 => b"Argument list too long",
            8 => b"Exec format error",
            9 => b"Bad file descriptor",
            10 => b"No child processes",
            11 => b"Resource temporarily unavailable",
            12 => b"Cannot allocate memory",
            13 => b"Permission denied",
            14 => b"Bad address",
            16 => b"Device or resource busy",
            17 => b"File exists",
            18 => b"Invalid cross-device link",
            20 => b"Not a directory",
            21 => b"Is a directory",
            22 => b"Invalid argument",
            24 => b"Too many open files",
            25 => b"Inappropriate ioctl for device",
            27 => b"File too large",
            28 => b"No space left on device",
            29 => b"Illegal seek",
            30 => b"Read-only file system",
            32 => b"Broken pipe",
            34 => b"Numerical result out of range",
            36 => b"File name too long",
            38 => b"Function not implemented",
            39 => b"Directory not empty",
            40 => b"Too many levels of symbolic links",
            75 => b"Value too large for defined data type",
            95 => b"Operation not supported",
            _ => b"Unknown error",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_negative_result_is_its_error_and_others_are_values() {
        assert_eq!(Errno::result(3), Ok(3));
        assert_eq!(Errno::result(0), Ok(0));
        assert_eq!(Errno::result(-2), Err(Errno::ENOENT));
        assert_eq!(Errno::result(isize::MIN), Err(Errno(u16::MAX)));
    }

    #[test]
    fn messages_are_strerror_s() {
        assert_eq!(Errno::ENOENT.message(), b"No such file or directory");
        assert_eq!(Errno::EISDIR.message(), b"Is a directory");
        assert_eq!(Errno(4095).message(), b"Unknown error");
    }
}
