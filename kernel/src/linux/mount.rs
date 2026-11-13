//! `mount(2)` for changing the options of a file system already mounted (D562): Linux's
//! `MS_REMOUNT` with `nosuid`, `noexec` and the access-time options. EggOS mounts its file
//! systems itself at boot, so mounting a new one is `ENODEV`.

use super::Errno;
use super::files::{PathBuffer, read_path};
use crate::process::Process;
use crate::vfs::{self, AccessTimes, MountOptions};
use eggos_hal::Platform;

/// The `mount` flags EggOS knows, as Linux numbers them.
pub mod flag {
    /// `MS_RDONLY`.
    pub const READ_ONLY: u64 = 1;
    /// `MS_NOSUID`.
    pub const NO_SET_IDS: u64 = 2;
    /// `MS_NODEV`.
    pub const NO_DEVICES: u64 = 4;
    /// `MS_NOEXEC`.
    pub const NO_EXECUTION: u64 = 8;
    /// `MS_REMOUNT`.
    pub const REMOUNT: u64 = 32;
    /// `MS_NOATIME`.
    pub const NO_ACCESS_TIMES: u64 = 1 << 10;
    /// `MS_NODIRATIME`.
    pub const NO_DIRECTORY_ACCESS_TIMES: u64 = 1 << 11;
    /// `MS_BIND`.
    pub const BIND: u64 = 1 << 12;
    /// `MS_SILENT`.
    pub const SILENT: u64 = 1 << 15;
    /// `MS_RELATIME`.
    pub const RELATIVE_ACCESS_TIMES: u64 = 1 << 21;
    /// `MS_STRICTATIME`.
    pub const STRICT_ACCESS_TIMES: u64 = 1 << 24;
    /// `MS_LAZYTIME`.
    pub const LAZY_TIMES: u64 = 1 << 25;
    /// The magic number old callers put in the top half (`MS_MGC_VAL`).
    pub const OLD_MAGIC_MASK: u64 = 0xffff_0000;
    /// `MS_MGC_VAL`.
    pub const OLD_MAGIC: u64 = 0xc0ed_0000;
}

/// The access times a remount with `flags` asks for: `MS_NOATIME` before `MS_STRICTATIME`,
/// `relatime` otherwise, as Linux.
pub fn access_times_of(flags: u64) -> AccessTimes {
    if flags & flag::NO_ACCESS_TIMES != 0 {
        AccessTimes::Never
    } else if flags & flag::STRICT_ACCESS_TIMES != 0 {
        AccessTimes::Strict
    } else {
        AccessTimes::Relative
    }
}

/// `mount(source, target, type, flags, data)`: with `MS_REMOUNT`, changes the options of the
/// file system mounted on `target`; `source`, `type` and `data` are not read. As Linux:
/// `EPERM` unless privileged, `EFAULT` and the path's errors for `target`, `EINVAL` when
/// nothing is mounted there or `MS_RDONLY` would change a writable file system (EggOS mounts
/// read-write ones read-write for good), `ENODEV` for anything but a remount.
pub fn mount<P: Platform>(
    process: &Process<P>,
    target: u64,
    flags: u64,
) -> Result<u64, Errno> {
    if !super::time::is_privileged(process) {
        return Err(Errno::EPERM);
    }
    let flags = if flags & flag::OLD_MAGIC_MASK == flag::OLD_MAGIC {
        flags & !flag::OLD_MAGIC_MASK
    } else {
        flags
    };
    if flags & flag::REMOUNT == 0 {
        return Err(Errno::ENODEV);
    }
    let mut buffer = PathBuffer::uninit();
    let path = read_path(process, target, &mut buffer)?;
    if path.is_empty() {
        return Err(Errno::ENOENT);
    }
    let root = vfs::root();
    let point = vfs::resolve_as(root, root, path, process.id().get(), true)
        .map_err(super::files::walk_error)?;
    let usage = point.file_system().usage();
    if (flags & flag::READ_ONLY != 0) != usage.read_only {
        return Err(Errno::EINVAL);
    }
    let options = MountOptions {
        no_set_ids: flags & flag::NO_SET_IDS != 0,
        no_execution: flags & flag::NO_EXECUTION != 0,
        access_times: access_times_of(flags),
    };
    vfs::remount(point, options).map_err(|_| Errno::EINVAL)?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noatime_wins_over_strictatime() {
        assert_eq!(
            access_times_of(flag::NO_ACCESS_TIMES | flag::STRICT_ACCESS_TIMES),
            AccessTimes::Never
        );
        assert_eq!(access_times_of(flag::STRICT_ACCESS_TIMES), AccessTimes::Strict);
        assert_eq!(access_times_of(flag::REMOUNT), AccessTimes::Relative);
    }
}
