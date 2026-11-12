//! Linux's sync files of the GPU's fences (D460): what `VIRTGPU_EXECBUF_FENCE_FD_OUT` hands
//! out ([`crate::rendering::Fence`]). One turns readable once its fence passed;
//! `SYNC_IOC_MERGE` joins two into a new file and `SYNC_IOC_FILE_INFO` describes one, as
//! `include/uapi/linux/sync_file.h` lays them out.

use super::Errno;
use super::descriptors::FileKind;
use super::poll::INPUT_EVENTS;
use super::thread::LinuxThread;
use crate::process::Process;
use crate::rendering::Fence;
use eggos_hal::Platform;

const SYNC_TYPE: u32 = b'>' as u32;
const SIZE_SHIFT: u32 = 16;
const SIZE_MASK: u32 = 0x1fff;
const MERGE: u32 = 3;
const FILE_INFO: u32 = 4;
const MERGE_BYTES: usize = 48;
const FILE_INFO_BYTES: usize = 56;
const FENCE_INFO_BYTES: usize = 80;
const NAME_BYTES: usize = 32;
const DRIVER_NAME: &[u8] = b"virtio_gpu";
const TIMELINE_NAME: &[u8] = b"eggos-gpu";
const STATUS_ACTIVE: i32 = 0;
const STATUS_SIGNALLED: i32 = 1;

/// The `poll` events of `requested` a sync file reports: readable once its fence `passed`.
pub fn events(passed: bool, requested: u16) -> u16 {
    match passed {
        true => requested & INPUT_EVENTS,
        false => 0,
    }
}

/// A sync file's `ioctl`s: `SYNC_IOC_MERGE` and `SYNC_IOC_FILE_INFO`; `ENOTTY` for the others.
pub fn ioctl<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    fence: Fence,
    request: u64,
    address: u64,
) -> Result<u64, Errno> {
    let request = request as u32;
    if (request >> 8) & 0xff != SYNC_TYPE {
        return Err(Errno::ENOTTY);
    }
    let size = ((request >> SIZE_SHIFT) & SIZE_MASK) as usize;
    match (request & 0xff, size) {
        (MERGE, MERGE_BYTES) => merge(process, thread, fence, address),
        (FILE_INFO, FILE_INFO_BYTES) => file_info(process, fence, address),
        _ => Err(Errno::ENOTTY),
    }
}

fn merge<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    fence: Fence,
    address: u64,
) -> Result<u64, Errno> {
    let mut argument = [0u8; MERGE_BYTES];
    process
        .read_user_memory(address, &mut argument)
        .map_err(|_| Errno::EFAULT)?;
    if word(&argument, 40) != 0 || word(&argument, 44) != 0 {
        return Err(Errno::EINVAL);
    }
    let other = u64::try_from(word(&argument, 32) as i32).map_err(|_| Errno::EBADF)?;
    let other = thread.files().file(other)?;
    let FileKind::GpuFence(other) = other.kind() else {
        return Err(Errno::EINVAL);
    };
    let merged = super::render_node::fence_file(process, thread, fence.merged(*other))?;
    argument[36..40].copy_from_slice(&merged.to_ne_bytes());
    process
        .write_user_memory(address, &argument)
        .map(|()| 0)
        .map_err(|_| Errno::EFAULT)
}

fn file_info<P: Platform>(process: &Process<P>, fence: Fence, address: u64) -> Result<u64, Errno> {
    let mut argument = [0u8; FILE_INFO_BYTES];
    process
        .read_user_memory(address, &mut argument)
        .map_err(|_| Errno::EFAULT)?;
    if word(&argument, 36) != 0 {
        return Err(Errno::EINVAL);
    }
    let status = match fence.has_passed::<P>() {
        true => STATUS_SIGNALLED,
        false => STATUS_ACTIVE,
    };
    let room = word(&argument, 40);
    if room > 0 {
        let mut info = [0u8; FENCE_INFO_BYTES];
        info[..TIMELINE_NAME.len()].copy_from_slice(TIMELINE_NAME);
        info[NAME_BYTES..NAME_BYTES + DRIVER_NAME.len()].copy_from_slice(DRIVER_NAME);
        info[64..68].copy_from_slice(&status.to_ne_bytes());
        let list = u64::from_ne_bytes(argument[48..56].try_into().unwrap_or_default());
        process
            .write_user_memory(list, &info)
            .map_err(|_| Errno::EFAULT)?;
    }
    argument[..NAME_BYTES].fill(0);
    argument[..DRIVER_NAME.len()].copy_from_slice(DRIVER_NAME);
    argument[32..36].copy_from_slice(&status.to_ne_bytes());
    argument[40..44].copy_from_slice(&1u32.to_ne_bytes());
    process
        .write_user_memory(address, &argument)
        .map(|()| 0)
        .map_err(|_| Errno::EFAULT)
}

fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap_or_default())
}
