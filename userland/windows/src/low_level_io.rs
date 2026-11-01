//! The C runtime's low-level I/O (`io.h`, `fcntl.h`, `share.h`; D179): what `_open`'s
//! `_O_*` flags, `_sopen_s`'s sharing mode and `_lseek`'s origin ask of Linux's `openat` and
//! `lseek`, kept here, outside the layer's Win32 code, so its host tests run.

/// `_open`'s flags (`_O_*`).
pub mod windows_flag {
    pub const RDONLY: i32 = 0x0;
    pub const WRONLY: i32 = 0x1;
    pub const RDWR: i32 = 0x2;
    pub const ACCESS: i32 = 0x3;
    pub const APPEND: i32 = 0x8;
    pub const RANDOM: i32 = 0x10;
    pub const SEQUENTIAL: i32 = 0x20;
    pub const TEMPORARY: i32 = 0x40;
    pub const NOINHERIT: i32 = 0x80;
    pub const CREAT: i32 = 0x100;
    pub const TRUNC: i32 = 0x200;
    pub const EXCL: i32 = 0x400;
    pub const SHORT_LIVED: i32 = 0x1000;
    pub const OBTAIN_DIR: i32 = 0x2000;
    pub const TEXT: i32 = 0x4000;
    pub const BINARY: i32 = 0x8000;
    pub const WTEXT: i32 = 0x1_0000;
    pub const U16TEXT: i32 = 0x2_0000;
    pub const U8TEXT: i32 = 0x4_0000;
    /// Every flag `_open` knows.
    pub const KNOWN: i32 = ACCESS
        | APPEND
        | RANDOM
        | SEQUENTIAL
        | TEMPORARY
        | NOINHERIT
        | CREAT
        | TRUNC
        | EXCL
        | SHORT_LIVED
        | OBTAIN_DIR
        | TEXT
        | BINARY
        | WTEXT
        | U16TEXT
        | U8TEXT;
}

/// Linux's `openat` flags the low-level functions ask for.
pub mod linux_flag {
    pub const RDONLY: usize = 0;
    pub const WRONLY: usize = 1;
    pub const RDWR: usize = 2;
    pub const CREAT: usize = 0o100;
    pub const EXCL: usize = 0o200;
    pub const TRUNC: usize = 0o1000;
    pub const APPEND: usize = 0o2000;
}

/// `_sopen_s`'s sharing modes (`_SH_*`).
pub mod share {
    pub const DENY_READ_WRITE: i32 = 0x10;
    pub const DENY_WRITE: i32 = 0x20;
    pub const DENY_READ: i32 = 0x30;
    pub const DENY_NONE: i32 = 0x40;
}

/// What `_open`'s `flags` ask of `openat`: its flags, the file to remove once opened
/// (`_O_TEMPORARY`, which Windows removes at the last close and EggOS removes at once, keeping
/// it while it is open). `None` for an access mode Windows has not (both `_O_WRONLY` and
/// `_O_RDWR`) or an unknown flag, which the runtime answers with `EINVAL`.
pub fn open_flags(flags: i32) -> Option<(usize, bool)> {
    if flags & !windows_flag::KNOWN != 0 {
        return None;
    }
    let access = match flags & windows_flag::ACCESS {
        windows_flag::RDONLY => linux_flag::RDONLY,
        windows_flag::WRONLY => linux_flag::WRONLY,
        windows_flag::RDWR => linux_flag::RDWR,
        _ => return None,
    };
    let asked = [
        (windows_flag::APPEND, linux_flag::APPEND),
        (windows_flag::CREAT, linux_flag::CREAT),
        (windows_flag::TRUNC, linux_flag::TRUNC),
        (windows_flag::EXCL, linux_flag::EXCL),
    ]
    .into_iter()
    .filter(|&(windows, _)| flags & windows != 0)
    .fold(access, |linux, (_, flag)| linux | flag);
    Some((asked, flags & windows_flag::TEMPORARY != 0))
}

/// Whether `share` is one of `_sopen_s`'s sharing modes. EggOS's files are always shared, as
/// Linux's are, so a mode is checked and then has no effect.
pub fn is_sharing_mode(share: i32) -> bool {
    matches!(
        share,
        share::DENY_READ_WRITE | share::DENY_WRITE | share::DENY_READ | share::DENY_NONE
    )
}

/// `lseek`'s `whence` for `_lseek`'s `origin` (`SEEK_SET`, `SEEK_CUR`, `SEEK_END`: the same
/// numbers); `None` for another.
pub fn whence(origin: i32) -> Option<usize> {
    match origin {
        0..=2 => Some(origin as usize),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_flags_become_linux_s_and_unknown_ones_are_refused() {
        assert_eq!(open_flags(windows_flag::RDONLY), Some((0, false)));
        assert_eq!(
            open_flags(
                windows_flag::WRONLY
                    | windows_flag::CREAT
                    | windows_flag::TRUNC
                    | windows_flag::BINARY
            ),
            Some((
                linux_flag::WRONLY | linux_flag::CREAT | linux_flag::TRUNC,
                false
            ))
        );
        assert_eq!(
            open_flags(
                windows_flag::RDWR
                    | windows_flag::APPEND
                    | windows_flag::EXCL
                    | windows_flag::CREAT
                    | windows_flag::TEMPORARY
                    | windows_flag::NOINHERIT
            ),
            Some((
                linux_flag::RDWR | linux_flag::APPEND | linux_flag::EXCL | linux_flag::CREAT,
                true
            ))
        );
        assert_eq!(open_flags(windows_flag::WRONLY | windows_flag::RDWR), None);
        assert_eq!(open_flags(0x0800), None);
        assert_eq!(open_flags(0x0010_0000), None);
    }

    #[test]
    fn sharing_modes_and_origins_are_checked() {
        assert!(is_sharing_mode(share::DENY_NONE));
        assert!(is_sharing_mode(share::DENY_READ_WRITE));
        assert!(!is_sharing_mode(0));
        assert!(!is_sharing_mode(0x50));
        assert_eq!(whence(0), Some(0));
        assert_eq!(whence(2), Some(2));
        assert_eq!(whence(3), None);
        assert_eq!(whence(-1), None);
    }
}
