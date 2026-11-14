//! `/dev/dri/renderD128` (D415): Linux's render node of a virtio GPU that takes 3D commands,
//! the device Mesa's virgl and Venus drivers open. Each opener is a rendering context
//! ([`crate::rendering`]). Its `ioctl`s are DRM's that a render node answers (`VERSION`,
//! `GET_CAP`, `GEM_CLOSE`) and virtio-gpu's (`include/uapi/drm/virtgpu_drm.h`): parameters,
//! capability sets, context creation, 3D resources, transfers, command submission and waits;
//! `mmap` at the offset `VIRTGPU_MAP` gives maps a resource's backing. A command stream is
//! posted and the call returns (D460): its out-fence is a sync file ([`super::sync_file`])
//! that turns readable once the stream is finished, and a wait on a buffer waits for the last
//! stream that used it. Every other command is finished when its call returns. Sync objects
//! are not answered yet.

use super::Errno;
use super::descriptors::{FileKind, OpenFile, open_file_limit, open_flag};
use super::thread::LinuxThread;
use crate::process::{Placement, Process};
use crate::regions::RegionError;
use crate::rendering::{
    CAPSET_ROOM, COMMAND_STREAM_BYTES, Context, Fence, RenderError, ResourceShape, TexelBox,
    Transfer,
};
use alloc::sync::Arc;
use alloc::vec::Vec;
use eggos_hal::Platform;
use eggos_hal::paging::Attributes;

const DRM_TYPE: u32 = b'd' as u32;
const SIZE_SHIFT: u32 = 16;
const SIZE_MASK: u32 = 0x1fff;
const DRIVER_NAME: &[u8] = b"virtio_gpu";
const DRIVER_DATE: &[u8] = b"0";
const DRIVER_DESCRIPTION: &[u8] = b"virtio GPU";
const DRIVER_VERSION: (i32, i32, i32) = (0, 1, 0);
const MAP_OFFSET_SHIFT: u32 = 32;
const LONGEST_ARGUMENT: usize = 64;
const LONGEST_DEBUG_NAME: usize = 64;
const MOST_CONTEXT_PARAMETERS: u32 = 8;

mod number {
    pub const VERSION: u32 = 0x00;
    pub const GEM_CLOSE: u32 = 0x09;
    pub const GET_CAP: u32 = 0x0c;
    pub const PRIME_HANDLE_TO_FD: u32 = 0x2d;
    pub const PRIME_FD_TO_HANDLE: u32 = 0x2e;
    pub const MAP: u32 = 0x41;
    pub const EXECBUFFER: u32 = 0x42;
    pub const GETPARAM: u32 = 0x43;
    pub const RESOURCE_CREATE: u32 = 0x44;
    pub const RESOURCE_INFO: u32 = 0x45;
    pub const TRANSFER_FROM_HOST: u32 = 0x46;
    pub const TRANSFER_TO_HOST: u32 = 0x47;
    pub const WAIT: u32 = 0x48;
    pub const GET_CAPS: u32 = 0x49;
    pub const RESOURCE_CREATE_BLOB: u32 = 0x4a;
    pub const CONTEXT_INIT: u32 = 0x4b;
    pub const SYNCOBJ_CREATE: u32 = 0xbf;
    pub const SYNCOBJ_DESTROY: u32 = 0xc0;
    pub const SYNCOBJ_HANDLE_TO_FD: u32 = 0xc1;
    pub const SYNCOBJ_FD_TO_HANDLE: u32 = 0xc2;
    pub const SYNCOBJ_WAIT: u32 = 0xc3;
    pub const SYNCOBJ_RESET: u32 = 0xc4;
    pub const SYNCOBJ_SIGNAL: u32 = 0xc5;
}

mod syncobj_flag {
    pub const CREATE_SIGNALED: u32 = 1;
    pub const WAIT_ALL: u32 = 1;
    pub const WAIT_FOR_SUBMIT: u32 = 2;
    pub const SYNC_FILE: u32 = 1;
}

const MOST_SYNCOBJS_PER_CALL: u32 = 64;
const SYNCOBJ_ENTRY_BYTES: u32 = 16;

mod capability {
    pub const DUMB_BUFFER: u64 = 0x1;
    pub const PRIME: u64 = 0x5;
    pub const TIMESTAMP_MONOTONIC: u64 = 0x6;
    pub const SYNCOBJ: u64 = 0x13;
    pub const SYNCOBJ_TIMELINE: u64 = 0x14;
}

mod parameter {
    pub const THREE_D_FEATURES: u64 = 1;
    pub const CAPSET_QUERY_FIX: u64 = 2;
    pub const RESOURCE_BLOB: u64 = 3;
    pub const HOST_VISIBLE: u64 = 4;
    pub const CROSS_DEVICE: u64 = 5;
    pub const CONTEXT_INIT: u64 = 6;
    pub const SUPPORTED_CAPSET_IDS: u64 = 7;
    pub const EXPLICIT_DEBUG_NAME: u64 = 8;
}

mod context_parameter {
    pub const CAPSET_ID: u64 = 1;
    pub const NUM_RINGS: u64 = 2;
    pub const POLL_RINGS_MASK: u64 = 3;
    pub const DEBUG_NAME: u64 = 4;
}

mod execbuffer_flag {
    pub const FENCE_FD_IN: u32 = 0x01;
    pub const FENCE_FD_OUT: u32 = 0x02;
    pub const RING_IDX: u32 = 0x04;
}

const WAIT_NOWAIT: u32 = 1;
const PRIME_IMPORT: u64 = 1;
const PRIME_EXPORT: u64 = 2;
const PRIME_CLOEXEC: u32 = 0o2_000_000;
const PRIME_RDWR: u32 = 2;

fn errno(error: RenderError) -> Errno {
    match error {
        RenderError::NoDevice => Errno::ENODEV,
        RenderError::NoMemory => Errno::ENOMEM,
        RenderError::Device => Errno::EIO,
        RenderError::Invalid => Errno::EINVAL,
        RenderError::AlreadyCreated => Errno::EEXIST,
        RenderError::Busy => Errno::EBUSY,
    }
}

/// Opens the render node: a new rendering context. `ENODEV` without a GPU that takes 3D
/// commands.
pub fn open<P: Platform>() -> Result<FileKind, Errno> {
    Context::open::<P>()
        .map(FileKind::RenderNode)
        .map_err(errno)
}

struct Argument {
    bytes: [u8; LONGEST_ARGUMENT],
    size: usize,
}

impl Argument {
    fn read<P: Platform>(process: &Process<P>, address: u64, size: usize) -> Result<Self, Errno> {
        let mut bytes = [0u8; LONGEST_ARGUMENT];
        let size = size.min(LONGEST_ARGUMENT);
        process
            .read_user_memory(address, &mut bytes[..size])
            .map_err(|_| Errno::EFAULT)?;
        Ok(Self { bytes, size })
    }

    fn write_back<P: Platform>(&self, process: &Process<P>, address: u64) -> Result<u64, Errno> {
        process
            .write_user_memory(address, &self.bytes[..self.size])
            .map(|()| 0)
            .map_err(|_| Errno::EFAULT)
    }

    fn u32_at(&self, offset: usize) -> u32 {
        let mut word = [0u8; 4];
        word.copy_from_slice(&self.bytes[offset..offset + 4]);
        u32::from_ne_bytes(word)
    }

    fn u64_at(&self, offset: usize) -> u64 {
        let mut word = [0u8; 8];
        word.copy_from_slice(&self.bytes[offset..offset + 8]);
        u64::from_ne_bytes(word)
    }

    fn set_u32(&mut self, offset: usize, value: u32) {
        self.bytes[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
    }

    fn set_u64(&mut self, offset: usize, value: u64) {
        self.bytes[offset..offset + 8].copy_from_slice(&value.to_ne_bytes());
    }
}

fn write_int<P: Platform>(process: &Process<P>, address: u64, value: i32) -> Result<u64, Errno> {
    process
        .write_user_memory(address, &value.to_ne_bytes())
        .map(|()| 0)
        .map_err(|_| Errno::EFAULT)
}

/// An `ioctl` on the render node by `process`, whose context is `context`. `ENOTTY` for a
/// request that is not DRM's, `EINVAL` for one it does not answer.
pub fn ioctl<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    context: &Context,
    request: u64,
    address: u64,
) -> Result<u64, Errno> {
    let request = request as u32;
    if (request >> 8) & 0xff != DRM_TYPE {
        return Err(Errno::ENOTTY);
    }
    let size = ((request >> SIZE_SHIFT) & SIZE_MASK) as usize;
    let mut argument = Argument::read(process, address, size)?;
    match request & 0xff {
        number::VERSION => version(process, &mut argument, address),
        number::GEM_CLOSE => match context.close_buffer::<P>(argument.u32_at(0)) {
            true => Ok(0),
            false => Err(Errno::EINVAL),
        },
        number::GET_CAP => {
            let value = match argument.u64_at(0) {
                capability::PRIME => PRIME_IMPORT | PRIME_EXPORT,
                capability::DUMB_BUFFER => 0,
                capability::TIMESTAMP_MONOTONIC => 1,
                capability::SYNCOBJ => 1,
                capability::SYNCOBJ_TIMELINE => 0,
                _ => return Err(Errno::EINVAL),
            };
            argument.set_u64(8, value);
            argument.write_back(process, address)
        }
        number::MAP => {
            let handle = argument.u32_at(8);
            context.buffer(handle).ok_or(Errno::ENOENT)?;
            argument.set_u64(0, u64::from(handle) << MAP_OFFSET_SHIFT);
            argument.write_back(process, address)
        }
        number::EXECBUFFER => execbuffer(process, thread, context, &mut argument, address),
        number::PRIME_HANDLE_TO_FD => {
            let buffer = context.buffer(argument.u32_at(0)).ok_or(Errno::ENOENT)?;
            let flags = argument.u32_at(4);
            if flags & !(PRIME_CLOEXEC | PRIME_RDWR) != 0 {
                return Err(Errno::EINVAL);
            }
            let file = Arc::new(OpenFile::new(FileKind::GpuBuffer(buffer), open_flag::RDWR));
            let limit = open_file_limit(process);
            let fd = thread
                .files()
                .lock()
                .open(file, flags & PRIME_CLOEXEC != 0, limit)?;
            argument.set_u32(8, fd);
            argument.write_back(process, address)
        }
        number::PRIME_FD_TO_HANDLE => {
            let file = thread.files().file(u64::from(argument.u32_at(8)))?;
            let FileKind::GpuBuffer(buffer) = file.kind() else {
                return Err(Errno::EINVAL);
            };
            let handle = context.import::<P>(Arc::clone(buffer)).map_err(errno)?;
            argument.set_u32(0, handle);
            argument.write_back(process, address)
        }
        number::GETPARAM => {
            let value = parameter_value::<P>(argument.u64_at(0))?;
            write_int(process, argument.u64_at(8), value)
        }
        number::RESOURCE_CREATE => resource_create(process, context, &mut argument, address),
        number::RESOURCE_INFO => {
            let buffer = context.buffer(argument.u32_at(0)).ok_or(Errno::ENOENT)?;
            argument.set_u32(4, buffer.resource());
            argument.set_u32(8, u32::try_from(buffer.size()).unwrap_or(u32::MAX));
            argument.set_u32(12, 0);
            argument.write_back(process, address)
        }
        number::TRANSFER_FROM_HOST | number::TRANSFER_TO_HOST => {
            let to_host = request & 0xff == number::TRANSFER_TO_HOST;
            let transfer = Transfer {
                resource: 0,
                area: TexelBox {
                    x: argument.u32_at(4),
                    y: argument.u32_at(8),
                    z: argument.u32_at(12),
                    width: argument.u32_at(16),
                    height: argument.u32_at(20),
                    depth: argument.u32_at(24),
                },
                level: argument.u32_at(28),
                offset: u64::from(argument.u32_at(32)),
                stride: argument.u32_at(36),
                layer_stride: argument.u32_at(40),
            };
            context
                .transfer::<P>(argument.u32_at(0), to_host, transfer)
                .map(|()| 0)
                .map_err(|error| match error {
                    RenderError::Invalid => Errno::ENOENT,
                    error => errno(error),
                })
        }
        number::WAIT => {
            let buffer = context.buffer(argument.u32_at(0)).ok_or(Errno::ENOENT)?;
            let flags = argument.u32_at(4);
            if flags & !WAIT_NOWAIT != 0 {
                return Err(Errno::EINVAL);
            }
            buffer
                .wait::<P>(flags & WAIT_NOWAIT != 0)
                .map(|()| 0)
                .map_err(errno)
        }
        number::GET_CAPS => {
            let (id, version) = (argument.u32_at(0), argument.u32_at(4));
            let (target, room) = (argument.u64_at(8), argument.u32_at(16) as usize);
            let mut capset = alloc::vec![0u8; CAPSET_ROOM];
            let length = crate::rendering::capset::<P>(id, version, &mut capset)
                .map_err(|_| Errno::EINVAL)?;
            let copied = length.min(room);
            process
                .write_user_memory(target, &capset[..copied])
                .map(|()| 0)
                .map_err(|_| Errno::EFAULT)
        }
        number::RESOURCE_CREATE_BLOB => create_blob(process, context, &mut argument, address),
        number::CONTEXT_INIT => context_init(process, context, &argument),
        number::SYNCOBJ_CREATE => {
            let signalled = argument.u32_at(4) & syncobj_flag::CREATE_SIGNALED != 0;
            let handle = context.create_syncobj(signalled).map_err(errno)?;
            argument.set_u32(0, handle);
            argument.write_back(process, address)
        }
        number::SYNCOBJ_DESTROY => match context.destroy_syncobj(argument.u32_at(0)) {
            true => Ok(0),
            false => Err(Errno::EINVAL),
        },
        number::SYNCOBJ_RESET | number::SYNCOBJ_SIGNAL => {
            let fence = match request & 0xff {
                number::SYNCOBJ_RESET => crate::rendering::UNSIGNALLED,
                _ => 0,
            };
            for handle in syncobj_handles(process, argument.u64_at(0), argument.u32_at(8))? {
                if !context.set_syncobj(handle, fence) {
                    return Err(Errno::EINVAL);
                }
            }
            Ok(0)
        }
        number::SYNCOBJ_WAIT => syncobj_wait::<P>(process, context, &mut argument, address),
        number::SYNCOBJ_HANDLE_TO_FD => {
            if argument.u32_at(4) & syncobj_flag::SYNC_FILE == 0 {
                return Err(Errno::EINVAL);
            }
            let fence = context.syncobj(argument.u32_at(0)).ok_or(Errno::EINVAL)?;
            let fence = match fence {
                crate::rendering::UNSIGNALLED => return Err(Errno::EINVAL),
                fence => fence,
            };
            let descriptor = fence_file(process, thread, Fence::new(fence))?;
            argument.set_u32(8, descriptor);
            argument.write_back(process, address)
        }
        number::SYNCOBJ_FD_TO_HANDLE => {
            if argument.u32_at(4) & syncobj_flag::SYNC_FILE == 0 {
                return Err(Errno::EINVAL);
            }
            let descriptor = argument.u32_at(8) as i32;
            let file = thread
                .files()
                .file(u64::try_from(descriptor).map_err(|_| Errno::EBADF)?)?;
            let FileKind::GpuFence(fence) = file.kind() else {
                return Err(Errno::EINVAL);
            };
            match context.set_syncobj(argument.u32_at(0), fence.sequence()) {
                true => Ok(0),
                false => Err(Errno::EINVAL),
            }
        }
        _ => Err(Errno::EINVAL),
    }
}

fn copy_text<P: Platform>(
    process: &Process<P>,
    text: &[u8],
    room: u64,
    target: u64,
) -> Result<(), Errno> {
    let copied = text.len().min(usize::try_from(room).unwrap_or(usize::MAX));
    if copied == 0 || target == 0 {
        return Ok(());
    }
    process
        .write_user_memory(target, &text[..copied])
        .map_err(|_| Errno::EFAULT)
}

fn version<P: Platform>(
    process: &Process<P>,
    argument: &mut Argument,
    address: u64,
) -> Result<u64, Errno> {
    let (major, minor, patch) = DRIVER_VERSION;
    argument.set_u32(0, major as u32);
    argument.set_u32(4, minor as u32);
    argument.set_u32(8, patch as u32);
    let texts = [DRIVER_NAME, DRIVER_DATE, DRIVER_DESCRIPTION];
    let wide = argument.size >= 64;
    for (index, text) in texts.into_iter().enumerate() {
        let (room, target) = match wide {
            true => (
                argument.u64_at(16 + 16 * index),
                argument.u64_at(24 + 16 * index),
            ),
            false => (
                u64::from(argument.u32_at(12 + 8 * index)),
                u64::from(argument.u32_at(16 + 8 * index)),
            ),
        };
        copy_text(process, text, room, target)?;
        match wide {
            true => argument.set_u64(16 + 16 * index, text.len() as u64),
            false => argument.set_u32(12 + 8 * index, text.len() as u32),
        }
    }
    argument.write_back(process, address)
}

fn parameter_value<P: Platform>(parameter: u64) -> Result<i32, Errno> {
    Ok(match parameter {
        parameter::THREE_D_FEATURES | parameter::CAPSET_QUERY_FIX => 1,
        parameter::EXPLICIT_DEBUG_NAME => 1,
        parameter::RESOURCE_BLOB => i32::from(crate::rendering::takes_blobs::<P>()),
        parameter::HOST_VISIBLE => i32::from(crate::rendering::has_host_window()),
        parameter::CROSS_DEVICE => 0,
        parameter::CONTEXT_INIT => i32::from(crate::rendering::takes_context_init::<P>()),
        parameter::SUPPORTED_CAPSET_IDS => crate::rendering::capsets::<P>()
            .map_err(errno)?
            .iter()
            .filter(|info| info.id < 32)
            .fold(0u32, |mask, info| mask | 1 << info.id)
            as i32,
        _ => return Err(Errno::EINVAL),
    })
}

fn resource_create<P: Platform>(
    process: &Process<P>,
    context: &Context,
    argument: &mut Argument,
    address: u64,
) -> Result<u64, Errno> {
    let shape = ResourceShape {
        target: argument.u32_at(0),
        format: argument.u32_at(4),
        bind: argument.u32_at(8),
        width: argument.u32_at(12),
        height: argument.u32_at(16),
        depth: argument.u32_at(20),
        array_size: argument.u32_at(24),
        last_level: argument.u32_at(28),
        samples: argument.u32_at(32),
        flags: argument.u32_at(36),
    };
    let size = u64::from(argument.u32_at(48));
    let (handle, buffer) = context.create_buffer::<P>(shape, size).map_err(errno)?;
    argument.set_u32(40, handle);
    argument.set_u32(44, buffer.resource());
    argument.write_back(process, address).inspect_err(|_| {
        context.close_buffer::<P>(handle);
    })
}

const BLOB_MEM_GUEST: u32 = 1;
const BLOB_MEM_HOST3D: u32 = 2;
const BLOB_FLAGS_KNOWN: u32 = 0x7;

fn create_blob<P: Platform>(
    process: &Process<P>,
    context: &Context,
    argument: &mut Argument,
    address: u64,
) -> Result<u64, Errno> {
    let (memory, flags, size) = (argument.u32_at(0), argument.u32_at(4), argument.u64_at(16));
    if flags & !BLOB_FLAGS_KNOWN != 0 || size == 0 || !crate::rendering::takes_blobs::<P>() {
        return Err(Errno::EINVAL);
    }
    let made = match memory {
        BLOB_MEM_GUEST => context.create_guest_blob::<P>(size, flags),
        BLOB_MEM_HOST3D => {
            let length = argument.u32_at(28) as usize;
            if length > COMMAND_STREAM_BYTES {
                return Err(Errno::EINVAL);
            }
            let mut stream = Vec::new();
            if stream.try_reserve_exact(length).is_err() {
                return Err(Errno::ENOMEM);
            }
            stream.resize(length, 0);
            process
                .read_user_memory(argument.u64_at(32), &mut stream)
                .map_err(|_| Errno::EFAULT)?;
            context.create_host_blob::<P>(argument.u64_at(40), size, flags, &stream)
        }
        _ => return Err(Errno::EINVAL),
    };
    let (handle, buffer) = made.map_err(errno)?;
    argument.set_u32(8, handle);
    argument.set_u32(12, buffer.resource());
    argument.write_back(process, address).inspect_err(|_| {
        context.close_buffer::<P>(handle);
    })
}

fn execbuffer<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    context: &Context,
    argument: &mut Argument,
    address: u64,
) -> Result<u64, Errno> {
    let flags = argument.u32_at(0);
    let known =
        execbuffer_flag::FENCE_FD_IN | execbuffer_flag::FENCE_FD_OUT | execbuffer_flag::RING_IDX;
    if flags & !known != 0 {
        return Err(Errno::EINVAL);
    }
    let length = argument.u32_at(4) as usize;
    if length > COMMAND_STREAM_BYTES {
        return Err(Errno::EINVAL);
    }
    let handles = argument.u32_at(24) as usize;
    let handle_list = argument.u64_at(16);
    let mut buffers = Vec::new();
    buffers
        .try_reserve_exact(handles)
        .map_err(|_| Errno::ENOMEM)?;
    for index in 0..handles {
        let mut word = [0u8; 4];
        process
            .read_user_memory(handle_list + 4 * index as u64, &mut word)
            .map_err(|_| Errno::EFAULT)?;
        let buffer = context
            .buffer(u32::from_ne_bytes(word))
            .ok_or(Errno::ENOENT)?;
        buffers.push(buffer);
    }
    if flags & execbuffer_flag::FENCE_FD_IN != 0 {
        let descriptor = argument.u32_at(28) as i32;
        let waited_for = thread
            .files()
            .file(u64::try_from(descriptor).map_err(|_| Errno::EBADF)?)?;
        let FileKind::GpuFence(_) = waited_for.kind() else {
            return Err(Errno::EINVAL);
        };
    }
    let mut stream = Vec::new();
    if stream.try_reserve_exact(length).is_err() {
        return Err(Errno::ENOMEM);
    }
    stream.resize(length, 0);
    process
        .read_user_memory(argument.u64_at(8), &mut stream)
        .map_err(|_| Errno::EFAULT)?;
    let fence = match length {
        0 => buffers
            .iter()
            .map(|buffer| buffer.last_fence())
            .max()
            .unwrap_or(0),
        _ => {
            let ring = match flags & execbuffer_flag::RING_IDX {
                0 => None,
                _ => {
                    let ring = argument.u32_at(32);
                    if ring >= context.rings() {
                        return Err(Errno::EINVAL);
                    }
                    Some(ring as u8)
                }
            };
            for (handle, _) in execbuffer_syncobjs(process, &argument, IN_SYNCOBJS)? {
                let fence = context.syncobj(handle).ok_or(Errno::EINVAL)?;
                if fence != crate::rendering::UNSIGNALLED {
                    crate::rendering::wait_fence::<P>(fence, false).map_err(errno)?;
                }
            }
            context.submit::<P>(&stream, &buffers, ring).map_err(errno)?
        }
    };
    for (handle, _) in execbuffer_syncobjs(process, &argument, OUT_SYNCOBJS)? {
        if !context.set_syncobj(handle, fence) {
            return Err(Errno::EINVAL);
        }
    }
    if flags & execbuffer_flag::FENCE_FD_OUT == 0 {
        return Ok(0);
    }
    let descriptor = fence_file(process, thread, Fence::new(fence))?;
    argument.set_u32(28, descriptor);
    argument.write_back(process, address)
}

const IN_SYNCOBJS: (usize, usize) = (40, 48);
const OUT_SYNCOBJS: (usize, usize) = (44, 56);
const EXECBUFFER_WITH_SYNCOBJS: usize = 64;

fn execbuffer_syncobjs<P: Platform>(
    process: &Process<P>,
    argument: &Argument,
    (count_at, list_at): (usize, usize),
) -> Result<Vec<(u32, u32)>, Errno> {
    if argument.size < EXECBUFFER_WITH_SYNCOBJS {
        return Ok(Vec::new());
    }
    let count = argument.u32_at(count_at);
    if count == 0 {
        return Ok(Vec::new());
    }
    if count > MOST_SYNCOBJS_PER_CALL {
        return Err(Errno::EINVAL);
    }
    let stride = match argument.u32_at(36) {
        0 => SYNCOBJ_ENTRY_BYTES,
        stride if stride >= 8 => stride,
        _ => return Err(Errno::EINVAL),
    };
    let list = argument.u64_at(list_at);
    let mut found = Vec::new();
    found
        .try_reserve_exact(count as usize)
        .map_err(|_| Errno::ENOMEM)?;
    for index in 0..u64::from(count) {
        let mut entry = [0u8; 8];
        process
            .read_user_memory(list + index * u64::from(stride), &mut entry)
            .map_err(|_| Errno::EFAULT)?;
        let handle = u32::from_ne_bytes(entry[..4].try_into().unwrap_or_default());
        let flags = u32::from_ne_bytes(entry[4..].try_into().unwrap_or_default());
        found.push((handle, flags));
    }
    Ok(found)
}

fn syncobj_handles<P: Platform>(
    process: &Process<P>,
    list: u64,
    count: u32,
) -> Result<Vec<u32>, Errno> {
    if count > MOST_SYNCOBJS_PER_CALL {
        return Err(Errno::EINVAL);
    }
    let mut handles = Vec::new();
    handles
        .try_reserve_exact(count as usize)
        .map_err(|_| Errno::ENOMEM)?;
    for index in 0..u64::from(count) {
        let mut word = [0u8; 4];
        process
            .read_user_memory(list + 4 * index, &mut word)
            .map_err(|_| Errno::EFAULT)?;
        handles.push(u32::from_ne_bytes(word));
    }
    Ok(handles)
}

fn syncobj_wait<P: Platform>(
    process: &Process<P>,
    context: &Context,
    argument: &mut Argument,
    address: u64,
) -> Result<u64, Errno> {
    let flags = argument.u32_at(20);
    let handles = syncobj_handles(process, argument.u64_at(0), argument.u32_at(16))?;
    let nowait = argument.u64_at(8) == 0;
    let all = flags & syncobj_flag::WAIT_ALL != 0;
    let mut fences = Vec::new();
    fences
        .try_reserve_exact(handles.len())
        .map_err(|_| Errno::ENOMEM)?;
    for &handle in &handles {
        let fence = context.syncobj(handle).ok_or(Errno::EINVAL)?;
        if fence == crate::rendering::UNSIGNALLED {
            return match flags & syncobj_flag::WAIT_FOR_SUBMIT {
                0 => Err(Errno::EINVAL),
                _ => Err(Errno::ETIME),
            };
        }
        fences.push(fence);
    }
    let passed = |fence: u64| crate::rendering::wait_fence::<P>(fence, true).is_ok();
    let first = match all {
        true => {
            for &fence in &fences {
                if !passed(fence) {
                    if nowait {
                        return Err(Errno::ETIME);
                    }
                    crate::rendering::wait_fence::<P>(fence, false).map_err(|_| Errno::ETIME)?;
                }
            }
            0
        }
        false => match fences.iter().position(|&fence| passed(fence)) {
            Some(index) => index,
            None if nowait => return Err(Errno::ETIME),
            None => {
                let oldest = fences
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, fence)| **fence)
                    .map_or(0, |(index, _)| index);
                crate::rendering::wait_fence::<P>(fences[oldest], false)
                    .map_err(|_| Errno::ETIME)?;
                oldest
            }
        },
    };
    argument.set_u32(24, first as u32);
    argument.write_back(process, address)
}

/// Opens `fence` as a sync file in `thread`'s descriptor table (`O_CLOEXEC`): its descriptor.
pub fn fence_file<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    fence: Fence,
) -> Result<u32, Errno> {
    let file = Arc::new(OpenFile::new(FileKind::GpuFence(fence), open_flag::RDWR));
    let limit = open_file_limit(process);
    thread.files().lock().open(file, true, limit)
}

fn context_init<P: Platform>(
    process: &Process<P>,
    context: &Context,
    argument: &Argument,
) -> Result<u64, Errno> {
    let count = argument.u32_at(0);
    if count > MOST_CONTEXT_PARAMETERS {
        return Err(Errno::EINVAL);
    }
    let list = argument.u64_at(8);
    let mut capset = 0;
    let mut name = [0u8; LONGEST_DEBUG_NAME];
    let mut name_length = 0;
    for index in 0..u64::from(count) {
        let mut pair = [0u8; 16];
        process
            .read_user_memory(list + 16 * index, &mut pair)
            .map_err(|_| Errno::EFAULT)?;
        let parameter = u64::from_ne_bytes(pair[..8].try_into().unwrap_or_default());
        let value = u64::from_ne_bytes(pair[8..].try_into().unwrap_or_default());
        match parameter {
            context_parameter::CAPSET_ID => {
                capset = u32::try_from(value).map_err(|_| Errno::EINVAL)?;
            }
            context_parameter::NUM_RINGS => {
                let rings = u32::try_from(value).map_err(|_| Errno::EINVAL)?;
                context.set_rings(rings).map_err(errno)?;
            }
            context_parameter::POLL_RINGS_MASK => {}
            context_parameter::DEBUG_NAME => {
                name_length = read_name(process, value, &mut name)?;
            }
            _ => return Err(Errno::EINVAL),
        }
    }
    context
        .initialize::<P>(capset, &name[..name_length])
        .map(|()| 0)
        .map_err(errno)
}

fn read_name<P: Platform>(
    process: &Process<P>,
    address: u64,
    name: &mut [u8; LONGEST_DEBUG_NAME],
) -> Result<usize, Errno> {
    for (index, place) in name.iter_mut().enumerate() {
        let mut byte = [0u8];
        process
            .read_user_memory(address + index as u64, &mut byte)
            .map_err(|_| Errno::EFAULT)?;
        if byte[0] == 0 {
            return Ok(index);
        }
        *place = byte[0];
    }
    Ok(LONGEST_DEBUG_NAME)
}

/// Maps a GPU buffer handed out as a file (D441) into `process`, from its start.
pub fn map_buffer<P: Platform>(
    process: &Process<P>,
    buffer: &Arc<crate::rendering::Buffer>,
    placement: Placement,
    length: u64,
    attributes: Attributes,
    offset: u64,
) -> Result<u64, Errno> {
    if offset != 0 || length > buffer.size() {
        return Err(Errno::EINVAL);
    }
    process
        .map_lent_frames(
            placement,
            length,
            attributes,
            Arc::clone(buffer) as Arc<dyn crate::process::LentFrames>,
        )
        .map_err(|error| match error {
            RegionError::Overlaps => Errno::EEXIST,
            RegionError::Invalid => Errno::EINVAL,
            RegionError::NotCovered
            | RegionError::NoRoom
            | RegionError::OutOfMemory
            | RegionError::NotWritable => Errno::ENOMEM,
        })
}

/// Maps the backing of the resource whose `VIRTGPU_MAP` offset is `offset` into `process`.
pub fn map<P: Platform>(
    process: &Process<P>,
    context: &Context,
    placement: Placement,
    length: u64,
    attributes: Attributes,
    offset: u64,
) -> Result<u64, Errno> {
    if offset & ((1 << MAP_OFFSET_SHIFT) - 1) != 0 {
        return Err(Errno::EINVAL);
    }
    let handle = u32::try_from(offset >> MAP_OFFSET_SHIFT).map_err(|_| Errno::EINVAL)?;
    let buffer = context.buffer(handle).ok_or(Errno::EINVAL)?;
    if length > buffer.size() {
        return Err(Errno::EINVAL);
    }
    process
        .map_lent_frames(placement, length, attributes, buffer)
        .map_err(|error| match error {
            RegionError::Overlaps => Errno::EEXIST,
            RegionError::Invalid => Errno::EINVAL,
            RegionError::NotCovered
            | RegionError::NoRoom
            | RegionError::OutOfMemory
            | RegionError::NotWritable => Errno::ENOMEM,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtio_gpu_requests_are_told_apart_by_their_number() {
        let get_param = 0xc010_6443u32;
        assert_eq!((get_param >> 8) & 0xff, DRM_TYPE);
        assert_eq!(get_param & 0xff, number::GETPARAM);
        assert_eq!((get_param >> SIZE_SHIFT) & SIZE_MASK, 16);
        let execbuffer = 0xc040_6442u32;
        assert_eq!((execbuffer >> SIZE_SHIFT) & SIZE_MASK, 64);
    }

    #[test]
    fn map_offsets_name_the_handle() {
        assert_eq!(3u64 << MAP_OFFSET_SHIFT >> MAP_OFFSET_SHIFT, 3);
        assert_eq!(
            (3u64 << MAP_OFFSET_SHIFT) & ((1 << MAP_OFFSET_SHIFT) - 1),
            0
        );
    }
}
