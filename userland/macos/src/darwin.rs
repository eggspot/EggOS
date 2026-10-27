//! Darwin's numbers and structure layouts and their Linux counterparts: open flags, error
//! numbers, clocks, signals, `struct stat` and `struct dirent`. Pure translation, host-tested.

/// The Linux structure layouts that differ between architectures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxLayout {
    X86_64,
    Generic,
}

impl LinuxLayout {
    pub const HOST: Self = if cfg!(target_arch = "x86_64") {
        Self::X86_64
    } else {
        Self::Generic
    };

    fn directory_flag(self) -> u32 {
        match self {
            Self::X86_64 => 0o200_000,
            Self::Generic => 0o40_000,
        }
    }

    fn no_follow_flag(self) -> u32 {
        match self {
            Self::X86_64 => 0o400_000,
            Self::Generic => 0o100_000,
        }
    }
}

pub mod open {
    pub const ACCESS_MODE: u32 = 3;
    pub const NONBLOCK: u32 = 0x4;
    pub const APPEND: u32 = 0x8;
    pub const NOFOLLOW: u32 = 0x100;
    pub const CREAT: u32 = 0x200;
    pub const TRUNC: u32 = 0x400;
    pub const EXCL: u32 = 0x800;
    pub const NOCTTY: u32 = 0x2_0000;
    pub const DIRECTORY: u32 = 0x10_0000;
    pub const CLOEXEC: u32 = 0x100_0000;
}

/// Linux's `open` flags for Darwin's `flags`, or `None` if they ask for something Linux's
/// `open` cannot do.
pub fn linux_open_flags(flags: u32, layout: LinuxLayout) -> Option<u32> {
    let known = open::ACCESS_MODE
        | open::NONBLOCK
        | open::APPEND
        | open::NOFOLLOW
        | open::CREAT
        | open::TRUNC
        | open::EXCL
        | open::NOCTTY
        | open::DIRECTORY
        | open::CLOEXEC;
    if flags & !known != 0 || flags & open::ACCESS_MODE == 3 {
        return None;
    }
    let pairs = [
        (open::NONBLOCK, 0o4000),
        (open::APPEND, 0o2000),
        (open::NOFOLLOW, layout.no_follow_flag()),
        (open::CREAT, 0o100),
        (open::TRUNC, 0o1000),
        (open::EXCL, 0o200),
        (open::NOCTTY, 0o400),
        (open::DIRECTORY, layout.directory_flag()),
        (open::CLOEXEC, 0o2_000_000),
    ];
    Some(
        pairs
            .iter()
            .filter(|(darwin, _)| flags & darwin != 0)
            .fold(flags & open::ACCESS_MODE, |linux, (_, bit)| linux | bit),
    )
}

pub const ENOSYS: i32 = 78;
pub const EINVAL: i32 = 22;
pub const ENOMEM: i32 = 12;
pub const ERANGE: i32 = 34;
pub const EBADF: i32 = 9;

/// Darwin's error number for Linux's `errno`.
pub fn darwin_errno(linux: u16) -> i32 {
    match linux {
        1..=10 | 12..=34 => i32::from(linux),
        11 => 35,
        35 => 11,
        36 => 63,
        37 => 77,
        38 => ENOSYS,
        39 => 66,
        40 => 62,
        61 => 96,
        62 => 101,
        71 => 100,
        74 => 94,
        75 => 84,
        84 => 92,
        95 => 45,
        110 => 60,
        111 => 61,
        113 => 65,
        114 => 37,
        115 => 36,
        125 => 89,
        _ => EINVAL,
    }
}

/// Linux's clock for Darwin's `clockid_t`.
pub fn linux_clock(darwin: u32) -> Option<usize> {
    match darwin {
        0 => Some(0),
        4 | 5 | 6 | 8 | 9 => Some(1),
        12 => Some(2),
        16 => Some(3),
        _ => None,
    }
}

/// Linux's signal for Darwin's signal number.
pub fn linux_signal(darwin: i32) -> Option<usize> {
    let linux = match darwin {
        0..=6 | 8 | 9 | 11 | 13..=15 => darwin,
        10 => 7,
        12 => 31,
        16 => 23,
        17 => 19,
        18 => 20,
        19 => 18,
        20 => 17,
        21 => 21,
        22 => 22,
        23 => 29,
        24 => 24,
        25 => 25,
        26 => 26,
        27 => 27,
        28 => 28,
        30 => 10,
        31 => 12,
        _ => return None,
    };
    Some(linux as usize)
}

/// A file's status, read from Linux's `struct stat`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Status {
    pub device: u64,
    pub inode: u64,
    pub mode: u32,
    pub links: u64,
    pub user: u32,
    pub group: u32,
    pub special_device: u64,
    pub size: i64,
    pub block_size: i64,
    pub blocks: i64,
    pub times: [[i64; 2]; 3],
}

pub const LINUX_STAT_SIZE: usize = 144;
pub const DARWIN_STAT_SIZE: usize = 144;

fn word(bytes: &[u8], offset: usize) -> u64 {
    let mut value = [0u8; 8];
    value.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_le_bytes(value)
}

fn half(bytes: &[u8], offset: usize) -> u32 {
    let mut value = [0u8; 4];
    value.copy_from_slice(&bytes[offset..offset + 4]);
    u32::from_le_bytes(value)
}

impl Status {
    /// Reads Linux's `struct stat` for `layout`.
    pub fn from_linux(bytes: &[u8; LINUX_STAT_SIZE], layout: LinuxLayout) -> Self {
        let times = |at: usize| {
            [
                [word(bytes, at) as i64, word(bytes, at + 8) as i64],
                [word(bytes, at + 16) as i64, word(bytes, at + 24) as i64],
                [word(bytes, at + 32) as i64, word(bytes, at + 40) as i64],
            ]
        };
        match layout {
            LinuxLayout::X86_64 => Self {
                device: word(bytes, 0),
                inode: word(bytes, 8),
                links: word(bytes, 16),
                mode: half(bytes, 24),
                user: half(bytes, 28),
                group: half(bytes, 32),
                special_device: word(bytes, 40),
                size: word(bytes, 48) as i64,
                block_size: word(bytes, 56) as i64,
                blocks: word(bytes, 64) as i64,
                times: times(72),
            },
            LinuxLayout::Generic => Self {
                device: word(bytes, 0),
                inode: word(bytes, 8),
                mode: half(bytes, 16),
                links: u64::from(half(bytes, 20)),
                user: half(bytes, 24),
                group: half(bytes, 28),
                special_device: word(bytes, 32),
                size: word(bytes, 48) as i64,
                block_size: i64::from(half(bytes, 56) as i32),
                blocks: word(bytes, 64) as i64,
                times: times(72),
            },
        }
    }

    /// Darwin's `struct stat` (64-bit inodes, the only layout on arm64 and `stat$INODE64` on
    /// x86_64): the access, modification and change times, then the birth time, which Linux's
    /// `stat` lacks and which gets the change time.
    pub fn to_darwin(self) -> [u8; DARWIN_STAT_SIZE] {
        let mut out = [0u8; DARWIN_STAT_SIZE];
        let mut put = |offset: usize, bytes: &[u8]| {
            out[offset..offset + bytes.len()].copy_from_slice(bytes);
        };
        put(0, &(self.device as i32).to_le_bytes());
        put(4, &(self.mode as u16).to_le_bytes());
        put(
            6,
            &(self.links.min(u64::from(u16::MAX)) as u16).to_le_bytes(),
        );
        put(8, &self.inode.to_le_bytes());
        put(16, &self.user.to_le_bytes());
        put(20, &self.group.to_le_bytes());
        put(24, &(self.special_device as i32).to_le_bytes());
        let [access, modification, change] = self.times;
        for (index, time) in [access, modification, change, change].iter().enumerate() {
            put(32 + index * 16, &time[0].to_le_bytes());
            put(40 + index * 16, &time[1].to_le_bytes());
        }
        put(96, &self.size.to_le_bytes());
        put(104, &self.blocks.to_le_bytes());
        put(112, &(self.block_size as i32).to_le_bytes());
        out
    }
}

pub const DARWIN_DIRENT_SIZE: usize = 1048;
pub const DARWIN_NAME_MAX: usize = 1023;

/// Writes Darwin's `struct dirent` (64-bit inodes) for one entry into `out`: `d_ino`,
/// `d_seekoff`, `d_reclen`, `d_namlen`, `d_type` and the NUL-terminated name, cut at 1023 bytes.
pub fn darwin_dirent(
    inode: u64,
    offset: u64,
    kind: u8,
    name: &[u8],
    out: &mut [u8; DARWIN_DIRENT_SIZE],
) {
    let name = &name[..name.len().min(DARWIN_NAME_MAX)];
    let record = (21 + name.len() + 1).next_multiple_of(8) as u16;
    out[..21].fill(0);
    out[0..8].copy_from_slice(&inode.to_le_bytes());
    out[8..16].copy_from_slice(&offset.to_le_bytes());
    out[16..18].copy_from_slice(&record.to_le_bytes());
    out[18..20].copy_from_slice(&(name.len() as u16).to_le_bytes());
    out[20] = kind;
    out[21..21 + name.len()].copy_from_slice(name);
    out[21 + name.len()] = 0;
}

/// One entry of Linux's `getdents64` buffer: `(inode, offset, type, name, record length)`.
pub fn linux_dirent(bytes: &[u8]) -> Option<(u64, u64, u8, &[u8], usize)> {
    let header = bytes.get(..19)?;
    let inode = word(header, 0);
    let offset = word(header, 8);
    let record = usize::from(u16::from_le_bytes([header[16], header[17]]));
    let kind = header[18];
    if record < 20 || record > bytes.len() {
        return None;
    }
    let name = &bytes[19..record];
    let end = name.iter().position(|&b| b == 0)?;
    Some((inode, offset, kind, &name[..end], record))
}

/// A Darwin symbol name without the variant suffix that picks a newer ABI of the same
/// function (`$INODE64`, `$UNIX2003`, `$DARWIN_EXTSN`, `$NOCANCEL`, `$1050`): the layer
/// implements only those ABIs, so both names reach the same function.
pub fn base_symbol(name: &[u8]) -> &[u8] {
    let end = name.iter().position(|&b| b == b'$').unwrap_or(name.len());
    &name[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_flags_become_linux_flags() {
        let x86 = LinuxLayout::X86_64;
        let arm = LinuxLayout::Generic;
        assert_eq!(linux_open_flags(0, x86), Some(0));
        assert_eq!(
            linux_open_flags(1 | open::CREAT | open::TRUNC, x86),
            Some(1 | 0o100 | 0o1000)
        );
        assert_eq!(
            linux_open_flags(2 | open::APPEND | open::EXCL, arm),
            Some(2 | 0o2000 | 0o200)
        );
        assert_eq!(
            linux_open_flags(open::DIRECTORY | open::CLOEXEC, x86),
            Some(0o200_000 | 0o2_000_000)
        );
        assert_eq!(
            linux_open_flags(open::DIRECTORY | open::NOFOLLOW, arm),
            Some(0o40_000 | 0o100_000)
        );
        assert_eq!(linux_open_flags(3, x86), None);
        assert_eq!(linux_open_flags(0x4000_0000, x86), None);
    }

    #[test]
    fn error_numbers_use_darwins_values() {
        assert_eq!(darwin_errno(2), 2);
        assert_eq!(darwin_errno(30), 30);
        assert_eq!(darwin_errno(11), 35);
        assert_eq!(darwin_errno(38), 78);
        assert_eq!(darwin_errno(39), 66);
        assert_eq!(darwin_errno(36), 63);
        assert_eq!(darwin_errno(500), EINVAL);
    }

    #[test]
    fn clocks_and_signals_translate() {
        assert_eq!(linux_clock(0), Some(0));
        assert_eq!(linux_clock(6), Some(1));
        assert_eq!(linux_clock(12), Some(2));
        assert_eq!(linux_clock(1), None);
        assert_eq!(linux_signal(6), Some(6));
        assert_eq!(linux_signal(30), Some(10));
        assert_eq!(linux_signal(20), Some(17));
        assert_eq!(linux_signal(99), None);
    }

    fn linux_stat(layout: LinuxLayout) -> [u8; LINUX_STAT_SIZE] {
        let mut bytes = [0u8; LINUX_STAT_SIZE];
        let mut put = |offset: usize, value: &[u8]| {
            bytes[offset..offset + value.len()].copy_from_slice(value);
        };
        put(0, &7u64.to_le_bytes());
        put(8, &99u64.to_le_bytes());
        match layout {
            LinuxLayout::X86_64 => {
                put(16, &2u64.to_le_bytes());
                put(24, &0o100_644u32.to_le_bytes());
                put(56, &4096u64.to_le_bytes());
            }
            LinuxLayout::Generic => {
                put(16, &0o100_644u32.to_le_bytes());
                put(20, &2u32.to_le_bytes());
                put(56, &4096u32.to_le_bytes());
            }
        }
        put(48, &6i64.to_le_bytes());
        put(64, &8i64.to_le_bytes());
        put(88, &1_700_000_000i64.to_le_bytes());
        put(96, &5i64.to_le_bytes());
        bytes
    }

    #[test]
    fn stat_becomes_darwins_layout_on_both_linux_layouts() {
        for layout in [LinuxLayout::X86_64, LinuxLayout::Generic] {
            let status = Status::from_linux(&linux_stat(layout), layout);
            assert_eq!(status.mode, 0o100_644);
            assert_eq!(status.links, 2);
            assert_eq!(status.size, 6);
            assert_eq!(status.block_size, 4096);
            let darwin = status.to_darwin();
            assert_eq!(i32::from_le_bytes(darwin[0..4].try_into().unwrap()), 7);
            assert_eq!(
                u16::from_le_bytes(darwin[4..6].try_into().unwrap()),
                0o100_644
            );
            assert_eq!(u16::from_le_bytes(darwin[6..8].try_into().unwrap()), 2);
            assert_eq!(u64::from_le_bytes(darwin[8..16].try_into().unwrap()), 99);
            assert_eq!(
                i64::from_le_bytes(darwin[48..56].try_into().unwrap()),
                1_700_000_000
            );
            assert_eq!(i64::from_le_bytes(darwin[56..64].try_into().unwrap()), 5);
            assert_eq!(i64::from_le_bytes(darwin[96..104].try_into().unwrap()), 6);
            assert_eq!(i64::from_le_bytes(darwin[104..112].try_into().unwrap()), 8);
            assert_eq!(
                i32::from_le_bytes(darwin[112..116].try_into().unwrap()),
                4096
            );
        }
    }

    #[test]
    fn directory_entries_become_darwins_layout() {
        let mut linux = [0u8; 32];
        linux[0..8].copy_from_slice(&5u64.to_le_bytes());
        linux[8..16].copy_from_slice(&3u64.to_le_bytes());
        linux[16..18].copy_from_slice(&32u16.to_le_bytes());
        linux[18] = 8;
        linux[19..24].copy_from_slice(b"about");
        let (inode, offset, kind, name, record) = linux_dirent(&linux).unwrap();
        assert_eq!(
            (inode, offset, kind, name, record),
            (5, 3, 8, &b"about"[..], 32)
        );
        assert_eq!(linux_dirent(&linux[..20]), None);
        let mut darwin = [0xffu8; DARWIN_DIRENT_SIZE];
        darwin_dirent(inode, offset, kind, name, &mut darwin);
        assert_eq!(u16::from_le_bytes([darwin[16], darwin[17]]), 32);
        assert_eq!(u16::from_le_bytes([darwin[18], darwin[19]]), 5);
        assert_eq!(darwin[20], 8);
        assert_eq!(&darwin[21..27], b"about\0");
    }

    #[test]
    fn symbol_variants_reach_the_same_function() {
        assert_eq!(base_symbol(b"_stat$INODE64"), b"_stat");
        assert_eq!(base_symbol(b"_fputs$UNIX2003"), b"_fputs");
        assert_eq!(base_symbol(b"_printf"), b"_printf");
    }
}
