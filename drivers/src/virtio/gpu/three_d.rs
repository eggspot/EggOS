//! The virtio GPU's 3D commands (virtio 1.2, 5.7.6.10; D415): what a rendering context needs
//! from a device that offers `VIRTIO_GPU_F_VIRGL`, encoded as the control queue carries them.
//! Every command starts with the common header, whose context field names the rendering
//! context the command belongs to. [`encode`] writes one into a request buffer and is plain
//! byte work, so the layouts are tested on the host.

/// `VIRTIO_GPU_F_VIRGL`: the device takes 3D commands.
pub const FEATURE_VIRGL: u64 = 1 << 0;
/// `VIRTIO_GPU_F_CONTEXT_INIT`: a context is created for one capability set.
pub const FEATURE_CONTEXT_INIT: u64 = 1 << 4;
/// `VIRTIO_GPU_F_RESOURCE_BLOB`: resources made of plain memory, not textures (D439).
pub const FEATURE_RESOURCE_BLOB: u64 = 1 << 3;
/// `VIRTIO_GPU_BLOB_MEM_GUEST`: a blob whose only copy is the guest memory backing it.
pub const BLOB_MEM_GUEST: u32 = 1;
/// `VIRTIO_GPU_BLOB_MEM_HOST3D`: a blob the host made for a context, reached through the
/// device's host-visible shared memory once mapped (D440).
pub const BLOB_MEM_HOST3D: u32 = 2;
/// `VIRTIO_GPU_SHM_ID_HOST_VISIBLE`: the shared memory region host blobs are mapped in.
pub const SHM_HOST_VISIBLE: u8 = 1;
/// `VIRTIO_GPU_BLOB_FLAG_USE_MAPPABLE`.
pub const BLOB_FLAG_MAPPABLE: u32 = 1;
/// `VIRTIO_GPU_BLOB_FLAG_USE_SHAREABLE`.
pub const BLOB_FLAG_SHAREABLE: u32 = 2;
/// `VIRTIO_GPU_CAPSET_VIRGL`.
pub const CAPSET_VIRGL: u32 = 1;
/// `VIRTIO_GPU_CAPSET_VIRGL2`.
pub const CAPSET_VIRGL2: u32 = 2;
/// `VIRTIO_GPU_CAPSET_VENUS`: Vulkan contexts.
pub const CAPSET_VENUS: u32 = 4;
/// The longest debug name a context is created with.
pub const NAME_CAPACITY: usize = 64;
/// The longest request [`encode`] writes.
pub const LONGEST_REQUEST: usize = CREATE_CONTEXT_BYTES;
/// The common header's length, before a command's own fields and an answer's data.
pub const HEADER_BYTES: usize = 24;
/// A [`ThreeDCommand::Submit`] request's length.
pub const SUBMIT_BYTES: usize = 32;

const CREATE_CONTEXT_BYTES: usize = 96;
const RESOURCE_BYTES: usize = 32;
const CREATE_RESOURCE_BYTES: usize = 72;
const CREATE_BLOB_BYTES: usize = 72;
const MAP_BLOB_BYTES: usize = 40;
const TRANSFER_BYTES: usize = 72;
const ATTACH_BACKING_BYTES: usize = 48;
const CAPSET_BYTES: usize = 32;
const CONTEXT_FIELD: usize = 16;
const FLAG_FENCE: u32 = 1;
const CONTEXT_INIT_CAPSET_MASK: u32 = 0xff;

/// The 3D command types.
pub mod command {
    pub const RESOURCE_UNREF: u32 = 0x0102;
    pub const RESOURCE_ATTACH_BACKING: u32 = 0x0106;
    pub const RESOURCE_DETACH_BACKING: u32 = 0x0107;
    pub const GET_CAPSET_INFO: u32 = 0x0108;
    pub const GET_CAPSET: u32 = 0x0109;
    pub const CTX_CREATE: u32 = 0x0200;
    pub const CTX_DESTROY: u32 = 0x0201;
    pub const CTX_ATTACH_RESOURCE: u32 = 0x0202;
    pub const CTX_DETACH_RESOURCE: u32 = 0x0203;
    pub const RESOURCE_CREATE_3D: u32 = 0x0204;
    pub const TRANSFER_TO_HOST_3D: u32 = 0x0205;
    pub const TRANSFER_FROM_HOST_3D: u32 = 0x0206;
    pub const SUBMIT_3D: u32 = 0x0207;
    pub const RESOURCE_CREATE_BLOB: u32 = 0x010c;
    pub const RESOURCE_MAP_BLOB: u32 = 0x0208;
    pub const RESOURCE_UNMAP_BLOB: u32 = 0x0209;
}

/// The answer types a 3D command succeeds with.
pub mod answer {
    pub const OK_NODATA: u32 = 0x1100;
    pub const OK_CAPSET_INFO: u32 = 0x1102;
    pub const OK_CAPSET: u32 = 0x1103;
    pub const OK_MAP_INFO: u32 = 0x1105;
}

/// What a 3D resource is: Gallium's texture target, format and bind flags, its size and its
/// mipmap and sample counts, as a context's command stream names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResourceShape {
    pub target: u32,
    pub format: u32,
    pub bind: u32,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub array_size: u32,
    pub last_level: u32,
    pub samples: u32,
    pub flags: u32,
}

/// A box of texels in a resource: its corner and its size in each dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TexelBox {
    pub x: u32,
    pub y: u32,
    pub z: u32,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
}

/// A copy between a resource's backing in guest memory and the host's copy of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Transfer {
    pub resource: u32,
    pub area: TexelBox,
    pub level: u32,
    /// Where the box starts in the backing, in bytes.
    pub offset: u64,
    /// Bytes from one row to the next in the backing; 0 for the resource's own.
    pub stride: u32,
    /// Bytes from one layer to the next in the backing; 0 for the resource's own.
    pub layer_stride: u32,
}

/// A 3D command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreeDCommand<'a> {
    /// The capability set with this index (below the device's count): its id, newest version
    /// and size.
    CapsetInfo {
        index: u32,
    },
    /// Capability set `id` at `version`, its bytes.
    Capset {
        id: u32,
        version: u32,
    },
    /// A rendering context, for capability set `capset` (0 for the device's default) when the
    /// device takes `VIRTIO_GPU_F_CONTEXT_INIT`, named `name` for the host's debugging.
    CreateContext {
        context: u32,
        capset: u32,
        name: &'a [u8],
    },
    DestroyContext {
        context: u32,
    },
    AttachResource {
        context: u32,
        resource: u32,
    },
    DetachResource {
        context: u32,
        resource: u32,
    },
    CreateResource {
        resource: u32,
        shape: ResourceShape,
    },
    /// A blob of `size` bytes in guest memory at `address` (one entry, its backing), for
    /// `context`, with `flags` ([`BLOB_FLAG_MAPPABLE`], [`BLOB_FLAG_SHAREABLE`]).
    CreateGuestBlob {
        context: u32,
        resource: u32,
        flags: u32,
        address: u64,
        size: u64,
    },
    /// A blob the host made for `context` under `blob_id` (by a command stream), `size` bytes.
    CreateHostBlob {
        context: u32,
        resource: u32,
        flags: u32,
        blob_id: u64,
        size: u64,
    },
    /// Host blob `resource` mapped at `offset` in the host-visible shared memory.
    MapBlob {
        context: u32,
        resource: u32,
        offset: u64,
    },
    /// Host blob `resource` taken out of the shared memory.
    UnmapBlob {
        context: u32,
        resource: u32,
    },
    /// Guest memory at `address` (a device address), `length` bytes, as the resource's backing.
    AttachBacking {
        resource: u32,
        address: u64,
        length: u32,
    },
    DetachBacking {
        resource: u32,
    },
    Unref {
        resource: u32,
    },
    TransferToHost {
        context: u32,
        transfer: Transfer,
    },
    TransferFromHost {
        context: u32,
        transfer: Transfer,
    },
    /// `length` bytes of the context's command stream, which follow the request in a
    /// descriptor of their own.
    Submit {
        context: u32,
        length: u32,
    },
}

/// A capability set the device has ([`ThreeDCommand::CapsetInfo`]'s answer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapsetInfo {
    pub id: u32,
    pub newest_version: u32,
    pub size: u32,
}

fn put32(out: &mut [u8], offset: usize, value: u32) {
    out[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put64(out: &mut [u8], offset: usize, value: u64) {
    out[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn get32(bytes: &[u8], offset: usize) -> Option<u32> {
    let word = bytes.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([word[0], word[1], word[2], word[3]]))
}

fn put_header(out: &mut [u8], kind: u32, fence: u64, context: u32) {
    put32(out, 0, kind);
    put32(out, 4, FLAG_FENCE);
    put64(out, 8, fence);
    put32(out, CONTEXT_FIELD, context);
    put32(out, 20, 0);
}

fn put_transfer(out: &mut [u8], transfer: &Transfer) {
    let area = transfer.area;
    for (index, value) in [area.x, area.y, area.z, area.width, area.height, area.depth]
        .into_iter()
        .enumerate()
    {
        put32(out, 24 + 4 * index, value);
    }
    put64(out, 48, transfer.offset);
    put32(out, 56, transfer.resource);
    put32(out, 60, transfer.level);
    put32(out, 64, transfer.stride);
    put32(out, 68, transfer.layer_stride);
}

/// Writes `command`, fenced with `fence`, into `out` ([`LONGEST_REQUEST`] bytes or more); the
/// request's length, the bytes the device reads before any command stream. Zero when `out`
/// is too short.
pub fn encode(command: &ThreeDCommand<'_>, fence: u64, out: &mut [u8]) -> usize {
    if out.len() < LONGEST_REQUEST {
        return 0;
    }
    out[..LONGEST_REQUEST].fill(0);
    match *command {
        ThreeDCommand::CapsetInfo { index } => {
            put_header(out, command::GET_CAPSET_INFO, fence, 0);
            put32(out, 24, index);
            CAPSET_BYTES
        }
        ThreeDCommand::Capset { id, version } => {
            put_header(out, command::GET_CAPSET, fence, 0);
            put32(out, 24, id);
            put32(out, 28, version);
            CAPSET_BYTES
        }
        ThreeDCommand::CreateContext {
            context,
            capset,
            name,
        } => {
            put_header(out, command::CTX_CREATE, fence, context);
            let name = &name[..name.len().min(NAME_CAPACITY)];
            put32(out, 24, name.len() as u32);
            put32(out, 28, capset & CONTEXT_INIT_CAPSET_MASK);
            out[32..32 + name.len()].copy_from_slice(name);
            CREATE_CONTEXT_BYTES
        }
        ThreeDCommand::DestroyContext { context } => {
            put_header(out, command::CTX_DESTROY, fence, context);
            HEADER_BYTES
        }
        ThreeDCommand::AttachResource { context, resource } => {
            put_header(out, command::CTX_ATTACH_RESOURCE, fence, context);
            put32(out, 24, resource);
            RESOURCE_BYTES
        }
        ThreeDCommand::DetachResource { context, resource } => {
            put_header(out, command::CTX_DETACH_RESOURCE, fence, context);
            put32(out, 24, resource);
            RESOURCE_BYTES
        }
        ThreeDCommand::CreateResource { resource, shape } => {
            put_header(out, command::RESOURCE_CREATE_3D, fence, 0);
            let fields = [
                resource,
                shape.target,
                shape.format,
                shape.bind,
                shape.width,
                shape.height,
                shape.depth,
                shape.array_size,
                shape.last_level,
                shape.samples,
                shape.flags,
            ];
            for (index, value) in fields.into_iter().enumerate() {
                put32(out, 24 + 4 * index, value);
            }
            CREATE_RESOURCE_BYTES
        }
        ThreeDCommand::CreateGuestBlob {
            context,
            resource,
            flags,
            address,
            size,
        } => {
            put_header(out, command::RESOURCE_CREATE_BLOB, fence, context);
            put32(out, 24, resource);
            put32(out, 28, BLOB_MEM_GUEST);
            put32(out, 32, flags);
            put32(out, 36, 1);
            put64(out, 40, 0);
            put64(out, 48, size);
            put64(out, 56, address);
            put32(out, 64, size.min(u64::from(u32::MAX)) as u32);
            CREATE_BLOB_BYTES
        }
        ThreeDCommand::CreateHostBlob {
            context,
            resource,
            flags,
            blob_id,
            size,
        } => {
            put_header(out, command::RESOURCE_CREATE_BLOB, fence, context);
            put32(out, 24, resource);
            put32(out, 28, BLOB_MEM_HOST3D);
            put32(out, 32, flags);
            put32(out, 36, 0);
            put64(out, 40, blob_id);
            put64(out, 48, size);
            CREATE_BLOB_BYTES - 16
        }
        ThreeDCommand::MapBlob {
            context,
            resource,
            offset,
        } => {
            put_header(out, command::RESOURCE_MAP_BLOB, fence, context);
            put32(out, 24, resource);
            put64(out, 32, offset);
            MAP_BLOB_BYTES
        }
        ThreeDCommand::UnmapBlob { context, resource } => {
            put_header(out, command::RESOURCE_UNMAP_BLOB, fence, context);
            put32(out, 24, resource);
            RESOURCE_BYTES
        }
        ThreeDCommand::AttachBacking {
            resource,
            address,
            length,
        } => {
            put_header(out, command::RESOURCE_ATTACH_BACKING, fence, 0);
            put32(out, 24, resource);
            put32(out, 28, 1);
            put64(out, 32, address);
            put32(out, 40, length);
            ATTACH_BACKING_BYTES
        }
        ThreeDCommand::DetachBacking { resource } => {
            put_header(out, command::RESOURCE_DETACH_BACKING, fence, 0);
            put32(out, 24, resource);
            RESOURCE_BYTES
        }
        ThreeDCommand::Unref { resource } => {
            put_header(out, command::RESOURCE_UNREF, fence, 0);
            put32(out, 24, resource);
            RESOURCE_BYTES
        }
        ThreeDCommand::TransferToHost { context, transfer } => {
            put_header(out, command::TRANSFER_TO_HOST_3D, fence, context);
            put_transfer(out, &transfer);
            TRANSFER_BYTES
        }
        ThreeDCommand::TransferFromHost { context, transfer } => {
            put_header(out, command::TRANSFER_FROM_HOST_3D, fence, context);
            put_transfer(out, &transfer);
            TRANSFER_BYTES
        }
        ThreeDCommand::Submit { context, length } => {
            put_header(out, command::SUBMIT_3D, fence, context);
            put32(out, 24, length);
            SUBMIT_BYTES
        }
    }
}

/// The answer type `command` succeeds with.
pub fn expected_answer(command: &ThreeDCommand<'_>) -> u32 {
    match command {
        ThreeDCommand::CapsetInfo { .. } => answer::OK_CAPSET_INFO,
        ThreeDCommand::Capset { .. } => answer::OK_CAPSET,
        ThreeDCommand::MapBlob { .. } => answer::OK_MAP_INFO,
        _ => answer::OK_NODATA,
    }
}

/// The bytes of answer the device writes for `command`, header included, given room for
/// `capset_room` bytes of capability set.
pub fn answer_bytes(command: &ThreeDCommand<'_>, capset_room: usize) -> usize {
    match command {
        ThreeDCommand::CapsetInfo { .. } => HEADER_BYTES + 16,
        ThreeDCommand::Capset { .. } => HEADER_BYTES + capset_room,
        ThreeDCommand::MapBlob { .. } => HEADER_BYTES + 8,
        _ => HEADER_BYTES,
    }
}

/// The capability set an `OK_CAPSET_INFO` answer's data (past the header) describes.
pub fn capset_info(data: &[u8]) -> Option<CapsetInfo> {
    Some(CapsetInfo {
        id: get32(data, 0)?,
        newest_version: get32(data, 4)?,
        size: get32(data, 8)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(command: ThreeDCommand<'_>) -> ([u8; LONGEST_REQUEST], usize) {
        let mut out = [0xaa; LONGEST_REQUEST];
        let length = encode(&command, 7, &mut out);
        (out, length)
    }

    #[test]
    fn every_request_is_fenced_and_names_its_context() {
        let (out, length) = encoded(ThreeDCommand::Submit {
            context: 3,
            length: 128,
        });
        assert_eq!(length, 32);
        assert_eq!(get32(&out, 0), Some(command::SUBMIT_3D));
        assert_eq!(get32(&out, 4), Some(FLAG_FENCE));
        assert_eq!(&out[8..16], &7u64.to_le_bytes());
        assert_eq!(get32(&out, 16), Some(3));
        assert_eq!(get32(&out, 24), Some(128));
    }

    #[test]
    fn a_context_carries_its_capset_and_its_name() {
        let (out, length) = encoded(ThreeDCommand::CreateContext {
            context: 2,
            capset: CAPSET_VENUS,
            name: b"vkcube",
        });
        assert_eq!(length, 96);
        assert_eq!(get32(&out, 24), Some(6));
        assert_eq!(get32(&out, 28), Some(CAPSET_VENUS));
        assert_eq!(&out[32..38], b"vkcube");
        assert!(out[38..96].iter().all(|&byte| byte == 0));
    }

    #[test]
    fn a_long_context_name_is_cut_to_capacity() {
        let name = [b'n'; 100];
        let (out, _) = encoded(ThreeDCommand::CreateContext {
            context: 1,
            capset: 0,
            name: &name,
        });
        assert_eq!(get32(&out, 24), Some(NAME_CAPACITY as u32));
    }

    #[test]
    fn a_resource_keeps_its_fields_in_order() {
        let shape = ResourceShape {
            target: 2,
            format: 1,
            bind: 0x8,
            width: 640,
            height: 480,
            depth: 1,
            array_size: 1,
            last_level: 0,
            samples: 0,
            flags: 0,
        };
        let (out, length) = encoded(ThreeDCommand::CreateResource {
            resource: 0x100,
            shape,
        });
        assert_eq!(length, 72);
        assert_eq!(get32(&out, 16), Some(0));
        let fields: Vec<u32> = (0..11)
            .filter_map(|index| get32(&out, 24 + 4 * index))
            .collect();
        assert_eq!(fields, [0x100, 2, 1, 0x8, 640, 480, 1, 1, 0, 0, 0]);
    }

    #[test]
    fn a_transfer_puts_its_box_before_its_offset() {
        let transfer = Transfer {
            resource: 9,
            area: TexelBox {
                x: 1,
                y: 2,
                z: 0,
                width: 3,
                height: 4,
                depth: 1,
            },
            level: 0,
            offset: 0x1_0000_0010,
            stride: 256,
            layer_stride: 0,
        };
        let (out, length) = encoded(ThreeDCommand::TransferFromHost {
            context: 5,
            transfer,
        });
        assert_eq!(length, 72);
        assert_eq!(get32(&out, 0), Some(command::TRANSFER_FROM_HOST_3D));
        assert_eq!(get32(&out, 24), Some(1));
        assert_eq!(get32(&out, 44), Some(1));
        assert_eq!(&out[48..56], &0x1_0000_0010u64.to_le_bytes());
        assert_eq!(get32(&out, 56), Some(9));
        assert_eq!(get32(&out, 64), Some(256));
    }

    #[test]
    fn a_backing_is_one_entry() {
        let (out, length) = encoded(ThreeDCommand::AttachBacking {
            resource: 4,
            address: 0x8000_0000,
            length: 8192,
        });
        assert_eq!(length, 48);
        assert_eq!(get32(&out, 28), Some(1));
        assert_eq!(&out[32..40], &0x8000_0000u64.to_le_bytes());
        assert_eq!(get32(&out, 40), Some(8192));
    }

    #[test]
    fn capset_answers_are_read_back() {
        let mut data = [0u8; 16];
        data[0..4].copy_from_slice(&CAPSET_VIRGL2.to_le_bytes());
        data[4..8].copy_from_slice(&2u32.to_le_bytes());
        data[8..12].copy_from_slice(&1024u32.to_le_bytes());
        assert_eq!(
            capset_info(&data),
            Some(CapsetInfo {
                id: CAPSET_VIRGL2,
                newest_version: 2,
                size: 1024,
            })
        );
        assert_eq!(capset_info(&data[..8]), None);
        assert_eq!(
            expected_answer(&ThreeDCommand::Capset { id: 2, version: 2 }),
            answer::OK_CAPSET
        );
        assert_eq!(
            answer_bytes(&ThreeDCommand::Capset { id: 2, version: 2 }, 1024),
            HEADER_BYTES + 1024
        );
    }

    #[test]
    fn a_guest_blob_names_its_memory_in_one_entry() {
        let (out, length) = encoded(ThreeDCommand::CreateGuestBlob {
            context: 2,
            resource: 0x101,
            flags: BLOB_FLAG_MAPPABLE,
            address: 0x4000_0000,
            size: 8192,
        });
        assert_eq!(length, 72);
        assert_eq!(get32(&out, 0), Some(command::RESOURCE_CREATE_BLOB));
        assert_eq!(get32(&out, 16), Some(2));
        assert_eq!(get32(&out, 24), Some(0x101));
        assert_eq!(get32(&out, 28), Some(BLOB_MEM_GUEST));
        assert_eq!(get32(&out, 36), Some(1));
        assert_eq!(&out[48..56], &8192u64.to_le_bytes());
        assert_eq!(&out[56..64], &0x4000_0000u64.to_le_bytes());
        assert_eq!(get32(&out, 64), Some(8192));
    }

    #[test]
    fn a_short_buffer_gets_nothing() {
        let mut out = [0u8; 16];
        assert_eq!(
            encode(&ThreeDCommand::DestroyContext { context: 1 }, 1, &mut out),
            0
        );
    }
}
