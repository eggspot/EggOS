//! Android's property service (D188): the system properties Android programs read with
//! bionic's `__system_property_get`, kept by the kernel in one property area in bionic's format
//! ([`eggos_android_properties`]), which `/dev/__properties__` shows as a read-only file that
//! bionic maps at start-up ([`crate::vfs::devfs`]).
//!
//! The area is made on first use, [`FIRST_AREA`] bytes, and grows to at most [`LARGEST_AREA`]
//! as properties are added; a program that mapped it earlier keeps the size it mapped and finds
//! the properties that fit there. At boot [`load_build_properties`] sets the properties
//! Android's own `/system/build.prop` lists, which images with Android programs have; after
//! that only a privileged process changes them, with [`SET_REQUEST`] on the file
//! (`setprop`), and a read-only (`ro.`) property keeps its first value. A system without
//! Android programs never touches the area and it costs nothing.
//!
//! The lock is held while the area changes or is copied out, never while a file is read.

use super::Errno;
use crate::process::Process;
use crate::regions::FilePages;
use crate::sync::SpinLock;
use crate::vfs::{self, NodeKind};
use alloc::vec::Vec;
use eggos_android_properties::{self as format, PropertyError};
use eggos_hal::Platform;

pub use eggos_android_properties::SET_REQUEST;

/// The area's size when it is made.
pub const FIRST_AREA: usize = 4096;

/// The most the area grows to.
pub const LARGEST_AREA: usize = 64 * 1024;

const LARGEST_BUILD_PROPERTIES: u64 = 64 * 1024;

static AREA: SpinLock<Vec<u8>> = SpinLock::new(Vec::new());

/// The area's size in bytes, as `stat` reports it: [`FIRST_AREA`] before it is made.
pub fn size() -> u64 {
    let area = AREA.lock();
    match area.len() {
        0 => FIRST_AREA as u64,
        length => length as u64,
    }
}

/// Copies the area's bytes from `offset` into `buffer`, making it first: how many, 0 at its
/// end or without memory for it.
pub fn read_into(offset: u64, buffer: &mut [u8]) -> usize {
    let mut area = AREA.lock();
    if made(&mut area).is_err() {
        return 0;
    }
    let Some(rest) = usize::try_from(offset)
        .ok()
        .and_then(|offset| area.get(offset..))
    else {
        return 0;
    };
    let length = rest.len().min(buffer.len());
    buffer[..length].copy_from_slice(&rest[..length]);
    length
}

/// Sets property `name` to `value`: `EINVAL` for a name or value that cannot be a property's,
/// `EROFS` for a read-only property that has a value, `ENOSPC` when the area is at
/// [`LARGEST_AREA`], `ENOMEM` without memory to grow it.
pub fn set(name: &[u8], value: &[u8]) -> Result<(), Errno> {
    let mut area = AREA.lock();
    made(&mut area)?;
    loop {
        match format::set(area.as_mut_slice(), name, value) {
            Ok(()) => return Ok(()),
            Err(PropertyError::Full { needed }) => grow(&mut area, needed)?,
            Err(PropertyError::InvalidName | PropertyError::InvalidValue) => {
                return Err(Errno::EINVAL);
            }
            Err(PropertyError::ReadOnly) => return Err(Errno::EROFS),
            Err(PropertyError::NotAnArea | PropertyError::TooDeep) => return Err(Errno::EIO),
        }
    }
}

/// The value of property `name`, copied into `buffer`: its length, `None` when it has none.
pub fn get(name: &[u8], buffer: &mut [u8; format::VALUE_MAX]) -> Option<usize> {
    let area = AREA.lock();
    let value = format::get(&area, name)?;
    buffer.get_mut(..value.len())?.copy_from_slice(value);
    Some(value.len())
}

/// `ioctl(fd, request, argument)` on `/dev/__properties__`: [`SET_REQUEST`] reads a request
/// ([`format::encode_request`]) at `argument` and sets the property, for a privileged process
/// only (`EPERM`); `EFAULT` for a request that is not the program's memory, `EINVAL` for one
/// whose lengths do not fit, then [`set`]'s errors. Any other request is `ENOTTY`.
pub fn ioctl<P: Platform>(process: &Process<P>, request: u64, argument: u64) -> Result<u64, Errno> {
    if request as u32 != SET_REQUEST {
        return Err(Errno::ENOTTY);
    }
    if !super::time::is_privileged(process) {
        return Err(Errno::EPERM);
    }
    let mut bytes = [0u8; format::REQUEST_SIZE];
    process
        .read_user_memory(argument, &mut bytes)
        .map_err(|_| Errno::EFAULT)?;
    let (name, value) = format::decode_request(&bytes).ok_or(Errno::EINVAL)?;
    set(name, value).map(|()| 0)
}

/// Sets the properties `/system/build.prop` lists, if the root has that file: once, at boot.
/// Lines that set nothing, and properties that cannot be set, are skipped.
pub fn load_build_properties() {
    let root = vfs::root();
    let Ok(node) = vfs::resolve(root, root, format::BUILD_PROPERTIES_PATH) else {
        return;
    };
    if node.kind() != NodeKind::Regular {
        return;
    }
    let Ok(size) = usize::try_from(node.attributes().size.min(LARGEST_BUILD_PROPERTIES)) else {
        return;
    };
    let mut text = Vec::new();
    if text.try_reserve_exact(size).is_err() {
        return;
    }
    text.resize(size, 0);
    let Some(read) = node.read_at(0, &mut text) else {
        return;
    };
    for line in text[..read].split(|&byte| byte == b'\n') {
        if let Some((name, value)) = format::assignment(line) {
            let _ = set(name, value);
        }
    }
}

fn made(area: &mut Vec<u8>) -> Result<(), Errno> {
    if !area.is_empty() {
        return Ok(());
    }
    area.try_reserve_exact(FIRST_AREA)
        .map_err(|_| Errno::ENOMEM)?;
    area.resize(FIRST_AREA, 0);
    format::initialize(area).map_err(|_| Errno::ENOMEM)
}

fn grow(area: &mut Vec<u8>, needed: usize) -> Result<(), Errno> {
    if needed > LARGEST_AREA {
        return Err(Errno::ENOSPC);
    }
    let length = needed
        .max(area.len().saturating_mul(2))
        .next_multiple_of(FIRST_AREA)
        .min(LARGEST_AREA);
    area.try_reserve_exact(length - area.len())
        .map_err(|_| Errno::ENOMEM)?;
    area.resize(length, 0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_area_grows_by_pages_up_to_its_largest_size() {
        let mut area = Vec::new();
        made(&mut area).unwrap();
        assert_eq!(area.len(), FIRST_AREA);
        assert!(format::is_area(&area));
        grow(&mut area, FIRST_AREA + 1).unwrap();
        assert_eq!(area.len(), 2 * FIRST_AREA);
        grow(&mut area, 3 * FIRST_AREA + 5).unwrap();
        assert_eq!(area.len(), 4 * FIRST_AREA);
        grow(&mut area, LARGEST_AREA - 1).unwrap();
        assert_eq!(area.len(), LARGEST_AREA);
        assert_eq!(grow(&mut area, LARGEST_AREA + 1), Err(Errno::ENOSPC));
        assert!(format::is_area(&area));
    }

    #[test]
    fn set_properties_are_read_back_and_read_only_ones_keep_their_value() {
        set(b"eggos.test.kernel_property", b"one").unwrap();
        let mut value = [0; format::VALUE_MAX];
        let length = get(b"eggos.test.kernel_property", &mut value).unwrap();
        assert_eq!(&value[..length], b"one");
        set(b"ro.eggos.test.kernel_property", b"first").unwrap();
        assert_eq!(
            set(b"ro.eggos.test.kernel_property", b"second"),
            Err(Errno::EROFS)
        );
        assert_eq!(set(b"eggos..test", b"x"), Err(Errno::EINVAL));
        assert_eq!(set(b"eggos.test.long", &[b'v'; 92]), Err(Errno::EINVAL));
        let mut header = [0; 16];
        assert_eq!(read_into(0, &mut header), 16);
        assert_eq!(&header[8..12], &format::MAGIC.to_ne_bytes());
        assert!(size() >= FIRST_AREA as u64);
        assert_eq!(read_into(u64::MAX, &mut header), 0);
    }

    #[test]
    fn many_properties_grow_the_area_and_each_stays_readable() {
        for index in 0..60 {
            let name = alloc::format!("eggos.test.grown{index}");
            set(name.as_bytes(), b"a value long enough to fill the area").unwrap();
        }
        assert!(size() > FIRST_AREA as u64);
        let mut value = [0; format::VALUE_MAX];
        assert!(get(b"eggos.test.grown59", &mut value).is_some());
    }
}
