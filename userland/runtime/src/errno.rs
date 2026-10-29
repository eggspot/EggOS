//! Linux's error numbers, as system calls return them negated, and what each means.

/// A system call's error number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Errno(pub u16);

impl Errno {
    /// `EPERM`.
    pub const EPERM: Self = Self(1);
    /// `ENOENT`.
    pub const ENOENT: Self = Self(2);
    /// `ESRCH`.
    pub const ESRCH: Self = Self(3);
    /// `EINTR`.
    pub const EINTR: Self = Self(4);
    /// `EIO`.
    pub const EIO: Self = Self(5);
    /// `ENXIO`.
    pub const ENXIO: Self = Self(6);
    /// `E2BIG`.
    pub const E2BIG: Self = Self(7);
    /// `ENOEXEC`.
    pub const ENOEXEC: Self = Self(8);
    /// `EAGAIN`.
    pub const EAGAIN: Self = Self(11);
    /// `EACCES`.
    pub const EACCES: Self = Self(13);
    /// `EBUSY`.
    pub const EBUSY: Self = Self(16);
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
    /// `EPIPE`.
    pub const EPIPE: Self = Self(32);
    /// `ENOSPC`.
    pub const ENOSPC: Self = Self(28);
    /// `ENOTEMPTY`.
    pub const ENOTEMPTY: Self = Self(39);
    /// `ENOMEM`.
    pub const ENOMEM: Self = Self(12);
    /// `ENODEV`.
    pub const ENODEV: Self = Self(19);
    /// `EOPNOTSUPP`.
    pub const EOPNOTSUPP: Self = Self(95);
    /// `ENETDOWN`.
    pub const ENETDOWN: Self = Self(100);
    /// `ENETUNREACH`.
    pub const ENETUNREACH: Self = Self(101);
    /// `ECONNRESET`.
    pub const ECONNRESET: Self = Self(104);
    /// `ETIMEDOUT`.
    pub const ETIMEDOUT: Self = Self(110);
    /// `ECONNREFUSED`.
    pub const ECONNREFUSED: Self = Self(111);
    /// `EHOSTUNREACH`.
    pub const EHOSTUNREACH: Self = Self(113);
    /// `EINPROGRESS`.
    pub const EINPROGRESS: Self = Self(115);

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
            19 => b"No such device",
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
            100 => b"Network is down",
            101 => b"Network is unreachable",
            104 => b"Connection reset by peer",
            110 => b"Connection timed out",
            111 => b"Connection refused",
            113 => b"No route to host",
            115 => b"Operation now in progress",
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
