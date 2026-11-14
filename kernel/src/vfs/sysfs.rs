//! `/sys` (D558): the few files programs read to size themselves and that make their
//! allocators fall back from guessing: `devices/system/cpu/{online,possible,present}` (the CPUs as
//! Linux's ranges, `0-3`), `kernel/mm/transparent_hugepage/enabled` (`always madvise [never]`:
//! EggOS gives programs no transparent huge pages) and `defrag`; and, with a GPU that renders,
//! the nodes libdrm and Mesa enumerate it by (D588): `dev/char/226:128` and
//! `class/drm/renderD128` linking to the render node's directory under the GPU's device, with
//! its `dev`, `uevent`, `subsystem` and `device`, the virtio device above it and its PCI
//! function's (or platform device's) identity. Read-only; each file's text is made when it is
//! opened. `devices/system/cpu/cpu<n>/online` takes CPU `n` out of service with `0` and puts it
//! back with `1` (D596).

use super::{Attributes, DirectoryEntry, FileSystem, Name, NodeKind, Usage};
use crate::sync::LeafSpinLock;
use alloc::vec::Vec;

const ROOT: u64 = 1;
const DEVICES: u64 = 2;
const SYSTEM: u64 = 3;
const CPU: u64 = 4;
const ONLINE: u64 = 5;
const POSSIBLE: u64 = 6;
const PRESENT: u64 = 7;
const KERNEL: u64 = 8;
const MM: u64 = 9;
const TRANSPARENT_HUGEPAGE: u64 = 10;
const ENABLED: u64 = 11;
const DEFRAG: u64 = 12;

/// Each node with its name and its parent.
const NODES: [(u64, &[u8], u64); 11] = [
    (DEVICES, b"devices", ROOT),
    (SYSTEM, b"system", DEVICES),
    (CPU, b"cpu", SYSTEM),
    (ONLINE, b"online", CPU),
    (POSSIBLE, b"possible", CPU),
    (PRESENT, b"present", CPU),
    (KERNEL, b"kernel", ROOT),
    (MM, b"mm", KERNEL),
    (TRANSPARENT_HUGEPAGE, b"transparent_hugepage", MM),
    (ENABLED, b"enabled", TRANSPARENT_HUGEPAGE),
    (DEFRAG, b"defrag", TRANSPARENT_HUGEPAGE),
];

const DEV: u64 = 20;
const CHAR: u64 = 21;
const CHAR_RENDER: u64 = 22;
const CLASS: u64 = 23;
const CLASS_DRM: u64 = 24;
const CLASS_RENDER: u64 = 25;
const BUS: u64 = 26;
const BUS_PCI: u64 = 27;
const BUS_VIRTIO: u64 = 28;
const BUS_PLATFORM: u64 = 29;
const GPU_ROOT: u64 = 30;
const GPU_PARENT: u64 = 31;
const PARENT_SUBSYSTEM: u64 = 32;
const PARENT_UEVENT: u64 = 33;
const PARENT_VENDOR: u64 = 34;
const PARENT_DEVICE: u64 = 35;
const PARENT_SUBSYSTEM_VENDOR: u64 = 36;
const PARENT_SUBSYSTEM_DEVICE: u64 = 37;
const PARENT_REVISION: u64 = 38;
const PARENT_CLASS: u64 = 39;
const VIRTIO: u64 = 40;
const VIRTIO_SUBSYSTEM: u64 = 41;
const VIRTIO_UEVENT: u64 = 42;
const DRM: u64 = 43;
const RENDER: u64 = 44;
const RENDER_DEV: u64 = 45;
const RENDER_UEVENT: u64 = 46;
const RENDER_SUBSYSTEM: u64 = 47;
const RENDER_DEVICE: u64 = 48;

/// The render node's device number, `226:128`.
pub const RENDER_NODE: (u32, u32) = (226, 128);

/// The GPU's place a render node is enumerated by (D588).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuLocation {
    /// A PCI function: its address and its configuration space's identity.
    Pci {
        bus: u8,
        device: u8,
        function: u8,
        vendor: u16,
        device_id: u16,
        subsystem_vendor: u16,
        subsystem_device: u16,
        revision: u8,
        class: u32,
    },
    /// A virtio-mmio slot.
    Platform { slot: u8 },
}

static GPU: LeafSpinLock<Option<GpuLocation>> = LeafSpinLock::new_leaf(None);

/// Records where the GPU behind the render node is, so `/sys` lists it (D588).
pub fn register_gpu(location: GpuLocation) {
    *GPU.lock() = Some(location);
}

fn gpu() -> Option<GpuLocation> {
    (*GPU.lock()).filter(|_| crate::rendering::is_present())
}

/// The DRM nodes: each with its name (`None` for one named from the GPU's place), its parent
/// and its kind.
const DRM_NODES: [(u64, Option<&[u8]>, u64, NodeKind); 29] = [
    (DEV, Some(b"dev"), ROOT, NodeKind::Directory),
    (CHAR, Some(b"char"), DEV, NodeKind::Directory),
    (CHAR_RENDER, Some(b"226:128"), CHAR, NodeKind::SymbolicLink),
    (CLASS, Some(b"class"), ROOT, NodeKind::Directory),
    (CLASS_DRM, Some(b"drm"), CLASS, NodeKind::Directory),
    (CLASS_RENDER, Some(b"renderD128"), CLASS_DRM, NodeKind::SymbolicLink),
    (BUS, Some(b"bus"), ROOT, NodeKind::Directory),
    (BUS_PCI, Some(b"pci"), BUS, NodeKind::Directory),
    (BUS_VIRTIO, Some(b"virtio"), BUS, NodeKind::Directory),
    (BUS_PLATFORM, Some(b"platform"), BUS, NodeKind::Directory),
    (GPU_ROOT, None, DEVICES, NodeKind::Directory),
    (GPU_PARENT, None, GPU_ROOT, NodeKind::Directory),
    (PARENT_SUBSYSTEM, Some(b"subsystem"), GPU_PARENT, NodeKind::SymbolicLink),
    (PARENT_UEVENT, Some(b"uevent"), GPU_PARENT, NodeKind::Regular),
    (PARENT_VENDOR, Some(b"vendor"), GPU_PARENT, NodeKind::Regular),
    (PARENT_DEVICE, Some(b"device"), GPU_PARENT, NodeKind::Regular),
    (PARENT_SUBSYSTEM_VENDOR, Some(b"subsystem_vendor"), GPU_PARENT, NodeKind::Regular),
    (PARENT_SUBSYSTEM_DEVICE, Some(b"subsystem_device"), GPU_PARENT, NodeKind::Regular),
    (PARENT_REVISION, Some(b"revision"), GPU_PARENT, NodeKind::Regular),
    (PARENT_CLASS, Some(b"class"), GPU_PARENT, NodeKind::Regular),
    (VIRTIO, Some(b"virtio0"), GPU_PARENT, NodeKind::Directory),
    (VIRTIO_SUBSYSTEM, Some(b"subsystem"), VIRTIO, NodeKind::SymbolicLink),
    (VIRTIO_UEVENT, Some(b"uevent"), VIRTIO, NodeKind::Regular),
    (DRM, Some(b"drm"), VIRTIO, NodeKind::Directory),
    (RENDER, Some(b"renderD128"), DRM, NodeKind::Directory),
    (RENDER_DEV, Some(b"dev"), RENDER, NodeKind::Regular),
    (RENDER_UEVENT, Some(b"uevent"), RENDER, NodeKind::Regular),
    (RENDER_SUBSYSTEM, Some(b"subsystem"), RENDER, NodeKind::SymbolicLink),
    (RENDER_DEVICE, Some(b"device"), RENDER, NodeKind::SymbolicLink),
];

fn is_pci_only(node: u64) -> bool {
    matches!(
        node,
        PARENT_VENDOR
            | PARENT_DEVICE
            | PARENT_SUBSYSTEM_VENDOR
            | PARENT_SUBSYSTEM_DEVICE
            | PARENT_REVISION
            | PARENT_CLASS
    )
}

fn drm_node(node: u64) -> Option<(Vec<u8>, u64, NodeKind)> {
    let gpu = gpu()?;
    if is_pci_only(node) && !matches!(gpu, GpuLocation::Pci { .. }) {
        return None;
    }
    let &(_, name, parent, kind) = DRM_NODES.iter().find(|&&(drm, ..)| drm == node)?;
    let name = match (name, node, gpu) {
        (Some(name), ..) => name.to_vec(),
        (None, GPU_ROOT, GpuLocation::Pci { bus, .. }) => alloc::format!("pci0000:{bus:02x}").into_bytes(),
        (None, GPU_ROOT, GpuLocation::Platform { .. }) => b"platform".to_vec(),
        (None, _, GpuLocation::Pci { bus, device, function, .. }) => {
            alloc::format!("0000:{bus:02x}:{device:02x}.{function:x}").into_bytes()
        }
        (None, _, GpuLocation::Platform { slot }) => alloc::format!("virtio-mmio.{slot}").into_bytes(),
    };
    Some((name, parent, kind))
}

fn gpu_path() -> Option<Vec<u8>> {
    let (root, ..) = drm_node(GPU_ROOT)?;
    let (parent, ..) = drm_node(GPU_PARENT)?;
    let mut path = b"devices/".to_vec();
    path.extend_from_slice(&root);
    path.push(b'/');
    path.extend_from_slice(&parent);
    path.extend_from_slice(b"/virtio0/drm/renderD128");
    Some(path)
}

fn drm_link(node: u64) -> Option<Vec<u8>> {
    let gpu = gpu()?;
    Some(match node {
        CHAR_RENDER | CLASS_RENDER => {
            let mut target = b"../../".to_vec();
            target.extend_from_slice(&gpu_path()?);
            target
        }
        PARENT_SUBSYSTEM => match gpu {
            GpuLocation::Pci { .. } => b"../../../bus/pci".to_vec(),
            GpuLocation::Platform { .. } => b"../../../bus/platform".to_vec(),
        },
        VIRTIO_SUBSYSTEM => b"../../../../bus/virtio".to_vec(),
        RENDER_SUBSYSTEM => b"../../../../../../class/drm".to_vec(),
        RENDER_DEVICE => b"../..".to_vec(),
        _ => return None,
    })
}

fn drm_text(node: u64) -> Option<Vec<u8>> {
    let gpu = gpu()?;
    let (major, minor) = RENDER_NODE;
    Some(match (node, gpu) {
        (RENDER_DEV, _) => alloc::format!("{major}:{minor}\n").into_bytes(),
        (RENDER_UEVENT, _) => alloc::format!(
            "MAJOR={major}\nMINOR={minor}\nDEVNAME=dri/renderD{minor}\nDEVTYPE=drm_minor\n"
        )
        .into_bytes(),
        (VIRTIO_UEVENT, _) => b"DRIVER=virtio_gpu\nMODALIAS=virtio:d00000010v00001AF4\n".to_vec(),
        (PARENT_UEVENT, GpuLocation::Pci { bus, device, function, vendor, device_id, subsystem_vendor, subsystem_device, class, .. }) => {
            alloc::format!(
                "DRIVER=virtio-pci\nPCI_CLASS={class:X}\nPCI_ID={vendor:04X}:{device_id:04X}\nPCI_SUBSYS_ID={subsystem_vendor:04X}:{subsystem_device:04X}\nPCI_SLOT_NAME=0000:{bus:02x}:{device:02x}.{function:x}\n"
            )
            .into_bytes()
        }
        (PARENT_UEVENT, GpuLocation::Platform { .. }) => {
            b"DRIVER=virtio-mmio\nOF_COMPATIBLE_0=virtio,mmio\nOF_COMPATIBLE_N=1\n".to_vec()
        }
        (PARENT_VENDOR, GpuLocation::Pci { vendor, .. }) => alloc::format!("0x{vendor:04x}\n").into_bytes(),
        (PARENT_DEVICE, GpuLocation::Pci { device_id, .. }) => alloc::format!("0x{device_id:04x}\n").into_bytes(),
        (PARENT_SUBSYSTEM_VENDOR, GpuLocation::Pci { subsystem_vendor, .. }) => {
            alloc::format!("0x{subsystem_vendor:04x}\n").into_bytes()
        }
        (PARENT_SUBSYSTEM_DEVICE, GpuLocation::Pci { subsystem_device, .. }) => {
            alloc::format!("0x{subsystem_device:04x}\n").into_bytes()
        }
        (PARENT_REVISION, GpuLocation::Pci { revision, .. }) => alloc::format!("0x{revision:02x}\n").into_bytes(),
        (PARENT_CLASS, GpuLocation::Pci { class, .. }) => alloc::format!("0x{class:06x}\n").into_bytes(),
        _ => return None,
    })
}

/// The device number `stat` reports for its files.
const DEVICE: (u32, u32) = (0, 21);

/// `/sys`, mounted at boot.
#[derive(Debug)]
pub struct SystemFiles;

fn is_directory(node: u64) -> bool {
    matches!(node, ROOT | DEVICES | SYSTEM | CPU | KERNEL | MM | TRANSPARENT_HUGEPAGE)
        || cpu_node(node).is_some_and(|(_, file)| !file)
}

/// The first per-CPU node: `cpu<n>` is `PER_CPU + 2n`, its `online` file the next (D596).
const PER_CPU: u64 = 1000;

/// What takes a CPU out of service (`false`) or puts it back (`true`), by index (D596).
static CPU_SERVICE: LeafSpinLock<Option<fn(usize, bool) -> Result<(), &'static str>>> =
    LeafSpinLock::new_leaf(None);

/// Lets `cpu<n>/online` take CPUs out of service and back through `service` (D596).
pub fn set_cpu_service(service: fn(usize, bool) -> Result<(), &'static str>) {
    *CPU_SERVICE.lock() = Some(service);
}

/// The CPU index of a per-CPU node and whether it is the `online` file; the boot CPU, index 0,
/// has no such directory, as on x86 Linux.
fn cpu_node(node: u64) -> Option<(usize, bool)> {
    let offset = node.checked_sub(PER_CPU)?;
    let index = usize::try_from(offset / 2).ok()?;
    (index >= 1 && index < crate::cpu::CPUS.all().len()).then_some((index, offset % 2 == 1))
}

fn cpu_directory_name(index: usize) -> Vec<u8> {
    alloc::format!("cpu{index}").into_bytes()
}

/// The CPUs in service as Linux writes a CPU list: `0-3,5`.
fn online_list() -> Vec<u8> {
    let cpus = crate::cpu::CPUS.all();
    let mut text = alloc::string::String::new();
    let mut index = 0;
    while index < cpus.len().max(1) {
        let online = cpus.get(index).is_none_or(|cpu| cpu.is_online());
        if !online {
            index += 1;
            continue;
        }
        let start = index;
        while cpus.get(index + 1).is_some_and(|cpu| cpu.is_online()) {
            index += 1;
        }
        if !text.is_empty() {
            text.push(',');
        }
        match start == index {
            true => text.push_str(&alloc::format!("{start}")),
            false => text.push_str(&alloc::format!("{start}-{index}")),
        }
        index += 1;
    }
    text.push('\n');
    text.into_bytes()
}

/// The CPUs `0..count` as Linux writes a CPU list: `0`, or `0-<last>`.
fn cpu_range(count: usize) -> Vec<u8> {
    let text = match count {
        0 | 1 => alloc::string::String::from("0\n"),
        many => alloc::format!("0-{}\n", many - 1),
    };
    text.into_bytes()
}

impl FileSystem for SystemFiles {
    fn root(&self) -> u64 {
        ROOT
    }

    fn device(&self) -> (u32, u32) {
        DEVICE
    }

    fn usage(&self) -> Usage {
        Usage::without_blocks(super::magic::SYSFS, true)
    }

    fn kind(&self, node: u64) -> NodeKind {
        if let Some((_, _, kind)) = drm_node(node) {
            return kind;
        }
        match is_directory(node) {
            true => NodeKind::Directory,
            false => NodeKind::Regular,
        }
    }

    fn attributes(&self, node: u64) -> Attributes {
        let kind = self.kind(node);
        let writable = cpu_node(node).is_some_and(|(_, file)| file);
        Attributes {
            kind,
            permissions: match kind {
                NodeKind::Directory => 0o555,
                NodeKind::SymbolicLink => 0o777,
                _ if writable => 0o644,
                _ => 0o444,
            },
            file_id: node,
            links: if kind == NodeKind::Directory { 2 } else { 1 },
            owner: 0,
            group: 0,
            size: 4096,
            modified: 0,
        }
    }

    fn lookup(&self, directory: u64, name: &[u8]) -> Option<u64> {
        if directory == CPU
            && let Some(index) = name
                .strip_prefix(b"cpu")
                .and_then(|digits| core::str::from_utf8(digits).ok())
                .and_then(|digits| digits.parse::<u64>().ok())
            && cpu_node(PER_CPU + 2 * index).is_some()
        {
            return Some(PER_CPU + 2 * index);
        }
        if let Some((index, false)) = cpu_node(directory) {
            return (name == b"online").then_some(PER_CPU + 2 * index as u64 + 1);
        }
        NODES
            .iter()
            .find(|&&(_, file, parent)| parent == directory && file == name)
            .map(|&(node, _, _)| node)
            .or_else(|| {
                DRM_NODES.iter().map(|&(node, ..)| node).find(|&node| {
                    drm_node(node).is_some_and(|(drm, parent, _)| parent == directory && drm == name)
                })
            })
    }

    fn parent(&self, node: u64) -> u64 {
        if let Some((_, parent, _)) = drm_node(node) {
            return parent;
        }
        if let Some((index, file)) = cpu_node(node) {
            return match file {
                true => PER_CPU + 2 * index as u64,
                false => CPU,
            };
        }
        NODES
            .iter()
            .find(|&&(file, _, _)| file == node)
            .map_or(ROOT, |&(_, _, parent)| parent)
    }

    fn name(&self, node: u64) -> Name {
        if let Some((name, _, _)) = drm_node(node) {
            return Name::new(&name);
        }
        if let Some((index, file)) = cpu_node(node) {
            return match file {
                true => Name::new(b"online"),
                false => Name::new(&cpu_directory_name(index)),
            };
        }
        Name::new(
            NODES
                .iter()
                .find(|&&(file, _, _)| file == node)
                .map_or(&[][..], |&(_, name, _)| name),
        )
    }

    fn link_target(&self, node: u64, _caller: u32) -> Option<Vec<u8>> {
        drm_link(node)
    }

    fn read(&self, _node: u64, _offset: u64, _length: usize) -> &[u8] {
        &[]
    }

    fn generate(&self, node: u64) -> Result<Option<Vec<u8>>, super::StorageError> {
        let cpus = crate::cpu::CPUS.all();
        Ok(Some(match node {
            ONLINE => online_list(),
            POSSIBLE | PRESENT => cpu_range(cpus.len()),
            ENABLED => b"always madvise [never]\n".to_vec(),
            DEFRAG => b"always defer defer+madvise madvise [never]\n".to_vec(),
            node => match (cpu_node(node), drm_text(node)) {
                (Some((index, true)), _) => {
                    match cpus.get(index).is_some_and(|cpu| cpu.is_online()) {
                        true => b"1\n".to_vec(),
                        false => b"0\n".to_vec(),
                    }
                }
                (_, Some(text)) => text,
                _ => return Ok(None),
            },
        }))
    }

    fn is_node_writable(&self, node: u64) -> bool {
        cpu_node(node).is_some_and(|(_, file)| file)
    }

    fn write(
        &self,
        node: u64,
        _at: super::WriteAt,
        bytes: &[u8],
        _now: u64,
    ) -> Result<(usize, u64), super::Refused> {
        let Some((index, true)) = cpu_node(node) else {
            return Err(super::Refused::ReadOnly);
        };
        let online = match bytes.trim_ascii() {
            b"1" => true,
            b"0" => false,
            _ => return Err(super::Refused::InvalidArgument),
        };
        let service = (*CPU_SERVICE.lock()).ok_or(super::Refused::ReadOnly)?;
        service(index, online).map_err(|_| super::Refused::InvalidArgument)?;
        Ok((bytes.len(), bytes.len() as u64))
    }

    fn entry(&self, directory: u64, position: u64) -> Option<DirectoryEntry> {
        let position = usize::try_from(position).ok()?;
        if let Some((index, false)) = cpu_node(directory) {
            return (position == 0).then(|| DirectoryEntry {
                name: Name::new(b"online"),
                node: PER_CPU + 2 * index as u64 + 1,
                kind: NodeKind::Regular,
            });
        }
        if directory == CPU {
            let fixed = NODES.iter().filter(|&&(_, _, parent)| parent == CPU).count();
            if let Some(index) = position
                .checked_sub(fixed)
                .map(|extra| extra + 1)
                .filter(|&index| index < crate::cpu::CPUS.all().len())
            {
                return Some(DirectoryEntry {
                    name: Name::new(&cpu_directory_name(index)),
                    node: PER_CPU + 2 * index as u64,
                    kind: NodeKind::Directory,
                });
            }
        }
        let fixed = NODES.iter().filter(|&&(_, _, parent)| parent == directory);
        let fixed_count = fixed.clone().count();
        if let Some(&(node, name, _)) = fixed.clone().nth(position) {
            return Some(DirectoryEntry {
                name: Name::new(name),
                node,
                kind: self.kind(node),
            });
        }
        let (node, (name, _, kind)) = DRM_NODES
            .iter()
            .filter_map(|&(node, ..)| drm_node(node).map(|found| (node, found)))
            .filter(|(_, (_, parent, _))| *parent == directory)
            .nth(position - fixed_count)?;
        Some(DirectoryEntry {
            name: Name::new(&name),
            node,
            kind,
        })
    }
}
